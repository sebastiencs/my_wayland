use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    num::NonZeroU32,
    os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd, RawFd},
};

use drm::control::FbCmd2Flags;

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

use crate::{
    dmabuf::DmaBuf,
    egl::{EglContext, EglFrameBuffer, GlTexture, EGL},
    gbm::UseFlags,
    io_uring::ring::ReadWriteCompleted,
    protocol_generated::WaylandId,
    runner::{AnyAction, Dispatcher, Model},
};

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct Format {
    pub name: &'static str,
    pub bpp: u32,
    pub drm: DrmFourcc,
    pub external_only_guess: bool,
    pub has_alpha: bool,
    pub wayland_id: u32,
}

pub static ARGB8888: &Format = &Format {
    name: "argb8888",
    bpp: 4,
    drm: DrmFourcc::Argb8888,
    external_only_guess: false,
    has_alpha: true,
    wayland_id: 0,
};

pub static XRGB8888: &Format = &Format {
    name: "xrgb8888",
    bpp: 4,
    drm: DrmFourcc::Xrgb8888,
    external_only_guess: false,
    has_alpha: false,
    wayland_id: 1,
};

pub static FORMATS: &[Format] = &[*ARGB8888, *XRGB8888];

pub static FORMATS_MAP: Lazy<HashMap<DrmFourcc, &'static Format>> =
    Lazy::new(|| FORMATS.iter().map(|format| (format.drm, format)).collect());

pub const KNOWN_FORMATS: &[DrmFourcc] = &[DrmFourcc::Argb8888, DrmFourcc::Xrgb8888];

#[derive(Debug)]
struct DrmFrameBuffer {
    fd: OwnedFd,
}

#[derive(Debug)]
pub struct DrmResources {
    planes: BTreeMap<plane::Handle, Plane>,
    crtcs: BTreeMap<crtc::Handle, Crtc>,
    encoders: BTreeMap<encoder::Handle, Encoder>,
    connectors: BTreeMap<connector::Handle, Connector>,
    cursor_height: u64,
    cursor_width: u64,
    resources: ResourceHandles,
}

#[derive(Debug)]
pub struct Drm {
    fd: DrmFd,
    device: udev::Device,
    resources: DrmResources,
    egl_context: EglContext,
    in_use: ConnectorInUse,
    buffers: [RenderBuffer; 2],
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
            drm_ffi::DRM_IOCTL_BASE,
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
    pub formats: HashMap<DrmFourcc, PlaneFormat>,
    pub possible_crtcs: u32,
    pub crtc_handle: Option<control::crtc::Handle>,
    pub fb_id: drm::control::property::Handle,
    pub crtc_id: drm::control::property::Handle,
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
    primary_plane_index: usize,
}

impl ConnectorInUse {
    fn primary_plane(&self) -> (&Plane, &BTreeSet<Modifier>) {
        let plane = &self.planes[self.primary_plane_index];
        let format = &plane.formats[&DrmFourcc::Xrgb8888];
        (plane, &format.modifiers)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Copy)]
pub struct Modifier(pub u64);

impl Modifier {
    pub const INVALID: Self = Self(0x00ff_ffff_ffff_ffff);
    pub const LINEAR: Self = Self(0);
    pub const ZERO: Self = Self(0);

    pub fn as_u64(&self) -> u64 {
        self.0
    }

    pub fn low(&self) -> u32 {
        (self.0 & 0xFFFF_FFFF) as u32
    }

    pub fn high(&self) -> u32 {
        (self.0 >> 32) as u32
    }
}

#[derive(Clone, Debug)]
pub struct PlaneFormat {
    format: Format,
    modifiers: BTreeSet<Modifier>,
}

#[derive(Debug)]
pub struct InFormat {
    format: DrmFourcc,
    modifiers: BTreeSet<Modifier>,
}

impl Drm {
    pub fn create_drm(fd: RawFd, device: udev::Device, egl: &EGL) -> Self {
        let fd = unsafe { OwnedFd::from_raw_fd(fd) };
        let drm = DrmFd { fd };
        drm.set_client_capability(drm::ClientCapability::Atomic, true)
            .unwrap();

        let driver = drm.get_driver().unwrap().into();
        let resources = Self::list_resources(&drm);

        let gbm = crate::gbm::GbmDevice::new(&drm).unwrap();
        let egl_context = EglContext::make(egl, gbm);
        let (in_use, buffers) = Self::init_drm(&drm, &egl_context, &resources);

        Self {
            fd: drm,
            device,
            resources,
            egl_context,
            driver,
            in_use,
            buffers,
        }
    }

    pub fn make_model(&self) -> DrmModel {
        DrmModel { fd: self.fd.as_raw_fd() }
    }

    fn list_resources(drm: &DrmFd) -> DrmResources {
        let resources = drm.resource_handles().unwrap();

        let (cursor_height, cursor_width) = {
            use drm::DriverCapability::{CursorHeight, CursorWidth};

            let height = drm.get_driver_capability(CursorHeight).unwrap_or(64);
            let width = drm.get_driver_capability(CursorWidth).unwrap_or(64);
            (height, width)
        };

        let make_plane = |plane: plane::Handle| Self::make_plane(drm, plane);
        let planes: BTreeMap<plane::Handle, Plane> = drm
            .plane_handles()
            .unwrap()
            .iter()
            .copied()
            .map(make_plane)
            .collect();

        let make_crtc = |(index, crtc)| Self::make_crtc(drm, index, crtc, &planes);
        let crtcs: BTreeMap<crtc::Handle, Crtc> = resources
            .crtcs()
            .iter()
            .copied()
            .enumerate()
            .map(make_crtc)
            .collect();

        let make_encoder = |encoder| Self::make_encoder(drm, encoder, &crtcs);
        let encoders: BTreeMap<encoder::Handle, Encoder> = resources
            .encoders()
            .iter()
            .copied()
            .map(make_encoder)
            .collect();

        let make_connector = |connector| Self::make_connector(drm, connector, &encoders);
        let connectors: BTreeMap<connector::Handle, Connector> = resources
            .connectors()
            .iter()
            .copied()
            .map(make_connector)
            .collect();

        DrmResources {
            planes,
            crtcs,
            encoders,
            connectors,
            cursor_height,
            cursor_width,
            resources,
        }
    }

    fn init_drm(
        drm: &DrmFd,
        egl: &EglContext,
        drm_resources: &DrmResources,
    ) -> (ConnectorInUse, [RenderBuffer; 2]) {
        let DrmResources { planes, crtcs, connectors, .. } = drm_resources;

        let in_use = Self::find_first_valid_connector(&connectors, &crtcs, &planes)
            .expect("Failed to find connector");

        let mut changes = control::atomic::AtomicModeReq::new();

        let buffers: [_; 2] =
            std::array::from_fn(|_| Self::make_render_buffer(drm, egl, &in_use, false).unwrap());

        dbg!(&buffers);

        let mode = in_use.connector.preferred_mode.as_ref().unwrap();

        let (primary_plane, _) = in_use.primary_plane();
        changes.add_property(
            dbg!(primary_plane.handle),
            dbg!(primary_plane.fb_id),
            control::property::Value::Object(NonZeroU32::new(
                buffers[0].drm_fb.fd.as_raw_fd() as u32
            )),
        );
        changes.add_property(
            dbg!(primary_plane.handle),
            dbg!(primary_plane.crtc_id),
            control::property::Value::CRTC(Some(dbg!(in_use.crtc.handle))),
        );
        changes.add_property(
            dbg!(primary_plane.handle),
            dbg!(primary_plane.crtc.x.handle),
            control::property::Value::UnsignedRange(0),
        );
        changes.add_property(
            dbg!(primary_plane.handle),
            dbg!(primary_plane.crtc.y.handle),
            control::property::Value::UnsignedRange(0),
        );
        let (hdisplay, vdisplay) = mode.size();
        changes.add_property(
            dbg!(primary_plane.handle),
            dbg!(primary_plane.crtc.width.handle),
            control::property::Value::UnsignedRange(hdisplay as _),
        );
        changes.add_property(
            dbg!(primary_plane.handle),
            dbg!(primary_plane.crtc.height.handle),
            control::property::Value::UnsignedRange(vdisplay as _),
        );
        changes.add_property(
            dbg!(primary_plane.handle),
            dbg!(primary_plane.src.x.handle),
            control::property::Value::UnsignedRange(0),
        );
        changes.add_property(
            dbg!(primary_plane.handle),
            dbg!(primary_plane.src.y.handle),
            control::property::Value::UnsignedRange(0),
        );
        changes.add_property(
            dbg!(primary_plane.handle),
            dbg!(primary_plane.src.width.handle),
            control::property::Value::UnsignedRange((hdisplay as u64) << 16),
        );
        changes.add_property(
            dbg!(primary_plane.handle),
            dbg!(primary_plane.src.height.handle),
            control::property::Value::UnsignedRange((vdisplay as u64) << 16),
        );

        drm.atomic_commit(control::AtomicCommitFlags::ALLOW_MODESET, changes)
            .expect("Failed to set mode");

        (in_use, buffers)
    }

    fn make_render_buffer(
        drm: &DrmFd,
        egl: &EglContext,
        in_use: &ConnectorInUse,
        is_cursor: bool,
    ) -> std::io::Result<RenderBuffer> {
        let (_, plane_modifiers) = in_use.primary_plane();
        let mode = in_use.connector.preferred_mode.as_ref().unwrap();

        let opengl_format = egl.opengl_formats.get(&DrmFourcc::Xrgb8888).unwrap();
        let modifiers: Vec<_> = opengl_format
            .write_modifiers
            .intersection(plane_modifiers)
            .copied()
            .collect();
        assert!(!modifiers.is_empty());

        let mut flags = UseFlags::RENDERING | UseFlags::SCANOUT;
        if is_cursor {
            flags |= UseFlags::LINEAR;
        }

        let bo = egl.gbm.create_bo(&mode, flags, &modifiers);
        let drm_fb = Self::add_framebuffer(drm, bo.dmabuf()).unwrap();
        let image = egl.import_dmabuf(bo.dmabuf());
        let fb = egl.make_fb_from_image(&image, bo.dmabuf());
        fb.clear();
        let texture = image.into_texture().unwrap();

        Ok(RenderBuffer { drm_fb, fb, texture })
    }

    fn add_framebuffer(drm: &DrmFd, dmabuf: &DmaBuf) -> std::io::Result<DrmFrameBuffer> {
        let mut modifier = Modifier::ZERO;
        let mut flags = FbCmd2Flags::empty();
        if dmabuf.modifier != Modifier::INVALID {
            modifier = dmabuf.modifier;
            flags |= FbCmd2Flags::MODIFIERS;
        }

        const N: usize = 4;
        let (mut strides, mut offsets, mut handles, mut modifiers) =
            ([0u32; N], [0u32; N], [0u32; N], [0u64; N]);

        for ((((plane, stride), offset), handle), modifiers) in dmabuf
            .planes
            .iter()
            .zip(&mut strides)
            .zip(&mut offsets)
            .zip(&mut handles)
            .zip(&mut modifiers)
        {
            *stride = plane.stride;
            *offset = plane.offset;
            *modifiers = modifier.0;
            *handle = prime_fd_to_gem_handle(drm, *plane.fd).unwrap();
        }

        let framebuffer = drm_ffi::mode::add_fb2(
            drm.as_fd(),
            dmabuf.width as u32,
            dmabuf.height as u32,
            dmabuf.format.drm as u32,
            &handles,
            &strides,
            &offsets,
            &modifiers,
            flags.bits(),
        )
        .unwrap();

        let handles = &handles[..dmabuf.planes.len()];
        close_gem_handles(drm, handles).unwrap();

        let framebuffer =
            DrmFrameBuffer { fd: unsafe { OwnedFd::from_raw_fd(framebuffer.fb_id as _) } };

        Ok(framebuffer)
    }

    // TODO: Support multiple monitors/connectors
    fn find_first_valid_connector(
        connectors: &BTreeMap<connector::Handle, Connector>,
        crtcs: &BTreeMap<crtc::Handle, Crtc>,
        planes: &BTreeMap<plane::Handle, Plane>,
    ) -> Option<ConnectorInUse> {
        let (connector, crtc) = 'here: {
            for connector in connectors.values() {
                let connector::State::Connected = connector.state else {
                    continue;
                };
                for crtc in &connector.crtcs {
                    let crtc = crtcs.get(crtc).unwrap();
                    if !crtc.active {
                        continue;
                    }
                    break 'here (connector, crtc);
                }
            }
            return None;
        };

        let planes = crtc
            .possible_planes
            .iter()
            .map(|handle| planes.get(handle).cloned().unwrap())
            .collect::<Vec<_>>();

        let primary_plane_index = planes
            .iter()
            .position(|plane| {
                let PlaneType::Primary = plane.kind else { return false };
                plane.formats.contains_key(&DrmFourcc::Xrgb8888)
            })
            .unwrap();

        Some(ConnectorInUse {
            crtc: crtc.clone(),
            connector: connector.clone(),
            planes,
            primary_plane_index,
        })
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

        let formats = match props.get("IN_FORMATS") {
            Some((prop, _)) => get_in_formats(drm, *prop)
                .unwrap()
                .into_iter()
                .filter(|format| !format.modifiers.is_empty())
                .filter_map(|format| FORMATS_MAP.get(&format.format).map(|our| (format, our)))
                .map(|(in_format, format)| {
                    let plane_format =
                        PlaneFormat { format: (*format).clone(), modifiers: in_format.modifiers };
                    (in_format.format, plane_format)
                })
                .collect(),
            None => info
                .formats()
                .iter()
                .filter_map(|id| DrmFourcc::try_from(*id).ok())
                .filter_map(|id| FORMATS_MAP.get(&id).map(|format| (id, format)))
                .map(|(id, format)| {
                    let plane_format = PlaneFormat {
                        format: (*format).clone(),
                        modifiers: [Modifier::INVALID].into_iter().collect(),
                    };
                    (id, plane_format)
                })
                .collect::<HashMap<_, _>>(),
        };

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
        let get_handle = |s: &str| {
            let (_, info) = &props[s];
            info.handle()
        };

        let kind = get_kind(&props).unwrap();
        let crtc_geometry = get_geometry("CRTC_X", "CRTC_Y", "CRTC_W", "CRTC_H");
        let src_geometry = get_geometry("SRC_X", "SRC_Y", "SRC_W", "SRC_H");
        let possible_crtcs = info.possible_crtcs().0;
        let fb_id = get_handle("FB_ID");
        let crtc_id = get_handle("CRTC_ID");

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
            fb_id,
            crtc_id,
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

fn get_in_formats(drm: &DrmFd, prop: u64) -> std::io::Result<Vec<InFormat>> {
    use std::io::ErrorKind::InvalidData;

    let blob = drm.get_property_blob(prop).unwrap();

    if blob.len() < std::mem::size_of::<drm_ffi::drm_format_modifier_blob>() {
        return Err(InvalidData.into());
    }

    let to_u32 = |bytes: &[u8; 4]| u32::from_ne_bytes(*bytes);
    let to_u64 = |bytes: &[u8; 8]| u64::from_ne_bytes(*bytes);

    let mut blob_iter = blob.array_chunks::<4>();
    let mut read_u32 = || -> u32 {
        let bytes = blob_iter.next().unwrap(); // We already checked length just above
        to_u32(bytes)
    };

    let header = drm_ffi::drm_format_modifier_blob {
        version: read_u32(),
        flags: read_u32(),
        count_formats: read_u32(),
        formats_offset: read_u32(),
        count_modifiers: read_u32(),
        modifiers_offset: read_u32(),
    };

    assert_eq!(header.version, 1);

    let formats_start = header.formats_offset as usize;
    let formats_end = formats_start.wrapping_add((header.count_formats as usize).wrapping_mul(4));
    let modifiers_start = header.modifiers_offset as usize;
    let modifiers_end = modifiers_start.wrapping_add(
        (header.count_modifiers as usize)
            .wrapping_mul(std::mem::size_of::<drm_ffi::drm_format_modifier>()),
    );
    let formats = blob
        .get(formats_start..formats_end)
        .ok_or_else(|| std::io::Error::from(InvalidData))?;
    let mut formats_iter = formats.array_chunks::<4>();
    let mut read_u32 = || -> std::io::Result<u32> {
        let bytes = formats_iter
            .next()
            .ok_or_else(|| std::io::Error::from(InvalidData))?;
        Ok(to_u32(bytes))
    };

    let mut formats = (0..header.count_formats)
        .map(|_| {
            let format = read_u32()?;
            let format = DrmFourcc::try_from(format).unwrap();
            Ok(InFormat { format, modifiers: BTreeSet::new() })
        })
        .collect::<Result<Vec<_>, std::io::Error>>()?;

    let modifiers = blob
        .get(modifiers_start..modifiers_end)
        .ok_or_else(|| std::io::Error::from(InvalidData))?;
    let mut modifiers_iter =
        modifiers.array_chunks::<{ std::mem::size_of::<drm_ffi::drm_format_modifier>() }>();
    let mut read_modifier = || -> std::io::Result<drm_ffi::drm_format_modifier> {
        let bytes = modifiers_iter
            .next()
            .ok_or_else(|| std::io::Error::from(InvalidData))?;
        Ok(drm_ffi::drm_format_modifier {
            formats: to_u64(&bytes[0..8].try_into().unwrap()),
            offset: to_u32(&bytes[8..12].try_into().unwrap()),
            pad: to_u32(&bytes[12..16].try_into().unwrap()),
            modifier: to_u64(&bytes[16..24].try_into().unwrap()),
        })
    };

    for _ in 0..header.count_modifiers {
        let drm_ffi::drm_format_modifier { formats: mut bitfields, offset, pad: _, modifier } =
            read_modifier()?;

        while bitfields != 0 {
            let index = bitfields.trailing_zeros();
            bitfields = (bitfields >> index + 1) << index + 1; // clear last bit
            formats[(index + offset) as usize]
                .modifiers
                .insert(Modifier(modifier));
        }
    }

    Ok(formats)
}

fn prime_fd_to_gem_handle(fd: &DrmFd, prime: RawFd) -> std::io::Result<u32> {
    let drm_ffi::drm_prime_handle { handle, flags: _, fd: _ } = unsafe {
        let prime = std::os::fd::BorrowedFd::borrow_raw(prime);
        drm_ffi::gem::fd_to_handle(fd.as_fd(), prime)?
    };
    Ok(handle)
}

fn close_gem_handles(fd: &DrmFd, handles: &[u32]) -> std::io::Result<()> {
    let close = |handle: u32| drm_ffi::gem::close(fd.as_fd(), handle);

    if handles.iter().skip(1).all(|h| *h == handles[0]) {
        close(handles[0]).unwrap();
    } else {
        let set: BTreeSet<_> = handles.iter().collect();
        for handle in set {
            close(*handle).unwrap();
        }
    }
    Ok(())
}

#[derive(Debug)]
struct RenderBuffer {
    drm_fb: DrmFrameBuffer,
    fb: EglFrameBuffer,
    texture: GlTexture,
}

/// Model to handle events
pub struct DrmModel {
    fd: RawFd, // Do not close, it's owned by `Drm`
}

pub enum DrmAction {
    Start,
    Event { event: drm_ffi::drm_event_vblank },
    Stop,
}

impl Model for DrmModel {
    type Action = DrmAction;
    const START: Option<Self::Action> = Some(DrmAction::Start);
    const STOP: Option<Self::Action> = Some(DrmAction::Stop);

    fn process(
        &mut self,
        _state: &mut crate::state::State,
        action: Self::Action,
        dispatcher: &mut crate::runner::Dispatcher,
    ) -> Result<(), ()> {
        match action {
            DrmAction::Start => {
                self.dispatch_read(dispatcher);
            }
            DrmAction::Event { event } => {
                self.handle_event(event);
            }
            DrmAction::Stop => todo!(),
        };
        Ok(())
    }
}

impl DrmModel {
    fn handle_event(&mut self, event: drm_ffi::drm_event_vblank) {
        dbg!(event);
        todo!()
    }

    fn dispatch_read(&self, dispatcher: &mut Dispatcher) {
        use crate::{io_uring::model::IoUringAction, runner::Redispatcher};

        dispatcher.dispatch(IoUringAction::Read {
            fd: self.fd,
            offset: 0,
            length: std::mem::size_of::<drm_ffi::drm_event_vblank>(),
            dispatch_to: Redispatcher::new::<Self>(Self::on_read_event),
        });
    }

    fn on_read_event(result: std::io::Result<ReadWriteCompleted>) -> AnyAction {
        let result = result.unwrap();
        let event = result.read_as::<drm_ffi::drm_event_vblank>();

        // TODO: Handle other events
        let drm_ffi::drm_event { type_, length } = event.base;
        assert_eq!(length as usize, result.buffer.len());
        assert_eq!(type_, drm_ffi::DRM_EVENT_FLIP_COMPLETE);

        AnyAction::from(DrmAction::Event { event })
    }
}
