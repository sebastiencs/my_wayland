use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    num::NonZeroU32,
    os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd, RawFd},
    rc::Rc,
};

use {drm::control::FbCmd2Flags, khronos_types::gl};

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
    egl::{EglContext, EglFrameBuffer, GlTexture, RenderItem, EGL},
    gbm::UseFlags,
    io_uring::ring::{ReadWriteCompleted, RingId},
    runner::{AnyAction, Dispatcher, Model},
    state::{DrmId, State},
    sync::DeferredRelease,
    transform::{self, OutputPresentationTransform},
    wayland_objects::{wl_buffer::WlBuffer, wl_surface::WlSurface},
};

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct Format {
    pub name: &'static str,
    pub bpp: u32,
    pub drm: DrmFourcc,
    pub external_only_guess: bool,
    pub has_alpha: bool,
    pub wayland_id: u32,
    pub gl_format: gl::GLint,
    pub gl_type: gl::GLint,
}

pub static ARGB8888: &Format = &Format {
    name: "argb8888",
    bpp: 4,
    drm: DrmFourcc::Argb8888,
    external_only_guess: false,
    has_alpha: true,
    wayland_id: 0,
    gl_format: gl::BGRA_EXT,
    gl_type: gl::UNSIGNED_BYTE,
};

pub static XRGB8888: &Format = &Format {
    name: "xrgb8888",
    bpp: 4,
    drm: DrmFourcc::Xrgb8888,
    external_only_guess: false,
    has_alpha: false,
    wayland_id: 1,
    gl_format: gl::BGRA_EXT,
    gl_type: gl::UNSIGNED_BYTE,
};

pub static FORMATS: &[Format] = &[*ARGB8888, *XRGB8888];

pub static FORMATS_MAP: Lazy<HashMap<DrmFourcc, &'static Format>> =
    Lazy::new(|| FORMATS.iter().map(|format| (format.drm, format)).collect());

fn probe_syncobj_eventfd_support(drm: &DrmFd) -> bool {
    let Ok(handle) = drm.create_syncobj(false) else {
        return false;
    };
    let raw = unsafe { libc::eventfd(0, libc::EFD_CLOEXEC | libc::EFD_NONBLOCK) };
    if raw < 0 {
        let _ = drm.destroy_syncobj(handle);
        return false;
    }
    let eventfd = unsafe { OwnedFd::from_raw_fd(raw) };
    let result = drm.syncobj_eventfd(handle, 0, eventfd.as_fd(), false);
    let _ = drm.destroy_syncobj(handle);
    result.is_ok()
}

pub const KNOWN_FORMATS: &[DrmFourcc] = &[DrmFourcc::Argb8888, DrmFourcc::Xrgb8888];

#[derive(Debug)]
struct DrmFrameBuffer {
    drm: DrmFd,
    handle: control::framebuffer::Handle,
}

impl DrmFrameBuffer {
    fn id(&self) -> control::framebuffer::Handle {
        self.handle
    }
}

impl Drop for DrmFrameBuffer {
    fn drop(&mut self) {
        if let Err(err) = self.drm.destroy_framebuffer(self.handle) {
            log::error!("Could not destroy DRM framebuffer {:?}: {:?}", self.handle, err);
        }
    }
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
    pub output: DrmOutput,
    driver: Driver,
    supports_syncobj_timeline: bool,
    supports_syncobj_eventfd: bool,
}

#[derive(Debug)]
pub struct DrmOutput {
    pub in_use: ConnectorInUse,
    presentation_transform: OutputPresentationTransform,
    render_sync_mode: RenderSyncMode,
    buffers: [RenderBuffer; 2],
    displayed_buffer_index: usize,
    render_buffer_index: usize,
    needs_frame: bool,
    frame_scheduled: bool,
    rendering_active: bool,
    page_flip_pending: bool,
    in_flight_releases: Vec<DeferredRelease>,
}

impl DrmOutput {
    fn request_frame(&mut self) -> bool {
        self.needs_frame = true;

        if self.frame_scheduled || self.rendering_active || self.page_flip_pending {
            return false;
        }

        self.frame_scheduled = true;
        true
    }

    fn begin_render(&mut self) -> bool {
        if !self.needs_frame
            || !self.frame_scheduled
            || self.rendering_active
            || self.page_flip_pending
        {
            return false;
        }

        self.frame_scheduled = false;
        self.rendering_active = true;
        true
    }

    fn mark_render_failed(&mut self) {
        self.rendering_active = false;
    }

    fn mark_frame_submitted(&mut self) {
        self.rendering_active = false;
        self.page_flip_pending = true;
        self.needs_frame = false;
    }

    fn finish_page_flip(&mut self) -> bool {
        if !self.page_flip_pending {
            return false;
        }

        self.page_flip_pending = false;
        std::mem::swap(&mut self.displayed_buffer_index, &mut self.render_buffer_index);

        if self.needs_frame && !self.frame_scheduled {
            self.frame_scheduled = true;
            return true;
        }

        false
    }

    fn render_buffer(&self) -> &RenderBuffer {
        &self.buffers[self.render_buffer_index]
    }

    fn render_projection(&self) -> transform::Mat3 {
        let framebuffer = &self.render_buffer().gl_framebuffer;
        transform::output_projection(
            framebuffer.width as f32,
            framebuffer.height as f32,
            self.presentation_transform,
        )
    }
}

#[derive(Clone, Debug)]
pub struct DrmFd {
    fd: Rc<OwnedFd>,
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

        Ok(DrmFd { fd: Rc::new(lease) })
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
pub struct Property<T> {
    value: T,
    handle: control::property::Handle,
}

impl<T> Property<T> {
    pub fn value(&self) -> &T {
        &self.value
    }
}

#[derive(Clone)]
pub struct Geometry {
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
pub struct Plane {
    pub handle: plane::Handle,
    pub kind: PlaneType,
    pub crtc: Geometry,
    pub src: Geometry,
    pub formats: HashMap<DrmFourcc, PlaneFormat>,
    pub possible_crtcs: u32,
    pub crtc_handle: Option<control::crtc::Handle>,
    pub fb_id: drm::control::property::Handle,
    pub crtc_id: drm::control::property::Handle,
    pub in_fence_fd: Option<control::property::Handle>,
    pub fb_handle: Option<control::framebuffer::Handle>,
}

impl Plane {
    pub fn fb_handle(&self) -> Option<control::property::Handle> {
        let handle = self.fb_handle?;
        let handle: control::RawResourceHandle = handle.into();
        let handle: control::property::Handle = handle.into();
        Some(handle)
    }

    fn supports_explicit_sync(&self) -> bool {
        self.in_fence_fd.is_some()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RenderSyncMode {
    ExplicitInFence,
    BlockingFinishFallback,
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
    pub mm_width: u32,
    pub mm_height: u32,
    pub model_name: Option<String>,
    pub manufacturer_id: String,
    pub interface_name: String,
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
pub struct ConnectorInUse {
    crtc: Crtc,
    pub connector: Connector,
    planes: Vec<Plane>,
    primary_plane_index: usize,
}

impl ConnectorInUse {
    pub fn primary_plane(&self) -> (&Plane, &BTreeSet<Modifier>) {
        let plane = &self.planes[self.primary_plane_index];
        let format = &plane.formats[&DrmFourcc::Xrgb8888];
        (plane, &format.modifiers)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Copy)]
#[repr(transparent)]
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
        let drm = DrmFd { fd: Rc::new(fd) };
        drm.set_client_capability(drm::ClientCapability::Atomic, true)
            .unwrap();

        let driver: Driver = drm.get_driver().unwrap().into();
        log::info!(
            "Initializing DRM backend: devnode={:?} driver={} desc={}",
            device.devnode(),
            driver.name,
            driver.desc
        );
        let resources = Self::list_resources(&drm);
        log::info!(
            "Discovered DRM resources: connectors={} crtcs={} planes={}",
            resources.connectors.len(),
            resources.crtcs.len(),
            resources.planes.len()
        );

        let gbm = crate::gbm::GbmDevice::new(&drm).unwrap();
        let egl_context = EglContext::make(egl, gbm);
        let output = Self::init_drm(&drm, &egl_context, &resources);

        let supports_syncobj_timeline = drm
            .get_driver_capability(drm::DriverCapability::TimelineSyncObj)
            .unwrap_or(0)
            != 0;
        let supports_syncobj_eventfd =
            supports_syncobj_timeline && probe_syncobj_eventfd_support(&drm);

        Self {
            fd: drm,
            device,
            resources,
            egl_context,
            driver,
            output,
            supports_syncobj_timeline,
            supports_syncobj_eventfd,
        }
    }

    pub fn make_model(&self, drm_id: DrmId) -> DrmModel {
        DrmModel { drm_id }
    }

    pub fn fd(&self) -> DrmFd {
        self.fd.clone()
    }

    pub fn supports_syncobj_timeline(&self) -> bool {
        self.supports_syncobj_timeline
    }

    pub fn supports_linux_drm_syncobj(&self) -> bool {
        self.supports_syncobj_timeline && self.supports_syncobj_eventfd
    }

    pub fn supported_dmabuf_read_formats(&self) -> Vec<(&'static Format, BTreeSet<Modifier>)> {
        self.egl_context
            .opengl_formats
            .values()
            .map(|format| (format.format, format.read_modifiers.clone()))
            .collect()
    }

    fn render_output(&self, items: &[RenderItem]) -> crate::egl::RenderCompletion {
        let projection = self.output.render_projection();
        self.output.render_buffer().gl_framebuffer.render(
            items,
            projection,
            self.output.render_sync_mode,
        )
    }

    fn submit_render_buffer(
        &mut self,
        render_completion: &crate::egl::RenderCompletion,
    ) -> std::io::Result<()> {
        let (primary_plane, fb, crtc) = {
            let output = &self.output;
            let (primary_plane, _) = output.in_use.primary_plane();
            let fb = output.render_buffer().drm_fb.id();
            (primary_plane.clone(), fb, output.in_use.crtc.handle)
        };

        let mut changes = control::atomic::AtomicModeReq::new();
        changes.add_property(
            primary_plane.handle,
            primary_plane.fb_id,
            control::property::Value::Object(Some(fb.into())),
        );
        changes.add_property(
            primary_plane.handle,
            primary_plane.crtc_id,
            control::property::Value::CRTC(Some(crtc)),
        );
        if let Some(in_fence_fd) = render_completion.in_fence_fd.as_ref() {
            if let Some(property) = primary_plane.in_fence_fd {
                changes.add_property(
                    primary_plane.handle,
                    property,
                    control::property::Value::SignedRange(in_fence_fd.as_raw_fd() as i64),
                );
                log::debug!(
                    "drm submit_render_buffer: attached explicit in-fence fd={} plane={:?}",
                    in_fence_fd.as_raw_fd(),
                    primary_plane.handle
                );
            }
        }

        self.fd.atomic_commit(
            control::AtomicCommitFlags::PAGE_FLIP_EVENT | control::AtomicCommitFlags::NONBLOCK,
            changes,
        )?;
        self.output.mark_frame_submitted();
        Ok(())
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

    fn init_drm(drm: &DrmFd, egl: &EglContext, drm_resources: &DrmResources) -> DrmOutput {
        let DrmResources { planes, crtcs, connectors, .. } = drm_resources;

        let in_use = Self::find_first_valid_connector(&connectors, &crtcs, &planes)
            .expect("Failed to find connector");
        let mode = in_use.connector.preferred_mode.as_ref().unwrap();
        let (hdisplay, vdisplay) = mode.size();
        let (primary_plane, plane_modifiers) = in_use.primary_plane();
        let render_sync_mode = if egl.supports_native_render_fence()
            && primary_plane.supports_explicit_sync()
        {
            log::info!(
                "Using explicit compositor sync: connector={} plane={:?} in_fence_fd={:?}",
                in_use.connector.interface_name,
                primary_plane.handle,
                primary_plane.in_fence_fd
            );
            RenderSyncMode::ExplicitInFence
        } else {
            log::warn!(
                "Falling back to blocking GL sync: connector={} plane={:?} egl_native_fence={} plane_in_fence_fd={}",
                in_use.connector.interface_name,
                primary_plane.handle,
                egl.supports_native_render_fence(),
                primary_plane.supports_explicit_sync()
            );
            RenderSyncMode::BlockingFinishFallback
        };
        log::info!(
            "Selected output: connector={} crtc={:?} plane={:?} mode={}x{} modifiers={:?}",
            in_use.connector.interface_name,
            in_use.crtc.handle,
            primary_plane.handle,
            hdisplay,
            vdisplay,
            plane_modifiers
        );

        let mut changes = control::atomic::AtomicModeReq::new();

        let buffers: [_; 2] =
            std::array::from_fn(|index| match Self::make_render_buffer(drm, egl, &in_use, false) {
                Ok(buffer) => buffer,
                Err(error) => {
                    log::error!("Failed to create DRM render buffer #{index}: {:?}", error);
                    panic!("Failed to create DRM render buffer #{index}: {:?}", error);
                }
            });

        changes.add_property(
            primary_plane.handle,
            primary_plane.fb_id,
            control::property::Value::Object(Some(buffers[0].drm_fb.id().into())),
        );
        changes.add_property(
            primary_plane.handle,
            primary_plane.crtc_id,
            control::property::Value::CRTC(Some(in_use.crtc.handle)),
        );
        changes.add_property(
            primary_plane.handle,
            primary_plane.crtc.x.handle,
            control::property::Value::UnsignedRange(0),
        );
        changes.add_property(
            primary_plane.handle,
            primary_plane.crtc.y.handle,
            control::property::Value::UnsignedRange(0),
        );
        changes.add_property(
            primary_plane.handle,
            primary_plane.crtc.width.handle,
            control::property::Value::UnsignedRange(hdisplay as _),
        );
        changes.add_property(
            primary_plane.handle,
            primary_plane.crtc.height.handle,
            control::property::Value::UnsignedRange(vdisplay as _),
        );
        changes.add_property(
            primary_plane.handle,
            primary_plane.src.x.handle,
            control::property::Value::UnsignedRange(0),
        );
        changes.add_property(
            primary_plane.handle,
            primary_plane.src.y.handle,
            control::property::Value::UnsignedRange(0),
        );
        changes.add_property(
            primary_plane.handle,
            primary_plane.src.width.handle,
            control::property::Value::UnsignedRange((hdisplay as u64) << 16),
        );
        changes.add_property(
            primary_plane.handle,
            primary_plane.src.height.handle,
            control::property::Value::UnsignedRange((vdisplay as u64) << 16),
        );

        if let Err(error) = drm.atomic_commit(control::AtomicCommitFlags::ALLOW_MODESET, changes) {
            log::error!(
                "Initial atomic modeset failed: connector={} crtc={:?} plane={:?} fb={:?} mode={}x{} error={:?}",
                in_use.connector.interface_name,
                in_use.crtc.handle,
                primary_plane.handle,
                buffers[0].drm_fb.id(),
                hdisplay,
                vdisplay,
                error
            );
            panic!("Failed to set mode: {:?}", error);
        }

        DrmOutput {
            in_use,
            presentation_transform: OutputPresentationTransform::FlipY,
            render_sync_mode,
            buffers,
            displayed_buffer_index: 0,
            render_buffer_index: 1,
            needs_frame: false,
            frame_scheduled: false,
            rendering_active: false,
            page_flip_pending: false,
            in_flight_releases: Vec::new(),
        }
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
        log::debug!(
            "Creating render buffer: cursor={} mode={:?} plane_modifiers={:?} egl_write_modifiers={:?} chosen_modifiers={:?}",
            is_cursor,
            mode.name(),
            plane_modifiers,
            opengl_format.write_modifiers,
            modifiers
        );

        let mut flags = UseFlags::RENDERING | UseFlags::SCANOUT;
        if is_cursor {
            flags |= UseFlags::LINEAR;
        }

        let bo = egl.gbm.create_bo(&mode, flags, &modifiers);
        let drm_fb = match Self::add_framebuffer(drm, bo.dmabuf()) {
            Ok(framebuffer) => framebuffer,
            Err(error) => {
                log::error!(
                    "Failed to create DRM framebuffer: size={}x{} format={} modifier={:?} flags={:?} error={:?}",
                    bo.dmabuf().width,
                    bo.dmabuf().height,
                    bo.dmabuf().format.name,
                    bo.dmabuf().modifier,
                    flags,
                    error
                );
                return Err(error);
            }
        };
        let image = egl.import_dmabuf(bo.dmabuf());
        let gl_framebuffer = egl.make_fb_from_image(&image, bo.dmabuf());
        gl_framebuffer.clear();
        let texture = image.into_texture().unwrap();

        Ok(RenderBuffer { bo, drm_fb, gl_framebuffer, texture })
    }

    fn add_framebuffer(drm: &DrmFd, dmabuf: &DmaBuf) -> std::io::Result<DrmFrameBuffer> {
        let mut modifier = Modifier::ZERO;
        let mut flags = FbCmd2Flags::empty();
        if dmabuf.modifier != Modifier::INVALID && dmabuf.modifier != Modifier::LINEAR {
            modifier = dmabuf.modifier;
            flags |= FbCmd2Flags::MODIFIERS;
        }
        log::debug!(
            "Adding framebuffer: size={}x{} format={} modifier={:?} addfb_flags={:?}",
            dmabuf.width,
            dmabuf.height,
            dmabuf.format.name,
            dmabuf.modifier,
            flags
        );

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
            *handle = prime_fd_to_gem_handle(drm, plane.fd.as_raw_fd()).unwrap();
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
        )?;

        let handles = &handles[..dmabuf.planes.len()];
        close_gem_handles(drm, handles).unwrap();

        let framebuffer = DrmFrameBuffer {
            drm: drm.clone(),
            handle: NonZeroU32::new(framebuffer.fb_id).unwrap().into(),
        };

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
        let in_fence_fd = props.get("IN_FENCE_FD").map(|(_, info)| info.handle());

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
            in_fence_fd,
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

        let (mm_width, mm_height) = connector.size().unwrap_or((0, 0));

        dbg!(&edid);

        let mut manufacturer_id = String::new();
        let mut model_name = None;
        if let Some(edid) = &edid {
            manufacturer_id = edid.product.manufacturer_id.0.iter().collect();

            use edid_rs::MonitorDescriptor::*;
            for desc in &edid.descriptors.0 {
                match desc {
                    MonitorName(name) => {
                        model_name = Some(name.clone());
                    }
                    SerialNumber(_) => (),
                    OtherString(_) => (),
                    RangeLimits { .. } => (),
                    Undefined(_, _) => (),
                    ManufacturerDefined(_, _) => (),
                }
            }
        }

        let interface_name =
            format!("{}-{}", connector.interface().as_str(), connector.interface_id());

        let connector = Connector {
            handler: connector_handle,
            edid,
            crtcs,
            preferred_mode,
            state,
            mm_width,
            mm_height,
            model_name,
            manufacturer_id,
            interface_name,
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

    let mut blob_iter = blob.iter().copied().array_chunks::<4>();
    let mut read_u32 = || -> u32 {
        let chunk = blob_iter.next().unwrap(); // We already checked length just above
        to_u32(&chunk)
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
    let mut formats_iter = formats.iter().copied().array_chunks::<4>();
    let mut read_u32 = || -> std::io::Result<u32> {
        let chunk = formats_iter
            .next()
            .ok_or_else(|| std::io::Error::from(InvalidData))?;
        Ok(to_u32(&chunk))
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
    const MODIFIER_BLOB_ENTRY: usize = std::mem::size_of::<drm_ffi::drm_format_modifier>();
    let mut modifiers_iter = modifiers
        .iter()
        .copied()
        .array_chunks::<MODIFIER_BLOB_ENTRY>();
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
    /// Keeps the GBM BO (and dma-buf fds) alive while the DRM FB / EGL image / texture exist.
    bo: crate::gbm::BufferObject,
    drm_fb: DrmFrameBuffer,
    gl_framebuffer: EglFrameBuffer,
    texture: GlTexture,
}

/// Store and reassemble DRM event bytes from `read(2)` / io_uring reads.
struct DrmBuffer {
    data: Vec<u8>,
    pos: usize,
}

#[derive(Debug)]
enum ParsedDrmEvent {
    FlipComplete { len: usize, sequence: u32, tv_sec: u32, tv_usec: u32, crtc_id: u32 },
    Ignored { len: usize },
}

impl ParsedDrmEvent {
    fn len(&self) -> usize {
        match self {
            Self::FlipComplete { len, .. } | Self::Ignored { len } => *len,
        }
    }
}

impl DrmBuffer {
    fn new() -> Self {
        Self { data: vec![0; 4096], pos: 0 }
    }

    fn append(&mut self, buffer: &[u8]) {
        let pos = self.pos;
        let buffer_len = buffer.len();

        if self.data.len() < pos + buffer_len {
            self.data.resize(pos + buffer_len, 0);
        }

        self.data[pos..pos + buffer_len].copy_from_slice(buffer);
        self.pos += buffer_len;
    }

    fn next_event(&self) -> Result<Option<ParsedDrmEvent>, ()> {
        let header_len = std::mem::size_of::<drm_ffi::drm_event>();
        let data = &self.data[..self.pos];
        let header = match data.get(..header_len) {
            Some(header) => header,
            None => return Ok(None),
        };

        let header: drm_ffi::drm_event = unsafe { std::ptr::read_unaligned(header.as_ptr() as _) };
        let len = header.length as usize;
        if len < header_len {
            return Err(());
        }
        if len > data.len() {
            return Ok(None);
        }

        match header.type_ {
            drm_ffi::DRM_EVENT_FLIP_COMPLETE => {
                let event_len = std::mem::size_of::<drm_ffi::drm_event_vblank>();
                if len < event_len {
                    return Err(());
                }
                let bytes = &data[..event_len];
                let event: drm_ffi::drm_event_vblank =
                    unsafe { std::ptr::read_unaligned(bytes.as_ptr() as _) };
                Ok(Some(ParsedDrmEvent::FlipComplete {
                    len,
                    sequence: event.sequence,
                    tv_sec: event.tv_sec,
                    tv_usec: event.tv_usec,
                    crtc_id: event.crtc_id,
                }))
            }
            _ => Ok(Some(ParsedDrmEvent::Ignored { len })),
        }
    }

    fn consume_event(&mut self, len: usize) {
        if self.pos == len {
            self.pos = 0;
        } else {
            assert!(self.pos > len);
            self.data.copy_within(len..self.pos, 0);
            self.pos -= len;
        }
    }
}

/// Responsible for reading the DRM fd, parsing kernel events, and forwarding to [`DrmModel`].
struct DrmReader {
    drm_id: DrmId,
    fd: RawFd,
    read_buffer: DrmBuffer,
    read_op_id: Option<RingId>,
}

#[derive(Debug)]
enum DrmReaderAction {
    Start,
    ReadChunk { buffer: Box<[u8]> },
    ReadError { error: std::io::Error },
    Stop,
}

impl Model for DrmReader {
    type Action = DrmReaderAction;
    const START: Option<Self::Action> = Some(DrmReaderAction::Start);
    const STOP: Option<Self::Action> = Some(DrmReaderAction::Stop);

    fn process(
        &mut self,
        _state: &mut crate::state::State,
        action: Self::Action,
        dispatcher: &mut crate::runner::Dispatcher,
    ) -> Result<(), ()> {
        use DrmReaderAction::*;

        match action {
            Start => self.dispatch_read(dispatcher),
            ReadChunk { buffer } => {
                self.read_op_id = None;
                self.read_buffer.append(&buffer);
                loop {
                    let event = match self.read_buffer.next_event() {
                        Ok(Some(event)) => event,
                        Ok(None) => {
                            self.dispatch_read(dispatcher);
                            return Ok(());
                        }
                        Err(()) => {
                            log::error!("Invalid DRM event stream");
                            self.dispatch_read(dispatcher);
                            return Ok(());
                        }
                    };

                    let len = event.len();
                    match event {
                        ParsedDrmEvent::FlipComplete {
                            sequence, tv_sec, tv_usec, crtc_id, ..
                        } => dispatcher.dispatch_to(
                            self.model_id(),
                            DrmAction::FlipComplete { sequence, tv_sec, tv_usec, crtc_id },
                        ),
                        ParsedDrmEvent::Ignored { .. } => {}
                    }
                    self.read_buffer.consume_event(len);
                }
            }
            ReadError { error } => {
                self.read_op_id = None;
                log::error!("Could not read DRM events: {:?}", error);
                self.dispatch_read(dispatcher);
            }
            Stop => {
                if let Some(read_op_id) = self.read_op_id.take() {
                    dispatcher.dispatch(crate::io_uring::model::IoUringAction::Cancel {
                        target: read_op_id,
                    });
                }
                log::debug!("DrmReader stopped for drm_id={:?}", self.drm_id);
            }
        };

        Ok(())
    }
}

impl DrmReader {
    fn new(drm_id: DrmId, fd: RawFd) -> Self {
        Self { drm_id, fd, read_buffer: DrmBuffer::new(), read_op_id: None }
    }

    fn model_id(&self) -> u64 {
        usize::from(self.drm_id) as u64
    }

    fn dispatch_read(&mut self, dispatcher: &mut Dispatcher) {
        use crate::{io_uring::model::IoUringAction, runner::Redispatcher};

        let read_op_id = RingId::fresh();
        self.read_op_id = Some(read_op_id);
        dispatcher.dispatch(IoUringAction::Read {
            id: Some(read_op_id),
            fd: self.fd,
            offset: 0,
            length: 4096,
            dispatch_to: Redispatcher::id(self.model_id(), Self::on_read_event),
        });
    }

    fn on_read_event(result: std::io::Result<ReadWriteCompleted>) -> AnyAction {
        match result {
            Ok(ReadWriteCompleted { fd: _, offset: _, buffer }) => {
                AnyAction::from(DrmReaderAction::ReadChunk { buffer })
            }
            Err(error) => AnyAction::from(DrmReaderAction::ReadError { error }),
        }
    }
}

/// DRM compositor-side model: flip completion and scheduling (fed by [`DrmReader`], like [`crate::dbus::Dbus`]).
pub struct DrmModel {
    drm_id: DrmId,
}

pub enum DrmAction {
    Start,
    ScheduleFrame,
    RenderFrame,
    FlipComplete { sequence: u32, tv_sec: u32, tv_usec: u32, crtc_id: u32 },
    Stop,
}

pub fn schedule_active_output(state: &State, dispatcher: &mut Dispatcher) {
    let Some((drm_id, _)) = state.drm.iter().next() else {
        log::warn!("schedule_active_output: no active DRM output");
        return;
    };

    let model_id = usize::from(drm_id) as u64;
    dispatcher.dispatch_to(model_id, DrmAction::ScheduleFrame);
}

impl Model for DrmModel {
    type Action = DrmAction;
    const START: Option<Self::Action> = Some(DrmAction::Start);
    const STOP: Option<Self::Action> = Some(DrmAction::Stop);

    fn process(
        &mut self,
        state: &mut crate::state::State,
        action: Self::Action,
        dispatcher: &mut crate::runner::Dispatcher,
    ) -> Result<(), ()> {
        match action {
            DrmAction::Start => {
                let fd = state.drm.get(self.drm_id).fd.as_raw_fd();
                dispatcher.start_model(self.model_id(), DrmReader::new(self.drm_id, fd));
            }
            DrmAction::ScheduleFrame => {
                self.schedule_frame(state, dispatcher);
            }
            DrmAction::RenderFrame => {
                self.render_frame(state, dispatcher);
            }
            DrmAction::FlipComplete { sequence, tv_sec, tv_usec, crtc_id } => {
                self.handle_flip_complete(state, dispatcher, sequence, tv_sec, tv_usec, crtc_id);
            }
            DrmAction::Stop => {
                dispatcher.stop_model::<_, DrmReader>(self.model_id());
            }
        };
        Ok(())
    }
}

impl DrmModel {
    fn model_id(&self) -> u64 {
        usize::from(self.drm_id) as u64
    }

    fn schedule_frame(&mut self, state: &mut crate::state::State, dispatcher: &mut Dispatcher) {
        let scheduled = state.drm.get_mut(self.drm_id).output.request_frame();
        if scheduled {
            dispatcher.dispatch_to(self.model_id(), DrmAction::RenderFrame);
        }
    }

    fn render_frame(&mut self, state: &mut crate::state::State, dispatcher: &mut Dispatcher) {
        let (render_items, deferred_releases) = collect_render_items(state, self.drm_id);
        let failed_releases = {
            let drm = state.drm.get_mut(self.drm_id);
            if !drm.output.begin_render() {
                log::warn!("DrmModel::render_frame skipped: output not ready");
                return;
            }

            drm.output.in_flight_releases = deferred_releases;
            let render_completion = drm.render_output(&render_items);
            if let Err(error) = drm.submit_render_buffer(&render_completion) {
                drm.output.mark_render_failed();
                let failed_releases = std::mem::take(&mut drm.output.in_flight_releases);
                if !render_completion.used_blocking_fallback {
                    if let Some(in_fence_fd) = render_completion.in_fence_fd {
                        if let Err(wait_error) = drm.egl_context.wait_on_native_fence(in_fence_fd) {
                            log::warn!(
                                "failed to wait on compositor render fence after submit failure: {:?}",
                                wait_error
                            );
                        }
                    }
                }
                log::error!("Could not submit DRM frame: {:?}", error);
                Some(failed_releases)
            } else {
                None
            }
        };

        if let Some(releases) = failed_releases {
            for release in releases {
                release.resolve(state, dispatcher);
            }
        }
    }

    fn handle_flip_complete(
        &mut self,
        state: &mut crate::state::State,
        dispatcher: &mut Dispatcher,
        _sequence: u32,
        _tv_sec: u32,
        _tv_usec: u32,
        _crtc_id: u32,
    ) {
        let (releases, should_render) = {
            let drm = state.drm.get_mut(self.drm_id);
            let releases = std::mem::take(&mut drm.output.in_flight_releases);
            let should_render = drm.output.finish_page_flip();
            (releases, should_render)
        };

        for release in releases {
            release.resolve(state, dispatcher);
        }
        if should_render {
            dispatcher.dispatch_to(self.model_id(), DrmAction::RenderFrame);
        }
    }
}

fn collect_render_items(
    state: &mut crate::state::State,
    drm_id: DrmId,
) -> (Vec<RenderItem>, Vec<DeferredRelease>) {
    let mut items = Vec::new();
    let mut deferred_releases = Vec::new();
    let mut retired_candidates = Vec::new();
    let surface_oids: Vec<_> = state
        .objects
        .iter()
        .filter_map(|(object_id, object)| object.downcast_ref::<WlSurface>().map(|_| object_id))
        .collect();
    let snapshots = {
        let mut snapshots = Vec::new();
        for object_id in surface_oids {
            let mut obj = state.objects.grab(object_id);
            let Some(surface) = obj.downcast_mut::<WlSurface>() else {
                continue;
            };
            retired_candidates.extend(surface.take_retired_buffers());
            let Some(surface_state) = surface.render_state() else {
                continue;
            };
            snapshots.push(surface_state);
        }
        snapshots
    };

    for retired in retired_candidates {
        deferred_releases.push(DeferredRelease::from_candidate(state, retired));
    }

    let drm = state.drm.get(drm_id);

    for surface_state in snapshots {
        let Some(obj) = state.objects.try_get(surface_state.buffer_object_id) else {
            log::warn!(
                "collect_render_items: buffer object {:?} missing (destroyed?)",
                surface_state.buffer_object_id
            );
            continue;
        };
        let Some(_) = obj.downcast_ref::<WlBuffer>() else {
            log::warn!(
                "collect_render_items: object {:?} is not a WlBuffer",
                surface_state.buffer_object_id
            );
            continue;
        };

        let mut obj = state.objects.grab(surface_state.buffer_object_id);
        let Some(buffer) = obj.downcast_mut::<WlBuffer>() else {
            continue;
        };
        let Some(texture) = buffer.prepare_texture(&drm.egl_context, &surface_state.sync) else {
            log::warn!(
                "collect_render_items: no prepared texture for buffer_object_id={:?}",
                surface_state.buffer_object_id
            );
            continue;
        };
        log::debug!(
            "collect_render_items sample_view buffer_object_id={:?} pos=({}, {}) size={}x{} tex_transform={:?}",
            surface_state.buffer_object_id,
            surface_state.x,
            surface_state.y,
            surface_state.width,
            surface_state.height,
            surface_state.tex_transform
        );
        items.push(RenderItem {
            texture,
            x: surface_state.x,
            y: surface_state.y,
            width: surface_state.width,
            height: surface_state.height,
            tex_transform: buffer.base_tex_transform().mul(surface_state.tex_transform),
        });
    }

    (items, deferred_releases)
}

#[cfg(test)]
mod drm_event_tests {
    use super::ParsedDrmEvent;

    #[test]
    fn parsed_drm_event_len() {
        let e = ParsedDrmEvent::Ignored { len: 64 };
        assert_eq!(e.len(), 64);
        let f = ParsedDrmEvent::FlipComplete {
            len: 32,
            sequence: 0,
            tv_sec: 0,
            tv_usec: 0,
            crtc_id: 0,
        };
        assert_eq!(f.len(), 32);
    }
}
