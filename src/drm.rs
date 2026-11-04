use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    mem::ManuallyDrop,
    num::NonZeroU32,
    os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd, RawFd},
    rc::Rc,
    time::{Duration, Instant},
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
    cursor::{CursorImage, CursorImageContentKey, CursorSourceRef},
    dmabuf::DmaBuf,
    gbm::{BufferObject, GbmDevice, UseFlags},
    io_uring::{
        model::IoUringAction,
        ring::{PollCompleted, ReadWriteCompleted, RingId},
    },
    object::Object,
    region::{DamageRegion, RectI},
    render::{
        create_renderer, EffectElement, FrameElement, FramePhase, OutputFrameRecipe, RenderFrame,
        RenderTarget, Renderer, RootSurfaceVisualState, RoundedRing, ShmImage, Texture,
        TextureElement, TextureMask, TextureSource, WindowClip,
    },
    runner::{isolate_session_fault, Dispatcher, Model, ModelInstanceId, SessionFault},
    scene::{ComposedEntry, CompositionTier, OutputCompositionView},
    state::{
        BackendOutputRef, ColorTransformMatrix, DrmId, ObjectId, OutputId, OutputMode,
        RootSurfaceClip, State, WorkspaceId,
    },
    sync::{DeferredRelease, RetiredBufferCandidate},
    transform::{self, OutputPresentationTransform},
    visual_animation::{FrozenClosingBlur, RetainedClosingSnapshot, WorkspaceMotion},
    wayland_objects::{
        wl_buffer::{CursorShmBuffer, WlBuffer},
        wl_surface::{
            Role, StackEntry, SurfacePresentationFeedback, SurfaceRenderState, WlSurface,
        },
    },
    ENABLE_DAMAGE,
};

use crate::{
    clients::{ClientAction, ClientId, ClientManagerAction, Resolved},
    protocol_generated::{
        WlBufferId, WlSurfaceId, WpPresentationFeedbackId, XdgPopupId, XdgSurfaceId,
    },
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
    pub outputs: BTreeMap<u32, DrmOutput>,
    renderer: Box<dyn Renderer>,
    gbm: GbmDevice,
    driver: Driver,
    supports_syncobj_timeline: bool,
    supports_syncobj_eventfd: bool,
    /// Bumped on each in-place restore. Flip events armed earlier are ignored.
    event_generation: u64,
}

impl Drop for Drm {
    fn drop(&mut self) {
        // Output buffers drop while the renderer and GBM device are still alive:
        // GPU target, then DRM framebuffer, then GBM BO. The renderer field then
        // terminates EGL, and the GBM device drops after that.
        self.renderer.make_current();
        self.outputs.clear();
    }
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
    ClosingSnapshot(u64),
    LockBlank,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SceneItemContentKey {
    Buffer(Resolved<WlBufferId>),
    NamedCursor(CursorImageContentKey),
    ClosingSnapshot { id: u64, alpha: u8 },
    LockBlank { red: bool },
}

#[derive(Clone, Debug)]
struct SceneItemState {
    key: SceneItemKey,
    surface_key: Option<SurfaceKey>,
    content_key: SceneItemContentKey,
    bounds: RectI,
    tex_transform: transform::Mat3,
    alpha: u8,
    mask: SceneItemMask,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SceneItemMask {
    None,
    RoundedClip { clip: RootSurfaceClip },
    CornerRoundedClip { clip: RootSurfaceClip },
    FrameRing { ring: RoundedRing },
}

#[derive(Debug)]
struct SceneItem {
    state: SceneItemState,
    mask: SceneItemMask,
    texture: Rc<dyn Texture>,
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
    /// This frame is the lock blank and lock surfaces. Its page flip may satisfy the barrier.
    protected: bool,
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
    /// Queuing this frame is not presentation. The flip completion is.
    protected: bool,
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
        let buffer = self.render_buffer();
        RectI::new(0, 0, buffer.width, buffer.height)
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
        protected: bool,
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
            protected,
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
        let buffer = self.render_buffer();
        transform::output_projection(
            buffer.width as f32,
            buffer.height as f32,
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

    fn queue_pending_damage(&mut self, rect: RectI) {
        self.pipeline.queue_pending_damage(rect);
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
    pub(crate) fn from_owned(fd: OwnedFd) -> Self {
        Self { fd: Rc::new(fd) }
    }

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

nix::ioctl_none!(drm_set_master, b'd', 0x1e);

impl DrmFd {
    /// Become DRM master on this fd. Logind's resume fd is a different master;
    /// closing it and calling this keeps framebuffer ids valid on the retained fd.
    pub(crate) fn set_master(&self) -> std::io::Result<()> {
        unsafe { drm_set_master(self.as_raw_fd()) }
            .map(|_| ())
            .map_err(std::io::Error::from)
    }
}

fn apply_output_disables(crtc_ids: &[u32], mut disable: impl FnMut(u32) -> Result<(), String>) {
    for &crtc_id in crtc_ids {
        if let Err(error) = disable(crtc_id) {
            log::warn!("failed to disable CRTC {crtc_id}: {error}");
        }
    }
}

fn output_disable_request(in_use: &ConnectorInUse) -> control::atomic::AtomicModeReq {
    let mut changes = control::atomic::AtomicModeReq::new();
    if let Some(active_prop) = in_use.crtc.active_prop {
        changes.add_property(
            in_use.crtc.handle,
            active_prop,
            control::property::Value::UnsignedRange(0),
        );
    } else {
        log::warn!("CRTC {:?} has no ACTIVE property", in_use.crtc.handle);
    }
    if let Some(mode_id_prop) = in_use.crtc.mode_id_prop {
        changes.add_property(in_use.crtc.handle, mode_id_prop, control::property::Value::Blob(0));
    }
    if let Some(crtc_id_prop) = in_use.connector.crtc_id_prop {
        changes.add_property(
            in_use.connector.handler,
            crtc_id_prop,
            control::property::Value::CRTC(None),
        );
    }
    for plane in &in_use.planes {
        changes.add_property(plane.handle, plane.fb_id, control::property::Value::Object(None));
        changes.add_property(plane.handle, plane.crtc_id, control::property::Value::CRTC(None));
    }
    changes
}

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

pub use crate::render::RenderSyncMode;

#[derive(Clone, Debug)]
struct Crtc {
    handle: crtc::Handle,
    index: usize,
    possible_planes: HashSet<plane::Handle>,
    active: bool,
    active_prop: Option<control::property::Handle>,
    mode_id: u32,
    mode_id_prop: Option<control::property::Handle>,
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
    pub crtc_id_prop: Option<control::property::Handle>,
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
    pub(crate) fn bind_context(&self) {
        self.renderer.make_current();
    }

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

    pub fn create_drm(
        fd: OwnedFd,
        device: udev::Device,
        selection: crate::config::RendererSelection,
    ) -> Result<Self, String> {
        let drm = DrmFd { fd: Rc::new(fd) };
        set_nonblocking(drm.as_raw_fd()).map_err(|error| format!("set nonblocking: {error}"))?;
        match drm.set_client_capability(drm::ClientCapability::UniversalPlanes, true) {
            Ok(()) => log::info!("Enabled DRM client capability: UniversalPlanes"),
            Err(error) => {
                log::warn!("Failed to enable DRM client capability UniversalPlanes: {:?}", error)
            }
        }
        drm.set_client_capability(drm::ClientCapability::Atomic, true)
            .map(|()| log::info!("Enabled DRM client capability: Atomic"))
            .map_err(|error| format!("atomic capability: {error}"))?;
        match drm.set_client_capability(drm::ClientCapability::CursorPlaneHotspot, true) {
            Ok(()) => log::info!("Enabled DRM client capability: CursorPlaneHotspot"),
            Err(error) => {
                log::warn!("Failed to enable DRM client capability CursorPlaneHotspot: {:?}", error)
            }
        }

        let driver: Driver = drm
            .get_driver()
            .map_err(|error| format!("driver: {error}"))?
            .into();
        log::info!(
            "Initializing DRM backend: devnode={:?} driver={} desc={}",
            device.devnode(),
            driver.name,
            driver.desc
        );
        let resources = Self::list_resources(&drm)?;
        log::info!(
            "Discovered DRM resources: connectors={} crtcs={} planes={}",
            resources.connectors.len(),
            resources.crtcs.len(),
            resources.planes.len()
        );

        let gbm = GbmDevice::new(&drm).map_err(|error| format!("gbm: {error}"))?;
        let mut renderer = create_renderer(selection, &drm, &gbm)?;
        let outputs = Self::init_drm_outputs(&drm, renderer.as_mut(), &gbm, &resources);
        if outputs.is_empty() {
            return Err("no usable connector".into());
        }

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
            outputs,
            renderer,
            gbm,
            driver,
            supports_syncobj_timeline,
            supports_syncobj_eventfd,
            event_generation: 0,
        };
        let drm_output_ids = drm_backend.outputs.keys().copied().collect::<Vec<_>>();
        for drm_output_id in drm_output_ids {
            drm_backend.initialize_cursor_plane_slots(drm_output_id);
        }
        Ok(drm_backend)
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

    pub(crate) fn reclaim_master(&self) -> std::io::Result<()> {
        self.fd.set_master()
    }

    pub fn devnum(&self) -> Option<libc::dev_t> {
        self.device.devnum()
    }

    /// Best-effort atomic disable of every active output. A failed commit does
    /// not stop the remaining outputs. This does not restore the mode that was
    /// active before the compositor started.
    pub fn disable_outputs(&mut self) {
        let crtc_ids: Vec<u32> = self.outputs.keys().copied().collect();
        apply_output_disables(&crtc_ids, |crtc_id| self.disable_output(crtc_id));
    }

    fn disable_output(&mut self, crtc_id: u32) -> Result<(), String> {
        let changes = {
            let Some(output) = self.outputs.get(&crtc_id) else {
                return Ok(());
            };
            output_disable_request(&output.in_use)
        };
        self.fd
            .atomic_commit(control::AtomicCommitFlags::ALLOW_MODESET, changes)
            .map_err(|error| format!("{error:?}"))
    }

    /// Refresh connector state and modeset every retained output with its scanout buffer.
    /// Failed CRTCs are returned so the caller can remove only those outputs.
    pub(crate) fn restore_outputs_in_place(
        &mut self,
    ) -> Vec<(
        u32,
        Result<
            (Vec<DeferredRelease>, Vec<SubmissionPresentationFeedback>, Option<u64>),
            OutputRestoreError,
        >,
    )> {
        let crtc_ids: Vec<u32> = self.outputs.keys().copied().collect();
        crtc_ids
            .into_iter()
            .map(|crtc_id| (crtc_id, self.restore_output(crtc_id)))
            .collect()
    }

    fn restore_output(
        &mut self,
        drm_crtc_id: u32,
    ) -> Result<
        (Vec<DeferredRelease>, Vec<SubmissionPresentationFeedback>, Option<u64>),
        OutputRestoreError,
    > {
        self.refresh_retained_output(drm_crtc_id)
            .map_err(OutputRestoreError::Refresh)?;
        let mode = self
            .output(drm_crtc_id)
            .and_then(|output| output.in_use.connector.preferred_mode)
            .ok_or_else(|| OutputRestoreError::Refresh("connector has no retained mode".into()))?;
        let raw_mode = Self::modeinfo_from_mode(mode);
        let mode_blob = self
            .fd
            .create_property_blob(&raw_mode)
            .map_err(|error| OutputRestoreError::Refresh(format!("mode blob: {error}")))?;
        let mode_blob_id = mode_blob
            .as_blob()
            .ok_or_else(|| OutputRestoreError::Refresh("mode blob id missing".into()))?;
        let changes = {
            let Some(output) = self.output(drm_crtc_id) else {
                let _ = self.fd.destroy_property_blob(mode_blob_id);
                return Err(OutputRestoreError::Refresh("missing DRM output".into()));
            };
            Self::restore_modeset_request(output, mode_blob_id)
        };
        if let Err(error) = self
            .fd
            .atomic_commit(control::AtomicCommitFlags::ALLOW_MODESET, changes)
        {
            let _ = self.fd.destroy_property_blob(mode_blob_id);
            return Err(OutputRestoreError::Modeset(error.to_string()));
        }
        let (previous, reset) = {
            let Some(output) = self.output_mut(drm_crtc_id) else {
                return Err(OutputRestoreError::Modeset("missing DRM output".into()));
            };
            let previous = std::mem::replace(&mut output.in_use.crtc.mode_id, mode_blob_id as u32);
            output.in_use.crtc.active = true;
            let bounds = output.output_bounds();
            let reset = output.pipeline.reset_after_session_resume();
            output.pipeline.resume_commits();
            output.pipeline.queue_pending_damage(bounds);
            (previous, reset)
        };
        if previous != 0 && u64::from(previous) != mode_blob_id {
            let _ = self.fd.destroy_property_blob(u64::from(previous));
        }
        Ok(reset)
    }

    fn refresh_retained_output(&mut self, drm_crtc_id: u32) -> Result<(), String> {
        let (connector_handle, retained_mode, crtc_handle, plane_handle) = {
            let output = self
                .output(drm_crtc_id)
                .ok_or_else(|| "missing DRM output".to_string())?;
            let (plane, _) = output.in_use.primary_plane();
            (
                output.in_use.connector.handler,
                output.in_use.connector.preferred_mode,
                output.in_use.crtc.handle,
                plane.handle,
            )
        };
        let info = self
            .fd
            .get_connector(connector_handle, true)
            .map_err(|error| format!("connector refresh: {error}"))?;
        let connected = matches!(info.state(), connector::State::Connected);
        let advertised = retained_mode.is_some_and(|mode| {
            info.modes()
                .iter()
                .any(|candidate| candidate.size() == mode.size())
        });
        if let Some(reason) =
            Self::retained_assignment_failure(connected, retained_mode.is_some(), advertised)
        {
            return Err(reason.into());
        }
        let crtc_props = Self::readable_properties(&self.fd, crtc_handle)?;
        let plane_props = Self::readable_properties(&self.fd, plane_handle)?;
        let connector_props = Self::readable_properties(&self.fd, connector_handle)?;
        let active_prop = crtc_props
            .get("ACTIVE")
            .map(|(_, info)| info.handle())
            .ok_or_else(|| "CRTC has no ACTIVE property".to_string())?;
        let mode_id_prop = crtc_props
            .get("MODE_ID")
            .map(|(_, info)| info.handle())
            .ok_or_else(|| "CRTC has no MODE_ID property".to_string())?;
        let fb_id = plane_props
            .get("FB_ID")
            .map(|(_, info)| info.handle())
            .ok_or_else(|| "primary plane has no FB_ID property".to_string())?;
        let plane_crtc = plane_props
            .get("CRTC_ID")
            .map(|(_, info)| info.handle())
            .ok_or_else(|| "primary plane has no CRTC_ID property".to_string())?;
        let connector_crtc = connector_props
            .get("CRTC_ID")
            .map(|(_, info)| info.handle())
            .ok_or_else(|| "connector has no CRTC_ID property".to_string())?;
        let output = self.output_mut(drm_crtc_id).unwrap();
        output.in_use.connector.state = info.state();
        output.in_use.crtc.active_prop = Some(active_prop);
        output.in_use.crtc.mode_id_prop = Some(mode_id_prop);
        output.in_use.connector.crtc_id_prop = Some(connector_crtc);
        let plane = &mut output.in_use.planes[output.in_use.primary_plane_index];
        plane.fb_id = fb_id;
        plane.crtc_id = plane_crtc;
        Ok(())
    }

    fn modeinfo_from_mode(mode: control::Mode) -> drm_ffi::drm_sys::drm_mode_modeinfo {
        let mut raw: drm_ffi::drm_sys::drm_mode_modeinfo = unsafe { std::mem::zeroed() };
        raw.clock = mode.clock();
        let (hdisplay, vdisplay) = mode.size();
        raw.hdisplay = hdisplay;
        raw.vdisplay = vdisplay;
        let (start, end, total) = mode.hsync();
        raw.hsync_start = start;
        raw.hsync_end = end;
        raw.htotal = total;
        let (start, end, total) = mode.vsync();
        raw.vsync_start = start;
        raw.vsync_end = end;
        raw.vtotal = total;
        raw.hskew = mode.hskew();
        raw.vscan = mode.vscan();
        raw.vrefresh = mode.vrefresh();
        raw.flags = mode.flags().bits();
        raw.type_ = mode.mode_type().bits();
        let name = mode.name().to_bytes();
        let len = name.len().min(raw.name.len().saturating_sub(1));
        for (slot, byte) in raw.name[..len].iter_mut().zip(name[..len].iter()) {
            *slot = *byte as i8;
        }
        raw
    }

    fn retained_assignment_failure(
        connected: bool,
        has_mode: bool,
        mode_advertised: bool,
    ) -> Option<&'static str> {
        if !connected {
            Some("connector is not connected")
        } else if !has_mode {
            Some("connector has no retained mode")
        } else if !mode_advertised {
            Some("retained mode is no longer advertised")
        } else {
            None
        }
    }

    fn readable_properties<T: control::ResourceHandle>(
        drm: &DrmFd,
        object: T,
    ) -> Result<BTreeMap<String, (u64, property::Info)>, String> {
        let properties = drm
            .get_properties(object)
            .map_err(|error| format!("properties: {error}"))?;
        let mut named = BTreeMap::new();
        for (prop, value) in properties {
            let info = drm
                .get_property(prop)
                .map_err(|error| format!("property: {error}"))?;
            let name = info
                .name()
                .to_str()
                .map_err(|_| "property name is not utf-8".to_string())?;
            named.insert(name.to_string(), (value, info));
        }
        Ok(named)
    }

    fn restore_modeset_request(
        output: &DrmOutput,
        mode_blob_id: u64,
    ) -> control::atomic::AtomicModeReq {
        let mut changes = control::atomic::AtomicModeReq::new();
        let (plane, _) = output.in_use.primary_plane();
        let fb = output.render_buffer().drm_fb.id();
        let (width, height) = output
            .in_use
            .connector
            .preferred_mode
            .map(|mode| mode.size())
            .unwrap_or((0, 0));
        if let Some(active_prop) = output.in_use.crtc.active_prop {
            changes.add_property(
                output.in_use.crtc.handle,
                active_prop,
                control::property::Value::UnsignedRange(1),
            );
        }
        if let Some(mode_id_prop) = output.in_use.crtc.mode_id_prop {
            changes.add_property(
                output.in_use.crtc.handle,
                mode_id_prop,
                control::property::Value::Blob(mode_blob_id),
            );
        }
        if let Some(crtc_id_prop) = output.in_use.connector.crtc_id_prop {
            changes.add_property(
                output.in_use.connector.handler,
                crtc_id_prop,
                control::property::Value::CRTC(Some(output.in_use.crtc.handle)),
            );
        }
        changes.add_property(
            plane.handle,
            plane.fb_id,
            control::property::Value::Object(Some(fb.into())),
        );
        changes.add_property(
            plane.handle,
            plane.crtc_id,
            control::property::Value::CRTC(Some(output.in_use.crtc.handle)),
        );
        changes.add_property(
            plane.handle,
            plane.crtc.x.handle,
            control::property::Value::UnsignedRange(0),
        );
        changes.add_property(
            plane.handle,
            plane.crtc.y.handle,
            control::property::Value::UnsignedRange(0),
        );
        changes.add_property(
            plane.handle,
            plane.crtc.width.handle,
            control::property::Value::UnsignedRange(width as u64),
        );
        changes.add_property(
            plane.handle,
            plane.crtc.height.handle,
            control::property::Value::UnsignedRange(height as u64),
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
            control::property::Value::UnsignedRange((width as u64) << 16),
        );
        changes.add_property(
            plane.handle,
            plane.src.height.handle,
            control::property::Value::UnsignedRange((height as u64) << 16),
        );
        if let Some(property) = plane.in_fence_fd {
            changes.add_property(plane.handle, property, control::property::Value::SignedRange(-1));
        }
        if let Some(property) = plane.fb_damage_clips {
            changes.add_property(plane.handle, property, control::property::Value::Blob(0));
        }
        for extra in &output.in_use.planes {
            if extra.handle == plane.handle {
                continue;
            }
            changes.add_property(extra.handle, extra.fb_id, control::property::Value::Object(None));
            changes.add_property(extra.handle, extra.crtc_id, control::property::Value::CRTC(None));
        }
        changes
    }

    pub fn supports_syncobj_timeline(&self) -> bool {
        self.supports_syncobj_timeline
    }

    pub fn supports_linux_drm_syncobj(&self) -> bool {
        self.supports_syncobj_timeline && self.supports_syncobj_eventfd
    }

    pub fn supported_dmabuf_read_formats(&self) -> Vec<(&'static Format, BTreeSet<Modifier>)> {
        self.renderer
            .capabilities()
            .sample_formats
            .iter()
            .filter_map(|format| {
                let mapped = *FORMATS_MAP.get(&format.fourcc)?;
                let modifiers = format.modifiers.iter().copied().map(Modifier).collect();
                Some((mapped, modifiers))
            })
            .collect()
    }

    fn render_output(
        &mut self,
        drm_crtc_id: u32,
        recipe: &OutputFrameRecipe,
        damage: &DamageRegion,
        preserve_contents: bool,
    ) -> crate::render::RenderCompletion {
        let Drm { renderer, outputs, .. } = self;
        let output = outputs
            .get_mut(&drm_crtc_id)
            .expect("missing DRM output pipeline for render");
        let presentation_transform = output.presentation_transform;
        let sync_mode = output.render_sync_mode;
        let target = output.pipeline.render_buffer_mut().target_mut();
        match renderer.render(
            target,
            RenderFrame {
                recipe,
                damage,
                preserve_contents,
                presentation_transform,
                sync_mode,
                blur_reference: None,
            },
        ) {
            Ok(completion) => completion,
            Err(error) => {
                log::error!("output render failed: {error}");
                crate::render::RenderCompletion::blocking_fallback()
            }
        }
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
        render_completion: &crate::render::gles::RenderCompletion,
        damage: &DamageRegion,
        output_id: Option<OutputId>,
        scene: Vec<SceneItemState>,
        frame_callback_surfaces: Vec<SurfaceKey>,
        presentation_feedbacks: Vec<SubmissionPresentationFeedback>,
        cursor_plane: Option<CursorPlaneCandidate>,
        had_effects: bool,
        scene_build_duration: Duration,
        render_started_at: Instant,
        protected: bool,
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
                        protected,
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
            false,
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
        let bo = self.gbm.create_bo_with_size_no_export(
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

    fn list_resources(drm: &DrmFd) -> Result<DrmResources, String> {
        let resources = drm
            .resource_handles()
            .map_err(|error| format!("resource handles: {error}"))?;

        let (cursor_height, cursor_width) = {
            use drm::DriverCapability::{CursorHeight, CursorWidth};

            let height = drm.get_driver_capability(CursorHeight).unwrap_or(64);
            let width = drm.get_driver_capability(CursorWidth).unwrap_or(64);
            (height, width)
        };

        let plane_handles = drm
            .plane_handles()
            .map_err(|error| format!("plane handles: {error}"))?
            .to_vec();
        log::info!("DRM plane handles reported by kernel: count={}", plane_handles.len());

        let make_plane = |plane: plane::Handle| Self::make_plane(drm, plane);
        let planes: BTreeMap<plane::Handle, Plane> = plane_handles
            .iter()
            .copied()
            .filter_map(make_plane)
            .collect();
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
            .filter_map(make_encoder)
            .collect();

        let make_connector = |connector| Self::make_connector(drm, connector, &encoders);
        let connectors: BTreeMap<connector::Handle, Connector> = resources
            .connectors()
            .iter()
            .copied()
            .filter_map(make_connector)
            .collect();

        Ok(DrmResources {
            planes,
            crtcs,
            encoders,
            connectors,
            cursor_height,
            cursor_width,
            resources,
        })
    }

    fn init_drm_outputs(
        drm: &DrmFd,
        renderer: &mut dyn Renderer,
        gbm: &GbmDevice,
        drm_resources: &DrmResources,
    ) -> BTreeMap<u32, DrmOutput> {
        let DrmResources { planes, crtcs, connectors, .. } = drm_resources;
        let mut outputs = BTreeMap::new();
        for in_use in Self::find_valid_connectors(connectors, crtcs, planes) {
            let drm_crtc_id = in_use.crtc_id();
            let name = in_use.connector.interface_name.clone();
            match Self::init_drm_output(drm, renderer, gbm, in_use) {
                Ok(output) => {
                    outputs.insert(drm_crtc_id, output);
                }
                Err(error) => log::error!("skipping connector {name}: {error}"),
            }
        }
        outputs
    }

    fn init_drm_output(
        drm: &DrmFd,
        renderer: &mut dyn Renderer,
        gbm: &GbmDevice,
        in_use: ConnectorInUse,
    ) -> Result<DrmOutput, String> {
        let Some(mode) = in_use.connector.preferred_mode.as_ref() else {
            return Err("connector has no mode".into());
        };
        let (hdisplay, vdisplay) = mode.size();
        let (primary_plane, plane_modifiers) = in_use.primary_plane();
        let explicit_sync = renderer.capabilities().explicit_sync;
        let render_sync_mode = if explicit_sync && primary_plane.supports_explicit_sync() {
            log::info!(
                "Using explicit compositor sync: connector={} plane={:?} in_fence_fd={:?}",
                in_use.connector.interface_name,
                primary_plane.handle,
                primary_plane.in_fence_fd
            );
            RenderSyncMode::ExplicitInFence
        } else {
            log::warn!(
                "Falling back to blocking GL sync: connector={} plane={:?} renderer_explicit_sync={} plane_in_fence_fd={}",
                in_use.connector.interface_name,
                primary_plane.handle,
                explicit_sync,
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

        let buffers: [_; 2] = [
            Self::make_render_buffer(drm, renderer, gbm, &in_use, false)
                .map_err(|error| format!("render buffer #0: {error}"))?,
            Self::make_render_buffer(drm, renderer, gbm, &in_use, false)
                .map_err(|error| format!("render buffer #1: {error}"))?,
        ];

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
            return Err(format!(
                "initial modeset connector={} crtc={:?} mode={}x{}: {error}",
                in_use.connector.interface_name, in_use.crtc.handle, hdisplay, vdisplay
            ));
        }

        Ok(DrmOutput {
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
        })
    }

    fn make_render_buffer(
        drm: &DrmFd,
        renderer: &mut dyn Renderer,
        gbm: &GbmDevice,
        in_use: &ConnectorInUse,
        is_cursor: bool,
    ) -> std::io::Result<RenderBuffer> {
        let (_, plane_modifiers) = in_use.primary_plane();
        let Some(mode) = in_use.connector.preferred_mode.as_ref() else {
            return Err(std::io::Error::other("connector has no mode"));
        };

        let modifiers: Vec<_> = {
            let Some(format) = renderer
                .capabilities()
                .render_formats
                .iter()
                .find(|format| format.fourcc == DrmFourcc::Xrgb8888)
            else {
                return Err(std::io::Error::other("renderer has no XRGB8888 render format"));
            };
            format
                .modifiers
                .iter()
                .copied()
                .map(Modifier)
                .filter(|modifier| plane_modifiers.contains(modifier))
                .collect()
        };
        if modifiers.is_empty() {
            return Err(std::io::Error::other("no modifiers shared by the renderer and the plane"));
        }

        let mut flags = UseFlags::RENDERING | UseFlags::SCANOUT;
        if is_cursor {
            flags |= UseFlags::LINEAR;
        }

        let bo = gbm.create_bo(&mode, flags, &modifiers);
        let Some(dmabuf) = bo.dmabuf() else {
            return Err(std::io::Error::other("GBM buffer did not export a dmabuf"));
        };
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
        let width = dmabuf.width;
        let height = dmabuf.height;
        let target = renderer.import_output_target(dmabuf).map_err(|error| {
            std::io::Error::other(format!("renderer output import failed: {error}"))
        })?;

        Ok(RenderBuffer {
            width,
            height,
            target: ManuallyDrop::new(target),
            drm_fb: ManuallyDrop::new(drm_fb),
            bo: ManuallyDrop::new(bo),
        })
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
            active_prop: props.get("ACTIVE").map(|(_, info)| info.handle()),
            mode_id: props.get("MODE_ID").map(|(v, _)| *v as u32).unwrap(),
            mode_id_prop: props.get("MODE_ID").map(|(_, info)| info.handle()),
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
    ) -> Option<(encoder::Handle, Encoder)> {
        let info = drm.get_encoder(encoder).ok()?;
        let possible_crtcs: HashSet<_> = crtcs
            .iter()
            .filter(|(_, crtc)| info.possible_crtcs().0 & (1 << crtc.index) != 0)
            .map(|(handle, _)| *handle)
            .collect();

        let handle = encoder;
        let encoder = Encoder { handle, possible_crtcs };
        Some((handle, encoder))
    }

    fn make_plane(drm: &DrmFd, plane: plane::Handle) -> Option<(plane::Handle, Plane)> {
        let get_kind = |props: &BTreeMap<String, (u64, property::Info)>| {
            let Some((value, prop)) = props.get("type") else {
                return None;
            };

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

        let info = match drm.get_plane(plane) {
            Ok(info) => info,
            Err(error) => {
                log::warn!("skipping DRM plane {plane:?}: {error}");
                return None;
            }
        };
        let props = get_props(drm, plane);

        let formats = match props.get("IN_FORMATS") {
            Some((prop, _)) => match get_in_formats(drm, *prop) {
                Ok(formats) => formats
                    .into_iter()
                    .filter(|format| !format.modifiers.is_empty())
                    .filter_map(|format| FORMATS_MAP.get(&format.format).map(|our| (format, our)))
                    .map(|(in_format, format)| {
                        let plane_format = PlaneFormat {
                            format: (*format).clone(),
                            modifiers: in_format.modifiers,
                        };
                        (in_format.format, plane_format)
                    })
                    .collect(),
                Err(error) => {
                    log::warn!("skipping DRM plane {plane:?}: {error}");
                    return None;
                }
            },
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

        let get_geometry = |x: &str, y: &str, w: &str, h: &str| -> Option<Geometry> {
            let get_prop = |(value, info): &(u64, property::Info)| Property {
                value: *value,
                handle: info.handle(),
            };
            Some(Geometry {
                x: props.get(x).map(get_prop)?,
                y: props.get(y).map(get_prop)?,
                width: props.get(w).map(get_prop)?,
                height: props.get(h).map(get_prop)?,
            })
        };
        let get_handle = |s: &str| props.get(s).map(|(_, info)| info.handle());

        let kind = get_kind(&props)?;
        let crtc_geometry = get_geometry("CRTC_X", "CRTC_Y", "CRTC_W", "CRTC_H")?;
        let src_geometry = get_geometry("SRC_X", "SRC_Y", "SRC_W", "SRC_H")?;
        let possible_crtcs = info.possible_crtcs().0;
        let fb_id = get_handle("FB_ID")?;
        let crtc_id = get_handle("CRTC_ID")?;
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

        Some((handle, plane))
    }

    // fn make_plane(drm: &DrmFd, plane: plane::Handle) -> (plane::Handle, Plane) {
    fn make_connector(
        drm: &DrmFd,
        connector_handle: connector::Handle,
        encoders: &BTreeMap<encoder::Handle, Encoder>,
    ) -> Option<(connector::Handle, Connector)> {
        let connector = match drm.get_connector(connector_handle, true) {
            Ok(connector) => connector,
            Err(error) => {
                log::warn!("skipping DRM connector {connector_handle:?}: {error}");
                return None;
            }
        };

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
            crtc_id_prop: props.get("CRTC_ID").map(|(_, info)| info.handle()),
        };

        Some((connector_handle, connector))
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

/// One scanout buffer. Drop deletes the GPU target, then the DRM framebuffer, then the GBM BO.
#[derive(Debug)]
struct RenderBuffer {
    width: i32,
    height: i32,
    target: ManuallyDrop<Box<dyn RenderTarget>>,
    drm_fb: ManuallyDrop<DrmFrameBuffer>,
    bo: ManuallyDrop<BufferObject>,
}

impl RenderBuffer {
    fn target(&self) -> &dyn RenderTarget {
        &**self.target
    }

    fn target_mut(&mut self) -> &mut dyn RenderTarget {
        &mut **self.target
    }
}

impl Drop for RenderBuffer {
    fn drop(&mut self) {
        unsafe {
            ManuallyDrop::drop(&mut self.target);
            ManuallyDrop::drop(&mut self.drm_fb);
            ManuallyDrop::drop(&mut self.bo);
        }
    }
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

    fn clear(&mut self) {
        self.pos = 0;
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
    /// Completions armed under another generation belong to the pre-resume fd session.
    generation: u64,
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
    PollReady {
        generation: u64,
    },
    ReadChunk {
        buffer: Box<[u8]>,
        generation: u64,
    },
    ReadDrained {
        generation: u64,
    },
    ReadError {
        error: std::io::Error,
        generation: u64,
    },
    /// Drop in-flight reads and ignore events armed before this generation.
    SetGeneration {
        generation: u64,
    },
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
    ) -> std::result::Result<(), crate::runner::ModelFailure> {
        use DrmReaderAction::*;

        match action {
            Start => {
                self.dispatch_poll(dispatcher);
            }
            SetGeneration { generation } => {
                self.generation = generation;
                if let Some(poll_op_id) = self.poll_op_id.take() {
                    dispatcher.dispatch(IoUringAction::PollRemove { target: poll_op_id });
                }
                if let Some(read_op_id) = self.read_op_id.take() {
                    dispatcher.dispatch(IoUringAction::Cancel { target: read_op_id });
                }
                self.read_buffer.clear();
                self.dispatch_poll(dispatcher);
            }
            PollReady { generation } => {
                if generation != self.generation || self.read_op_id.is_some() {
                    return Ok(());
                }
                self.dispatch_read_some(dispatcher);
            }
            ReadChunk { buffer, generation } => {
                self.read_op_id = None;
                if generation != self.generation {
                    return Ok(());
                }
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
                                    DrmAction::FlipComplete {
                                        sequence,
                                        tv_sec,
                                        tv_usec,
                                        crtc_id,
                                        generation,
                                    },
                                );
                            }
                        }
                        ParsedDrmEvent::Ignored { .. } => {}
                    }
                    self.read_buffer.consume_event(len);
                }
            }
            ReadDrained { generation } => {
                if generation == self.generation {
                    self.read_op_id = None;
                }
            }
            ReadError { error, generation } => {
                if generation == self.generation {
                    self.read_op_id = None;
                    log::error!("Could not read DRM events: {:?}", error);
                }
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
            generation: 0,
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
            dispatch_to: Redispatcher::to_instance(self.model_id(), {
                let generation = self.generation;
                move |result| match result {
                    Ok(PollCompleted {}) => DrmReaderAction::PollReady { generation },
                    Err(error) => DrmReaderAction::ReadError { error, generation },
                }
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
            dispatch_to: Redispatcher::to_instance(self.model_id(), {
                let generation = self.generation;
                move |result: std::io::Result<ReadWriteCompleted>| match result {
                    Ok(ReadWriteCompleted { buffer, .. }) => {
                        DrmReaderAction::ReadChunk { buffer, generation }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        DrmReaderAction::ReadDrained { generation }
                    }
                    Err(error) => DrmReaderAction::ReadError { error, generation },
                }
            }),
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
    FlipComplete { sequence: u32, tv_sec: u32, tv_usec: u32, crtc_id: u32, generation: u64 },
    FlipWatchdog,
    SessionChanged { active: bool },
    Stop,
}

pub enum VisualAnimationPumpAction {
    Start,
    Kick,
    KickOutput { output_id: OutputId },
    Stop,
}

pub struct VisualAnimationPump;

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
        log::warn!("request_output: invalid logical output {:?}", output_id);
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

pub fn request_active_output(state: &State, dispatcher: &mut Dispatcher) {
    let Some(output_id) = state.active_output_id() else {
        log::warn!("request_active_output: no active logical output");
        return;
    };

    request_output(state, output_id, dispatcher);
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

pub fn kick_visual_animation_pump(dispatcher: &mut Dispatcher) {
    dispatcher.dispatch(VisualAnimationPumpAction::Kick);
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

fn remember_animation_outputs(state: &State, rect: RectI, output_ids: &mut BTreeSet<OutputId>) {
    let rect_outputs = state.outputs_intersecting_rect(rect);
    if rect_outputs.is_empty() {
        if let Some(output_id) = state.primary_output_for_rect(rect, None) {
            output_ids.insert(output_id);
        }
        return;
    }
    output_ids.extend(rect_outputs);
}

fn active_animation_output_ids(state: &State) -> BTreeSet<OutputId> {
    let mut output_ids = BTreeSet::new();
    for (&surface, managed) in &state.managed_root_surfaces {
        if !managed.focus_animation.is_active()
            && !managed.lifecycle_animation.is_active()
            && !managed.geometry_animation.is_active()
        {
            continue;
        }
        let Some(rect) = state.root_surface_motion_bounds(surface) else {
            continue;
        };
        remember_animation_outputs(state, rect, &mut output_ids);
    }
    for snapshot in &state.closing_snapshots {
        if snapshot.is_active() {
            remember_animation_outputs(state, snapshot.motion_bounds(), &mut output_ids);
        }
    }
    for (_, group) in state.workspace_groups.iter() {
        if group
            .transition
            .is_some_and(|transition| transition.needs_frames())
        {
            output_ids.extend(group.outputs.iter().copied());
        }
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

impl Model for VisualAnimationPump {
    type Action = VisualAnimationPumpAction;
    const START: Option<Self::Action> = Some(VisualAnimationPumpAction::Start);
    const STOP: Option<Self::Action> = Some(VisualAnimationPumpAction::Stop);

    fn process(
        &mut self,
        state: &mut crate::state::State,
        action: Self::Action,
        dispatcher: &mut crate::runner::Dispatcher,
    ) -> std::result::Result<(), crate::runner::ModelFailure> {
        match action {
            VisualAnimationPumpAction::Start => {}
            VisualAnimationPumpAction::Kick => {
                if dispatcher.shutdown_requested() {
                    return Ok(());
                }
                self.advance_and_schedule(state, dispatcher, Instant::now());
            }
            VisualAnimationPumpAction::KickOutput { output_id } => {
                if dispatcher.shutdown_requested() {
                    return Ok(());
                }
                self.advance_and_schedule_output(state, dispatcher, Instant::now(), output_id);
            }
            VisualAnimationPumpAction::Stop => {}
        }
        Ok(())
    }
}

impl VisualAnimationPump {
    pub fn new() -> Self {
        Self
    }

    // Sole mutable animation phase. Output completion may call `advance_and_schedule_output`
    // for the same focus track, but neither path arms its own timer: both request
    // `RequestFrameReason::Animation` through the output scheduler. Sampling is absolute-time,
    // so a repeated kick at the same instant does not advance the fade twice. Visual resolution
    // reads the cached sample and must not advance it again.
    pub(crate) fn advance_and_schedule(
        &mut self,
        state: &mut State,
        dispatcher: &mut Dispatcher,
        now: Instant,
    ) {
        let result = state.advance_visual_animations(now);
        let has_no_damage = result.damaged_rects.is_empty();
        publish_animation_damage(state, dispatcher, &result.damaged_rects, None);
        if result.has_active_animations && has_no_damage {
            for output_id in active_animation_output_ids(state) {
                schedule_animation_wakeup(state, dispatcher, output_id);
            }
        }
        publish_completed_workspace_transitions(
            state,
            dispatcher,
            &result.completed_transition_outputs,
        );
    }

    fn advance_and_schedule_output(
        &mut self,
        state: &mut State,
        dispatcher: &mut Dispatcher,
        now: Instant,
        output_id: OutputId,
    ) {
        let result = state.advance_visual_animations_for_output(output_id, now);
        let completed_outputs = result.completed_transition_outputs.clone();
        let has_no_damage = result.damaged_rects.is_empty();
        publish_animation_damage(state, dispatcher, &result.damaged_rects, Some(output_id));
        if has_no_damage && result.has_active_animations {
            schedule_animation_wakeup(state, dispatcher, output_id);
        }
        publish_completed_workspace_transitions(state, dispatcher, &completed_outputs);
    }
}

fn publish_animation_damage(
    state: &mut State,
    dispatcher: &mut Dispatcher,
    rects: &[RectI],
    always_include: Option<OutputId>,
) {
    queue_animation_damage(state, rects);
    if rects.is_empty() {
        return;
    }
    if let Some(output_id) = always_include {
        let mut outputs = BTreeSet::new();
        outputs.insert(output_id);
        for rect in rects {
            outputs.extend(state.outputs_intersecting_rect(*rect));
        }
        for output_id in outputs {
            request_output_frame(state, output_id, RequestFrameReason::Animation, dispatcher);
        }
        return;
    }
    for rect in rects {
        let output_ids = state.outputs_intersecting_rect(*rect);
        if output_ids.is_empty() {
            if let Some(output_id) = state.primary_output_for_rect(*rect, None) {
                request_output_frame(state, output_id, RequestFrameReason::Animation, dispatcher);
            }
            continue;
        }
        for output_id in output_ids {
            request_output_frame(state, output_id, RequestFrameReason::Animation, dispatcher);
        }
    }
}

fn schedule_animation_wakeup(state: &State, dispatcher: &mut Dispatcher, output_id: OutputId) {
    let delay_msec = animation_delay_msec_for_output(state, output_id);
    dispatcher.dispatch_to(
        output_scheduler_id(output_id),
        OutputSchedulerAction::ScheduleAnimation { delay_msec },
    );
}

fn animation_damage_targets(state: &State, rects: &[RectI]) -> Vec<(OutputId, RectI)> {
    let mut targets = Vec::new();
    for rect in rects {
        for output_id in state.outputs_intersecting_rect(*rect) {
            let Some(bounds) = state.output_bounds(output_id) else {
                continue;
            };
            let Some(local) = rect.intersection(bounds) else {
                continue;
            };
            targets.push((output_id, local.translate(-bounds.x, -bounds.y)));
        }
    }
    targets
}

fn queue_animation_damage(state: &mut State, rects: &[RectI]) {
    let targets = animation_damage_targets(state, rects);
    for (output_id, local) in targets {
        let Some(output) = state.outputs.try_get(output_id) else {
            continue;
        };
        let drm_id = output.backend.drm_id;
        let crtc_id = output.drm_crtc_id;
        let Some(drm) = state.drm.try_get_mut(drm_id) else {
            continue;
        };
        if let Some(drm_output) = drm.output_mut(crtc_id) {
            drm_output.queue_pending_damage(local);
        }
    }
}

fn publish_completed_workspace_transitions(
    state: &mut State,
    dispatcher: &mut Dispatcher,
    outputs: &[OutputId],
) {
    if outputs.is_empty() {
        return;
    }
    crate::wayland_objects::ext_workspace_manager_v1::publish_workspace_state_changed(
        state,
        dispatcher,
        outputs.iter().copied(),
    );
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
    ) -> std::result::Result<(), crate::runner::ModelFailure> {
        match action {
            DrmAction::Start => {}
            DrmAction::RenderFrame { decision } => {
                self.render_frame(state, dispatcher, decision);
            }
            DrmAction::FlipComplete { sequence, tv_sec, tv_usec, crtc_id, generation } => {
                let current = state
                    .drm
                    .try_get(self.drm_id)
                    .map(|drm| drm.event_generation);
                if current != Some(generation) {
                    return Ok(());
                }
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

fn read_output_capture_copies(
    drm: &mut Drm,
    drm_crtc_id: u32,
    specs: Vec<crate::capture::OutputCopySpec>,
    cursor: Option<&crate::capture::CaptureCursorImage>,
) -> Vec<crate::capture::RenderedOutputCopy> {
    let Drm { renderer, outputs, .. } = drm;
    let Some(output) = outputs.get(&drm_crtc_id) else {
        return Vec::new();
    };
    let target = output.pipeline.render_buffer().target();
    specs
        .into_iter()
        .map(|spec| {
            let len = spec.stride.saturating_mul(spec.rect.height.max(0) as usize);
            let mut bytes = vec![0u8; len];
            let read_ok =
                match RectI::new(spec.rect.x, spec.rect.y, spec.rect.width, spec.rect.height) {
                    Some(rect) => renderer
                        .readback(target, rect, spec.stride, &mut bytes)
                        .is_ok(),
                    None => false,
                };
            if read_ok && spec.cursor == crate::capture::CaptureCursor::Painted {
                if let Some(cursor) = cursor {
                    crate::capture::blend_cursor_into_copy(
                        &mut bytes,
                        spec.stride,
                        spec.rect,
                        cursor,
                    );
                }
            }
            crate::capture::RenderedOutputCopy {
                job_id: spec.job_id,
                bytes,
                stride: spec.stride,
                read_ok,
            }
        })
        .collect()
}

struct PreparedToplevelCapture {
    job_id: crate::capture::CaptureJobId,
    key: crate::capture::ToplevelKey,
    width: i32,
    height: i32,
    bounds: Option<RectI>,
    snapshots: Vec<crate::wayland_objects::wl_surface::SurfaceRenderState>,
    visual: Option<RootSurfaceVisualState>,
}

struct RenderedToplevelCapture {
    key: crate::capture::ToplevelKey,
    bounds_width: i32,
    bounds_height: i32,
    damage: Vec<crate::capture::CapturePixelRect>,
    copy: crate::capture::RenderedOutputCopy,
}

fn sample_pending_toplevel_captures(
    state: &mut State,
    drm_id: DrmId,
    drm_crtc_id: u32,
    output_id: OutputId,
) {
    if !crate::capture::toplevel_sample_pending(&state.capture, output_id) {
        return;
    }
    if state.session_lock.input_gated() {
        for job_id in crate::capture::toplevel_jobs_for_output(&state.capture, output_id) {
            crate::capture::fail_capture(state, job_id);
        }
        return;
    }
    let jobs = crate::capture::toplevel_jobs_for_output(&state.capture, output_id);
    let mut prepared = Vec::new();
    for job_id in jobs {
        let Some((key, width, height)) = crate::capture::queued_toplevel_job(state, job_id) else {
            continue;
        };
        let bounds = crate::capture::toplevel_rendered_bounds(state, key);
        let snapshots = crate::capture::peek_toplevel_snapshots(state, key);
        let visual = crate::capture::toplevel_root_surface(state, key)
            .and_then(|root| state.root_surface_visual_state(root));
        prepared.push(PreparedToplevelCapture {
            job_id,
            key,
            width,
            height,
            bounds,
            snapshots,
            visual,
        });
    }
    let rendered = {
        let (objects, drm_map) = (&mut state.objects, &mut state.drm);
        let drm = drm_map.get_mut(drm_id);
        render_prepared_toplevels(objects, drm, drm_crtc_id, &prepared)
    };
    let timestamp = crate::capture::capture_timestamp_now();
    for mut item in rendered {
        if crate::capture::capture_job_cursor(state, item.copy.job_id)
            == Some(crate::capture::CaptureCursor::Painted)
        {
            crate::capture::paint_toplevel_cursor(
                state,
                item.key,
                &mut item.copy.bytes,
                item.copy.stride,
            );
        }
        crate::capture::apply_rendered_toplevel(
            &mut state.capture,
            &state.objects,
            item.key,
            item.bounds_width,
            item.bounds_height,
            &item.damage,
            item.copy,
            timestamp,
        );
    }
}

fn render_prepared_toplevels(
    objects: &mut Box<crate::state::IndexMap<ObjectId, Box<dyn Object>>>,
    drm: &mut Drm,
    drm_crtc_id: u32,
    prepared: &[PreparedToplevelCapture],
) -> Vec<RenderedToplevelCapture> {
    drm.bind_context();
    let drm_fd = drm.fd();
    prepared
        .iter()
        .map(|item| {
            let failed = |bounds_width, bounds_height| RenderedToplevelCapture {
                key: item.key,
                bounds_width,
                bounds_height,
                damage: Vec::new(),
                copy: crate::capture::RenderedOutputCopy {
                    job_id: item.job_id,
                    bytes: Vec::new(),
                    stride: 0,
                    read_ok: false,
                },
            };
            let Some(bounds) = item.bounds else {
                return failed(0, 0);
            };
            if bounds.width != item.width
                || bounds.height != item.height
                || bounds.width > 16_384
                || bounds.height > 16_384
            {
                return failed(bounds.width, bounds.height);
            }
            let mut items = Vec::new();
            let mut elements = Vec::new();
            let alpha = item
                .visual
                .map(|visual| visual.effects.opacity_alpha)
                .unwrap_or(u8::MAX);
            let effect_start = elements.len();
            push_root_surface_effect_elements(&mut elements, item.visual, FramePhase::UnderWindow);
            push_root_surface_effect_elements(&mut elements, item.visual, FramePhase::OverWindow);
            push_root_surface_effect_elements(&mut elements, item.visual, FramePhase::Overlay);
            let mut window_effects = elements.split_off(effect_start);
            window_effects.retain(|element| {
                !matches!(
                    element,
                    RecordedFrameElement::Effect {
                        element: EffectElement::WindowBlurBackdrop { .. },
                        ..
                    }
                )
            });
            elements.append(&mut window_effects);
            let item_start = items.len();
            push_scene_items_from_snapshots(
                &mut items,
                objects,
                drm,
                &drm_fd,
                drm_crtc_id,
                bounds,
                item.snapshots.clone(),
                false,
                item.visual.and_then(|visual| visual.clip),
                alpha,
                None,
            );
            push_scene_item_frame_elements(&mut elements, item_start..items.len());
            translate_scene_items(&mut items, -bounds.x, -bounds.y);
            translate_recorded_frame_elements(&mut elements, -bounds.x, -bounds.y);
            let recipe = build_output_frame_recipe(&items, &elements);
            wait_scene_acquires(drm, &items);
            let (reference_width, reference_height) = drm
                .output(drm_crtc_id)
                .map(|output| {
                    let buffer = output.render_buffer();
                    (buffer.width, buffer.height)
                })
                .unwrap_or((bounds.width, bounds.height));
            let mut target = match drm
                .renderer
                .create_offscreen_target(bounds.width, bounds.height)
            {
                Ok(target) => target,
                Err(_) => return failed(bounds.width, bounds.height),
            };
            let Some(capture_rect) = RectI::new(0, 0, bounds.width, bounds.height) else {
                return failed(bounds.width, bounds.height);
            };
            let damage = DamageRegion::from_rect(capture_rect);
            if drm
                .renderer
                .render(
                    &mut *target,
                    RenderFrame {
                        recipe: &recipe,
                        damage: &damage,
                        preserve_contents: false,
                        presentation_transform: OutputPresentationTransform::FlipY,
                        sync_mode: RenderSyncMode::BlockingFinishFallback,
                        blur_reference: Some((reference_width, reference_height)),
                    },
                )
                .is_err()
            {
                return failed(bounds.width, bounds.height);
            }
            let stride = (bounds.width as usize).saturating_mul(4);
            let mut bytes = vec![0u8; stride.saturating_mul(bounds.height.max(0) as usize)];
            let read_ok = drm
                .renderer
                .readback(&*target, capture_rect, stride, &mut bytes)
                .is_ok();
            let damage = item
                .snapshots
                .iter()
                .flat_map(|snapshot| snapshot.damage.rects().iter().copied())
                .map(|rect| crate::capture::CapturePixelRect {
                    x: rect.x.saturating_sub(bounds.x),
                    y: rect.y.saturating_sub(bounds.y),
                    width: rect.width,
                    height: rect.height,
                })
                .collect();
            RenderedToplevelCapture {
                key: item.key,
                bounds_width: bounds.width,
                bounds_height: bounds.height,
                damage,
                copy: crate::capture::RenderedOutputCopy {
                    job_id: item.job_id,
                    bytes,
                    stride,
                    read_ok,
                },
            }
        })
        .collect()
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
        if !state.session_active {
            dispatcher.dispatch_to(
                output_scheduler_id(self.output_id),
                OutputSchedulerAction::RenderAttempted { started: false },
            );
            return;
        }
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
            if crate::capture::output_capture_pending(state, self.output_id) {
                let drm = state.drm.get_mut(self.drm_id);
                if let Some(output) = drm.output_mut(drm_crtc_id) {
                    // A capture reads this frame's back buffer, so buffer age must not leave
                    // stale pixels outside the damaged area.
                    output.pipeline.set_force_full_redraw(true);
                }
            }
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
                mut direct_scanout,
                mut cursor_plane,
                mut preserve_contents,
                protected,
            } = frame;
            if crate::capture::output_capture_pending(state, self.output_id) {
                direct_scanout = None;
            }
            // The cursor is blended into a painted SHM copy only. A protected frame has no
            // desktop cursor in its composition, so it does not receive one here either.
            let capture_cursor = if protected {
                None
            } else if crate::capture::output_capture_pending(state, self.output_id) {
                capture_cursor_overlay(state, self.output_id)
            } else {
                None
            };
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
                        sample_pending_toplevel_captures(
                            state,
                            self.drm_id,
                            drm_crtc_id,
                            self.output_id,
                        );
                        return;
                    }
                    Err(_) => {}
                }
                if !software_cursor_items.is_empty() {
                    items.append(&mut software_cursor_items);
                    // Clone the effect recipe on cursor fallback to keep the retry path simple;
                    // if fallback becomes hot we should switch this to recipe reuse.
                    let fallback_plan = render_planner::build_frame_plan(
                        drm.output_mut(drm_crtc_id).unwrap(),
                        items,
                        Vec::new(),
                        frame_elements.clone(),
                        None,
                        protected,
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
                wait_scene_acquires(drm, &items);
                let recipe = build_output_frame_recipe(&items, &frame_elements);
                let render_started_at = Instant::now();
                let render_completion =
                    drm.render_output(drm_crtc_id, &recipe, &damage, preserve_contents);
                if crate::capture::output_sample_pending(&state.capture, self.output_id) {
                    let (mode_width, mode_height, scale, transform) = state
                        .outputs
                        .try_get(self.output_id)
                        .map(|output| {
                            (
                                output.current_mode.width,
                                output.current_mode.height,
                                output.scale,
                                output.transform,
                            )
                        })
                        .unwrap_or((0, 0, crate::state::FractionalScale::ONE, 0));
                    let frame_damage: Vec<_> = damage
                        .rects()
                        .iter()
                        .map(|rect| crate::capture::CapturePixelRect {
                            x: rect.x,
                            y: rect.y,
                            width: rect.width,
                            height: rect.height,
                        })
                        .collect();
                    let specs = crate::capture::output_copy_specs(
                        &state.capture,
                        &state.objects,
                        self.output_id,
                    );
                    let copies = read_output_capture_copies(
                        drm,
                        drm_crtc_id,
                        specs,
                        capture_cursor.as_ref(),
                    );
                    crate::capture::apply_rendered_output(
                        &mut state.capture,
                        &state.objects,
                        self.output_id,
                        mode_width,
                        mode_height,
                        scale,
                        transform,
                        &frame_damage,
                        copies,
                        crate::capture::capture_timestamp_now(),
                    );
                }
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
                    protected,
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
                                drm.output_mut(drm_crtc_id).unwrap(),
                                items,
                                Vec::new(),
                                frame_elements.clone(),
                                None,
                                protected,
                            );
                            wait_scene_acquires(drm, &fallback_plan.items);
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
                                protected,
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
                                                drm.renderer.wait_fence(in_fence_fd)
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
                                    if let Err(wait_error) = drm.renderer.wait_fence(in_fence_fd) {
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
                                if let Err(wait_error) = drm.renderer.wait_fence(in_fence_fd) {
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
        sample_pending_toplevel_captures(state, self.drm_id, drm_crtc_id, self.output_id);
        crate::capture::publish_screencopy(state, dispatcher);
        crate::capture::publish_ext_frames(state, dispatcher);
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
        let mut protected_presented = false;
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
                        protected_presented = completed_submission.submission.protected;
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
        if protected_presented {
            crate::wayland_objects::ext_session_lock_v1::on_protected_frame_presented(
                state,
                self.output_id,
                dispatcher,
            );
        }
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
            quiesce_output(state, dispatcher, self.drm_id, self.output_id);
        }
    }
}

/// Modeset retained outputs on the current DRM fd and ignore flips from the previous generation.
#[derive(Debug)]
pub(crate) enum OutputRestoreError {
    Refresh(String),
    Modeset(String),
}

impl OutputRestoreError {
    fn fault(&self) -> SessionFault {
        match self {
            Self::Refresh(_) => SessionFault::KmsRefresh,
            Self::Modeset(_) => SessionFault::Modeset,
        }
    }
}

impl std::fmt::Display for OutputRestoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Refresh(detail) | Self::Modeset(detail) => f.write_str(detail),
        }
    }
}

pub(crate) fn resume_drm_backend(
    state: &mut State,
    dispatcher: &mut Dispatcher,
    drm_id: DrmId,
) -> Vec<u32> {
    let generation = {
        let drm = state.drm.get_mut(drm_id);
        drm.event_generation = drm.event_generation.wrapping_add(1);
        drm.event_generation
    };
    dispatcher.dispatch_to(drm_id, DrmReaderAction::SetGeneration { generation });
    let results = state.drm.get_mut(drm_id).restore_outputs_in_place();
    let mut failed = Vec::new();
    for (crtc_id, result) in results {
        match result {
            Ok((releases, feedbacks, damage_blob)) => {
                let Some(output_id) = state.output_id_for_drm_crtc(drm_id, crtc_id) else {
                    continue;
                };
                settle_pipeline_reset(
                    state,
                    dispatcher,
                    drm_id,
                    output_id,
                    releases,
                    feedbacks,
                    damage_blob,
                );
            }
            Err(error) => {
                isolate_session_fault(error.fault(), format!("CRTC {crtc_id}: {error}"));
                failed.push(crtc_id);
            }
        }
    }
    failed
}

pub(crate) fn quiesce_backend(state: &mut State, dispatcher: &mut Dispatcher, drm_id: DrmId) {
    let output_ids = state.output_ids_for_backend(BackendOutputRef::new(drm_id));
    for output_id in output_ids {
        quiesce_output(state, dispatcher, drm_id, output_id);
    }
}

fn quiesce_output(
    state: &mut State,
    dispatcher: &mut Dispatcher,
    drm_id: DrmId,
    output_id: OutputId,
) {
    let Some(drm_crtc_id) = state.output_crtc_id(output_id) else {
        return;
    };
    let (releases, feedbacks, damage_blob) = {
        let Some(output) = state.drm.get_mut(drm_id).output_mut(drm_crtc_id) else {
            return;
        };
        output.pipeline.quiesce()
    };
    settle_pipeline_reset(state, dispatcher, drm_id, output_id, releases, feedbacks, damage_blob);
}

fn settle_pipeline_reset(
    state: &mut State,
    dispatcher: &mut Dispatcher,
    drm_id: DrmId,
    output_id: OutputId,
    releases: Vec<DeferredRelease>,
    feedbacks: Vec<SubmissionPresentationFeedback>,
    damage_blob: Option<u64>,
) {
    for release in releases {
        release.resolve(state, dispatcher);
    }
    if feedbacks.is_empty() && damage_blob.is_none() {
        return;
    }
    log::info!("clearing in-flight pipeline state for output {output_id:?}");
    discard_frame_for_timeline(
        dispatcher,
        &[crate::scheduler::presentation::DiscardedFeedback { feedbacks }],
        damage_blob,
        |blob| {
            let drm = state.drm.get(drm_id);
            let _ = drm.fd.destroy_property_blob(blob);
        },
        |dispatcher, completion| {
            dispatcher.dispatch_to(
                output_scheduler_id(output_id),
                OutputSchedulerAction::FrameCompleted(completion),
            );
        },
    );
}

fn collect_protected_scene(
    state: &mut crate::state::State,
    drm_id: DrmId,
    drm_crtc_id: u32,
    output_id: OutputId,
    global_bounds: RectI,
) -> (Vec<SceneItem>, Vec<RecordedFrameElement>, Vec<DeferredRelease>) {
    let red = matches!(state.session_lock.blank(), Some(crate::session_lock::LockBlank::Red));
    let blank = {
        let drm = state.drm.get_mut(drm_id);
        lock_blank_item(drm, global_bounds, red)
    };
    let mut snapshots = Vec::new();
    let mut retired_candidates = Vec::new();
    if state.session_lock.shows_lock_surface(output_id) {
        if let Some(root) = state
            .session_lock
            .surface_for_output(output_id)
            .and_then(|surface| surface.try_ref(state))
            .map(|surface| surface.wl_surface())
        {
            collect_lock_surface_snapshots(
                state,
                root,
                global_bounds.x,
                global_bounds.y,
                &mut retired_candidates,
                &mut snapshots,
            );
        }
    }
    let mut items = Vec::new();
    let mut frame_elements = Vec::new();
    if let Some(blank) = blank {
        items.push(blank);
        push_scene_item_frame_elements(&mut frame_elements, 0..1);
    } else {
        log::error!("session lock blank texture is unavailable; desktop content stays hidden");
    }
    let surface_start = items.len();
    {
        let (objects, drm_map) = (&mut state.objects, &mut state.drm);
        let drm = drm_map.get_mut(drm_id);
        let drm_fd = drm.fd();
        push_scene_items_from_snapshots(
            &mut items,
            objects,
            drm,
            &drm_fd,
            drm_crtc_id,
            global_bounds,
            snapshots,
            false,
            None,
            u8::MAX,
            None,
        );
    }
    push_scene_item_frame_elements(&mut frame_elements, surface_start..items.len());
    let deferred_releases = retired_candidates
        .into_iter()
        .map(|retired| DeferredRelease::from_candidate(state, retired))
        .collect();
    (items, frame_elements, deferred_releases)
}

fn lock_blank_item(drm: &mut Drm, bounds: RectI, red: bool) -> Option<SceneItem> {
    let pixels: [u8; 4] = if red { [255, 0, 0, 255] } else { [0, 0, 0, 255] };
    let texture = drm.renderer.upload_rgba(&pixels, 1, 1).ok()?;
    Some(SceneItem {
        state: SceneItemState {
            key: SceneItemKey::LockBlank,
            surface_key: None,
            content_key: SceneItemContentKey::LockBlank { red },
            bounds,
            tex_transform: transform::Mat3::identity(),
            alpha: u8::MAX,
            mask: SceneItemMask::None,
        },
        mask: SceneItemMask::None,
        texture,
        damage: DamageRegion::from_rect(bounds),
        sample_damage_padding: 0,
        state_has_alpha: false,
        alpha: u8::MAX,
        opaque_region: DamageRegion::from_rect(bounds),
        opaque: true,
        sync: crate::sync::SurfaceSyncState::default(),
        direct_scanout: None,
        presentation_feedbacks: Vec::new(),
    })
}

fn collect_lock_surface_snapshots(
    state: &mut crate::state::State,
    surface: Resolved<WlSurfaceId>,
    base_x: i32,
    base_y: i32,
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
    surface_state.inherit_root_clip = false;
    for entry in &stack {
        if let StackEntry::Child(child_surface_id) = *entry {
            let child_base = {
                let Some(child_surface) = child_surface_id.try_ref(state) else {
                    continue;
                };
                let (x, y) = child_surface.current_subsurface_position();
                (surface_state.x + x, surface_state.y + y)
            };
            collect_lock_surface_snapshots(
                state,
                child_surface_id,
                child_base.0,
                child_base.1,
                retired_candidates,
                snapshots,
            );
        } else {
            snapshots.push(surface_state.clone());
        }
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

fn surface_allows_direct_scanout(root_clip: Option<WindowClip>, alpha: u8) -> bool {
    root_clip.is_none() && alpha == u8::MAX
}

fn effect_transition_requires_full_redraw(has_effects: bool, had_effects_last_frame: bool) -> bool {
    has_effects || had_effects_last_frame
}

fn collect_named_cursor_render_item(
    state: &mut crate::state::State,
    drm_id: DrmId,
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

            let texture = state
                .drm
                .get_mut(drm_id)
                .renderer
                .upload_rgba(&image.pixels_rgba, image.width, image.height)
                .ok()?;
            Some((
                SceneItem {
                    state: SceneItemState {
                        key: SceneItemKey::Cursor,
                        surface_key: None,
                        content_key: SceneItemContentKey::NamedCursor(image.content_key()),
                        bounds,
                        tex_transform: transform::Mat3::identity(),
                        alpha: u8::MAX,
                        mask: SceneItemMask::None,
                    },
                    mask: SceneItemMask::None,
                    texture,
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

fn commit_visible_scene_state(items: &mut [SceneItem]) {
    for item in items {
        item.state.alpha = item.alpha;
        item.state.mask = item.mask;
    }
}

// Frame callbacks and presentation feedback follow the items actually submitted for this frame.
// Interactive participation and logical workspace activity do not create a second pacing path.
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

fn capture_cursor_overlay(
    state: &crate::state::State,
    output_id: OutputId,
) -> Option<crate::capture::CaptureCursorImage> {
    let output = state.outputs.try_get(output_id)?;
    let request = build_cursor_plane_request(state, monotonic_time_msec())?;
    let (argb, stride) = match &request.upload {
        CursorPlaneUploadSource::Named { image } => {
            let image = unsafe { image.as_ref() }?;
            let width = image.width.max(0) as usize;
            let height = image.height.max(0) as usize;
            if width == 0 || height == 0 {
                return None;
            }
            let stride = width.saturating_mul(4);
            let argb = rgba_to_drm_argb8888(&image.pixels_rgba, width, height, stride, height);
            (argb, stride)
        }
        CursorPlaneUploadSource::SurfaceShm { buffer } => {
            let argb = buffer.argb_bytes();
            if argb.is_empty() {
                return None;
            }
            (argb, buffer.stride())
        }
    };
    Some(crate::capture::CaptureCursorImage {
        local_x: request.x.saturating_sub(output.x),
        local_y: request.y.saturating_sub(output.y),
        width: request.width,
        height: request.height,
        stride,
        argb,
        mode_width: output.current_mode.width,
        mode_height: output.current_mode.height,
        scale: output.scale,
        transform: output.transform,
    })
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

fn import_client_texture(drm: &mut Drm, buffer: &WlBuffer) -> Option<Rc<dyn Texture>> {
    if let Some(dmabuf) = buffer.dmabuf() {
        return match drm.renderer.import_dmabuf(dmabuf) {
            Ok(texture) => Some(texture),
            Err(error) => {
                log::warn!("failed to import dma-buf wl_buffer: {error}");
                None
            }
        };
    }
    let Some(sample) = buffer.shm_sample() else {
        log::warn!("shm wl_buffer has no readable pixels");
        return None;
    };
    match drm.renderer.import_shm(ShmImage {
        bytes: sample.bytes,
        width: sample.width,
        height: sample.height,
        stride: sample.stride,
        fourcc: sample.fourcc,
    }) {
        Ok(texture) => Some(texture),
        Err(error) => {
            log::warn!("failed to upload shm wl_buffer: {error}");
            None
        }
    }
}

fn wait_scene_acquires(drm: &mut Drm, items: &[SceneItem]) {
    for item in items {
        let _ = drm.renderer.wait_acquire(&item.sync.acquire);
    }
}

fn push_scene_items_from_snapshots(
    items: &mut Vec<SceneItem>,
    objects: &mut Box<crate::state::IndexMap<ObjectId, Box<dyn Object>>>,
    drm: &mut Drm,
    drm_fd: &DrmFd,
    drm_crtc_id: u32,
    output_bounds: RectI,
    snapshots: Vec<crate::wayland_objects::wl_surface::SurfaceRenderState>,
    allow_direct_scanout: bool,
    root_clip: Option<WindowClip>,
    alpha: u8,
    geometry_scale: Option<(RectI, RectI)>,
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
        if !bounds_visible_on_output(bounds, output_bounds, geometry_scale) {
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
        let Some(texture) = import_client_texture(drm, buffer) else {
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
            if let Some(window_clip) = root_clip {
                let mask = match window_clip {
                    WindowClip::Frame(clip) => SceneItemMask::RoundedClip { clip },
                    WindowClip::Corners(clip) => SceneItemMask::CornerRoundedClip { clip },
                };
                (mask, DamageRegion::new(), false)
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
                alpha,
                mask,
            },
            mask,
            texture,
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
    let inner_radius = visual_state
        .clip
        .map(|clip| clip.clip().corner_radius)
        .unwrap_or(0);
    let ring = crate::render::RoundedRing {
        outer: visual_state.frame_clip,
        inner: RootSurfaceClip::new(
            visual_state.content_rect,
            inner_radius,
            visual_state.frame_clip.rounding_power,
        ),
    };
    let shadow_clip = visual_state.shadow_clip();
    let dim_clip = visual_state.frame_clip;
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
                    clip: shadow_clip,
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
            element: EffectElement::DimOverlay {
                clip: dim_clip,
                alpha: visual_state.effects.dim_alpha,
            },
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
        collect_named_cursor_render_item(state, drm_id, output_bounds, frame_time_msec)
    {
        return (vec![item], next_frame_in_msec);
    }
    if !matches!(state.seat.visible_cursor_source(), Some(CursorSourceRef::Surface { .. })) {
        return (Vec::new(), None);
    }
    let cursor_snapshots = collect_cursor_surface_snapshots(state, retired_candidates);
    let (objects, drm_map) = (&mut state.objects, &mut state.drm);
    let drm = drm_map.get_mut(drm_id);
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
        None,
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

struct ClosingRootPeek {
    render: SurfaceRenderState,
    stack: Vec<StackEntry>,
    role: Role,
    xdg_surface: Option<Resolved<XdgSurfaceId>>,
    has_buffer: bool,
    active_output: Option<OutputId>,
}

impl ClosingRootPeek {
    fn from_surface(surface: &WlSurface) -> Option<Self> {
        Some(Self {
            render: surface.render_state_at(0, 0)?,
            stack: surface.current_stack().to_vec(),
            role: surface.role(),
            xdg_surface: surface.xdg_surface(),
            has_buffer: surface.has_committed_buffer(),
            active_output: surface.active_output_id(),
        })
    }
}

pub(crate) struct PlannedClosingSurface {
    pub buffer: Resolved<WlBufferId>,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub tex_transform: transform::Mat3,
    pub sync: crate::sync::SurfaceSyncState,
    pub inherit_root_clip: bool,
}

pub(crate) struct PlannedClosingEffect {
    pub phase: FramePhase,
    pub element: EffectElement,
}

// Window-local frozen image of one closing toplevel. This is the only close-animation source:
// geometry and effects are copied before teardown, the pump fades the texture, and dropping the
// realized snapshot releases it. Unsupported capture returns no plan. This is not an output
// capture, a debug view of the CRTC, or a `zwlr_screencopy` source. Output-sized capture must use
// `collect_output_render_plan` so both live workspace participants and the same frame-plan
// participation are visible.
pub(crate) struct ClosingCapturePlan {
    pub source: Resolved<WlSurfaceId>,
    pub drm_id: DrmId,
    pub output_id: OutputId,
    pub crtc_id: u32,
    pub bounds: RectI,
    pub presentation_transform: OutputPresentationTransform,
    pub surfaces: Vec<PlannedClosingSurface>,
    pub effects: Vec<PlannedClosingEffect>,
    pub blur: Option<FrozenClosingBlur>,
    pub content_alpha: u8,
    pub root_clip: Option<WindowClip>,
}

pub(crate) fn begin_managed_toplevel_close_surface(
    state: &mut State,
    surface_id: Resolved<WlSurfaceId>,
    surface: &WlSurface,
    dispatcher: &mut Dispatcher,
) {
    let Some(peek) = ClosingRootPeek::from_surface(surface) else {
        return;
    };
    if !closing_snapshot_eligible(state, surface_id, &peek) {
        return;
    }
    state.refresh_root_geometry_for_closing(surface_id, surface, None);
    let Some(peek) = ClosingRootPeek::from_surface(surface) else {
        return;
    };
    begin_closing_from_peek(state, surface_id, peek, dispatcher);
}

pub(crate) fn begin_managed_toplevel_close_id(
    state: &mut State,
    surface_id: Resolved<WlSurfaceId>,
    decoration_mode: Option<
        crate::wayland_objects::xdg_surface::xdg_toplevel::ToplevelDecorationMode,
    >,
    dispatcher: &mut Dispatcher,
) {
    let Some(surface) = state
        .objects
        .try_grab_object::<WlSurface>(surface_id.object_id())
    else {
        return;
    };
    let Some(peek) = ClosingRootPeek::from_surface(&surface) else {
        return;
    };
    if !closing_snapshot_eligible(state, surface_id, &peek) {
        return;
    }
    state.refresh_root_geometry_for_closing(surface_id, &surface, decoration_mode);
    let peek = ClosingRootPeek::from_surface(&surface);
    drop(surface);
    let Some(peek) = peek else {
        return;
    };
    begin_closing_from_peek(state, surface_id, peek, dispatcher);
}

fn closing_snapshot_eligible(
    state: &State,
    surface_id: Resolved<WlSurfaceId>,
    peek: &ClosingRootPeek,
) -> bool {
    if state
        .closing_snapshots
        .iter()
        .any(|snapshot| snapshot.source() == surface_id)
    {
        return false;
    }
    if !state.session_active
        || (state.config.animations.closing().duration().is_zero()
            && state.config.animations.fade_out().duration().is_zero())
    {
        return false;
    }
    let Some(managed) = state.managed_root_surface_state(surface_id) else {
        return false;
    };
    if !managed.mapped || !peek.has_buffer || peek.role != Role::XdgSurface {
        return false;
    }
    let Some(xdg_surface) = peek.xdg_surface else {
        return false;
    };
    state.xdg_toplevel(xdg_surface).is_some()
}

fn begin_closing_from_peek(
    state: &mut State,
    surface_id: Resolved<WlSurfaceId>,
    peek: ClosingRootPeek,
    dispatcher: &mut Dispatcher,
) {
    if !closing_snapshot_eligible(state, surface_id, &peek) {
        return;
    }
    let Some(plan) = plan_closing_from_peek(state, surface_id, &peek) else {
        return;
    };
    let Some(snapshot) = realize_closing_capture(state, plan) else {
        return;
    };
    state.closing_snapshots.push(snapshot);
    kick_visual_animation_pump(dispatcher);
}

pub(crate) fn plan_closing_capture(
    state: &State,
    surface_id: Resolved<WlSurfaceId>,
) -> Option<ClosingCapturePlan> {
    let peek = {
        let surface = surface_id.try_ref(state)?;
        ClosingRootPeek::from_surface(surface)?
    };
    plan_closing_from_peek(state, surface_id, &peek)
}

fn plan_closing_from_peek(
    state: &State,
    surface_id: Resolved<WlSurfaceId>,
    peek: &ClosingRootPeek,
) -> Option<ClosingCapturePlan> {
    let mut renders = Vec::new();
    collect_peeked_root(state, peek, &mut renders);
    if renders.is_empty() {
        return None;
    }
    let visual = state.root_surface_visual_state(surface_id);
    let mut rects = Vec::new();
    if let Some(influence) = visual.as_ref().map(|visual| visual.influence_bounds()) {
        rects.push(influence);
    }
    for render in &renders {
        rects.push(RectI::new(render.x, render.y, render.width, render.height)?);
    }
    let bounds = union_rects(rects)?;
    if bounds.is_empty() {
        return None;
    }
    let outputs = state.outputs_intersecting_rect(bounds);
    if outputs.is_empty() {
        return None;
    }
    let mut drm_ids = Vec::new();
    for output_id in &outputs {
        let backend = state.backend_for_output(*output_id)?;
        if !drm_ids.contains(&backend.drm_id) {
            drm_ids.push(backend.drm_id);
        }
    }
    if drm_ids.len() != 1 {
        return None;
    }
    let drm_id = drm_ids[0];
    let preferred = peek
        .active_output
        .filter(|output_id| outputs.contains(output_id));
    let output_id = state.primary_output_for_rect(bounds, preferred)?;
    let backend = state.backend_for_output(output_id)?;
    if backend.drm_id != drm_id {
        return None;
    }
    let crtc_id = state.output_crtc_id(output_id)?;
    let presentation_transform = state
        .drm
        .try_get(drm_id)
        .and_then(|drm| drm.output(crtc_id))
        .map(|output| output.presentation_transform)
        .unwrap_or(OutputPresentationTransform::FlipY);
    let (blur, mut effects) = split_closing_effects(visual);
    let dx = bounds.x.saturating_neg();
    let dy = bounds.y.saturating_neg();
    for effect in &mut effects {
        translate_effect_element(&mut effect.element, dx, dy);
    }
    let content_alpha = visual
        .map(|visual| visual.effects.opacity_alpha)
        .unwrap_or(u8::MAX);
    let root_clip = visual.and_then(|visual| visual.clip);
    let surfaces = renders
        .into_iter()
        .map(|render| PlannedClosingSurface {
            buffer: render.buffer,
            x: render.x.saturating_add(dx),
            y: render.y.saturating_add(dy),
            width: render.width,
            height: render.height,
            tex_transform: render.tex_transform,
            sync: render.sync,
            inherit_root_clip: render.inherit_root_clip,
        })
        .collect();
    Some(ClosingCapturePlan {
        source: surface_id,
        drm_id,
        output_id,
        crtc_id,
        bounds,
        presentation_transform,
        surfaces,
        effects,
        blur,
        content_alpha,
        root_clip,
    })
}

fn collect_peeked_root(state: &State, peek: &ClosingRootPeek, out: &mut Vec<SurfaceRenderState>) {
    let render = &peek.render;
    for entry in &peek.stack {
        match *entry {
            StackEntry::Child(child) => {
                let offset = child
                    .try_ref(state)
                    .map(|surface| surface.current_subsurface_position())
                    .unwrap_or((0, 0));
                peek_surface_tree(
                    state,
                    child,
                    render.x.saturating_add(offset.0),
                    render.y.saturating_add(offset.1),
                    true,
                    out,
                );
            }
            StackEntry::Parent => out.push(render.clone()),
        }
    }
    let Some(xdg_surface) = peek.xdg_surface else {
        return;
    };
    for popup in popup_surfaces_for_xdg(state, xdg_surface) {
        let Some(origin) = popup_origin_from_parent(state, popup, (render.x, render.y)) else {
            continue;
        };
        peek_surface_tree(state, popup, origin.0, origin.1, false, out);
    }
}

fn peek_surface_tree(
    state: &State,
    surface: Resolved<WlSurfaceId>,
    base_x: i32,
    base_y: i32,
    inherit_root_clip: bool,
    out: &mut Vec<SurfaceRenderState>,
) {
    let Some((render, stack, xdg_surface)) = (|| {
        let wl_surface = surface.try_ref(state)?;
        let mut render = wl_surface.render_state_at(base_x, base_y)?;
        render.inherit_root_clip = inherit_root_clip;
        Some((render, wl_surface.current_stack().to_vec(), wl_surface.xdg_surface()))
    })() else {
        return;
    };
    for entry in stack {
        match entry {
            StackEntry::Child(child) => {
                let offset = child
                    .try_ref(state)
                    .map(|surface| surface.current_subsurface_position())
                    .unwrap_or((0, 0));
                peek_surface_tree(
                    state,
                    child,
                    render.x.saturating_add(offset.0),
                    render.y.saturating_add(offset.1),
                    inherit_root_clip,
                    out,
                );
            }
            StackEntry::Parent => out.push(render.clone()),
        }
    }
    let Some(xdg_surface) = xdg_surface else {
        return;
    };
    for popup in popup_surfaces_for_xdg(state, xdg_surface) {
        let Some(origin) = popup_origin_from_parent(state, popup, (render.x, render.y)) else {
            continue;
        };
        peek_surface_tree(state, popup, origin.0, origin.1, false, out);
    }
}

fn popup_surfaces_for_xdg(
    state: &State,
    xdg_surface: Resolved<XdgSurfaceId>,
) -> Vec<Resolved<WlSurfaceId>> {
    let Some(xdg_surface) = xdg_surface.try_ref(state) else {
        return Vec::new();
    };
    xdg_surface
        .popup_children()
        .iter()
        .copied()
        .filter_map(|popup_id: Resolved<XdgPopupId>| {
            let popup = popup_id.try_ref(state)?;
            let popup_xdg = popup.xdg_surface().try_ref(state)?;
            Some(popup_xdg.wl_surface())
        })
        .collect()
}

fn popup_origin_from_parent(
    state: &State,
    popup: Resolved<WlSurfaceId>,
    parent_origin: (i32, i32),
) -> Option<(i32, i32)> {
    let wl_surface = popup.try_ref(state)?;
    let xdg_surface = wl_surface.xdg_surface()?.try_ref(state)?;
    let popup_role = xdg_surface.popup_role()?.try_ref(state)?;
    let local_window_geometry = xdg_surface.window_geometry_rect_for_surface(popup, state);
    let geometry = popup_role.current_geometry();
    Some((
        parent_origin.0 + geometry.x - local_window_geometry.x,
        parent_origin.1 + geometry.y - local_window_geometry.y,
    ))
}

fn split_closing_effects(
    visual: Option<crate::render::RootSurfaceVisualState>,
) -> (Option<FrozenClosingBlur>, Vec<PlannedClosingEffect>) {
    let Some(visual) = visual else {
        return (None, Vec::new());
    };
    let mut recorded = Vec::new();
    push_root_surface_effect_elements(&mut recorded, Some(visual), FramePhase::UnderWindow);
    push_root_surface_effect_elements(&mut recorded, Some(visual), FramePhase::OverWindow);
    push_root_surface_effect_elements(&mut recorded, Some(visual), FramePhase::Overlay);
    let mut blur = None;
    let mut effects = Vec::new();
    for element in recorded {
        let RecordedFrameElement::Effect { phase, element } = element else {
            continue;
        };
        if let EffectElement::WindowBlurBackdrop { ring, style, alpha } = element {
            blur = Some(FrozenClosingBlur { ring, style, alpha });
            continue;
        }
        effects.push(PlannedClosingEffect { phase, element });
    }
    (blur, effects)
}

fn bounds_visible_on_output(
    bounds: RectI,
    output_bounds: RectI,
    geometry_scale: Option<(RectI, RectI)>,
) -> bool {
    let visible = match geometry_scale {
        Some((logical, visual)) => crate::render::map_layout_rect(logical, visual, bounds),
        None => bounds,
    };
    visible.intersects(output_bounds)
}

fn union_rects(rects: Vec<RectI>) -> Option<RectI> {
    let mut rects = rects.into_iter();
    let mut acc = rects.next()?;
    for rect in rects {
        acc = RectI::from_extents(
            acc.x.min(rect.x),
            acc.y.min(rect.y),
            acc.x2().max(rect.x2()),
            acc.y2().max(rect.y2()),
        )?;
    }
    Some(acc)
}

fn realize_closing_capture(
    state: &mut State,
    plan: ClosingCapturePlan,
) -> Option<RetainedClosingSnapshot> {
    let texture = realize_closing_texture(state, &plan).or_else(|| {
        #[cfg(test)]
        if state.realize_closing_snapshots_in_tests {
            return Some(crate::render::inert_texture());
        }
        #[cfg(not(test))]
        {
            return None;
        }
        #[cfg(test)]
        None
    })?;
    let id = state.next_closing_snapshot_id;
    state.next_closing_snapshot_id = state.next_closing_snapshot_id.saturating_add(1);
    let workspace_id = state
        .managed_root_surface_state(plan.source)
        .map(|managed| managed.workspace_id);
    Some(RetainedClosingSnapshot::new(
        id,
        plan.source,
        workspace_id,
        plan.bounds,
        plan.blur,
        plan.drm_id,
        texture,
    ))
}

fn realize_closing_texture(
    state: &mut State,
    plan: &ClosingCapturePlan,
) -> Option<Rc<dyn Texture>> {
    let (objects, drm_map) = (&mut state.objects, &mut state.drm);
    let drm = drm_map.try_get_mut(plan.drm_id)?;
    drm.bind_context();
    let mut uploaded = Vec::new();
    for surface in &plan.surfaces {
        let mut obj = objects.grab(surface.buffer.object_id());
        let buffer = obj.downcast_mut::<WlBuffer>()?;
        let _ = drm.renderer.wait_acquire(&surface.sync.acquire);
        let texture = import_client_texture(drm, buffer)?;
        let tex_transform = buffer.base_tex_transform().mul(surface.tex_transform);
        uploaded.push((texture, tex_transform));
    }
    let elements = closing_capture_elements(plan, &uploaded);
    let (reference_width, reference_height) = {
        let buffer = drm.output(plan.crtc_id)?.render_buffer();
        (buffer.width, buffer.height)
    };
    drm.renderer
        .capture_transparent(
            &elements,
            plan.bounds.width,
            plan.bounds.height,
            (reference_width, reference_height),
            plan.presentation_transform,
        )
        .ok()
}

fn closing_capture_elements(
    plan: &ClosingCapturePlan,
    uploaded: &[(Rc<dyn Texture>, transform::Mat3)],
) -> Vec<FrameElement> {
    let mut elements = Vec::new();
    for effect in &plan.effects {
        if effect.phase == FramePhase::UnderWindow {
            elements.push(FrameElement::Effect { phase: effect.phase, element: effect.element });
        }
    }
    for (surface, (texture, tex_transform)) in plan.surfaces.iter().zip(uploaded.iter()) {
        let Some(bounds) = RectI::new(surface.x, surface.y, surface.width, surface.height) else {
            continue;
        };
        let mask = if surface.inherit_root_clip {
            plan.root_clip
                .map(|window_clip| {
                    let clip = RootSurfaceClip::new(
                        window_clip.clip().outer_rect.translate(
                            plan.bounds.x.saturating_neg(),
                            plan.bounds.y.saturating_neg(),
                        ),
                        window_clip.clip().corner_radius,
                        window_clip.clip().rounding_power,
                    );
                    match window_clip {
                        WindowClip::Frame(_) => {
                            TextureMask::RoundedClip { clip: localize_clip(clip, bounds) }
                        }
                        WindowClip::Corners(_) => {
                            TextureMask::CornerRoundedClip { clip: localize_clip(clip, bounds) }
                        }
                    }
                })
                .unwrap_or(TextureMask::None)
        } else {
            TextureMask::None
        };
        elements.push(FrameElement::Texture {
            phase: FramePhase::Content,
            element: TextureElement {
                source: TextureSource::Texture,
                texture: Rc::clone(texture),
                x: surface.x,
                y: surface.y,
                width: surface.width,
                height: surface.height,
                tex_transform: *tex_transform,
                damage: DamageRegion::from_rect(bounds),
                blend: true,
                alpha: plan.content_alpha,
                mask,
            },
        });
    }
    for effect in &plan.effects {
        if effect.phase != FramePhase::UnderWindow {
            elements.push(FrameElement::Effect { phase: effect.phase, element: effect.element });
        }
    }
    elements
}

fn closing_snapshots_precede_tier(tier: CompositionTier) -> bool {
    matches!(tier, CompositionTier::Top | CompositionTier::Overlay)
}

fn scale_u8(value: u8, factor: u8) -> u8 {
    u8::try_from((u16::from(value) * u16::from(factor)) / 255u16).unwrap_or(0)
}

fn append_closing_snapshots(
    state: &State,
    output_bounds: RectI,
    workspace_id: Option<WorkspaceId>,
    motion: WorkspaceMotion,
    items: &mut Vec<SceneItem>,
    frame_elements: &mut Vec<RecordedFrameElement>,
) {
    for snapshot in &state.closing_snapshots {
        if snapshot.workspace_id() != workspace_id {
            continue;
        }
        let alpha = scale_u8(snapshot.alpha(), motion.alpha);
        let bounds = snapshot.visual_bounds().translate(motion.translation_x, 0);
        if alpha == 0 || bounds.is_empty() || !bounds.intersects(output_bounds) {
            continue;
        }
        if let Some(blur) = snapshot.blur() {
            let ring = posed_blur_ring(
                snapshot.bounds(),
                snapshot.visual_bounds(),
                blur.ring,
                motion.translation_x,
            );
            frame_elements.push(RecordedFrameElement::Effect {
                phase: FramePhase::UnderWindow,
                element: EffectElement::WindowBlurBackdrop {
                    ring,
                    style: blur.style,
                    alpha: scale_u8(blur.alpha, alpha),
                },
            });
        }
        let item_index = items.len();
        items.push(SceneItem {
            state: SceneItemState {
                key: SceneItemKey::ClosingSnapshot(snapshot.id()),
                surface_key: None,
                content_key: SceneItemContentKey::ClosingSnapshot { id: snapshot.id(), alpha },
                bounds,
                tex_transform: transform::Mat3::identity(),
                alpha,
                mask: SceneItemMask::None,
            },
            mask: SceneItemMask::None,
            texture: snapshot.texture(),
            damage: DamageRegion::from_rect(bounds),
            sample_damage_padding: 0,
            state_has_alpha: true,
            alpha,
            opaque_region: DamageRegion::new(),
            opaque: false,
            sync: crate::sync::SurfaceSyncState::default(),
            direct_scanout: None,
            presentation_feedbacks: Vec::new(),
        });
        frame_elements
            .push(RecordedFrameElement::Texture { phase: FramePhase::Content, item_index });
    }
}

fn posed_blur_ring(
    captured: RectI,
    visual: RectI,
    ring: RoundedRing,
    translation_x: i32,
) -> RoundedRing {
    RoundedRing {
        outer: posed_clip(captured, visual, ring.outer, translation_x),
        inner: posed_clip(captured, visual, ring.inner, translation_x),
    }
}

fn posed_clip(
    captured: RectI,
    visual: RectI,
    clip: RootSurfaceClip,
    translation_x: i32,
) -> RootSurfaceClip {
    let mapped = crate::render::map_layout_rect(captured, visual, clip.outer_rect)
        .translate(translation_x, 0);
    RootSurfaceClip::new(mapped, clip.corner_radius, clip.rounding_power)
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
    let mut content_entries = Vec::new();
    let mut rendered_content = false;
    for entry in &view.entries {
        if entry.tier == CompositionTier::Content && !rendered_content {
            content_entries.push(entry);
            continue;
        }
        if !rendered_content
            && (closing_snapshots_precede_tier(entry.tier) || !content_entries.is_empty())
        {
            render_workspace_content(
                state,
                drm_id,
                drm_crtc_id,
                output_id,
                output_bounds,
                &content_entries,
                &mut items,
                &mut frame_elements,
                &mut retired_candidates,
            );
            rendered_content = true;
            content_entries.clear();
        }
        render_composed_entry(
            state,
            drm_id,
            drm_crtc_id,
            output_bounds,
            entry,
            WorkspaceMotion::IDENTITY,
            &mut items,
            &mut frame_elements,
            &mut retired_candidates,
        );
    }
    if !rendered_content {
        render_workspace_content(
            state,
            drm_id,
            drm_crtc_id,
            output_id,
            output_bounds,
            &content_entries,
            &mut items,
            &mut frame_elements,
            &mut retired_candidates,
        );
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
    let (objects, drm_map) = (&mut state.objects, &mut state.drm);
    let drm = drm_map.get_mut(drm_id);
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
        None,
    );
}

fn scene_item_to_texture_element(item: &SceneItem) -> TextureElement {
    TextureElement {
        source: TextureSource::Texture,
        texture: Rc::clone(&item.texture),
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
            SceneItemMask::CornerRoundedClip { clip } => {
                TextureMask::CornerRoundedClip { clip: localize_clip(clip, item.state.bounds) }
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
        translate_scene_item_mask(&mut item.mask, dx, dy);
    }
}

fn translate_scene_item_mask(mask: &mut SceneItemMask, dx: i32, dy: i32) {
    match mask {
        SceneItemMask::None => {}
        SceneItemMask::RoundedClip { clip } | SceneItemMask::CornerRoundedClip { clip } => {
            clip.outer_rect = clip.outer_rect.translate(dx, dy);
        }
        SceneItemMask::FrameRing { ring } => {
            ring.outer.outer_rect = ring.outer.outer_rect.translate(dx, dy);
            ring.inner.outer_rect = ring.inner.outer_rect.translate(dx, dy);
        }
    }
}

fn scale_effect_alpha(element: &mut EffectElement, factor: u8) {
    let alpha = match element {
        EffectElement::WindowBorder { alpha, .. }
        | EffectElement::WindowShadow { alpha, .. }
        | EffectElement::WindowBlurBackdrop { alpha, .. }
        | EffectElement::DimOverlay { alpha, .. } => alpha,
    };
    *alpha = scale_u8(*alpha, factor);
}

fn workspace_transition_blocks_scanout(
    state: &State,
    root_surface: crate::clients::Resolved<crate::protocol_generated::WlSurfaceId>,
) -> bool {
    let Some(workspace_id) = state.root_surface_workspace(root_surface) else {
        return false;
    };
    let Some(group_id) = state
        .workspaces
        .try_get(workspace_id)
        .and_then(|workspace| workspace.group)
    else {
        return false;
    };
    state
        .workspace_groups
        .try_get(group_id)
        .and_then(|group| group.transition)
        .is_some_and(|transition| transition.involves(workspace_id))
}

fn apply_workspace_motion(
    items: &mut [SceneItem],
    frame_elements: &mut [RecordedFrameElement],
    motion: WorkspaceMotion,
) {
    if motion.is_identity() {
        return;
    }
    for item in items {
        if motion.translation_x != 0 {
            item.state.bounds = item.state.bounds.translate(motion.translation_x, 0);
            item.damage.translate(motion.translation_x, 0);
            item.opaque_region.translate(motion.translation_x, 0);
            translate_scene_item_mask(&mut item.mask, motion.translation_x, 0);
        }
        item.alpha = scale_u8(item.alpha, motion.alpha);
        if motion.alpha < u8::MAX {
            item.opaque = false;
            item.opaque_region.clear();
            item.direct_scanout = None;
            item.state_has_alpha = true;
        }
    }
    for frame_element in frame_elements {
        let RecordedFrameElement::Effect { element, .. } = frame_element else {
            continue;
        };
        if motion.translation_x != 0 {
            translate_effect_element(element, motion.translation_x, 0);
        }
        scale_effect_alpha(element, motion.alpha);
    }
}

fn render_workspace_content(
    state: &mut State,
    drm_id: DrmId,
    drm_crtc_id: u32,
    output_id: OutputId,
    output_bounds: RectI,
    entries: &[&ComposedEntry],
    items: &mut Vec<SceneItem>,
    frame_elements: &mut Vec<RecordedFrameElement>,
    retired_candidates: &mut Vec<RetiredBufferCandidate>,
) {
    let workspace_order = crate::scene::visibility::output_workspace_ids(
        state,
        output_id,
        crate::scene::visibility::WorkspaceParticipationPolicy::Steady,
    );
    let mut rendered = BTreeSet::new();
    for workspace_id in workspace_order {
        let motion = state.workspace_motion_for_output(output_id, Some(workspace_id));
        let matching = entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| {
                state.root_surface_workspace(entry.root_surface) == Some(workspace_id)
            })
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        for index in matching {
            render_composed_entry(
                state,
                drm_id,
                drm_crtc_id,
                output_bounds,
                entries[index],
                motion,
                items,
                frame_elements,
                retired_candidates,
            );
        }
        append_closing_snapshots(
            state,
            output_bounds,
            Some(workspace_id),
            motion,
            items,
            frame_elements,
        );
        rendered.insert(workspace_id);
    }
    for entry in entries {
        let workspace_id = state.root_surface_workspace(entry.root_surface);
        if workspace_id.is_some_and(|workspace_id| rendered.contains(&workspace_id)) {
            continue;
        }
        let motion = workspace_id
            .and_then(|workspace_id| {
                let owner = state.workspace_output_id(workspace_id)?;
                Some(state.workspace_motion_for_output(owner, Some(workspace_id)))
            })
            .unwrap_or(WorkspaceMotion::IDENTITY);
        render_composed_entry(
            state,
            drm_id,
            drm_crtc_id,
            output_bounds,
            entry,
            motion,
            items,
            frame_elements,
            retired_candidates,
        );
    }
    append_closing_snapshots(
        state,
        output_bounds,
        None,
        WorkspaceMotion::IDENTITY,
        items,
        frame_elements,
    );
}

fn render_composed_entry(
    state: &mut State,
    drm_id: DrmId,
    drm_crtc_id: u32,
    output_bounds: RectI,
    entry: &ComposedEntry,
    motion: WorkspaceMotion,
    items: &mut Vec<SceneItem>,
    frame_elements: &mut Vec<RecordedFrameElement>,
    retired_candidates: &mut Vec<RetiredBufferCandidate>,
) {
    if !entry.participates_in_render || motion.alpha == 0 {
        return;
    }
    let root_surface = entry.root_surface;
    let Some(surface) = state
        .objects
        .try_ref_object::<WlSurface>(root_surface.object_id())
    else {
        return;
    };
    if !surface.is_root()
        || surface.has_cursor_role()
        || surface.has_dnd_icon_role()
        || surface.has_lock_surface_role()
    {
        return;
    }
    let logical_layout = state
        .committed_root_geometry(root_surface)
        .map(|geometry| geometry.layout_rect_global);
    let visual_layout = state
        .managed_root_surfaces
        .get(&root_surface)
        .and_then(|managed| managed.geometry_animation.visual_rect());
    let geometry_scale = match (logical_layout, visual_layout) {
        (Some(logical), Some(visual))
            if logical != visual && logical.width > 0 && logical.height > 0 =>
        {
            Some((logical, visual))
        }
        _ => None,
    };
    let mut visual_state = state.root_surface_visual_state(root_surface);
    if let Some((logical, visual)) = geometry_scale {
        if let Some(visual_state) = visual_state.as_mut() {
            crate::render::place_visual_frame(visual_state, logical, visual);
        }
    }
    let allow_direct_scanout = entry.allow_direct_scanout
        && geometry_scale.is_none()
        && motion.is_identity()
        && !workspace_transition_blocks_scanout(state, root_surface);
    let root_clip = visual_state.and_then(|visual| visual.clip);
    let effects_on_output = visual_state
        .map(|visual| visual.influence_bounds().intersects(output_bounds))
        .unwrap_or(false);
    let effect_start = frame_elements.len();
    if effects_on_output {
        push_root_surface_effect_elements(frame_elements, visual_state, FramePhase::UnderWindow);
    }
    let mut snapshots = Vec::new();
    collect_surface_snapshots(state, root_surface, 0, 0, true, retired_candidates, &mut snapshots);
    let (objects, drm_map) = (&mut state.objects, &mut state.drm);
    let drm = drm_map.get_mut(drm_id);
    let drm_fd = drm.fd();
    let item_start = items.len();
    push_scene_items_from_snapshots(
        items,
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
        geometry_scale,
    );
    push_scene_item_frame_elements(frame_elements, item_start..items.len());
    if effects_on_output {
        push_root_surface_effect_elements(frame_elements, visual_state, FramePhase::OverWindow);
        push_root_surface_effect_elements(frame_elements, visual_state, FramePhase::Overlay);
    }
    if let Some((logical, visual)) = geometry_scale {
        scale_scene_items_into_visual_box(&mut items[item_start..], logical, visual);
    }
    apply_workspace_motion(&mut items[item_start..], &mut frame_elements[effect_start..], motion);
}

fn scale_scene_items_into_visual_box(items: &mut [SceneItem], logical: RectI, visual: RectI) {
    for item in items {
        item.state.bounds = crate::render::map_layout_rect(logical, visual, item.state.bounds);
        item.damage = DamageRegion::from_rect(item.state.bounds);
        item.opaque = false;
        item.opaque_region.clear();
        item.direct_scanout = None;
        item.sample_damage_padding = item.sample_damage_padding.max(1);
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

    #[test]
    fn a_stale_flip_cannot_target_the_restored_backend() {
        use crate::{
            runner::{Dispatcher, Model},
            state::DrmId,
            test_support::{state_for_tests, test_output_with_crtc_name},
        };

        use super::{DrmAction, DrmReader, DrmReaderAction};

        let drm_id = DrmId::from(1usize);
        let fd: std::os::fd::OwnedFd = std::fs::File::open("/dev/null").unwrap().into();
        let mut reader = DrmReader::new(drm_id, super::DrmFd { fd: std::rc::Rc::new(fd) });
        let mut state = state_for_tests();
        state.insert_output(test_output_with_crtc_name(
            crate::state::BackendOutputRef::new(drm_id),
            7,
            0,
            0,
            100,
            100,
        ));
        let mut dispatcher = Dispatcher::default();
        reader
            .process(&mut state, DrmReaderAction::SetGeneration { generation: 2 }, &mut dispatcher)
            .unwrap();

        reader
            .process(
                &mut state,
                DrmReaderAction::ReadChunk { buffer: flip_event(7).into(), generation: 1 },
                &mut dispatcher,
            )
            .unwrap();
        assert!(!dispatcher.test_has_queued_action(|action: &DrmAction| {
            matches!(action, DrmAction::FlipComplete { .. })
        }));

        reader
            .process(
                &mut state,
                DrmReaderAction::ReadChunk { buffer: flip_event(7).into(), generation: 2 },
                &mut dispatcher,
            )
            .unwrap();
        let output_id = state.output_id_for_drm_crtc(drm_id, 7).unwrap();
        assert!(dispatcher.test_has_queued_action_for::<DrmAction, _>(output_id, |action| {
            matches!(action, DrmAction::FlipComplete { crtc_id: 7, generation: 2, .. })
        }));
    }

    fn flip_event(crtc_id: u32) -> Vec<u8> {
        let mut bytes = vec![0u8; 32];
        bytes[0..4].copy_from_slice(&2u32.to_ne_bytes());
        bytes[4..8].copy_from_slice(&32u32.to_ne_bytes());
        bytes[28..32].copy_from_slice(&crtc_id.to_ne_bytes());
        bytes
    }

    #[test]
    fn retained_assignment_requires_a_connected_advertised_mode() {
        assert!(super::Drm::retained_assignment_failure(true, true, true).is_none());
        assert_eq!(
            super::Drm::retained_assignment_failure(false, true, true),
            Some("connector is not connected")
        );
        assert_eq!(
            super::Drm::retained_assignment_failure(true, false, false),
            Some("connector has no retained mode")
        );
        assert_eq!(
            super::Drm::retained_assignment_failure(true, true, false),
            Some("retained mode is no longer advertised")
        );
    }
}

#[cfg(test)]
mod window_animation_tests {
    use std::time::{Duration, Instant};

    use {
        super::{animation_delay_msec_for_output, VisualAnimationPump},
        crate::{
            clients::ClientId,
            protocol_generated::WlSurfaceId,
            region::RectI,
            runner::Dispatcher,
            scheduler::{output_scheduler_id, OutputSchedulerAction, RequestFrameReason},
            state::{BackendOutputRef, DrmId, ObjectId, OutputId},
            test_support::{
                resolved_wl_surface as root, state_for_tests, test_output_with_crtc_name,
            },
            wayland_objects::wl_surface::WlSurface,
        },
    };

    fn mapped_focus_target(
        state: &mut crate::state::State,
    ) -> (OutputId, crate::clients::Resolved<WlSurfaceId>) {
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
        state.config.animations.set_focus_duration_msec(120);
        let surface = root(state, object_id);
        state.register_root_surface(surface);
        state.assign_root_surface_to_workspace(surface, workspace_id);
        (output_id, surface)
    }

    fn queues_animation_frame(dispatcher: &Dispatcher, output_id: OutputId) -> bool {
        dispatcher.test_has_queued_action_for::<OutputSchedulerAction, _>(
            output_scheduler_id(output_id),
            |action: &OutputSchedulerAction| {
                matches!(
                    action,
                    OutputSchedulerAction::RequestFrame { reason: RequestFrameReason::Animation }
                )
            },
        )
    }

    fn queues_animation_wakeup(dispatcher: &Dispatcher, output_id: OutputId) -> bool {
        dispatcher.test_has_queued_action_for::<OutputSchedulerAction, _>(
            output_scheduler_id(output_id),
            |action: &OutputSchedulerAction| {
                matches!(action, OutputSchedulerAction::ScheduleAnimation { .. })
            },
        )
    }

    fn content_alpha(
        state: &crate::state::State,
        surface: crate::clients::Resolved<WlSurfaceId>,
    ) -> u8 {
        state
            .managed_root_surfaces
            .get(&surface)
            .unwrap()
            .focus_animation
            .current()
            .content_alpha
    }

    fn visual_opacity(
        state: &crate::state::State,
        surface: crate::clients::Resolved<WlSurfaceId>,
    ) -> u8 {
        state
            .root_surface_visual_state(surface)
            .unwrap()
            .effects
            .opacity_alpha
    }

    #[test]
    fn unchanged_focus_start_schedules_a_wakeup_without_damage() {
        let mut state = state_for_tests();
        let (output_id, surface) = mapped_focus_target(&mut state);
        state.seat.keyboard_focus = Some(crate::input::SurfaceFocus::new(surface));

        let mut manager = VisualAnimationPump::new();
        let mut dispatcher = Dispatcher::default();
        manager.advance_and_schedule(&mut state, &mut dispatcher, Instant::now());

        assert!(queues_animation_wakeup(&dispatcher, output_id));
        assert!(!queues_animation_frame(&dispatcher, output_id));
    }

    #[test]
    fn changed_focus_sample_damages_the_output() {
        let mut state = state_for_tests();
        let (output_id, surface) = mapped_focus_target(&mut state);
        state.seat.keyboard_focus = Some(crate::input::SurfaceFocus::new(surface));
        let now = Instant::now();
        let mut manager = VisualAnimationPump::new();
        let mut dispatcher = Dispatcher::default();
        manager.advance_and_schedule(&mut state, &mut dispatcher, now);

        let mut dispatcher = Dispatcher::default();
        manager.advance_and_schedule(&mut state, &mut dispatcher, now + Duration::from_millis(60));

        assert!(queues_animation_frame(&dispatcher, output_id));
        assert_ne!(content_alpha(&state, surface), 120);
        assert_ne!(content_alpha(&state, surface), 255);
    }

    #[test]
    fn settled_focus_track_stops_scheduling() {
        let mut state = state_for_tests();
        let (output_id, surface) = mapped_focus_target(&mut state);
        state.seat.keyboard_focus = Some(crate::input::SurfaceFocus::new(surface));
        state.config.animations.set_focus_duration_msec(0);
        let mut manager = VisualAnimationPump::new();
        let mut dispatcher = Dispatcher::default();
        manager.advance_and_schedule(&mut state, &mut dispatcher, Instant::now());
        assert!(queues_animation_frame(&dispatcher, output_id));

        let mut dispatcher = Dispatcher::default();
        manager.advance_and_schedule(&mut state, &mut dispatcher, Instant::now());
        assert!(!queues_animation_frame(&dispatcher, output_id));
        assert!(!queues_animation_wakeup(&dispatcher, output_id));
        assert_eq!(content_alpha(&state, surface), 255);
    }

    #[test]
    fn repeated_output_sampling_follows_elapsed_time() {
        let mut state = state_for_tests();
        let (output_id, surface) = mapped_focus_target(&mut state);
        state.seat.keyboard_focus = Some(crate::input::SurfaceFocus::new(surface));
        let now = Instant::now();
        let mut manager = VisualAnimationPump::new();
        let mut dispatcher = Dispatcher::default();
        manager.advance_and_schedule_output(&mut state, &mut dispatcher, now, output_id);
        assert_eq!(content_alpha(&state, surface), 120);

        manager.advance_and_schedule_output(
            &mut state,
            &mut dispatcher,
            now + Duration::from_millis(40),
            output_id,
        );
        let once = content_alpha(&state, surface);
        manager.advance_and_schedule_output(
            &mut state,
            &mut dispatcher,
            now + Duration::from_millis(40),
            output_id,
        );
        assert_eq!(content_alpha(&state, surface), once);

        manager.advance_and_schedule_output(
            &mut state,
            &mut dispatcher,
            now + Duration::from_millis(80),
            output_id,
        );
        assert_ne!(content_alpha(&state, surface), once);
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

    #[test]
    fn mapped_visual_bounds_stay_visible_on_the_output_they_enter() {
        let logical = RectI::new(0, 0, 100, 80).unwrap();
        let visual = RectI::new(200, 0, 50, 40).unwrap();
        let destination = RectI::new(200, 0, 100, 100).unwrap();
        assert!(!logical.intersects(destination));
        assert!(super::bounds_visible_on_output(logical, destination, Some((logical, visual))));
        assert!(!super::bounds_visible_on_output(logical, destination, None));
    }

    #[test]
    fn opening_only_window_schedules_until_it_returns_to_idle() {
        let mut state = state_for_tests();
        let (output_id, surface) = mapped_focus_target(&mut state);
        state.config.animations.set_open_duration_msec(100);
        state
            .managed_root_surfaces
            .get_mut(&surface)
            .unwrap()
            .lifecycle_animation
            .start_opening();
        let now = Instant::now();
        let mut manager = VisualAnimationPump::new();
        let mut dispatcher = Dispatcher::default();
        manager.advance_and_schedule(&mut state, &mut dispatcher, now);

        assert!(queues_animation_wakeup(&dispatcher, output_id));
        assert!(!queues_animation_frame(&dispatcher, output_id));
        assert_eq!(visual_opacity(&state, surface), 0);
        assert_eq!(content_alpha(&state, surface), 120);

        let mut dispatcher = Dispatcher::default();
        manager.advance_and_schedule(&mut state, &mut dispatcher, now + Duration::from_millis(50));
        assert!(queues_animation_frame(&dispatcher, output_id));
        let mid = visual_opacity(&state, surface);
        assert!(mid > 0 && mid < 120);

        let mut dispatcher = Dispatcher::default();
        manager.advance_and_schedule(&mut state, &mut dispatcher, now + Duration::from_millis(100));
        assert!(queues_animation_frame(&dispatcher, output_id));
        assert_eq!(visual_opacity(&state, surface), 120);
        assert!(!state
            .managed_root_surfaces
            .get(&surface)
            .unwrap()
            .lifecycle_animation
            .is_active());

        let mut dispatcher = Dispatcher::default();
        manager.advance_and_schedule(&mut state, &mut dispatcher, now + Duration::from_millis(100));
        assert!(!queues_animation_frame(&dispatcher, output_id));
        assert!(!queues_animation_wakeup(&dispatcher, output_id));
    }

    #[test]
    fn zero_open_duration_settles_on_the_next_pump() {
        let mut state = state_for_tests();
        let (output_id, surface) = mapped_focus_target(&mut state);
        state.config.animations.set_open_duration_msec(0);
        state
            .managed_root_surfaces
            .get_mut(&surface)
            .unwrap()
            .lifecycle_animation
            .start_opening();
        let mut manager = VisualAnimationPump::new();
        let mut dispatcher = Dispatcher::default();
        manager.advance_and_schedule(&mut state, &mut dispatcher, Instant::now());

        assert!(queues_animation_frame(&dispatcher, output_id));
        assert_eq!(visual_opacity(&state, surface), 120);
        assert!(!state
            .managed_root_surfaces
            .get(&surface)
            .unwrap()
            .lifecycle_animation
            .is_active());

        let mut dispatcher = Dispatcher::default();
        manager.advance_and_schedule(&mut state, &mut dispatcher, Instant::now());
        assert!(!queues_animation_frame(&dispatcher, output_id));
        assert!(!queues_animation_wakeup(&dispatcher, output_id));
    }

    #[test]
    fn closing_snapshot_schedules_until_it_is_released() {
        let mut state = state_for_tests();
        let backend = BackendOutputRef::new(DrmId::from(0usize));
        let output_id =
            state.insert_output(test_output_with_crtc_name(backend, 1, 0, 0, 1280, 720));
        let source = crate::test_support::test_resolved_wl_surface_client1(4);
        let (snapshot, probe) = crate::visual_animation::RetainedClosingSnapshot::test_pending(
            1,
            source,
            RectI::new(20, 30, 100, 80).unwrap(),
            DrmId::from(0usize),
        );
        state.closing_snapshots.push(snapshot);
        state.config.animations.set_close_duration_msec(100);
        let mut manager = VisualAnimationPump::new();
        let now = Instant::now();

        let mut dispatcher = Dispatcher::default();
        manager.advance_and_schedule(&mut state, &mut dispatcher, now);
        assert!(queues_animation_wakeup(&dispatcher, output_id));
        assert!(!queues_animation_frame(&dispatcher, output_id));
        assert_eq!(state.closing_snapshots[0].alpha(), 255);

        let mut dispatcher = Dispatcher::default();
        manager.advance_and_schedule(&mut state, &mut dispatcher, now + Duration::from_millis(50));
        assert!(queues_animation_frame(&dispatcher, output_id));
        assert!(state.closing_snapshots[0].alpha() > 0 && state.closing_snapshots[0].alpha() < 255);

        let mut dispatcher = Dispatcher::default();
        manager.advance_and_schedule(&mut state, &mut dispatcher, now + Duration::from_millis(100));
        assert!(queues_animation_frame(&dispatcher, output_id));
        assert!(state.closing_snapshots.is_empty());
        assert_eq!(probe.get(), 1);

        let mut dispatcher = Dispatcher::default();
        manager.advance_and_schedule(&mut state, &mut dispatcher, now + Duration::from_millis(100));
        assert!(!queues_animation_frame(&dispatcher, output_id));
        assert!(!queues_animation_wakeup(&dispatcher, output_id));
    }

    #[test]
    fn zero_close_duration_releases_the_snapshot_on_the_next_pump() {
        let mut state = state_for_tests();
        let backend = BackendOutputRef::new(DrmId::from(0usize));
        let output_id = state.insert_output(test_output_with_crtc_name(backend, 1, 0, 0, 800, 600));
        let (snapshot, probe) = crate::visual_animation::RetainedClosingSnapshot::test_pending(
            2,
            crate::test_support::test_resolved_wl_surface_client1(8),
            RectI::new(0, 0, 40, 40).unwrap(),
            DrmId::from(0usize),
        );
        state.closing_snapshots.push(snapshot);
        state.config.animations.set_close_duration_msec(0);
        let mut manager = VisualAnimationPump::new();
        let mut dispatcher = Dispatcher::default();
        manager.advance_and_schedule(&mut state, &mut dispatcher, Instant::now());
        assert!(state.closing_snapshots.is_empty());
        assert_eq!(probe.get(), 1);
        assert!(queues_animation_frame(&dispatcher, output_id));
    }

    #[test]
    fn closing_completion_damages_every_intersecting_output() {
        let mut state = state_for_tests();
        let left = state.insert_output(crate::test_support::test_output_drm0_wh(100, 100));
        let right = state.insert_output(crate::test_support::test_output_drm1(100, 0, 100, 100));
        let (snapshot, probe) = crate::visual_animation::RetainedClosingSnapshot::test_pending(
            3,
            crate::test_support::test_resolved_wl_surface_client1(9),
            RectI::new(50, 0, 100, 40).unwrap(),
            DrmId::from(0usize),
        );
        state.closing_snapshots.push(snapshot);
        state.config.animations.set_close_duration_msec(100);
        let mut manager = VisualAnimationPump::new();
        let now = Instant::now();
        let mut dispatcher = Dispatcher::default();
        manager.advance_and_schedule_output(&mut state, &mut dispatcher, now, left);
        assert!(queues_animation_wakeup(&dispatcher, left));
        assert!(!queues_animation_frame(&dispatcher, left));
        assert!(!queues_animation_frame(&dispatcher, right));

        let mut dispatcher = Dispatcher::default();
        manager.advance_and_schedule_output(
            &mut state,
            &mut dispatcher,
            now + Duration::from_millis(100),
            left,
        );
        assert!(queues_animation_frame(&dispatcher, left));
        assert!(queues_animation_frame(&dispatcher, right));
        assert!(state.closing_snapshots.is_empty());
        assert_eq!(probe.get(), 1);
    }

    fn live_group(
        state: &mut crate::state::State,
        width: i32,
        height: i32,
        crtc: u32,
    ) -> (
        OutputId,
        crate::state::WorkspaceGroupId,
        crate::state::WorkspaceId,
        crate::state::WorkspaceId,
    ) {
        let backend = BackendOutputRef::new(DrmId::from(0usize));
        let output_id =
            state.insert_output(test_output_with_crtc_name(backend, crtc, 0, 0, width, height));
        let group_id = state.workspace_group_for_output(output_id).unwrap();
        let from = state.active_workspace_for_output(output_id).unwrap();
        let to = state.create_workspace(Some(group_id), Some(format!("ws-{crtc}")));
        (output_id, group_id, from, to)
    }

    #[test]
    fn workspace_switch_reverses_restarts_and_settles_on_zero_duration() {
        let mut state = state_for_tests();
        state.config.animations.set_workspace_duration_msec(200);
        let (output_id, group_id, first, second) = live_group(&mut state, 800, 600, 1);
        let third = state.create_workspace(Some(group_id), Some("3".to_string()));
        assert!(state.activate_workspace(second));
        let now = Instant::now();
        assert!(state
            .advance_visual_animations(now)
            .damaged_rects
            .is_empty());
        state.advance_visual_animations(now + Duration::from_millis(80));
        let progress = state
            .workspace_groups
            .try_get(group_id)
            .unwrap()
            .transition
            .unwrap()
            .progress();
        assert!(progress > crate::visual_animation::UnitValue::ZERO);
        assert!(state.activate_workspace(first));
        let reversed = state
            .workspace_groups
            .try_get(group_id)
            .unwrap()
            .transition
            .unwrap();
        assert_eq!(reversed.from_workspace(), first);
        assert_eq!(reversed.to_workspace(), second);
        assert_eq!(reversed.progress(), progress);
        assert!(state.activate_workspace(second));
        assert!(state.activate_workspace(third));
        let restarted = state
            .workspace_groups
            .try_get(group_id)
            .unwrap()
            .transition
            .unwrap();
        assert_eq!(restarted.from_workspace(), second);
        assert_eq!(restarted.to_workspace(), third);
        assert_eq!(restarted.progress(), crate::visual_animation::UnitValue::ZERO);

        state.config.animations.set_workspace_duration_msec(0);
        let result = state.advance_visual_animations(now + Duration::from_millis(90));
        assert!(state
            .workspace_groups
            .try_get(group_id)
            .unwrap()
            .transition
            .is_none());
        assert_eq!(result.completed_transition_outputs, vec![output_id]);
        assert!(state.workspace_is_active(third));
    }

    #[test]
    fn next_and_previous_actions_share_the_activation_path() {
        let mut state = state_for_tests();
        state.config.animations.set_workspace_duration_msec(0);
        let backend = BackendOutputRef::new(DrmId::from(0usize));
        let output_id = state.insert_output(test_output_with_crtc_name(backend, 1, 0, 0, 200, 100));
        let group_id = state.workspace_group_for_output(output_id).unwrap();
        let workspaces = state
            .workspace_groups
            .try_get(group_id)
            .unwrap()
            .workspaces
            .clone();
        assert_eq!(workspaces.len(), 4);

        let mut dispatcher = Dispatcher::default();
        crate::config::execute_action(
            &mut state,
            &mut dispatcher,
            crate::config::CompositorAction::WorkspaceNext,
        );
        assert!(state.workspace_is_active(workspaces[1]));
        assert!(dispatcher.test_has_queued_action::<crate::clients::ClientManagerAction>(
            |action| {
                matches!(action, crate::clients::ClientManagerAction::WorkspaceStateChanged)
            }
        ));

        let mut dispatcher = Dispatcher::default();
        crate::config::execute_action(
            &mut state,
            &mut dispatcher,
            crate::config::CompositorAction::WorkspacePrevious,
        );
        assert!(state.workspace_is_active(workspaces[0]));

        assert!(state.activate_workspace(workspaces[3]));
        assert_eq!(
            state.activate_adjacent_workspace(
                crate::visual_animation::WorkspaceTransitionDirection::Next
            ),
            Some(group_id)
        );
        assert!(state.workspace_is_active(workspaces[0]));
        assert_eq!(
            state.activate_adjacent_workspace(
                crate::visual_animation::WorkspaceTransitionDirection::Previous
            ),
            Some(group_id)
        );
        assert!(state.workspace_is_active(workspaces[3]));
        assert_eq!(
            state
                .workspace_groups
                .try_get(group_id)
                .unwrap()
                .workspaces
                .len(),
            4
        );
    }

    #[test]
    fn shared_progress_uses_each_output_width_and_pauses_without_presentation() {
        let mut state = state_for_tests();
        state.config.animations.set_workspace_duration_msec(200);
        let (wide, group_id, from, to) = live_group(&mut state, 800, 600, 1);
        let narrow = state.insert_output(test_output_with_crtc_name(
            BackendOutputRef::new(DrmId::from(0usize)),
            2,
            800,
            0,
            400,
            600,
        ));
        let narrow_group = state.workspace_group_for_output(narrow).unwrap();
        state
            .workspace_groups
            .try_get_mut(narrow_group)
            .unwrap()
            .outputs
            .remove(&narrow);
        state
            .workspace_groups
            .try_get_mut(group_id)
            .unwrap()
            .outputs
            .insert(narrow);
        state.output_workspace_groups.insert(narrow, group_id);
        assert!(state.activate_workspace(to));
        let now = Instant::now();
        state.advance_visual_animations(now);
        let progressed = state.advance_visual_animations(now + Duration::from_millis(100));
        assert!(progressed
            .damaged_rects
            .contains(&state.output_bounds(wide).unwrap()));
        assert!(progressed
            .damaged_rects
            .contains(&state.output_bounds(narrow).unwrap()));
        let wide_motion = state.workspace_motion_for_output(wide, Some(from));
        let narrow_motion = state.workspace_motion_for_output(narrow, Some(from));
        assert_eq!(wide_motion.alpha, narrow_motion.alpha);
        assert!(wide_motion.translation_x < narrow_motion.translation_x);
        assert!(wide_motion.translation_x < 0);
        assert!(narrow_motion.translation_x < 0);
        let incoming = state.workspace_motion_for_output(wide, Some(to));
        assert!(incoming.translation_x > 0);
        assert!(incoming.alpha > 0 && incoming.alpha < 255);

        let held = state
            .workspace_groups
            .try_get(group_id)
            .unwrap()
            .transition
            .unwrap()
            .progress();
        state.outputs.try_get_mut(narrow).unwrap().enabled = false;
        state.advance_visual_animations(now + Duration::from_millis(150));
        assert!(
            state
                .workspace_groups
                .try_get(group_id)
                .unwrap()
                .transition
                .unwrap()
                .progress()
                > held
        );

        state.session_active = false;
        let paused = state
            .workspace_groups
            .try_get(group_id)
            .unwrap()
            .transition
            .unwrap()
            .progress();
        state.advance_visual_animations(now + Duration::from_millis(400));
        assert_eq!(
            state
                .workspace_groups
                .try_get(group_id)
                .unwrap()
                .transition
                .unwrap()
                .progress(),
            paused
        );
        assert!(!state
            .workspace_groups
            .try_get(group_id)
            .unwrap()
            .transition
            .unwrap()
            .needs_frames());
    }

    #[test]
    fn workspace_fade_can_be_disabled_without_stopping_the_slide() {
        let mut state = state_for_tests();
        state.config.animations.set_workspace_duration_msec(200);
        state.config.animations.set_workspace_fade(false);
        let (output_id, _, from, to) = live_group(&mut state, 800, 600, 1);
        assert!(state.activate_workspace(to));
        let now = Instant::now();
        state.advance_visual_animations(now);
        state.advance_visual_animations(now + Duration::from_millis(100));
        let outgoing = state.workspace_motion_for_output(output_id, Some(from));
        let incoming = state.workspace_motion_for_output(output_id, Some(to));
        assert_eq!(outgoing.alpha, 255);
        assert_eq!(incoming.alpha, 255);
        assert!(outgoing.translation_x < 0);
        assert!(incoming.translation_x > 0);
    }

    #[test]
    fn workspace_sides_follow_their_own_clocks_and_styles() {
        let mut state = state_for_tests();
        state
            .config
            .animations
            .set_duration_msec(crate::animation_config::AnimationNode::WorkspacesIn, 100);
        state
            .config
            .animations
            .set_linear(crate::animation_config::AnimationNode::WorkspacesIn);
        state.config.animations.set_style(
            crate::animation_config::AnimationNode::WorkspacesIn,
            crate::animation_config::AnimationStyle::Slide { edge: None },
        );
        state
            .config
            .animations
            .set_duration_msec(crate::animation_config::AnimationNode::WorkspacesOut, 200);
        state
            .config
            .animations
            .set_linear(crate::animation_config::AnimationNode::WorkspacesOut);
        let (output_id, group_id, from, to) = live_group(&mut state, 800, 600, 1);
        assert!(state.activate_workspace(to));
        let now = Instant::now();
        state.advance_visual_animations(now);
        state.advance_visual_animations(now + Duration::from_millis(100));
        let outgoing = state.workspace_motion_for_output(output_id, Some(from));
        let incoming = state.workspace_motion_for_output(output_id, Some(to));
        assert_eq!(incoming.translation_x, 0);
        assert_eq!(incoming.alpha, 255);
        assert!(outgoing.translation_x < 0);
        assert!(outgoing.alpha > 0 && outgoing.alpha < 255);
        assert!(state
            .workspace_groups
            .try_get(group_id)
            .unwrap()
            .transition
            .is_some());

        state.advance_visual_animations(now + Duration::from_millis(200));
        assert!(state
            .workspace_groups
            .try_get(group_id)
            .unwrap()
            .transition
            .is_none());
    }

    #[test]
    fn completion_publishes_once_and_output_local_kicks_do_not_double_advance() {
        let mut state = state_for_tests();
        state.config.animations.set_workspace_duration_msec(100);
        let (output_id, group_id, _, second) = live_group(&mut state, 320, 200, 1);
        assert!(state.activate_workspace(second));
        let mut manager = VisualAnimationPump::new();
        let now = Instant::now();
        let mut dispatcher = Dispatcher::default();
        manager.advance_and_schedule(&mut state, &mut dispatcher, now);
        assert!(queues_animation_wakeup(&dispatcher, output_id));
        assert!(!dispatcher.test_has_queued_action::<crate::clients::ClientManagerAction>(
            |action| {
                matches!(action, crate::clients::ClientManagerAction::WorkspaceStateChanged)
            }
        ));
        assert_eq!(
            state
                .workspace_groups
                .try_get(group_id)
                .unwrap()
                .transition
                .unwrap()
                .progress(),
            crate::visual_animation::UnitValue::ZERO
        );
        assert!(state.output_has_live_workspace_transition(output_id));

        let mut dispatcher = Dispatcher::default();
        manager.advance_and_schedule_output(&mut state, &mut dispatcher, now, output_id);
        assert_eq!(
            state
                .workspace_groups
                .try_get(group_id)
                .unwrap()
                .transition
                .unwrap()
                .progress(),
            crate::visual_animation::UnitValue::ZERO
        );

        let mut dispatcher = Dispatcher::default();
        manager.advance_and_schedule(&mut state, &mut dispatcher, now + Duration::from_millis(100));
        assert!(state
            .workspace_groups
            .try_get(group_id)
            .unwrap()
            .transition
            .is_none());
        assert!(!state.output_has_live_workspace_transition(output_id));
        assert!(queues_animation_frame(&dispatcher, output_id));
        assert!(dispatcher.test_has_queued_action::<crate::clients::ClientManagerAction>(
            |action| {
                matches!(action, crate::clients::ClientManagerAction::WorkspaceStateChanged)
            }
        ));
        assert_eq!(
            crate::scene::visibility::output_workspace_ids(
                &state,
                output_id,
                crate::scene::visibility::WorkspaceParticipationPolicy::Steady,
            ),
            vec![second]
        );

        let mut dispatcher = Dispatcher::default();
        manager.advance_and_schedule(&mut state, &mut dispatcher, now + Duration::from_millis(100));
        assert!(!dispatcher.test_has_queued_action::<crate::clients::ClientManagerAction>(
            |action| {
                matches!(action, crate::clients::ClientManagerAction::WorkspaceStateChanged)
            }
        ));
    }

    #[test]
    fn removing_an_endpoint_drops_the_transition() {
        let mut state = state_for_tests();
        state.config.animations.set_workspace_duration_msec(200);
        let (_, group_id, first, second) = live_group(&mut state, 100, 100, 1);
        assert!(state.activate_workspace(second));
        assert!(state.remove_workspace(first));
        assert!(state
            .workspace_groups
            .try_get(group_id)
            .unwrap()
            .transition
            .is_none());
    }

    #[test]
    fn workspace_motion_moves_the_whole_root_recipe() {
        let bounds = RectI::new(10, 20, 40, 30).unwrap();
        let clip =
            crate::state::RootSurfaceClip::new(bounds, 0, crate::config::RoundingPower::DEFAULT);
        let surface_key = crate::clients::Resolved::new(
            ClientId::from_raw(1),
            WlSurfaceId::from(1u32),
            ObjectId::from(1usize),
        );
        let mut items = vec![super::SceneItem {
            state: super::SceneItemState {
                key: super::SceneItemKey::Surface(surface_key),
                surface_key: Some(surface_key),
                content_key: super::SceneItemContentKey::Buffer(crate::clients::Resolved::new(
                    ClientId::from_raw(1),
                    crate::protocol_generated::WlBufferId::from(2u32),
                    ObjectId::from(2usize),
                )),
                bounds,
                tex_transform: crate::transform::Mat3::identity(),
                alpha: 200,
                mask: super::SceneItemMask::RoundedClip { clip },
            },
            mask: super::SceneItemMask::RoundedClip { clip },
            texture: crate::render::inert_texture(),
            damage: crate::region::DamageRegion::from_rect(bounds),
            sample_damage_padding: 0,
            state_has_alpha: false,
            alpha: 200,
            opaque_region: crate::region::DamageRegion::from_rect(bounds),
            opaque: true,
            sync: crate::sync::SurfaceSyncState::default(),
            direct_scanout: None,
            presentation_feedbacks: Vec::new(),
        }];
        let mut elements = vec![super::RecordedFrameElement::Effect {
            phase: super::FramePhase::Overlay,
            element: super::EffectElement::DimOverlay { clip, alpha: 180 },
        }];
        super::apply_workspace_motion(
            &mut items,
            &mut elements,
            crate::visual_animation::WorkspaceMotion { translation_x: 25, alpha: 128 },
        );

        assert_eq!(items[0].state.bounds.x, 35);
        assert_eq!(items[0].alpha, super::scale_u8(200, 128));
        assert!(!items[0].opaque);
        assert!(items[0].opaque_region.is_empty());
        assert!(items[0].direct_scanout.is_none());
        assert!(items[0].state_has_alpha);
        match items[0].mask {
            super::SceneItemMask::RoundedClip { clip } => assert_eq!(clip.outer_rect.x, 35),
            _ => panic!("rounded clip should move with the workspace"),
        }
        match &elements[0] {
            super::RecordedFrameElement::Effect {
                element: super::EffectElement::DimOverlay { clip, alpha },
                ..
            } => {
                assert_eq!(clip.outer_rect.x, 35);
                assert_eq!(*alpha, super::scale_u8(180, 128));
            }
            _ => panic!("overlay should move and fade with the workspace"),
        }
    }

    #[test]
    fn a_live_workspace_transition_blocks_direct_scanout_for_its_roots() {
        let mut state = state_for_tests();
        state.config.animations.set_workspace_duration_msec(200);
        let (_, _, from, to) = live_group(&mut state, 200, 100, 1);
        assert!(state.activate_workspace(to));
        let (_, object_id) =
            state.insert_object(WlSurface::new(WlSurfaceId::from(70u32), ClientId::from_raw(1), 6));
        let surface = root(&mut state, object_id);
        state.register_root_surface(surface);
        state.assign_root_surface_to_workspace(surface, from);
        let (_, other_id) =
            state.insert_object(WlSurface::new(WlSurfaceId::from(71u32), ClientId::from_raw(1), 6));
        let other = root(&mut state, other_id);

        assert!(super::workspace_transition_blocks_scanout(&state, surface));
        assert!(!super::workspace_transition_blocks_scanout(&state, other));
    }

    #[test]
    fn animation_damage_reaches_every_intersecting_output_only() {
        let mut state = state_for_tests();
        let backend = BackendOutputRef::new(DrmId::from(0usize));
        let left = state.insert_output(test_output_with_crtc_name(backend, 1, 0, 0, 100, 100));
        let right = state.insert_output(test_output_with_crtc_name(backend, 2, 100, 0, 100, 100));
        let below = state.insert_output(test_output_with_crtc_name(backend, 3, 0, 200, 100, 100));
        let span = RectI::new(90, 0, 30, 20).unwrap();
        let local_only = RectI::new(10, 10, 20, 20).unwrap();

        let targets = super::animation_damage_targets(&state, &[span, local_only]);

        assert!(targets.contains(&(left, RectI::new(90, 0, 10, 20).unwrap())));
        assert!(targets.contains(&(right, RectI::new(0, 0, 20, 20).unwrap())));
        assert!(targets.contains(&(left, local_only)));
        assert!(targets.iter().all(|(output_id, _)| *output_id != below));
        assert!(targets
            .iter()
            .all(|(output_id, local)| { *output_id != right || *local != local_only }));
        super::queue_animation_damage(&mut state, &[span, local_only]);
    }
}

#[cfg(test)]
mod closing_capture_tests {
    use crate::{
        clients::ClientId,
        protocol_generated::{WlBufferId, WlSurfaceId, XdgPopupId, XdgSurfaceId},
        region::RectI,
        render::{BlurVisualStyle, EffectElement, RoundedRing},
        state::{DrmId, ObjectId, RootSurfaceClip},
        test_support::{
            insert_test_shm_buffer, state_for_tests, test_output_drm0_wh, test_output_drm1,
            test_resolved_wl_surface_client1,
        },
        wayland_objects::{
            wl_surface::WlSurface,
            xdg_popup::XdgPopup,
            xdg_positioner::PositionerState,
            xdg_surface::{xdg_toplevel::ToplevelDecorationMode, XdgSurface},
        },
    };

    use super::{
        append_closing_snapshots, closing_snapshots_precede_tier, plan_closing_capture,
        CompositionTier, FramePhase, RecordedFrameElement, SceneItemKey,
    };

    fn mapped_root(
        state: &mut crate::state::State,
        x: i32,
        y: i32,
    ) -> crate::clients::Resolved<WlSurfaceId> {
        let (_, object_id) =
            state.insert_object(WlSurface::new(WlSurfaceId::from(40u32), ClientId::from_raw(1), 6));
        let buffer =
            insert_test_shm_buffer(state, ClientId::from_raw(1), WlBufferId::from(41), 20, 10);
        state
            .objects
            .get_mut(object_id)
            .downcast_mut::<WlSurface>()
            .unwrap()
            .test_map(buffer, x, y, 80, 60);
        let surface = crate::test_support::resolved_wl_surface(state, object_id);
        state.register_root_surface(surface);
        surface
    }

    #[test]
    fn capture_plan_skips_when_no_output_or_backends_differ() {
        let mut state = state_for_tests();
        let surface = mapped_root(&mut state, 0, 0);
        assert!(plan_closing_capture(&state, surface).is_none());

        state.insert_output(test_output_drm0_wh(100, 100));
        state.insert_output(test_output_drm1(100, 0, 100, 100));
        let buffer =
            insert_test_shm_buffer(&mut state, ClientId::from_raw(1), WlBufferId::from(42), 4, 4);
        state
            .objects
            .get_mut(surface.object_id())
            .downcast_mut::<WlSurface>()
            .unwrap()
            .test_map(buffer, 80, 0, 40, 40);
        assert!(plan_closing_capture(&state, surface).is_none());
    }

    #[test]
    fn flattened_plan_localizes_subsurfaces_and_popups_without_taking_live_state() {
        let mut state = state_for_tests();
        let output_id = state.insert_output(test_output_drm0_wh(800, 600));
        state.config.decorations.border.size = 4;
        state.config.decorations.shadow.radius = 16;
        state.config.effects.blur.enabled = true;
        let parent = mapped_root(&mut state, 100, 80);
        let (_, xdg_object) = state.insert_object(XdgSurface::new(
            XdgSurfaceId::from(50),
            crate::clients::Resolved::new(
                ClientId::from_raw(1),
                crate::protocol_generated::XdgWmBaseId::from(1),
                ObjectId::from(1usize),
            ),
            parent,
            7,
        ));
        let xdg_resolved = crate::clients::Resolved::new(
            ClientId::from_raw(1),
            XdgSurfaceId::from(50),
            xdg_object,
        );
        state
            .objects
            .get_mut(parent.object_id())
            .downcast_mut::<WlSurface>()
            .unwrap()
            .set_xdg_surface_impl(xdg_resolved);
        let (_, toplevel_object) = state.insert_object(
            crate::wayland_objects::xdg_surface::xdg_toplevel::XdgToplevel::new(
                crate::protocol_generated::XdgToplevelId::from(51),
                xdg_resolved,
                7,
            ),
        );
        let toplevel = crate::clients::Resolved::new(
            ClientId::from_raw(1),
            crate::protocol_generated::XdgToplevelId::from(51),
            toplevel_object,
        );
        state.register_xdg_toplevel_role(xdg_resolved, toplevel);
        state
            .objects
            .get_mut(toplevel_object)
            .downcast_mut::<crate::wayland_objects::xdg_surface::xdg_toplevel::XdgToplevel>()
            .unwrap()
            .set_decoration_mode(ToplevelDecorationMode::ServerSide);
        let workspace = state.active_workspace_for_output(output_id).unwrap();
        state.assign_root_surface_to_workspace(parent, workspace);
        state.managed_root_surfaces.get_mut(&parent).unwrap().mapped = true;
        state.mark_root_geometry_dirty(parent);

        let (_, child_object) =
            state.insert_object(WlSurface::new(WlSurfaceId::from(60), ClientId::from_raw(1), 6));
        let child_buffer =
            insert_test_shm_buffer(&mut state, ClientId::from_raw(1), WlBufferId::from(61), 8, 8);
        let child = crate::test_support::resolved_wl_surface(&state, child_object);
        state
            .objects
            .get_mut(child_object)
            .downcast_mut::<WlSurface>()
            .unwrap()
            .test_map(child_buffer, 0, 0, 24, 16);
        state
            .objects
            .get_mut(child_object)
            .downcast_mut::<WlSurface>()
            .unwrap()
            .test_mark_subsurface_at(parent, (30, 40));
        state
            .objects
            .get_mut(parent.object_id())
            .downcast_mut::<WlSurface>()
            .unwrap()
            .test_push_current_child(child);

        let (popup_wl, _) = {
            let (_, object_id) = state.insert_object(WlSurface::new(
                WlSurfaceId::from(70),
                ClientId::from_raw(1),
                6,
            ));
            let buffer = insert_test_shm_buffer(
                &mut state,
                ClientId::from_raw(1),
                WlBufferId::from(71),
                8,
                8,
            );
            state
                .objects
                .get_mut(object_id)
                .downcast_mut::<WlSurface>()
                .unwrap()
                .test_map(buffer, 0, 0, 30, 20);
            (crate::test_support::resolved_wl_surface(&state, object_id), object_id)
        };
        let (_, popup_xdg_object) = state.insert_object(XdgSurface::new(
            XdgSurfaceId::from(72),
            crate::clients::Resolved::new(
                ClientId::from_raw(1),
                crate::protocol_generated::XdgWmBaseId::from(1),
                ObjectId::from(1usize),
            ),
            popup_wl,
            7,
        ));
        let popup_xdg = crate::clients::Resolved::new(
            ClientId::from_raw(1),
            XdgSurfaceId::from(72),
            popup_xdg_object,
        );
        state
            .objects
            .get_mut(popup_wl.object_id())
            .downcast_mut::<WlSurface>()
            .unwrap()
            .set_xdg_surface_impl(popup_xdg);
        let (_, popup_object) = state.insert_object(XdgPopup::new(
            XdgPopupId::from(73),
            popup_xdg,
            Some(xdg_resolved),
            PositionerState::default(),
            7,
        ));
        let popup = crate::clients::Resolved::new(
            ClientId::from_raw(1),
            XdgPopupId::from(73),
            popup_object,
        );
        state
            .objects
            .get_mut(popup_object)
            .downcast_mut::<XdgPopup>()
            .unwrap()
            .apply_acked_configure(RectI::new(15, 25, 30, 20).unwrap());
        state
            .objects
            .get_mut(popup_xdg.object_id())
            .downcast_mut::<XdgSurface>()
            .unwrap()
            .test_set_current_window_geometry(Some(RectI::new(0, 0, 30, 20).unwrap()));
        state
            .objects
            .get_mut(popup_xdg.object_id())
            .downcast_mut::<XdgSurface>()
            .unwrap()
            .test_set_popup_role(popup);
        state
            .objects
            .get_mut(xdg_resolved.object_id())
            .downcast_mut::<XdgSurface>()
            .unwrap()
            .add_popup_child(popup);

        state
            .objects
            .get_mut(parent.object_id())
            .downcast_mut::<WlSurface>()
            .unwrap()
            .test_push_frame_callback_and_feedback();
        let before = state
            .objects
            .try_ref_object::<WlSurface>(parent.object_id())
            .unwrap()
            .render_state_at(0, 0)
            .unwrap();

        let plan = plan_closing_capture(&state, parent).unwrap();
        let parent_part = plan
            .surfaces
            .iter()
            .find(|surface| surface.width == 80)
            .unwrap();
        let child_part = plan
            .surfaces
            .iter()
            .find(|surface| surface.width == 24)
            .unwrap();
        let popup_part = plan
            .surfaces
            .iter()
            .find(|surface| surface.width == 30)
            .unwrap();
        assert_eq!(child_part.x - parent_part.x, 30);
        assert_eq!(child_part.y - parent_part.y, 40);
        assert_eq!(popup_part.x - parent_part.x, 15);
        assert_eq!(popup_part.y - parent_part.y, 25);
        assert!(plan.blur.is_some());
        assert!(plan
            .effects
            .iter()
            .all(|effect| { !matches!(effect.element, EffectElement::WindowBlurBackdrop { .. }) }));
        assert!(plan
            .effects
            .iter()
            .any(|effect| matches!(effect.element, EffectElement::WindowShadow { .. })));
        assert!(plan
            .effects
            .iter()
            .any(|effect| matches!(effect.element, EffectElement::WindowBorder { .. })));
        assert!(plan.effects.iter().any(|effect| {
            effect.phase == FramePhase::Overlay
                && matches!(effect.element, EffectElement::DimOverlay { .. })
        }));
        let after = state
            .objects
            .try_ref_object::<WlSurface>(parent.object_id())
            .unwrap();
        assert_eq!(after.test_frame_callback_count(), 1);
        assert_eq!(after.test_presentation_feedback_count(), 1);
        let after_render = after.render_state_at(0, 0).unwrap();
        assert_eq!(after_render.damage.rects().len(), before.damage.rects().len());
        assert!(!after_render.damage.is_empty());
        assert!(after_render.presentation_feedbacks.is_empty());
    }

    #[test]
    fn closing_snapshots_render_above_content_without_protocol_participation() {
        assert!(!closing_snapshots_precede_tier(CompositionTier::Background));
        assert!(!closing_snapshots_precede_tier(CompositionTier::Bottom));
        assert!(!closing_snapshots_precede_tier(CompositionTier::Content));
        assert!(closing_snapshots_precede_tier(CompositionTier::Top));
        assert!(closing_snapshots_precede_tier(CompositionTier::Overlay));

        let mut state = state_for_tests();
        let clip = RootSurfaceClip::new(
            RectI::new(0, 0, 40, 20).unwrap(),
            0,
            crate::config::RoundingPower::DEFAULT,
        );
        let snapshot = crate::visual_animation::RetainedClosingSnapshot::new(
            11,
            test_resolved_wl_surface_client1(3),
            None,
            RectI::new(5, 6, 40, 20).unwrap(),
            Some(crate::visual_animation::FrozenClosingBlur {
                ring: RoundedRing { outer: clip, inner: clip },
                style: BlurVisualStyle { radius: 4, passes: 1 },
                alpha: 220,
            }),
            DrmId::from(0usize),
            crate::render::inert_texture(),
        );
        state.closing_snapshots.push(snapshot);
        let mut items = Vec::new();
        let mut elements = Vec::new();
        append_closing_snapshots(
            &state,
            RectI::new(0, 0, 200, 200).unwrap(),
            None,
            crate::visual_animation::WorkspaceMotion::IDENTITY,
            &mut items,
            &mut elements,
        );
        assert!(matches!(
            elements[0],
            RecordedFrameElement::Effect {
                element: EffectElement::WindowBlurBackdrop { alpha, .. },
                ..
            } if alpha == 220
        ));
        assert!(matches!(elements[1], RecordedFrameElement::Texture { item_index: 0, .. }));
        assert!(matches!(items[0].state.key, SceneItemKey::ClosingSnapshot(11)));
        assert!(items[0].state.surface_key.is_none());
        assert!(items[0].presentation_feedbacks.is_empty());
        assert!(items[0].direct_scanout.is_none());
        assert!(!items[0].opaque);
        assert!(state.root_surfaces.is_empty());
    }

    #[test]
    fn closing_snapshots_follow_their_workspace_under_incoming_content() {
        let mut state = state_for_tests();
        state.config.animations.set_workspace_duration_msec(200);
        let output_id = state.insert_output(test_output_drm0_wh(800, 600));
        let group_id = state.workspace_group_for_output(output_id).unwrap();
        let outgoing = state.active_workspace_for_output(output_id).unwrap();
        let incoming = state.create_workspace(Some(group_id), Some("2".to_string()));
        assert!(state.activate_workspace(incoming));
        let now = std::time::Instant::now();
        state.advance_visual_animations(now);
        state.advance_visual_animations(now + std::time::Duration::from_millis(100));
        let outgoing_motion = state.workspace_motion_for_output(output_id, Some(outgoing));
        let incoming_motion = state.workspace_motion_for_output(output_id, Some(incoming));
        for (id, workspace_id) in [(21u64, Some(outgoing)), (22, Some(incoming)), (23, None)] {
            state
                .closing_snapshots
                .push(crate::visual_animation::RetainedClosingSnapshot::new(
                    id,
                    test_resolved_wl_surface_client1(id as usize),
                    workspace_id,
                    RectI::new(200, 8, 400, 16).unwrap(),
                    None,
                    DrmId::from(0usize),
                    crate::render::inert_texture(),
                ));
        }
        let mut items = Vec::new();
        let mut elements = Vec::new();
        super::render_workspace_content(
            &mut state,
            DrmId::from(0usize),
            0,
            output_id,
            RectI::new(0, 0, 800, 600).unwrap(),
            &[],
            &mut items,
            &mut elements,
            &mut Vec::new(),
        );

        assert!(
            state
                .workspace_groups
                .try_get(group_id)
                .unwrap()
                .transition
                .unwrap()
                .progress()
                > crate::visual_animation::UnitValue::ZERO,
            "outgoing {outgoing_motion:?} incoming {incoming_motion:?}"
        );
        assert_eq!(items.len(), 3);
        assert!(matches!(items[0].state.key, SceneItemKey::ClosingSnapshot(21)));
        assert!(matches!(items[1].state.key, SceneItemKey::ClosingSnapshot(22)));
        assert!(matches!(items[2].state.key, SceneItemKey::ClosingSnapshot(23)));
        assert_eq!(items[0].state.bounds.x, 200 + outgoing_motion.translation_x);
        assert_eq!(items[1].state.bounds.x, 200 + incoming_motion.translation_x);
        assert_eq!(items[2].state.bounds.x, 200);
        assert_eq!(items[0].alpha, super::scale_u8(255, outgoing_motion.alpha));
        assert_eq!(items[1].alpha, super::scale_u8(255, incoming_motion.alpha));
        assert!(items[0].alpha > 0 && items[0].alpha < 255);
        assert!(items[1].alpha > 0 && items[1].alpha < 255);
        assert!(outgoing_motion.translation_x < 0);
        assert!(incoming_motion.translation_x > 0);
    }

    #[test]
    fn closing_pose_is_translated_with_the_workspace_and_then_released() {
        let mut state = state_for_tests();
        state.config.animations.set_workspace_duration_msec(200);
        state.config.animations.set_close_duration_msec(100);
        state
            .config
            .animations
            .set_linear(crate::animation_config::AnimationNode::WindowsOut);
        let output_id = state.insert_output(test_output_drm0_wh(800, 600));
        let group_id = state.workspace_group_for_output(output_id).unwrap();
        let outgoing = state.active_workspace_for_output(output_id).unwrap();
        let incoming = state.create_workspace(Some(group_id), Some("2".to_string()));
        assert!(state.activate_workspace(incoming));
        let now = std::time::Instant::now();
        state.advance_visual_animations(now);
        let captured = RectI::new(200, 8, 400, 16).unwrap();
        state
            .closing_snapshots
            .push(crate::visual_animation::RetainedClosingSnapshot::new(
                31,
                test_resolved_wl_surface_client1(31),
                Some(outgoing),
                captured,
                None,
                DrmId::from(0usize),
                crate::render::inert_texture(),
            ));
        state.advance_visual_animations(now + std::time::Duration::from_millis(50));
        assert_eq!(state.closing_snapshots[0].visual_bounds(), captured);
        state.advance_visual_animations(now + std::time::Duration::from_millis(100));
        let visual = state.closing_snapshots[0].visual_bounds();
        assert_ne!(visual, captured);
        let motion = state.workspace_motion_for_output(output_id, Some(outgoing));
        assert!(motion.translation_x < 0);

        let mut items = Vec::new();
        let mut elements = Vec::new();
        super::render_workspace_content(
            &mut state,
            DrmId::from(0usize),
            0,
            output_id,
            RectI::new(0, 0, 800, 600).unwrap(),
            &[],
            &mut items,
            &mut elements,
            &mut Vec::new(),
        );
        let drawn = items
            .iter()
            .find(|item| matches!(item.state.key, SceneItemKey::ClosingSnapshot(31)))
            .expect("posed snapshot is drawn with its workspace");
        assert_eq!(drawn.state.bounds.x, visual.x + motion.translation_x);
        assert_eq!(drawn.state.bounds.y, visual.y);

        state.advance_visual_animations(now + std::time::Duration::from_millis(150));
        assert!(state.closing_snapshots.is_empty());
    }

    #[test]
    fn begin_skips_unsupported_windows_and_deduplicates() {
        let mut state = state_for_tests();
        state.realize_closing_snapshots_in_tests = true;
        state.insert_output(test_output_drm0_wh(400, 300));
        let surface = mapped_root(&mut state, 10, 10);
        let workspace = state
            .active_workspace_for_output(state.outputs.iter().next().unwrap().0)
            .unwrap();
        state.assign_root_surface_to_workspace(surface, workspace);
        state
            .managed_root_surfaces
            .get_mut(&surface)
            .unwrap()
            .mapped = true;
        let mut dispatcher = crate::runner::Dispatcher::default();
        super::begin_managed_toplevel_close_id(&mut state, surface, None, &mut dispatcher);
        assert!(state.closing_snapshots.is_empty());

        state.session_active = false;
        super::begin_managed_toplevel_close_id(&mut state, surface, None, &mut dispatcher);
        assert!(state.closing_snapshots.is_empty());
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
                WindowClip,
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
                alpha: u8::MAX,
                mask: SceneItemMask::RoundedClip { clip },
            },
            mask: SceneItemMask::RoundedClip { clip },
            texture: crate::render::inert_texture(),
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
            crate::render::gles::RenderItemMask::RoundedClip {
                clip: RootSurfaceClip::new(
                    RectI::new(-6, -12, 130, 96).unwrap(),
                    18,
                    crate::config::RoundingPower::DEFAULT,
                ),
            }
        );
    }

    #[test]
    fn scene_items_to_texture_elements_map_corner_clip_to_local_coordinates() {
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
                alpha: u8::MAX,
                mask: SceneItemMask::CornerRoundedClip { clip },
            },
            mask: SceneItemMask::CornerRoundedClip { clip },
            texture: crate::render::inert_texture(),
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
            crate::render::gles::RenderItemMask::CornerRoundedClip {
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
            clip: Some(WindowClip::Frame(clip)),
            frame_clip: clip,
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
                    alpha: u8::MAX,
                    mask: SceneItemMask::None,
                },
                mask: SceneItemMask::None,
                texture: crate::render::inert_texture(),
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
                    alpha: u8::MAX,
                    mask: SceneItemMask::None,
                },
                mask: SceneItemMask::None,
                texture: crate::render::inert_texture(),
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
                    ring: rounded_ring(
                        clip.outer_rect,
                        RectI::new(14, 22, 72, 54).unwrap(),
                        10,
                        0,
                        clip.rounding_power,
                    ),
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
                    ring: rounded_ring(
                        clip.outer_rect,
                        RectI::new(314, 222, 84, 66).unwrap(),
                        10,
                        0,
                        clip.rounding_power,
                    ),
                    style: BlurVisualStyle { radius: 12, passes: 2 },
                    alpha: 180,
                },
            },
            RecordedFrameElement::Effect {
                phase: FramePhase::OverWindow,
                element: EffectElement::WindowBorder {
                    ring: rounded_ring(
                        clip.outer_rect,
                        RectI::new(314, 222, 84, 66).unwrap(),
                        10,
                        0,
                        clip.rounding_power,
                    ),
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
                alpha: u8::MAX,
                mask: SceneItemMask::None,
            },
            mask: SceneItemMask::None,
            texture: crate::render::inert_texture(),
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
                        RectI::new(308, 216, 96, 78).unwrap(),
                        RectI::new(314, 222, 84, 66).unwrap(),
                        10,
                        0,
                        crate::config::RoundingPower::DEFAULT,
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
            render_planner::select_presentation_output_for_scene, request_output,
            request_outputs_for_rects, rgba_to_drm_argb8888,
            scene_item_requires_compositor_compositing, select_cursor_upload_slot,
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
                alpha: u8::MAX,
                mask: SceneItemMask::None,
            },
            mask: SceneItemMask::None,
            texture: crate::render::inert_texture(),
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
                protected: false,
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
    fn submission_metadata_includes_only_rendered_items() {
        let rendered = make_surface_scene_item(
            3,
            RectI::new(0, 0, 10, 10).unwrap(),
            vec![surface_feedback(8, 1)],
        );
        let omitted = make_surface_scene_item(
            4,
            RectI::new(20, 0, 10, 10).unwrap(),
            vec![surface_feedback(9, 1)],
        );

        let (scene, callback_surfaces, presentation_feedbacks) =
            collect_submission_metadata(&[rendered], &[]);

        assert_eq!(scene.len(), 1);
        assert_eq!(callback_surfaces.len(), 1);
        assert_eq!(callback_surfaces[0].wayland_id(), WlSurfaceId::from(3));
        assert_eq!(presentation_feedbacks.len(), 1);
        assert_ne!(omitted.state.surface_key, scene[0].surface_key);
        assert!(presentation_feedbacks
            .iter()
            .all(|feedback| feedback.surface == scene[0].surface_key));
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
            alpha: u8::MAX,
            mask: SceneItemMask::None,
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
            alpha: u8::MAX,
            mask: SceneItemMask::None,
        }];

        let selected = select_presentation_output_for_scene(&state, drm_id, &scene);

        assert_eq!(selected, Some(left));
        assert_ne!(selected, Some(right));
    }

    #[test]
    fn request_output_targets_logical_output_model_id() {
        let mut state = state_for_tests();
        let drm_id = DrmId::from(0usize);
        let backend = BackendOutputRef::new(drm_id);
        let left = state.insert_output(test_output(backend, 1, 0, 0, 100, 100, ""));
        let right = state.insert_output(test_output(backend, 2, 100, 0, 100, 100, ""));
        let mut dispatcher = Dispatcher::default();

        request_output(&state, right, &mut dispatcher);

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
    fn request_outputs_for_rects_marks_old_and_new_outputs() {
        let mut state = state_for_tests();
        let drm_id = DrmId::from(0usize);
        let backend = BackendOutputRef::new(drm_id);
        let left = state.insert_output(test_output(backend, 1, 0, 0, 100, 100, ""));
        let right = state.insert_output(test_output(backend, 2, 100, 0, 100, 100, ""));
        let mut dispatcher = Dispatcher::default();

        request_outputs_for_rects(
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
            Some(crate::render::WindowClip::Frame(crate::state::RootSurfaceClip::new(
                RectI::new(0, 0, 100, 100).unwrap(),
                8,
                crate::config::RoundingPower::DEFAULT,
            ))),
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

#[cfg(test)]
mod output_disable_tests {
    #[test]
    fn a_failed_disable_still_attempts_every_output() {
        let mut attempted = Vec::new();
        super::apply_output_disables(&[4, 9, 2], |crtc_id| {
            attempted.push(crtc_id);
            Err(format!("forced failure {crtc_id}"))
        });
        assert_eq!(attempted, [4, 9, 2]);
    }
}
