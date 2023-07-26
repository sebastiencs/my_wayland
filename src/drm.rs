use std::{
    collections::{BTreeMap, BTreeSet, HashSet},
    os::fd::{AsFd, FromRawFd, OwnedFd, RawFd},
};

use drm::{
    buffer::DrmFourcc,
    control::{
        self, connector, crtc, encoder, plane,
        property::{self, ValueType},
        Device as CT, ModeTypeFlags, PlaneType, ResourceHandles,
    },
    Device,
};

const KNOWN_FORMATS: &[DrmFourcc] = &[DrmFourcc::Argb8888, DrmFourcc::Xrgb8888];

#[derive(Debug)]
pub struct Drm {
    fd: DrmFd,
    device: udev::Device,
    planes: BTreeMap<plane::Handle, Plane>,
    crtcs: BTreeMap<crtc::Handle, Crtc>,
    encoders: BTreeMap<encoder::Handle, Encoder>,
    connectors: BTreeMap<connector::Handle, Connector>,
    cursor_height: u64,
    cursor_width: u64,
    resources: ResourceHandles,
    driver: Driver,
}

#[derive(Debug)]
pub struct DrmFd {
    fd: OwnedFd,
}

impl AsFd for DrmFd {
    fn as_fd(&self) -> std::os::fd::BorrowedFd<'_> {
        self.fd.as_fd()
    }
}

impl drm::Device for DrmFd {}
impl drm::control::Device for DrmFd {}

fn get_props<T: control::ResourceHandle>(
    drm: &DrmFd,
    plane: T,
) -> BTreeMap<String, (u64, property::Info)> {
    drm.get_properties(plane)
        .unwrap()
        .into_iter()
        .map(|(prop, value)| {
            let prop = drm.get_property(prop).unwrap();
            let name = prop.name().to_str().expect("invalid name");
            (name.to_string(), (value, prop))
        })
        .collect()
}

struct Geometry {
    pub x: u64,
    pub y: u64,
    pub width: u64,
    pub height: u64,
}

impl std::fmt::Debug for Geometry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let Self { x, y, width, height } = self;
        f.write_fmt(format_args!(
            "Geometry {{ x: {:?}, y: {:?}, width: {:?}, height: {:?} }}",
            x, y, width, height
        ))
    }
}

#[derive(Debug)]
struct Plane {
    pub handle: plane::Handle,
    pub kind: PlaneType,
    pub crtc: Geometry,
    pub src: Geometry,
    pub formats: HashSet<DrmFourcc>,
    pub possible_crtcs: u32,
    pub crtc_handle: Option<control::crtc::Handle>,
    pub fb_handle: Option<control::framebuffer::Handle>,
}

#[derive(Debug)]
struct Crtc {
    handle: crtc::Handle,
    index: usize,
    possible_planes: HashSet<plane::Handle>,
    active: bool,
    mode_id: u32,
}

#[derive(Debug)]
struct Encoder {
    handle: encoder::Handle,
    possible_crtcs: HashSet<crtc::Handle>,
}

#[derive(Debug)]
struct Connector {
    handler: connector::Handle,
    edid: Option<edid_rs::EDID>,
    crtcs: BTreeSet<crtc::Handle>,
    preferred_mode: Option<control::Mode>,
    state: connector::State,
    // status:
    // props:
}

#[derive(Debug)]
pub struct Driver {
    pub name: String,
    pub date: String,
    pub desc: String,
}

impl From<drm::Driver> for Driver {
    fn from(value: drm::Driver) -> Self {
        let drm::Driver { name, date, desc } = value;
        Self {
            name: name.to_string_lossy().to_string(),
            date: date.to_string_lossy().to_string(),
            desc: desc.to_string_lossy().to_string(),
        }
    }
}

impl Drm {
    pub fn create_drm(fd: RawFd, device: udev::Device) -> Self {
        let fd = unsafe { OwnedFd::from_raw_fd(fd) };
        let drm = DrmFd { fd };

        drm.set_client_capability(drm::ClientCapability::Atomic, true)
            .unwrap();
        let resources = drm.resource_handles().unwrap();

        let (cursor_height, cursor_width) = {
            use drm::DriverCapability::{CursorHeight, CursorWidth};

            let height = drm.get_driver_capability(CursorHeight).unwrap_or(64);
            let width = drm.get_driver_capability(CursorWidth).unwrap_or(64);
            (height, width)
        };

        let driver = drm.get_driver().unwrap().into();

        let planes: BTreeMap<plane::Handle, Plane> = drm
            .plane_handles()
            .unwrap()
            .iter()
            .map(|plane| Self::make_plane(&drm, *plane))
            .collect();

        let crtcs: BTreeMap<crtc::Handle, Crtc> = resources
            .crtcs()
            .iter()
            .enumerate()
            .map(|(index, crtc)| Self::make_crtc(&drm, index, *crtc, &planes))
            .collect();

        let encoders: BTreeMap<encoder::Handle, Encoder> = resources
            .encoders()
            .iter()
            .map(|encoder| Self::make_encoder(&drm, *encoder, &crtcs))
            .collect();

        let connectors: BTreeMap<connector::Handle, Connector> = resources
            .connectors()
            .iter()
            .map(|connector| Self::make_connector(&drm, *connector, &encoders))
            .collect();

        Self {
            fd: drm,
            device,
            planes,
            crtcs,
            encoders,
            cursor_height,
            cursor_width,
            resources,
            driver,
            connectors,
        }
    }

    fn make_crtc(
        drm: &DrmFd,
        index: usize,
        crtc: crtc::Handle,
        planes: &BTreeMap<plane::Handle, Plane>,
    ) -> (crtc::Handle, Crtc) {
        let possible_planes: HashSet<_> = planes
            .iter()
            .filter(|(_, plane)| plane.possible_crtcs & (1 << index) != 0)
            .map(|(handle, _)| *handle)
            .collect();

        let props = get_props(drm, crtc);

        let handle = crtc;
        let crtc = Crtc {
            handle,
            index,
            possible_planes,
            active: props.get("ACTIVE").map(|(v, _)| *v != 0).unwrap_or(false),
            mode_id: props.get("MODE_ID").map(|(v, _)| *v as u32).unwrap(),
        };

        (handle, crtc)
    }

    fn make_encoder(
        drm: &DrmFd,
        encoder: encoder::Handle,
        crtcs: &BTreeMap<crtc::Handle, Crtc>,
    ) -> (encoder::Handle, Encoder) {
        let info = drm.get_encoder(encoder).unwrap();
        let possible_crtcs: HashSet<_> = crtcs
            .iter()
            .filter(|(_, crtc)| info.possible_crtcs().0 & (1 << crtc.index) != 0)
            .map(|(handle, _)| *handle)
            .collect();

        let handle = encoder;
        let encoder = Encoder { handle, possible_crtcs };
        (handle, encoder)
    }

    fn make_plane(drm: &DrmFd, plane: plane::Handle) -> (plane::Handle, Plane) {
        let get_kind = |props: &BTreeMap<String, (u64, property::Info)>| {
            let (value, prop) = props.get("type").expect("plane missing `type`");

            let ValueType::Enum(values) = prop.value_type() else {
                return None;
            };

            values
                .values()
                .1
                .iter()
                .find(|v| v.value() == *value)
                .and_then(|value| match value.name().to_bytes() {
                    b"Primary" => Some(PlaneType::Primary),
                    b"Cursor" => Some(PlaneType::Cursor),
                    b"Overlay" => Some(PlaneType::Overlay),
                    _ => None,
                })
        };

        let info = drm.get_plane(plane).unwrap();
        let props = get_props(drm, plane);

        let formats: HashSet<DrmFourcc> = info
            .formats()
            .iter()
            .filter_map(|f| DrmFourcc::try_from(*f).ok())
            .filter(|f| KNOWN_FORMATS.contains(f))
            .collect();

        let get_geometry = |x: &str, y: &str, w: &str, h: &str| {
            let first = |&(a, _)| a;
            Geometry {
                x: props.get(x).map(first).unwrap(),
                y: props.get(y).map(first).unwrap(),
                width: props.get(w).map(first).unwrap(),
                height: props.get(h).map(first).unwrap(),
            }
        };

        let kind = get_kind(&props).unwrap();
        let crtc_geometry = get_geometry("CRTC_X", "CRTC_Y", "CRTC_W", "CRTC_H");
        let src_geometry = get_geometry("SRC_X", "SRC_Y", "SRC_W", "SRC_H");
        let possible_crtcs = info.possible_crtcs().0;

        let handle = plane;
        let plane = Plane {
            handle,
            kind,
            crtc: crtc_geometry,
            src: src_geometry,
            formats,
            possible_crtcs,
            crtc_handle: info.crtc(),
            fb_handle: info.framebuffer(),
        };

        (handle, plane)
    }

    // fn make_plane(drm: &DrmFd, plane: plane::Handle) -> (plane::Handle, Plane) {
    fn make_connector(
        drm: &DrmFd,
        connector_handle: connector::Handle,
        encoders: &BTreeMap<encoder::Handle, Encoder>,
    ) -> (connector::Handle, Connector) {
        let connector = drm.get_connector(connector_handle, true).unwrap();

        let props = get_props(drm, connector_handle);

        let edid = props
            .get("EDID")
            .and_then(|(value, _)| drm.get_property_blob(*value).ok())
            .map(std::io::Cursor::new)
            .and_then(|mut blob| edid_rs::parse(&mut blob).ok());

        let crtcs: BTreeSet<_> = connector
            .encoders()
            .iter()
            .filter_map(|encoder| encoders.get(encoder))
            .flat_map(|encoder| encoder.possible_crtcs.iter().copied())
            .collect();

        let preferred_mode = connector
            .modes()
            .iter()
            .find(|mode| mode.mode_type().contains(ModeTypeFlags::PREFERRED))
            .or_else(|| connector.modes().first())
            .cloned();

        let state = connector.state();

        let connector = Connector {
            handler: connector_handle,
            edid,
            crtcs,
            preferred_mode,
            state,
        };

        (connector_handle, connector)
    }
}
