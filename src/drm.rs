use std::{
    collections::{BTreeMap, HashMap, HashSet},
    os::fd::{AsFd, FromRawFd, OwnedFd, RawFd},
};

use drm::{
    buffer::DrmFourcc,
    control::{
        self, crtc, encoder, plane,
        property::{self, ValueType},
        Device as CT, PlaneType, ResourceHandles,
    },
    Device,
};

const KNOWN_FORMATS: &[DrmFourcc] = &[DrmFourcc::Argb8888, DrmFourcc::Xrgb8888];

pub struct Drm {
    fd: DrmFd,
    device: udev::Device,
    planes: HashMap<plane::Handle, Plane>,
    crtcs: HashMap<crtc::Handle, Crtc>,
    encoders: HashMap<encoder::Handle, Encoder>,
    cursor_height: u64,
    cursor_width: u64,
    resources: ResourceHandles,
}

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
    let mut container = BTreeMap::default();

    let props = drm.get_properties(plane).unwrap();
    for (prop, value) in props {
        let prop = drm.get_property(prop).unwrap();
        let name = prop.name().to_str().expect("invalid name");
        container.insert(name.to_string(), (value, prop));
    }

    container
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

impl Drm {
    pub fn create_drm(fd: RawFd, device: udev::Device) -> Self {
        let fd = unsafe { OwnedFd::from_raw_fd(fd) };
        let drm = DrmFd { fd };

        drm.set_client_capability(drm::ClientCapability::Atomic, true)
            .unwrap();
        let resources = drm.resource_handles().unwrap();

        let (cursor_height, cursor_width) = {
            let height = drm
                .get_driver_capability(drm::DriverCapability::CursorHeight)
                .unwrap();
            let width = drm
                .get_driver_capability(drm::DriverCapability::CursorWidth)
                .unwrap();
            (height, width)
        };

        dbg!(drm.get_bus_id());
        dbg!(drm.get_driver());

        let planes: HashMap<plane::Handle, Plane> = drm
            .plane_handles()
            .unwrap()
            .iter()
            .map(|plane| Self::make_plane(&drm, *plane))
            .collect();

        let crtcs: HashMap<crtc::Handle, Crtc> = resources
            .crtcs()
            .iter()
            .enumerate()
            .map(|(index, crtc)| Self::make_crtc(&drm, index, *crtc, &planes))
            .collect();

        let encoders: HashMap<encoder::Handle, Encoder> = resources
            .encoders()
            .iter()
            .map(|encoder| Self::make_encoder(&drm, *encoder, &crtcs))
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
        }
    }

    fn make_crtc(
        drm: &DrmFd,
        index: usize,
        crtc: crtc::Handle,
        planes: &HashMap<plane::Handle, Plane>,
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
        crtcs: &HashMap<crtc::Handle, Crtc>,
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
}
