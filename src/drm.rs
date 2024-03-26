use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd, RawFd},
};

use {
    drm::{
        buffer::DrmFourcc,
        control::{
            self, connector, crtc, encoder, plane,
            property::{self, ValueType},
            Device as CT, ModeTypeFlags, PlaneType, ResourceHandles,
        },
        Device,
    },
    once_cell::sync::Lazy,
};

use crate::egl::{self, EglContext, EglInstance};

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct Format {
    pub name: &'static str,
    pub bpp: u32,
    pub drm: DrmFourcc,
    pub external_only_guess: bool,
    pub has_alpha: bool,
}

pub static ARGB8888: &Format = &Format {
    name: "argb8888",
    bpp: 4,
    drm: DrmFourcc::Argb8888,
    external_only_guess: false,
    has_alpha: true,
};

pub static XRGB8888: &Format = &Format {
    name: "xrgb8888",
    bpp: 4,
    drm: DrmFourcc::Xrgb8888,
    external_only_guess: false,
    has_alpha: false,
};

pub static FORMATS: &[Format] = &[*ARGB8888, *XRGB8888];

pub static FORMATS_MAP: Lazy<HashMap<DrmFourcc, &'static Format>> =
    Lazy::new(|| FORMATS.iter().map(|format| (format.drm, format)).collect());

pub const KNOWN_FORMATS: &[DrmFourcc] = &[DrmFourcc::Argb8888, DrmFourcc::Xrgb8888];

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
    egl_context: EglContext,
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

impl AsRawFd for DrmFd {
    fn as_raw_fd(&self) -> RawFd {
        self.fd.as_raw_fd()
    }
}

impl DrmFd {
    pub fn try_create_lease(&self) -> std::io::Result<DrmFd> {
        const DRM_IOCTL_MODE_CREATE_LEASE: usize = 0xC6;
        nix::ioctl_readwrite!(
            create_lease,
            b'd',
            DRM_IOCTL_MODE_CREATE_LEASE,
            drm_ffi::drm_mode_create_lease
        );

        let mut params = drm_ffi::drm_mode_create_lease {
            object_ids: 0,
            object_count: 0,
            flags: libc::O_CLOEXEC as u32,
            lessee_id: 0,
            fd: 0,
        };
        let fd = self.fd.as_raw_fd();

        let lease = unsafe {
            create_lease(fd, &mut params)?;
            OwnedFd::from_raw_fd(params.fd as i32)
        };

        Ok(DrmFd { fd: lease })
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

#[derive(Clone)]
struct Property<T> {
    value: T,
    handle: control::property::Handle,
}

impl<T> Property<T> {
    pub fn value(&self) -> &T {
        &self.value
    }
}

#[derive(Clone)]
struct Geometry {
    pub x: Property<u64>,
    pub y: Property<u64>,
    pub width: Property<u64>,
    pub height: Property<u64>,
}

impl std::fmt::Debug for Geometry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let Self { x, y, width, height } = self;
        f.write_fmt(format_args!(
            "Geometry {{ x: {:?}, y: {:?}, width: {:?}, height: {:?} }}",
            x.value(),
            y.value(),
            width.value(),
            height.value()
        ))
    }
}

#[derive(Clone, Debug)]
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

impl Plane {
    pub fn fb_handle(&self) -> Option<control::property::Handle> {
        let handle = self.fb_handle?;
        let handle: control::RawResourceHandle = handle.into();
        let handle: control::property::Handle = handle.into();
        Some(handle)
    }
}

#[derive(Clone, Debug)]
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

#[derive(Clone, Debug)]
pub struct Connector {
    pub handler: connector::Handle,
    pub edid: Option<edid_rs::EDID>,
    pub crtcs: BTreeSet<crtc::Handle>,
    pub preferred_mode: Option<control::Mode>,
    pub state: connector::State,
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

#[derive(Debug)]
struct ConnectorInUse {
    crtc: Crtc,
    connector: Connector,
    planes: Vec<Plane>,
}

impl Drm {
    pub fn create_drm(fd: RawFd, device: udev::Device, egl: &EglInstance) -> Self {
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

        for c in connectors.values() {
            let connector::State::Connected = c.state else {
                continue;
            };

            for crtc in &c.crtcs {
                let crtc = crtcs.get(crtc).unwrap();
                if !crtc.active {
                    continue;
                }
                // let planes = crtc.possible_planes.clone();
                eprintln!("connector {:?} crtc={:?}", c, crtc);
                // eprintln!("connector {:?} crtc={:?}", c.handler, crtc);
                // eprintln!("connector {:?} crtc={:?} planes={:?}", c.handler, crtc, planes);
            }
        }

        let gbm = crate::gbm::GbmDevice::new(&drm).unwrap();

        let egl_context = EglContext::make(egl, gbm);

        // let in_use = Self::find_first_valid_connector(&connectors, &crtcs, &planes)
        //     .expect("Failed to find connector");

        let in_use = Self::init_drm(&drm, &connectors, &crtcs, &planes);

        // control::atomic::AtomicModeReq;
        // drm::
        // drm.atomic_commit(flags, req);

        // egl::make_display(&drm, egl, &gbm, connector, *crtc);

        // egl.get_platform_display(EGL_PLATFORM_GBM_KHR, native_display, attrib_list);

        // egl = eglGetPlatformDisplay(EGL_PLATFORM_GBM_KHR, dpy.gbm, NULL);

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
            egl_context,
        }
    }

    fn init_drm(
        drm: &DrmFd,
        connectors: &BTreeMap<connector::Handle, Connector>,
        crtcs: &BTreeMap<crtc::Handle, Crtc>,
        planes: &BTreeMap<plane::Handle, Plane>,
    ) {
        let in_use = Self::find_first_valid_connector(&connectors, &crtcs, &planes)
            .expect("Failed to find connector");

        let primary_plane = in_use
            .planes
            .iter()
            .find(|p| matches!(p.kind, PlaneType::Primary))
            .unwrap();

        let mut changes = control::atomic::AtomicModeReq::new();

        dbg!(primary_plane.handle);
        dbg!(primary_plane.fb_handle());

        changes.add_property(
            primary_plane.handle,
            primary_plane.fb_handle().unwrap(),
            control::property::Value::Bitmask(0),
        );

        // changes.change_object(primary_plane.id, |c| {
        //     c.change(primary_plane.fb_id, buffers[0].drm.id().0 as _);
        //     c.change(primary_plane.crtc_id.id, crtc.id.0 as _);
        //     c.change(primary_plane.crtc_x.id, 0);
        //     c.change(primary_plane.crtc_y.id, 0);
        //     c.change(primary_plane.crtc_w.id, mode.hdisplay as _);
        //     c.change(primary_plane.crtc_h.id, mode.vdisplay as _);
        //     c.change(primary_plane.src_x.id, 0);
        //     c.change(primary_plane.src_y.id, 0);
        //     c.change(primary_plane.src_w.id, (mode.hdisplay as u64) << 16);
        //     c.change(primary_plane.src_h.id, (mode.vdisplay as u64) << 16);
        // });

        dbg!(in_use);
    }

    // TODO: Support multiple monitors/connectors
    fn find_first_valid_connector(
        connectors: &BTreeMap<connector::Handle, Connector>,
        crtcs: &BTreeMap<crtc::Handle, Crtc>,
        planes: &BTreeMap<plane::Handle, Plane>,
    ) -> Option<ConnectorInUse> {
        let (connector, crtc) = (|| {
            for connector in connectors.values() {
                let connector::State::Connected = connector.state else {
                    continue;
                };
                for crtc in &connector.crtcs {
                    let crtc = crtcs.get(crtc).unwrap();
                    if !crtc.active {
                        continue;
                    }
                    return Some((connector, crtc));
                }
            }
            None
        })()?;

        let planes = crtc
            .possible_planes
            .iter()
            .map(|handle| planes.get(handle).cloned().unwrap())
            .collect::<Vec<_>>();

        Some(ConnectorInUse { crtc: crtc.clone(), connector: connector.clone(), planes })
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
            let get_prop = |(value, info): &(u64, property::Info)| Property {
                value: *value,
                handle: info.handle(),
            };
            Geometry {
                x: props.get(x).map(get_prop).unwrap(),
                y: props.get(y).map(get_prop).unwrap(),
                width: props.get(w).map(get_prop).unwrap(),
                height: props.get(h).map(get_prop).unwrap(),
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
