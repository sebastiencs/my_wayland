use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque},
    num::NonZeroU32,
    os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd, RawFd},
    rc::Rc,
    time::Duration,
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
    nix::sys::timerfd::{self, Expiration, TimerFd, TimerFlags, TimerSetTimeFlags},
    once_cell::sync::Lazy,
};

use crate::{
    cursor::{CursorImage, CursorImageContentKey, CursorSourceRef},
    dmabuf::DmaBuf,
    egl::{EglContext, EglFrameBuffer, GlTexture, RenderItem, EGL},
    gbm::{BufferObject, UseFlags},
    io_uring::{
        model::IoUringAction,
        ring::{PollCompleted, ReadWriteCompleted, RingId},
    },
    region::{DamageRegion, RectI},
    runner::{Dispatcher, Model, ModelInstanceId},
    state::{
        BackendOutputRef, ColorTransformMatrix, DrmId, ObjectId, OutputId, OutputMode,
        SsdFrameLayout, State,
    },
    sync::{DeferredRelease, RetiredBufferCandidate},
    transform::{self, OutputPresentationTransform},
    wayland_objects::{
        wl_buffer::{CursorShmBuffer, WlBuffer},
        wl_surface::{StackEntry, SurfacePresentationFeedback, WlSurface},
        zwlr_layer_surface_v1::ordered_scene_root_surface_ids,
    },
    ENABLE_DAMAGE,
};

use crate::{
    clients::{ClientAction, ClientId, ClientManagerAction},
    protocol_generated::{WlSurfaceId, WpPresentationFeedbackId},
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
    buffers: [RenderBuffer; 2],
    displayed_buffer_index: usize,
    render_buffer_index: usize,
    needs_frame: bool,
    frame_scheduled: bool,
    rendering_active: bool,
    page_flip_pending: bool,
    in_flight_releases: Vec<DeferredRelease>,
    present_count: u64,
    damage_history: VecDeque<DamageRegion>,
    last_scene: Vec<SceneItemState>,
    force_full_redraw: bool,
    rendered_buffer_present_id: [Option<u64>; 2],
    in_flight_frame: Option<SubmittedFrame>,
    displayed_external_fb: Option<DrmFrameBuffer>,
    retiring_external_fb: Option<DrmFrameBuffer>,
    displayed_cursor_slot: Option<usize>,
    retiring_cursor_slot: Option<usize>,
    cursor_slots: [Option<CachedCursorPlaneSlot>; 2],
    cursor_plane_state: CursorPlaneState,
    cursor_animation_delay_msec: Option<u32>,
    allow_in_fence_fd: bool,
    allow_fb_damage_clips: bool,
    next_submission_id: u64,
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

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
struct SurfaceKey {
    client_id: ClientId,
    surface_id: WlSurfaceId,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
enum SceneItemKey {
    Surface(SurfaceKey),
    SsdFrame { root_surface_object_id: ObjectId, segment: SsdFrameSegment },
    Cursor,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
enum SsdFrameSegment {
    Top,
    Left,
    Right,
    Bottom,
}

#[derive(Clone, Debug)]
struct SceneItemState {
    key: SceneItemKey,
    surface_key: Option<SurfaceKey>,
    buffer_object_id: ObjectId,
    bounds: RectI,
    tex_transform: transform::Mat3,
}

#[derive(Debug)]
struct SceneItem {
    state: SceneItemState,
    texture: gl::Uint,
    owned_texture: Option<GlTexture>,
    damage: DamageRegion,
    sample_damage_padding: i32,
    state_has_alpha: bool,
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
    surface_object_id: ObjectId,
    buffer_object_id: ObjectId,
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
    scene: Vec<SceneItemState>,
    frame_callback_surfaces: Vec<SurfaceKey>,
    presentation_feedbacks: Vec<SubmissionPresentationFeedback>,
    damage: DamageRegion,
    direct_scanout: Option<DirectScanoutCandidate>,
    cursor_plane: Option<CursorPlaneCandidate>,
    preserve_contents: bool,
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
struct OutputSubmission {
    id: u64,
    output_id: Option<OutputId>,
    scene: Vec<SceneItemState>,
    frame_callback_surfaces: Vec<SurfaceKey>,
    presentation_feedbacks: Vec<SubmissionPresentationFeedback>,
    zero_copy: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct SubmissionPresentationFeedback {
    client_id: ClientId,
    feedback_id: WpPresentationFeedbackId,
    version: u32,
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

impl DrmOutput {
    fn output_name(&self) -> &str {
        &self.in_use.connector.interface_name
    }

    fn output_bounds(&self) -> RectI {
        let framebuffer = &self.render_buffer().gl_framebuffer;
        RectI::new(0, 0, framebuffer.width, framebuffer.height)
            .expect("framebuffer dimensions must be non-negative")
    }

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
        self.force_full_redraw = true;
        self.in_flight_frame = None;
    }

    fn mark_frame_idle(&mut self) {
        self.rendering_active = false;
        self.needs_frame = false;
    }

    fn allocate_submission(
        &mut self,
        output_id: Option<OutputId>,
        scene: Vec<SceneItemState>,
        frame_callback_surfaces: Vec<SurfaceKey>,
        presentation_feedbacks: Vec<SubmissionPresentationFeedback>,
        zero_copy: bool,
    ) -> OutputSubmission {
        let id = self.next_submission_id;
        self.next_submission_id = self.next_submission_id.saturating_add(1);
        OutputSubmission {
            id,
            output_id,
            scene,
            frame_callback_surfaces,
            presentation_feedbacks,
            zero_copy,
        }
    }

    fn mark_frame_submitted(&mut self, frame: SubmittedFrame) {
        self.rendering_active = false;
        self.page_flip_pending = true;
        self.needs_frame = false;
        self.in_flight_frame = Some(frame);
        self.retiring_external_fb = self.displayed_external_fb.take();
        self.retiring_cursor_slot = self.displayed_cursor_slot.take();
    }

    fn take_in_flight_presentation_feedbacks_for_discard(
        &mut self,
    ) -> (Vec<SubmissionPresentationFeedback>, Option<u64>) {
        let Some(in_flight) = self.in_flight_frame.take() else {
            return (Vec::new(), None);
        };
        submitted_frame_feedbacks_for_discard(in_flight)
    }

    fn finish_page_flip(&mut self) -> (bool, Option<CompletedSubmission>) {
        if !self.page_flip_pending {
            return (false, None);
        }

        self.page_flip_pending = false;
        self.present_count = self.present_count.saturating_add(1);
        let mut completed_submission = None;

        match self.in_flight_frame.take() {
            Some(SubmittedFrame::Composited {
                buffer_index,
                damage,
                submission,
                damage_blob,
                cursor_slot,
            }) => {
                self.displayed_buffer_index = buffer_index;
                self.render_buffer_index = if buffer_index == 0 { 1 } else { 0 };
                self.rendered_buffer_present_id[buffer_index] = Some(self.present_count);
                self.damage_history.push_front(damage);
                while self.damage_history.len() > 4 {
                    self.damage_history.pop_back();
                }
                self.last_scene = submission.scene.clone();
                self.displayed_external_fb = None;
                self.displayed_cursor_slot = cursor_slot;
                self.force_full_redraw = false;
                completed_submission = Some(CompletedSubmission { submission, damage_blob });
            }
            Some(SubmittedFrame::DirectScanout { submission, framebuffer, cursor_slot }) => {
                self.force_full_redraw = true;
                self.last_scene = submission.scene.clone();
                self.displayed_external_fb = Some(framebuffer);
                self.displayed_cursor_slot = cursor_slot;
                self.rendered_buffer_present_id = [None, None];
                self.damage_history.clear();
                completed_submission = Some(CompletedSubmission { submission, damage_blob: None });
            }
            None => {}
        }
        self.retiring_external_fb = None;
        self.retiring_cursor_slot = None;

        if self.needs_frame && !self.frame_scheduled {
            self.frame_scheduled = true;
            return (true, completed_submission);
        }

        (false, completed_submission)
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

    fn buffer_age(&self) -> Option<usize> {
        let last_presented = self.rendered_buffer_present_id[self.render_buffer_index]?;
        let age = self
            .present_count
            .checked_sub(last_presented)?
            .saturating_add(1);
        usize::try_from(age).ok()
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
            self.displayed_cursor_slot,
            std::array::from_fn(|index| self.cursor_slot(index).is_some()),
        )
    }

    fn has_displayed_cursor_slot(&self) -> bool {
        self.displayed_cursor_slot.is_some()
    }

    fn preserve_render_buffer_contents(&self) -> bool {
        ENABLE_DAMAGE && self.buffer_age().is_some() && !self.force_full_redraw
    }

    fn accumulate_damage_from_age(&self, base_damage: &DamageRegion) -> DamageRegion {
        let mut damage = base_damage.clone();
        if !ENABLE_DAMAGE {
            damage.add_rect(self.output_bounds());
            return damage;
        }
        let Some(age) = self.buffer_age() else {
            damage.add_rect(self.output_bounds());
            return damage;
        };
        if age == 0 || self.force_full_redraw {
            damage.add_rect(self.output_bounds());
            return damage;
        }
        if age.saturating_sub(1) > self.damage_history.len() {
            damage.add_rect(self.output_bounds());
            return damage;
        }
        for previous in self.damage_history.iter().take(age.saturating_sub(1)) {
            damage.add_region(previous);
        }
        damage
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
        items: &[RenderItem],
        damage: &DamageRegion,
        preserve_contents: bool,
    ) -> crate::egl::RenderCompletion {
        let output = self
            .output(drm_crtc_id)
            .expect("missing DRM output pipeline for render");
        let projection = output.render_projection();
        output.render_buffer().gl_framebuffer.render(
            items,
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
            slot
        } else {
            let slot = self.output(drm_crtc_id)?.preferred_upload_cursor_slot()?;
            self.upload_cursor_plane_slot(drm_crtc_id, slot, request)?;
            let cached = self.output(drm_crtc_id)?.cursor_slot(slot).unwrap();
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
    ) -> std::io::Result<Vec<SurfaceKey>> {
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

            let commit_result = self.fd.atomic_commit(
                control::AtomicCommitFlags::PAGE_FLIP_EVENT | control::AtomicCommitFlags::NONBLOCK,
                changes,
            );
            match commit_result {
                Ok(()) => {
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
                    let buffer_index = self.output(drm_crtc_id).unwrap().render_buffer_index;
                    let submission = self.output_mut(drm_crtc_id).unwrap().allocate_submission(
                        output_id,
                        scene,
                        frame_callback_surfaces,
                        presentation_feedbacks,
                        false,
                    );
                    let callback_surfaces = submission.frame_callback_surfaces.clone();
                    self.output_mut(drm_crtc_id).unwrap().mark_frame_submitted(
                        SubmittedFrame::Composited {
                            buffer_index,
                            damage: damage.clone(),
                            submission,
                            damage_blob,
                            cursor_slot: cursor_plane.map(|cursor| cursor.slot),
                        },
                    );
                    return Ok(callback_surfaces);
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
    ) -> std::io::Result<Vec<SurfaceKey>> {
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
        if let Err(error) = self.fd.atomic_commit(
            control::AtomicCommitFlags::PAGE_FLIP_EVENT | control::AtomicCommitFlags::NONBLOCK,
            changes,
        ) {
            self.destroy_pending_color_blobs(color_blobs);
            return Err(error);
        }
        self.finish_output_color_commit(drm_crtc_id, color_blobs);
        let submission = self.output_mut(drm_crtc_id).unwrap().allocate_submission(
            output_id,
            scene,
            frame_callback_surfaces,
            presentation_feedbacks,
            true,
        );
        let callback_surfaces = submission.frame_callback_surfaces.clone();
        self.output_mut(drm_crtc_id)
            .unwrap()
            .mark_frame_submitted(SubmittedFrame::DirectScanout {
                submission,
                framebuffer: candidate.framebuffer,
                cursor_slot: cursor_plane.map(|cursor| cursor.slot),
            });
        Ok(callback_surfaces)
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
            buffers,
            displayed_buffer_index: 0,
            render_buffer_index: 1,
            needs_frame: false,
            frame_scheduled: false,
            rendering_active: false,
            page_flip_pending: false,
            in_flight_releases: Vec::new(),
            present_count: 0,
            damage_history: VecDeque::new(),
            last_scene: Vec::new(),
            force_full_redraw: true,
            rendered_buffer_present_id: [None, None],
            in_flight_frame: None,
            displayed_external_fb: None,
            retiring_external_fb: None,
            displayed_cursor_slot: None,
            retiring_cursor_slot: None,
            cursor_slots: [None, None],
            cursor_plane_state: CursorPlaneState::Ready,
            cursor_animation_delay_msec: None,
            allow_in_fence_fd: true,
            allow_fb_damage_clips: true,
            next_submission_id: 1,
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
    ScheduleFrame,
    RenderFrame,
    FlipComplete { sequence: u32, tv_sec: u32, tv_usec: u32, crtc_id: u32 },
    Stop,
}

enum DrmCursorAnimationTimerAction {
    Start,
    Arm { delay_msec: Option<u32> },
    Fired,
    Failed { error: std::io::Error },
    Stop,
}

struct DrmCursorAnimationTimer {
    drm_id: DrmId,
    output_id: OutputId,
    timer_fd: TimerFd,
    read_op_id: Option<RingId>,
}

model_instance_id!(DrmCursorAnimationTimerId => DrmCursorAnimationTimer);

pub fn sync_output_nodes_for_backend(state: &mut State, drm_id: DrmId) {
    state.refresh_outputs_for_backend(BackendOutputRef::new(drm_id));
}

pub fn schedule_output(state: &State, output_id: OutputId, dispatcher: &mut Dispatcher) {
    if state.backend_for_output(output_id).is_none() {
        log::warn!("schedule_output: invalid logical output {:?}", output_id);
        return;
    }
    dispatcher.dispatch_to(output_id, DrmAction::ScheduleFrame);
}

pub fn schedule_active_output(state: &State, dispatcher: &mut Dispatcher) {
    let Some(output_id) = state.active_output_id() else {
        log::warn!("schedule_active_output: no active logical output");
        return;
    };

    schedule_output(state, output_id, dispatcher);
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

pub fn schedule_outputs<I>(state: &State, output_ids: I, dispatcher: &mut Dispatcher)
where
    I: IntoIterator<Item = OutputId>,
{
    let mut unique = BTreeSet::new();
    for output_id in output_ids {
        if unique.insert(output_id) {
            schedule_output(state, output_id, dispatcher);
        }
    }
}

pub fn schedule_outputs_for_rect(state: &State, rect: RectI, dispatcher: &mut Dispatcher) {
    let output_ids = state.outputs_intersecting_rect(rect);
    if output_ids.is_empty() {
        if let Some(output_id) = state.primary_output_for_rect(rect, None) {
            schedule_output(state, output_id, dispatcher);
        }
        return;
    }
    schedule_outputs(state, output_ids, dispatcher);
}

pub fn schedule_outputs_for_rects<I>(state: &State, rects: I, dispatcher: &mut Dispatcher)
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
    schedule_outputs(state, output_ids, dispatcher);
}

pub fn schedule_outputs_for_points(
    state: &State,
    points: impl IntoIterator<Item = (f64, f64)>,
    dispatcher: &mut Dispatcher,
) {
    let output_ids = points.into_iter().filter_map(|(x, y)| {
        let (x, y) = state.clamp_point_to_outputs(x.round() as i32, y.round() as i32)?;
        state.output_at_point(x, y)
    });
    schedule_outputs(state, output_ids, dispatcher);
}

fn arm_cursor_animation_timer(
    dispatcher: &mut Dispatcher,
    output_id: OutputId,
    delay_msec: Option<u32>,
) {
    dispatcher.dispatch_to(
        DrmCursorAnimationTimerId::from(output_id.as_u64()),
        DrmCursorAnimationTimerAction::Arm { delay_msec },
    );
}

pub fn discard_backend_presentation_feedbacks(
    state: &mut State,
    drm_id: DrmId,
    dispatcher: &mut Dispatcher,
) {
    let discarded = {
        let drm = state.drm.get_mut(drm_id);
        drm.outputs
            .values_mut()
            .map(DrmOutput::take_in_flight_presentation_feedbacks_for_discard)
            .collect::<Vec<_>>()
    };
    let mut feedbacks = Vec::new();
    for (pipeline_feedbacks, damage_blob) in discarded {
        feedbacks.extend(pipeline_feedbacks);
        if let Some(blob) = damage_blob {
            let drm = state.drm.get(drm_id);
            let _ = drm.fd.destroy_property_blob(blob);
        }
    }
    if feedbacks.is_empty() {
        return;
    }
    dispatch_presentation_feedback_discarded(dispatcher, &feedbacks);
}

fn output_overlap_area(scene: &[SceneItemState], output_rect: RectI) -> i64 {
    scene
        .iter()
        .map(|item| {
            let x1 = item.bounds.x.max(output_rect.x);
            let y1 = item.bounds.y.max(output_rect.y);
            let x2 = item.bounds.x2().min(output_rect.x2());
            let y2 = item.bounds.y2().min(output_rect.y2());
            let width = (x2 - x1).max(0) as i64;
            let height = (y2 - y1).max(0) as i64;
            width * height
        })
        .sum()
}

fn select_presentation_output_for_scene(
    state: &State,
    drm_id: DrmId,
    scene: &[SceneItemState],
) -> Option<OutputId> {
    let backend = BackendOutputRef::new(drm_id);
    let layout = state.output_layout();
    let output_ids = state.output_ids_for_backend(backend);
    let mut best = None;
    let mut best_area = -1i64;
    for output_id in output_ids {
        let Some(output_rect) = layout.bounds(output_id) else {
            continue;
        };
        let area = output_overlap_area(scene, output_rect);
        if area > best_area {
            best_area = area;
            best = Some(output_id);
        }
    }
    best.or_else(|| {
        state.active_output_id().filter(|output_id| {
            layout
                .entry(*output_id)
                .is_some_and(|entry| entry.backend == backend)
        })
    })
}

fn collect_output_render_plan(
    state: &mut State,
    drm_id: DrmId,
    logical_output_id: Option<OutputId>,
    frame_time_msec: u32,
) -> OutputRenderPlan {
    let logical_output_id = logical_output_id.expect("missing logical output for DRM render plan");
    let drm_crtc_id = state
        .output_crtc_id(logical_output_id)
        .expect("missing DRM CRTC for logical output");
    let global_bounds = state.output_bounds(logical_output_id).unwrap_or_else(|| {
        state
            .drm
            .get(drm_id)
            .output(drm_crtc_id)
            .unwrap()
            .output_bounds()
    });
    let (
        mut items,
        mut software_cursor_items,
        deferred_releases,
        cursor_plane,
        cursor_animation_delay_msec,
    ) = collect_render_items(state, drm_id, drm_crtc_id, global_bounds, frame_time_msec);
    translate_scene_items(&mut items, -global_bounds.x, -global_bounds.y);
    translate_scene_items(&mut software_cursor_items, -global_bounds.x, -global_bounds.y);
    let frame = {
        let drm = state.drm.get(drm_id);
        build_frame_plan(
            drm.output(drm_crtc_id)
                .expect("missing DRM output pipeline for frame plan"),
            items,
            software_cursor_items,
            cursor_plane,
        )
    };
    let output_id = Some(logical_output_id)
        .or_else(|| select_presentation_output_for_scene(state, drm_id, &frame.scene));
    OutputRenderPlan {
        output_id,
        global_bounds,
        local_origin: (global_bounds.x, global_bounds.y),
        deferred_releases,
        cursor_animation_delay_msec,
        frame,
    }
}

impl Model for DrmCursorAnimationTimer {
    type Action = DrmCursorAnimationTimerAction;
    const START: Option<Self::Action> = Some(DrmCursorAnimationTimerAction::Start);
    const STOP: Option<Self::Action> = Some(DrmCursorAnimationTimerAction::Stop);

    fn process(
        &mut self,
        state: &mut crate::state::State,
        action: Self::Action,
        dispatcher: &mut crate::runner::Dispatcher,
    ) -> Result<(), ()> {
        match action {
            DrmCursorAnimationTimerAction::Start => {
                self.dispatch_read(dispatcher);
            }
            DrmCursorAnimationTimerAction::Arm { delay_msec } => {
                let expiration = delay_msec.map(|delay| {
                    Expiration::OneShot(Duration::from_millis(u64::from(delay)).into())
                });
                if let Err(error) = self.set_expiration(expiration) {
                    log::warn!("failed to arm cursor animation timer: {:?}", error);
                }
            }
            DrmCursorAnimationTimerAction::Fired => {
                let keep_animating = state
                    .output_crtc_id(self.output_id)
                    .and_then(|drm_crtc_id| {
                        state
                            .drm
                            .get(self.drm_id)
                            .output(drm_crtc_id)
                            .and_then(|output| output.cursor_animation_delay_msec)
                    })
                    .is_some();
                if keep_animating {
                    dispatcher.dispatch_to(self.output_id, DrmAction::ScheduleFrame);
                }
                self.dispatch_read(dispatcher);
            }
            DrmCursorAnimationTimerAction::Failed { error } => {
                log::warn!("cursor animation timer read failed: {:?}", error);
                self.dispatch_read(dispatcher);
            }
            DrmCursorAnimationTimerAction::Stop => {
                let _ = self.set_expiration(None);
                if let Some(read_op_id) = self.read_op_id.take() {
                    dispatcher.dispatch(IoUringAction::Cancel { target: read_op_id });
                }
            }
        }
        Ok(())
    }
}

impl DrmCursorAnimationTimer {
    fn new(drm_id: DrmId, output_id: OutputId) -> Self {
        let timer_fd = TimerFd::new(
            timerfd::ClockId::CLOCK_MONOTONIC,
            TimerFlags::TFD_CLOEXEC | TimerFlags::TFD_NONBLOCK,
        )
        .expect("failed to create cursor animation timerfd");
        Self { drm_id, output_id, timer_fd, read_op_id: None }
    }

    fn model_id(&self) -> DrmCursorAnimationTimerId {
        DrmCursorAnimationTimerId::from(self.output_id.as_u64())
    }

    fn dispatch_read(&mut self, dispatcher: &mut Dispatcher) {
        if self.read_op_id.is_some() {
            return;
        }
        let read_op_id = RingId::fresh();
        self.read_op_id = Some(read_op_id);
        dispatcher.dispatch(IoUringAction::Read {
            id: Some(read_op_id),
            fd: self.timer_fd.as_fd().as_raw_fd(),
            offset: 0,
            length: std::mem::size_of::<u64>(),
            dispatch_to: crate::runner::Redispatcher::to_instance(self.model_id(), |result| {
                match result {
                    Ok(_) => DrmCursorAnimationTimerAction::Fired,
                    Err(error) => DrmCursorAnimationTimerAction::Failed { error },
                }
            }),
        });
    }

    fn set_expiration(&mut self, expiration: Option<Expiration>) -> std::io::Result<()> {
        self.timer_fd
            .set(
                expiration.unwrap_or_else(|| Expiration::OneShot(Duration::ZERO.into())),
                TimerSetTimeFlags::empty(),
            )
            .map_err(std::io::Error::other)
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
            DrmAction::Start => {
                dispatcher.start_model(
                    self.timer_id(),
                    DrmCursorAnimationTimer::new(self.drm_id, self.output_id),
                );
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
                dispatcher.stop_model(self.timer_id());
            }
        };
        Ok(())
    }
}

impl DrmOutputModel {
    fn model_id(&self) -> OutputId {
        self.output_id
    }

    fn timer_id(&self) -> DrmCursorAnimationTimerId {
        DrmCursorAnimationTimerId::from(self.output_id.as_u64())
    }

    fn schedule_frame(&mut self, state: &mut crate::state::State, dispatcher: &mut Dispatcher) {
        let Some(drm_crtc_id) = state.output_crtc_id(self.output_id) else {
            return;
        };
        let scheduled = state
            .drm
            .get_mut(self.drm_id)
            .output_mut(drm_crtc_id)
            .is_some_and(DrmOutput::request_frame);
        if scheduled {
            dispatcher.dispatch_to(self.model_id(), DrmAction::RenderFrame);
        }
    }

    fn render_frame(&mut self, state: &mut crate::state::State, dispatcher: &mut Dispatcher) {
        let Some(drm_crtc_id) = state.output_crtc_id(self.output_id) else {
            return;
        };
        let frame_time_msec = monotonic_time_msec();
        let ready = {
            let drm = state.drm.get_mut(self.drm_id);
            drm.output_mut(drm_crtc_id)
                .is_some_and(DrmOutput::begin_render)
        };
        let failed_releases = {
            if !ready {
                log::warn!("DrmOutputModel::render_frame skipped: output not ready");
                return;
            }
            let output_render_plan = collect_output_render_plan(
                state,
                self.drm_id,
                Some(self.output_id),
                frame_time_msec,
            );
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
                mut scene,
                mut frame_callback_surfaces,
                mut presentation_feedbacks,
                mut damage,
                direct_scanout,
                mut cursor_plane,
                mut preserve_contents,
            } = frame;
            let drm = state.drm.get_mut(self.drm_id);
            drm.output_mut(drm_crtc_id).unwrap().in_flight_releases = deferred_releases;
            drm.output_mut(drm_crtc_id)
                .unwrap()
                .cursor_animation_delay_msec = cursor_animation_delay_msec;

            if let Some(candidate) = direct_scanout {
                if drm
                    .submit_direct_scanout(
                        drm_crtc_id,
                        candidate,
                        output_id,
                        scene.clone(),
                        frame_callback_surfaces.clone(),
                        presentation_feedbacks.clone(),
                        cursor_plane.take(),
                    )
                    .is_ok()
                {
                    drm.output_mut(drm_crtc_id)
                        .unwrap()
                        .note_cursor_plane_success();
                    dispatch_submission_frame_callbacks(
                        &frame_callback_surfaces,
                        frame_time_msec,
                        dispatcher,
                    );
                    drm.output_mut(drm_crtc_id).unwrap().force_full_redraw = true;
                    return;
                }
                if !software_cursor_items.is_empty() {
                    items.append(&mut software_cursor_items);
                    let fallback_plan =
                        build_frame_plan(drm.output(drm_crtc_id).unwrap(), items, Vec::new(), None);
                    items = fallback_plan.items;
                    software_cursor_items = fallback_plan.software_cursor_items;
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
                let releases =
                    std::mem::take(&mut drm.output_mut(drm_crtc_id).unwrap().in_flight_releases);
                dispatch_submission_frame_callbacks(
                    &frame_callback_surfaces,
                    frame_time_msec,
                    dispatcher,
                );
                Some(releases)
            } else {
                let render_items = scene_items_to_render_items(&items);
                let render_completion =
                    drm.render_output(drm_crtc_id, &render_items, &damage, preserve_contents);
                let callback_surfaces = frame_callback_surfaces.clone();
                let submit_had_cursor_plane = cursor_plane.is_some();
                if submit_had_cursor_plane {
                    log::debug!("Submitting DRM frame with cursor plane enabled");
                }
                let submit_result = drm.submit_render_buffer(
                    drm_crtc_id,
                    &render_completion,
                    &damage,
                    output_id,
                    scene,
                    frame_callback_surfaces,
                    presentation_feedbacks.clone(),
                    cursor_plane,
                );
                if let Err(error) = submit_result {
                    if submit_had_cursor_plane {
                        if !software_cursor_items.is_empty() {
                            log::warn!(
                                "DRM cursor plane submission failed, retrying with software cursor: {:?}",
                                error
                            );
                            items.append(&mut software_cursor_items);
                            let fallback_plan = build_frame_plan(
                                drm.output(drm_crtc_id).unwrap(),
                                items,
                                Vec::new(),
                                None,
                            );
                            let fallback_render_items =
                                scene_items_to_render_items(&fallback_plan.items);
                            let fallback_render_completion = drm.render_output(
                                drm_crtc_id,
                                &fallback_render_items,
                                &fallback_plan.damage,
                                fallback_plan.preserve_contents,
                            );
                            let fallback_callbacks = fallback_plan.frame_callback_surfaces.clone();
                            match drm.submit_render_buffer(
                                drm_crtc_id,
                                &fallback_render_completion,
                                &fallback_plan.damage,
                                output_id,
                                fallback_plan.scene,
                                fallback_plan.frame_callback_surfaces,
                                fallback_plan.presentation_feedbacks.clone(),
                                None,
                            ) {
                                Ok(_) => {
                                    drm.output_mut(drm_crtc_id)
                                        .unwrap()
                                        .note_cursor_plane_failure();
                                    dispatch_submission_frame_callbacks(
                                        &fallback_callbacks,
                                        frame_time_msec,
                                        dispatcher,
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
                                    let failed_releases = std::mem::take(
                                        &mut drm
                                            .output_mut(drm_crtc_id)
                                            .unwrap()
                                            .in_flight_releases,
                                    );
                                    dispatch_presentation_feedback_discarded(
                                        dispatcher,
                                        &fallback_plan.presentation_feedbacks,
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
                            let failed_releases = std::mem::take(
                                &mut drm.output_mut(drm_crtc_id).unwrap().in_flight_releases,
                            );
                            dispatch_presentation_feedback_discarded(
                                dispatcher,
                                &presentation_feedbacks,
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
                        let failed_releases = std::mem::take(
                            &mut drm.output_mut(drm_crtc_id).unwrap().in_flight_releases,
                        );
                        dispatch_presentation_feedback_discarded(
                            dispatcher,
                            &presentation_feedbacks,
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
                    if submit_had_cursor_plane {
                        drm.output_mut(drm_crtc_id)
                            .unwrap()
                            .note_cursor_plane_success();
                    }
                    dispatch_submission_frame_callbacks(
                        &callback_surfaces,
                        frame_time_msec,
                        dispatcher,
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
        let (releases, should_render, completed_submission, cursor_animation_delay_msec) = {
            let drm = state.drm.get_mut(self.drm_id);
            let releases =
                std::mem::take(&mut drm.output_mut(drm_crtc_id).unwrap().in_flight_releases);
            let (should_render, completed_submission) =
                drm.output_mut(drm_crtc_id).unwrap().finish_page_flip();
            let cursor_animation_delay_msec =
                drm.output(drm_crtc_id).unwrap().cursor_animation_delay_msec;
            (releases, should_render, completed_submission, cursor_animation_delay_msec)
        };

        for release in releases {
            release.resolve(state, dispatcher);
        }
        if let Some(completed_submission) = completed_submission {
            dispatch_presentation_feedback_presented(
                state,
                self.drm_id,
                dispatcher,
                completed_submission.submission.output_id,
                &completed_submission.submission.presentation_feedbacks,
                completed_submission.submission.zero_copy,
                tv_sec,
                tv_usec,
                sequence,
            );
            if let Some(blob) = completed_submission.damage_blob {
                let drm = state.drm.get(self.drm_id);
                let _ = drm.fd.destroy_property_blob(blob);
            }
        }
        sync_output_nodes_for_backend(state, self.drm_id);
        arm_cursor_animation_timer(dispatcher, self.output_id, cursor_animation_delay_msec);
        if should_render {
            dispatcher.dispatch_to(self.model_id(), DrmAction::RenderFrame);
        }
    }
}

fn collect_surface_snapshots(
    state: &mut crate::state::State,
    surface_object_id: ObjectId,
    base_x: i32,
    base_y: i32,
    retired_candidates: &mut Vec<RetiredBufferCandidate>,
    snapshots: &mut Vec<crate::wayland_objects::wl_surface::SurfaceRenderState>,
) {
    let (stack, surface_state) = {
        let mut surface = state.objects.grab_object::<WlSurface>(surface_object_id);
        retired_candidates.extend(surface.take_retired_buffers());
        let Some(surface_state) = surface.take_render_state_at(base_x, base_y) else {
            return;
        };
        (surface.current_stack().to_vec(), surface_state)
    };

    for entry in &stack {
        if let StackEntry::Child(child_surface_object_id) = *entry {
            let child_base = {
                let Some(child_surface) = state
                    .objects
                    .try_ref_object::<WlSurface>(child_surface_object_id)
                else {
                    continue;
                };
                let (x, y) = child_surface.current_subsurface_position();
                (surface_state.x + x, surface_state.y + y)
            };
            collect_surface_snapshots(
                state,
                child_surface_object_id,
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
    let mut per_client = BTreeMap::<ClientId, Vec<WlSurfaceId>>::new();
    let mut seen = BTreeSet::<(ClientId, WlSurfaceId)>::new();
    for surface in callback_surfaces {
        let client_id = surface.client_id;
        let surface_id = surface.surface_id;
        if !seen.insert((client_id, surface_id)) {
            continue;
        }
        per_client.entry(client_id).or_default().push(surface_id);
    }
    for (client_id, surface_ids) in per_client {
        dispatcher.dispatch_to(
            client_id,
            ClientAction::CompleteSurfaceFrameCallbacks { surface_ids, time_msec: frame_time_msec },
        );
    }
}

/// `wp_presentation_feedback.presented`: v1 requires `refresh` to be 0 when the refresh rate is not constant.
const WP_PRESENTATION_FEEDBACK_PRESENTED_REFRESH_RELAXED_SINCE: u32 = 2;

fn presentation_refresh_interval_ns(
    refresh_millihz: i32,
    feedback_version: u32,
    refresh_is_constant: bool,
) -> u32 {
    if refresh_millihz <= 0 {
        return 0;
    }
    if feedback_version < WP_PRESENTATION_FEEDBACK_PRESENTED_REFRESH_RELAXED_SINCE
        && !refresh_is_constant
    {
        return 0;
    }
    let refresh = u64::try_from(refresh_millihz).unwrap_or(0);
    let refresh_ns = 1_000_000_000_000u64 / refresh;
    refresh_ns.min(u64::from(u32::MAX)) as u32
}

fn refresh_interval_ns_for_output(
    state: &State,
    output_id: Option<OutputId>,
    feedback_version: u32,
) -> u32 {
    let Some(output_id) = output_id else {
        return 0;
    };
    let Some(drm_id) = state
        .backend_for_output(output_id)
        .map(|backend| backend.drm_id)
    else {
        return 0;
    };
    let Some(drm_crtc_id) = state.output_crtc_id(output_id) else {
        return 0;
    };
    let Some(mode) = state.drm.get(drm_id).current_output_mode(drm_crtc_id) else {
        return 0;
    };
    let refresh_is_constant = true;
    presentation_refresh_interval_ns(mode.refresh_millihz, feedback_version, refresh_is_constant)
}

fn presentation_feedback_flags(zero_copy: bool) -> u32 {
    use crate::protocol_generated::wp_presentation_feedback::Kind;

    let mut flags = Kind::Vsync as u32 | Kind::HwCompletion as u32;
    if zero_copy {
        flags |= Kind::ZeroCopy as u32;
    }
    flags
}

fn sanitize_present_timestamp(tv_sec: u32, tv_usec: u32) -> (u64, u32) {
    if tv_usec < 1_000_000 && tv_sec > 2 {
        return (u64::from(tv_sec), tv_usec.saturating_mul(1000));
    }

    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) } != 0 {
        return (u64::from(tv_sec), tv_usec.min(999_999).saturating_mul(1000));
    }

    let tv_sec = u64::try_from(ts.tv_sec.max(0)).unwrap_or(0);
    let tv_nsec = u32::try_from(ts.tv_nsec.clamp(0, 999_999_999)).unwrap_or(0);
    (tv_sec, tv_nsec)
}

fn dispatch_presentation_feedback_discarded(
    dispatcher: &mut Dispatcher,
    feedbacks: &[SubmissionPresentationFeedback],
) {
    let mut per_client = BTreeMap::<ClientId, Vec<WpPresentationFeedbackId>>::new();
    for feedback in feedbacks {
        per_client
            .entry(feedback.client_id)
            .or_default()
            .push(feedback.feedback_id);
    }
    for (client_id, feedback_ids) in per_client {
        dispatcher
            .dispatch_to(client_id, ClientAction::PresentationFeedbackDiscarded { feedback_ids });
    }
}

fn dispatch_presentation_feedback_presented(
    state: &State,
    _drm_id: DrmId,
    dispatcher: &mut Dispatcher,
    output_id: Option<OutputId>,
    feedbacks: &[SubmissionPresentationFeedback],
    zero_copy: bool,
    tv_sec: u32,
    tv_usec: u32,
    sequence: u32,
) {
    if feedbacks.is_empty() {
        return;
    }
    let (tv_sec, tv_nsec) = sanitize_present_timestamp(tv_sec, tv_usec);
    let tv_sec_hi = (tv_sec >> 32) as u32;
    let tv_sec_lo = tv_sec as u32;
    let sequence = u64::from(sequence);
    let seq_hi = (sequence >> 32) as u32;
    let seq_lo = sequence as u32;
    let flags = presentation_feedback_flags(zero_copy);

    let mut per_client = BTreeMap::<(ClientId, u32), Vec<WpPresentationFeedbackId>>::new();
    for feedback in feedbacks {
        per_client
            .entry((feedback.client_id, feedback.version))
            .or_default()
            .push(feedback.feedback_id);
    }

    for ((client_id, feedback_version), feedback_ids) in per_client {
        let refresh = refresh_interval_ns_for_output(state, output_id, feedback_version);
        dispatcher.dispatch_to(
            client_id,
            ClientAction::PresentationFeedbackPresented {
                feedback_ids,
                output_id,
                tv_sec_hi,
                tv_sec_lo,
                tv_nsec,
                refresh,
                seq_hi,
                seq_lo,
                flags,
            },
        );
    }
}

fn build_frame_plan(
    output: &DrmOutput,
    mut items: Vec<SceneItem>,
    mut software_cursor_items: Vec<SceneItem>,
    cursor_plane: Option<CursorPlaneCandidate>,
) -> FramePlan {
    let output_bounds = output.output_bounds();
    let mut base_damage = DamageRegion::new();
    if cursor_plane.is_none() {
        items.append(&mut software_cursor_items);
    }
    let (scene, frame_callback_surfaces, presentation_feedbacks) =
        collect_submission_metadata(&items, &software_cursor_items);
    let last_scene = &output.last_scene;
    let order_changed = scene_order(last_scene) != scene_order(&scene);

    if !ENABLE_DAMAGE
        || output.force_full_redraw
        || order_changed
        || scene.len() != last_scene.len()
    {
        base_damage.add_rect(output_bounds);
    } else {
        for (current, previous) in items.iter().zip(last_scene.iter()) {
            if current.state.key != previous.key {
                base_damage.add_rect(output_bounds);
                break;
            }
            if current.state.bounds != previous.bounds
                || current.state.buffer_object_id != previous.buffer_object_id
                || current.state.tex_transform != previous.tex_transform
            {
                base_damage.add_rect(previous.bounds);
                base_damage.add_rect(current.state.bounds);
            } else {
                base_damage.add_region(&current.damage);
            }
        }
    }

    base_damage = output.accumulate_damage_from_age(&base_damage);
    base_damage.intersect_rect(output_bounds);
    if !presentation_feedbacks.is_empty()
        && cursor_plane.is_none()
        && !scene.is_empty()
        && base_damage.is_empty()
    {
        base_damage.add_rect(output_bounds);
    }
    for item in &items {
        if item.sample_damage_padding <= 0 {
            continue;
        }
        let mut padded_item_damage = base_damage.clone();
        padded_item_damage.intersect_rect(item.state.bounds);
        if padded_item_damage.is_empty() {
            continue;
        }
        padded_item_damage.expand(item.sample_damage_padding);
        padded_item_damage.intersect_rect(item.state.bounds);
        base_damage.add_region(&padded_item_damage);
    }

    let mut occluded = DamageRegion::new();
    for item in items.iter_mut().rev() {
        let mut visible_damage = base_damage.clone();
        visible_damage.intersect_rect(item.state.bounds);
        visible_damage.subtract_region(&occluded);
        item.damage = visible_damage;
        if item.opaque {
            occluded.add_region(&item.opaque_region);
        }
    }

    let direct_scanout = if items.len() == 1
        && items[0].state.bounds == output_bounds
        && output.page_flip_pending == false
    {
        let bounds = items[0].state.bounds;
        let sync = items[0].sync.clone();
        items[0]
            .direct_scanout
            .take()
            .map(|framebuffer| DirectScanoutCandidate { bounds, sync, framebuffer })
    } else {
        None
    };

    FramePlan {
        items,
        software_cursor_items,
        scene,
        frame_callback_surfaces,
        presentation_feedbacks,
        damage: base_damage,
        direct_scanout,
        cursor_plane,
        preserve_contents: ENABLE_DAMAGE
            && output.preserve_render_buffer_contents()
            && !output.force_full_redraw,
    }
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
                        buffer_object_id: ObjectId::from_parts(u32::MAX, 0),
                        bounds,
                        tex_transform: transform::Mat3::identity(),
                    },
                    texture: texture_id,
                    owned_texture: Some(texture),
                    damage: DamageRegion::from_rect(bounds),
                    sample_damage_padding: 0,
                    state_has_alpha: true,
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
                client_id: surface_key.client_id,
                feedback_id: feedback.id,
                version: feedback.version,
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
    objects: &mut Box<crate::state::IndexMap<ObjectId, Box<dyn crate::object::Object>>>,
    drm: &Drm,
    drm_fd: &DrmFd,
    drm_crtc_id: u32,
    output_bounds: RectI,
    snapshots: Vec<crate::wayland_objects::wl_surface::SurfaceRenderState>,
    allow_direct_scanout: bool,
) {
    for surface_state in snapshots {
        let buffer_oid = surface_state.buffer_object_id;
        if !objects.contains(buffer_oid) {
            log::warn!("collect_render_items: buffer object {:?} missing (destroyed?)", buffer_oid);
            continue;
        }
        let Some(_) = objects.try_ref_object::<WlBuffer>(buffer_oid) else {
            log::warn!("collect_render_items: object {:?} is not a WlBuffer", buffer_oid);
            continue;
        };

        let mut obj = objects.grab(surface_state.buffer_object_id);
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
                "collect_render_items: no prepared texture for buffer_object_id={:?}",
                surface_state.buffer_object_id
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
        let direct_scanout = if allow_direct_scanout
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
        let surface_key = SurfaceKey {
            client_id: surface_state.surface_client_id,
            surface_id: surface_state.surface_id,
        };
        items.push(SceneItem {
            state: SceneItemState {
                key: SceneItemKey::Surface(surface_key),
                surface_key: Some(surface_key),
                buffer_object_id: surface_state.buffer_object_id,
                bounds,
                tex_transform,
            },
            texture,
            owned_texture: None,
            damage,
            sample_damage_padding,
            state_has_alpha: buffer.format().has_alpha,
            opaque_region: opaque_region.clone(),
            opaque: !opaque_region.is_empty(),
            sync: surface_state.sync,
            direct_scanout,
            presentation_feedbacks: surface_state.presentation_feedbacks,
        });
    }
}

fn ssd_frame_segments(layout: SsdFrameLayout) -> impl Iterator<Item = (SsdFrameSegment, RectI)> {
    let top = RectI::from_extents(
        layout.frame_rect.x,
        layout.frame_rect.y,
        layout.frame_rect.x2(),
        layout.content_rect.y,
    )
    .map(|rect| (SsdFrameSegment::Top, rect));
    let left = RectI::from_extents(
        layout.frame_rect.x,
        layout.content_rect.y,
        layout.content_rect.x,
        layout.content_rect.y2(),
    )
    .map(|rect| (SsdFrameSegment::Left, rect));
    let right = RectI::from_extents(
        layout.content_rect.x2(),
        layout.content_rect.y,
        layout.frame_rect.x2(),
        layout.content_rect.y2(),
    )
    .map(|rect| (SsdFrameSegment::Right, rect));
    let bottom = RectI::from_extents(
        layout.frame_rect.x,
        layout.content_rect.y2(),
        layout.frame_rect.x2(),
        layout.frame_rect.y2(),
    )
    .map(|rect| (SsdFrameSegment::Bottom, rect));

    [top, left, right, bottom].into_iter().flatten()
}

fn push_ssd_frame_scene_items(
    items: &mut Vec<SceneItem>,
    root_surface_object_id: ObjectId,
    layout: SsdFrameLayout,
    texture_id: gl::Uint,
    owned_texture: &mut Option<GlTexture>,
) {
    for (index, (segment, bounds)) in ssd_frame_segments(layout).enumerate() {
        items.push(SceneItem {
            state: SceneItemState {
                key: SceneItemKey::SsdFrame { root_surface_object_id, segment },
                surface_key: None,
                buffer_object_id: root_surface_object_id,
                bounds,
                tex_transform: transform::Mat3::identity(),
            },
            texture: texture_id,
            owned_texture: (index == 0).then(|| owned_texture.take()).flatten(),
            damage: DamageRegion::from_rect(bounds),
            sample_damage_padding: 0,
            state_has_alpha: false,
            opaque_region: DamageRegion::from_rect(bounds),
            opaque: true,
            sync: crate::sync::SurfaceSyncState::default(),
            direct_scanout: None,
            presentation_feedbacks: Vec::new(),
        });
    }
}

fn collect_cursor_surface_snapshots(
    state: &mut crate::state::State,
    retired_candidates: &mut Vec<RetiredBufferCandidate>,
) -> Vec<crate::wayland_objects::wl_surface::SurfaceRenderState> {
    let (pointer_x, pointer_y) = state.seat.pointer_position();
    let Some(CursorSourceRef::Surface { cursor, .. }) = state.seat.visible_cursor_source() else {
        return Vec::new();
    };
    let Some(surface) = state.objects.try_ref_object::<WlSurface>(cursor.object_id) else {
        return Vec::new();
    };
    let (root_x, root_y) = surface.root_position();
    let base_x = pointer_x.round() as i32 - cursor.hotspot_x - root_x;
    let base_y = pointer_y.round() as i32 - cursor.hotspot_y - root_y;
    let mut snapshots = Vec::new();
    collect_surface_snapshots(
        state,
        cursor.object_id,
        base_x,
        base_y,
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
        .try_ref_object::<WlSurface>(cursor.object_id)?;
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
    let buffer_object_id = render_state.buffer_object_id;
    let content_revision = surface.content_revision();
    let buffer = state.objects.try_ref_object::<WlBuffer>(buffer_object_id)?;
    if buffer.dimensions() != (render_state.width, render_state.height) {
        return None;
    }
    let Some(buffer) = buffer.shm_cursor_argb8888() else {
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
            surface_object_id: cursor.object_id,
            buffer_object_id,
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

fn collect_render_items(
    state: &mut crate::state::State,
    drm_id: DrmId,
    drm_crtc_id: u32,
    output_bounds: RectI,
    frame_time_msec: u32,
) -> (Vec<SceneItem>, Vec<SceneItem>, Vec<DeferredRelease>, Option<CursorPlaneCandidate>, Option<u32>)
{
    let mut items = Vec::new();
    let mut deferred_releases = Vec::new();
    let mut retired_candidates = Vec::new();
    let mut cursor_animation_delay_msec = None;
    let focused_root_surface_object_id = state.focused_root_surface_object_id();
    let root_surface_oids: Vec<_> = ordered_scene_root_surface_ids(state)
        .into_iter()
        .filter(|object_id| {
            let Some(surface) = state.objects.try_ref_object::<WlSurface>(*object_id) else {
                return false;
            };
            surface.is_root() && !surface.has_cursor_role()
        })
        .collect();
    let mut focused_frame_texture = GlTexture::from_rgba(&[0x50, 0x78, 0xc8, 0xff], 1, 1).ok();
    let mut unfocused_frame_texture = GlTexture::from_rgba(&[0x33, 0x38, 0x44, 0xff], 1, 1).ok();
    let focused_frame_texture_id = focused_frame_texture.as_ref().map(GlTexture::texture_id);
    let unfocused_frame_texture_id = unfocused_frame_texture.as_ref().map(GlTexture::texture_id);
    let mut snapshots = Vec::new();
    for object_id in root_surface_oids {
        if let Some(layout) = state.root_surface_ssd_layout(object_id) {
            let focused = focused_root_surface_object_id == Some(object_id);
            let (texture_id, owned_texture) = if focused {
                (focused_frame_texture_id, &mut focused_frame_texture)
            } else {
                (unfocused_frame_texture_id, &mut unfocused_frame_texture)
            };
            if let Some(texture_id) = texture_id {
                push_ssd_frame_scene_items(
                    &mut items,
                    object_id,
                    layout,
                    texture_id,
                    owned_texture,
                );
            }
        }
        collect_surface_snapshots(state, object_id, 0, 0, &mut retired_candidates, &mut snapshots);
    }

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
    let (objects, drm_map) = (&mut state.objects, &state.drm);
    let drm = drm_map.get(drm_id);
    let drm_fd = drm.fd();
    push_scene_items_from_snapshots(
        &mut items,
        objects,
        drm,
        &drm_fd,
        drm_crtc_id,
        output_bounds,
        snapshots,
        true,
    );
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

    (items, software_cursor_items, deferred_releases, cursor_plane, cursor_animation_delay_msec)
}

fn scene_items_to_render_items(items: &[SceneItem]) -> Vec<RenderItem> {
    items
        .iter()
        .map(|item| RenderItem {
            texture: item.texture,
            x: item.state.bounds.x,
            y: item.state.bounds.y,
            width: item.state.bounds.width,
            height: item.state.bounds.height,
            tex_transform: item.state.tex_transform,
            damage: item.damage.clone(),
            blend: item.state_has_alpha,
        })
        .collect()
}

fn translate_scene_items(items: &mut [SceneItem], dx: i32, dy: i32) {
    for item in items {
        item.state.bounds = item.state.bounds.translate(dx, dy);
        item.damage.translate(dx, dy);
        item.opaque_region.translate(dx, dy);
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
mod ssd_render_tests {
    use {
        super::{ssd_frame_segments, SsdFrameSegment},
        crate::{region::RectI, state::SsdFrameLayout},
    };

    #[test]
    fn ssd_frame_segments_cover_each_non_content_edge() {
        let layout = SsdFrameLayout {
            frame_rect: RectI::new(10, 20, 120, 90).unwrap(),
            content_rect: RectI::new(14, 32, 112, 74).unwrap(),
            titlebar_rect: RectI::new(14, 20, 112, 12),
        };

        let segments = ssd_frame_segments(layout).collect::<Vec<_>>();

        assert_eq!(
            segments,
            vec![
                (SsdFrameSegment::Top, RectI::new(10, 20, 120, 12).unwrap()),
                (SsdFrameSegment::Left, RectI::new(10, 32, 4, 74).unwrap()),
                (SsdFrameSegment::Right, RectI::new(126, 32, 4, 74).unwrap()),
                (SsdFrameSegment::Bottom, RectI::new(10, 106, 120, 4).unwrap()),
            ]
        );
    }
}

#[cfg(test)]
mod presentation_timing_tests {
    use {
        super::{
            build_surface_cursor_plane_request, collect_submission_metadata,
            cursor_plane_commit_dimensions, find_cursor_slot_index_for_key,
            presentation_feedback_flags, presentation_refresh_interval_ns, rgba_to_drm_argb8888,
            sanitize_present_timestamp, schedule_output, schedule_outputs_for_rects,
            select_cursor_upload_slot, select_presentation_output_for_scene,
            submitted_frame_feedbacks_for_discard, CursorPlaneContentKey, CursorSurfaceContentKey,
            OutputSubmission, SceneItem, SceneItemKey, SceneItemState,
            SubmissionPresentationFeedback, SubmittedFrame, SurfaceKey,
        },
        crate::{
            clients::ClientId,
            cursor::{CursorImageContentKey, CursorSurface},
            egl::EGL,
            globals::Globals,
            protocol_generated::{WlBufferId, WlSurfaceId, WpPresentationFeedbackId},
            region::{DamageRegion, RectI},
            runner::Dispatcher,
            state::{
                BackendOutputRef, DrmId, FractionalScale, ObjectId, Output, OutputId, OutputMode,
                State,
            },
            transform,
            wayland_objects::{
                wl_buffer::WlBuffer,
                wl_surface::{SurfacePresentationFeedback, WlSurface},
            },
            xkb::Xkb,
        },
    };

    fn submission_feedback(id: u32, version: u32) -> SubmissionPresentationFeedback {
        SubmissionPresentationFeedback {
            client_id: ClientId::from_raw(1),
            feedback_id: WpPresentationFeedbackId::from(id),
            version,
        }
    }

    fn surface_feedback(id: u32, version: u32) -> SurfacePresentationFeedback {
        SurfacePresentationFeedback {
            id: WpPresentationFeedbackId::from(id),
            version,
            output_id: None,
        }
    }

    fn make_state() -> State {
        let xkb = Xkb::new();
        let keyboard = xkb.create_keyboard_state();
        let mut globals = Globals::new();
        globals.initialize();
        State {
            objects: Default::default(),
            root_surfaces: Default::default(),
            layer_surface_objects: Default::default(),
            inputs: Default::default(),
            drm: Default::default(),
            outputs: Default::default(),
            active_output: None,
            workspace_groups: Default::default(),
            workspaces: Default::default(),
            output_workspace_groups: Default::default(),
            root_surface_workspaces: Default::default(),
            managed_root_surfaces: Default::default(),
            xdg_toplevel_roles: Default::default(),
            next_workspace_serial: 1,
            backend_syncobj_globals: Default::default(),
            globals,
            activation_tokens: Default::default(),
            egl: EGL::new(),
            config: Default::default(),
            seat: Default::default(),
            keyboard,
            xkb,
        }
    }

    fn make_output(
        backend: BackendOutputRef,
        drm_crtc_id: u32,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
    ) -> Output {
        Output {
            backend,
            drm_crtc_id,
            global_name: None,
            gamma_lut_size: 0,
            ctm_supported: false,
            gamma_control_object_id: None,
            x,
            y,
            physical_width: 0,
            physical_height: 0,
            scale: FractionalScale::ONE,
            transform: 0,
            make: String::new(),
            model: String::new(),
            name: String::new(),
            description: String::new(),
            enabled: true,
            current_mode: OutputMode { width, height, refresh_millihz: 60_000 },
            preferred_mode: None,
        }
    }

    fn make_surface_scene_item(
        surface_id: u32,
        bounds: RectI,
        feedbacks: Vec<SurfacePresentationFeedback>,
    ) -> SceneItem {
        let surface_key = SurfaceKey {
            client_id: ClientId::from_raw(1),
            surface_id: WlSurfaceId::from(surface_id),
        };
        SceneItem {
            state: SceneItemState {
                key: SceneItemKey::Surface(surface_key),
                surface_key: Some(surface_key),
                buffer_object_id: ObjectId::from((surface_id as usize) + 100usize),
                bounds,
                tex_transform: transform::Mat3::identity(),
            },
            texture: 0,
            owned_texture: None,
            damage: DamageRegion::from_rect(bounds),
            sample_damage_padding: 0,
            state_has_alpha: true,
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
    fn v1_feedback_uses_zero_refresh_when_rate_is_not_constant() {
        assert_eq!(presentation_refresh_interval_ns(60_000, 1, false), 0);
        assert_ne!(presentation_refresh_interval_ns(60_000, 2, false), 0);
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
        let mut state = make_state();
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
                surface_id: WlSurfaceId::from(2),
                object_id: surface_object_id,
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
                surface_object_id,
                buffer_object_id,
                content_revision: 1,
                width: 16,
                height: 12,
            })
        );
    }

    #[test]
    fn surface_cursor_request_rejects_subsurface_trees() {
        let mut state = make_state();
        let surface_object_id = state
            .insert_object(WlSurface::new(WlSurfaceId::from(10), ClientId::from_raw(1), 6))
            .1;
        {
            let surface = state
                .objects
                .get_mut(surface_object_id)
                .downcast_mut::<WlSurface>()
                .unwrap();
            surface.add_pending_child(ObjectId::from(999usize));
        }
        {
            let mut surface = state.objects.grab_object::<WlSurface>(surface_object_id);
            assert!(surface.apply_pending_tree_state(&mut state));
        }

        let request = build_surface_cursor_plane_request(
            &state,
            &CursorSurface {
                surface_id: WlSurfaceId::from(10),
                object_id: surface_object_id,
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
            vec![SurfaceKey {
                client_id: ClientId::from_raw(1),
                surface_id: WlSurfaceId::from(9),
            }]
        );
        assert_eq!(presentation_feedbacks, vec![submission_feedback(7, 2)]);
    }

    #[test]
    fn cursor_only_logical_scene_still_selects_an_output() {
        let mut state = make_state();
        let drm_id = DrmId::from(0usize);
        let backend = BackendOutputRef::new(drm_id);
        let left = state.insert_output(make_output(backend, 1, 0, 0, 100, 100));
        let right = state.insert_output(make_output(backend, 2, 100, 0, 100, 100));
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
        let mut state = make_state();
        let drm_id = DrmId::from(0usize);
        let backend = BackendOutputRef::new(drm_id);
        let left = state.insert_output(make_output(backend, 1, 0, 0, 100, 100));
        let right = state.insert_output(make_output(backend, 2, 100, 0, 100, 100));
        let scene = vec![SceneItemState {
            key: SceneItemKey::Surface(SurfaceKey {
                client_id: ClientId::from_raw(1),
                surface_id: crate::protocol_generated::WlSurfaceId::from(1),
            }),
            surface_key: Some(SurfaceKey {
                client_id: ClientId::from_raw(1),
                surface_id: crate::protocol_generated::WlSurfaceId::from(1),
            }),
            buffer_object_id: crate::state::ObjectId::from(1usize),
            bounds: RectI::new(80, 0, 80, 100).unwrap(),
            tex_transform: transform::Mat3::identity(),
        }];

        let selected = select_presentation_output_for_scene(&state, drm_id, &scene);

        assert_eq!(selected, Some(right));
        assert_ne!(selected, Some(left));
    }

    #[test]
    fn schedule_output_targets_logical_output_model_id() {
        let mut state = make_state();
        let drm_id = DrmId::from(0usize);
        let backend = BackendOutputRef::new(drm_id);
        let left = state.insert_output(make_output(backend, 1, 0, 0, 100, 100));
        let right = state.insert_output(make_output(backend, 2, 100, 0, 100, 100));
        let mut dispatcher = Dispatcher::default();

        schedule_output(&state, right, &mut dispatcher);

        assert!(dispatcher.test_has_queued_action_for::<super::DrmAction, OutputId>(
            right,
            |action| matches!(action, super::DrmAction::ScheduleFrame)
        ));
        assert!(!dispatcher.test_has_queued_action_for::<super::DrmAction, OutputId>(
            left,
            |action| matches!(action, super::DrmAction::ScheduleFrame)
        ));
    }

    #[test]
    fn schedule_outputs_for_rects_marks_old_and_new_outputs() {
        let mut state = make_state();
        let drm_id = DrmId::from(0usize);
        let backend = BackendOutputRef::new(drm_id);
        let left = state.insert_output(make_output(backend, 1, 0, 0, 100, 100));
        let right = state.insert_output(make_output(backend, 2, 100, 0, 100, 100));
        let mut dispatcher = Dispatcher::default();

        schedule_outputs_for_rects(
            &state,
            [RectI::new(0, 0, 100, 100).unwrap(), RectI::new(100, 0, 100, 100).unwrap()],
            &mut dispatcher,
        );

        assert!(dispatcher.test_has_queued_action_for::<super::DrmAction, OutputId>(
            left,
            |action| matches!(action, super::DrmAction::ScheduleFrame)
        ));
        assert!(dispatcher.test_has_queued_action_for::<super::DrmAction, OutputId>(
            right,
            |action| matches!(action, super::DrmAction::ScheduleFrame)
        ));
    }

    #[test]
    fn sanitize_present_timestamp_falls_back_for_suspicious_values() {
        let (tv_sec, tv_nsec) = sanitize_present_timestamp(1, 2_000_000);
        assert!(tv_sec > 0);
        assert!(tv_nsec < 1_000_000_000);
    }
}

#[cfg(test)]
mod surface_tree_render_tests {
    use {
        super::{collect_cursor_surface_snapshots, collect_surface_snapshots},
        crate::{
            clients::ClientId,
            egl::EGL,
            globals::Globals,
            protocol_generated::{WlSubsurfaceId, WlSurfaceId},
            state::{ObjectId, State},
            wayland_objects::wl_surface::WlSurface,
            xkb::Xkb,
        },
    };

    fn make_state() -> State {
        let xkb = Xkb::new();
        let keyboard = xkb.create_keyboard_state();
        let mut globals = Globals::new();
        globals.initialize();
        State {
            objects: Default::default(),
            root_surfaces: Default::default(),
            layer_surface_objects: Default::default(),
            inputs: Default::default(),
            drm: Default::default(),
            outputs: Default::default(),
            active_output: None,
            workspace_groups: Default::default(),
            workspaces: Default::default(),
            output_workspace_groups: Default::default(),
            root_surface_workspaces: Default::default(),
            managed_root_surfaces: Default::default(),
            xdg_toplevel_roles: Default::default(),
            next_workspace_serial: 1,
            backend_syncobj_globals: Default::default(),
            globals,
            activation_tokens: Default::default(),
            egl: EGL::new(),
            config: Default::default(),
            seat: Default::default(),
            keyboard,
            xkb,
        }
    }

    fn insert_surface(state: &mut State, id: u32) -> ObjectId {
        let object_id = state
            .insert_object(WlSurface::new(WlSurfaceId::from(id), ClientId::from_raw(1), 6))
            .1;
        state.register_root_surface(object_id);
        object_id
    }

    #[test]
    fn collect_surface_snapshots_walks_below_self_above() {
        let mut state = make_state();
        let root = insert_surface(&mut state, 1);
        let below = insert_surface(&mut state, 2);
        let above = insert_surface(&mut state, 3);

        {
            let root_surface = state
                .objects
                .get_mut(root)
                .downcast_mut::<WlSurface>()
                .unwrap();
            root_surface.test_map(ObjectId::from(100usize), 10, 20, 50, 40);
            root_surface.add_pending_child(below);
            root_surface.add_pending_child(above);
            assert!(root_surface.restack_child_relative(below, None, false));
            assert!(root_surface.restack_child_relative(above, None, true));
        }
        {
            let below_surface = state
                .objects
                .get_mut(below)
                .downcast_mut::<WlSurface>()
                .unwrap();
            below_surface.assign_subsurface(WlSubsurfaceId::from(11), root);
            below_surface.set_subsurface_position(2, 3);
            below_surface.test_map(ObjectId::from(101usize), 0, 0, 10, 10);
        }
        {
            let above_surface = state
                .objects
                .get_mut(above)
                .downcast_mut::<WlSurface>()
                .unwrap();
            above_surface.assign_subsurface(WlSubsurfaceId::from(12), root);
            above_surface.set_subsurface_position(7, 8);
            above_surface.test_map(ObjectId::from(102usize), 0, 0, 10, 10);
        }
        {
            let mut root_surface = state.objects.grab_object::<WlSurface>(root);
            assert!(root_surface.apply_pending_tree_state(&mut state));
        }

        let mut retired = Vec::new();
        let mut snapshots = Vec::new();
        collect_surface_snapshots(&mut state, root, 0, 0, &mut retired, &mut snapshots);

        let ids: Vec<_> = snapshots
            .iter()
            .map(|snapshot| snapshot.buffer_object_id)
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
        let mut state = make_state();
        let root = insert_surface(&mut state, 1);
        let child = insert_surface(&mut state, 2);

        {
            let root_surface = state
                .objects
                .get_mut(root)
                .downcast_mut::<WlSurface>()
                .unwrap();
            root_surface.test_map(ObjectId::from(120usize), 10, 20, 50, 40);
            root_surface.add_pending_child(child);
        }
        {
            let child_surface = state
                .objects
                .get_mut(child)
                .downcast_mut::<WlSurface>()
                .unwrap();
            child_surface.assign_subsurface(WlSubsurfaceId::from(31), root);
            child_surface.set_subsurface_position(3, 4);
            child_surface.test_map(ObjectId::from(121usize), 0, 0, 10, 10);
        }
        {
            let mut root_surface = state.objects.grab_object::<WlSurface>(root);
            assert!(root_surface.apply_pending_tree_state(&mut state));
        }

        let mut retired = Vec::new();
        let mut snapshots = Vec::new();
        collect_surface_snapshots(&mut state, root, 0, 0, &mut retired, &mut snapshots);
        let ids: Vec<_> = snapshots
            .iter()
            .map(|snapshot| snapshot.buffer_object_id)
            .collect();
        assert_eq!(ids, vec![ObjectId::from(121usize), ObjectId::from(120usize)]);
    }

    #[test]
    fn collect_surface_snapshots_accumulates_nested_subsurface_offsets() {
        let mut state = make_state();
        let root = insert_surface(&mut state, 1);
        let child = insert_surface(&mut state, 2);
        let grandchild = insert_surface(&mut state, 3);

        {
            let root_surface = state
                .objects
                .get_mut(root)
                .downcast_mut::<WlSurface>()
                .unwrap();
            root_surface.test_map(ObjectId::from(110usize), 100, 200, 40, 30);
            root_surface.add_pending_child(child);
        }
        {
            let child_surface = state
                .objects
                .get_mut(child)
                .downcast_mut::<WlSurface>()
                .unwrap();
            child_surface.assign_subsurface(WlSubsurfaceId::from(21), root);
            child_surface.set_subsurface_position(5, 7);
            child_surface.test_map(ObjectId::from(111usize), 1, 2, 20, 10);
            child_surface.add_pending_child(grandchild);
        }
        {
            let grandchild_surface = state
                .objects
                .get_mut(grandchild)
                .downcast_mut::<WlSurface>()
                .unwrap();
            grandchild_surface.assign_subsurface(WlSubsurfaceId::from(22), child);
            grandchild_surface.set_subsurface_position(11, 13);
            grandchild_surface.test_map(ObjectId::from(112usize), 3, 4, 8, 6);
        }
        {
            let mut root_surface = state.objects.grab_object::<WlSurface>(root);
            assert!(root_surface.apply_pending_tree_state(&mut state));
        }
        {
            let mut child_surface = state.objects.grab_object::<WlSurface>(child);
            assert!(child_surface.apply_pending_tree_state(&mut state));
        }

        let mut retired = Vec::new();
        let mut snapshots = Vec::new();
        collect_surface_snapshots(&mut state, root, 0, 0, &mut retired, &mut snapshots);

        let positions: Vec<_> = snapshots
            .iter()
            .map(|snapshot| (snapshot.x, snapshot.y))
            .collect();
        assert_eq!(positions, vec![(120, 226), (106, 209), (100, 200)]);
    }

    #[test]
    fn collect_cursor_surface_snapshots_walks_cursor_subsurface_tree() {
        let mut state = make_state();
        let root = insert_surface(&mut state, 1);
        let child = insert_surface(&mut state, 2);

        {
            let root_surface = state
                .objects
                .get_mut(root)
                .downcast_mut::<WlSurface>()
                .unwrap();
            root_surface.test_map(ObjectId::from(130usize), 10, 15, 20, 20);
            root_surface.add_pending_child(child);
            assert!(root_surface.restack_child_relative(child, None, true));
        }
        {
            let child_surface = state
                .objects
                .get_mut(child)
                .downcast_mut::<WlSurface>()
                .unwrap();
            child_surface.assign_subsurface(WlSubsurfaceId::from(81), root);
            child_surface.set_subsurface_position(4, 5);
            child_surface.test_map(ObjectId::from(131usize), 0, 0, 8, 9);
        }
        {
            let mut root_surface = state.objects.grab_object::<WlSurface>(root);
            assert!(root_surface.apply_pending_tree_state(&mut state));
        }

        state
            .seat
            .set_cursor_surface(ClientId::from_raw(1), WlSurfaceId::from(1), root, 3, 4);
        let snapshots = collect_cursor_surface_snapshots(&mut state, &mut Vec::new());

        let ids: Vec<_> = snapshots
            .iter()
            .map(|snapshot| snapshot.surface_id)
            .collect();
        assert_eq!(ids, vec![WlSurfaceId::from(1), WlSurfaceId::from(2)]);
    }
}
