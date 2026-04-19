use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    num::NonZeroU32,
    os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd, RawFd},
    rc::Rc,
    time::{Duration, Instant},
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
    cursor::{CursorImage, CursorImageContentKey, CursorSourceRef},
    dmabuf::DmaBuf,
    egl::{EglContext, EglFrameBuffer, GlTexture, EGL},
    gbm::{BufferObject, UseFlags},
    io_uring::{
        model::IoUringAction,
        ring::{PollCompleted, ReadWriteCompleted, RingId},
    },
    object::Object,
    region::{DamageRegion, RectI},
    render::{
        rounded_ring, EffectElement, FrameElement, FramePhase, OutputFrameRecipe,
        RootSurfaceVisualState, RoundedRing, TextureElement, TextureMask, TextureSource,
    },
    runner::{Dispatcher, Model, ModelInstanceId},
    scene::OutputCompositionView,
    state::{
        BackendOutputRef, ColorTransformMatrix, DrmId, ObjectId, OutputId, OutputMode,
        RootSurfaceClip, State,
    },
    sync::{DeferredRelease, RetiredBufferCandidate},
    transform::{self, OutputPresentationTransform},
    wayland_objects::{
        wl_buffer::{CursorShmBuffer, WlBuffer},
        wl_surface::{StackEntry, SurfacePresentationFeedback, WlSurface},
    },
    ENABLE_DAMAGE,
};

use crate::{
    clients::{ClientAction, ClientId, ClientManagerAction, Resolved},
    protocol_generated::{WlBufferId, WlSurfaceId, WpPresentationFeedbackId},
    scheduler::{
        complete_frame_for_timeline, discard_frame_for_timeline, frame_clock::ClockDecision,
        output_scheduler_id, FrameCompletionOutcome, OutputScheduler, OutputSchedulerAction,
        PresentationSubsystem, RequestFrameReason,
    },
};

mod kms_submit;
mod output_pipeline;
mod render_planner;

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
    pub outputs: BTreeMap<u32, DrmOutput>,
    driver: Driver,
    supports_syncobj_timeline: bool,
    supports_syncobj_eventfd: bool,
}

#[derive(Debug)]
pub struct DrmOutput {
    pub in_use: ConnectorInUse,
    color_state: OutputColorState,
    presentation_transform: OutputPresentationTransform,
    render_sync_mode: RenderSyncMode,
    pipeline: output_pipeline::OutputPipeline,
    cursor_slots: [Option<CachedCursorPlaneSlot>; 2],
    cursor_plane_state: CursorPlaneState,
    cursor_animation_delay_msec: Option<u32>,
    allow_in_fence_fd: bool,
    allow_fb_damage_clips: bool,
}

#[derive(Clone, Debug, Default)]
struct CrtcColorProperties {
    gamma_lut: Option<control::property::Handle>,
    gamma_lut_size: u32,
    ctm: Option<control::property::Handle>,
}

#[derive(Debug, Default)]
struct OutputColorState {
    gamma_lut: Option<Vec<u16>>,
    ctm: Option<ColorTransformMatrix>,
    gamma_dirty: bool,
    ctm_dirty: bool,
    gamma_blob_id: Option<u64>,
    ctm_blob_id: Option<u64>,
}

#[derive(Debug, Default)]
struct PendingColorBlobs {
    gamma_blob_id: Option<u64>,
    ctm_blob_id: Option<u64>,
}

type SurfaceKey = Resolved<WlSurfaceId>;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
enum SceneItemKey {
    Surface(SurfaceKey),
    Cursor,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SceneItemContentKey {
    Buffer(Resolved<WlBufferId>),
    NamedCursor(CursorImageContentKey),
}

#[derive(Clone, Debug)]
struct SceneItemState {
    key: SceneItemKey,
    surface_key: Option<SurfaceKey>,
    content_key: SceneItemContentKey,
    bounds: RectI,
    tex_transform: transform::Mat3,
}

#[derive(Clone, Copy, Debug)]
enum SceneItemMask {
    None,
    RoundedClip { clip: RootSurfaceClip },
    FrameRing { ring: RoundedRing },
}

#[derive(Debug)]
struct SceneItem {
    state: SceneItemState,
    mask: SceneItemMask,
    texture: gl::Uint,
    owned_texture: Option<GlTexture>,
    damage: DamageRegion,
    sample_damage_padding: i32,
    state_has_alpha: bool,
    alpha: u8,
    opaque_region: DamageRegion,
    opaque: bool,
    sync: crate::sync::SurfaceSyncState,
    direct_scanout: Option<DrmFrameBuffer>,
    presentation_feedbacks: Vec<SurfacePresentationFeedback>,
}

#[derive(Debug)]
struct DirectScanoutCandidate {
    bounds: RectI,
    sync: crate::sync::SurfaceSyncState,
    framebuffer: DrmFrameBuffer,
}

#[derive(Debug)]
struct CursorPlaneBuffer {
    bo: crate::gbm::BufferObject,
    framebuffer: DrmFrameBuffer,
}

#[derive(Debug)]
struct CursorPlaneCandidate {
    x: i32,
    y: i32,
    slot: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CursorPlaneContentKey {
    Named(CursorImageContentKey),
    Surface(CursorSurfaceContentKey),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct CursorSurfaceContentKey {
    surface: Resolved<WlSurfaceId>,
    buffer: Resolved<WlBufferId>,
    content_revision: u64,
    width: i32,
    height: i32,
}

#[derive(Clone, Debug)]
enum CursorPlaneUploadSource {
    Named { image: *const CursorImage },
    SurfaceShm { buffer: CursorShmBuffer },
}

#[derive(Clone, Debug)]
struct CursorPlaneRequest {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    hotspot_x: i32,
    hotspot_y: i32,
    content_key: CursorPlaneContentKey,
    upload: CursorPlaneUploadSource,
    next_frame_in_msec: Option<u32>,
}

#[derive(Debug)]
struct CachedCursorPlaneSlot {
    content_key: Option<CursorPlaneContentKey>,
    buffer: CursorPlaneBuffer,
}

fn find_cursor_slot_index_for_key(
    slot_keys: [Option<CursorPlaneContentKey>; 2],
    key: CursorPlaneContentKey,
) -> Option<usize> {
    slot_keys
        .into_iter()
        .enumerate()
        .find_map(|(index, slot_key)| (slot_key == Some(key)).then_some(index))
}

fn select_cursor_upload_slot(
    displayed_slot: Option<usize>,
    available_slots: [bool; 2],
) -> Option<usize> {
    available_slots
        .into_iter()
        .enumerate()
        .find_map(|(index, available)| {
            (available && Some(index) != displayed_slot).then_some(index)
        })
        .or_else(|| {
            available_slots
                .into_iter()
                .enumerate()
                .find_map(|(index, available)| available.then_some(index))
        })
}

fn cursor_plane_commit_dimensions(fixed_cursor_size: (u32, u32)) -> (u64, u64, u64, u64) {
    let (width, height) = fixed_cursor_size;
    (width as u64, height as u64, (width as u64) << 16, (height as u64) << 16)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CursorPlaneState {
    Ready,
    Backoff { remaining_frames: u8, consecutive_failures: u8 },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CursorOutputVisibility {
    Hidden,
    PartiallyVisible,
    FullyVisible,
}

#[derive(Debug)]
struct FramePlan {
    items: Vec<SceneItem>,
    software_cursor_items: Vec<SceneItem>,
    frame_elements: Vec<RecordedFrameElement>,
    scene: Vec<SceneItemState>,
    frame_callback_surfaces: Vec<SurfaceKey>,
    presentation_feedbacks: Vec<SubmissionPresentationFeedback>,
    damage: DamageRegion,
    direct_scanout: Option<DirectScanoutCandidate>,
    cursor_plane: Option<CursorPlaneCandidate>,
    preserve_contents: bool,
}

#[derive(Clone, Copy, Debug)]
enum RecordedFrameElement {
    Texture { phase: FramePhase, item_index: usize },
    Effect { phase: FramePhase, element: EffectElement },
}

#[derive(Debug)]
struct OutputRenderPlan {
    output_id: Option<OutputId>,
    global_bounds: RectI,
    local_origin: (i32, i32),
    deferred_releases: Vec<DeferredRelease>,
    cursor_animation_delay_msec: Option<u32>,
    frame: FramePlan,
}

#[derive(Debug)]
pub(crate) struct OutputSubmission {
    id: u64,
    output_id: Option<OutputId>,
    scene: Vec<SceneItemState>,
    frame_callback_surfaces: Vec<SurfaceKey>,
    presentation_feedbacks: Vec<SubmissionPresentationFeedback>,
    zero_copy: bool,
    had_effects: bool,
    scene_build_duration: Duration,
    render_submit_duration: Duration,
    submitted_at: Instant,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SubmissionPresentationFeedback {
    pub(crate) feedback: Resolved<WpPresentationFeedbackId>,
    pub(crate) version: u32,
    pub(crate) surface: Option<SurfaceKey>,
}

#[derive(Debug)]
enum SubmittedFrame {
    Composited {
        buffer_index: usize,
        damage: DamageRegion,
        submission: OutputSubmission,
        damage_blob: Option<u64>,
        cursor_slot: Option<usize>,
    },
    DirectScanout {
        submission: OutputSubmission,
        framebuffer: DrmFrameBuffer,
        cursor_slot: Option<usize>,
    },
}

#[derive(Debug)]
struct CompletedSubmission {
    submission: OutputSubmission,
    damage_blob: Option<u64>,
}

#[derive(Debug)]
struct SubmittedFrameRegistration {
    callback_surfaces: Vec<SurfaceKey>,
    submission_id: u64,
    output_id: Option<OutputId>,
    zero_copy: bool,
    presentation_feedbacks: Vec<SubmissionPresentationFeedback>,
}

impl DrmOutput {
    fn output_name(&self) -> &str {
        &self.in_use.connector.interface_name
    }

    fn output_bounds(&self) -> RectI {
        let framebuffer = &self.render_buffer().gl_framebuffer;
        RectI::new(0, 0, framebuffer.width, framebuffer.height)
            .expect("framebuffer dimensions must be non-negative")
    }

    fn begin_render(&mut self) -> bool {
        self.pipeline.begin_render()
    }

    fn mark_render_failed(&mut self) {
        self.pipeline.mark_render_failed();
    }

    fn mark_frame_idle(&mut self) {
        self.pipeline.mark_frame_idle();
    }

    fn allocate_submission(
        &mut self,
        output_id: Option<OutputId>,
        scene: Vec<SceneItemState>,
        frame_callback_surfaces: Vec<SurfaceKey>,
        presentation_feedbacks: Vec<SubmissionPresentationFeedback>,
        zero_copy: bool,
        had_effects: bool,
        scene_build_duration: Duration,
        render_submit_duration: Duration,
        submitted_at: Instant,
    ) -> OutputSubmission {
        self.pipeline.allocate_submission(
            output_id,
            scene,
            frame_callback_surfaces,
            presentation_feedbacks,
            zero_copy,
            had_effects,
            scene_build_duration,
            render_submit_duration,
            submitted_at,
        )
    }

    fn mark_frame_submitted(&mut self, frame: SubmittedFrame) {
        self.pipeline.mark_frame_submitted(frame);
    }

    fn take_in_flight_presentation_feedbacks_for_discard(
        &mut self,
    ) -> (Vec<SubmissionPresentationFeedback>, Option<u64>) {
        self.pipeline
            .take_in_flight_presentation_feedbacks_for_discard()
    }

    fn finish_page_flip(&mut self) -> Option<CompletedSubmission> {
        self.pipeline.finish_page_flip()
    }

    fn render_buffer(&self) -> &RenderBuffer {
        self.pipeline.render_buffer()
    }

    fn render_buffer_index(&self) -> usize {
        self.pipeline.render_buffer_index()
    }

    fn render_projection(&self) -> transform::Mat3 {
        let framebuffer = &self.render_buffer().gl_framebuffer;
        transform::output_projection(
            framebuffer.width as f32,
            framebuffer.height as f32,
            self.presentation_transform,
        )
    }

    fn buffer_age(&self) -> Option<usize> {
        self.pipeline.buffer_age()
    }

    fn cursor_plane_available(&mut self) -> bool {
        let (next_state, available) = advance_cursor_plane_state(self.cursor_plane_state);
        self.cursor_plane_state = next_state;
        available
    }

    fn note_cursor_plane_failure(&mut self) {
        self.cursor_plane_state = next_cursor_plane_backoff_state(self.cursor_plane_state);
    }

    fn note_cursor_plane_success(&mut self) {
        self.cursor_plane_state = CursorPlaneState::Ready;
    }

    fn cursor_slot(&self, index: usize) -> Option<&CachedCursorPlaneSlot> {
        self.cursor_slots.get(index)?.as_ref()
    }

    fn cursor_slot_index_for_content_key(
        &self,
        content_key: CursorPlaneContentKey,
    ) -> Option<usize> {
        find_cursor_slot_index_for_key(
            std::array::from_fn(|index| self.cursor_slot(index).and_then(|slot| slot.content_key)),
            content_key,
        )
    }

    fn preferred_upload_cursor_slot(&self) -> Option<usize> {
        select_cursor_upload_slot(
            self.pipeline.displayed_cursor_slot(),
            std::array::from_fn(|index| self.cursor_slot(index).is_some()),
        )
    }

    fn has_displayed_cursor_slot(&self) -> bool {
        self.pipeline.has_displayed_cursor_slot()
    }

    fn preserve_render_buffer_contents(&self) -> bool {
        self.pipeline.preserve_render_buffer_contents()
    }

    fn accumulate_damage_from_age(&self, base_damage: &DamageRegion) -> DamageRegion {
        self.pipeline
            .accumulate_damage_from_age(base_damage.clone(), self.output_bounds())
    }
}

fn submitted_frame_feedbacks_for_discard(
    in_flight: SubmittedFrame,
) -> (Vec<SubmissionPresentationFeedback>, Option<u64>) {
    match in_flight {
        SubmittedFrame::Composited { submission, damage_blob, .. } => {
            (submission.presentation_feedbacks, damage_blob)
        }
        SubmittedFrame::DirectScanout { submission, .. } => {
            (submission.presentation_feedbacks, None)
        }
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

fn set_nonblocking(fd: RawFd) -> std::io::Result<()> {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags == -1 {
        return Err(std::io::Error::last_os_error());
    }
    if (flags & libc::O_NONBLOCK) != 0 {
        return Ok(());
    }
    let result = unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) };
    if result == -1 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
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

fn color_matrix_fixed_point(value: f32) -> u64 {
    let scaled = (value.abs() as f64 * (1u64 << 32) as f64).round() as u64;
    if value.is_sign_negative() {
        scaled | (1u64 << 63)
    } else {
        scaled
    }
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
    pub fb_damage_clips: Option<control::property::Handle>,
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
    color: CrtcColorProperties,
    vrr_enabled_prop: Option<control::property::Handle>,
    vrr_enabled: bool,
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
    pub vrr_capable: bool,
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
    pub fn crtc_id(&self) -> u32 {
        let handle: control::RawResourceHandle = self.crtc.handle.into();
        handle.into()
    }

    pub fn primary_plane(&self) -> (&Plane, &BTreeSet<Modifier>) {
        let plane = &self.planes[self.primary_plane_index];
        let format = &plane.formats[&DrmFourcc::Xrgb8888];
        (plane, &format.modifiers)
    }

    pub fn cursor_plane(&self) -> Option<&Plane> {
        self.planes.iter().find(|plane| {
            matches!(plane.kind, PlaneType::Cursor)
                && plane.formats.contains_key(&DrmFourcc::Argb8888)
        })
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
    fn output(&self, drm_crtc_id: u32) -> Option<&DrmOutput> {
        self.outputs.get(&drm_crtc_id)
    }

    fn output_mut(&mut self, drm_crtc_id: u32) -> Option<&mut DrmOutput> {
        self.outputs.get_mut(&drm_crtc_id)
    }

    pub fn output_connector_in_use(&self, drm_crtc_id: u32) -> Option<&ConnectorInUse> {
        self.output(drm_crtc_id).map(|output| &output.in_use)
    }

    pub fn current_output_mode(&self, drm_crtc_id: u32) -> Option<OutputMode> {
        let mode_blob = self.outputs.get(&drm_crtc_id)?.in_use.crtc.mode_id;
        if mode_blob == 0 {
            return None;
        }
        let blob = self.fd.get_property_blob(mode_blob as u64).ok()?;
        if blob.len() < std::mem::size_of::<drm_ffi::drm_sys::drm_mode_modeinfo>() {
            return None;
        }
        let raw: drm_ffi::drm_sys::drm_mode_modeinfo = unsafe {
            std::ptr::read_unaligned(blob.as_ptr() as *const drm_ffi::drm_sys::drm_mode_modeinfo)
        };
        let clock_milli_hertz = raw.clock as u64 * 1_000_000;
        let refresh_millihz = (((clock_milli_hertz / raw.htotal as u64) + (raw.vtotal as u64 / 2))
            / raw.vtotal as u64) as i32;
        Some(OutputMode {
            width: raw.hdisplay as i32,
            height: raw.vdisplay as i32,
            refresh_millihz,
        })
    }

    pub fn output_gamma_size(&self, drm_crtc_id: u32) -> u32 {
        self.output(drm_crtc_id)
            .map(|output| output.in_use.crtc.color.gamma_lut_size)
            .unwrap_or(0)
    }

    pub fn output_supports_ctm(&self, drm_crtc_id: u32) -> bool {
        self.output(drm_crtc_id)
            .is_some_and(|output| output.in_use.crtc.color.ctm.is_some())
    }

    pub fn output_vrr_capable(&self, drm_crtc_id: u32) -> bool {
        self.output(drm_crtc_id)
            .is_some_and(|output| output.in_use.connector.vrr_capable)
    }

    pub fn output_vrr_enabled(&self, drm_crtc_id: u32) -> bool {
        self.output(drm_crtc_id)
            .is_some_and(|output| output.in_use.crtc.vrr_enabled)
    }

    pub fn output_vrr_range(
        &self,
        drm_crtc_id: u32,
    ) -> Option<crate::scheduler::frame_clock::VrrRange> {
        let output = self.output(drm_crtc_id)?;
        let current_refresh_millihz = self
            .current_output_mode(drm_crtc_id)?
            .refresh_millihz
            .max(0) as u32;
        let edid = output.in_use.connector.edid.as_ref()?;
        use edid_rs::MonitorDescriptor;
        edid.descriptors.0.iter().find_map(|desc| match desc {
            MonitorDescriptor::RangeLimits { vertical_rate, .. } => {
                let min_refresh_millihz = u32::from(vertical_rate.0).saturating_mul(1000);
                let max_refresh_millihz =
                    current_refresh_millihz.min(u32::from(vertical_rate.1).saturating_mul(1000));
                (min_refresh_millihz > 0 && max_refresh_millihz >= min_refresh_millihz).then_some(
                    crate::scheduler::frame_clock::VrrRange {
                        min_refresh_millihz,
                        max_refresh_millihz,
                    },
                )
            }
            _ => None,
        })
    }

    pub fn set_output_vrr_enabled(
        &mut self,
        drm_crtc_id: u32,
        enabled: bool,
    ) -> std::io::Result<()> {
        let Some(output) = self.output_mut(drm_crtc_id) else {
            return Err(std::io::Error::new(std::io::ErrorKind::NotFound, "missing DRM output"));
        };
        if !output.in_use.connector.vrr_capable {
            if enabled {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::Unsupported,
                    "output does not support VRR",
                ));
            }
            output.in_use.crtc.vrr_enabled = false;
            return Ok(());
        }
        if output.in_use.crtc.vrr_enabled_prop.is_none() {
            if enabled {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::Unsupported,
                    "CRTC does not expose VRR_ENABLED",
                ));
            }
            output.in_use.crtc.vrr_enabled = false;
            return Ok(());
        }
        output.in_use.crtc.vrr_enabled = enabled;
        Ok(())
    }

    pub fn set_output_gamma_lut(
        &mut self,
        drm_crtc_id: u32,
        lut: Option<Vec<u16>>,
    ) -> std::io::Result<()> {
        let Some(output) = self.output_mut(drm_crtc_id) else {
            return Err(std::io::Error::new(std::io::ErrorKind::NotFound, "missing DRM output"));
        };
        let expected_len = output.in_use.crtc.color.gamma_lut_size as usize * 3;
        if expected_len == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "output does not support gamma LUTs",
            ));
        }
        if let Some(lut) = lut.as_ref() {
            if lut.len() != expected_len {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "gamma LUT length does not match advertised ramp size",
                ));
            }
        }
        if output.color_state.gamma_lut == lut {
            return Ok(());
        }
        output.color_state.gamma_lut = lut;
        output.color_state.gamma_dirty = true;
        Ok(())
    }

    pub fn set_output_ctm(
        &mut self,
        drm_crtc_id: u32,
        ctm: Option<ColorTransformMatrix>,
    ) -> std::io::Result<()> {
        let Some(output) = self.output_mut(drm_crtc_id) else {
            return Err(std::io::Error::new(std::io::ErrorKind::NotFound, "missing DRM output"));
        };
        if output.in_use.crtc.color.ctm.is_none() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "output does not support CTM",
            ));
        }
        if output.color_state.ctm == ctm {
            return Ok(());
        }
        output.color_state.ctm = ctm;
        output.color_state.ctm_dirty = true;
        Ok(())
    }

    fn optional_plane_attempts(
        &self,
        want_damage_clips: bool,
        want_in_fence: bool,
    ) -> Vec<(bool, bool)> {
        let mut attempts = vec![(want_damage_clips, want_in_fence)];
        if want_damage_clips {
            attempts.push((false, want_in_fence));
        }
        if want_in_fence {
            attempts.push((want_damage_clips, false));
        }
        if want_damage_clips || want_in_fence {
            attempts.push((false, false));
        }
        attempts.sort_unstable();
        attempts.dedup();
        attempts.reverse();
        attempts
    }

    fn append_output_color_changes(
        &mut self,
        drm_crtc_id: u32,
        changes: &mut control::atomic::AtomicModeReq,
    ) -> std::io::Result<PendingColorBlobs> {
        let Some(output) = self.output(drm_crtc_id) else {
            return Err(std::io::Error::new(std::io::ErrorKind::NotFound, "missing DRM output"));
        };
        let crtc_handle = output.in_use.crtc.handle;
        let color_props = output.in_use.crtc.color.clone();
        let gamma_dirty = output.color_state.gamma_dirty;
        let ctm_dirty = output.color_state.ctm_dirty;
        let gamma_lut = output.color_state.gamma_lut.clone();
        let ctm = output.color_state.ctm;
        let ctm_blob_id = output.color_state.ctm_blob_id;

        let mut pending = PendingColorBlobs::default();

        if gamma_dirty {
            let Some(property) = color_props.gamma_lut else {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::Unsupported,
                    "output does not support gamma LUTs",
                ));
            };
            match gamma_lut.as_ref() {
                Some(gamma_lut) => {
                    let lut = gamma_lut
                        .chunks_exact(3)
                        .map(|entry| drm_ffi::drm_sys::drm_color_lut {
                            red: entry[0],
                            green: entry[1],
                            blue: entry[2],
                            reserved: 0,
                        })
                        .collect::<Vec<_>>();
                    let blob = self.fd.create_property_blob(&lut)?;
                    pending.gamma_blob_id = blob.as_blob();
                    changes.add_property(crtc_handle, property, blob);
                }
                None => {
                    changes.add_property(crtc_handle, property, control::property::Value::Blob(0));
                }
            }
        }

        if ctm_dirty || ctm.is_some() {
            let Some(property) = color_props.ctm else {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::Unsupported,
                    "output does not support CTM",
                ));
            };
            match ctm {
                Some(ctm) => {
                    if ctm_dirty || ctm_blob_id.is_none() {
                        let mut raw = drm_ffi::drm_sys::drm_color_ctm { matrix: [0; 9] };
                        for (index, value) in ctm.values.into_iter().enumerate() {
                            raw.matrix[index] = color_matrix_fixed_point(value);
                        }
                        let blob = self.fd.create_property_blob(&raw)?;
                        pending.ctm_blob_id = blob.as_blob();
                        changes.add_property(crtc_handle, property, blob);
                    } else {
                        changes.add_property(
                            crtc_handle,
                            property,
                            control::property::Value::Blob(
                                ctm_blob_id.expect("non-dirty CTM must have an existing blob id"),
                            ),
                        );
                    }
                }
                None => {
                    changes.add_property(crtc_handle, property, control::property::Value::Blob(0));
                }
            }
        }

        Ok(pending)
    }

    fn append_output_vrr_change(
        &self,
        drm_crtc_id: u32,
        changes: &mut control::atomic::AtomicModeReq,
    ) -> std::io::Result<()> {
        let Some(output) = self.output(drm_crtc_id) else {
            return Err(std::io::Error::new(std::io::ErrorKind::NotFound, "missing DRM output"));
        };
        let Some(property) = output.in_use.crtc.vrr_enabled_prop else {
            return Ok(());
        };
        changes.add_property(
            output.in_use.crtc.handle,
            property,
            control::property::Value::Boolean(output.in_use.crtc.vrr_enabled),
        );
        Ok(())
    }

    fn destroy_pending_color_blobs(&self, blobs: PendingColorBlobs) {
        if let Some(blob_id) = blobs.gamma_blob_id {
            let _ = self.fd.destroy_property_blob(blob_id);
        }
        if let Some(blob_id) = blobs.ctm_blob_id {
            let _ = self.fd.destroy_property_blob(blob_id);
        }
    }

    fn finish_output_color_commit(&mut self, drm_crtc_id: u32, blobs: PendingColorBlobs) {
        let Some(_) = self.output(drm_crtc_id) else {
            return;
        };
        let (old_gamma_blob, old_ctm_blob) = {
            let output = self.output_mut(drm_crtc_id).unwrap();
            let old_gamma_blob = if output.color_state.gamma_dirty {
                output.color_state.gamma_dirty = false;
                std::mem::replace(&mut output.color_state.gamma_blob_id, blobs.gamma_blob_id)
            } else {
                None
            };
            let old_ctm_blob = if output.color_state.ctm_dirty {
                output.color_state.ctm_dirty = false;
                std::mem::replace(&mut output.color_state.ctm_blob_id, blobs.ctm_blob_id)
            } else {
                None
            };
            (old_gamma_blob, old_ctm_blob)
        };

        if let Some(blob_id) = old_gamma_blob {
            let _ = self.fd.destroy_property_blob(blob_id);
        }
        if let Some(blob_id) = old_ctm_blob {
            let _ = self.fd.destroy_property_blob(blob_id);
        }
    }

    pub fn create_drm(fd: OwnedFd, device: udev::Device, egl: &EGL) -> Self {
        let drm = DrmFd { fd: Rc::new(fd) };
        set_nonblocking(drm.as_raw_fd()).unwrap();
        match drm.set_client_capability(drm::ClientCapability::UniversalPlanes, true) {
            Ok(()) => log::info!("Enabled DRM client capability: UniversalPlanes"),
            Err(error) => {
                log::warn!("Failed to enable DRM client capability UniversalPlanes: {:?}", error)
            }
        }
        drm.set_client_capability(drm::ClientCapability::Atomic, true)
            .map(|()| log::info!("Enabled DRM client capability: Atomic"))
            .unwrap();
        match drm.set_client_capability(drm::ClientCapability::CursorPlaneHotspot, true) {
            Ok(()) => log::info!("Enabled DRM client capability: CursorPlaneHotspot"),
            Err(error) => {
                log::warn!("Failed to enable DRM client capability CursorPlaneHotspot: {:?}", error)
            }
        }

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
        let outputs = Self::init_drm_outputs(&drm, &egl_context, &resources);

        let supports_syncobj_timeline = drm
            .get_driver_capability(drm::DriverCapability::TimelineSyncObj)
            .unwrap_or(0)
            != 0;
        let supports_syncobj_eventfd =
            supports_syncobj_timeline && probe_syncobj_eventfd_support(&drm);

        let mut drm_backend = Self {
            fd: drm,
            device,
            resources,
            egl_context,
            driver,
            outputs,
            supports_syncobj_timeline,
            supports_syncobj_eventfd,
        };
        let drm_output_ids = drm_backend.outputs.keys().copied().collect::<Vec<_>>();
        for drm_output_id in drm_output_ids {
            drm_backend.initialize_cursor_plane_slots(drm_output_id);
        }
        drm_backend
    }

    pub fn make_output_model(&self, drm_id: DrmId, output_id: OutputId) -> DrmOutputModel {
        DrmOutputModel { drm_id, output_id }
    }

    pub fn make_output_scheduler(&self, drm_id: DrmId, output_id: OutputId) -> OutputScheduler {
        OutputScheduler::new(drm_id, output_id)
    }

    pub fn fd(&self) -> DrmFd {
        self.fd.clone()
    }

    pub fn devnum(&self) -> Option<libc::dev_t> {
        self.device.devnum()
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

    fn render_output(
        &self,
        drm_crtc_id: u32,
        recipe: &OutputFrameRecipe,
        damage: &DamageRegion,
        preserve_contents: bool,
    ) -> crate::egl::RenderCompletion {
        let output = self
            .output(drm_crtc_id)
            .expect("missing DRM output pipeline for render");
        let projection = output.render_projection();
        output.render_buffer().gl_framebuffer.render(
            recipe,
            projection,
            output.render_sync_mode,
            damage,
            preserve_contents,
            output.presentation_transform,
        )
    }

    fn collect_cursor_plane_candidate(
        &mut self,
        drm_crtc_id: u32,
        request: &CursorPlaneRequest,
        output_bounds: RectI,
    ) -> Option<CursorPlaneCandidate> {
        if request.width <= 0
            || request.height <= 0
            || request.width as u64 > self.resources.cursor_width
            || request.height as u64 > self.resources.cursor_height
        {
            return None;
        }
        let x = request.x;
        let y = request.y;
        let bounds = RectI::new(x, y, request.width, request.height)?;
        if classify_cursor_output_visibility(bounds, output_bounds)
            != CursorOutputVisibility::FullyVisible
        {
            return None;
        }
        let (fb_width, fb_height) = self.fixed_cursor_plane_size();
        let slot = if let Some(slot) = self
            .output(drm_crtc_id)?
            .cursor_slot_index_for_content_key(request.content_key)
        {
            let cached = self.output(drm_crtc_id)?.cursor_slot(slot).unwrap();
            if crate::logger::ENABLE_NOISY_LOG {
                log::debug!(
                "Reusing cached DRM cursor plane buffer: slot={} visible_size={}x{} fb_size={}x{} pos=({}, {}) hotspot=({}, {}) fb={:?}",
                slot,
                request.width,
                request.height,
                fb_width,
                fb_height,
                x,
                y,
                request.hotspot_x,
                request.hotspot_y,
                cached.buffer.framebuffer.id()
                );
            }
            slot
        } else {
            let slot = self.output(drm_crtc_id)?.preferred_upload_cursor_slot()?;
            self.upload_cursor_plane_slot(drm_crtc_id, slot, request)?;
            let cached = self.output(drm_crtc_id)?.cursor_slot(slot).unwrap();
            if crate::logger::ENABLE_NOISY_LOG {
                log::debug!(
                "Prepared DRM cursor plane candidate: slot={} visible_size={}x{} fb_size={}x{} pos=({}, {}) hotspot=({}, {}) fb={:?}",
                slot,
                request.width,
                request.height,
                fb_width,
                fb_height,
                x,
                y,
                request.hotspot_x,
                request.hotspot_y,
                cached.buffer.framebuffer.id()
                );
            }
            slot
        };
        Some(CursorPlaneCandidate { x, y, slot })
    }

    fn append_cursor_plane_changes(
        &self,
        drm_crtc_id: u32,
        changes: &mut control::atomic::AtomicModeReq,
        cursor: Option<&CursorPlaneCandidate>,
    ) {
        let Some(output) = self.output(drm_crtc_id) else {
            return;
        };
        let Some(plane) = output.in_use.cursor_plane() else {
            return;
        };
        let crtc = output.in_use.crtc.handle;
        let (cursor_plane_width, cursor_plane_height, cursor_src_width, cursor_src_height) =
            cursor_plane_commit_dimensions(self.fixed_cursor_plane_size());
        let (fb, crtc_value, x, y, width, height, src_width, src_height) = match cursor {
            Some(cursor) => (
                control::property::Value::Object(Some(
                    output
                        .cursor_slot(cursor.slot)
                        .expect("cursor plane slot missing during atomic commit")
                        .buffer
                        .framebuffer
                        .id()
                        .into(),
                )),
                control::property::Value::CRTC(Some(crtc)),
                cursor.x as u64,
                cursor.y as u64,
                cursor_plane_width,
                cursor_plane_height,
                cursor_src_width,
                cursor_src_height,
            ),
            None => (
                control::property::Value::Object(None),
                control::property::Value::CRTC(None),
                0,
                0,
                0,
                0,
                0,
                0,
            ),
        };
        changes.add_property(plane.handle, plane.fb_id, fb);
        changes.add_property(plane.handle, plane.crtc_id, crtc_value);
        changes.add_property(
            plane.handle,
            plane.crtc.x.handle,
            control::property::Value::UnsignedRange(x),
        );
        changes.add_property(
            plane.handle,
            plane.crtc.y.handle,
            control::property::Value::UnsignedRange(y),
        );
        changes.add_property(
            plane.handle,
            plane.crtc.width.handle,
            control::property::Value::UnsignedRange(width),
        );
        changes.add_property(
            plane.handle,
            plane.crtc.height.handle,
            control::property::Value::UnsignedRange(height),
        );
        changes.add_property(
            plane.handle,
            plane.src.x.handle,
            control::property::Value::UnsignedRange(0),
        );
        changes.add_property(
            plane.handle,
            plane.src.y.handle,
            control::property::Value::UnsignedRange(0),
        );
        changes.add_property(
            plane.handle,
            plane.src.width.handle,
            control::property::Value::UnsignedRange(src_width),
        );
        changes.add_property(
            plane.handle,
            plane.src.height.handle,
            control::property::Value::UnsignedRange(src_height),
        );
        if let Some(property) = plane.in_fence_fd {
            changes.add_property(plane.handle, property, control::property::Value::SignedRange(-1));
        }
        if let Some(property) = plane.fb_damage_clips {
            changes.add_property(plane.handle, property, control::property::Value::Blob(0));
        }
    }

    fn submit_render_buffer(
        &mut self,
        drm_crtc_id: u32,
        render_completion: &crate::egl::RenderCompletion,
        damage: &DamageRegion,
        output_id: Option<OutputId>,
        scene: Vec<SceneItemState>,
        frame_callback_surfaces: Vec<SurfaceKey>,
        presentation_feedbacks: Vec<SubmissionPresentationFeedback>,
        cursor_plane: Option<CursorPlaneCandidate>,
        had_effects: bool,
        scene_build_duration: Duration,
        render_started_at: Instant,
    ) -> std::io::Result<SubmittedFrameRegistration> {
        let (primary_plane, fb, crtc) = {
            let output = self
                .output(drm_crtc_id)
                .expect("missing DRM output pipeline for submit");
            let (primary_plane, _) = output.in_use.primary_plane();
            let fb = output.render_buffer().drm_fb.id();
            (primary_plane.clone(), fb, output.in_use.crtc.handle)
        };
        let rects: Vec<drm_ffi::drm_mode_rect> = damage
            .rects()
            .iter()
            .map(|rect| drm_ffi::drm_mode_rect {
                x1: rect.x,
                y1: rect.y,
                x2: rect.x2(),
                y2: rect.y2(),
            })
            .collect();
        let (want_damage_clips, want_in_fence) = {
            let output = self.output(drm_crtc_id).unwrap();
            (
                ENABLE_DAMAGE
                    && output.allow_fb_damage_clips
                    && primary_plane.fb_damage_clips.is_some()
                    && !rects.is_empty(),
                output.allow_in_fence_fd
                    && primary_plane.in_fence_fd.is_some()
                    && render_completion.in_fence_fd.is_some(),
            )
        };
        let mut last_error = None;

        for (use_damage_clips, use_in_fence) in
            self.optional_plane_attempts(want_damage_clips, want_in_fence)
        {
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
            self.append_cursor_plane_changes(drm_crtc_id, &mut changes, cursor_plane.as_ref());

            if let Some(property) = primary_plane.in_fence_fd {
                let fence_fd = if use_in_fence {
                    render_completion
                        .in_fence_fd
                        .as_ref()
                        .map(|fd| fd.as_raw_fd() as i64)
                        .unwrap_or(-1)
                } else {
                    -1
                };
                changes.add_property(
                    primary_plane.handle,
                    property,
                    control::property::Value::SignedRange(fence_fd),
                );
            }

            let mut damage_blob = None;
            if let Some(property) = primary_plane.fb_damage_clips {
                if use_damage_clips {
                    if let Ok(blob) = self.fd.create_property_blob(&rects) {
                        changes.add_property(primary_plane.handle, property, blob);
                        damage_blob = blob.as_blob();
                    } else {
                        log::warn!(
                            "drm submit_render_buffer: failed to create FB_DAMAGE_CLIPS blob, falling back to Blob(0)"
                        );
                        changes.add_property(
                            primary_plane.handle,
                            property,
                            control::property::Value::Blob(0),
                        );
                    }
                } else {
                    changes.add_property(
                        primary_plane.handle,
                        property,
                        control::property::Value::Blob(0),
                    );
                }
            }

            let color_blobs = match self.append_output_color_changes(drm_crtc_id, &mut changes) {
                Ok(blobs) => blobs,
                Err(error) => return Err(error),
            };
            self.append_output_vrr_change(drm_crtc_id, &mut changes)?;

            let commit_result = self.fd.atomic_commit(
                control::AtomicCommitFlags::PAGE_FLIP_EVENT | control::AtomicCommitFlags::NONBLOCK,
                changes,
            );
            match commit_result {
                Ok(()) => {
                    let submitted_at = Instant::now();
                    let render_submit_duration =
                        submitted_at.saturating_duration_since(render_started_at);
                    self.finish_output_color_commit(drm_crtc_id, color_blobs);
                    if want_damage_clips && !use_damage_clips {
                        log::warn!(
                            "drm submit_render_buffer: driver rejected FB_DAMAGE_CLIPS on {}, disabling output damage clips",
                            self.driver.name
                        );
                        self.output_mut(drm_crtc_id).unwrap().allow_fb_damage_clips = false;
                    }
                    if want_in_fence && !use_in_fence {
                        log::warn!(
                            "drm submit_render_buffer: driver rejected IN_FENCE_FD on {}, disabling explicit KMS in-fences",
                            self.driver.name
                        );
                        self.output_mut(drm_crtc_id).unwrap().allow_in_fence_fd = false;
                    }
                    let buffer_index = self.output(drm_crtc_id).unwrap().render_buffer_index();
                    let submission = self.output_mut(drm_crtc_id).unwrap().allocate_submission(
                        output_id,
                        scene,
                        frame_callback_surfaces,
                        presentation_feedbacks.clone(),
                        false,
                        had_effects,
                        scene_build_duration,
                        render_submit_duration,
                        submitted_at,
                    );
                    let callback_surfaces = submission.frame_callback_surfaces.clone();
                    let submission_id = submission.id;
                    self.output_mut(drm_crtc_id).unwrap().mark_frame_submitted(
                        SubmittedFrame::Composited {
                            buffer_index,
                            damage: damage.clone(),
                            submission,
                            damage_blob,
                            cursor_slot: cursor_plane.map(|cursor| cursor.slot),
                        },
                    );
                    return Ok(SubmittedFrameRegistration {
                        callback_surfaces,
                        submission_id,
                        output_id,
                        zero_copy: false,
                        presentation_feedbacks,
                    });
                }
                Err(error) => {
                    self.destroy_pending_color_blobs(color_blobs);
                    if let Some(blob) = damage_blob {
                        let _ = self.fd.destroy_property_blob(blob);
                    }
                    let retryable = error.raw_os_error() == Some(libc::EINVAL)
                        && ((want_damage_clips && use_damage_clips)
                            || (want_in_fence && use_in_fence));
                    if retryable {
                        log::warn!(
                            "drm submit_render_buffer: atomic commit rejected optional props (damage_clips={}, in_fence={}) on {}: {:?}",
                            use_damage_clips,
                            use_in_fence,
                            self.driver.name,
                            error
                        );
                        last_error = Some(error);
                        continue;
                    }
                    return Err(error);
                }
            }
        }

        let Some(error) = last_error else {
            return Err(std::io::Error::other("no atomic commit attempt executed"));
        };
        return Err(error);
    }

    fn submit_direct_scanout(
        &mut self,
        drm_crtc_id: u32,
        candidate: DirectScanoutCandidate,
        output_id: Option<OutputId>,
        scene: Vec<SceneItemState>,
        frame_callback_surfaces: Vec<SurfaceKey>,
        presentation_feedbacks: Vec<SubmissionPresentationFeedback>,
        cursor_plane: Option<CursorPlaneCandidate>,
        scene_build_duration: Duration,
        render_started_at: Instant,
    ) -> std::io::Result<SubmittedFrameRegistration> {
        let (primary_plane, crtc) = {
            let output = self
                .output(drm_crtc_id)
                .expect("missing DRM output pipeline for scanout");
            let (primary_plane, _) = output.in_use.primary_plane();
            (primary_plane.clone(), output.in_use.crtc.handle)
        };

        let mut changes = control::atomic::AtomicModeReq::new();
        changes.add_property(
            primary_plane.handle,
            primary_plane.fb_id,
            control::property::Value::Object(Some(candidate.framebuffer.id().into())),
        );
        changes.add_property(
            primary_plane.handle,
            primary_plane.crtc_id,
            control::property::Value::CRTC(Some(crtc)),
        );
        self.append_cursor_plane_changes(drm_crtc_id, &mut changes, cursor_plane.as_ref());
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
            control::property::Value::UnsignedRange(candidate.bounds.width as u64),
        );
        changes.add_property(
            primary_plane.handle,
            primary_plane.crtc.height.handle,
            control::property::Value::UnsignedRange(candidate.bounds.height as u64),
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
            control::property::Value::UnsignedRange((candidate.bounds.width as u64) << 16),
        );
        changes.add_property(
            primary_plane.handle,
            primary_plane.src.height.handle,
            control::property::Value::UnsignedRange((candidate.bounds.height as u64) << 16),
        );
        if let Some(property) = primary_plane.fb_damage_clips {
            changes.add_property(primary_plane.handle, property, control::property::Value::Blob(0));
        }
        if let crate::sync::AcquireSync::SyncFile(sync_file) = &candidate.sync.acquire {
            let Some(property) = primary_plane.in_fence_fd.filter(|_| {
                self.output(drm_crtc_id)
                    .is_some_and(|output| output.allow_in_fence_fd)
            }) else {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "scanout sync-file acquire requires IN_FENCE_FD support",
                ));
            };
            changes.add_property(
                primary_plane.handle,
                property,
                control::property::Value::SignedRange(sync_file.as_raw_fd() as i64),
            );
        } else if !candidate.sync.acquire.is_none() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "scanout currently only supports empty or sync-file acquire fences",
            ));
        } else if let Some(property) = primary_plane.in_fence_fd.filter(|_| {
            self.output(drm_crtc_id)
                .is_some_and(|output| output.allow_in_fence_fd)
        }) {
            changes.add_property(
                primary_plane.handle,
                property,
                control::property::Value::SignedRange(-1),
            );
        }

        let color_blobs = self.append_output_color_changes(drm_crtc_id, &mut changes)?;
        self.append_output_vrr_change(drm_crtc_id, &mut changes)?;
        if let Err(error) = self.fd.atomic_commit(
            control::AtomicCommitFlags::PAGE_FLIP_EVENT | control::AtomicCommitFlags::NONBLOCK,
            changes,
        ) {
            self.destroy_pending_color_blobs(color_blobs);
            return Err(error);
        }
        self.finish_output_color_commit(drm_crtc_id, color_blobs);
        let submitted_at = Instant::now();
        let render_submit_duration = submitted_at.saturating_duration_since(render_started_at);
        let submission = self.output_mut(drm_crtc_id).unwrap().allocate_submission(
            output_id,
            scene,
            frame_callback_surfaces,
            presentation_feedbacks.clone(),
            true,
            false,
            scene_build_duration,
            render_submit_duration,
            submitted_at,
        );
        let callback_surfaces = submission.frame_callback_surfaces.clone();
        let submission_id = submission.id;
        self.output_mut(drm_crtc_id)
            .unwrap()
            .mark_frame_submitted(SubmittedFrame::DirectScanout {
                submission,
                framebuffer: candidate.framebuffer,
                cursor_slot: cursor_plane.map(|cursor| cursor.slot),
            });
        Ok(SubmittedFrameRegistration {
            callback_surfaces,
            submission_id,
            output_id,
            zero_copy: true,
            presentation_feedbacks,
        })
    }

    fn initialize_cursor_plane_slots(&mut self, drm_crtc_id: u32) {
        let Some(plane_format) = self.cursor_plane_format(drm_crtc_id) else {
            return;
        };
        let (fb_width, fb_height) = self.fixed_cursor_plane_size();
        let slot_count = self
            .output(drm_crtc_id)
            .map(|output| output.cursor_slots.len())
            .unwrap_or(0);
        for slot in 0..slot_count {
            let Some(buffer) = self.allocate_cursor_plane_buffer(&plane_format) else {
                log::warn!(
                    "Failed to preallocate DRM cursor plane slot {} at {}x{}, falling back to software cursor composition",
                    slot,
                    fb_width,
                    fb_height
                );
                if let Some(output) = self.output_mut(drm_crtc_id) {
                    output.cursor_slots = [None, None];
                }
                return;
            };
            log::debug!(
                "Preallocated DRM cursor plane slot: slot={} fb_size={}x{} fb={:?}",
                slot,
                fb_width,
                fb_height,
                buffer.framebuffer.id()
            );
            self.output_mut(drm_crtc_id).unwrap().cursor_slots[slot] =
                Some(CachedCursorPlaneSlot { content_key: None, buffer });
        }
    }

    fn cursor_plane_format(&self, drm_crtc_id: u32) -> Option<PlaneFormat> {
        let plane = self.output(drm_crtc_id)?.in_use.cursor_plane()?;
        let plane_format = plane.formats.get(&DrmFourcc::Argb8888)?.clone();
        if !plane_format.modifiers.contains(&Modifier::INVALID)
            && !plane_format.modifiers.contains(&Modifier::LINEAR)
            && !plane_format.modifiers.contains(&Modifier::ZERO)
        {
            log::debug!(
                "Skipping DRM cursor plane initialization: plane modifiers {:?} do not accept linear ARGB8888 cursor buffers",
                plane_format.modifiers
            );
            return None;
        }
        Some(plane_format)
    }

    fn fixed_cursor_plane_size(&self) -> (u32, u32) {
        // virtio rejects AddFB2 framebuffers smaller than 32x32 even for cursor planes.
        let min_cursor_fb_size = if self.driver.name == "virtio_gpu" { 32 } else { 1 };
        let cursor_width = u32::try_from(self.resources.cursor_width).unwrap_or(u32::MAX);
        let cursor_height = u32::try_from(self.resources.cursor_height).unwrap_or(u32::MAX);
        (cursor_width.max(min_cursor_fb_size), cursor_height.max(min_cursor_fb_size))
    }

    fn upload_cursor_plane_slot(
        &mut self,
        drm_crtc_id: u32,
        slot: usize,
        request: &CursorPlaneRequest,
    ) -> Option<()> {
        let (_, fb_height) = self.fixed_cursor_plane_size();
        let cached = self.output_mut(drm_crtc_id)?.cursor_slots[slot]
            .as_mut()
            .unwrap();
        if cached.content_key != Some(request.content_key) {
            let pitch = drm::buffer::Buffer::pitch(&cached.buffer.bo) as usize;
            let bytes = match &request.upload {
                CursorPlaneUploadSource::Named { image } => {
                    let image = unsafe { image.as_ref() }?;
                    rgba_to_drm_argb8888(
                        &image.pixels_rgba,
                        image.width as usize,
                        image.height as usize,
                        pitch,
                        fb_height as usize,
                    )
                }
                CursorPlaneUploadSource::SurfaceShm { buffer } => {
                    buffer.copy_to_drm_argb8888_cursor_bytes(pitch, fb_height as usize)
                }
            };
            cached.buffer.bo.write(&bytes);
            cached.content_key = Some(request.content_key);
            log::debug!(
                "Updated cached DRM cursor plane slot pixels: slot={} fb={:?}",
                slot,
                cached.buffer.framebuffer.id()
            );
        } else {
            log::debug!(
                "Reused cached DRM cursor plane slot without pixel upload: slot={} fb={:?}",
                slot,
                cached.buffer.framebuffer.id()
            );
        }
        Some(())
    }

    fn allocate_cursor_plane_buffer(
        &self,
        plane_format: &PlaneFormat,
    ) -> Option<CursorPlaneBuffer> {
        let (fb_width, fb_height) = self.fixed_cursor_plane_size();
        let bo = self.egl_context.gbm.create_bo_with_size_no_export(
            fb_width,
            fb_height,
            DrmFourcc::Argb8888,
            UseFlags::CURSOR | UseFlags::WRITE,
            &[],
        );
        let bo_modifier = bo.modifier();
        if !plane_format.modifiers.contains(&Modifier::INVALID)
            && !plane_format.modifiers.contains(&bo_modifier)
        {
            log::debug!(
                "Skipping DRM cursor plane candidate: plane modifiers {:?} do not accept cursor BO modifier {:?}",
                plane_format.modifiers,
                bo_modifier
            );
            return None;
        }
        let pitch = drm::buffer::Buffer::pitch(&bo) as usize;
        bo.write(&vec![0; pitch.saturating_mul(fb_height as usize)]);
        let framebuffer = match Self::add_framebuffer_for_bo(&self.fd, &bo) {
            Ok(framebuffer) => framebuffer,
            Err(error) => {
                log::debug!(
                    "Skipping DRM cursor plane candidate: failed to create framebuffer for {}x{} cursor BO: {:?}",
                    fb_width,
                    fb_height,
                    error
                );
                return None;
            }
        };
        Some(CursorPlaneBuffer { bo, framebuffer })
    }

    fn list_resources(drm: &DrmFd) -> DrmResources {
        let resources = drm.resource_handles().unwrap();

        let (cursor_height, cursor_width) = {
            use drm::DriverCapability::{CursorHeight, CursorWidth};

            let height = drm.get_driver_capability(CursorHeight).unwrap_or(64);
            let width = drm.get_driver_capability(CursorWidth).unwrap_or(64);
            (height, width)
        };

        let plane_handles = drm.plane_handles().unwrap().to_vec();
        log::info!("DRM plane handles reported by kernel: count={}", plane_handles.len());

        let make_plane = |plane: plane::Handle| Self::make_plane(drm, plane);
        let planes: BTreeMap<plane::Handle, Plane> =
            plane_handles.iter().copied().map(make_plane).collect();
        for plane in planes.values() {
            let kind = match plane.kind {
                PlaneType::Primary => "Primary",
                PlaneType::Cursor => "Cursor",
                PlaneType::Overlay => "Overlay",
            };
            log::info!(
                "Discovered DRM plane: handle={:?} kind={} possible_crtcs=0x{:x} current_crtc={:?} current_fb={:?} argb8888={} xrgb8888={}",
                plane.handle,
                kind,
                plane.possible_crtcs,
                plane.crtc_handle,
                plane.fb_handle,
                plane.formats.contains_key(&DrmFourcc::Argb8888),
                plane.formats.contains_key(&DrmFourcc::Xrgb8888)
            );
        }

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

    fn init_drm_outputs(
        drm: &DrmFd,
        egl: &EglContext,
        drm_resources: &DrmResources,
    ) -> BTreeMap<u32, DrmOutput> {
        let DrmResources { planes, crtcs, connectors, .. } = drm_resources;
        let outputs = Self::find_valid_connectors(connectors, crtcs, planes)
            .into_iter()
            .map(|in_use| {
                let drm_crtc_id = in_use.crtc_id();
                let output = Self::init_drm_output(drm, egl, in_use);
                (drm_crtc_id, output)
            })
            .collect::<BTreeMap<_, _>>();
        assert!(!outputs.is_empty(), "Failed to find connector");
        outputs
    }

    fn init_drm_output(drm: &DrmFd, egl: &EglContext, in_use: ConnectorInUse) -> DrmOutput {
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
            color_state: OutputColorState::default(),
            presentation_transform: OutputPresentationTransform::FlipY,
            render_sync_mode,
            pipeline: output_pipeline::OutputPipeline::new(buffers),
            cursor_slots: [None, None],
            cursor_plane_state: CursorPlaneState::Ready,
            cursor_animation_delay_msec: None,
            allow_in_fence_fd: true,
            allow_fb_damage_clips: true,
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

        let mut flags = UseFlags::RENDERING | UseFlags::SCANOUT;
        if is_cursor {
            flags |= UseFlags::LINEAR;
        }

        let bo = egl.gbm.create_bo(&mode, flags, &modifiers);
        let dmabuf = bo
            .dmabuf()
            .expect("render buffer GBM BO must export dmabuf");
        let drm_fb = match Self::add_framebuffer(drm, dmabuf) {
            Ok(framebuffer) => framebuffer,
            Err(error) => {
                log::error!(
                    "Failed to create DRM framebuffer: size={}x{} format={} modifier={:?} flags={:?} error={:?}",
                    dmabuf.width,
                    dmabuf.height,
                    dmabuf.format.name,
                    dmabuf.modifier,
                    flags,
                    error
                );
                return Err(error);
            }
        };
        let image = egl.import_dmabuf(dmabuf);
        let gl_framebuffer = egl.make_fb_from_image(&image, dmabuf);
        gl_framebuffer.clear();
        let texture = image.into_texture().unwrap();

        Ok(RenderBuffer { bo, drm_fb, gl_framebuffer, texture })
    }

    fn add_framebuffer_for_bo(drm: &DrmFd, bo: &BufferObject) -> std::io::Result<DrmFrameBuffer> {
        let flags = match bo.modifier() {
            Modifier::INVALID | Modifier::LINEAR => FbCmd2Flags::empty(),
            _ => FbCmd2Flags::MODIFIERS,
        };

        match drm.add_planar_framebuffer(bo, flags) {
            Ok(handle) => {
                log::debug!(
                    "Created DRM framebuffer directly from GBM BO: handle={:?} size={:?} format={:?} flags={:?}",
                    handle,
                    drm::buffer::Buffer::size(bo),
                    drm::buffer::Buffer::format(bo),
                    flags
                );
                Ok(DrmFrameBuffer { drm: drm.clone(), handle })
            }
            Err(source) => {
                log::debug!(
                    "Direct DRM framebuffer creation from GBM BO failed: size={:?} format={:?} planes={} modifier={:?} flags={:?} error={:?}",
                    drm::buffer::Buffer::size(bo),
                    drm::buffer::Buffer::format(bo),
                    bo.plane_count(),
                    bo.modifier(),
                    flags,
                    source
                );
                if bo.plane_count() == 1 && drm::buffer::Buffer::format(bo) == DrmFourcc::Argb8888 {
                    drm.add_framebuffer(bo, 32, 32).map(|handle| {
                        log::debug!(
                            "Created legacy DRM framebuffer from GBM BO: handle={:?} size={:?} format={:?}",
                            handle,
                            drm::buffer::Buffer::size(bo),
                            drm::buffer::Buffer::format(bo)
                        );
                        DrmFrameBuffer { drm: drm.clone(), handle }
                    })
                } else {
                    Err(source)
                }
            }
        }
    }

    fn add_framebuffer(drm: &DrmFd, dmabuf: &DmaBuf) -> std::io::Result<DrmFrameBuffer> {
        let mut modifier = Modifier::ZERO;
        let mut flags = FbCmd2Flags::empty();
        if dmabuf.modifier != Modifier::INVALID && dmabuf.modifier != Modifier::LINEAR {
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

    fn find_valid_connectors(
        connectors: &BTreeMap<connector::Handle, Connector>,
        crtcs: &BTreeMap<crtc::Handle, Crtc>,
        planes: &BTreeMap<plane::Handle, Plane>,
    ) -> Vec<ConnectorInUse> {
        let mut claimed_crtcs = BTreeSet::new();
        let mut outputs = Vec::new();

        for connector in connectors.values() {
            let connector::State::Connected = connector.state else {
                continue;
            };

            let Some(crtc) = connector.crtcs.iter().find_map(|crtc_handle| {
                let crtc = crtcs.get(crtc_handle)?;
                (crtc.active && !claimed_crtcs.contains(crtc_handle)).then_some(crtc)
            }) else {
                continue;
            };

            let planes = crtc
                .possible_planes
                .iter()
                .map(|handle| planes.get(handle).cloned().unwrap())
                .collect::<Vec<_>>();

            let Some(primary_plane_index) = planes.iter().position(|plane| {
                let PlaneType::Primary = plane.kind else { return false };
                plane.formats.contains_key(&DrmFourcc::Xrgb8888)
            }) else {
                continue;
            };

            claimed_crtcs.insert(crtc.handle);
            outputs.push(ConnectorInUse {
                crtc: crtc.clone(),
                connector: connector.clone(),
                planes,
                primary_plane_index,
            });
        }

        outputs
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
            color: CrtcColorProperties {
                gamma_lut: props.get("GAMMA_LUT").map(|(_, info)| info.handle()),
                gamma_lut_size: props
                    .get("GAMMA_LUT_SIZE")
                    .map(|(value, _)| *value as u32)
                    .unwrap_or(0),
                ctm: props.get("CTM").map(|(_, info)| info.handle()),
            },
            vrr_enabled_prop: props.get("VRR_ENABLED").map(|(_, info)| info.handle()),
            vrr_enabled: props
                .get("VRR_ENABLED")
                .map(|(value, _)| *value != 0)
                .unwrap_or(false),
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
        let fb_damage_clips = props.get("FB_DAMAGE_CLIPS").map(|(_, info)| info.handle());

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
            fb_damage_clips,
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

        // dbg!(&edid);

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
            vrr_capable: props
                .get("vrr_capable")
                .map(|(value, _)| *value != 0)
                .unwrap_or(false),
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
    Ignored { len: usize, type_: u32 },
}

impl ParsedDrmEvent {
    fn len(&self) -> usize {
        match self {
            Self::FlipComplete { len, .. } | Self::Ignored { len, .. } => *len,
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
            log::error!(
                "drm reader invalid event header: event_type={} event_len={} header_len={}",
                header.type_,
                len,
                header_len,
            );
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
            _ => Ok(Some(ParsedDrmEvent::Ignored { len, type_: header.type_ })),
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
pub struct DrmReader {
    drm_id: DrmId,
    fd: DrmFd,
    read_buffer: DrmBuffer,
    poll_op_id: Option<RingId>,
    read_op_id: Option<RingId>,
}

impl ModelInstanceId for DrmId {
    type Model = DrmReader;

    fn into_raw(self) -> u64 {
        self.as_u64()
    }
}

#[derive(Debug)]
pub enum DrmReaderAction {
    Start,
    PollReady,
    ReadChunk { buffer: Box<[u8]> },
    ReadDrained,
    ReadError { error: std::io::Error },
    Stop,
}

impl Model for DrmReader {
    type Action = DrmReaderAction;
    const START: Option<Self::Action> = Some(DrmReaderAction::Start);
    const STOP: Option<Self::Action> = Some(DrmReaderAction::Stop);

    fn process(
        &mut self,
        state: &mut crate::state::State,
        action: Self::Action,
        dispatcher: &mut crate::runner::Dispatcher,
    ) -> Result<(), ()> {
        use DrmReaderAction::*;

        match action {
            Start => {
                self.dispatch_poll(dispatcher);
            }
            PollReady => {
                if self.read_op_id.is_some() {
                    return Ok(());
                }
                self.dispatch_read_some(dispatcher);
            }
            ReadChunk { buffer } => {
                self.read_op_id = None;
                self.read_buffer.append(&buffer);
                loop {
                    let event = match self.read_buffer.next_event() {
                        Ok(Some(event)) => event,
                        Ok(None) => {
                            self.dispatch_read_some(dispatcher);
                            return Ok(());
                        }
                        Err(()) => {
                            log::error!("Invalid DRM event stream");
                            self.dispatch_read_some(dispatcher);
                            return Ok(());
                        }
                    };

                    let len = event.len();
                    match event {
                        ParsedDrmEvent::FlipComplete {
                            sequence, tv_sec, tv_usec, crtc_id, ..
                        } => {
                            if let Some(output_id) =
                                state.output_id_for_drm_crtc(self.drm_id, crtc_id)
                            {
                                dispatcher.dispatch_to(
                                    output_id,
                                    DrmAction::FlipComplete { sequence, tv_sec, tv_usec, crtc_id },
                                );
                            }
                        }
                        ParsedDrmEvent::Ignored { .. } => {}
                    }
                    self.read_buffer.consume_event(len);
                }
            }
            ReadDrained => {
                self.read_op_id = None;
            }
            ReadError { error } => {
                self.read_op_id = None;
                log::error!("Could not read DRM events: {:?}", error);
            }
            Stop => {
                if let Some(poll_op_id) = self.poll_op_id.take() {
                    dispatcher.dispatch(IoUringAction::PollRemove { target: poll_op_id });
                }
                if let Some(read_op_id) = self.read_op_id.take() {
                    dispatcher.dispatch(IoUringAction::Cancel { target: read_op_id });
                }
            }
        };

        Ok(())
    }
}

impl DrmReader {
    pub fn new(drm_id: DrmId, fd: DrmFd) -> Self {
        Self {
            drm_id,
            fd,
            read_buffer: DrmBuffer::new(),
            poll_op_id: None,
            read_op_id: None,
        }
    }

    fn model_id(&self) -> DrmId {
        self.drm_id
    }

    fn dispatch_poll(&mut self, dispatcher: &mut Dispatcher) {
        use crate::runner::Redispatcher;

        let poll_op_id = RingId::fresh();
        self.poll_op_id = Some(poll_op_id);
        dispatcher.dispatch(IoUringAction::Poll {
            id: Some(poll_op_id),
            fd: self.fd.as_raw_fd(),
            dispatch_to: Redispatcher::to_instance(self.model_id(), |result| match result {
                Ok(PollCompleted {}) => DrmReaderAction::PollReady,
                Err(error) => DrmReaderAction::ReadError { error },
            }),
        });
    }

    fn dispatch_read_some(&mut self, dispatcher: &mut Dispatcher) {
        use crate::runner::Redispatcher;

        let read_op_id = RingId::fresh();
        self.read_op_id = Some(read_op_id);
        dispatcher.dispatch(IoUringAction::ReadSome {
            id: Some(read_op_id),
            fd: self.fd.as_raw_fd(),
            offset: 0,
            length: 4096,
            dispatch_to: Redispatcher::to_instance(
                self.model_id(),
                |result: std::io::Result<ReadWriteCompleted>| match result {
                    Ok(ReadWriteCompleted { buffer, .. }) => DrmReaderAction::ReadChunk { buffer },
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        DrmReaderAction::ReadDrained
                    }
                    Err(error) => DrmReaderAction::ReadError { error },
                },
            ),
        });
    }
}

/// DRM compositor-side per-output model: scheduling, rendering, and flip completion.
pub struct DrmOutputModel {
    drm_id: DrmId,
    output_id: OutputId,
}

impl ModelInstanceId for OutputId {
    type Model = DrmOutputModel;

    fn into_raw(self) -> u64 {
        self.as_u64()
    }
}

pub enum DrmAction {
    Start,
    RenderFrame { decision: ClockDecision },
    FlipComplete { sequence: u32, tv_sec: u32, tv_usec: u32, crtc_id: u32 },
    FlipWatchdog,
    SessionChanged { active: bool },
    Stop,
}

pub enum WindowAnimationManagerAction {
    Start,
    Kick,
    KickOutput { output_id: OutputId },
    Stop,
}

pub struct WindowAnimationManager;

pub fn sync_output_nodes_for_backend(state: &mut State, drm_id: DrmId) {
    state.refresh_outputs_for_backend(BackendOutputRef::new(drm_id));
}

pub fn request_output_frame(
    state: &State,
    output_id: OutputId,
    reason: RequestFrameReason,
    dispatcher: &mut Dispatcher,
) {
    if state.backend_for_output(output_id).is_none() {
        log::warn!("schedule_output: invalid logical output {:?}", output_id);
        return;
    }
    dispatcher.dispatch_to(
        output_scheduler_id(output_id),
        OutputSchedulerAction::RequestFrame { reason },
    );
}

pub fn request_output(state: &State, output_id: OutputId, dispatcher: &mut Dispatcher) {
    request_output_frame(state, output_id, RequestFrameReason::Damage, dispatcher);
}

pub fn schedule_output(state: &State, output_id: OutputId, dispatcher: &mut Dispatcher) {
    request_output(state, output_id, dispatcher);
}

pub fn request_active_output(state: &State, dispatcher: &mut Dispatcher) {
    let Some(output_id) = state.active_output_id() else {
        log::warn!("schedule_active_output: no active logical output");
        return;
    };

    request_output(state, output_id, dispatcher);
}

pub fn schedule_active_output(state: &State, dispatcher: &mut Dispatcher) {
    request_active_output(state, dispatcher);
}

fn dispatch_gamma_control_failed_if_pending(
    drm: &Drm,
    drm_crtc_id: u32,
    output_id: Option<OutputId>,
    dispatcher: &mut Dispatcher,
) {
    let Some(output_id) = output_id else {
        return;
    };
    if drm
        .output(drm_crtc_id)
        .is_some_and(|output| output.color_state.gamma_dirty)
    {
        dispatcher.dispatch(ClientManagerAction::GammaControlFailed { output_id });
    }
}

pub fn request_outputs<I>(state: &State, output_ids: I, dispatcher: &mut Dispatcher)
where
    I: IntoIterator<Item = OutputId>,
{
    let mut unique = BTreeSet::new();
    for output_id in output_ids {
        if unique.insert(output_id) {
            request_output_frame(state, output_id, RequestFrameReason::Damage, dispatcher);
        }
    }
}

pub fn schedule_outputs<I>(state: &State, output_ids: I, dispatcher: &mut Dispatcher)
where
    I: IntoIterator<Item = OutputId>,
{
    request_outputs(state, output_ids, dispatcher);
}

pub fn request_outputs_for_rect(state: &State, rect: RectI, dispatcher: &mut Dispatcher) {
    let output_ids = state.outputs_intersecting_rect(rect);
    if output_ids.is_empty() {
        if let Some(output_id) = state.primary_output_for_rect(rect, None) {
            request_output(state, output_id, dispatcher);
        }
        return;
    }
    request_outputs(state, output_ids, dispatcher);
}

pub fn schedule_outputs_for_rect(state: &State, rect: RectI, dispatcher: &mut Dispatcher) {
    request_outputs_for_rect(state, rect, dispatcher);
}

pub fn request_outputs_for_rects<I>(state: &State, rects: I, dispatcher: &mut Dispatcher)
where
    I: IntoIterator<Item = RectI>,
{
    let mut output_ids = BTreeSet::new();
    for rect in rects {
        let rect_outputs = state.outputs_intersecting_rect(rect);
        if rect_outputs.is_empty() {
            if let Some(output_id) = state.primary_output_for_rect(rect, None) {
                output_ids.insert(output_id);
            }
            continue;
        }
        output_ids.extend(rect_outputs);
    }
    request_outputs(state, output_ids, dispatcher);
}

pub fn schedule_outputs_for_rects<I>(state: &State, rects: I, dispatcher: &mut Dispatcher)
where
    I: IntoIterator<Item = RectI>,
{
    request_outputs_for_rects(state, rects, dispatcher);
}

pub fn request_outputs_for_points(
    state: &State,
    points: impl IntoIterator<Item = (f64, f64)>,
    dispatcher: &mut Dispatcher,
) {
    let output_ids = points.into_iter().filter_map(|(x, y)| {
        let (x, y) = state.clamp_point_to_outputs(x.round() as i32, y.round() as i32)?;
        state.output_at_point(x, y)
    });
    request_outputs(state, output_ids, dispatcher);
}

pub fn schedule_outputs_for_points(
    state: &State,
    points: impl IntoIterator<Item = (f64, f64)>,
    dispatcher: &mut Dispatcher,
) {
    request_outputs_for_points(state, points, dispatcher);
}

pub fn kick_window_animation_manager(dispatcher: &mut Dispatcher) {
    dispatcher.dispatch(WindowAnimationManagerAction::Kick);
}

fn animation_delay_msec_for_output(state: &State, output_id: OutputId) -> Option<u32> {
    let refresh_millihz = state
        .outputs
        .try_get(output_id)
        .map(|output| output.current_mode.refresh_millihz.max(0) as u32)
        .filter(|refresh| *refresh > 0)?;
    let interval_ns = (1_000_000_000_000u64 / u64::from(refresh_millihz)).max(1);
    Some(((interval_ns + 999_999) / 1_000_000).max(1) as u32)
}

fn animation_delay_msec_for_rects(state: &State, rects: &[RectI]) -> Option<u32> {
    let mut output_ids = BTreeSet::new();
    for &rect in rects {
        let rect_outputs = state.outputs_intersecting_rect(rect);
        if rect_outputs.is_empty() {
            if let Some(output_id) = state.primary_output_for_rect(rect, None) {
                output_ids.insert(output_id);
            }
            continue;
        }
        output_ids.extend(rect_outputs);
    }
    output_ids
        .into_iter()
        .filter_map(|output_id| animation_delay_msec_for_output(state, output_id))
        .min()
}

fn active_animation_output_ids(state: &State) -> BTreeSet<OutputId> {
    let mut output_ids = BTreeSet::new();
    for (&surface, managed) in &state.managed_root_surfaces {
        if !managed.visual_animation.is_animating() {
            continue;
        }
        let Some(rect) = state.root_surface_animation_damage_rect(surface) else {
            continue;
        };
        let rect_outputs = state.outputs_intersecting_rect(rect);
        if rect_outputs.is_empty() {
            if let Some(output_id) = state.primary_output_for_rect(rect, None) {
                output_ids.insert(output_id);
            }
            continue;
        }
        output_ids.extend(rect_outputs);
    }
    output_ids
}

pub fn discard_backend_presentation_feedbacks(
    state: &mut State,
    drm_id: DrmId,
    dispatcher: &mut Dispatcher,
) {
    let discarded_damage_blobs = {
        let drm = state.drm.get_mut(drm_id);
        drm.outputs
            .values_mut()
            .map(|output| output.take_in_flight_presentation_feedbacks_for_discard().1)
            .collect::<Vec<_>>()
    };
    let output_ids = state.output_ids_for_backend(BackendOutputRef::new(drm_id));
    let mut discarded_feedbacks = Vec::new();
    for output_id in output_ids {
        discarded_feedbacks.extend(state.presentation.discard_timeline(output_id));
    }
    for damage_blob in discarded_damage_blobs {
        if let Some(blob) = damage_blob {
            let drm = state.drm.get(drm_id);
            let _ = drm.fd.destroy_property_blob(blob);
        }
    }
    if discarded_feedbacks.is_empty() {
        return;
    }
    PresentationSubsystem::dispatch_discarded(dispatcher, &discarded_feedbacks);
}

impl Model for WindowAnimationManager {
    type Action = WindowAnimationManagerAction;
    const START: Option<Self::Action> = Some(WindowAnimationManagerAction::Start);
    const STOP: Option<Self::Action> = Some(WindowAnimationManagerAction::Stop);

    fn process(
        &mut self,
        state: &mut crate::state::State,
        action: Self::Action,
        dispatcher: &mut crate::runner::Dispatcher,
    ) -> Result<(), ()> {
        match action {
            WindowAnimationManagerAction::Start => {}
            WindowAnimationManagerAction::Kick => {
                self.advance_and_schedule(state, dispatcher, monotonic_time_msec());
            }
            WindowAnimationManagerAction::KickOutput { output_id } => {
                self.advance_and_schedule_output(
                    state,
                    dispatcher,
                    monotonic_time_msec(),
                    output_id,
                );
            }
            WindowAnimationManagerAction::Stop => {}
        }
        Ok(())
    }
}

impl WindowAnimationManager {
    pub fn new() -> Self {
        Self
    }

    fn advance_and_schedule(
        &mut self,
        state: &mut State,
        dispatcher: &mut Dispatcher,
        frame_time_msec: u32,
    ) {
        let result = state.advance_window_visual_animations(frame_time_msec);
        let has_no_damage = result.damaged_rects.is_empty();
        if !result.damaged_rects.is_empty() {
            for rect in result.damaged_rects {
                let output_ids = state.outputs_intersecting_rect(rect);
                if output_ids.is_empty() {
                    if let Some(output_id) = state.primary_output_for_rect(rect, None) {
                        request_output_frame(
                            state,
                            output_id,
                            RequestFrameReason::Animation,
                            dispatcher,
                        );
                    }
                    continue;
                }
                for output_id in output_ids {
                    request_output_frame(
                        state,
                        output_id,
                        RequestFrameReason::Animation,
                        dispatcher,
                    );
                }
            }
        }
        if result.has_active_animations && has_no_damage {
            let output_ids = active_animation_output_ids(state);
            for output_id in output_ids {
                let delay_msec = animation_delay_msec_for_output(state, output_id);
                dispatcher.dispatch_to(
                    output_scheduler_id(output_id),
                    OutputSchedulerAction::ScheduleAnimation { delay_msec },
                );
            }
        }
    }

    fn advance_and_schedule_output(
        &mut self,
        state: &mut State,
        dispatcher: &mut Dispatcher,
        frame_time_msec: u32,
        output_id: OutputId,
    ) {
        let result = state.advance_window_visual_animations_for_output(output_id, frame_time_msec);
        if !result.damaged_rects.is_empty() {
            request_output_frame(state, output_id, RequestFrameReason::Animation, dispatcher);
            return;
        }
        if result.has_active_animations {
            let delay_msec = animation_delay_msec_for_output(state, output_id);
            dispatcher.dispatch_to(
                output_scheduler_id(output_id),
                OutputSchedulerAction::ScheduleAnimation { delay_msec },
            );
        }
    }
}

impl Model for DrmOutputModel {
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
            DrmAction::Start => {}
            DrmAction::RenderFrame { decision } => {
                self.render_frame(state, dispatcher, decision);
            }
            DrmAction::FlipComplete { sequence, tv_sec, tv_usec, crtc_id } => {
                self.handle_flip_complete(state, dispatcher, sequence, tv_sec, tv_usec, crtc_id);
            }
            DrmAction::FlipWatchdog => {
                self.handle_flip_watchdog(state, dispatcher);
            }
            DrmAction::SessionChanged { active } => {
                self.handle_session_changed(state, dispatcher, active);
            }
            DrmAction::Stop => {}
        };
        Ok(())
    }
}

impl DrmOutputModel {
    fn model_id(&self) -> OutputId {
        self.output_id
    }

    fn render_frame(
        &mut self,
        state: &mut crate::state::State,
        dispatcher: &mut Dispatcher,
        decision: ClockDecision,
    ) {
        let Some(drm_crtc_id) = state.output_crtc_id(self.output_id) else {
            return;
        };
        let frame_time_msec = monotonic_time_msec();
        let ready = {
            let drm = state.drm.get_mut(self.drm_id);
            if let Some(output) = drm.output_mut(drm_crtc_id) {
                output.begin_render()
            } else {
                false
            }
        };
        let failed_releases = {
            if !ready {
                dispatcher.dispatch_to(
                    output_scheduler_id(self.output_id),
                    OutputSchedulerAction::RenderAttempted { started: false },
                );
                return;
            }
            dispatcher.dispatch_to(
                output_scheduler_id(self.output_id),
                OutputSchedulerAction::RenderAttempted { started: true },
            );
            let planner_started_at = Instant::now();
            let watchdog_deadline = flip_watchdog_deadline_for_output(state, self.output_id);
            let scene = state.build_scene_model();
            let output_render_plan = render_planner::collect_output_render_plan(
                state,
                self.drm_id,
                Some(self.output_id),
                frame_time_msec,
                &scene,
            );
            let scene_build_duration = Instant::now().saturating_duration_since(planner_started_at);
            let OutputRenderPlan {
                output_id,
                global_bounds: _,
                local_origin: _,
                deferred_releases,
                cursor_animation_delay_msec,
                frame,
            } = output_render_plan;
            let FramePlan {
                mut items,
                mut software_cursor_items,
                mut frame_elements,
                mut scene,
                mut frame_callback_surfaces,
                mut presentation_feedbacks,
                mut damage,
                direct_scanout,
                mut cursor_plane,
                mut preserve_contents,
            } = frame;
            let drm = state.drm.get_mut(self.drm_id);
            let target_vrr_enabled =
                matches!(decision.mode, crate::scheduler::frame_clock::FrameClockMode::Variable)
                    && drm.output_vrr_capable(drm_crtc_id);
            if let Err(error) = drm.set_output_vrr_enabled(drm_crtc_id, target_vrr_enabled) {
                log::debug!(
                    "failed to update VRR state for output {:?}: {:?}",
                    self.output_id,
                    error
                );
            }
            if let Some(output) = state.outputs.try_get_mut(self.output_id) {
                output.vrr_enabled = drm.output_vrr_enabled(drm_crtc_id);
            }
            drm.output_mut(drm_crtc_id)
                .unwrap()
                .pipeline
                .set_in_flight_releases(deferred_releases);
            drm.output_mut(drm_crtc_id)
                .unwrap()
                .cursor_animation_delay_msec = cursor_animation_delay_msec;

            if let Some(candidate) = direct_scanout {
                match kms_submit::submit_direct_scanout(
                    drm,
                    drm_crtc_id,
                    candidate,
                    output_id,
                    scene.clone(),
                    frame_callback_surfaces.clone(),
                    presentation_feedbacks.clone(),
                    cursor_plane.take(),
                    scene_build_duration,
                    Instant::now(),
                ) {
                    Ok(registration) => {
                        state.presentation.register_submission(
                            self.output_id,
                            crate::scheduler::presentation::SubmissionRecord {
                                submission_id: registration.submission_id,
                                output_id: registration.output_id,
                                feedbacks: registration.presentation_feedbacks,
                                zero_copy: registration.zero_copy,
                            },
                        );
                        drm.output_mut(drm_crtc_id)
                            .unwrap()
                            .note_cursor_plane_success();
                        dispatch_submission_frame_callbacks(
                            &registration.callback_surfaces,
                            frame_time_msec,
                            dispatcher,
                        );
                        dispatcher.dispatch_to(
                            output_scheduler_id(self.output_id),
                            OutputSchedulerAction::PipelineBlocked { watchdog_deadline },
                        );
                        drm.output_mut(drm_crtc_id)
                            .unwrap()
                            .pipeline
                            .set_force_full_redraw(true);
                        return;
                    }
                    Err(_) => {}
                }
                if !software_cursor_items.is_empty() {
                    items.append(&mut software_cursor_items);
                    // Clone the effect recipe on cursor fallback to keep the retry path simple;
                    // if fallback becomes hot we should switch this to recipe reuse.
                    let fallback_plan = render_planner::build_frame_plan(
                        drm.output(drm_crtc_id).unwrap(),
                        items,
                        Vec::new(),
                        frame_elements.clone(),
                        None,
                    );
                    items = fallback_plan.items;
                    software_cursor_items = fallback_plan.software_cursor_items;
                    frame_elements = fallback_plan.frame_elements;
                    scene = fallback_plan.scene;
                    frame_callback_surfaces = fallback_plan.frame_callback_surfaces;
                    presentation_feedbacks = fallback_plan.presentation_feedbacks;
                    damage = fallback_plan.damage;
                    preserve_contents = fallback_plan.preserve_contents;
                    cursor_plane = None;
                }
            }

            if damage.is_empty()
                && cursor_plane.is_none()
                && !drm.output(drm_crtc_id).unwrap().has_displayed_cursor_slot()
            {
                drm.output_mut(drm_crtc_id).unwrap().mark_frame_idle();
                let releases = drm
                    .output_mut(drm_crtc_id)
                    .unwrap()
                    .pipeline
                    .take_in_flight_releases();
                dispatch_submission_frame_callbacks(
                    &frame_callback_surfaces,
                    frame_time_msec,
                    dispatcher,
                );
                dispatcher.dispatch_to(
                    output_scheduler_id(self.output_id),
                    OutputSchedulerAction::FrameCompleted(FrameCompletionOutcome::NoPresent),
                );
                Some(releases)
            } else {
                let recipe = build_output_frame_recipe(&items, &frame_elements);
                let render_started_at = Instant::now();
                let render_completion =
                    drm.render_output(drm_crtc_id, &recipe, &damage, preserve_contents);
                let submit_had_cursor_plane = cursor_plane.is_some();
                if submit_had_cursor_plane && crate::logger::ENABLE_NOISY_LOG {
                    log::debug!("Submitting DRM frame with cursor plane enabled");
                }
                let submit_result = kms_submit::submit_render_buffer(
                    drm,
                    drm_crtc_id,
                    &render_completion,
                    &damage,
                    output_id,
                    scene,
                    frame_callback_surfaces,
                    presentation_feedbacks.clone(),
                    cursor_plane,
                    recipe.has_effects(),
                    scene_build_duration,
                    render_started_at,
                );
                if let Err(error) = submit_result {
                    if submit_had_cursor_plane {
                        if !software_cursor_items.is_empty() {
                            log::warn!(
                                "DRM cursor plane submission failed, retrying with software cursor: {:?}",
                                error
                            );
                            items.append(&mut software_cursor_items);
                            let fallback_plan = render_planner::build_frame_plan(
                                drm.output(drm_crtc_id).unwrap(),
                                items,
                                Vec::new(),
                                frame_elements.clone(),
                                None,
                            );
                            let fallback_recipe = build_output_frame_recipe(
                                &fallback_plan.items,
                                &fallback_plan.frame_elements,
                            );
                            let fallback_render_completion = drm.render_output(
                                drm_crtc_id,
                                &fallback_recipe,
                                &fallback_plan.damage,
                                fallback_plan.preserve_contents,
                            );
                            match kms_submit::submit_render_buffer(
                                drm,
                                drm_crtc_id,
                                &fallback_render_completion,
                                &fallback_plan.damage,
                                output_id,
                                fallback_plan.scene,
                                fallback_plan.frame_callback_surfaces,
                                fallback_plan.presentation_feedbacks.clone(),
                                None,
                                fallback_recipe.has_effects(),
                                scene_build_duration,
                                render_started_at,
                            ) {
                                Ok(registration) => {
                                    state.presentation.register_submission(
                                        self.output_id,
                                        crate::scheduler::presentation::SubmissionRecord {
                                            submission_id: registration.submission_id,
                                            output_id: registration.output_id,
                                            feedbacks: registration.presentation_feedbacks,
                                            zero_copy: registration.zero_copy,
                                        },
                                    );
                                    drm.output_mut(drm_crtc_id)
                                        .unwrap()
                                        .note_cursor_plane_failure();
                                    dispatch_submission_frame_callbacks(
                                        &registration.callback_surfaces,
                                        frame_time_msec,
                                        dispatcher,
                                    );
                                    dispatcher.dispatch_to(
                                        output_scheduler_id(self.output_id),
                                        OutputSchedulerAction::PipelineBlocked {
                                            watchdog_deadline,
                                        },
                                    );
                                    None
                                }
                                Err(fallback_error) => {
                                    dispatch_gamma_control_failed_if_pending(
                                        drm,
                                        drm_crtc_id,
                                        output_id,
                                        dispatcher,
                                    );
                                    drm.output_mut(drm_crtc_id).unwrap().mark_render_failed();
                                    let failed_releases = drm
                                        .output_mut(drm_crtc_id)
                                        .unwrap()
                                        .pipeline
                                        .take_in_flight_releases();
                                    discard_frame_for_timeline(
                                        dispatcher,
                                        &[crate::scheduler::presentation::DiscardedFeedback {
                                            feedbacks: fallback_plan.presentation_feedbacks.clone(),
                                        }],
                                        None,
                                        |_| {},
                                        |dispatcher, completion| {
                                            dispatcher.dispatch_to(
                                                output_scheduler_id(self.output_id),
                                                OutputSchedulerAction::FrameCompleted(completion),
                                            );
                                        },
                                    );
                                    if !fallback_render_completion.used_blocking_fallback {
                                        if let Some(in_fence_fd) =
                                            fallback_render_completion.in_fence_fd
                                        {
                                            if let Err(wait_error) =
                                                drm.egl_context.wait_on_native_fence(in_fence_fd)
                                            {
                                                log::warn!(
                                                    "failed to wait on compositor render fence after fallback submit failure: {:?}",
                                                    wait_error
                                                );
                                            }
                                        }
                                    }
                                    log::error!(
                                        "Could not submit DRM frame after software cursor fallback: {:?}",
                                        fallback_error
                                    );
                                    Some(failed_releases)
                                }
                            }
                        } else {
                            dispatch_gamma_control_failed_if_pending(
                                drm,
                                drm_crtc_id,
                                output_id,
                                dispatcher,
                            );
                            drm.output_mut(drm_crtc_id)
                                .unwrap()
                                .note_cursor_plane_failure();
                            drm.output_mut(drm_crtc_id).unwrap().mark_render_failed();
                            let failed_releases = drm
                                .output_mut(drm_crtc_id)
                                .unwrap()
                                .pipeline
                                .take_in_flight_releases();
                            discard_frame_for_timeline(
                                dispatcher,
                                &[crate::scheduler::presentation::DiscardedFeedback {
                                    feedbacks: presentation_feedbacks.clone(),
                                }],
                                None,
                                |_| {},
                                |dispatcher, completion| {
                                    dispatcher.dispatch_to(
                                        output_scheduler_id(self.output_id),
                                        OutputSchedulerAction::FrameCompleted(completion),
                                    );
                                },
                            );
                            if !render_completion.used_blocking_fallback {
                                if let Some(in_fence_fd) = render_completion.in_fence_fd {
                                    if let Err(wait_error) =
                                        drm.egl_context.wait_on_native_fence(in_fence_fd)
                                    {
                                        log::warn!(
                                            "failed to wait on compositor render fence after submit failure: {:?}",
                                            wait_error
                                        );
                                    }
                                }
                            }
                            log::error!("Could not submit DRM frame: {:?}", error);
                            Some(failed_releases)
                        }
                    } else {
                        dispatch_gamma_control_failed_if_pending(
                            drm,
                            drm_crtc_id,
                            output_id,
                            dispatcher,
                        );
                        drm.output_mut(drm_crtc_id).unwrap().mark_render_failed();
                        let failed_releases = drm
                            .output_mut(drm_crtc_id)
                            .unwrap()
                            .pipeline
                            .take_in_flight_releases();
                        discard_frame_for_timeline(
                            dispatcher,
                            &[crate::scheduler::presentation::DiscardedFeedback {
                                feedbacks: presentation_feedbacks.clone(),
                            }],
                            None,
                            |_| {},
                            |dispatcher, completion| {
                                dispatcher.dispatch_to(
                                    output_scheduler_id(self.output_id),
                                    OutputSchedulerAction::FrameCompleted(completion),
                                );
                            },
                        );
                        if !render_completion.used_blocking_fallback {
                            if let Some(in_fence_fd) = render_completion.in_fence_fd {
                                if let Err(wait_error) =
                                    drm.egl_context.wait_on_native_fence(in_fence_fd)
                                {
                                    log::warn!(
                                    "failed to wait on compositor render fence after submit failure: {:?}",
                                    wait_error
                                );
                                }
                            }
                        }
                        log::error!("Could not submit DRM frame: {:?}", error);
                        Some(failed_releases)
                    }
                } else {
                    let registration = submit_result.expect("submit result already checked");
                    state.presentation.register_submission(
                        self.output_id,
                        crate::scheduler::presentation::SubmissionRecord {
                            submission_id: registration.submission_id,
                            output_id: registration.output_id,
                            feedbacks: registration.presentation_feedbacks,
                            zero_copy: registration.zero_copy,
                        },
                    );
                    if submit_had_cursor_plane {
                        drm.output_mut(drm_crtc_id)
                            .unwrap()
                            .note_cursor_plane_success();
                    }
                    dispatch_submission_frame_callbacks(
                        &registration.callback_surfaces,
                        frame_time_msec,
                        dispatcher,
                    );
                    dispatcher.dispatch_to(
                        output_scheduler_id(self.output_id),
                        OutputSchedulerAction::PipelineBlocked { watchdog_deadline },
                    );
                    None
                }
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
        sequence: u32,
        tv_sec: u32,
        tv_usec: u32,
        crtc_id: u32,
    ) {
        let Some(drm_crtc_id) = state.output_crtc_id(self.output_id) else {
            return;
        };
        if drm_crtc_id != crtc_id {
            return;
        }
        let (releases, cursor_animation_delay_msec) = {
            let drm = state.drm.get_mut(self.drm_id);
            let releases = drm
                .output_mut(drm_crtc_id)
                .unwrap()
                .pipeline
                .take_in_flight_releases();
            let cursor_animation_delay_msec =
                drm.output(drm_crtc_id).unwrap().cursor_animation_delay_msec;
            (releases, cursor_animation_delay_msec)
        };

        for release in releases {
            release.resolve(state, dispatcher);
        }
        let refresh_is_constant = state
            .outputs
            .try_get(self.output_id)
            .map(|output| !output.vrr_enabled)
            .unwrap_or(true);
        let nominal_refresh_millihz = state
            .outputs
            .try_get(self.output_id)
            .map(|output| output.current_mode.refresh_millihz)
            .unwrap_or(0);
        let drm_fd = state.drm.get(self.drm_id).fd.clone();
        let mut presentation = std::mem::take(&mut state.presentation);
        complete_frame_for_timeline(
            dispatcher,
            &mut presentation,
            crate::scheduler::completion::TimelinePresentInfo {
                timeline: self.output_id,
                nominal_refresh_millihz,
                refresh_is_constant,
                tv_sec,
                tv_usec,
                sequence,
            },
            || {
                let drm = state.drm.get_mut(self.drm_id);
                drm.output_mut(drm_crtc_id).unwrap().finish_page_flip().map(
                    |completed_submission| {
                        crate::scheduler::completion::TimelineCompletedSubmission {
                            submission_id: completed_submission.submission.id,
                            output_id: completed_submission.submission.output_id,
                            zero_copy: completed_submission.submission.zero_copy,
                            damage_blob: completed_submission.damage_blob,
                            scene_build_duration: completed_submission
                                .submission
                                .scene_build_duration,
                            render_submit_duration: completed_submission
                                .submission
                                .render_submit_duration,
                            submitted_at: completed_submission.submission.submitted_at,
                        }
                    },
                )
            },
            |blob| {
                let _ = drm_fd.destroy_property_blob(blob);
            },
            |dispatcher, completion| {
                dispatcher.dispatch_to(
                    output_scheduler_id(self.output_id),
                    OutputSchedulerAction::FrameCompleted(completion),
                );
            },
        );
        state.presentation = presentation;
        sync_output_nodes_for_backend(state, self.drm_id);
        dispatcher.dispatch_to(
            output_scheduler_id(self.output_id),
            OutputSchedulerAction::ScheduleAnimation { delay_msec: cursor_animation_delay_msec },
        );
    }

    fn handle_flip_watchdog(&mut self, state: &mut State, dispatcher: &mut Dispatcher) {
        let Some(drm_crtc_id) = state.output_crtc_id(self.output_id) else {
            return;
        };
        let (releases, damage_blob) = {
            let drm = state.drm.get_mut(self.drm_id);
            let output = drm.output_mut(drm_crtc_id).unwrap();
            let releases = output.pipeline.take_in_flight_releases();
            let (_, damage_blob) = output.take_in_flight_presentation_feedbacks_for_discard();
            (releases, damage_blob)
        };
        for release in releases {
            release.resolve(state, dispatcher);
        }
        let discarded = state.presentation.discard_timeline(self.output_id);
        log::warn!(
            "flip watchdog fired for output {:?}; discarding in-flight submission and retrying",
            self.output_id
        );
        discard_frame_for_timeline(
            dispatcher,
            &discarded,
            damage_blob,
            |blob| {
                let drm = state.drm.get(self.drm_id);
                let _ = drm.fd.destroy_property_blob(blob);
            },
            |dispatcher, completion| {
                dispatcher.dispatch_to(
                    output_scheduler_id(self.output_id),
                    OutputSchedulerAction::FrameCompleted(completion),
                );
            },
        );
    }

    fn handle_session_changed(
        &mut self,
        state: &mut State,
        dispatcher: &mut Dispatcher,
        active: bool,
    ) {
        if !active {
            return;
        }
        let Some(drm_crtc_id) = state.output_crtc_id(self.output_id) else {
            return;
        };
        let (releases, feedbacks, damage_blob) = {
            let drm = state.drm.get_mut(self.drm_id);
            drm.output_mut(drm_crtc_id)
                .unwrap()
                .pipeline
                .reset_after_session_resume()
        };
        for release in releases {
            release.resolve(state, dispatcher);
        }
        if feedbacks.is_empty() && damage_blob.is_none() {
            return;
        }
        log::info!(
            "session resumed for output {:?}; clearing stale in-flight pipeline state",
            self.output_id
        );
        discard_frame_for_timeline(
            dispatcher,
            &[crate::scheduler::presentation::DiscardedFeedback { feedbacks }],
            damage_blob,
            |blob| {
                let drm = state.drm.get(self.drm_id);
                let _ = drm.fd.destroy_property_blob(blob);
            },
            |dispatcher, completion| {
                dispatcher.dispatch_to(
                    output_scheduler_id(self.output_id),
                    OutputSchedulerAction::FrameCompleted(completion),
                );
            },
        );
    }
}

fn collect_surface_snapshots(
    state: &mut crate::state::State,
    surface: Resolved<WlSurfaceId>,
    base_x: i32,
    base_y: i32,
    inherit_root_clip: bool,
    retired_candidates: &mut Vec<RetiredBufferCandidate>,
    snapshots: &mut Vec<crate::wayland_objects::wl_surface::SurfaceRenderState>,
) {
    let (stack, mut surface_state) = {
        let mut surface_obj = state.objects.grab_object::<WlSurface>(surface.object_id());
        retired_candidates.extend(surface_obj.take_retired_buffers());
        let Some(surface_state) = surface_obj.take_render_state_at(surface, base_x, base_y) else {
            return;
        };
        (surface_obj.current_stack().to_vec(), surface_state)
    };
    surface_state.inherit_root_clip = inherit_root_clip;

    for entry in &stack {
        if let StackEntry::Child(child_surface_id) = *entry {
            let child_base = {
                let Some(child_surface) = child_surface_id.try_ref(state) else {
                    continue;
                };
                let (x, y) = child_surface.current_subsurface_position();
                (surface_state.x + x, surface_state.y + y)
            };
            collect_surface_snapshots(
                state,
                child_surface_id,
                child_base.0,
                child_base.1,
                inherit_root_clip,
                retired_candidates,
                snapshots,
            );
        } else {
            snapshots.push(surface_state.clone());
        }
    }
    for popup_surface in state.popup_child_surfaces(surface) {
        let Some((popup_x, popup_y)) = state.surface_scene_origin(popup_surface) else {
            continue;
        };
        collect_surface_snapshots(
            state,
            popup_surface,
            popup_x,
            popup_y,
            false,
            retired_candidates,
            snapshots,
        );
    }
}

fn scene_order(scene: &[SceneItemState]) -> Vec<SceneItemKey> {
    scene.iter().map(|item| item.key).collect()
}

fn monotonic_time_msec() -> u32 {
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) } != 0 {
        return 0;
    }
    let millis = (ts.tv_sec as u64)
        .saturating_mul(1000)
        .saturating_add((ts.tv_nsec as u64) / 1_000_000);
    millis.min(u64::from(u32::MAX)) as u32
}

fn flip_watchdog_deadline_for_output(state: &State, output_id: OutputId) -> Instant {
    let refresh_millihz = state
        .outputs
        .try_get(output_id)
        .map(|output| output.current_mode.refresh_millihz.max(0) as u64)
        .unwrap_or(60_000);
    let interval_ns =
        if refresh_millihz == 0 { 16_666_667 } else { 1_000_000_000_000u64 / refresh_millihz };
    let watchdog =
        Duration::from_nanos(interval_ns.saturating_mul(4)).max(Duration::from_millis(50));
    Instant::now() + watchdog
}

fn classify_cursor_output_visibility(
    bounds: RectI,
    output_bounds: RectI,
) -> CursorOutputVisibility {
    if !bounds.intersects(output_bounds) {
        return CursorOutputVisibility::Hidden;
    }
    if bounds.x >= output_bounds.x
        && bounds.y >= output_bounds.y
        && bounds.x2() <= output_bounds.x2()
        && bounds.y2() <= output_bounds.y2()
    {
        CursorOutputVisibility::FullyVisible
    } else {
        CursorOutputVisibility::PartiallyVisible
    }
}

fn advance_cursor_plane_state(state: CursorPlaneState) -> (CursorPlaneState, bool) {
    match state {
        CursorPlaneState::Ready => (CursorPlaneState::Ready, true),
        CursorPlaneState::Backoff { remaining_frames, consecutive_failures }
            if remaining_frames > 0 =>
        {
            (
                CursorPlaneState::Backoff {
                    remaining_frames: remaining_frames - 1,
                    consecutive_failures,
                },
                false,
            )
        }
        CursorPlaneState::Backoff { .. } => (CursorPlaneState::Ready, true),
    }
}

fn next_cursor_plane_backoff_state(state: CursorPlaneState) -> CursorPlaneState {
    let consecutive_failures = match state {
        CursorPlaneState::Ready => 1,
        CursorPlaneState::Backoff { consecutive_failures, .. } => {
            consecutive_failures.saturating_add(1)
        }
    };
    let shift = u32::from(consecutive_failures.saturating_sub(1).min(5));
    let remaining_frames = 1u8.checked_shl(shift).unwrap_or(32).min(32);
    CursorPlaneState::Backoff { remaining_frames, consecutive_failures }
}

fn dispatch_submission_frame_callbacks(
    callback_surfaces: &[SurfaceKey],
    frame_time_msec: u32,
    dispatcher: &mut Dispatcher,
) {
    let mut per_client = BTreeMap::<ClientId, Vec<Resolved<WlSurfaceId>>>::new();
    let mut seen = BTreeSet::<SurfaceKey>::new();
    for &surface in callback_surfaces {
        if !seen.insert(surface) {
            continue;
        }
        per_client
            .entry(surface.client_id())
            .or_default()
            .push(surface);
    }
    for (client_id, surfaces) in per_client {
        dispatcher.dispatch_to(
            client_id,
            ClientAction::CompleteSurfaceFrameCallbacks { surfaces, time_msec: frame_time_msec },
        );
    }
}

fn presentation_feedback_flags(zero_copy: bool) -> u32 {
    use crate::protocol_generated::wp_presentation_feedback::Kind;

    let mut flags = Kind::Vsync as u32 | Kind::HwCompletion as u32;
    if zero_copy {
        flags |= Kind::ZeroCopy as u32;
    }
    flags
}

fn scene_item_requires_compositor_compositing(item: &SceneItem) -> bool {
    item.alpha < u8::MAX || !matches!(item.mask, SceneItemMask::None)
}

fn surface_allows_direct_scanout(root_clip: Option<RootSurfaceClip>, alpha: u8) -> bool {
    root_clip.is_none() && alpha == u8::MAX
}

fn effect_transition_requires_full_redraw(has_effects: bool, had_effects_last_frame: bool) -> bool {
    has_effects || had_effects_last_frame
}

fn collect_named_cursor_render_item(
    state: &mut crate::state::State,
    output_bounds: RectI,
    now_msec: u32,
) -> Option<(SceneItem, Option<u32>)> {
    let (pointer_x, pointer_y) = state.seat.pointer_position();
    match state.seat.visible_cursor_source_at(now_msec)? {
        CursorSourceRef::Named { image, next_frame_in_msec, .. } => {
            let x = pointer_x.round() as i32 - image.hotspot_x;
            let y = pointer_y.round() as i32 - image.hotspot_y;
            let bounds = RectI::new(x, y, image.width, image.height)?;
            if classify_cursor_output_visibility(bounds, output_bounds)
                == CursorOutputVisibility::Hidden
            {
                return None;
            }

            let texture =
                GlTexture::from_rgba(&image.pixels_rgba, image.width, image.height).ok()?;
            let texture_id = texture.texture_id();
            Some((
                SceneItem {
                    state: SceneItemState {
                        key: SceneItemKey::Cursor,
                        surface_key: None,
                        content_key: SceneItemContentKey::NamedCursor(image.content_key()),
                        bounds,
                        tex_transform: transform::Mat3::identity(),
                    },
                    mask: SceneItemMask::None,
                    texture: texture_id,
                    owned_texture: Some(texture),
                    damage: DamageRegion::from_rect(bounds),
                    sample_damage_padding: 0,
                    state_has_alpha: true,
                    alpha: u8::MAX,
                    opaque_region: DamageRegion::new(),
                    opaque: false,
                    sync: crate::sync::SurfaceSyncState::default(),
                    direct_scanout: None,
                    presentation_feedbacks: Vec::new(),
                },
                next_frame_in_msec,
            ))
        }
        CursorSourceRef::Surface { .. } => None,
    }
}

fn collect_submission_metadata(
    items: &[SceneItem],
    software_cursor_items: &[SceneItem],
) -> (Vec<SceneItemState>, Vec<SurfaceKey>, Vec<SubmissionPresentationFeedback>) {
    let mut scene = Vec::with_capacity(items.len() + software_cursor_items.len());
    let mut frame_callback_surfaces = Vec::new();
    let mut presentation_feedbacks = Vec::new();
    let mut seen_callbacks = BTreeSet::new();
    for item in items.iter().chain(software_cursor_items.iter()) {
        scene.push(item.state.clone());
        let Some(surface_key) = item.state.surface_key else {
            continue;
        };
        if seen_callbacks.insert(surface_key) {
            frame_callback_surfaces.push(surface_key);
        }
        presentation_feedbacks.extend(item.presentation_feedbacks.iter().copied().map(
            |feedback| SubmissionPresentationFeedback {
                feedback: feedback.feedback,
                version: feedback.version,
                surface: item.state.surface_key,
            },
        ));
    }
    (scene, frame_callback_surfaces, presentation_feedbacks)
}

fn rgba_to_drm_argb8888(
    src: &[u8],
    width: usize,
    height: usize,
    pitch: usize,
    output_height: usize,
) -> Vec<u8> {
    let mut dst = vec![0; pitch.saturating_mul(output_height)];
    let src_stride = width.saturating_mul(4);
    for row in 0..height {
        let src_row = &src[row * src_stride..(row + 1) * src_stride];
        let dst_row = &mut dst[row * pitch..row * pitch + src_stride];
        for (src_px, dst_px) in src_row.chunks_exact(4).zip(dst_row.chunks_exact_mut(4)) {
            dst_px[0] = src_px[2];
            dst_px[1] = src_px[1];
            dst_px[2] = src_px[0];
            dst_px[3] = src_px[3];
        }
    }
    dst
}

fn push_scene_items_from_snapshots(
    items: &mut Vec<SceneItem>,
    objects: &mut Box<crate::state::IndexMap<ObjectId, Box<dyn Object>>>,
    drm: &Drm,
    drm_fd: &DrmFd,
    drm_crtc_id: u32,
    output_bounds: RectI,
    snapshots: Vec<crate::wayland_objects::wl_surface::SurfaceRenderState>,
    allow_direct_scanout: bool,
    root_clip: Option<RootSurfaceClip>,
    alpha: u8,
) {
    for surface_state in snapshots {
        let buffer_oid = surface_state.buffer.object_id();
        if !objects.contains(buffer_oid) {
            log::warn!("collect_render_items: buffer object {:?} missing (destroyed?)", buffer_oid);
            continue;
        }
        let Some(_) = objects.try_ref_object::<WlBuffer>(buffer_oid) else {
            log::warn!("collect_render_items: object {:?} is not a WlBuffer", buffer_oid);
            continue;
        };

        let mut obj = objects.grab(surface_state.buffer.object_id());
        let Some(buffer) = obj.downcast_mut::<WlBuffer>() else {
            continue;
        };
        let Some(bounds) =
            RectI::new(surface_state.x, surface_state.y, surface_state.width, surface_state.height)
        else {
            continue;
        };
        if !bounds.intersects(output_bounds) {
            continue;
        }
        let mut damage = surface_state.damage.clone();
        damage.intersect_rect(bounds);
        damage.intersect_rect(output_bounds);
        let mut opaque_region = if buffer.format().has_alpha {
            surface_state.opaque_region.clone()
        } else {
            DamageRegion::from_rect(bounds)
        };
        opaque_region.intersect_rect(bounds);
        opaque_region.intersect_rect(output_bounds);
        let Some(texture) = buffer.prepare_texture(&drm.egl_context, &surface_state.sync) else {
            log::warn!(
                "collect_render_items: no prepared texture for buffer={:?}",
                surface_state.buffer
            );
            continue;
        };
        let tex_transform = buffer.base_tex_transform().mul(surface_state.tex_transform);
        let (buffer_width, buffer_height) = buffer.dimensions();
        let sample_damage_padding = if surface_state.tex_transform != transform::Mat3::identity()
            || buffer_width != bounds.width
            || buffer_height != bounds.height
        {
            1
        } else {
            0
        };
        let scanout_sync_ok =
            matches!(
                surface_state.sync.acquire,
                crate::sync::AcquireSync::None | crate::sync::AcquireSync::SyncFile(_)
            ) && (!matches!(surface_state.sync.acquire, crate::sync::AcquireSync::SyncFile(_))
                || drm
                    .output(drm_crtc_id)
                    .is_some_and(|output| output.in_use.primary_plane().0.in_fence_fd.is_some()));
        let direct_scanout = if surface_allows_direct_scanout(root_clip, alpha)
            && allow_direct_scanout
            && buffer.dmabuf().is_some()
            && !buffer.y_inverted()
            && !buffer.format().has_alpha
            && bounds == output_bounds
            && buffer_width == bounds.width
            && buffer_height == bounds.height
            && tex_transform == transform::Mat3::identity()
            && scanout_sync_ok
        {
            buffer.dmabuf().and_then(|dmabuf| {
                let primary_plane = drm.output(drm_crtc_id)?.in_use.primary_plane().0;
                let plane_format = primary_plane.formats.get(&dmabuf.format.drm)?;
                if !plane_format.modifiers.contains(&dmabuf.modifier)
                    && !plane_format.modifiers.contains(&Modifier::INVALID)
                {
                    return None;
                }
                Drm::add_framebuffer(drm_fd, dmabuf).ok()
            })
        } else {
            None
        };
        let surface = surface_state
            .surface
            .expect("rendered surface snapshots carry their object id");
        let surface_key = surface;
        let (mask, opaque_region, opaque) = if surface_state.inherit_root_clip {
            if let Some(clip) = root_clip {
                (SceneItemMask::RoundedClip { clip }, DamageRegion::new(), false)
            } else {
                let opaque = !opaque_region.is_empty();
                (SceneItemMask::None, opaque_region, opaque)
            }
        } else {
            let opaque = !opaque_region.is_empty();
            (SceneItemMask::None, opaque_region, opaque)
        };
        items.push(SceneItem {
            state: SceneItemState {
                key: SceneItemKey::Surface(surface_key),
                surface_key: Some(surface_key),
                content_key: SceneItemContentKey::Buffer(surface_state.buffer),
                bounds,
                tex_transform,
            },
            mask,
            texture,
            owned_texture: None,
            damage,
            sample_damage_padding,
            state_has_alpha: buffer.format().has_alpha,
            alpha,
            opaque_region,
            opaque,
            sync: surface_state.sync,
            direct_scanout,
            presentation_feedbacks: surface_state.presentation_feedbacks,
        });
    }
}

fn push_root_surface_effect_elements(
    frame_elements: &mut Vec<RecordedFrameElement>,
    visual_state: Option<RootSurfaceVisualState>,
    phase: FramePhase,
) {
    let Some(visual_state) = visual_state else {
        return;
    };
    let clip = visual_state.clip.unwrap_or_else(|| {
        RootSurfaceClip::new(visual_state.outer_rect, 0, crate::config::RoundingPower::DEFAULT)
    });
    let ring = rounded_ring(clip, visual_state.content_rect);
    if phase == FramePhase::UnderWindow {
        if let Some(blur) = visual_state.effects.blur {
            frame_elements.push(RecordedFrameElement::Effect {
                phase: FramePhase::UnderWindow,
                element: EffectElement::WindowBlurBackdrop {
                    ring,
                    style: blur,
                    alpha: visual_state.effects.opacity_alpha,
                },
            });
        }
    }
    if phase == FramePhase::UnderWindow {
        if let Some(shadow) = visual_state.shadow {
            frame_elements.push(RecordedFrameElement::Effect {
                phase: FramePhase::UnderWindow,
                element: EffectElement::WindowShadow {
                    clip,
                    style: shadow,
                    alpha: visual_state.shadow_alpha,
                },
            });
        }
    }
    if phase == FramePhase::OverWindow {
        if let Some(border) = visual_state.border {
            frame_elements.push(RecordedFrameElement::Effect {
                phase: FramePhase::OverWindow,
                element: EffectElement::WindowBorder {
                    ring,
                    gradient: border.gradient,
                    alpha: visual_state.border_alpha,
                },
            });
        }
    }
    if phase == FramePhase::Overlay && visual_state.effects.dim_alpha > 0 {
        frame_elements.push(RecordedFrameElement::Effect {
            phase: FramePhase::Overlay,
            element: EffectElement::DimOverlay { clip, alpha: visual_state.effects.dim_alpha },
        });
    }
}

fn push_scene_item_frame_elements(
    frame_elements: &mut Vec<RecordedFrameElement>,
    item_indices: std::ops::Range<usize>,
) {
    frame_elements.extend(item_indices.map(|item_index| RecordedFrameElement::Texture {
        phase: FramePhase::Content,
        item_index,
    }));
}

fn collect_cursor_surface_snapshots(
    state: &mut crate::state::State,
    retired_candidates: &mut Vec<RetiredBufferCandidate>,
) -> Vec<crate::wayland_objects::wl_surface::SurfaceRenderState> {
    let (pointer_x, pointer_y) = state.seat.pointer_position();
    let Some(CursorSourceRef::Surface { cursor, .. }) = state.seat.visible_cursor_source() else {
        return Vec::new();
    };
    let Some(surface) = state
        .objects
        .try_ref_object::<WlSurface>(cursor.surface.object_id())
    else {
        return Vec::new();
    };
    let (root_x, root_y) = surface.root_position();
    let base_x = pointer_x.round() as i32 - cursor.hotspot_x - root_x;
    let base_y = pointer_y.round() as i32 - cursor.hotspot_y - root_y;
    let mut snapshots = Vec::new();
    collect_surface_snapshots(
        state,
        cursor.surface,
        base_x,
        base_y,
        true,
        retired_candidates,
        &mut snapshots,
    );
    snapshots
}

fn collect_software_cursor_items(
    state: &mut crate::state::State,
    drm_id: DrmId,
    drm_crtc_id: u32,
    output_bounds: RectI,
    frame_time_msec: u32,
    retired_candidates: &mut Vec<RetiredBufferCandidate>,
) -> (Vec<SceneItem>, Option<u32>) {
    if let Some((item, next_frame_in_msec)) =
        collect_named_cursor_render_item(state, output_bounds, frame_time_msec)
    {
        return (vec![item], next_frame_in_msec);
    }
    if !matches!(state.seat.visible_cursor_source(), Some(CursorSourceRef::Surface { .. })) {
        return (Vec::new(), None);
    }
    let cursor_snapshots = collect_cursor_surface_snapshots(state, retired_candidates);
    let (objects, drm_map) = (&mut state.objects, &state.drm);
    let drm = drm_map.get(drm_id);
    let drm_fd = drm.fd();
    let mut items = Vec::new();
    push_scene_items_from_snapshots(
        &mut items,
        objects,
        drm,
        &drm_fd,
        drm_crtc_id,
        output_bounds,
        cursor_snapshots,
        false,
        None,
        u8::MAX,
    );
    (items, None)
}

fn build_named_cursor_plane_request(
    pointer_position: (f64, f64),
    image: &CursorImage,
    next_frame_in_msec: Option<u32>,
) -> CursorPlaneRequest {
    CursorPlaneRequest {
        x: pointer_position.0.round() as i32 - image.hotspot_x,
        y: pointer_position.1.round() as i32 - image.hotspot_y,
        width: image.width,
        height: image.height,
        hotspot_x: image.hotspot_x,
        hotspot_y: image.hotspot_y,
        content_key: CursorPlaneContentKey::Named(image.content_key()),
        upload: CursorPlaneUploadSource::Named { image: image as *const CursorImage },
        next_frame_in_msec,
    }
}

fn build_surface_cursor_plane_request(
    state: &crate::state::State,
    cursor: &crate::cursor::CursorSurface,
) -> Option<CursorPlaneRequest> {
    let (pointer_x, pointer_y) = state.seat.pointer_position();
    let surface = state
        .objects
        .try_ref_object::<WlSurface>(cursor.surface.object_id())?;
    if !matches!(surface.current_stack(), [StackEntry::Parent]) {
        return None;
    }
    let (root_x, root_y) = surface.root_position();
    let base_x = pointer_x.round() as i32 - cursor.hotspot_x - root_x;
    let base_y = pointer_y.round() as i32 - cursor.hotspot_y - root_y;
    let render_state = surface.render_state_at(base_x, base_y)?;
    if render_state.tex_transform != transform::Mat3::identity() {
        return None;
    }
    let content_revision = surface.content_revision();
    let buffer_ref = render_state.buffer.try_ref(state)?;
    if buffer_ref.dimensions() != (render_state.width, render_state.height) {
        return None;
    }
    let Some(buffer) = buffer_ref.shm_cursor_argb8888() else {
        return None;
    };
    Some(CursorPlaneRequest {
        x: render_state.x,
        y: render_state.y,
        width: render_state.width,
        height: render_state.height,
        hotspot_x: cursor.hotspot_x,
        hotspot_y: cursor.hotspot_y,
        content_key: CursorPlaneContentKey::Surface(CursorSurfaceContentKey {
            surface: cursor.surface,
            buffer: render_state.buffer,
            content_revision,
            width: render_state.width,
            height: render_state.height,
        }),
        upload: CursorPlaneUploadSource::SurfaceShm { buffer },
        next_frame_in_msec: None,
    })
}

fn build_cursor_plane_request(
    state: &crate::state::State,
    frame_time_msec: u32,
) -> Option<CursorPlaneRequest> {
    let pointer_position = state.seat.pointer_position();
    match state.seat.visible_cursor_source_at(frame_time_msec)? {
        CursorSourceRef::Named { image, next_frame_in_msec, .. } => {
            Some(build_named_cursor_plane_request(pointer_position, image, next_frame_in_msec))
        }
        CursorSourceRef::Surface { cursor, .. } => {
            build_surface_cursor_plane_request(state, cursor)
        }
    }
}

fn collect_render_items_for_output_view(
    state: &mut crate::state::State,
    drm_id: DrmId,
    drm_crtc_id: u32,
    output_id: OutputId,
    output_bounds: RectI,
    frame_time_msec: u32,
    view: &OutputCompositionView,
) -> (
    Vec<SceneItem>,
    Vec<SceneItem>,
    Vec<RecordedFrameElement>,
    Vec<DeferredRelease>,
    Option<CursorPlaneCandidate>,
    Option<u32>,
) {
    let mut items = Vec::new();
    let mut frame_elements = Vec::new();
    let mut deferred_releases = Vec::new();
    let mut retired_candidates = Vec::new();
    let mut cursor_animation_delay_msec = None;
    let root_surfaces: Vec<_> = view
        .entries
        .iter()
        .filter(|entry| entry.participates_in_render)
        .map(|entry| entry.root_surface)
        .collect();
    for root_surface in root_surfaces {
        let Some(surface) = state
            .objects
            .try_ref_object::<WlSurface>(root_surface.object_id())
        else {
            continue;
        };
        if !surface.is_root() || surface.has_cursor_role() || surface.has_dnd_icon_role() {
            continue;
        }
        let visual_state = state.root_surface_visual_state(root_surface);
        let root_clip = visual_state.and_then(|visual| visual.clip);
        push_root_surface_effect_elements(
            &mut frame_elements,
            visual_state,
            FramePhase::UnderWindow,
        );
        let mut snapshots = Vec::new();
        collect_surface_snapshots(
            state,
            root_surface,
            0,
            0,
            true,
            &mut retired_candidates,
            &mut snapshots,
        );
        let (objects, drm_map) = (&mut state.objects, &state.drm);
        let drm = drm_map.get(drm_id);
        let drm_fd = drm.fd();
        let item_start_index = items.len();
        let allow_direct_scanout = view
            .entries
            .iter()
            .find(|entry| entry.root_surface == root_surface)
            .map(|entry| entry.allow_direct_scanout)
            .unwrap_or(false);
        push_scene_items_from_snapshots(
            &mut items,
            objects,
            drm,
            &drm_fd,
            drm_crtc_id,
            output_bounds,
            snapshots,
            allow_direct_scanout,
            root_clip,
            visual_state
                .map(|visual| visual.effects.opacity_alpha)
                .unwrap_or(u8::MAX),
        );
        push_scene_item_frame_elements(&mut frame_elements, item_start_index..items.len());
        push_root_surface_effect_elements(
            &mut frame_elements,
            visual_state,
            FramePhase::OverWindow,
        );
        push_root_surface_effect_elements(&mut frame_elements, visual_state, FramePhase::Overlay);
    }
    collect_dnd_render_items(
        state,
        drm_id,
        drm_crtc_id,
        output_id,
        output_bounds,
        &mut items,
        &mut retired_candidates,
    );

    let cursor_plane = {
        let cursor_request = build_cursor_plane_request(state, frame_time_msec);
        let allow_cursor_plane = {
            let drm = state.drm.get_mut(drm_id);
            cursor_request.is_some()
                && drm
                    .output_mut(drm_crtc_id)
                    .is_some_and(DrmOutput::cursor_plane_available)
        };
        if allow_cursor_plane {
            let request = cursor_request.as_ref().unwrap();
            let cursor_plane = state.drm.get_mut(drm_id).collect_cursor_plane_candidate(
                drm_crtc_id,
                request,
                output_bounds,
            );
            if cursor_plane.is_some() {
                cursor_animation_delay_msec = request.next_frame_in_msec;
            }
            cursor_plane
        } else {
            None
        }
    };
    let (software_cursor_items, software_cursor_animation_delay_msec) =
        collect_software_cursor_items(
            state,
            drm_id,
            drm_crtc_id,
            output_bounds,
            frame_time_msec,
            &mut retired_candidates,
        );
    if cursor_animation_delay_msec.is_none() {
        cursor_animation_delay_msec = software_cursor_animation_delay_msec;
    }

    for retired in retired_candidates {
        deferred_releases.push(DeferredRelease::from_candidate(state, retired));
    }

    (
        items,
        software_cursor_items,
        frame_elements,
        deferred_releases,
        cursor_plane,
        cursor_animation_delay_msec,
    )
}

fn collect_dnd_render_items(
    state: &mut State,
    drm_id: DrmId,
    drm_crtc_id: u32,
    output_id: OutputId,
    output_bounds: RectI,
    items: &mut Vec<SceneItem>,
    retired_candidates: &mut Vec<RetiredBufferCandidate>,
) {
    let Some(drag) = state.seat.active_drag() else {
        return;
    };
    let Some(root_surface) = drag.icon_surface else {
        return;
    };
    let Some(surface) = root_surface.try_ref(state) else {
        return;
    };
    let (pointer_x, pointer_y) = state.seat.pointer_position();
    let (root_x, root_y) = surface.root_position();
    let base_x = pointer_x.round() as i32 - root_x;
    let base_y = pointer_y.round() as i32 - root_y;
    let icon_bounds = surface
        .render_state_at(base_x, base_y)
        .and_then(|render_state| {
            RectI::new(render_state.x, render_state.y, render_state.width, render_state.height)
        });
    if let Some(bounds) = icon_bounds {
        let owns_output = state
            .primary_output_for_rect(bounds, None)
            .unwrap_or(output_id);
        if owns_output != output_id && !bounds.intersects(output_bounds) {
            return;
        }
    } else if state.active_output_id() != Some(output_id) {
        return;
    }
    let mut snapshots = Vec::new();
    collect_surface_snapshots(
        state,
        root_surface,
        base_x,
        base_y,
        true,
        retired_candidates,
        &mut snapshots,
    );
    let (objects, drm_map) = (&mut state.objects, &state.drm);
    let drm = drm_map.get(drm_id);
    let drm_fd = drm.fd();
    push_scene_items_from_snapshots(
        items,
        objects,
        drm,
        &drm_fd,
        drm_crtc_id,
        output_bounds,
        snapshots,
        false,
        None,
        u8::MAX,
    );
}

fn scene_item_to_texture_element(item: &SceneItem) -> TextureElement {
    TextureElement {
        source: TextureSource::Texture,
        texture: item.texture,
        x: item.state.bounds.x,
        y: item.state.bounds.y,
        width: item.state.bounds.width,
        height: item.state.bounds.height,
        tex_transform: item.state.tex_transform,
        damage: item.damage.clone(),
        blend: item.state_has_alpha,
        alpha: item.alpha,
        mask: match item.mask {
            SceneItemMask::None => TextureMask::None,
            SceneItemMask::RoundedClip { clip } => {
                TextureMask::RoundedClip { clip: localize_clip(clip, item.state.bounds) }
            }
            SceneItemMask::FrameRing { ring } => {
                TextureMask::FrameRing { ring: localize_ring(ring, item.state.bounds) }
            }
        },
    }
}

fn build_output_frame_recipe(
    items: &[SceneItem],
    frame_elements: &[RecordedFrameElement],
) -> OutputFrameRecipe {
    OutputFrameRecipe {
        elements: frame_elements
            .iter()
            .filter_map(|element| match element {
                RecordedFrameElement::Texture { phase, item_index } => {
                    items.get(*item_index).map(|item| FrameElement::Texture {
                        phase: *phase,
                        element: scene_item_to_texture_element(item),
                    })
                }
                RecordedFrameElement::Effect { phase, element } => {
                    Some(FrameElement::Effect { phase: *phase, element: *element })
                }
            })
            .collect(),
    }
}

fn translate_scene_items(items: &mut [SceneItem], dx: i32, dy: i32) {
    for item in items {
        item.state.bounds = item.state.bounds.translate(dx, dy);
        item.damage.translate(dx, dy);
        item.opaque_region.translate(dx, dy);
    }
}

fn translate_recorded_frame_elements(
    frame_elements: &mut [RecordedFrameElement],
    dx: i32,
    dy: i32,
) {
    for frame_element in frame_elements {
        let RecordedFrameElement::Effect { element, .. } = frame_element else {
            continue;
        };
        translate_effect_element(element, dx, dy);
    }
}

fn translate_effect_element(element: &mut EffectElement, dx: i32, dy: i32) {
    match element {
        EffectElement::WindowBorder { ring, .. }
        | EffectElement::WindowBlurBackdrop { ring, .. } => {
            ring.outer.outer_rect = ring.outer.outer_rect.translate(dx, dy);
            ring.inner.outer_rect = ring.inner.outer_rect.translate(dx, dy);
        }
        EffectElement::WindowShadow { clip, .. } | EffectElement::DimOverlay { clip, .. } => {
            clip.outer_rect = clip.outer_rect.translate(dx, dy);
        }
    }
}

fn localize_clip(clip: RootSurfaceClip, item_bounds: RectI) -> RootSurfaceClip {
    RootSurfaceClip::new(
        clip.outer_rect
            .translate(item_bounds.x.saturating_neg(), item_bounds.y.saturating_neg()),
        clip.corner_radius,
        clip.rounding_power,
    )
}

fn localize_ring(ring: RoundedRing, item_bounds: RectI) -> RoundedRing {
    RoundedRing {
        outer: localize_clip(ring.outer, item_bounds),
        inner: localize_clip(ring.inner, item_bounds),
    }
}

#[cfg(test)]
mod drm_event_tests {
    use super::ParsedDrmEvent;

    #[test]
    fn parsed_drm_event_len() {
        let e = ParsedDrmEvent::Ignored { len: 64, type_: 0 };
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

#[cfg(test)]
mod window_animation_tests {
    use {
        super::{
            animation_delay_msec_for_output, WindowAnimationManager, WindowAnimationManagerAction,
        },
        crate::{
            clients::ClientId,
            protocol_generated::WlSurfaceId,
            runner::{Dispatcher, Model},
            scheduler::{output_scheduler_id, OutputSchedulerAction, RequestFrameReason},
            state::{BackendOutputRef, DrmId, ObjectId},
            test_support::{
                resolved_wl_surface as root, state_for_tests, test_output_with_crtc_name,
            },
            wayland_objects::wl_surface::WlSurface,
        },
    };

    #[test]
    fn window_animation_manager_kick_schedules_outputs_without_global_timer_rearm() {
        let mut state = state_for_tests();
        let backend = BackendOutputRef::new(DrmId::from(0usize));
        let output_id =
            state.insert_output(test_output_with_crtc_name(backend, 1, 0, 0, 1280, 720));
        let workspace_id = state.active_workspace_for_output(output_id).unwrap();
        let (_, object_id) =
            state.insert_object(WlSurface::new(WlSurfaceId::from(55u32), ClientId::from_raw(2), 6));
        {
            let mut surface = state.objects.grab_object::<WlSurface>(object_id);
            surface.test_map(ObjectId::from(0usize), 50, 60, 320, 200);
        }
        state.config.effects.opacity.active_alpha = 255;
        state.config.effects.opacity.inactive_alpha = 120;
        state.config.effects.opacity.inactive_dim = 44;
        state.config.animations.window.duration_msec = 120;
        state.register_root_surface(root(&state, object_id));
        state.assign_root_surface_to_workspace(root(&state, object_id), workspace_id);
        let surface = root(&state, object_id);
        state.seat.keyboard_focus = Some(crate::input::SurfaceFocus::new(surface));

        let mut manager = WindowAnimationManager::new();
        let mut dispatcher = Dispatcher::default();
        manager
            .process(&mut state, WindowAnimationManagerAction::Kick, &mut dispatcher)
            .unwrap();

        assert!(dispatcher.test_has_queued_action_for::<OutputSchedulerAction, _>(
            output_scheduler_id(output_id),
            |action: &OutputSchedulerAction| {
                matches!(
                    action,
                    OutputSchedulerAction::RequestFrame { reason: RequestFrameReason::Animation }
                )
            }
        ));
    }

    #[test]
    fn animation_delay_is_computed_per_output() {
        let mut state = state_for_tests();
        let backend = BackendOutputRef::new(DrmId::from(0usize));
        let left_id = state.insert_output(test_output_with_crtc_name(backend, 1, 0, 0, 1280, 720));
        let right_id =
            state.insert_output(test_output_with_crtc_name(backend, 2, 1280, 0, 1280, 720));
        {
            let right = state.outputs.try_get_mut(right_id).unwrap();
            right.current_mode.refresh_millihz = 120_000;
        }

        assert_eq!(animation_delay_msec_for_output(&state, left_id), Some(17));
        assert_eq!(animation_delay_msec_for_output(&state, right_id), Some(9));
    }
}

#[cfg(test)]
mod cursor_policy_tests {
    use {
        super::{
            advance_cursor_plane_state, classify_cursor_output_visibility,
            next_cursor_plane_backoff_state, CursorOutputVisibility, CursorPlaneState,
        },
        crate::region::RectI,
    };

    #[test]
    fn cursor_output_visibility_distinguishes_hidden_partial_and_full() {
        let output_bounds = RectI::new(10, 20, 100, 80).unwrap();

        assert_eq!(
            classify_cursor_output_visibility(RectI::new(0, 0, 5, 5).unwrap(), output_bounds),
            CursorOutputVisibility::Hidden
        );
        assert_eq!(
            classify_cursor_output_visibility(RectI::new(5, 20, 20, 10).unwrap(), output_bounds),
            CursorOutputVisibility::PartiallyVisible
        );
        assert_eq!(
            classify_cursor_output_visibility(RectI::new(20, 30, 10, 10).unwrap(), output_bounds),
            CursorOutputVisibility::FullyVisible
        );
    }

    #[test]
    fn cursor_plane_backoff_skips_frames_then_recovers() {
        let state = next_cursor_plane_backoff_state(CursorPlaneState::Ready);
        assert_eq!(
            state,
            CursorPlaneState::Backoff { remaining_frames: 1, consecutive_failures: 1 }
        );

        let (state, available) = advance_cursor_plane_state(state);
        assert!(!available);
        assert_eq!(
            state,
            CursorPlaneState::Backoff { remaining_frames: 0, consecutive_failures: 1 }
        );

        let (state, available) = advance_cursor_plane_state(state);
        assert!(available);
        assert_eq!(state, CursorPlaneState::Ready);
    }

    #[test]
    fn cursor_plane_backoff_grows_with_repeated_failures() {
        let state = next_cursor_plane_backoff_state(CursorPlaneState::Ready);
        let state = next_cursor_plane_backoff_state(state);
        assert_eq!(
            state,
            CursorPlaneState::Backoff { remaining_frames: 2, consecutive_failures: 2 }
        );
    }
}

#[cfg(test)]
mod render_recipe_tests {
    use {
        super::{
            build_output_frame_recipe, push_root_surface_effect_elements,
            scene_item_to_texture_element, translate_recorded_frame_elements, RecordedFrameElement,
            SceneItem, SceneItemContentKey, SceneItemKey, SceneItemMask, SceneItemState,
        },
        crate::{
            region::{DamageRegion, RectI},
            render::{
                rounded_ring, BlurVisualStyle, BorderGradient, BorderVisualStyle, EffectElement,
                FramePhase, RootSurfaceVisualState, ShadowVisualStyle, SurfaceEffectStyle,
            },
            state::RootSurfaceClip,
            transform,
        },
    };

    #[test]
    fn scene_items_to_texture_elements_map_root_clip_to_local_coordinates() {
        let item_bounds = RectI::new(40, 50, 120, 80).unwrap();
        let clip = RootSurfaceClip::new(
            RectI::new(34, 38, 130, 96).unwrap(),
            18,
            crate::config::RoundingPower::DEFAULT,
        );
        let items = vec![SceneItem {
            state: SceneItemState {
                key: SceneItemKey::Cursor,
                surface_key: None,
                content_key: SceneItemContentKey::NamedCursor(
                    crate::cursor::CursorImageContentKey { width: 1, height: 2, fingerprint: 3 },
                ),
                bounds: item_bounds,
                tex_transform: transform::Mat3::identity(),
            },
            mask: SceneItemMask::RoundedClip { clip },
            texture: 0,
            owned_texture: None,
            damage: DamageRegion::from_rect(item_bounds),
            sample_damage_padding: 0,
            state_has_alpha: false,
            alpha: u8::MAX,
            opaque_region: DamageRegion::new(),
            opaque: false,
            sync: crate::sync::SurfaceSyncState::default(),
            direct_scanout: None,
            presentation_feedbacks: Vec::new(),
        }];

        let render_item = scene_item_to_texture_element(&items[0]);
        assert_eq!(
            render_item.mask,
            crate::egl::RenderItemMask::RoundedClip {
                clip: RootSurfaceClip::new(
                    RectI::new(-6, -12, 130, 96).unwrap(),
                    18,
                    crate::config::RoundingPower::DEFAULT,
                ),
            }
        );
    }

    #[test]
    fn root_surface_effect_elements_use_runtime_alpha_blur_and_clip() {
        let clip = RootSurfaceClip::new(
            RectI::new(20, 30, 100, 80).unwrap(),
            12,
            crate::config::RoundingPower::DEFAULT,
        );
        let visual_state = RootSurfaceVisualState {
            outer_rect: clip.outer_rect,
            content_rect: RectI::new(26, 36, 88, 68).unwrap(),
            clip: Some(clip),
            border: Some(BorderVisualStyle {
                gradient: BorderGradient {
                    start_color: crate::config::RgbaColor::new(1, 2, 3, 4),
                    end_color: crate::config::RgbaColor::new(5, 6, 7, 8),
                    angle_degrees: 90,
                },
            }),
            border_alpha: 120,
            shadow: Some(ShadowVisualStyle {
                radius: 18,
                offset_x: 3,
                offset_y: 5,
                color: crate::config::RgbaColor::new(0, 0, 0, 100),
                power: crate::config::ShadowPower::sanitize(2.5),
            }),
            shadow_alpha: 90,
            effects: SurfaceEffectStyle {
                blur: Some(BlurVisualStyle { radius: 22, passes: 4 }),
                opacity_alpha: 200,
                dim_alpha: 44,
            },
        };
        let mut frame_elements = Vec::new();

        push_root_surface_effect_elements(
            &mut frame_elements,
            Some(visual_state),
            FramePhase::UnderWindow,
        );
        push_root_surface_effect_elements(
            &mut frame_elements,
            Some(visual_state),
            FramePhase::OverWindow,
        );
        push_root_surface_effect_elements(
            &mut frame_elements,
            Some(visual_state),
            FramePhase::Overlay,
        );

        assert!(frame_elements.iter().any(|effect| matches!(
            effect,
            RecordedFrameElement::Effect {
                phase: FramePhase::UnderWindow,
                element: EffectElement::WindowBlurBackdrop { style, alpha, .. }
            } if style.radius == 22 && style.passes == 4 && *alpha == 200
        )));
        assert!(frame_elements.iter().any(|effect| matches!(
            effect,
            RecordedFrameElement::Effect {
                phase: FramePhase::OverWindow,
                element: EffectElement::WindowBorder { alpha, .. }
            } if *alpha == 120
        )));
        assert!(frame_elements.iter().any(|effect| matches!(
            effect,
            RecordedFrameElement::Effect {
                phase: FramePhase::UnderWindow,
                element: EffectElement::WindowShadow { alpha, .. }
            } if *alpha == 90
        )));
        assert!(frame_elements.iter().any(|effect| matches!(
            effect,
            RecordedFrameElement::Effect {
                phase: FramePhase::Overlay,
                element: EffectElement::DimOverlay { clip: overlay_clip, alpha }
            } if *overlay_clip == clip && *alpha == 44
        )));
    }

    #[test]
    fn ordered_frame_recipe_preserves_window_local_phase_order() {
        let bounds_a = RectI::new(10, 20, 80, 60).unwrap();
        let bounds_b = RectI::new(40, 50, 90, 70).unwrap();
        let clip = RootSurfaceClip::new(
            RectI::new(8, 16, 96, 78).unwrap(),
            10,
            crate::config::RoundingPower::DEFAULT,
        );
        let items = vec![
            SceneItem {
                state: SceneItemState {
                    key: SceneItemKey::Cursor,
                    surface_key: None,
                    content_key: SceneItemContentKey::NamedCursor(
                        crate::cursor::CursorImageContentKey {
                            width: 1,
                            height: 1,
                            fingerprint: 1,
                        },
                    ),
                    bounds: bounds_a,
                    tex_transform: transform::Mat3::identity(),
                },
                mask: SceneItemMask::None,
                texture: 11,
                owned_texture: None,
                damage: DamageRegion::from_rect(bounds_a),
                sample_damage_padding: 0,
                state_has_alpha: true,
                alpha: u8::MAX,
                opaque_region: DamageRegion::new(),
                opaque: false,
                sync: crate::sync::SurfaceSyncState::default(),
                direct_scanout: None,
                presentation_feedbacks: Vec::new(),
            },
            SceneItem {
                state: SceneItemState {
                    key: SceneItemKey::Cursor,
                    surface_key: None,
                    content_key: SceneItemContentKey::NamedCursor(
                        crate::cursor::CursorImageContentKey {
                            width: 2,
                            height: 2,
                            fingerprint: 2,
                        },
                    ),
                    bounds: bounds_b,
                    tex_transform: transform::Mat3::identity(),
                },
                mask: SceneItemMask::None,
                texture: 22,
                owned_texture: None,
                damage: DamageRegion::from_rect(bounds_b),
                sample_damage_padding: 0,
                state_has_alpha: true,
                alpha: u8::MAX,
                opaque_region: DamageRegion::new(),
                opaque: false,
                sync: crate::sync::SurfaceSyncState::default(),
                direct_scanout: None,
                presentation_feedbacks: Vec::new(),
            },
        ];
        let frame_elements = vec![
            RecordedFrameElement::Effect {
                phase: FramePhase::UnderWindow,
                element: EffectElement::WindowShadow {
                    clip,
                    style: ShadowVisualStyle {
                        radius: 12,
                        offset_x: 0,
                        offset_y: 4,
                        color: crate::config::RgbaColor::new(0, 0, 0, 100),
                        power: crate::config::ShadowPower::DEFAULT,
                    },
                    alpha: 120,
                },
            },
            RecordedFrameElement::Texture { phase: FramePhase::Content, item_index: 0 },
            RecordedFrameElement::Effect {
                phase: FramePhase::OverWindow,
                element: EffectElement::WindowBorder {
                    ring: rounded_ring(clip, RectI::new(14, 22, 72, 54).unwrap()),
                    gradient: BorderGradient {
                        start_color: crate::config::RgbaColor::new(1, 2, 3, 4),
                        end_color: crate::config::RgbaColor::new(5, 6, 7, 8),
                        angle_degrees: 0,
                    },
                    alpha: 200,
                },
            },
            RecordedFrameElement::Texture { phase: FramePhase::Content, item_index: 1 },
        ];

        let recipe = build_output_frame_recipe(&items, &frame_elements);

        assert!(matches!(
            recipe.elements[0],
            crate::render::FrameElement::Effect { phase: FramePhase::UnderWindow, .. }
        ));
        assert!(matches!(
            recipe.elements[1],
            crate::render::FrameElement::Texture { phase: FramePhase::Content, .. }
        ));
        assert!(matches!(
            recipe.elements[2],
            crate::render::FrameElement::Effect { phase: FramePhase::OverWindow, .. }
        ));
        assert!(matches!(
            recipe.elements[3],
            crate::render::FrameElement::Texture { phase: FramePhase::Content, .. }
        ));
    }

    #[test]
    fn translate_recorded_frame_elements_moves_effect_geometry_to_output_local_space() {
        let clip = RootSurfaceClip::new(
            RectI::new(308, 216, 96, 78).unwrap(),
            10,
            crate::config::RoundingPower::DEFAULT,
        );
        let mut frame_elements = vec![
            RecordedFrameElement::Effect {
                phase: FramePhase::UnderWindow,
                element: EffectElement::WindowBlurBackdrop {
                    ring: rounded_ring(clip, RectI::new(314, 222, 84, 66).unwrap()),
                    style: BlurVisualStyle { radius: 12, passes: 2 },
                    alpha: 180,
                },
            },
            RecordedFrameElement::Effect {
                phase: FramePhase::OverWindow,
                element: EffectElement::WindowBorder {
                    ring: rounded_ring(clip, RectI::new(314, 222, 84, 66).unwrap()),
                    gradient: BorderGradient {
                        start_color: crate::config::RgbaColor::new(1, 2, 3, 4),
                        end_color: crate::config::RgbaColor::new(5, 6, 7, 8),
                        angle_degrees: 45,
                    },
                    alpha: 220,
                },
            },
            RecordedFrameElement::Effect {
                phase: FramePhase::UnderWindow,
                element: EffectElement::WindowShadow {
                    clip,
                    style: ShadowVisualStyle {
                        radius: 16,
                        offset_x: 2,
                        offset_y: 6,
                        color: crate::config::RgbaColor::new(0, 0, 0, 120),
                        power: crate::config::ShadowPower::DEFAULT,
                    },
                    alpha: 120,
                },
            },
            RecordedFrameElement::Effect {
                phase: FramePhase::Overlay,
                element: EffectElement::DimOverlay { clip, alpha: 40 },
            },
        ];

        translate_recorded_frame_elements(&mut frame_elements, -300, -200);

        assert!(matches!(
            frame_elements[0],
            RecordedFrameElement::Effect {
                element: EffectElement::WindowBlurBackdrop { ring, .. },
                ..
            } if ring.outer.outer_rect == RectI::new(8, 16, 96, 78).unwrap()
                && ring.inner.outer_rect == RectI::new(14, 22, 84, 66).unwrap()
        ));
        assert!(matches!(
            frame_elements[1],
            RecordedFrameElement::Effect {
                element: EffectElement::WindowBorder { ring, .. },
                ..
            } if ring.outer.outer_rect == RectI::new(8, 16, 96, 78).unwrap()
                && ring.inner.outer_rect == RectI::new(14, 22, 84, 66).unwrap()
        ));
        assert!(matches!(
            frame_elements[2],
            RecordedFrameElement::Effect {
                element: EffectElement::WindowShadow { clip, .. },
                ..
            } if clip.outer_rect == RectI::new(8, 16, 96, 78).unwrap()
        ));
        assert!(matches!(
            frame_elements[3],
            RecordedFrameElement::Effect {
                element: EffectElement::DimOverlay { clip, .. },
                ..
            } if clip.outer_rect == RectI::new(8, 16, 96, 78).unwrap()
        ));
    }

    #[test]
    fn output_frame_recipe_preserves_translated_effect_geometry() {
        let item_bounds = RectI::new(14, 22, 84, 66).unwrap();
        let items = vec![SceneItem {
            state: SceneItemState {
                key: SceneItemKey::Cursor,
                surface_key: None,
                content_key: SceneItemContentKey::NamedCursor(
                    crate::cursor::CursorImageContentKey { width: 1, height: 1, fingerprint: 9 },
                ),
                bounds: item_bounds,
                tex_transform: transform::Mat3::identity(),
            },
            mask: SceneItemMask::None,
            texture: 7,
            owned_texture: None,
            damage: DamageRegion::from_rect(item_bounds),
            sample_damage_padding: 0,
            state_has_alpha: true,
            alpha: u8::MAX,
            opaque_region: DamageRegion::new(),
            opaque: false,
            sync: crate::sync::SurfaceSyncState::default(),
            direct_scanout: None,
            presentation_feedbacks: Vec::new(),
        }];
        let mut frame_elements = vec![
            RecordedFrameElement::Effect {
                phase: FramePhase::UnderWindow,
                element: EffectElement::WindowShadow {
                    clip: RootSurfaceClip::new(
                        RectI::new(308, 216, 96, 78).unwrap(),
                        10,
                        crate::config::RoundingPower::DEFAULT,
                    ),
                    style: ShadowVisualStyle {
                        radius: 14,
                        offset_x: 0,
                        offset_y: 4,
                        color: crate::config::RgbaColor::new(0, 0, 0, 100),
                        power: crate::config::ShadowPower::DEFAULT,
                    },
                    alpha: 120,
                },
            },
            RecordedFrameElement::Texture { phase: FramePhase::Content, item_index: 0 },
            RecordedFrameElement::Effect {
                phase: FramePhase::OverWindow,
                element: EffectElement::WindowBorder {
                    ring: rounded_ring(
                        RootSurfaceClip::new(
                            RectI::new(308, 216, 96, 78).unwrap(),
                            10,
                            crate::config::RoundingPower::DEFAULT,
                        ),
                        RectI::new(314, 222, 84, 66).unwrap(),
                    ),
                    gradient: BorderGradient {
                        start_color: crate::config::RgbaColor::new(1, 2, 3, 4),
                        end_color: crate::config::RgbaColor::new(5, 6, 7, 8),
                        angle_degrees: 0,
                    },
                    alpha: 200,
                },
            },
        ];

        translate_recorded_frame_elements(&mut frame_elements, -300, -200);
        let recipe = build_output_frame_recipe(&items, &frame_elements);

        assert!(matches!(
            recipe.elements[0],
            crate::render::FrameElement::Effect {
                phase: FramePhase::UnderWindow,
                element: EffectElement::WindowShadow { clip, .. }
            } if clip.outer_rect == RectI::new(8, 16, 96, 78).unwrap()
        ));
        assert!(matches!(
            recipe.elements[2],
            crate::render::FrameElement::Effect {
                phase: FramePhase::OverWindow,
                element: EffectElement::WindowBorder { ring, .. }
            } if ring.outer.outer_rect == RectI::new(8, 16, 96, 78).unwrap()
                && ring.inner.outer_rect == RectI::new(14, 22, 84, 66).unwrap()
        ));
    }
}

#[cfg(test)]
mod presentation_timing_tests {
    use {
        super::{
            build_surface_cursor_plane_request, collect_submission_metadata,
            cursor_plane_commit_dimensions, effect_transition_requires_full_redraw,
            find_cursor_slot_index_for_key, presentation_feedback_flags,
            render_planner::select_presentation_output_for_scene, rgba_to_drm_argb8888,
            scene_item_requires_compositor_compositing, schedule_output,
            schedule_outputs_for_rects, select_cursor_upload_slot,
            submitted_frame_feedbacks_for_discard, surface_allows_direct_scanout,
            CursorPlaneContentKey, CursorSurfaceContentKey, OutputSubmission, SceneItem,
            SceneItemContentKey, SceneItemKey, SceneItemMask, SceneItemState,
            SubmissionPresentationFeedback, SubmittedFrame,
        },
        crate::{
            clients::{ClientId, Resolved},
            cursor::{CursorImageContentKey, CursorSurface},
            protocol_generated::{WlBufferId, WlSurfaceId, WpPresentationFeedbackId},
            region::{DamageRegion, RectI},
            runner::Dispatcher,
            scheduler::{output_scheduler_id, OutputSchedulerAction, RequestFrameReason},
            state::{BackendOutputRef, DrmId, ObjectId, OutputId},
            test_support::{state_for_tests, test_output},
            transform,
            wayland_objects::{
                wl_buffer::WlBuffer,
                wl_surface::{SurfacePresentationFeedback, WlSurface},
            },
        },
    };

    fn submission_feedback(id: u32, version: u32) -> SubmissionPresentationFeedback {
        SubmissionPresentationFeedback {
            feedback: crate::clients::Resolved::new(
                ClientId::from_raw(1),
                WpPresentationFeedbackId::from(id),
                crate::state::ObjectId::from(id as usize),
            ),
            version,
            surface: None,
        }
    }

    fn surface_feedback(id: u32, version: u32) -> SurfacePresentationFeedback {
        SurfacePresentationFeedback {
            feedback: crate::clients::Resolved::new(
                ClientId::from_raw(1),
                WpPresentationFeedbackId::from(id),
                crate::state::ObjectId::from(id as usize),
            ),
            version,
            output_id: None,
        }
    }

    fn make_surface_scene_item(
        surface_id: u32,
        bounds: RectI,
        feedbacks: Vec<SurfacePresentationFeedback>,
    ) -> SceneItem {
        let surface_key = Resolved::new(
            ClientId::from_raw(1),
            WlSurfaceId::from(surface_id),
            ObjectId::from(surface_id as usize),
        );
        let buffer = Resolved::new(
            ClientId::from_raw(1),
            WlBufferId::from(surface_id.saturating_add(100)),
            ObjectId::from((surface_id as usize) + 100usize),
        );
        SceneItem {
            state: SceneItemState {
                key: SceneItemKey::Surface(surface_key),
                surface_key: Some(surface_key),
                content_key: SceneItemContentKey::Buffer(buffer),
                bounds,
                tex_transform: transform::Mat3::identity(),
            },
            mask: SceneItemMask::None,
            texture: 0,
            owned_texture: None,
            damage: DamageRegion::from_rect(bounds),
            sample_damage_padding: 0,
            state_has_alpha: true,
            alpha: u8::MAX,
            opaque_region: DamageRegion::new(),
            opaque: false,
            sync: crate::sync::SurfaceSyncState::default(),
            direct_scanout: None,
            presentation_feedbacks: feedbacks,
        }
    }

    #[test]
    fn presentation_flags_do_not_claim_hw_clock() {
        let flags = presentation_feedback_flags(false);
        let hw_clock = crate::protocol_generated::wp_presentation_feedback::Kind::HwClock as u32;
        assert_eq!(flags & hw_clock, 0);
    }

    #[test]
    fn presentation_flags_only_add_zero_copy_for_scanout() {
        let base_flags = presentation_feedback_flags(false);
        let zero_copy_flags = presentation_feedback_flags(true);
        let zero_copy = crate::protocol_generated::wp_presentation_feedback::Kind::ZeroCopy as u32;
        assert_eq!(base_flags & zero_copy, 0);
        assert_ne!(zero_copy_flags & zero_copy, 0);
    }

    #[test]
    fn submitted_frame_discard_helper_returns_feedbacks() {
        let frame = SubmittedFrame::Composited {
            buffer_index: 0,
            damage: DamageRegion::new(),
            submission: OutputSubmission {
                id: 7,
                output_id: Some(OutputId::from(0usize)),
                scene: Vec::new(),
                frame_callback_surfaces: Vec::new(),
                presentation_feedbacks: vec![submission_feedback(4, 2)],
                zero_copy: false,
                had_effects: false,
                scene_build_duration: std::time::Duration::ZERO,
                render_submit_duration: std::time::Duration::ZERO,
                submitted_at: std::time::Instant::now(),
            },
            damage_blob: None,
            cursor_slot: None,
        };

        let (feedbacks, blob) = submitted_frame_feedbacks_for_discard(frame);

        assert_eq!(feedbacks, vec![submission_feedback(4, 2)]);
        assert!(blob.is_none());
    }

    #[test]
    fn cursor_slot_lookup_reuses_matching_content_key() {
        let key = CursorPlaneContentKey::Named(CursorImageContentKey {
            width: 24,
            height: 24,
            fingerprint: 7,
        });
        assert_eq!(find_cursor_slot_index_for_key([Some(key), None], key), Some(0));
        assert_eq!(find_cursor_slot_index_for_key([None, Some(key)], key), Some(1));
    }

    #[test]
    fn cursor_upload_slot_avoids_displayed_slot_when_possible() {
        assert_eq!(select_cursor_upload_slot(Some(0), [true, true]), Some(1));
        assert_eq!(select_cursor_upload_slot(Some(1), [true, true]), Some(0));
        assert_eq!(select_cursor_upload_slot(None, [true, true]), Some(0));
    }

    #[test]
    fn cursor_plane_commit_dimensions_use_fixed_cursor_size() {
        assert_eq!(cursor_plane_commit_dimensions((64, 32)), (64, 32, 64 << 16, 32 << 16));
    }

    #[test]
    fn simple_surface_cursor_can_build_hardware_cursor_request() {
        let mut state = state_for_tests();
        let buffer_object_id = state
            .insert_object(WlBuffer::test_argb8888_shm(
                WlBufferId::from(1),
                ClientId::from_raw(1),
                16,
                12,
                vec![0x80; 16 * 12 * 4],
            ))
            .1;
        let surface_object_id = state
            .insert_object(WlSurface::new(WlSurfaceId::from(2), ClientId::from_raw(1), 6))
            .1;
        {
            let surface = state
                .objects
                .get_mut(surface_object_id)
                .downcast_mut::<WlSurface>()
                .unwrap();
            surface.test_map(buffer_object_id, 4, 5, 16, 12);
        }

        let request = build_surface_cursor_plane_request(
            &state,
            &CursorSurface {
                surface: crate::clients::Resolved::new(
                    ClientId::from_raw(1),
                    WlSurfaceId::from(2),
                    surface_object_id,
                ),
                hotspot_x: 3,
                hotspot_y: 2,
            },
        )
        .unwrap();

        assert_eq!(request.x, -3);
        assert_eq!(request.y, -2);
        assert_eq!(request.width, 16);
        assert_eq!(request.height, 12);
        assert_eq!(
            request.content_key,
            CursorPlaneContentKey::Surface(CursorSurfaceContentKey {
                surface: Resolved::new(
                    ClientId::from_raw(1),
                    WlSurfaceId::from(2),
                    surface_object_id,
                ),
                buffer: Resolved::new(
                    ClientId::from_raw(1),
                    WlBufferId::from(buffer_object_id.index()),
                    buffer_object_id,
                ),
                content_revision: 1,
                width: 16,
                height: 12,
            })
        );
    }

    #[test]
    fn surface_cursor_request_rejects_subsurface_trees() {
        let mut state = state_for_tests();
        let surface_object_id = state
            .insert_object(WlSurface::new(WlSurfaceId::from(10), ClientId::from_raw(1), 6))
            .1;
        {
            let surface = state
                .objects
                .get_mut(surface_object_id)
                .downcast_mut::<WlSurface>()
                .unwrap();
            surface.add_pending_child(crate::clients::Resolved::new(
                ClientId::from_raw(1),
                WlSurfaceId::from(999u32),
                ObjectId::from(999usize),
            ));
        }
        {
            let mut surface = state.objects.grab_object::<WlSurface>(surface_object_id);
            assert!(surface.apply_pending_tree_state(&mut state));
        }

        let request = build_surface_cursor_plane_request(
            &state,
            &CursorSurface {
                surface: crate::clients::Resolved::new(
                    ClientId::from_raw(1),
                    WlSurfaceId::from(10),
                    surface_object_id,
                ),
                hotspot_x: 0,
                hotspot_y: 0,
            },
        );

        assert!(request.is_none());
    }

    #[test]
    fn submission_metadata_keeps_surface_cursor_callbacks_and_feedbacks() {
        let cursor_item = make_surface_scene_item(
            9,
            RectI::new(20, 10, 12, 8).unwrap(),
            vec![surface_feedback(7, 2)],
        );

        let (scene, callback_surfaces, presentation_feedbacks) =
            collect_submission_metadata(&[], &[cursor_item]);

        assert_eq!(scene.len(), 1);
        assert_eq!(
            callback_surfaces,
            vec![Resolved::new(
                ClientId::from_raw(1),
                WlSurfaceId::from(9),
                ObjectId::from(9usize),
            )]
        );
        assert_eq!(
            presentation_feedbacks,
            vec![SubmissionPresentationFeedback {
                feedback: crate::clients::Resolved::new(
                    ClientId::from_raw(1),
                    WpPresentationFeedbackId::from(7),
                    crate::state::ObjectId::from(7usize),
                ),
                version: 2,
                surface: Some(Resolved::new(
                    ClientId::from_raw(1),
                    WlSurfaceId::from(9),
                    ObjectId::from(9usize),
                )),
            }]
        );
    }

    #[test]
    fn cursor_only_logical_scene_still_selects_an_output() {
        let mut state = state_for_tests();
        let drm_id = DrmId::from(0usize);
        let backend = BackendOutputRef::new(drm_id);
        let left = state.insert_output(test_output(backend, 1, 0, 0, 100, 100, ""));
        let right = state.insert_output(test_output(backend, 2, 100, 0, 100, 100, ""));
        let cursor_item = make_surface_scene_item(
            10,
            RectI::new(120, 5, 16, 16).unwrap(),
            vec![surface_feedback(8, 1)],
        );

        let (scene, _, _) = collect_submission_metadata(&[], &[cursor_item]);
        let selected = select_presentation_output_for_scene(&state, drm_id, &scene);

        assert_eq!(selected, Some(right));
        assert_ne!(selected, Some(left));
    }

    #[test]
    fn rgba_to_drm_argb8888_swaps_channels_and_preserves_pitch_padding() {
        let converted = rgba_to_drm_argb8888(
            &[
                0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee,
                0xff, 0x10,
            ],
            2,
            2,
            12,
            2,
        );
        assert_eq!(
            converted,
            vec![
                0x33, 0x22, 0x11, 0x44, 0x77, 0x66, 0x55, 0x88, 0x00, 0x00, 0x00, 0x00, 0xbb, 0xaa,
                0x99, 0xcc, 0xff, 0xee, 0xdd, 0x10, 0x00, 0x00, 0x00, 0x00,
            ]
        );
    }

    #[test]
    fn rgba_to_drm_argb8888_zero_fills_padded_rows() {
        let converted = rgba_to_drm_argb8888(&[0x11, 0x22, 0x33, 0x44], 1, 1, 8, 3);
        assert_eq!(
            converted,
            vec![
                0x33, 0x22, 0x11, 0x44, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            ]
        );
    }

    #[test]
    fn select_presentation_output_prefers_largest_overlap() {
        let mut state = state_for_tests();
        let drm_id = DrmId::from(0usize);
        let backend = BackendOutputRef::new(drm_id);
        let left = state.insert_output(test_output(backend, 1, 0, 0, 100, 100, ""));
        let right = state.insert_output(test_output(backend, 2, 100, 0, 100, 100, ""));
        let surface_key =
            Resolved::new(ClientId::from_raw(1), WlSurfaceId::from(1), ObjectId::from(1usize));
        let buffer =
            Resolved::new(ClientId::from_raw(1), WlBufferId::from(1), ObjectId::from(1usize));
        let scene = vec![SceneItemState {
            key: SceneItemKey::Surface(surface_key),
            surface_key: Some(surface_key),
            content_key: SceneItemContentKey::Buffer(buffer),
            bounds: RectI::new(80, 0, 80, 100).unwrap(),
            tex_transform: transform::Mat3::identity(),
        }];

        let selected = select_presentation_output_for_scene(&state, drm_id, &scene);

        assert_eq!(selected, Some(right));
        assert_ne!(selected, Some(left));
    }

    #[test]
    fn select_presentation_output_breaks_equal_overlap_by_lowest_output_id() {
        let mut state = state_for_tests();
        let drm_id = DrmId::from(0usize);
        let backend = BackendOutputRef::new(drm_id);
        let left = state.insert_output(test_output(backend, 1, 0, 0, 100, 100, ""));
        let right = state.insert_output(test_output(backend, 2, 100, 0, 100, 100, ""));
        let surface_key =
            Resolved::new(ClientId::from_raw(1), WlSurfaceId::from(2), ObjectId::from(2usize));
        let buffer =
            Resolved::new(ClientId::from_raw(1), WlBufferId::from(2), ObjectId::from(2usize));
        let scene = vec![SceneItemState {
            key: SceneItemKey::Surface(surface_key),
            surface_key: Some(surface_key),
            content_key: SceneItemContentKey::Buffer(buffer),
            bounds: RectI::new(50, 0, 100, 100).unwrap(),
            tex_transform: transform::Mat3::identity(),
        }];

        let selected = select_presentation_output_for_scene(&state, drm_id, &scene);

        assert_eq!(selected, Some(left));
        assert_ne!(selected, Some(right));
    }

    #[test]
    fn schedule_output_targets_logical_output_model_id() {
        let mut state = state_for_tests();
        let drm_id = DrmId::from(0usize);
        let backend = BackendOutputRef::new(drm_id);
        let left = state.insert_output(test_output(backend, 1, 0, 0, 100, 100, ""));
        let right = state.insert_output(test_output(backend, 2, 100, 0, 100, 100, ""));
        let mut dispatcher = Dispatcher::default();

        schedule_output(&state, right, &mut dispatcher);

        assert!(dispatcher.test_has_queued_action_for::<OutputSchedulerAction, _>(
            output_scheduler_id(right),
            |action| matches!(
                action,
                OutputSchedulerAction::RequestFrame { reason: RequestFrameReason::Damage }
            )
        ));
        assert!(!dispatcher.test_has_queued_action_for::<OutputSchedulerAction, _>(
            output_scheduler_id(left),
            |action| matches!(
                action,
                OutputSchedulerAction::RequestFrame { reason: RequestFrameReason::Damage }
            )
        ));
    }

    #[test]
    fn schedule_outputs_for_rects_marks_old_and_new_outputs() {
        let mut state = state_for_tests();
        let drm_id = DrmId::from(0usize);
        let backend = BackendOutputRef::new(drm_id);
        let left = state.insert_output(test_output(backend, 1, 0, 0, 100, 100, ""));
        let right = state.insert_output(test_output(backend, 2, 100, 0, 100, 100, ""));
        let mut dispatcher = Dispatcher::default();

        schedule_outputs_for_rects(
            &state,
            [RectI::new(0, 0, 100, 100).unwrap(), RectI::new(100, 0, 100, 100).unwrap()],
            &mut dispatcher,
        );

        assert!(dispatcher.test_has_queued_action_for::<OutputSchedulerAction, _>(
            output_scheduler_id(left),
            |action| matches!(
                action,
                OutputSchedulerAction::RequestFrame { reason: RequestFrameReason::Damage }
            )
        ));
        assert!(dispatcher.test_has_queued_action_for::<OutputSchedulerAction, _>(
            output_scheduler_id(right),
            |action| matches!(
                action,
                OutputSchedulerAction::RequestFrame { reason: RequestFrameReason::Damage }
            )
        ));
    }

    fn direct_scanout_requires_surface_without_compositor_processing() {
        let mut item = make_surface_scene_item(30, RectI::new(0, 0, 100, 100).unwrap(), Vec::new());
        item.alpha = 200;
        assert!(scene_item_requires_compositor_compositing(&item));

        item.alpha = u8::MAX;
        item.mask = SceneItemMask::RoundedClip {
            clip: crate::state::RootSurfaceClip::new(
                RectI::new(0, 0, 100, 100).unwrap(),
                12,
                crate::config::RoundingPower::DEFAULT,
            ),
        };
        assert!(scene_item_requires_compositor_compositing(&item));
    }

    #[test]
    fn candidate_scanout_requires_full_opacity_no_dim_and_no_clip() {
        assert!(surface_allows_direct_scanout(None, u8::MAX));
        assert!(!surface_allows_direct_scanout(None, 200));
        assert!(!surface_allows_direct_scanout(
            Some(crate::state::RootSurfaceClip::new(
                RectI::new(0, 0, 100, 100).unwrap(),
                8,
                crate::config::RoundingPower::DEFAULT,
            )),
            u8::MAX,
        ));
    }

    #[test]
    fn effect_transitions_force_full_redraw_on_enter_and_exit() {
        assert!(effect_transition_requires_full_redraw(true, false));
        assert!(effect_transition_requires_full_redraw(false, true));
        assert!(effect_transition_requires_full_redraw(true, true));
        assert!(!effect_transition_requires_full_redraw(false, false));
    }
}

#[cfg(test)]
mod surface_tree_render_tests {
    use {
        super::{collect_cursor_surface_snapshots, collect_surface_snapshots},
        crate::{
            clients::ClientId,
            protocol_generated::{
                WlSubsurfaceId, WlSurfaceId, XdgPopupId, XdgSurfaceId, XdgWmBaseId,
            },
            state::ObjectId,
            test_support::{
                insert_root_surface as insert_surface,
                insert_wl_subsurface_client1 as insert_subsurface, resolved_wl_surface as root,
                state_for_tests,
            },
            wayland_objects::{
                wl_surface::WlSurface, xdg_popup::XdgPopup, xdg_positioner::PositionerState,
                xdg_surface::XdgSurface,
            },
        },
    };

    fn insert_test_xdg_surface(
        state: &mut crate::state::State,
        client_id: ClientId,
        wl_surface_id: WlSurfaceId,
        xdg_surface_id: XdgSurfaceId,
    ) -> (crate::clients::Resolved<WlSurfaceId>, crate::clients::Resolved<XdgSurfaceId>) {
        let (_, surface_object_id) =
            state.insert_object(WlSurface::new(wl_surface_id, client_id, 6));
        let wl_surface = crate::clients::Resolved::new(client_id, wl_surface_id, surface_object_id);
        state.register_root_surface(wl_surface);
        let (_, xdg_surface_object_id) = state.insert_object(XdgSurface::new(
            xdg_surface_id,
            crate::clients::Resolved::new(client_id, XdgWmBaseId::from(1), ObjectId::from(1usize)),
            wl_surface,
            7,
        ));
        let xdg_surface =
            crate::clients::Resolved::new(client_id, xdg_surface_id, xdg_surface_object_id);
        state
            .objects
            .get_mut(surface_object_id)
            .downcast_mut::<WlSurface>()
            .unwrap()
            .set_xdg_surface_impl(xdg_surface);
        (wl_surface, xdg_surface)
    }

    #[test]
    fn collect_surface_snapshots_walks_below_self_above() {
        let mut state = state_for_tests();
        let root_object_id = insert_surface(&mut state, 1);
        let below = insert_surface(&mut state, 2);
        let above = insert_surface(&mut state, 3);
        let root_resolved = root(&state, root_object_id);
        let below_resolved = root(&state, below);
        let above_resolved = root(&state, above);
        let below_subsurface =
            insert_subsurface(&mut state, WlSubsurfaceId::from(11), below, root_object_id);
        let above_subsurface =
            insert_subsurface(&mut state, WlSubsurfaceId::from(12), above, root_object_id);

        {
            let root_surface = state
                .objects
                .get_mut(root_object_id)
                .downcast_mut::<WlSurface>()
                .unwrap();
            root_surface.test_map(ObjectId::from(100usize), 10, 20, 50, 40);
            root_surface.add_pending_child(below_resolved);
            root_surface.add_pending_child(above_resolved);
            assert!(root_surface.restack_child_relative(below_resolved, None, false));
            assert!(root_surface.restack_child_relative(above_resolved, None, true));
        }
        {
            let below_surface = state
                .objects
                .get_mut(below)
                .downcast_mut::<WlSurface>()
                .unwrap();
            below_surface.assign_subsurface(below_subsurface, root_resolved);
            below_surface.set_subsurface_position(2, 3);
            below_surface.test_map(ObjectId::from(101usize), 0, 0, 10, 10);
        }
        {
            let above_surface = state
                .objects
                .get_mut(above)
                .downcast_mut::<WlSurface>()
                .unwrap();
            above_surface.assign_subsurface(above_subsurface, root_resolved);
            above_surface.set_subsurface_position(7, 8);
            above_surface.test_map(ObjectId::from(102usize), 0, 0, 10, 10);
        }
        {
            let mut root_surface = state.objects.grab_object::<WlSurface>(root_object_id);
            assert!(root_surface.apply_pending_tree_state(&mut state));
        }

        let mut retired = Vec::new();
        let mut snapshots = Vec::new();
        let root_resolved = root(&state, root_object_id);
        collect_surface_snapshots(
            &mut state,
            root_resolved,
            0,
            0,
            true,
            &mut retired,
            &mut snapshots,
        );

        let ids: Vec<_> = snapshots
            .iter()
            .map(|snapshot| snapshot.buffer.object_id())
            .collect();
        let positions: Vec<_> = snapshots
            .iter()
            .map(|snapshot| (snapshot.x, snapshot.y))
            .collect();
        assert_eq!(
            ids,
            vec![ObjectId::from(101usize), ObjectId::from(100usize), ObjectId::from(102usize),]
        );
        assert_eq!(positions, vec![(12, 23), (10, 20), (17, 28)]);
    }

    #[test]
    fn new_subsurface_defaults_below_parent() {
        let mut state = state_for_tests();
        let root_object_id = insert_surface(&mut state, 1);
        let child = insert_surface(&mut state, 2);
        let root_resolved = root(&state, root_object_id);
        let child_resolved = root(&state, child);
        let child_subsurface =
            insert_subsurface(&mut state, WlSubsurfaceId::from(31), child, root_object_id);

        {
            let root_surface = state
                .objects
                .get_mut(root_object_id)
                .downcast_mut::<WlSurface>()
                .unwrap();
            root_surface.test_map(ObjectId::from(120usize), 10, 20, 50, 40);
            root_surface.add_pending_child(child_resolved);
        }
        {
            let child_surface = state
                .objects
                .get_mut(child)
                .downcast_mut::<WlSurface>()
                .unwrap();
            child_surface.assign_subsurface(child_subsurface, root_resolved);
            child_surface.set_subsurface_position(3, 4);
            child_surface.test_map(ObjectId::from(121usize), 0, 0, 10, 10);
        }
        {
            let mut root_surface = state.objects.grab_object::<WlSurface>(root_object_id);
            assert!(root_surface.apply_pending_tree_state(&mut state));
        }

        let mut retired = Vec::new();
        let mut snapshots = Vec::new();
        let root_resolved = root(&state, root_object_id);
        collect_surface_snapshots(
            &mut state,
            root_resolved,
            0,
            0,
            true,
            &mut retired,
            &mut snapshots,
        );
        let ids: Vec<_> = snapshots
            .iter()
            .map(|snapshot| snapshot.buffer.object_id())
            .collect();
        assert_eq!(ids, vec![ObjectId::from(121usize), ObjectId::from(120usize)]);
    }

    #[test]
    fn collect_surface_snapshots_accumulates_nested_subsurface_offsets() {
        let mut state = state_for_tests();
        let root_object_id = insert_surface(&mut state, 1);
        let child = insert_surface(&mut state, 2);
        let grandchild = insert_surface(&mut state, 3);
        let root_resolved = root(&state, root_object_id);
        let child_resolved = root(&state, child);
        let grandchild_resolved = root(&state, grandchild);
        let child_subsurface =
            insert_subsurface(&mut state, WlSubsurfaceId::from(21), child, root_object_id);
        let grandchild_subsurface =
            insert_subsurface(&mut state, WlSubsurfaceId::from(22), grandchild, child);

        {
            let root_surface = state
                .objects
                .get_mut(root_object_id)
                .downcast_mut::<WlSurface>()
                .unwrap();
            root_surface.test_map(ObjectId::from(110usize), 100, 200, 40, 30);
            root_surface.add_pending_child(child_resolved);
        }
        {
            let child_surface = state
                .objects
                .get_mut(child)
                .downcast_mut::<WlSurface>()
                .unwrap();
            child_surface.assign_subsurface(child_subsurface, root_resolved);
            child_surface.set_subsurface_position(5, 7);
            child_surface.test_map(ObjectId::from(111usize), 1, 2, 20, 10);
            child_surface.add_pending_child(grandchild_resolved);
        }
        {
            let grandchild_surface = state
                .objects
                .get_mut(grandchild)
                .downcast_mut::<WlSurface>()
                .unwrap();
            grandchild_surface.assign_subsurface(grandchild_subsurface, child_resolved);
            grandchild_surface.set_subsurface_position(11, 13);
            grandchild_surface.test_map(ObjectId::from(112usize), 3, 4, 8, 6);
        }
        {
            let mut root_surface = state.objects.grab_object::<WlSurface>(root_object_id);
            assert!(root_surface.apply_pending_tree_state(&mut state));
        }
        {
            let mut child_surface = state.objects.grab_object::<WlSurface>(child);
            assert!(child_surface.apply_pending_tree_state(&mut state));
        }

        let mut retired = Vec::new();
        let mut snapshots = Vec::new();
        let root_resolved = root(&state, root_object_id);
        collect_surface_snapshots(
            &mut state,
            root_resolved,
            0,
            0,
            true,
            &mut retired,
            &mut snapshots,
        );

        let positions: Vec<_> = snapshots
            .iter()
            .map(|snapshot| (snapshot.x, snapshot.y))
            .collect();
        assert_eq!(positions, vec![(120, 226), (106, 209), (100, 200)]);
    }

    #[test]
    fn collect_cursor_surface_snapshots_walks_cursor_subsurface_tree() {
        let mut state = state_for_tests();
        let root_object_id = insert_surface(&mut state, 1);
        let child = insert_surface(&mut state, 2);
        let root_resolved = root(&state, root_object_id);
        let child_resolved = root(&state, child);
        let child_subsurface =
            insert_subsurface(&mut state, WlSubsurfaceId::from(81), child, root_object_id);

        {
            let root_surface = state
                .objects
                .get_mut(root_object_id)
                .downcast_mut::<WlSurface>()
                .unwrap();
            root_surface.test_map(ObjectId::from(130usize), 10, 15, 20, 20);
            root_surface.add_pending_child(child_resolved);
            assert!(root_surface.restack_child_relative(child_resolved, None, true));
        }
        {
            let child_surface = state
                .objects
                .get_mut(child)
                .downcast_mut::<WlSurface>()
                .unwrap();
            child_surface.assign_subsurface(child_subsurface, root_resolved);
            child_surface.set_subsurface_position(4, 5);
            child_surface.test_map(ObjectId::from(131usize), 0, 0, 8, 9);
        }
        {
            let mut root_surface = state.objects.grab_object::<WlSurface>(root_object_id);
            assert!(root_surface.apply_pending_tree_state(&mut state));
        }

        state.seat.set_cursor_surface(
            crate::clients::Resolved::new(
                ClientId::from_raw(1),
                WlSurfaceId::from(1),
                root_object_id,
            ),
            3,
            4,
        );
        let snapshots = collect_cursor_surface_snapshots(&mut state, &mut Vec::new());

        let ids: Vec<_> = snapshots
            .iter()
            .map(|snapshot| {
                snapshot
                    .surface
                    .expect("snapshot carries resolved surface")
                    .wayland_id()
            })
            .collect();
        assert_eq!(ids, vec![WlSurfaceId::from(1), WlSurfaceId::from(2)]);
    }

    #[test]
    fn collect_surface_snapshots_places_popups_relative_to_parent_without_root_clip() {
        let mut state = state_for_tests();
        let client_id = ClientId::from_raw(1);
        let (parent_wl, parent_xdg) = insert_test_xdg_surface(
            &mut state,
            client_id,
            WlSurfaceId::from(90),
            XdgSurfaceId::from(91),
        );
        let (popup_wl, popup_xdg) = insert_test_xdg_surface(
            &mut state,
            client_id,
            WlSurfaceId::from(92),
            XdgSurfaceId::from(93),
        );
        let (nested_wl, nested_xdg) = insert_test_xdg_surface(
            &mut state,
            client_id,
            WlSurfaceId::from(94),
            XdgSurfaceId::from(95),
        );

        state
            .objects
            .get_mut(parent_wl.object_id())
            .downcast_mut::<WlSurface>()
            .unwrap()
            .test_map(ObjectId::from(300usize), 640, 0, 640, 800);
        state
            .objects
            .get_mut(parent_wl.object_id())
            .downcast_mut::<WlSurface>()
            .unwrap()
            .set_root_position(640, 0);
        state
            .objects
            .get_mut(popup_wl.object_id())
            .downcast_mut::<WlSurface>()
            .unwrap()
            .test_map(ObjectId::from(301usize), 0, 0, 299, 457);
        state
            .objects
            .get_mut(nested_wl.object_id())
            .downcast_mut::<WlSurface>()
            .unwrap()
            .test_map(ObjectId::from(302usize), 0, 0, 110, 140);

        let (_, popup_object_id) = state.insert_object(XdgPopup::new(
            XdgPopupId::from(96),
            popup_xdg,
            Some(parent_xdg),
            PositionerState::default(),
            7,
        ));
        let popup = crate::clients::Resolved::new(client_id, XdgPopupId::from(96), popup_object_id);
        state
            .objects
            .get_mut(popup_object_id)
            .downcast_mut::<XdgPopup>()
            .unwrap()
            .apply_acked_configure(crate::region::RectI { x: 387, y: 40, width: 253, height: 411 });
        state
            .objects
            .get_mut(popup_xdg.object_id())
            .downcast_mut::<XdgSurface>()
            .unwrap()
            .test_set_current_window_geometry(Some(crate::region::RectI {
                x: 23,
                y: 21,
                width: 253,
                height: 411,
            }));
        state
            .objects
            .get_mut(popup_xdg.object_id())
            .downcast_mut::<XdgSurface>()
            .unwrap()
            .test_set_popup_role(popup);
        state
            .objects
            .get_mut(parent_xdg.object_id())
            .downcast_mut::<XdgSurface>()
            .unwrap()
            .add_popup_child(popup);

        let (_, nested_popup_object_id) = state.insert_object(XdgPopup::new(
            XdgPopupId::from(97),
            nested_xdg,
            Some(popup_xdg),
            PositionerState::default(),
            7,
        ));
        let nested_popup =
            crate::clients::Resolved::new(client_id, XdgPopupId::from(97), nested_popup_object_id);
        state
            .objects
            .get_mut(nested_popup_object_id)
            .downcast_mut::<XdgPopup>()
            .unwrap()
            .apply_acked_configure(crate::region::RectI { x: 50, y: 60, width: 100, height: 120 });
        state
            .objects
            .get_mut(nested_xdg.object_id())
            .downcast_mut::<XdgSurface>()
            .unwrap()
            .test_set_current_window_geometry(Some(crate::region::RectI {
                x: 5,
                y: 6,
                width: 100,
                height: 120,
            }));
        state
            .objects
            .get_mut(nested_xdg.object_id())
            .downcast_mut::<XdgSurface>()
            .unwrap()
            .test_set_popup_role(nested_popup);
        state
            .objects
            .get_mut(popup_xdg.object_id())
            .downcast_mut::<XdgSurface>()
            .unwrap()
            .add_popup_child(nested_popup);

        let mut retired = Vec::new();
        let mut snapshots = Vec::new();
        collect_surface_snapshots(&mut state, parent_wl, 0, 0, true, &mut retired, &mut snapshots);

        let positions: Vec<_> = snapshots
            .iter()
            .map(|snapshot| (snapshot.x, snapshot.y))
            .collect();
        let clip_flags: Vec<_> = snapshots
            .iter()
            .map(|snapshot| snapshot.inherit_root_clip)
            .collect();
        assert_eq!(positions, vec![(640, 0), (1004, 19), (1049, 73)]);
        assert_eq!(clip_flags, vec![true, false, false]);
    }
}
