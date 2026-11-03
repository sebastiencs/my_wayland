//! Shared lifecycle and SHM capture queue for both screenshot protocols.
//!
//! Output and toplevel copies record pixels, damage, and completion here. A painted screencopy
//! can blend the cursor into its own buffer. This registry also tracks which
//! protocol objects must stop when an output, toplevel, or pointer disappears, and completes each
//! queued copy once. The five capture globals are advertised at their implemented versions. Hyprland's screencopy
//! permission prompt is intentionally not copied.

use std::collections::{BTreeMap, BTreeSet};

use crate::{
    clients::{Client, ClientAction, ClientDispatcher, ClientId, Resolved},
    protocol_generated::{
        ExtForeignToplevelHandleV1Id, ExtForeignToplevelListV1Id, ExtImageCopyCaptureFrameV1Id,
        ExtImageCopyCaptureSessionV1Id, WlBufferId, WlPointerId, WlSurfaceId, XdgToplevelId,
        ZwlrScreencopyFrameV1Id,
    },
    region::RectI,
    runner::Dispatcher,
    state::{OutputId, State},
    wayland_objects::ext_foreign_toplevel_handle_v1::ExtForeignToplevelHandleV1,
};

/// Stable mapped-toplevel identity. Values are never reused after [`close_toplevel`].
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct ToplevelKey(u64);

impl ToplevelKey {
    pub fn identifier(self) -> String {
        format!("{:08x}", self.0)
    }
}

/// What an ext session or screencopy frame is capturing.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CaptureTarget {
    Output(OutputId),
    Toplevel(ToplevelKey),
}

/// Cursor inclusion is recorded per job. A painted copy can receive the cursor in its own
/// buffer; a hidden copy and the scanout image stay unchanged.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum CaptureCursor {
    Hidden,
    Painted,
}

/// Pixels inside the output framebuffer. Region captures are translated to this space.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CapturePixelRect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

/// What a finished output copy recorded for a later protocol event.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CaptureSample {
    pub rect: CapturePixelRect,
    pub stride: usize,
    pub flags: u32,
    pub tv_sec_hi: u32,
    pub tv_sec_lo: u32,
    pub tv_nsec: u32,
    pub damage: Vec<CapturePixelRect>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CaptureTimestamp {
    pub sec: u64,
    pub nsec: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct CaptureJobId(u64);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CaptureJobRequest {
    pub target: CaptureTarget,
    pub buffer: Resolved<WlBufferId>,
    pub width: i32,
    pub height: i32,
    pub cursor: CaptureCursor,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CaptureJobStatus {
    Queued,
    Written,
    Failed,
}

#[derive(Clone, Debug)]
struct CaptureJob {
    buffer: Resolved<WlBufferId>,
    target: CaptureTarget,
    width: i32,
    height: i32,
    cursor: CaptureCursor,
    outputs: Vec<OutputId>,
    status: CaptureJobStatus,
    /// Output framebuffer rect. Absent for a copy that the renderer does not sample.
    pixel_rect: Option<CapturePixelRect>,
    logical_region: Option<(i32, i32, i32, i32)>,
    with_damage: bool,
}

#[derive(Clone, Debug)]
struct ToplevelRecord {
    closed: bool,
    title: String,
    app_id: String,
    /// Keyboard focus is on this toplevel or one of its popups.
    activated: bool,
    /// `Maximized` bit last sent in `xdg_toplevel.configure`, including the tiled hint.
    maximized: bool,
    fullscreen: bool,
    /// Outputs whose `wl_surface.enter` set currently contains this toplevel.
    outputs: BTreeSet<OutputId>,
    /// Mapped XDG parent. Absent when the parent is unset or not currently mapped.
    parent: Option<ToplevelKey>,
}

/// A client watching mapped toplevels. Ext lists ignore shell, output, and parent updates.
/// wlr managers join this list when that protocol is implemented.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ToplevelSubscriber {
    ExtList(Resolved<ExtForeignToplevelListV1Id>),
}

impl ToplevelSubscriber {
    fn client_id(self) -> ClientId {
        match self {
            Self::ExtList(list) => list.client_id(),
        }
    }

    fn ext_list(self) -> Option<Resolved<ExtForeignToplevelListV1Id>> {
        match self {
            Self::ExtList(list) => Some(list),
        }
    }

    fn is_finished(self, state: &State) -> bool {
        match self {
            Self::ExtList(list) => list.try_ref(state).is_none_or(|list| list.is_finished()),
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct SessionSlot {
    session: Resolved<ExtImageCopyCaptureSessionV1Id>,
    target: CaptureTarget,
    pointer: Option<Resolved<WlPointerId>>,
}

#[derive(Clone, Copy, Debug)]
struct ExtFrameSlot {
    frame: Resolved<ExtImageCopyCaptureFrameV1Id>,
    target: CaptureTarget,
    pointer: Option<Resolved<WlPointerId>>,
    job: Option<CaptureJobId>,
    reported: bool,
}

#[derive(Clone, Copy, Debug)]
struct ScreencopySlot {
    frame: Resolved<ZwlrScreencopyFrameV1Id>,
    output_id: OutputId,
    job: Option<CaptureJobId>,
    reported: bool,
}

#[derive(Clone, Debug)]
enum Pending {
    StopSession(Resolved<ExtImageCopyCaptureSessionV1Id>),
    FailExtFrame(Resolved<ExtImageCopyCaptureFrameV1Id>),
    FinishExtFrame {
        frame: Resolved<ExtImageCopyCaptureFrameV1Id>,
        job: CaptureJobId,
    },
    CursorLeft(Resolved<crate::protocol_generated::ExtImageCopyCaptureCursorSessionV1Id>),
    FailScreencopy(Resolved<ZwlrScreencopyFrameV1Id>),
    FinishScreencopy {
        frame: Resolved<ZwlrScreencopyFrameV1Id>,
        job: CaptureJobId,
    },
    CloseHandle(Resolved<ExtForeignToplevelHandleV1Id>),
    Announce {
        key: ToplevelKey,
        title: String,
        app_id: String,
    },
    Retitle {
        key: ToplevelKey,
        title: String,
    },
    Reapp {
        key: ToplevelKey,
        app_id: String,
    },
    /// Full shell state. Ext lists do not send this.
    ShellState {
        key: ToplevelKey,
        activated: bool,
        maximized: bool,
        fullscreen: bool,
    },
    OutputEntered {
        key: ToplevelKey,
        output: OutputId,
    },
    OutputLeft {
        key: ToplevelKey,
        output: OutputId,
    },
    ParentChanged {
        key: ToplevelKey,
        parent: Option<ToplevelKey>,
    },
}

#[derive(Debug, Default)]
pub struct CaptureRegistry {
    next_toplevel: u64,
    toplevels: BTreeMap<ToplevelKey, ToplevelRecord>,
    /// Mapped XDG toplevels. A new map after unmap gets a new [`ToplevelKey`].
    by_xdg: BTreeMap<Resolved<XdgToplevelId>, ToplevelKey>,
    /// Lists that already received a handle for a key. Destroying that handle does not create
    /// another one until the toplevel is unmapped and mapped again.
    announced: BTreeSet<(Resolved<ExtForeignToplevelListV1Id>, ToplevelKey)>,
    sessions: Vec<SessionSlot>,
    ext_frames: Vec<ExtFrameSlot>,
    screencopy: Vec<ScreencopySlot>,
    subscribers: Vec<ToplevelSubscriber>,
    handles: Vec<(Resolved<ExtForeignToplevelHandleV1Id>, ToplevelKey)>,
    pending: BTreeMap<ClientId, Vec<Pending>>,
    next_job: u64,
    jobs: BTreeMap<CaptureJobId, CaptureJob>,
    samples: BTreeMap<CaptureJobId, CaptureSample>,
    /// Damage since the previous `with_damage` copy, keyed by client, output, and cursor mode.
    damage: BTreeMap<(ClientId, OutputId, CaptureCursor), ConsumerDamage>,
    /// Same accumulator for a toplevel image. Hidden and painted consumers do not share it.
    toplevel_damage: BTreeMap<(ClientId, ToplevelKey, CaptureCursor), ConsumerDamage>,
}

#[derive(Clone, Debug, Default)]
struct ConsumerDamage {
    started: bool,
    rects: Vec<CapturePixelRect>,
}

impl CaptureRegistry {
    pub(crate) fn track_session(
        &mut self,
        session: Resolved<ExtImageCopyCaptureSessionV1Id>,
        target: CaptureTarget,
        pointer: Option<Resolved<WlPointerId>>,
    ) {
        self.sessions.push(SessionSlot { session, target, pointer });
    }

    pub(crate) fn track_ext_frame(
        &mut self,
        frame: Resolved<ExtImageCopyCaptureFrameV1Id>,
        target: CaptureTarget,
        pointer: Option<Resolved<WlPointerId>>,
    ) {
        self.ext_frames
            .push(ExtFrameSlot { frame, target, pointer, job: None, reported: false });
    }

    pub(crate) fn bind_ext_frame_job(
        &mut self,
        frame: ExtImageCopyCaptureFrameV1Id,
        job: CaptureJobId,
    ) {
        if let Some(slot) = self
            .ext_frames
            .iter_mut()
            .find(|slot| slot.frame.wayland_id() == frame)
        {
            slot.job = Some(job);
        }
    }

    pub(crate) fn track_screencopy(
        &mut self,
        frame: Resolved<ZwlrScreencopyFrameV1Id>,
        output_id: OutputId,
    ) {
        self.screencopy
            .push(ScreencopySlot { frame, output_id, job: None, reported: false });
    }

    pub(crate) fn bind_screencopy_job(
        &mut self,
        frame: ZwlrScreencopyFrameV1Id,
        job: CaptureJobId,
    ) {
        if let Some(slot) = self
            .screencopy
            .iter_mut()
            .find(|slot| slot.frame.wayland_id() == frame)
        {
            slot.job = Some(job);
        }
    }

    pub(crate) fn track_list(&mut self, list: Resolved<ExtForeignToplevelListV1Id>) {
        self.subscribers.push(ToplevelSubscriber::ExtList(list));
    }

    pub(crate) fn track_handle(
        &mut self,
        handle: Resolved<ExtForeignToplevelHandleV1Id>,
        key: ToplevelKey,
    ) {
        self.handles.push((handle, key));
    }

    pub(crate) fn forget_session(&mut self, session: Resolved<ExtImageCopyCaptureSessionV1Id>) {
        self.sessions.retain(|slot| slot.session != session);
        self.retain_pending(|item| !matches!(item, Pending::StopSession(id) if *id == session));
    }

    pub(crate) fn forget_ext_frame(&mut self, frame: Resolved<ExtImageCopyCaptureFrameV1Id>) {
        self.ext_frames.retain(|slot| slot.frame != frame);
        self.retain_pending(|item| !matches!(item, Pending::FailExtFrame(id) if *id == frame));
    }

    pub(crate) fn forget_screencopy(&mut self, frame: Resolved<ZwlrScreencopyFrameV1Id>) {
        self.screencopy.retain(|slot| slot.frame != frame);
        self.retain_pending(|item| !matches!(item, Pending::FailScreencopy(id) if *id == frame));
    }

    pub(crate) fn forget_list(&mut self, list: Resolved<ExtForeignToplevelListV1Id>) {
        self.subscribers
            .retain(|subscriber| subscriber.ext_list() != Some(list));
        self.announced.retain(|(candidate, _)| *candidate != list);
    }

    pub(crate) fn forget_handle(&mut self, handle: Resolved<ExtForeignToplevelHandleV1Id>) {
        self.handles.retain(|(candidate, _)| *candidate != handle);
        self.retain_pending(|item| !matches!(item, Pending::CloseHandle(id) if *id == handle));
    }

    /// Drop every capture object and queued event owned by a disconnected client.
    pub fn drop_client(&mut self, client_id: ClientId) {
        self.sessions
            .retain(|slot| slot.session.client_id() != client_id);
        self.ext_frames
            .retain(|slot| slot.frame.client_id() != client_id);
        self.screencopy
            .retain(|slot| slot.frame.client_id() != client_id);
        self.subscribers
            .retain(|subscriber| subscriber.client_id() != client_id);
        self.handles
            .retain(|(handle, _)| handle.client_id() != client_id);
        self.announced
            .retain(|(list, _)| list.client_id() != client_id);
        self.jobs
            .retain(|_, job| job.buffer.client_id() != client_id);
        self.samples.retain(|id, _| self.jobs.contains_key(id));
        self.damage.retain(|(client, _, _), _| *client != client_id);
        self.toplevel_damage
            .retain(|(client, _, _), _| *client != client_id);
        self.pending.remove(&client_id);
    }

    pub(crate) fn toplevel_is_open(&self, key: ToplevelKey) -> bool {
        self.toplevels
            .get(&key)
            .is_some_and(|record| !record.closed)
    }

    pub(crate) fn handles_for(
        &self,
        key: ToplevelKey,
    ) -> Vec<Resolved<ExtForeignToplevelHandleV1Id>> {
        self.handles
            .iter()
            .filter(|(_, candidate)| *candidate == key)
            .map(|(handle, _)| *handle)
            .collect()
    }

    fn retain_pending(&mut self, mut keep: impl FnMut(&Pending) -> bool) {
        for queue in self.pending.values_mut() {
            queue.retain(|item| keep(item));
        }
        self.pending.retain(|_, queue| !queue.is_empty());
    }

    fn enqueue(&mut self, client_id: ClientId, item: Pending) {
        self.pending.entry(client_id).or_default().push(item);
    }
}

pub(crate) fn target_is_live(state: &State, target: CaptureTarget) -> bool {
    match target {
        CaptureTarget::Output(output_id) => state.outputs.try_get(output_id).is_some(),
        CaptureTarget::Toplevel(key) => state.capture.toplevel_is_open(key),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CaptureQueueError {
    SourceGone,
    Buffer(crate::wayland_objects::wl_buffer::CaptureBufferError),
    BufferBusy,
    EmptyRegion,
    /// The session is protected, so a toplevel image would expose the hidden desktop.
    Protected,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CaptureFinish {
    Written,
    Failed,
    AlreadyFinished,
}

/// Queue one capture, check its destination, and ask the relevant output for a frame.
///
/// Completion writes the supplied top-left pixels once. A missing source, a second completion, or
/// a buffer that no longer matches fails without writing again.
pub(crate) fn submit_capture(
    state: &mut State,
    dispatcher: &mut Dispatcher,
    request: CaptureJobRequest,
) -> Result<CaptureJobId, CaptureQueueError> {
    if !target_is_live(state, request.target) {
        return Err(CaptureQueueError::SourceGone);
    }
    if state
        .capture
        .jobs
        .values()
        .any(|job| job.status == CaptureJobStatus::Queued && job.buffer == request.buffer)
    {
        return Err(CaptureQueueError::BufferBusy);
    }
    let Some(buffer) = request.buffer.try_ref(state) else {
        return Err(CaptureQueueError::Buffer(
            crate::wayland_objects::wl_buffer::CaptureBufferError::Destroyed,
        ));
    };
    buffer
        .begin_capture_write(request.width, request.height)
        .map_err(CaptureQueueError::Buffer)?;
    let outputs = schedule_capture(state, dispatcher, request.target);
    state.capture.next_job = state.capture.next_job.saturating_add(1);
    let id = CaptureJobId(state.capture.next_job);
    state.capture.jobs.insert(
        id,
        CaptureJob {
            buffer: request.buffer,
            target: request.target,
            width: request.width,
            height: request.height,
            cursor: request.cursor,
            outputs,
            status: CaptureJobStatus::Queued,
            pixel_rect: None,
            logical_region: None,
            with_damage: false,
        },
    );
    Ok(id)
}

pub(crate) fn capture_job_cursor(state: &State, job_id: CaptureJobId) -> Option<CaptureCursor> {
    state.capture.jobs.get(&job_id).map(|job| job.cursor)
}

pub(crate) fn queued_jobs_for_output(state: &State, output_id: OutputId) -> Vec<CaptureJobId> {
    state
        .capture
        .jobs
        .iter()
        .filter(|(_, job)| {
            job.status == CaptureJobStatus::Queued && job.outputs.contains(&output_id)
        })
        .map(|(id, _)| *id)
        .collect()
}

pub(crate) fn complete_capture(
    state: &mut State,
    job_id: CaptureJobId,
    src: &[u8],
    src_stride: usize,
) -> CaptureFinish {
    let Some(job) = state.capture.jobs.get(&job_id) else {
        return CaptureFinish::AlreadyFinished;
    };
    if job.status != CaptureJobStatus::Queued {
        return CaptureFinish::AlreadyFinished;
    }
    let buffer = job.buffer;
    let width = job.width;
    let height = job.height;
    let target = job.target;
    if !target_is_live(state, target) {
        if let Some(job) = state.capture.jobs.get_mut(&job_id) {
            job.status = CaptureJobStatus::Failed;
        }
        return CaptureFinish::Failed;
    }
    let written = buffer.try_ref(state).is_some_and(|buffer| {
        buffer
            .begin_capture_write(width, height)
            .and_then(|write| write.write_top_left(src, src_stride))
            .is_ok()
    });
    if let Some(job) = state.capture.jobs.get_mut(&job_id) {
        job.status = if written { CaptureJobStatus::Written } else { CaptureJobStatus::Failed };
    }
    if written {
        CaptureFinish::Written
    } else {
        CaptureFinish::Failed
    }
}

pub(crate) fn fail_capture(state: &mut State, job_id: CaptureJobId) -> CaptureFinish {
    let Some(job) = state.capture.jobs.get_mut(&job_id) else {
        return CaptureFinish::AlreadyFinished;
    };
    if job.status != CaptureJobStatus::Queued {
        return CaptureFinish::AlreadyFinished;
    }
    job.status = CaptureJobStatus::Failed;
    CaptureFinish::Failed
}

fn fail_queued_jobs(state: &mut State, target: CaptureTarget) {
    for job in state.capture.jobs.values_mut() {
        if job.status == CaptureJobStatus::Queued && job.target == target {
            job.status = CaptureJobStatus::Failed;
        }
    }
}

fn schedule_capture(
    state: &State,
    dispatcher: &mut Dispatcher,
    target: CaptureTarget,
) -> Vec<OutputId> {
    match target {
        CaptureTarget::Output(output_id) => {
            crate::drm::request_outputs(state, [output_id], dispatcher);
            vec![output_id]
        }
        CaptureTarget::Toplevel(key) => {
            let outputs = outputs_for_toplevel(state, key);
            if outputs.is_empty() {
                crate::drm::request_active_output(state, dispatcher);
            } else {
                crate::drm::request_outputs(state, outputs.iter().copied(), dispatcher);
            }
            outputs
        }
    }
}

fn outputs_for_toplevel(state: &State, key: ToplevelKey) -> Vec<OutputId> {
    let Some(toplevel) = state
        .capture
        .by_xdg
        .iter()
        .find(|(_, candidate)| **candidate == key)
        .map(|(toplevel, _)| *toplevel)
    else {
        return Vec::new();
    };
    let Some(surface) = toplevel
        .try_ref(state)
        .and_then(|toplevel| toplevel.xdg_surface().try_ref(state))
        .map(|xdg_surface| xdg_surface.wl_surface())
    else {
        return Vec::new();
    };
    let Some(bounds) = surface
        .try_ref(state)
        .and_then(|surface| surface.global_logical_bounds())
    else {
        return Vec::new();
    };
    state.outputs_intersecting_rect(bounds)
}

pub(crate) fn output_sample_pending(capture: &CaptureRegistry, output_id: OutputId) -> bool {
    capture.jobs.values().any(|job| {
        job.status == CaptureJobStatus::Queued
            && job.target == CaptureTarget::Output(output_id)
            && job.pixel_rect.is_some()
    })
}

pub(crate) fn output_capture_pending(state: &State, output_id: OutputId) -> bool {
    output_sample_pending(&state.capture, output_id)
}

/// Queue an output copy whose size is the mode buffer, or the clipped logical region in buffer pixels.
pub(crate) fn submit_output_capture(
    state: &mut State,
    dispatcher: &mut Dispatcher,
    output_id: OutputId,
    buffer: Resolved<WlBufferId>,
    cursor: CaptureCursor,
    region: Option<(i32, i32, i32, i32)>,
    with_damage: bool,
) -> Result<CaptureJobId, CaptureQueueError> {
    let Some(output) = state.outputs.try_get(output_id) else {
        return Err(CaptureQueueError::SourceGone);
    };
    let Some(rect) = output_capture_rect(
        output.current_mode.width,
        output.current_mode.height,
        output.scale,
        output.transform,
        region,
    ) else {
        return Err(CaptureQueueError::EmptyRegion);
    };
    let id = submit_capture(
        state,
        dispatcher,
        CaptureJobRequest {
            target: CaptureTarget::Output(output_id),
            buffer,
            width: rect.width,
            height: rect.height,
            cursor,
        },
    )?;
    if let Some(job) = state.capture.jobs.get_mut(&id) {
        job.pixel_rect = Some(rect);
        job.logical_region = region;
        job.with_damage = with_damage;
    }
    Ok(id)
}

pub(crate) fn capture_sample(state: &State, job_id: CaptureJobId) -> Option<&CaptureSample> {
    state.capture.samples.get(&job_id)
}

/// Send `flags`, `damage`, and `ready` — or `failed` — for screencopy jobs that already finished.
pub(crate) fn publish_screencopy(state: &mut State, dispatcher: &mut Dispatcher) {
    let finished: Vec<_> = state
        .capture
        .screencopy
        .iter()
        .filter_map(|slot| {
            if slot.reported {
                return None;
            }
            let job = slot.job?;
            let status = state.capture.jobs.get(&job)?.status;
            if status == CaptureJobStatus::Queued {
                return None;
            }
            Some((slot.frame, job))
        })
        .collect();
    let mut clients = BTreeSet::new();
    for (frame, job) in finished {
        if let Some(slot) = state
            .capture
            .screencopy
            .iter_mut()
            .find(|slot| slot.frame == frame)
        {
            slot.reported = true;
        }
        let client_id = frame.client_id();
        state
            .capture
            .enqueue(client_id, Pending::FinishScreencopy { frame, job });
        clients.insert(client_id);
    }
    dispatch_deliver(dispatcher, clients);
}

/// Send ext-image-copy `damage`, `transform`, `presentation_time`, and `ready`, or `failed`.
pub(crate) fn publish_ext_frames(state: &mut State, dispatcher: &mut Dispatcher) {
    let finished: Vec<_> = state
        .capture
        .ext_frames
        .iter()
        .filter_map(|slot| {
            if slot.reported {
                return None;
            }
            let job = slot.job?;
            let status = state.capture.jobs.get(&job)?.status;
            if status == CaptureJobStatus::Queued {
                return None;
            }
            Some((slot.frame, job))
        })
        .collect();
    let mut clients = BTreeSet::new();
    for (frame, job) in finished {
        if let Some(slot) = state
            .capture
            .ext_frames
            .iter_mut()
            .find(|slot| slot.frame == frame)
        {
            slot.reported = true;
        }
        let client_id = frame.client_id();
        state
            .capture
            .enqueue(client_id, Pending::FinishExtFrame { frame, job });
        clients.insert(client_id);
    }
    dispatch_deliver(dispatcher, clients);
}

/// Pixel size a client must allocate for this session. Cursor sessions use the cursor image.
fn ext_failure_reason(state: &State, job_id: CaptureJobId) -> u32 {
    use crate::protocol_generated::ext_image_copy_capture_frame_v1::FailureReason;
    let Some(job) = state.capture.jobs.get(&job_id) else {
        return FailureReason::Unknown as u32;
    };
    if !target_is_live(state, job.target) {
        return FailureReason::Stopped as u32;
    }
    if let Some((width, height)) = image_copy_buffer_size(state, job.target, false) {
        if job.width != width || job.height != height {
            return FailureReason::BufferConstraints as u32;
        }
    }
    FailureReason::Unknown as u32
}

pub(crate) fn image_copy_buffer_size(
    state: &State,
    target: CaptureTarget,
    cursor: bool,
) -> Option<(i32, i32)> {
    if cursor {
        let placement = cursor_placement(state, target)?;
        return Some((placement.image.width, placement.image.height));
    }
    match target {
        CaptureTarget::Output(output_id) => {
            let output = state.outputs.try_get(output_id)?;
            let rect = output_capture_rect(
                output.current_mode.width,
                output.current_mode.height,
                output.scale,
                output.transform,
                None,
            )?;
            Some((rect.width, rect.height))
        }
        CaptureTarget::Toplevel(key) => {
            let bounds = toplevel_rendered_bounds(state, key)?;
            Some((bounds.width, bounds.height))
        }
    }
}

/// Cursor image in output-local or toplevel-local coordinates. `argb` is little-endian BGRA.
#[derive(Clone, Debug)]
pub(crate) struct CursorSnapshot {
    pub width: i32,
    pub height: i32,
    pub hotspot_x: i32,
    pub hotspot_y: i32,
    pub stride: usize,
    pub argb: Vec<u8>,
    pub origin_x: i32,
    pub origin_y: i32,
}

#[derive(Clone, Debug)]
pub(crate) struct CursorPlacement {
    pub image: CursorSnapshot,
    pub inside: bool,
    /// Hotspot in the source buffer, which may lie outside that buffer.
    pub position: (i32, i32),
}

pub(crate) fn cursor_placement(state: &State, target: CaptureTarget) -> Option<CursorPlacement> {
    if state.session_lock.input_gated() {
        return None;
    }
    let image = cursor_snapshot(state)?;
    let position = cursor_hotspot_position(state, target, &image)?;
    let cursor_rect = RectI::new(image.origin_x, image.origin_y, image.width, image.height)?;
    let inside = source_bounds(state, target).is_some_and(|bounds| cursor_rect.intersects(bounds));
    Some(CursorPlacement { image, inside, position })
}

/// Blend a painted cursor into a toplevel capture. The desktop frame is not modified.
pub(crate) fn paint_toplevel_cursor(
    state: &State,
    key: ToplevelKey,
    bytes: &mut [u8],
    stride: usize,
) {
    if state.session_lock.input_gated() {
        return;
    }
    let Some(bounds) = toplevel_rendered_bounds(state, key) else {
        return;
    };
    let Some(image) = cursor_snapshot(state) else {
        return;
    };
    let cursor = CaptureCursorImage {
        local_x: image.origin_x.saturating_sub(bounds.x),
        local_y: image.origin_y.saturating_sub(bounds.y),
        width: image.width,
        height: image.height,
        stride: image.stride,
        argb: image.argb,
        mode_width: bounds.width,
        mode_height: bounds.height,
        scale: crate::state::FractionalScale::ONE,
        transform: 0,
    };
    blend_cursor_into_copy(
        bytes,
        stride,
        CapturePixelRect { x: 0, y: 0, width: bounds.width, height: bounds.height },
        &cursor,
    );
}

fn cursor_snapshot(state: &State) -> Option<CursorSnapshot> {
    let (pointer_x, pointer_y) = state.seat.pointer_position();
    let pointer_x = pointer_x.round() as i32;
    let pointer_y = pointer_y.round() as i32;
    match state.seat.visible_cursor_source()? {
        crate::cursor::CursorSourceRef::Named { image, .. } => {
            let width = image.width;
            let height = image.height;
            if width <= 0 || height <= 0 {
                return None;
            }
            Some(CursorSnapshot {
                width,
                height,
                hotspot_x: image.hotspot_x,
                hotspot_y: image.hotspot_y,
                stride: (width as usize).saturating_mul(4),
                argb: rgba_to_argb(&image.pixels_rgba, width as usize, height as usize),
                origin_x: pointer_x.saturating_sub(image.hotspot_x),
                origin_y: pointer_y.saturating_sub(image.hotspot_y),
            })
        }
        crate::cursor::CursorSourceRef::Surface { cursor } => {
            let surface = cursor.surface.try_ref(state)?;
            let render = surface.render_state_at(0, 0)?;
            if render.tex_transform != crate::transform::Mat3::identity() {
                return None;
            }
            let buffer = render.buffer.try_ref(state)?;
            let shm = buffer.shm_cursor_argb8888()?;
            let argb = shm.argb_bytes();
            if argb.is_empty() || render.width <= 0 || render.height <= 0 {
                return None;
            }
            Some(CursorSnapshot {
                width: render.width,
                height: render.height,
                hotspot_x: cursor.hotspot_x,
                hotspot_y: cursor.hotspot_y,
                stride: shm.stride(),
                argb,
                origin_x: pointer_x.saturating_sub(cursor.hotspot_x),
                origin_y: pointer_y.saturating_sub(cursor.hotspot_y),
            })
        }
    }
}

fn cursor_hotspot_position(
    state: &State,
    target: CaptureTarget,
    image: &CursorSnapshot,
) -> Option<(i32, i32)> {
    let hotspot_x = image.origin_x.saturating_add(image.hotspot_x);
    let hotspot_y = image.origin_y.saturating_add(image.hotspot_y);
    match target {
        CaptureTarget::Toplevel(key) => {
            let bounds = toplevel_rendered_bounds(state, key)?;
            Some((hotspot_x.saturating_sub(bounds.x), hotspot_y.saturating_sub(bounds.y)))
        }
        CaptureTarget::Output(output_id) => {
            let output = state.outputs.try_get(output_id)?;
            let local_x = hotspot_x.saturating_sub(output.x);
            let local_y = hotspot_y.saturating_sub(output.y);
            let wayland = crate::transform::BufferTransform::from_wayland(output.transform)
                .unwrap_or(crate::transform::BufferTransform::Normal);
            let (buffer_width, buffer_height) =
                wayland.maybe_swap((output.current_mode.width, output.current_mode.height));
            let logical_width = output.scale.logical_length(buffer_width);
            let logical_height = output.scale.logical_length(buffer_height);
            let (x, y, _, _) = transform_output_box(
                local_x,
                local_y,
                1,
                1,
                invert_output_transform(output.transform),
                logical_width,
                logical_height,
            )?;
            Some((
                scale_logical_to_buffer(x, output.scale),
                scale_logical_to_buffer(y, output.scale),
            ))
        }
    }
}

fn source_bounds(state: &State, target: CaptureTarget) -> Option<RectI> {
    match target {
        CaptureTarget::Output(output_id) => state.outputs.try_get(output_id)?.logical_bounds(),
        CaptureTarget::Toplevel(key) => toplevel_rendered_bounds(state, key),
    }
}

fn rgba_to_argb(src: &[u8], width: usize, height: usize) -> Vec<u8> {
    let stride = width.saturating_mul(4);
    let mut dst = vec![0u8; stride.saturating_mul(height)];
    for row in 0..height {
        let start = row.saturating_mul(stride);
        let Some(src_row) = src.get(start..start + stride) else {
            break;
        };
        let dst_row = &mut dst[start..start + stride];
        for (src_px, dst_px) in src_row.chunks_exact(4).zip(dst_row.chunks_exact_mut(4)) {
            dst_px[0] = src_px[2];
            dst_px[1] = src_px[1];
            dst_px[2] = src_px[0];
            dst_px[3] = src_px[3];
        }
    }
    dst
}

/// Blend `cursor` over one capture image. The destination is premultiplied BGRA from the
/// framebuffer read. The cursor stays out of every other buffer, including scanout.
pub(crate) fn blend_cursor_into_copy(
    bytes: &mut [u8],
    dst_stride: usize,
    rect: CapturePixelRect,
    cursor: &CaptureCursorImage,
) {
    let Some(oriented) = orient_cursor(cursor) else {
        return;
    };
    let Some(place) = cursor_buffer_rect(cursor) else {
        return;
    };
    if place.width <= 0 || place.height <= 0 || rect.width <= 0 || rect.height <= 0 {
        return;
    }
    for row in 0..rect.height {
        for column in 0..rect.width {
            let buffer_x = rect.x.saturating_add(column);
            let buffer_y = rect.y.saturating_add(row);
            if buffer_x < place.x
                || buffer_y < place.y
                || buffer_x >= place.x.saturating_add(place.width)
                || buffer_y >= place.y.saturating_add(place.height)
            {
                continue;
            }
            let src_x = (i64::from(buffer_x - place.x) * i64::from(oriented.width))
                .div_euclid(i64::from(place.width));
            let src_y = (i64::from(buffer_y - place.y) * i64::from(oriented.height))
                .div_euclid(i64::from(place.height));
            if src_x < 0
                || src_y < 0
                || src_x >= i64::from(oriented.width)
                || src_y >= i64::from(oriented.height)
            {
                continue;
            }
            let src_index = (src_y as usize)
                .saturating_mul(oriented.stride)
                .saturating_add((src_x as usize).saturating_mul(4));
            let dst_index = (row as usize)
                .saturating_mul(dst_stride)
                .saturating_add((column as usize).saturating_mul(4));
            let Some(src) = oriented.argb.get(src_index..src_index + 4) else {
                continue;
            };
            let Some(dst) = bytes.get_mut(dst_index..dst_index + 4) else {
                continue;
            };
            blend_premultiplied(dst, src);
        }
    }
}

struct OrientedCursor {
    width: i32,
    height: i32,
    stride: usize,
    argb: Vec<u8>,
}

fn orient_cursor(cursor: &CaptureCursorImage) -> Option<OrientedCursor> {
    if cursor.width <= 0 || cursor.height <= 0 {
        return None;
    }
    let width = cursor.width as usize;
    let height = cursor.height as usize;
    let needed = cursor.stride.saturating_mul(height);
    if cursor.argb.len() < needed || cursor.stride < width.saturating_mul(4) {
        return None;
    }
    let inverted = invert_output_transform(cursor.transform);
    let (dest_width, dest_height) = if inverted % 2 == 0 {
        (cursor.width, cursor.height)
    } else {
        (cursor.height, cursor.width)
    };
    let dest_stride = (dest_width as usize).saturating_mul(4);
    let mut argb = vec![0u8; dest_stride.saturating_mul(dest_height as usize)];
    for sy in 0..cursor.height {
        for sx in 0..cursor.width {
            let (dx, dy, _, _) =
                transform_output_box(sx, sy, 1, 1, inverted, cursor.width, cursor.height)?;
            if dx < 0 || dy < 0 || dx >= dest_width || dy >= dest_height {
                continue;
            }
            let src_index = (sy as usize)
                .saturating_mul(cursor.stride)
                .saturating_add((sx as usize).saturating_mul(4));
            let dst_index = (dy as usize)
                .saturating_mul(dest_stride)
                .saturating_add((dx as usize).saturating_mul(4));
            argb[dst_index..dst_index + 4].copy_from_slice(&cursor.argb[src_index..src_index + 4]);
        }
    }
    Some(OrientedCursor {
        width: dest_width,
        height: dest_height,
        stride: dest_stride,
        argb,
    })
}

fn cursor_buffer_rect(cursor: &CaptureCursorImage) -> Option<CapturePixelRect> {
    if cursor.mode_width <= 0 || cursor.mode_height <= 0 {
        return None;
    }
    let wayland = crate::transform::BufferTransform::from_wayland(cursor.transform)
        .unwrap_or(crate::transform::BufferTransform::Normal);
    let (buffer_width, buffer_height) = wayland.maybe_swap((cursor.mode_width, cursor.mode_height));
    let logical_width = cursor.scale.logical_length(buffer_width);
    let logical_height = cursor.scale.logical_length(buffer_height);
    let (x, y, width, height) = transform_output_box(
        cursor.local_x,
        cursor.local_y,
        cursor.width,
        cursor.height,
        invert_output_transform(cursor.transform),
        logical_width,
        logical_height,
    )?;
    let x1 = scale_logical_to_buffer(x, cursor.scale);
    let y1 = scale_logical_to_buffer(y, cursor.scale);
    let x2 = scale_logical_to_buffer(x.saturating_add(width), cursor.scale);
    let y2 = scale_logical_to_buffer(y.saturating_add(height), cursor.scale);
    if x2 <= x1 || y2 <= y1 {
        return None;
    }
    Some(CapturePixelRect { x: x1, y: y1, width: x2 - x1, height: y2 - y1 })
}

fn blend_premultiplied(dst: &mut [u8], src: &[u8]) {
    let src_a = u32::from(src[3]);
    if src_a == 0 {
        return;
    }
    if src_a == 255 {
        dst.copy_from_slice(&src[..4]);
        return;
    }
    let inv = 255 - src_a;
    for channel in 0..3 {
        dst[channel] =
            ((u32::from(src[channel]) * src_a + u32::from(dst[channel]) * inv + 127) / 255) as u8;
    }
    dst[3] = ((src_a * 255 + u32::from(dst[3]) * inv + 127) / 255) as u8;
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct OutputCopySpec {
    pub job_id: CaptureJobId,
    pub rect: CapturePixelRect,
    pub stride: usize,
    pub cursor: CaptureCursor,
}

/// Cursor image in output-local logical coordinates. `argb` is little-endian BGRA, matching
/// the SHM capture buffer.
#[derive(Clone, Debug)]
pub(crate) struct CaptureCursorImage {
    pub local_x: i32,
    pub local_y: i32,
    pub width: i32,
    pub height: i32,
    pub stride: usize,
    pub argb: Vec<u8>,
    pub mode_width: i32,
    pub mode_height: i32,
    pub scale: crate::state::FractionalScale,
    pub transform: i32,
}

pub(crate) fn output_copy_specs(
    capture: &CaptureRegistry,
    objects: &crate::indexmap::IndexMap<crate::state::ObjectId, Box<dyn crate::object::Object>>,
    output_id: OutputId,
) -> Vec<OutputCopySpec> {
    let mut specs = Vec::new();
    for (job_id, job) in &capture.jobs {
        if job.status != CaptureJobStatus::Queued || job.target != CaptureTarget::Output(output_id)
        {
            continue;
        }
        let Some(rect) = job.pixel_rect else {
            continue;
        };
        let Some(buffer) = objects
            .try_ref_object::<crate::wayland_objects::wl_buffer::WlBuffer>(job.buffer.object_id())
        else {
            continue;
        };
        let Ok(write) = buffer.begin_capture_write(rect.width, rect.height) else {
            continue;
        };
        specs.push(OutputCopySpec {
            job_id: *job_id,
            rect,
            stride: write.stride_bytes(),
            cursor: job.cursor,
        });
    }
    specs
}

pub(crate) struct RenderedOutputCopy {
    pub job_id: CaptureJobId,
    pub bytes: Vec<u8>,
    pub stride: usize,
    pub read_ok: bool,
}

/// Write copies taken from the composed framebuffer. Each job finishes once.
pub(crate) fn apply_rendered_output(
    capture: &mut CaptureRegistry,
    objects: &crate::indexmap::IndexMap<crate::state::ObjectId, Box<dyn crate::object::Object>>,
    output_id: OutputId,
    mode_width: i32,
    mode_height: i32,
    scale: crate::state::FractionalScale,
    transform: i32,
    frame_damage: &[CapturePixelRect],
    copies: Vec<RenderedOutputCopy>,
    timestamp: CaptureTimestamp,
) {
    let touched: BTreeSet<_> = copies.iter().map(|copy| copy.job_id).collect();
    for copy in copies {
        let Some(job) = capture.jobs.get(&copy.job_id) else {
            continue;
        };
        if job.status != CaptureJobStatus::Queued || job.target != CaptureTarget::Output(output_id)
        {
            continue;
        }
        let stored = job.pixel_rect;
        let region = job.logical_region;
        let with_damage = job.with_damage;
        let cursor = job.cursor;
        let client_id = job.buffer.client_id();
        let buffer = job.buffer;
        let Some(stored) = stored else {
            continue;
        };
        let current = output_capture_rect(mode_width, mode_height, scale, transform, region);
        if current != Some(stored) || !copy.read_ok {
            if let Some(job) = capture.jobs.get_mut(&copy.job_id) {
                job.status = CaptureJobStatus::Failed;
            }
            continue;
        }
        let Some(buffer) = objects
            .try_ref_object::<crate::wayland_objects::wl_buffer::WlBuffer>(buffer.object_id())
        else {
            if let Some(job) = capture.jobs.get_mut(&copy.job_id) {
                job.status = CaptureJobStatus::Failed;
            }
            continue;
        };
        let written = buffer
            .begin_capture_write(stored.width, stored.height)
            .and_then(|write| write.write_top_left(&copy.bytes, copy.stride))
            .is_ok();
        if !written {
            if let Some(job) = capture.jobs.get_mut(&copy.job_id) {
                job.status = CaptureJobStatus::Failed;
            }
            continue;
        }
        let damage = take_capture_damage(
            capture,
            client_id,
            output_id,
            cursor,
            stored,
            with_damage,
            frame_damage,
        );
        if let Some(job) = capture.jobs.get_mut(&copy.job_id) {
            job.status = CaptureJobStatus::Written;
        }
        capture.samples.insert(
            copy.job_id,
            CaptureSample {
                rect: stored,
                stride: copy.stride,
                flags: 0,
                tv_sec_hi: (timestamp.sec >> 32) as u32,
                tv_sec_lo: timestamp.sec as u32,
                tv_nsec: timestamp.nsec,
                damage,
            },
        );
    }
    let leftovers: Vec<_> = capture
        .jobs
        .iter()
        .filter(|(id, job)| {
            !touched.contains(id)
                && job.status == CaptureJobStatus::Queued
                && job.target == CaptureTarget::Output(output_id)
                && job.pixel_rect.is_some()
        })
        .map(|(id, _)| *id)
        .collect();
    for job_id in leftovers {
        if let Some(job) = capture.jobs.get_mut(&job_id) {
            job.status = CaptureJobStatus::Failed;
        }
    }
}

pub(crate) fn capture_timestamp_now() -> CaptureTimestamp {
    let mut time = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut time) } != 0 {
        return CaptureTimestamp { sec: 0, nsec: 0 };
    }
    CaptureTimestamp { sec: time.tv_sec as u64, nsec: time.tv_nsec as u32 }
}

fn take_capture_damage(
    capture: &mut CaptureRegistry,
    client_id: ClientId,
    output_id: OutputId,
    cursor: CaptureCursor,
    rect: CapturePixelRect,
    with_damage: bool,
    frame_damage: &[CapturePixelRect],
) -> Vec<CapturePixelRect> {
    let entry = capture
        .damage
        .entry((client_id, output_id, cursor))
        .or_default();
    if entry.started {
        entry.rects.extend(frame_damage.iter().copied());
    }
    if !with_damage {
        return Vec::new();
    }
    let damage = if entry.started {
        clip_damage_to_capture(&entry.rects, rect)
    } else {
        vec![CapturePixelRect { x: 0, y: 0, width: rect.width, height: rect.height }]
    };
    entry.started = true;
    entry.rects.clear();
    damage
}

fn clip_damage_to_capture(
    rects: &[CapturePixelRect],
    capture: CapturePixelRect,
) -> Vec<CapturePixelRect> {
    rects
        .iter()
        .filter_map(|rect| {
            let x1 = rect.x.max(capture.x);
            let y1 = rect.y.max(capture.y);
            let x2 = rect
                .x
                .saturating_add(rect.width)
                .min(capture.x.saturating_add(capture.width));
            let y2 = rect
                .y
                .saturating_add(rect.height)
                .min(capture.y.saturating_add(capture.height));
            if x2 <= x1 || y2 <= y1 {
                return None;
            }
            Some(CapturePixelRect {
                x: x1 - capture.x,
                y: y1 - capture.y,
                width: x2 - x1,
                height: y2 - y1,
            })
        })
        .collect()
}

/// Clip a logical output region and map it into mode-buffer pixels.
///
/// The transform step follows wlroots `wlr_box_transform` with the inverted output transform.
/// Edges are then scaled like Jay, rounding each edge so a fractional scale does not leave a gap.
pub(crate) fn output_capture_rect(
    mode_width: i32,
    mode_height: i32,
    scale: crate::state::FractionalScale,
    transform: i32,
    region: Option<(i32, i32, i32, i32)>,
) -> Option<CapturePixelRect> {
    if mode_width <= 0 || mode_height <= 0 {
        return None;
    }
    let wayland = crate::transform::BufferTransform::from_wayland(transform)
        .unwrap_or(crate::transform::BufferTransform::Normal);
    let (buffer_width, buffer_height) = wayland.maybe_swap((mode_width, mode_height));
    let logical_width = scale.logical_length(buffer_width);
    let logical_height = scale.logical_length(buffer_height);
    let logical = region.unwrap_or((0, 0, logical_width, logical_height));
    let full = crate::region::RectI::new(0, 0, logical_width, logical_height)?;
    let requested = crate::region::RectI::new(logical.0, logical.1, logical.2, logical.3)?;
    let clipped = requested.intersection(full)?;
    if clipped.width <= 0 || clipped.height <= 0 {
        return None;
    }
    let transformed = transform_output_box(
        clipped.x,
        clipped.y,
        clipped.width,
        clipped.height,
        invert_output_transform(transform),
        logical_width,
        logical_height,
    )?;
    let x1 = scale_logical_to_buffer(transformed.0, scale);
    let y1 = scale_logical_to_buffer(transformed.1, scale);
    let x2 = scale_logical_to_buffer(transformed.0.saturating_add(transformed.2), scale);
    let y2 = scale_logical_to_buffer(transformed.1.saturating_add(transformed.3), scale);
    let mode = crate::region::RectI::new(0, 0, mode_width, mode_height)?;
    let scaled = crate::region::RectI::new(x1, y1, x2.saturating_sub(x1), y2.saturating_sub(y1))?;
    let hit = scaled.intersection(mode)?;
    if hit.width <= 0 || hit.height <= 0 {
        return None;
    }
    Some(CapturePixelRect { x: hit.x, y: hit.y, width: hit.width, height: hit.height })
}

fn scale_logical_to_buffer(logical: i32, scale: crate::state::FractionalScale) -> i32 {
    let logical = i64::from(logical);
    let numerator = i64::from(scale.as_v120());
    let denominator = i64::from(crate::state::FractionalScale::DENOMINATOR);
    ((logical * numerator) + (denominator / 2)).div_euclid(denominator) as i32
}

fn invert_output_transform(transform: i32) -> i32 {
    if transform & 1 != 0 && transform & 4 == 0 {
        transform ^ 2
    } else {
        transform
    }
}

fn transform_output_box(
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    transform: i32,
    bounds_width: i32,
    bounds_height: i32,
) -> Option<(i32, i32, i32, i32)> {
    let (dest_width, dest_height) =
        if transform % 2 == 0 { (width, height) } else { (height, width) };
    let (dest_x, dest_y) = match transform {
        0 => (x, y),
        1 => (bounds_height.saturating_sub(y).saturating_sub(height), x),
        2 => (
            bounds_width.saturating_sub(x).saturating_sub(width),
            bounds_height.saturating_sub(y).saturating_sub(height),
        ),
        3 => (y, bounds_width.saturating_sub(x).saturating_sub(width)),
        4 => (bounds_width.saturating_sub(x).saturating_sub(width), y),
        5 => (y, x),
        6 => (x, bounds_height.saturating_sub(y).saturating_sub(height)),
        7 => (
            bounds_height.saturating_sub(y).saturating_sub(height),
            bounds_width.saturating_sub(x).saturating_sub(width),
        ),
        _ => return None,
    };
    Some((dest_x, dest_y, dest_width, dest_height))
}

pub(crate) fn toplevel_sample_pending(capture: &CaptureRegistry, output_id: OutputId) -> bool {
    capture
        .jobs
        .values()
        .any(|job| toplevel_job_matches(job, output_id))
}

fn toplevel_job_matches(job: &CaptureJob, output_id: OutputId) -> bool {
    job.status == CaptureJobStatus::Queued
        && matches!(job.target, CaptureTarget::Toplevel(_))
        && job.pixel_rect.is_some()
        && (job.outputs.is_empty() || job.outputs.contains(&output_id))
}

pub(crate) fn queued_toplevel_job(
    state: &State,
    job_id: CaptureJobId,
) -> Option<(ToplevelKey, i32, i32)> {
    let job = state.capture.jobs.get(&job_id)?;
    let CaptureTarget::Toplevel(key) = job.target else {
        return None;
    };
    if job.status != CaptureJobStatus::Queued || job.pixel_rect.is_none() {
        return None;
    }
    Some((key, job.width, job.height))
}

pub(crate) fn toplevel_jobs_for_output(
    capture: &CaptureRegistry,
    output_id: OutputId,
) -> Vec<CaptureJobId> {
    capture
        .jobs
        .iter()
        .filter(|(_, job)| toplevel_job_matches(job, output_id))
        .map(|(id, _)| *id)
        .collect()
}

/// Queue a toplevel copy the size of the rendered window, including its tree and decorations.
pub(crate) fn submit_toplevel_capture(
    state: &mut State,
    dispatcher: &mut Dispatcher,
    key: ToplevelKey,
    buffer: Resolved<WlBufferId>,
    cursor: CaptureCursor,
    with_damage: bool,
) -> Result<CaptureJobId, CaptureQueueError> {
    if state.session_lock.input_gated() {
        return Err(CaptureQueueError::Protected);
    }
    let Some(bounds) = toplevel_rendered_bounds(state, key) else {
        return Err(CaptureQueueError::SourceGone);
    };
    let id = submit_capture(
        state,
        dispatcher,
        CaptureJobRequest {
            target: CaptureTarget::Toplevel(key),
            buffer,
            width: bounds.width,
            height: bounds.height,
            cursor,
        },
    )?;
    if let Some(job) = state.capture.jobs.get_mut(&id) {
        job.pixel_rect =
            Some(CapturePixelRect { x: 0, y: 0, width: bounds.width, height: bounds.height });
        job.with_damage = with_damage;
    }
    Ok(id)
}

/// Stop toplevel sessions and fail their frames while the desktop is hidden.
///
/// Foreign-toplevel handles stay listed. Output captures are left to sample the protected frame.
pub(crate) fn stop_protected_toplevel_captures(state: &mut State, dispatcher: &mut Dispatcher) {
    if !state.session_lock.input_gated() {
        return;
    }
    for job in state.capture.jobs.values_mut() {
        if job.status == CaptureJobStatus::Queued
            && matches!(job.target, CaptureTarget::Toplevel(_))
        {
            job.status = CaptureJobStatus::Failed;
        }
    }
    state.capture.toplevel_damage.clear();
    let mut clients = BTreeSet::new();
    let sessions: Vec<_> = state
        .capture
        .sessions
        .iter()
        .filter(|slot| matches!(slot.target, CaptureTarget::Toplevel(_)))
        .copied()
        .collect();
    for slot in sessions {
        state
            .capture
            .enqueue(slot.session.client_id(), Pending::StopSession(slot.session));
        clients.insert(slot.session.client_id());
    }
    state
        .capture
        .sessions
        .retain(|slot| !matches!(slot.target, CaptureTarget::Toplevel(_)));
    let cursor_hosts: Vec<_> = state
        .capture
        .sessions
        .iter()
        .filter(|slot| slot.pointer.is_some() && matches!(slot.target, CaptureTarget::Output(_)))
        .map(|slot| slot.session)
        .collect();
    let cursor_sessions: Vec<_> = cursor_hosts
        .into_iter()
        .filter_map(|session| {
            session
                .try_ref(state)
                .and_then(|session| session.cursor_session())
        })
        .collect();
    for cursor in cursor_sessions {
        state
            .capture
            .enqueue(cursor.client_id(), Pending::CursorLeft(cursor));
        clients.insert(cursor.client_id());
    }
    let frames: Vec<_> = state
        .capture
        .ext_frames
        .iter()
        .filter(|slot| matches!(slot.target, CaptureTarget::Toplevel(_)))
        .copied()
        .collect();
    for slot in frames {
        state
            .capture
            .enqueue(slot.frame.client_id(), Pending::FailExtFrame(slot.frame));
        clients.insert(slot.frame.client_id());
    }
    state
        .capture
        .ext_frames
        .retain(|slot| !matches!(slot.target, CaptureTarget::Toplevel(_)));
    dispatch_deliver(dispatcher, clients);
}

pub(crate) fn toplevel_root_surface(
    state: &State,
    key: ToplevelKey,
) -> Option<Resolved<crate::protocol_generated::WlSurfaceId>> {
    let toplevel = state
        .capture
        .by_xdg
        .iter()
        .find(|(_, candidate)| **candidate == key)
        .map(|(toplevel, _)| *toplevel)?;
    let xdg_surface = toplevel.try_ref(state)?.xdg_surface();
    Some(xdg_surface.try_ref(state)?.wl_surface())
}

/// Bounds of the window as this compositor draws it: the surface tree, popups, and decoration box.
pub(crate) fn toplevel_rendered_bounds(state: &State, key: ToplevelKey) -> Option<RectI> {
    let root = toplevel_root_surface(state, key)?;
    let mut bounds = None;
    let mut seen = BTreeSet::new();
    accumulate_rendered_bounds(state, root, &mut bounds, &mut seen);
    if let Some(visual) = state.root_surface_visual_state(root) {
        bounds = union_bounds(bounds, visual.influence_bounds());
    }
    bounds.filter(|bounds| bounds.width > 0 && bounds.height > 0)
}

fn accumulate_rendered_bounds(
    state: &State,
    surface: Resolved<crate::protocol_generated::WlSurfaceId>,
    bounds: &mut Option<RectI>,
    seen: &mut BTreeSet<Resolved<crate::protocol_generated::WlSurfaceId>>,
) {
    if !seen.insert(surface) {
        return;
    }
    if let Some(rect) = state.surface_scene_bounds(surface) {
        *bounds = union_bounds(*bounds, rect);
    }
    for popup in state.popup_child_surfaces(surface) {
        accumulate_rendered_bounds(state, popup, bounds, seen);
    }
}

fn union_bounds(current: Option<RectI>, next: RectI) -> Option<RectI> {
    if next.width <= 0 || next.height <= 0 {
        return current;
    }
    let Some(current) = current else {
        return Some(next);
    };
    RectI::from_extents(
        current.x.min(next.x),
        current.y.min(next.y),
        current.x2().max(next.x2()),
        current.y2().max(next.y2()),
    )
}

/// Read-only view of the toplevel tree. Damage, frame callbacks, feedbacks, and retired buffers stay put.
pub(crate) fn peek_toplevel_snapshots(
    state: &State,
    key: ToplevelKey,
) -> Vec<crate::wayland_objects::wl_surface::SurfaceRenderState> {
    let Some(root) = toplevel_root_surface(state, key) else {
        return Vec::new();
    };
    let mut snapshots = Vec::new();
    peek_surface_snapshots(state, root, 0, 0, true, &mut snapshots);
    snapshots
}

fn peek_surface_snapshots(
    state: &State,
    surface: Resolved<crate::protocol_generated::WlSurfaceId>,
    base_x: i32,
    base_y: i32,
    inherit_root_clip: bool,
    snapshots: &mut Vec<crate::wayland_objects::wl_surface::SurfaceRenderState>,
) {
    let Some(surface_obj) = surface.try_ref(state) else {
        return;
    };
    let stack = surface_obj.current_stack().to_vec();
    let Some(mut surface_state) = surface_obj.render_state_at(base_x, base_y) else {
        return;
    };
    surface_state.surface = Some(surface);
    surface_state.inherit_root_clip = inherit_root_clip;
    surface_state.presentation_feedbacks.clear();
    for entry in &stack {
        if let crate::wayland_objects::wl_surface::StackEntry::Child(child) = *entry {
            let Some(child_surface) = child.try_ref(state) else {
                continue;
            };
            let (x, y) = child_surface.current_subsurface_position();
            peek_surface_snapshots(
                state,
                child,
                surface_state.x.saturating_add(x),
                surface_state.y.saturating_add(y),
                inherit_root_clip,
                snapshots,
            );
        } else {
            snapshots.push(surface_state.clone());
        }
    }
    for popup in state.popup_child_surfaces(surface) {
        let Some((x, y)) = state.surface_scene_origin(popup) else {
            continue;
        };
        peek_surface_snapshots(state, popup, x, y, false, snapshots);
    }
}

pub(crate) fn apply_rendered_toplevel(
    capture: &mut CaptureRegistry,
    objects: &crate::indexmap::IndexMap<crate::state::ObjectId, Box<dyn crate::object::Object>>,
    key: ToplevelKey,
    bounds_width: i32,
    bounds_height: i32,
    frame_damage: &[CapturePixelRect],
    copy: RenderedOutputCopy,
    timestamp: CaptureTimestamp,
) {
    let Some(job) = capture.jobs.get(&copy.job_id) else {
        return;
    };
    if job.status != CaptureJobStatus::Queued || job.target != CaptureTarget::Toplevel(key) {
        return;
    }
    let cursor = job.cursor;
    let with_damage = job.with_damage;
    let client_id = job.buffer.client_id();
    let buffer = job.buffer;
    let stored = job.pixel_rect;
    let Some(stored) = stored else {
        return;
    };
    if bounds_width != stored.width || bounds_height != stored.height || !copy.read_ok {
        if let Some(job) = capture.jobs.get_mut(&copy.job_id) {
            job.status = CaptureJobStatus::Failed;
        }
        return;
    }
    let Some(buffer) =
        objects.try_ref_object::<crate::wayland_objects::wl_buffer::WlBuffer>(buffer.object_id())
    else {
        if let Some(job) = capture.jobs.get_mut(&copy.job_id) {
            job.status = CaptureJobStatus::Failed;
        }
        return;
    };
    let written = buffer
        .begin_capture_write(stored.width, stored.height)
        .and_then(|write| write.write_top_left(&copy.bytes, copy.stride))
        .is_ok();
    if !written {
        if let Some(job) = capture.jobs.get_mut(&copy.job_id) {
            job.status = CaptureJobStatus::Failed;
        }
        return;
    }
    let damage =
        take_toplevel_damage(capture, client_id, key, cursor, stored, with_damage, frame_damage);
    if let Some(job) = capture.jobs.get_mut(&copy.job_id) {
        job.status = CaptureJobStatus::Written;
    }
    capture.samples.insert(
        copy.job_id,
        CaptureSample {
            rect: stored,
            stride: copy.stride,
            flags: 0,
            tv_sec_hi: (timestamp.sec >> 32) as u32,
            tv_sec_lo: timestamp.sec as u32,
            tv_nsec: timestamp.nsec,
            damage,
        },
    );
}

fn take_toplevel_damage(
    capture: &mut CaptureRegistry,
    client_id: ClientId,
    key: ToplevelKey,
    cursor: CaptureCursor,
    rect: CapturePixelRect,
    with_damage: bool,
    frame_damage: &[CapturePixelRect],
) -> Vec<CapturePixelRect> {
    let entry = capture
        .toplevel_damage
        .entry((client_id, key, cursor))
        .or_default();
    if entry.started {
        entry.rects.extend(frame_damage.iter().copied());
    }
    if !with_damage {
        return Vec::new();
    }
    let damage = if entry.started {
        clip_damage_to_capture(&entry.rects, rect)
    } else {
        vec![CapturePixelRect { x: 0, y: 0, width: rect.width, height: rect.height }]
    };
    entry.started = true;
    entry.rects.clear();
    damage
}

pub(crate) fn output_removed(state: &mut State, dispatcher: &mut Dispatcher, output_id: OutputId) {
    let target = CaptureTarget::Output(output_id);
    let mut clients = BTreeSet::new();
    let sessions: Vec<_> = state
        .capture
        .sessions
        .iter()
        .filter(|slot| slot.target == target)
        .copied()
        .collect();
    for slot in sessions {
        let client_id = slot.session.client_id();
        state
            .capture
            .enqueue(client_id, Pending::StopSession(slot.session));
        clients.insert(client_id);
    }
    state.capture.sessions.retain(|slot| slot.target != target);

    let frames: Vec<_> = state
        .capture
        .ext_frames
        .iter()
        .filter(|slot| slot.target == target)
        .copied()
        .collect();
    for slot in frames {
        let client_id = slot.frame.client_id();
        state
            .capture
            .enqueue(client_id, Pending::FailExtFrame(slot.frame));
        clients.insert(client_id);
    }
    state
        .capture
        .ext_frames
        .retain(|slot| slot.target != target);

    let screencopy: Vec<_> = state
        .capture
        .screencopy
        .iter()
        .filter(|slot| slot.output_id == output_id)
        .copied()
        .collect();
    for slot in screencopy {
        let client_id = slot.frame.client_id();
        state
            .capture
            .enqueue(client_id, Pending::FailScreencopy(slot.frame));
        clients.insert(client_id);
    }
    state
        .capture
        .screencopy
        .retain(|slot| slot.output_id != output_id);
    fail_queued_jobs(state, target);
    state
        .capture
        .damage
        .retain(|(_, damaged, _), _| *damaged != output_id);
    dispatch_deliver(dispatcher, clients);
}

pub(crate) fn pointer_released(
    state: &mut State,
    dispatcher: &mut Dispatcher,
    pointer: Resolved<WlPointerId>,
) {
    let mut clients = BTreeSet::new();
    let sessions: Vec<_> = state
        .capture
        .sessions
        .iter()
        .filter(|slot| slot.pointer == Some(pointer))
        .copied()
        .collect();
    for slot in sessions {
        let client_id = slot.session.client_id();
        state
            .capture
            .enqueue(client_id, Pending::StopSession(slot.session));
        clients.insert(client_id);
    }
    state
        .capture
        .sessions
        .retain(|slot| slot.pointer != Some(pointer));

    let frames: Vec<_> = state
        .capture
        .ext_frames
        .iter()
        .filter(|slot| slot.pointer == Some(pointer))
        .copied()
        .collect();
    for slot in frames {
        let client_id = slot.frame.client_id();
        state
            .capture
            .enqueue(client_id, Pending::FailExtFrame(slot.frame));
        clients.insert(client_id);
    }
    state
        .capture
        .ext_frames
        .retain(|slot| slot.pointer != Some(pointer));
    dispatch_deliver(dispatcher, clients);
}

/// Create one toplevel identity and queue its initial events for every live list.
pub(crate) fn announce_toplevel(
    state: &mut State,
    dispatcher: &mut Dispatcher,
    title: impl Into<String>,
    app_id: impl Into<String>,
) -> ToplevelKey {
    state.capture.next_toplevel = state.capture.next_toplevel.saturating_add(1);
    let key = ToplevelKey(state.capture.next_toplevel);
    let title = title.into();
    let app_id = app_id.into();
    debug_assert!(key.identifier().len() <= 32);
    debug_assert!(key
        .identifier()
        .bytes()
        .all(|byte| byte.is_ascii() && !byte.is_ascii_control()));
    state.capture.toplevels.insert(
        key,
        ToplevelRecord {
            closed: false,
            title: title.clone(),
            app_id: app_id.clone(),
            activated: false,
            maximized: false,
            fullscreen: false,
            outputs: BTreeSet::new(),
            parent: None,
        },
    );
    let subscribers = state.capture.subscribers.clone();
    let mut clients = BTreeSet::new();
    for subscriber in subscribers {
        if subscriber.is_finished(state) {
            continue;
        }
        state.capture.enqueue(
            subscriber.client_id(),
            Pending::Announce { key, title: title.clone(), app_id: app_id.clone() },
        );
        clients.insert(subscriber.client_id());
    }
    dispatch_deliver(dispatcher, clients);
    key
}

/// Unmap a toplevel: close its handles and stop sessions that captured it.
pub(crate) fn close_toplevel(state: &mut State, dispatcher: &mut Dispatcher, key: ToplevelKey) {
    let already_closed = match state.capture.toplevels.get_mut(&key) {
        Some(record) if !record.closed => {
            record.closed = true;
            false
        }
        _ => true,
    };
    if already_closed {
        return;
    }
    state.capture.retain_pending(|item| match item {
        Pending::Announce { key: announced, .. }
        | Pending::Retitle { key: announced, .. }
        | Pending::Reapp { key: announced, .. }
        | Pending::ShellState { key: announced, .. }
        | Pending::OutputEntered { key: announced, .. }
        | Pending::OutputLeft { key: announced, .. }
        | Pending::ParentChanged { key: announced, .. } => *announced != key,
        _ => true,
    });
    state
        .capture
        .announced
        .retain(|(_, announced)| *announced != key);
    state
        .capture
        .by_xdg
        .retain(|_, announced| *announced != key);
    fail_queued_jobs(state, CaptureTarget::Toplevel(key));
    state
        .capture
        .toplevel_damage
        .retain(|(_, damaged, _), _| *damaged != key);

    let mut clients = BTreeSet::new();
    let handles = state.capture.handles_for(key);
    for handle in handles {
        state
            .capture
            .enqueue(handle.client_id(), Pending::CloseHandle(handle));
        clients.insert(handle.client_id());
    }
    let target = CaptureTarget::Toplevel(key);
    let sessions: Vec<_> = state
        .capture
        .sessions
        .iter()
        .filter(|slot| slot.target == target)
        .copied()
        .collect();
    for slot in sessions {
        state
            .capture
            .enqueue(slot.session.client_id(), Pending::StopSession(slot.session));
        clients.insert(slot.session.client_id());
    }
    state.capture.sessions.retain(|slot| slot.target != target);
    let frames: Vec<_> = state
        .capture
        .ext_frames
        .iter()
        .filter(|slot| slot.target == target)
        .copied()
        .collect();
    for slot in frames {
        state
            .capture
            .enqueue(slot.frame.client_id(), Pending::FailExtFrame(slot.frame));
        clients.insert(slot.frame.client_id());
    }
    state
        .capture
        .ext_frames
        .retain(|slot| slot.target != target);
    clients.extend(clear_parent_references(state, dispatcher, key));
    dispatch_deliver(dispatcher, clients);
}

fn record_mut(state: &mut State, key: ToplevelKey) -> Option<&mut ToplevelRecord> {
    let record = state.capture.toplevels.get_mut(&key)?;
    (!record.closed).then_some(record)
}

fn is_announced(state: &State, key: ToplevelKey) -> bool {
    state
        .capture
        .announced
        .iter()
        .any(|(_, announced)| *announced == key)
}

fn enqueue_if_announced(
    state: &mut State,
    dispatcher: &mut Dispatcher,
    key: ToplevelKey,
    item: Pending,
) {
    if !is_announced(state, key) {
        return;
    }
    let clients: BTreeSet<_> = state
        .capture
        .announced
        .iter()
        .filter(|(_, announced)| *announced == key)
        .map(|(list, _)| list.client_id())
        .collect();
    for client_id in clients.iter().copied() {
        state.capture.enqueue(client_id, item.clone());
    }
    dispatch_deliver(dispatcher, clients);
}

fn note_shell_state(
    state: &mut State,
    dispatcher: &mut Dispatcher,
    key: ToplevelKey,
    maximized: bool,
    fullscreen: bool,
) {
    let activated = {
        let Some(record) = record_mut(state, key) else {
            return;
        };
        if record.maximized == maximized && record.fullscreen == fullscreen {
            return;
        }
        record.maximized = maximized;
        record.fullscreen = fullscreen;
        record.activated
    };
    enqueue_if_announced(
        state,
        dispatcher,
        key,
        Pending::ShellState { key, activated, maximized, fullscreen },
    );
}

fn note_parent(
    state: &mut State,
    dispatcher: &mut Dispatcher,
    key: ToplevelKey,
    parent: Option<ToplevelKey>,
) {
    let changed = {
        let Some(record) = record_mut(state, key) else {
            return;
        };
        if record.parent == parent {
            return;
        }
        record.parent = parent;
        true
    };
    if changed {
        enqueue_if_announced(state, dispatcher, key, Pending::ParentChanged { key, parent });
    }
}

fn clear_parent_references(
    state: &mut State,
    dispatcher: &mut Dispatcher,
    parent: ToplevelKey,
) -> BTreeSet<ClientId> {
    let children: Vec<ToplevelKey> = state
        .capture
        .toplevels
        .iter()
        .filter(|(_, record)| !record.closed && record.parent == Some(parent))
        .map(|(key, _)| *key)
        .collect();
    let mut clients = BTreeSet::new();
    for child in children {
        note_parent(state, dispatcher, child, None);
        if is_announced(state, child) {
            clients.extend(
                state
                    .capture
                    .announced
                    .iter()
                    .filter(|(_, announced)| *announced == child)
                    .map(|(list, _)| list.client_id()),
            );
        }
    }
    clients
}

/// `xdg_toplevel.configure` sent `maximized` and `fullscreen`. Stored even while the window is
/// still unmapped, then copied onto the record at map.
pub(crate) fn note_xdg_shell_state(
    state: &mut State,
    dispatcher: &mut Dispatcher,
    toplevel: Resolved<XdgToplevelId>,
    maximized: bool,
    fullscreen: bool,
) {
    let Some(key) = state.capture.by_xdg.get(&toplevel).copied() else {
        return;
    };
    note_shell_state(state, dispatcher, key, maximized, fullscreen);
}

pub(crate) fn note_xdg_parent(
    state: &mut State,
    dispatcher: &mut Dispatcher,
    toplevel: Resolved<XdgToplevelId>,
    parent: Option<Resolved<XdgToplevelId>>,
) {
    let Some(key) = state.capture.by_xdg.get(&toplevel).copied() else {
        return;
    };
    let parent_key = parent.and_then(|parent| state.capture.by_xdg.get(&parent).copied());
    note_parent(state, dispatcher, key, parent_key);
}

fn adopt_children_of(
    state: &mut State,
    dispatcher: &mut Dispatcher,
    parent: Resolved<XdgToplevelId>,
    parent_key: ToplevelKey,
) {
    let mapped: Vec<Resolved<XdgToplevelId>> = state.capture.by_xdg.keys().copied().collect();
    let children: Vec<Resolved<XdgToplevelId>> = mapped
        .into_iter()
        .filter(|toplevel| {
            toplevel
                .try_ref(state)
                .and_then(|toplevel| toplevel.parent_toplevel())
                == Some(parent)
        })
        .collect();
    for child in children {
        if let Some(child_key) = state.capture.by_xdg.get(&child).copied() {
            note_parent(state, dispatcher, child_key, Some(parent_key));
        }
    }
}

pub(crate) fn sync_activated_from_keyboard_focus(state: &mut State, dispatcher: &mut Dispatcher) {
    apply_activated(state, dispatcher, keyboard_activated_toplevel(state));
}

fn apply_activated(state: &mut State, dispatcher: &mut Dispatcher, active: Option<ToplevelKey>) {
    let changes: Vec<(ToplevelKey, bool)> = state
        .capture
        .toplevels
        .iter()
        .filter(|(_, record)| !record.closed)
        .filter_map(|(key, record)| {
            let activated = Some(*key) == active;
            (record.activated != activated).then_some((*key, activated))
        })
        .collect();
    for (key, activated) in changes {
        let item = {
            let Some(record) = record_mut(state, key) else {
                continue;
            };
            record.activated = activated;
            Pending::ShellState {
                key,
                activated,
                maximized: record.maximized,
                fullscreen: record.fullscreen,
            }
        };
        enqueue_if_announced(state, dispatcher, key, item);
    }
}

fn keyboard_activated_toplevel(state: &State) -> Option<ToplevelKey> {
    let focus = state.seat.keyboard_focus()?;
    let root = state
        .root_surface_for_surface(focus.surface)
        .unwrap_or(focus.surface);
    let xdg_surface = root.try_ref(state)?.xdg_surface()?;
    let toplevel = state.xdg_toplevel(xdg_surface).or_else(|| {
        xdg_surface
            .try_ref(state)
            .and_then(crate::wayland_objects::xdg_surface::XdgSurface::toplevel_role)
    })?;
    state.capture.by_xdg.get(&toplevel).copied()
}

/// Copy the toplevel root's `wl_surface` output membership. Subsurfaces and popups are ignored.
pub(crate) fn sync_toplevel_outputs(
    state: &mut State,
    dispatcher: &mut Dispatcher,
    surface: Resolved<WlSurfaceId>,
) {
    let Some(xdg_surface) = surface
        .try_ref(state)
        .and_then(|surface| surface.xdg_surface())
    else {
        return;
    };
    let Some(toplevel) = state.xdg_toplevel(xdg_surface).or_else(|| {
        xdg_surface
            .try_ref(state)
            .and_then(crate::wayland_objects::xdg_surface::XdgSurface::toplevel_role)
    }) else {
        return;
    };
    let Some(key) = state.capture.by_xdg.get(&toplevel).copied() else {
        return;
    };
    let next = surface
        .try_ref(state)
        .map(|surface| {
            surface
                .output_membership_ids()
                .into_iter()
                .collect::<BTreeSet<_>>()
        })
        .unwrap_or_default();
    let (entered, left) = {
        let Some(record) = record_mut(state, key) else {
            return;
        };
        if record.outputs == next {
            return;
        }
        let entered: Vec<OutputId> = next.difference(&record.outputs).copied().collect();
        let left: Vec<OutputId> = record.outputs.difference(&next).copied().collect();
        record.outputs = next;
        (entered, left)
    };
    for output in entered {
        enqueue_if_announced(state, dispatcher, key, Pending::OutputEntered { key, output });
    }
    for output in left {
        enqueue_if_announced(state, dispatcher, key, Pending::OutputLeft { key, output });
    }
}

pub fn deliver(client: &mut Client, state: &mut State, dispatcher: &mut ClientDispatcher) {
    let Some(pending) = state.capture.pending.remove(&client.client_id()) else {
        return;
    };
    for item in pending {
        match item {
            Pending::StopSession(session) => {
                if let Some(session) = session.try_ref_mut(state) {
                    session.mark_stopped(dispatcher);
                }
            }
            Pending::FailExtFrame(frame) => {
                if let Some(frame) = frame.try_ref_mut(state) {
                    frame.fail_stopped(dispatcher);
                }
            }
            Pending::FinishExtFrame { frame, job } => {
                let status = state.capture.jobs.get(&job).map(|job| job.status);
                let sample = state.capture.samples.get(&job).cloned();
                let reason = ext_failure_reason(state, job);
                if let Some(frame) = frame.try_ref_mut(state) {
                    if let (Some(CaptureJobStatus::Written), Some(sample)) = (status, sample) {
                        frame.ready(&sample, dispatcher);
                    } else {
                        frame.fail_reason(reason, dispatcher);
                    }
                }
            }
            Pending::CursorLeft(cursor) => {
                if let Some(cursor) = cursor.try_ref_mut(state) {
                    cursor.note_left(dispatcher);
                }
            }
            Pending::FailScreencopy(frame) => {
                if let Some(frame) = frame.try_ref_mut(state) {
                    frame.fail(dispatcher);
                }
            }
            Pending::FinishScreencopy { frame, job } => {
                let status = state.capture.jobs.get(&job).map(|job| job.status);
                let sample = state.capture.samples.get(&job).cloned();
                if let Some(frame) = frame.try_ref_mut(state) {
                    if let (Some(CaptureJobStatus::Written), Some(sample)) = (status, sample) {
                        frame.ready(&sample, dispatcher);
                    } else {
                        frame.fail(dispatcher);
                    }
                }
            }
            Pending::CloseHandle(handle) => {
                if let Some(handle) = handle.try_ref_mut(state) {
                    handle.send_closed(dispatcher);
                }
            }
            Pending::Announce { key, title, app_id } => {
                if state.capture.toplevel_is_open(key) {
                    announce_to_client(client, state, dispatcher, key, &title, &app_id);
                }
            }
            Pending::Retitle { key, title } => {
                for handle in state.capture.handles_for(key) {
                    if let Some(handle) = handle.try_ref_mut(state) {
                        handle.send_title(&title, dispatcher);
                    }
                }
            }
            Pending::Reapp { key, app_id } => {
                for handle in state.capture.handles_for(key) {
                    if let Some(handle) = handle.try_ref_mut(state) {
                        handle.send_app_id(&app_id, dispatcher);
                    }
                }
            }
            // Ext lists have no state, output, or parent events. wlr handles consume these later.
            Pending::ShellState { .. }
            | Pending::OutputEntered { .. }
            | Pending::OutputLeft { .. }
            | Pending::ParentChanged { .. } => {}
        }
    }
}

fn announce_to_client(
    client: &mut Client,
    state: &mut State,
    dispatcher: &mut ClientDispatcher,
    key: ToplevelKey,
    title: &str,
    app_id: &str,
) {
    let lists: Vec<_> = state
        .capture
        .subscribers
        .iter()
        .copied()
        .filter_map(ToplevelSubscriber::ext_list)
        .filter(|list| list.client_id() == client.client_id())
        .filter(|list| list.try_ref(state).is_some_and(|list| !list.is_finished()))
        .collect();
    let lists: Vec<_> = lists
        .into_iter()
        .filter(|list| !state.capture.announced.contains(&(*list, key)))
        .collect();
    for list in lists {
        let handle_id = client.allocate_server_object_id::<ExtForeignToplevelHandleV1Id>();
        let (handle, _) = client.insert_object(
            state,
            ExtForeignToplevelHandleV1::new(handle_id, key, title.to_string(), app_id.to_string()),
        );
        state.capture.track_handle(handle, key);
        state.capture.announced.insert((list, key));
        list.rref(state).send_toplevel(handle_id, dispatcher);
        handle.rref(state).send_initial(dispatcher);
    }
}

/// Publish one mapped XDG toplevel. A later map of the same toplevel gets a new identifier.
pub(crate) fn map_xdg_toplevel(
    state: &mut State,
    dispatcher: &mut Dispatcher,
    toplevel: Resolved<XdgToplevelId>,
    title: impl Into<String>,
    app_id: impl Into<String>,
) {
    if state.capture.by_xdg.contains_key(&toplevel) {
        return;
    }
    let key = announce_toplevel(state, dispatcher, title, app_id);
    state.capture.by_xdg.insert(toplevel, key);
    if let Some(toplevel_ref) = toplevel.try_ref(state) {
        let (maximized, fullscreen) = toplevel_ref.published_shell_state();
        let parent = toplevel_ref.parent_toplevel();
        drop(toplevel_ref);
        note_shell_state(state, dispatcher, key, maximized, fullscreen);
        let parent_key = parent.and_then(|parent| state.capture.by_xdg.get(&parent).copied());
        note_parent(state, dispatcher, key, parent_key);
    }
    adopt_children_of(state, dispatcher, toplevel, key);
}

/// Stop listing and capturing an XDG toplevel that unmapped or was destroyed.
pub(crate) fn unmap_xdg_toplevel(
    state: &mut State,
    dispatcher: &mut Dispatcher,
    toplevel: Resolved<XdgToplevelId>,
) {
    let Some(key) = state.capture.by_xdg.remove(&toplevel) else {
        return;
    };
    close_toplevel(state, dispatcher, key);
}

pub(crate) fn retitle_xdg_toplevel(
    state: &mut State,
    dispatcher: &mut Dispatcher,
    toplevel: Resolved<XdgToplevelId>,
    title: String,
) {
    update_xdg_detail(state, dispatcher, toplevel, true, title);
}

pub(crate) fn reapp_xdg_toplevel(
    state: &mut State,
    dispatcher: &mut Dispatcher,
    toplevel: Resolved<XdgToplevelId>,
    app_id: String,
) {
    update_xdg_detail(state, dispatcher, toplevel, false, app_id);
}

fn update_xdg_detail(
    state: &mut State,
    dispatcher: &mut Dispatcher,
    toplevel: Resolved<XdgToplevelId>,
    title: bool,
    value: String,
) {
    let Some(key) = state.capture.by_xdg.get(&toplevel).copied() else {
        return;
    };
    let Some(record) = state.capture.toplevels.get_mut(&key) else {
        return;
    };
    if record.closed {
        return;
    }
    if title {
        if record.title == value {
            return;
        }
        record.title = value.clone();
    } else {
        if record.app_id == value {
            return;
        }
        record.app_id = value.clone();
    }
    let clients: BTreeSet<_> = state
        .capture
        .handles_for(key)
        .into_iter()
        .map(|handle| handle.client_id())
        .collect();
    for client_id in clients.iter().copied() {
        let item = if title {
            Pending::Retitle { key, title: value.clone() }
        } else {
            Pending::Reapp { key, app_id: value.clone() }
        };
        state.capture.enqueue(client_id, item);
    }
    dispatch_deliver(dispatcher, clients);
}

/// Send every currently mapped toplevel to a list that just bound.
pub(crate) fn sync_mapped_toplevels(
    state: &mut State,
    dispatcher: &mut Dispatcher,
    list: Resolved<ExtForeignToplevelListV1Id>,
) {
    if list.try_ref(state).is_none_or(|list| list.is_finished()) {
        return;
    }
    let open: Vec<_> = state
        .capture
        .toplevels
        .iter()
        .filter(|(_, record)| !record.closed)
        .map(|(key, record)| (*key, record.title.clone(), record.app_id.clone()))
        .collect();
    if open.is_empty() {
        return;
    }
    let client_id = list.client_id();
    for (key, title, app_id) in open {
        state
            .capture
            .enqueue(client_id, Pending::Announce { key, title, app_id });
    }
    dispatch_deliver(dispatcher, BTreeSet::from([client_id]));
}

fn dispatch_deliver(dispatcher: &mut Dispatcher, clients: BTreeSet<ClientId>) {
    for client_id in clients {
        dispatcher.dispatch_to(client_id, ClientAction::DeliverCaptureEvents);
    }
}

#[cfg(test)]
pub(crate) fn test_request(object: u32, opcode: u16, words: u16) -> crate::clients::RawEvent {
    crate::clients::RawEvent::new(
        crate::protocol_generated::WaylandId::from(object),
        opcode,
        words.into(),
    )
}

#[cfg(test)]
pub(crate) fn test_dispatch<K>(
    client: &mut Client,
    state: &mut State,
    dispatcher: &mut Dispatcher,
    object: K,
    event: crate::clients::RawEvent,
) where
    K: crate::object::IdToObject + Into<crate::protocol_generated::WaylandId> + Copy,
{
    use crate::object::Object;
    let mut client_dispatcher = client.test_dispatcher(dispatcher);
    client_dispatcher.set_requester_object_id_for_tests(object.into());
    client
        .grab_object(object, state)
        .on(
            client,
            state,
            crate::test_support::raw_request_from_event(event),
            &mut client_dispatcher,
        )
        .unwrap();
}

#[cfg(test)]
pub(crate) fn test_sent(dispatcher: &Dispatcher, object: u32, opcode: u32) -> bool {
    use crate::io_uring::model::IoUringAction;
    dispatcher.test_has_queued_action::<IoUringAction>(|action| match action {
        IoUringAction::Sendmsg { buffer, .. } => {
            buffer.len() >= 8
                && u32::from_ne_bytes(buffer[0..4].try_into().unwrap()) == object
                && u32::from_ne_bytes(buffer[4..8].try_into().unwrap()) & 0xffff == opcode
        }
        _ => false,
    })
}

#[cfg(test)]
pub(crate) fn test_sent_arg(dispatcher: &Dispatcher, object: u32, opcode: u32, value: u32) -> bool {
    use crate::io_uring::model::IoUringAction;
    dispatcher.test_has_queued_action::<IoUringAction>(|action| match action {
        IoUringAction::Sendmsg { buffer, .. } => {
            buffer.len() >= 12
                && u32::from_ne_bytes(buffer[0..4].try_into().unwrap()) == object
                && u32::from_ne_bytes(buffer[4..8].try_into().unwrap()) & 0xffff == opcode
                && u32::from_ne_bytes(buffer[8..12].try_into().unwrap()) == value
        }
        _ => false,
    })
}

#[cfg(test)]
pub(crate) fn test_closed(dispatcher: &Dispatcher, client: &Client) -> bool {
    dispatcher.test_has_queued_action::<crate::clients::ClientManagerAction>(|action| {
        matches!(action, crate::clients::ClientManagerAction::Close { id } if *id == client.client_id())
    })
}

#[cfg(test)]
pub(crate) fn test_deliver(client: &mut Client, state: &mut State, dispatcher: &mut Dispatcher) {
    use crate::runner::Model;
    client
        .process(state, ClientAction::DeliverCaptureEvents, dispatcher)
        .unwrap();
}

#[cfg(test)]
mod tests {
    use {
        super::*,
        crate::{
            clients::{ClientAction, RawEvent},
            protocol_generated::{
                Constants, ExtForeignToplevelImageCaptureSourceManagerV1Id,
                ExtForeignToplevelListV1Id, ExtImageCaptureSourceV1Id,
                ExtImageCopyCaptureCursorSessionV1Id, ExtImageCopyCaptureFrameV1Id,
                ExtImageCopyCaptureManagerV1Id, ExtImageCopyCaptureSessionV1Id,
                ExtOutputImageCaptureSourceManagerV1Id, WlBufferId, WlOutputId, WlPointerId,
                ZwlrScreencopyFrameV1Id, ZwlrScreencopyManagerV1Id,
            },
            runner::Model,
            test_support::{state_for_tests, test_client_and_dispatcher, test_output_drm1},
            wayland_objects::{
                ext_foreign_toplevel_image_capture_source_manager_v1::ExtForeignToplevelImageCaptureSourceManagerV1,
                ext_foreign_toplevel_list_v1::ExtForeignToplevelListV1,
                ext_image_copy_capture_cursor_session_v1::ExtImageCopyCaptureCursorSessionV1,
                ext_image_copy_capture_frame_v1::ExtImageCopyCaptureFrameV1,
                ext_image_copy_capture_manager_v1::ExtImageCopyCaptureManagerV1,
                ext_image_copy_capture_session_v1::ExtImageCopyCaptureSessionV1,
                ext_output_image_capture_source_manager_v1::ExtOutputImageCaptureSourceManagerV1,
                wl_buffer::{NewWlBufferParams, WlBuffer},
                wl_output::WlOutput,
                wl_pointer::WlPointer,
                zwlr_screencopy_frame_v1::ZwlrScreencopyFrameV1,
                zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1,
            },
        },
    };

    const OUTPUT_OBJECT: u32 = 20;
    const BUFFER: u32 = 21;

    fn client_state() -> (crate::clients::Client, Dispatcher, State) {
        let (client, dispatcher) = test_client_and_dispatcher(1);
        (client, dispatcher, state_for_tests())
    }

    fn insert_output(client: &mut crate::clients::Client, state: &mut State) -> OutputId {
        let output_id = state.insert_output(test_output_drm1(0, 0, 640, 480));
        client.insert_object(state, WlOutput::new(WlOutputId::from(OUTPUT_OBJECT), output_id, 4));
        output_id
    }

    fn insert_buffer(client: &mut crate::clients::Client, state: &mut State) {
        let bytes = vec![0u8; 16];
        client.insert_object(
            state,
            WlBuffer::new(NewWlBufferParams {
                id: WlBufferId::from(BUFFER),
                owner_client_id: client.client_id(),
                offset: 0,
                width: 2,
                height: 2,
                stride: 8,
                format: &crate::drm::ARGB8888,
                mem: crate::wayland_objects::wl_shm_pool::shared_memory_from_bytes(&bytes),
            })
            .unwrap(),
        );
    }

    fn with_id(
        mut event: RawEvent,
        id: impl Into<crate::protocol_generated::WaylandId>,
    ) -> RawEvent {
        event.id(id.into());
        event
    }

    #[test]
    fn capture_globals_are_advertised_at_their_implemented_versions() {
        let state = state_for_tests();
        for (interface, version) in [
            (ZwlrScreencopyManagerV1::INTERFACE_NAME, ZwlrScreencopyManagerV1::INTERFACE_VERSION),
            (
                ExtImageCopyCaptureManagerV1::INTERFACE_NAME,
                ExtImageCopyCaptureManagerV1::INTERFACE_VERSION,
            ),
            (
                ExtOutputImageCaptureSourceManagerV1::INTERFACE_NAME,
                ExtOutputImageCaptureSourceManagerV1::INTERFACE_VERSION,
            ),
            (
                ExtForeignToplevelImageCaptureSourceManagerV1::INTERFACE_NAME,
                ExtForeignToplevelImageCaptureSourceManagerV1::INTERFACE_VERSION,
            ),
            (ExtForeignToplevelListV1::INTERFACE_NAME, ExtForeignToplevelListV1::INTERFACE_VERSION),
        ] {
            let advertised = state.globals.iter().find_map(|(_, global)| {
                (global.interface().0 == interface).then_some(global.interface().1)
            });
            assert_eq!(advertised, Some(version), "{interface}");
        }
    }

    #[test]
    fn screencopy_copy_is_one_shot_and_survives_manager_destruction() {
        let (mut client, mut dispatcher, mut state) = client_state();
        let _output_id = insert_output(&mut client, &mut state);
        insert_buffer(&mut client, &mut state);
        client.insert_object(
            &mut state,
            ZwlrScreencopyManagerV1::new(ZwlrScreencopyManagerV1Id::from(8), 3),
        );
        let mut event = test_request(8, 1, 7);
        event.id(ZwlrScreencopyFrameV1Id::from(9));
        event.int(1);
        event.id(WlOutputId::from(OUTPUT_OBJECT));
        event.int(0);
        event.int(0);
        event.int(2);
        event.int(2);
        test_dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ZwlrScreencopyManagerV1Id::from(8),
            event,
        );
        let frame = client
            .resolve(ZwlrScreencopyFrameV1Id::from(9), &state)
            .unwrap()
            .rref(&state);
        assert!(frame.overlay_cursor());
        assert_eq!(frame.region(), Some((0, 0, 2, 2)));
        test_dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ZwlrScreencopyManagerV1Id::from(8),
            test_request(8, 2, 0),
        );
        assert!(client
            .resolve(ZwlrScreencopyFrameV1Id::from(9), &state)
            .is_some());
        let copy = with_id(test_request(9, 0, 1), WlBufferId::from(BUFFER));
        test_dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ZwlrScreencopyFrameV1Id::from(9),
            copy,
        );
        assert!(!test_closed(&dispatcher, &client));
        let copy_again = with_id(test_request(9, 0, 1), WlBufferId::from(BUFFER));
        test_dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ZwlrScreencopyFrameV1Id::from(9),
            copy_again,
        );
        assert!(test_closed(&dispatcher, &client));
    }

    #[test]
    fn screencopy_region_is_kept_and_a_new_frame_can_replace_a_destroyed_one() {
        let (mut client, mut dispatcher, mut state) = client_state();
        insert_output(&mut client, &mut state);
        client.insert_object(
            &mut state,
            ZwlrScreencopyManagerV1::new(ZwlrScreencopyManagerV1Id::from(8), 3),
        );
        let mut event = test_request(8, 1, 7);
        event.id(ZwlrScreencopyFrameV1Id::from(9));
        event.int(0);
        event.id(WlOutputId::from(OUTPUT_OBJECT));
        event.int(4);
        event.int(6);
        event.int(20);
        event.int(10);
        test_dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ZwlrScreencopyManagerV1Id::from(8),
            event,
        );
        assert_eq!(
            client
                .resolve(ZwlrScreencopyFrameV1Id::from(9), &state)
                .unwrap()
                .rref(&state)
                .region(),
            Some((4, 6, 20, 10))
        );
        test_dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ZwlrScreencopyFrameV1Id::from(9),
            test_request(9, 1, 0),
        );
        assert!(state.capture.screencopy.is_empty());
        let mut event = test_request(8, 0, 3);
        event.id(ZwlrScreencopyFrameV1Id::from(10));
        event.int(0);
        event.id(WlOutputId::from(OUTPUT_OBJECT));
        test_dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ZwlrScreencopyManagerV1Id::from(8),
            event,
        );
        assert!(client
            .resolve(ZwlrScreencopyFrameV1Id::from(10), &state)
            .is_some());
    }

    #[test]
    fn losing_the_output_fails_an_unfinished_screencopy_frame() {
        let (mut client, mut dispatcher, mut state) = client_state();
        let output_id = insert_output(&mut client, &mut state);
        client.insert_object(
            &mut state,
            ZwlrScreencopyManagerV1::new(ZwlrScreencopyManagerV1Id::from(8), 3),
        );
        let mut event = test_request(8, 0, 3);
        event.id(ZwlrScreencopyFrameV1Id::from(9));
        event.int(0);
        event.id(WlOutputId::from(OUTPUT_OBJECT));
        test_dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ZwlrScreencopyManagerV1Id::from(8),
            event,
        );
        assert!(state.remove_output(output_id, &mut dispatcher).is_some());
        test_deliver(&mut client, &mut state, &mut dispatcher);
        assert!(test_sent(&dispatcher, 9, 3));
        assert!(state.capture.screencopy.is_empty());
    }

    #[test]
    fn copy_with_damage_on_version_one_is_rejected() {
        let (mut client, mut dispatcher, mut state) = client_state();
        let output_id = insert_output(&mut client, &mut state);
        insert_buffer(&mut client, &mut state);
        client.insert_object(
            &mut state,
            ZwlrScreencopyFrameV1::new(ZwlrScreencopyFrameV1Id::from(9), 1, output_id, false, None),
        );
        let copy = with_id(test_request(9, 2, 1), WlBufferId::from(BUFFER));
        test_dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ZwlrScreencopyFrameV1Id::from(9),
            copy,
        );
        assert!(test_closed(&dispatcher, &client));
    }

    #[test]
    fn screencopy_advertises_shm_and_completes_in_protocol_order() {
        let (mut client, mut dispatcher, mut state) = client_state();
        let output_id = insert_output(&mut client, &mut state);
        insert_buffer(&mut client, &mut state);
        client.insert_object(
            &mut state,
            ZwlrScreencopyManagerV1::new(ZwlrScreencopyManagerV1Id::from(8), 3),
        );
        let mut event = test_request(8, 1, 7);
        event.id(ZwlrScreencopyFrameV1Id::from(9));
        event.int(0);
        event.id(WlOutputId::from(OUTPUT_OBJECT));
        event.int(0);
        event.int(0);
        event.int(2);
        event.int(2);
        test_dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ZwlrScreencopyManagerV1Id::from(8),
            event,
        );
        assert!(test_sent_arg(&dispatcher, 9, 0, 0));
        assert!(test_sent_arg(&dispatcher, 9, 0, 1));
        assert!(test_sent(&dispatcher, 9, 6));
        assert!(!test_sent(&dispatcher, 9, 5));
        let copy = with_id(test_request(9, 2, 1), WlBufferId::from(BUFFER));
        test_dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ZwlrScreencopyFrameV1Id::from(9),
            copy,
        );
        assert!(!test_closed(&dispatcher, &client));
        let specs = output_copy_specs(&state.capture, &state.objects, output_id);
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].rect, CapturePixelRect { x: 0, y: 0, width: 2, height: 2 });
        let pixels = vec![9u8; specs[0].stride * 2];
        apply_rendered_output(
            &mut state.capture,
            &state.objects,
            output_id,
            640,
            480,
            crate::state::FractionalScale::ONE,
            0,
            &[CapturePixelRect { x: 0, y: 0, width: 2, height: 2 }],
            vec![RenderedOutputCopy {
                job_id: specs[0].job_id,
                bytes: pixels,
                stride: specs[0].stride,
                read_ok: true,
            }],
            CaptureTimestamp { sec: 5, nsec: 7 },
        );
        publish_screencopy(&mut state, &mut dispatcher);
        test_deliver(&mut client, &mut state, &mut dispatcher);
        assert!(test_sent_arg(&dispatcher, 9, 1, 0));
        assert!(test_sent_arg(&dispatcher, 9, 4, 0));
        assert!(test_sent(&dispatcher, 9, 2));
        let stored = client
            .resolve(WlBufferId::from(BUFFER), &state)
            .unwrap()
            .rref(&state)
            .begin_capture_write(2, 2)
            .unwrap();
        assert_eq!(stored.as_slice()[..8], [9, 9, 9, 9, 9, 9, 9, 9]);

        let mut again = test_request(8, 1, 7);
        again.id(ZwlrScreencopyFrameV1Id::from(11));
        again.int(0);
        again.id(WlOutputId::from(OUTPUT_OBJECT));
        again.int(0);
        again.int(0);
        again.int(2);
        again.int(2);
        test_dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ZwlrScreencopyManagerV1Id::from(8),
            again,
        );
        let copy = with_id(test_request(11, 2, 1), WlBufferId::from(BUFFER));
        test_dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ZwlrScreencopyFrameV1Id::from(11),
            copy,
        );
        let specs = output_copy_specs(&state.capture, &state.objects, output_id);
        assert_eq!(specs.len(), 1);
        apply_rendered_output(
            &mut state.capture,
            &state.objects,
            output_id,
            640,
            480,
            crate::state::FractionalScale::ONE,
            0,
            &[CapturePixelRect { x: 1, y: 0, width: 1, height: 1 }],
            vec![RenderedOutputCopy {
                job_id: specs[0].job_id,
                bytes: vec![0; specs[0].stride * 2],
                stride: specs[0].stride,
                read_ok: true,
            }],
            CaptureTimestamp { sec: 6, nsec: 1 },
        );
        publish_screencopy(&mut state, &mut dispatcher);
        test_deliver(&mut client, &mut state, &mut dispatcher);
        assert!(test_sent_arg(&dispatcher, 11, 4, 1));
        assert!(!test_sent_arg(&dispatcher, 11, 4, 0));
    }

    #[test]
    fn screencopy_rejects_a_mismatched_buffer_and_fails_an_empty_region() {
        let (mut client, mut dispatcher, mut state) = client_state();
        insert_output(&mut client, &mut state);
        insert_buffer(&mut client, &mut state);
        client.insert_object(
            &mut state,
            ZwlrScreencopyManagerV1::new(ZwlrScreencopyManagerV1Id::from(8), 3),
        );
        let mut event = test_request(8, 0, 3);
        event.id(ZwlrScreencopyFrameV1Id::from(9));
        event.int(0);
        event.id(WlOutputId::from(OUTPUT_OBJECT));
        test_dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ZwlrScreencopyManagerV1Id::from(8),
            event,
        );
        let copy = with_id(test_request(9, 0, 1), WlBufferId::from(BUFFER));
        test_dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ZwlrScreencopyFrameV1Id::from(9),
            copy,
        );
        assert!(test_closed(&dispatcher, &client));

        let (mut client, mut dispatcher, mut state) = client_state();
        insert_output(&mut client, &mut state);
        client.insert_object(
            &mut state,
            ZwlrScreencopyManagerV1::new(ZwlrScreencopyManagerV1Id::from(8), 3),
        );
        let mut event = test_request(8, 1, 7);
        event.id(ZwlrScreencopyFrameV1Id::from(9));
        event.int(0);
        event.id(WlOutputId::from(OUTPUT_OBJECT));
        event.int(0);
        event.int(0);
        event.int(0);
        event.int(2);
        test_dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ZwlrScreencopyManagerV1Id::from(8),
            event,
        );
        assert!(test_sent(&dispatcher, 9, 3));
        assert!(!test_sent(&dispatcher, 9, 0));
    }

    #[test]
    fn a_painted_capture_blends_the_cursor_without_changing_a_hidden_copy() {
        let cursor = CaptureCursorImage {
            local_x: 1,
            local_y: 0,
            width: 1,
            height: 1,
            stride: 4,
            argb: vec![10, 20, 30, 255],
            mode_width: 4,
            mode_height: 2,
            scale: crate::state::FractionalScale::ONE,
            transform: 0,
        };
        let rect = CapturePixelRect { x: 0, y: 0, width: 2, height: 1 };
        let hidden = vec![1, 1, 1, 255, 2, 2, 2, 255];
        let mut painted = hidden.clone();
        blend_cursor_into_copy(&mut painted, 8, rect, &cursor);
        assert_eq!(hidden, [1, 1, 1, 255, 2, 2, 2, 255]);
        assert_eq!(painted, [1, 1, 1, 255, 10, 20, 30, 255]);
    }

    #[test]
    fn client_disconnect_drops_screencopy_tracking() {
        let (mut client, mut dispatcher, mut state) = client_state();
        let output_id = insert_output(&mut client, &mut state);
        client.insert_object(
            &mut state,
            ZwlrScreencopyFrameV1::new(ZwlrScreencopyFrameV1Id::from(9), 3, output_id, false, None),
        );
        state.capture.track_screencopy(
            client
                .resolve(ZwlrScreencopyFrameV1Id::from(9), &state)
                .unwrap(),
            output_id,
        );
        client
            .process(&mut state, ClientAction::Close, &mut dispatcher)
            .unwrap();
        assert!(state.capture.screencopy.is_empty());
        assert!(state.capture.pending.is_empty());
    }

    fn output_source(
        client: &mut crate::clients::Client,
        state: &mut State,
        dispatcher: &mut Dispatcher,
    ) {
        insert_output(client, state);
        client.insert_object(
            state,
            ExtOutputImageCaptureSourceManagerV1::new(
                ExtOutputImageCaptureSourceManagerV1Id::from(30),
            ),
        );
        let mut event = test_request(30, 0, 2);
        event.id(ExtImageCaptureSourceV1Id::from(31));
        event.id(WlOutputId::from(OUTPUT_OBJECT));
        test_dispatch(
            client,
            state,
            dispatcher,
            ExtOutputImageCaptureSourceManagerV1Id::from(30),
            event,
        );
    }

    #[test]
    fn invalid_capture_option_disconnects() {
        let (mut client, mut dispatcher, mut state) = client_state();
        output_source(&mut client, &mut state, &mut dispatcher);
        client.insert_object(
            &mut state,
            ExtImageCopyCaptureManagerV1::new(ExtImageCopyCaptureManagerV1Id::from(32)),
        );
        let mut event = test_request(32, 0, 3);
        event.id(ExtImageCopyCaptureSessionV1Id::from(33));
        event.id(ExtImageCaptureSourceV1Id::from(31));
        event.uint(2);
        test_dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ExtImageCopyCaptureManagerV1Id::from(32),
            event,
        );
        assert!(test_closed(&dispatcher, &client));
        assert!(client
            .resolve(ExtImageCopyCaptureSessionV1Id::from(33), &state)
            .is_none());
    }

    #[test]
    fn a_second_frame_on_the_same_session_disconnects() {
        let (mut client, mut dispatcher, mut state) = client_state();
        output_source(&mut client, &mut state, &mut dispatcher);
        insert_buffer(&mut client, &mut state);
        client.insert_object(
            &mut state,
            ExtImageCopyCaptureManagerV1::new(ExtImageCopyCaptureManagerV1Id::from(32)),
        );
        let mut event = test_request(32, 0, 3);
        event.id(ExtImageCopyCaptureSessionV1Id::from(33));
        event.id(ExtImageCaptureSourceV1Id::from(31));
        event.uint(1);
        test_dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ExtImageCopyCaptureManagerV1Id::from(32),
            event,
        );
        assert!(client
            .resolve(ExtImageCopyCaptureSessionV1Id::from(33), &state)
            .unwrap()
            .rref(&state)
            .paint_cursors());
        let mut event = test_request(33, 0, 1);
        event.id(ExtImageCopyCaptureFrameV1Id::from(34));
        test_dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ExtImageCopyCaptureSessionV1Id::from(33),
            event,
        );
        let mut again = test_request(33, 0, 1);
        again.id(ExtImageCopyCaptureFrameV1Id::from(35));
        test_dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ExtImageCopyCaptureSessionV1Id::from(33),
            again,
        );
        assert!(test_closed(&dispatcher, &client));
    }

    #[test]
    fn capture_without_a_buffer_disconnects() {
        let (mut client, mut dispatcher, mut state) = client_state();
        output_source(&mut client, &mut state, &mut dispatcher);
        insert_buffer(&mut client, &mut state);
        let output_id = state.outputs.iter().next().unwrap().0;
        client.insert_object(
            &mut state,
            ExtImageCopyCaptureSessionV1::new(
                ExtImageCopyCaptureSessionV1Id::from(33),
                CaptureTarget::Output(output_id),
                false,
                None,
            ),
        );
        let session = client
            .resolve(ExtImageCopyCaptureSessionV1Id::from(33), &state)
            .unwrap();
        state
            .capture
            .track_session(session, session.rref(&state).target(), None);
        let mut event = test_request(33, 0, 1);
        event.id(ExtImageCopyCaptureFrameV1Id::from(34));
        test_dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ExtImageCopyCaptureSessionV1Id::from(33),
            event,
        );
        test_dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ExtImageCopyCaptureFrameV1Id::from(34),
            test_request(34, 3, 0),
        );
        assert!(test_closed(&dispatcher, &client));
    }

    #[test]
    fn invalid_buffer_damage_disconnects() {
        let (mut client, mut dispatcher, mut state) = client_state();
        let output_id = insert_output(&mut client, &mut state);
        insert_buffer(&mut client, &mut state);
        let session_id = ExtImageCopyCaptureSessionV1Id::from(33);
        client.insert_object(
            &mut state,
            ExtImageCopyCaptureSessionV1::new(
                session_id,
                CaptureTarget::Output(output_id),
                false,
                None,
            ),
        );
        let session = client.resolve(session_id, &state).unwrap();
        client.insert_object(
            &mut state,
            ExtImageCopyCaptureFrameV1::new(ExtImageCopyCaptureFrameV1Id::from(34), session),
        );
        let mut damage = test_request(34, 2, 4);
        damage.int(-1);
        damage.int(0);
        damage.int(1);
        damage.int(1);
        test_dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ExtImageCopyCaptureFrameV1Id::from(34),
            damage,
        );
        assert!(test_closed(&dispatcher, &client));
    }

    #[test]
    fn output_loss_stops_the_session_and_fails_a_capturing_frame() {
        let (mut client, mut dispatcher, mut state) = client_state();
        let output_id = state.insert_output(test_output_drm1(0, 0, 2, 2));
        insert_buffer(&mut client, &mut state);
        let session_id = ExtImageCopyCaptureSessionV1Id::from(33);
        client.insert_object(
            &mut state,
            ExtImageCopyCaptureSessionV1::new(
                session_id,
                CaptureTarget::Output(output_id),
                false,
                None,
            ),
        );
        let session = client.resolve(session_id, &state).unwrap();
        state
            .capture
            .track_session(session, CaptureTarget::Output(output_id), None);
        client.insert_object(
            &mut state,
            ExtImageCopyCaptureFrameV1::new(ExtImageCopyCaptureFrameV1Id::from(34), session),
        );
        let frame = client
            .resolve(ExtImageCopyCaptureFrameV1Id::from(34), &state)
            .unwrap();
        state
            .capture
            .track_ext_frame(frame, CaptureTarget::Output(output_id), None);
        let attach = with_id(test_request(34, 1, 1), WlBufferId::from(BUFFER));
        test_dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ExtImageCopyCaptureFrameV1Id::from(34),
            attach,
        );
        let mut damage = test_request(34, 2, 4);
        damage.int(0);
        damage.int(0);
        damage.int(2);
        damage.int(2);
        test_dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ExtImageCopyCaptureFrameV1Id::from(34),
            damage,
        );
        test_dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ExtImageCopyCaptureFrameV1Id::from(34),
            test_request(34, 3, 0),
        );
        assert!(!test_closed(&dispatcher, &client));
        assert!(state.remove_output(output_id, &mut dispatcher).is_some());
        test_deliver(&mut client, &mut state, &mut dispatcher);
        assert!(test_sent(&dispatcher, 33, 5));
        assert!(test_sent_arg(&dispatcher, 34, 4, 2));
    }

    #[test]
    fn destroying_a_source_leaves_the_session_usable_until_the_output_is_gone() {
        let (mut client, mut dispatcher, mut state) = client_state();
        output_source(&mut client, &mut state, &mut dispatcher);
        client.insert_object(
            &mut state,
            ExtImageCopyCaptureManagerV1::new(ExtImageCopyCaptureManagerV1Id::from(32)),
        );
        let mut event = test_request(32, 0, 3);
        event.id(ExtImageCopyCaptureSessionV1Id::from(33));
        event.id(ExtImageCaptureSourceV1Id::from(31));
        event.uint(0);
        test_dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ExtImageCopyCaptureManagerV1Id::from(32),
            event,
        );
        test_dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ExtImageCaptureSourceV1Id::from(31),
            test_request(31, 0, 0),
        );
        assert!(client
            .resolve(ExtImageCopyCaptureSessionV1Id::from(33), &state)
            .is_some());
        assert_eq!(state.capture.sessions.len(), 1);
    }

    #[test]
    fn cursor_capture_sessions_cannot_be_requested_twice() {
        let (mut client, mut dispatcher, mut state) = client_state();
        output_source(&mut client, &mut state, &mut dispatcher);
        client.insert_object(&mut state, WlPointer::new(WlPointerId::from(40), 3));
        client.insert_object(
            &mut state,
            ExtImageCopyCaptureManagerV1::new(ExtImageCopyCaptureManagerV1Id::from(32)),
        );
        let mut event = test_request(32, 1, 3);
        event.id(ExtImageCopyCaptureCursorSessionV1Id::from(41));
        event.id(ExtImageCaptureSourceV1Id::from(31));
        event.id(WlPointerId::from(40));
        test_dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ExtImageCopyCaptureManagerV1Id::from(32),
            event,
        );
        let mut event = test_request(41, 1, 1);
        event.id(ExtImageCopyCaptureSessionV1Id::from(33));
        test_dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ExtImageCopyCaptureCursorSessionV1Id::from(41),
            event,
        );
        let mut again = test_request(41, 1, 1);
        again.id(ExtImageCopyCaptureSessionV1Id::from(42));
        test_dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ExtImageCopyCaptureCursorSessionV1Id::from(41),
            again,
        );
        assert!(test_closed(&dispatcher, &client));
    }

    #[test]
    fn destroying_an_ext_frame_allows_another_and_a_second_capture_is_rejected() {
        let (mut client, mut dispatcher, mut state) = client_state();
        let output_id = insert_output(&mut client, &mut state);
        insert_buffer(&mut client, &mut state);
        client.insert_object(
            &mut state,
            ExtImageCopyCaptureSessionV1::new(
                ExtImageCopyCaptureSessionV1Id::from(33),
                CaptureTarget::Output(output_id),
                false,
                None,
            ),
        );
        let mut event = test_request(33, 0, 1);
        event.id(ExtImageCopyCaptureFrameV1Id::from(34));
        test_dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ExtImageCopyCaptureSessionV1Id::from(33),
            event,
        );
        test_dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ExtImageCopyCaptureFrameV1Id::from(34),
            test_request(34, 0, 0),
        );
        let mut event = test_request(33, 0, 1);
        event.id(ExtImageCopyCaptureFrameV1Id::from(35));
        test_dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ExtImageCopyCaptureSessionV1Id::from(33),
            event,
        );
        assert!(client
            .resolve(ExtImageCopyCaptureFrameV1Id::from(35), &state)
            .is_some());
        let attach = with_id(test_request(35, 1, 1), WlBufferId::from(BUFFER));
        test_dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ExtImageCopyCaptureFrameV1Id::from(35),
            attach,
        );
        test_dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ExtImageCopyCaptureFrameV1Id::from(35),
            test_request(35, 3, 0),
        );
        test_dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ExtImageCopyCaptureFrameV1Id::from(35),
            test_request(35, 3, 0),
        );
        assert!(test_closed(&dispatcher, &client));
    }

    #[test]
    fn pointer_release_stops_a_cursor_capture_session() {
        let (mut client, mut dispatcher, mut state) = client_state();
        output_source(&mut client, &mut state, &mut dispatcher);
        client.insert_object(&mut state, WlPointer::new(WlPointerId::from(40), 3));
        let pointer = client.resolve(WlPointerId::from(40), &state).unwrap();
        let output_id = state.outputs.iter().next().unwrap().0;
        client.insert_object(
            &mut state,
            ExtImageCopyCaptureCursorSessionV1::new(
                ExtImageCopyCaptureCursorSessionV1Id::from(41),
                CaptureTarget::Output(output_id),
                pointer,
            ),
        );
        let mut event = test_request(41, 1, 1);
        event.id(ExtImageCopyCaptureSessionV1Id::from(33));
        test_dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ExtImageCopyCaptureCursorSessionV1Id::from(41),
            event,
        );
        test_dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            WlPointerId::from(40),
            test_request(40, 1, 0),
        );
        test_deliver(&mut client, &mut state, &mut dispatcher);
        assert!(test_sent(&dispatcher, 33, 5));
    }

    #[test]
    fn an_ext_session_advertises_shm_constraints_and_completes_a_frame() {
        let (mut client, mut dispatcher, mut state) = client_state();
        let output_id = state.insert_output(test_output_drm1(0, 0, 2, 2));
        client.insert_object(
            &mut state,
            WlOutput::new(WlOutputId::from(OUTPUT_OBJECT), output_id, 4),
        );
        client.insert_object(
            &mut state,
            ExtOutputImageCaptureSourceManagerV1::new(
                ExtOutputImageCaptureSourceManagerV1Id::from(30),
            ),
        );
        let mut event = test_request(30, 0, 2);
        event.id(ExtImageCaptureSourceV1Id::from(31));
        event.id(WlOutputId::from(OUTPUT_OBJECT));
        test_dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ExtOutputImageCaptureSourceManagerV1Id::from(30),
            event,
        );
        insert_buffer(&mut client, &mut state);
        client.insert_object(
            &mut state,
            ExtImageCopyCaptureManagerV1::new(ExtImageCopyCaptureManagerV1Id::from(32)),
        );
        let mut event = test_request(32, 0, 3);
        event.id(ExtImageCopyCaptureSessionV1Id::from(33));
        event.id(ExtImageCaptureSourceV1Id::from(31));
        event.uint(1);
        test_dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ExtImageCopyCaptureManagerV1Id::from(32),
            event,
        );
        assert!(test_sent_arg(&dispatcher, 33, 1, 0));
        assert!(test_sent_arg(&dispatcher, 33, 1, 1));
        assert!(test_sent_arg(&dispatcher, 33, 0, 2));
        assert!(test_sent(&dispatcher, 33, 4));
        assert!(!test_sent(&dispatcher, 33, 2));
        let mut frame = test_request(33, 0, 1);
        frame.id(ExtImageCopyCaptureFrameV1Id::from(34));
        test_dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ExtImageCopyCaptureSessionV1Id::from(33),
            frame,
        );
        let attach = with_id(test_request(34, 1, 1), WlBufferId::from(BUFFER));
        test_dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ExtImageCopyCaptureFrameV1Id::from(34),
            attach,
        );
        test_dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ExtImageCopyCaptureFrameV1Id::from(34),
            test_request(34, 3, 0),
        );
        assert!(!test_closed(&dispatcher, &client));
        let specs = output_copy_specs(&state.capture, &state.objects, output_id);
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].cursor, CaptureCursor::Painted);
        apply_rendered_output(
            &mut state.capture,
            &state.objects,
            output_id,
            2,
            2,
            crate::state::FractionalScale::ONE,
            0,
            &[CapturePixelRect { x: 0, y: 0, width: 2, height: 2 }],
            vec![RenderedOutputCopy {
                job_id: specs[0].job_id,
                bytes: vec![4, 5, 6, 7, 4, 5, 6, 7, 4, 5, 6, 7, 4, 5, 6, 7],
                stride: specs[0].stride,
                read_ok: true,
            }],
            CaptureTimestamp { sec: 9, nsec: 8 },
        );
        publish_ext_frames(&mut state, &mut dispatcher);
        test_deliver(&mut client, &mut state, &mut dispatcher);
        assert!(test_sent_arg(&dispatcher, 34, 1, 0));
        assert!(test_sent_arg(&dispatcher, 34, 0, 0));
        assert!(test_sent(&dispatcher, 34, 2));
        assert!(test_sent(&dispatcher, 34, 3));
        let stored = client
            .resolve(WlBufferId::from(BUFFER), &state)
            .unwrap()
            .rref(&state)
            .begin_capture_write(2, 2)
            .unwrap();
        assert_eq!(stored.as_slice()[..4], [4, 5, 6, 7]);
    }

    #[test]
    fn an_ext_frame_fails_a_buffer_that_misses_the_constraints() {
        let (mut client, mut dispatcher, mut state) = client_state();
        output_source(&mut client, &mut state, &mut dispatcher);
        insert_buffer(&mut client, &mut state);
        client.insert_object(
            &mut state,
            ExtImageCopyCaptureManagerV1::new(ExtImageCopyCaptureManagerV1Id::from(32)),
        );
        let mut event = test_request(32, 0, 3);
        event.id(ExtImageCopyCaptureSessionV1Id::from(33));
        event.id(ExtImageCaptureSourceV1Id::from(31));
        event.uint(0);
        test_dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ExtImageCopyCaptureManagerV1Id::from(32),
            event,
        );
        let mut frame = test_request(33, 0, 1);
        frame.id(ExtImageCopyCaptureFrameV1Id::from(34));
        test_dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ExtImageCopyCaptureSessionV1Id::from(33),
            frame,
        );
        let attach = with_id(test_request(34, 1, 1), WlBufferId::from(BUFFER));
        test_dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ExtImageCopyCaptureFrameV1Id::from(34),
            attach,
        );
        test_dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ExtImageCopyCaptureFrameV1Id::from(34),
            test_request(34, 3, 0),
        );
        assert!(!test_closed(&dispatcher, &client));
        assert!(test_sent_arg(&dispatcher, 34, 4, 1));
    }

    #[test]
    fn a_pointer_cursor_session_copies_the_cursor_image() {
        let (mut client, mut dispatcher, mut state) = client_state();
        output_source(&mut client, &mut state, &mut dispatcher);
        let pixels = vec![10, 20, 30, 255];
        let (cursor_buffer, _) = client.insert_object(
            &mut state,
            WlBuffer::new(NewWlBufferParams {
                id: WlBufferId::from(70),
                owner_client_id: client.client_id(),
                offset: 0,
                width: 1,
                height: 1,
                stride: 4,
                format: &crate::drm::ARGB8888,
                mem: crate::wayland_objects::wl_shm_pool::shared_memory_from_bytes(&pixels),
            })
            .unwrap(),
        );
        let surface = {
            let (resolved, surface) = client.insert_object(
                &mut state,
                crate::wayland_objects::wl_surface::WlSurface::new(
                    crate::protocol_generated::WlSurfaceId::from(72),
                    client.client_id(),
                    6,
                ),
            );
            surface.test_map(cursor_buffer.object_id(), 0, 0, 1, 1);
            resolved
        };
        state.seat.set_cursor_surface(surface, 0, 0);
        state.seat.pointer_pos = (10.0, 20.0);
        client.insert_object(&mut state, WlPointer::new(WlPointerId::from(40), 3));
        client.insert_object(
            &mut state,
            ExtImageCopyCaptureManagerV1::new(ExtImageCopyCaptureManagerV1Id::from(32)),
        );
        let mut event = test_request(32, 1, 3);
        event.id(ExtImageCopyCaptureCursorSessionV1Id::from(41));
        event.id(ExtImageCaptureSourceV1Id::from(31));
        event.id(WlPointerId::from(40));
        test_dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ExtImageCopyCaptureManagerV1Id::from(32),
            event,
        );
        let mut event = test_request(41, 1, 1);
        event.id(ExtImageCopyCaptureSessionV1Id::from(33));
        test_dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ExtImageCopyCaptureCursorSessionV1Id::from(41),
            event,
        );
        assert!(test_sent(&dispatcher, 41, 0));
        assert!(test_sent_arg(&dispatcher, 41, 2, 10));
        assert!(test_sent_arg(&dispatcher, 41, 3, 0));
        assert!(test_sent_arg(&dispatcher, 33, 0, 1));
        assert!(test_sent(&dispatcher, 33, 4));
        let capture_pixels = vec![0, 0, 0, 0];
        client.insert_object(
            &mut state,
            WlBuffer::new(NewWlBufferParams {
                id: WlBufferId::from(71),
                owner_client_id: client.client_id(),
                offset: 0,
                width: 1,
                height: 1,
                stride: 4,
                format: &crate::drm::ARGB8888,
                mem: crate::wayland_objects::wl_shm_pool::shared_memory_from_bytes(&capture_pixels),
            })
            .unwrap(),
        );
        let mut frame = test_request(33, 0, 1);
        frame.id(ExtImageCopyCaptureFrameV1Id::from(34));
        test_dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ExtImageCopyCaptureSessionV1Id::from(33),
            frame,
        );
        let attach = with_id(test_request(34, 1, 1), WlBufferId::from(71));
        test_dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ExtImageCopyCaptureFrameV1Id::from(34),
            attach,
        );
        test_dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ExtImageCopyCaptureFrameV1Id::from(34),
            test_request(34, 3, 0),
        );
        assert!(!test_closed(&dispatcher, &client));
        assert!(test_sent(&dispatcher, 34, 3));
        let stored = client
            .resolve(WlBufferId::from(71), &state)
            .unwrap()
            .rref(&state)
            .begin_capture_write(1, 1)
            .unwrap();
        assert_eq!(stored.as_slice(), [10, 20, 30, 255]);
    }

    #[test]
    fn closing_a_toplevel_closes_its_handle_and_stops_capture() {
        let (mut client, mut dispatcher, mut state) = client_state();
        ExtForeignToplevelListV1::attach(
            &mut client,
            &mut state,
            &mut dispatcher,
            ExtForeignToplevelListV1Id::from(50),
        );
        let key = announce_toplevel(&mut state, &mut dispatcher, "Terminal", "app");
        test_deliver(&mut client, &mut state, &mut dispatcher);
        let handle = state.capture.handles_for(key)[0];
        let handle_id: u32 = handle.wayland_id().into();
        assert!(test_sent(&dispatcher, handle_id, 4));
        assert!(!handle.rref(&state).is_closed());
        client.insert_object(
            &mut state,
            ExtForeignToplevelImageCaptureSourceManagerV1::new(
                ExtForeignToplevelImageCaptureSourceManagerV1Id::from(51),
            ),
        );
        let mut event = test_request(51, 0, 2);
        event.id(ExtImageCaptureSourceV1Id::from(52));
        event.id(handle.wayland_id());
        test_dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ExtForeignToplevelImageCaptureSourceManagerV1Id::from(51),
            event,
        );
        client.insert_object(
            &mut state,
            ExtImageCopyCaptureManagerV1::new(ExtImageCopyCaptureManagerV1Id::from(53)),
        );
        insert_buffer(&mut client, &mut state);
        let mut event = test_request(53, 0, 3);
        event.id(ExtImageCopyCaptureSessionV1Id::from(54));
        event.id(ExtImageCaptureSourceV1Id::from(52));
        event.uint(0);
        test_dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ExtImageCopyCaptureManagerV1Id::from(53),
            event,
        );
        let mut event = test_request(54, 0, 1);
        event.id(ExtImageCopyCaptureFrameV1Id::from(55));
        test_dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ExtImageCopyCaptureSessionV1Id::from(54),
            event,
        );
        let attach = with_id(test_request(55, 1, 1), WlBufferId::from(BUFFER));
        test_dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ExtImageCopyCaptureFrameV1Id::from(55),
            attach,
        );
        test_dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ExtImageCopyCaptureFrameV1Id::from(55),
            test_request(55, 3, 0),
        );
        close_toplevel(&mut state, &mut dispatcher, key);
        test_deliver(&mut client, &mut state, &mut dispatcher);
        assert!(test_sent(&dispatcher, handle_id, 0));
        assert!(handle.rref(&state).is_closed());
        assert!(test_sent(&dispatcher, 54, 5));
        assert!(test_sent_arg(&dispatcher, 55, 4, 2));
        let mut event = test_request(51, 0, 2);
        event.id(ExtImageCaptureSourceV1Id::from(56));
        event.id(handle.wayland_id());
        test_dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ExtForeignToplevelImageCaptureSourceManagerV1Id::from(51),
            event,
        );
        assert!(test_closed(&dispatcher, &client));
    }

    #[test]
    fn stopping_a_toplevel_list_sends_finished_once() {
        let (mut client, mut dispatcher, mut state) = client_state();
        ExtForeignToplevelListV1::attach(
            &mut client,
            &mut state,
            &mut dispatcher,
            ExtForeignToplevelListV1Id::from(50),
        );
        test_dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ExtForeignToplevelListV1Id::from(50),
            test_request(50, 0, 0),
        );
        test_dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ExtForeignToplevelListV1Id::from(50),
            test_request(50, 0, 0),
        );
        assert!(test_sent(&dispatcher, 50, 1));
        let key = announce_toplevel(&mut state, &mut dispatcher, "Hidden", "app");
        test_deliver(&mut client, &mut state, &mut dispatcher);
        assert!(state.capture.handles_for(key).is_empty());
    }

    fn insert_toplevel(
        client: &mut crate::clients::Client,
        state: &mut State,
    ) -> Resolved<crate::protocol_generated::XdgToplevelId> {
        client
            .insert_object(
                state,
                crate::wayland_objects::xdg_toplevel::XdgToplevel::new(
                    crate::protocol_generated::XdgToplevelId::from(60),
                    Resolved::new(
                        client.client_id(),
                        crate::protocol_generated::XdgSurfaceId::from(1),
                        crate::state::ObjectId::from(1usize),
                    ),
                    7,
                ),
            )
            .0
    }

    #[test]
    fn mapping_a_toplevel_publishes_its_identity_and_unmap_stops_capture() {
        let (mut client, mut dispatcher, mut state) = client_state();
        let toplevel = insert_toplevel(&mut client, &mut state);
        ExtForeignToplevelListV1::attach(
            &mut client,
            &mut state,
            &mut dispatcher,
            ExtForeignToplevelListV1Id::from(50),
        );
        map_xdg_toplevel(&mut state, &mut dispatcher, toplevel, "Terminal", "app");
        test_deliver(&mut client, &mut state, &mut dispatcher);
        let key = state.capture.by_xdg[&toplevel];
        let handle = state.capture.handles_for(key)[0];
        let handle_id: u32 = handle.wayland_id().into();
        assert_eq!(handle.rref(&state).title(), "Terminal");
        assert_eq!(handle.rref(&state).identifier(), key.identifier());
        assert!(test_sent(&dispatcher, handle_id, 4));
        assert!(test_sent(&dispatcher, handle_id, 1));

        let mut event = test_request(60, 2, 1);
        event.string("Next");
        test_dispatch(&mut client, &mut state, &mut dispatcher, toplevel.wayland_id(), event);
        test_deliver(&mut client, &mut state, &mut dispatcher);
        assert_eq!(handle.rref(&state).title(), "Next");
        retitle_xdg_toplevel(&mut state, &mut dispatcher, toplevel, "Next".to_string());
        assert!(state.capture.pending.get(&client.client_id()).is_none());

        client.insert_object(
            &mut state,
            ExtForeignToplevelImageCaptureSourceManagerV1::new(
                ExtForeignToplevelImageCaptureSourceManagerV1Id::from(51),
            ),
        );
        let mut event = test_request(51, 0, 2);
        event.id(ExtImageCaptureSourceV1Id::from(52));
        event.id(handle.wayland_id());
        test_dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ExtForeignToplevelImageCaptureSourceManagerV1Id::from(51),
            event,
        );
        test_dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ExtForeignToplevelImageCaptureSourceManagerV1Id::from(51),
            test_request(51, 1, 0),
        );
        assert!(client
            .resolve(ExtImageCaptureSourceV1Id::from(52), &state)
            .is_some());
        client.insert_object(
            &mut state,
            ExtImageCopyCaptureManagerV1::new(ExtImageCopyCaptureManagerV1Id::from(53)),
        );
        let mut event = test_request(53, 0, 3);
        event.id(ExtImageCopyCaptureSessionV1Id::from(54));
        event.id(ExtImageCaptureSourceV1Id::from(52));
        event.uint(0);
        test_dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ExtImageCopyCaptureManagerV1Id::from(53),
            event,
        );
        let first_identifier = handle.rref(&state).identifier().to_string();
        unmap_xdg_toplevel(&mut state, &mut dispatcher, toplevel);
        test_deliver(&mut client, &mut state, &mut dispatcher);
        assert!(handle.rref(&state).is_closed());
        assert!(test_sent(&dispatcher, 54, 5));

        map_xdg_toplevel(&mut state, &mut dispatcher, toplevel, "Next", "app");
        test_deliver(&mut client, &mut state, &mut dispatcher);
        let remapped = state.capture.by_xdg[&toplevel];
        let again = state.capture.handles_for(remapped)[0];
        assert_ne!(again.rref(&state).identifier(), first_identifier);
        assert_ne!(remapped, key);
    }

    #[test]
    fn a_later_list_receives_mapped_toplevels_without_replacing_old_handles() {
        let (mut client, mut dispatcher, mut state) = client_state();
        let toplevel = insert_toplevel(&mut client, &mut state);
        map_xdg_toplevel(&mut state, &mut dispatcher, toplevel, "Terminal", "app");
        let first = ExtForeignToplevelListV1::attach(
            &mut client,
            &mut state,
            &mut dispatcher,
            ExtForeignToplevelListV1Id::from(50),
        );
        test_deliver(&mut client, &mut state, &mut dispatcher);
        let key = state.capture.by_xdg[&toplevel];
        assert_eq!(state.capture.handles_for(key).len(), 1);
        let original = state.capture.handles_for(key)[0];
        ExtForeignToplevelListV1::attach(
            &mut client,
            &mut state,
            &mut dispatcher,
            ExtForeignToplevelListV1Id::from(61),
        );
        test_deliver(&mut client, &mut state, &mut dispatcher);
        assert_eq!(state.capture.handles_for(key).len(), 2);
        test_dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            original.wayland_id(),
            test_request(original.wayland_id().into(), 0, 0),
        );
        assert_eq!(state.capture.handles_for(key).len(), 1);
        sync_mapped_toplevels(&mut state, &mut dispatcher, first);
        test_deliver(&mut client, &mut state, &mut dispatcher);
        assert_eq!(state.capture.handles_for(key).len(), 1);
        assert!(test_sent(&dispatcher, 50, 0));
        assert!(test_sent(&dispatcher, 61, 0));
    }

    #[test]
    fn destroying_an_output_source_manager_leaves_the_source() {
        let (mut client, mut dispatcher, mut state) = client_state();
        output_source(&mut client, &mut state, &mut dispatcher);
        test_dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ExtOutputImageCaptureSourceManagerV1Id::from(30),
            test_request(30, 1, 0),
        );
        assert!(client
            .resolve(ExtOutputImageCaptureSourceManagerV1Id::from(30), &state)
            .is_none());
        assert!(client
            .resolve(ExtImageCaptureSourceV1Id::from(31), &state)
            .is_some());
    }

    fn shm_capture_buffer(
        client: &mut crate::clients::Client,
        state: &mut State,
        id: u32,
        width: i32,
        height: i32,
        stride: i32,
        pixels: Vec<u8>,
        format: &'static crate::drm::Format,
    ) -> Resolved<crate::protocol_generated::WlBufferId> {
        client
            .insert_object(
                state,
                WlBuffer::new(NewWlBufferParams {
                    id: crate::protocol_generated::WlBufferId::from(id),
                    owner_client_id: client.client_id(),
                    offset: 0,
                    width,
                    height,
                    stride,
                    format,
                    mem: crate::wayland_objects::wl_shm_pool::shared_memory_from_bytes(&pixels),
                })
                .unwrap(),
            )
            .0
    }

    #[test]
    fn capture_shm_rejects_a_buffer_that_cannot_hold_the_image() {
        let (mut client, _, mut state) = client_state();
        let tight = shm_capture_buffer(
            &mut client,
            &mut state,
            70,
            2,
            2,
            8,
            vec![0; 32],
            crate::drm::ARGB8888,
        );
        assert_eq!(
            tight.rref(&state).begin_capture_write(1, 2).unwrap_err(),
            crate::wayland_objects::wl_buffer::CaptureBufferError::Size
        );
        let short_stride = shm_capture_buffer(
            &mut client,
            &mut state,
            71,
            2,
            1,
            4,
            vec![0; 4],
            crate::drm::ARGB8888,
        );
        assert_eq!(
            short_stride
                .rref(&state)
                .begin_capture_write(2, 1)
                .unwrap_err(),
            crate::wayland_objects::wl_buffer::CaptureBufferError::Stride
        );
        let overflow = shm_capture_buffer(
            &mut client,
            &mut state,
            72,
            2,
            2,
            16,
            vec![0; 16],
            crate::drm::XRGB8888,
        );
        assert_eq!(
            overflow.rref(&state).begin_capture_write(2, 2).unwrap_err(),
            crate::wayland_objects::wl_buffer::CaptureBufferError::Overflow
        );
        tight.ref_mut(&mut state).mark_destroyed();
        assert_eq!(
            tight.rref(&state).begin_capture_write(2, 2).unwrap_err(),
            crate::wayland_objects::wl_buffer::CaptureBufferError::Destroyed
        );
        let format = Box::leak(Box::new(crate::drm::Format {
            drm: drm::buffer::DrmFourcc::Abgr8888,
            ..*crate::drm::ARGB8888
        }));
        let foreign = shm_capture_buffer(&mut client, &mut state, 73, 1, 1, 4, vec![0; 4], format);
        assert_eq!(
            foreign.rref(&state).begin_capture_write(1, 1).unwrap_err(),
            crate::wayland_objects::wl_buffer::CaptureBufferError::Format
        );
    }

    #[test]
    fn a_queued_capture_writes_top_left_rows_once_and_stops_with_its_source() {
        let (mut client, mut dispatcher, mut state) = client_state();
        let output_id = insert_output(&mut client, &mut state);
        let buffer = shm_capture_buffer(
            &mut client,
            &mut state,
            80,
            2,
            1,
            16,
            vec![0xAB; 16],
            crate::drm::ARGB8888,
        );
        let other = shm_capture_buffer(
            &mut client,
            &mut state,
            81,
            2,
            1,
            8,
            vec![0xCD; 8],
            crate::drm::XRGB8888,
        );
        let hidden = submit_capture(
            &mut state,
            &mut dispatcher,
            CaptureJobRequest {
                target: CaptureTarget::Output(output_id),
                buffer,
                width: 2,
                height: 1,
                cursor: CaptureCursor::Hidden,
            },
        )
        .unwrap();
        let painted = submit_capture(
            &mut state,
            &mut dispatcher,
            CaptureJobRequest {
                target: CaptureTarget::Output(output_id),
                buffer: other,
                width: 2,
                height: 1,
                cursor: CaptureCursor::Painted,
            },
        )
        .unwrap();
        assert_eq!(
            submit_capture(
                &mut state,
                &mut dispatcher,
                CaptureJobRequest {
                    target: CaptureTarget::Output(output_id),
                    buffer,
                    width: 2,
                    height: 1,
                    cursor: CaptureCursor::Hidden,
                },
            ),
            Err(CaptureQueueError::BufferBusy)
        );
        assert_eq!(queued_jobs_for_output(&state, output_id), vec![hidden, painted]);
        assert_eq!(capture_job_cursor(&state, painted), Some(CaptureCursor::Painted));
        assert!(dispatcher
            .test_has_queued_action::<crate::scheduler::output_scheduler::OutputSchedulerAction>(
                |action| matches!(
                    action,
                    crate::scheduler::output_scheduler::OutputSchedulerAction::RequestFrame { .. }
                ),
            ));
        let pixels = [1, 2, 3, 4, 5, 6, 7, 8];
        assert_eq!(complete_capture(&mut state, hidden, &pixels, 8), CaptureFinish::Written);
        assert_eq!(
            buffer
                .rref(&state)
                .begin_capture_write(2, 1)
                .unwrap()
                .as_slice(),
            &[1, 2, 3, 4, 5, 6, 7, 8, 0xAB, 0xAB, 0xAB, 0xAB, 0xAB, 0xAB, 0xAB, 0xAB]
        );
        assert_eq!(
            complete_capture(&mut state, hidden, &[9; 8], 8),
            CaptureFinish::AlreadyFinished
        );
        assert_eq!(
            buffer
                .rref(&state)
                .begin_capture_write(2, 1)
                .unwrap()
                .as_slice()[0],
            1
        );
        assert_eq!(capture_job_cursor(&state, painted), Some(CaptureCursor::Painted));
        output_removed(&mut state, &mut dispatcher, output_id);
        assert_eq!(
            complete_capture(&mut state, painted, &[9; 8], 8),
            CaptureFinish::AlreadyFinished
        );
        assert_eq!(
            other
                .rref(&state)
                .begin_capture_write(2, 1)
                .unwrap()
                .as_slice(),
            &[0xCD; 8]
        );

        let key = announce_toplevel(&mut state, &mut dispatcher, "Terminal", "app");
        let toplevel_buffer = shm_capture_buffer(
            &mut client,
            &mut state,
            82,
            1,
            1,
            4,
            vec![0; 4],
            crate::drm::ARGB8888,
        );
        let toplevel_job = submit_capture(
            &mut state,
            &mut dispatcher,
            CaptureJobRequest {
                target: CaptureTarget::Toplevel(key),
                buffer: toplevel_buffer,
                width: 1,
                height: 1,
                cursor: CaptureCursor::Hidden,
            },
        )
        .unwrap();
        close_toplevel(&mut state, &mut dispatcher, key);
        assert_eq!(fail_capture(&mut state, toplevel_job), CaptureFinish::AlreadyFinished);
        assert_eq!(
            submit_capture(
                &mut state,
                &mut dispatcher,
                CaptureJobRequest {
                    target: CaptureTarget::Toplevel(key),
                    buffer: toplevel_buffer,
                    width: 1,
                    height: 1,
                    cursor: CaptureCursor::Hidden,
                },
            ),
            Err(CaptureQueueError::SourceGone)
        );
    }

    #[test]
    fn an_output_region_becomes_buffer_pixels() {
        let scale = crate::state::FractionalScale::ONE;
        assert_eq!(
            output_capture_rect(200, 100, scale, 0, Some((10, 20, 30, 40))),
            Some(CapturePixelRect { x: 10, y: 20, width: 30, height: 40 })
        );
        assert_eq!(
            output_capture_rect(4, 2, scale, 0, Some((3, 1, 8, 8))),
            Some(CapturePixelRect { x: 3, y: 1, width: 1, height: 1 })
        );
        assert_eq!(output_capture_rect(4, 2, scale, 0, Some((10, 0, 1, 1))), None);
        assert_eq!(
            output_capture_rect(8, 4, crate::state::FractionalScale::TWO, 0, Some((1, 1, 2, 1))),
            Some(CapturePixelRect { x: 2, y: 2, width: 4, height: 2 })
        );
        assert_eq!(
            output_capture_rect(8, 4, scale, 1, None),
            Some(CapturePixelRect { x: 0, y: 0, width: 8, height: 4 })
        );
    }

    #[test]
    fn output_capture_records_damage_without_sharing_cursor_modes() {
        let (mut client, mut dispatcher, mut state) = client_state();
        let output_id = state.insert_output(test_output_drm1(0, 0, 4, 2));
        let hidden = shm_capture_buffer(
            &mut client,
            &mut state,
            70,
            4,
            2,
            16,
            vec![0xAB; 32],
            crate::drm::ARGB8888,
        );
        let painted = shm_capture_buffer(
            &mut client,
            &mut state,
            71,
            4,
            2,
            16,
            vec![0xCD; 32],
            crate::drm::ARGB8888,
        );
        let too_small = shm_capture_buffer(
            &mut client,
            &mut state,
            72,
            2,
            2,
            8,
            vec![0; 16],
            crate::drm::ARGB8888,
        );
        assert!(matches!(
            submit_output_capture(
                &mut state,
                &mut dispatcher,
                output_id,
                too_small,
                CaptureCursor::Hidden,
                None,
                false,
            ),
            Err(CaptureQueueError::Buffer(_))
        ));
        assert_eq!(
            submit_output_capture(
                &mut state,
                &mut dispatcher,
                output_id,
                hidden,
                CaptureCursor::Hidden,
                Some((10, 0, 1, 1)),
                false,
            ),
            Err(CaptureQueueError::EmptyRegion)
        );
        let hidden_job = submit_output_capture(
            &mut state,
            &mut dispatcher,
            output_id,
            hidden,
            CaptureCursor::Hidden,
            None,
            true,
        )
        .unwrap();
        let painted_job = submit_output_capture(
            &mut state,
            &mut dispatcher,
            output_id,
            painted,
            CaptureCursor::Painted,
            None,
            true,
        )
        .unwrap();
        assert!(output_capture_pending(&state, output_id));
        let bytes = vec![0x11; 32];
        apply_rendered_output(
            &mut state.capture,
            &state.objects,
            output_id,
            4,
            2,
            crate::state::FractionalScale::ONE,
            0,
            &[CapturePixelRect { x: 1, y: 0, width: 2, height: 1 }],
            vec![
                RenderedOutputCopy {
                    job_id: hidden_job,
                    bytes: bytes.clone(),
                    stride: 16,
                    read_ok: true,
                },
                RenderedOutputCopy {
                    job_id: painted_job,
                    bytes: bytes.clone(),
                    stride: 16,
                    read_ok: true,
                },
            ],
            CaptureTimestamp { sec: 5, nsec: 7 },
        );
        let hidden_sample = capture_sample(&state, hidden_job).unwrap();
        assert_eq!(hidden_sample.flags, 0);
        assert_eq!(hidden_sample.tv_sec_lo, 5);
        assert_eq!(hidden_sample.tv_nsec, 7);
        assert_eq!(
            hidden_sample.damage,
            vec![CapturePixelRect { x: 0, y: 0, width: 4, height: 2 }]
        );
        assert_eq!(
            capture_sample(&state, painted_job).unwrap().damage,
            vec![CapturePixelRect { x: 0, y: 0, width: 4, height: 2 }]
        );
        assert_eq!(
            hidden
                .rref(&state)
                .begin_capture_write(4, 2)
                .unwrap()
                .as_slice()[..4],
            [0x11, 0x11, 0x11, 0x11]
        );
        let again = shm_capture_buffer(
            &mut client,
            &mut state,
            73,
            4,
            2,
            16,
            vec![0; 32],
            crate::drm::ARGB8888,
        );
        let later = submit_output_capture(
            &mut state,
            &mut dispatcher,
            output_id,
            again,
            CaptureCursor::Hidden,
            None,
            true,
        )
        .unwrap();
        apply_rendered_output(
            &mut state.capture,
            &state.objects,
            output_id,
            4,
            2,
            crate::state::FractionalScale::ONE,
            0,
            &[CapturePixelRect { x: 1, y: 0, width: 2, height: 1 }],
            vec![RenderedOutputCopy { job_id: later, bytes, stride: 16, read_ok: true }],
            CaptureTimestamp { sec: 6, nsec: 0 },
        );
        assert_eq!(
            capture_sample(&state, later).unwrap().damage,
            vec![CapturePixelRect { x: 1, y: 0, width: 2, height: 1 }]
        );
        assert!(!output_capture_pending(&state, output_id));
    }

    fn mapped_window(
        client: &mut crate::clients::Client,
        state: &mut State,
        dispatcher: &mut crate::runner::Dispatcher,
    ) -> (Resolved<crate::protocol_generated::WlSurfaceId>, ToplevelKey) {
        let surface = client
            .insert_object(
                state,
                crate::wayland_objects::wl_surface::WlSurface::new(
                    crate::protocol_generated::WlSurfaceId::from(80),
                    client.client_id(),
                    6,
                ),
            )
            .0;
        state.register_root_surface(surface);
        surface
            .ref_mut(state)
            .test_map(crate::state::ObjectId::from(1usize), 10, 20, 30, 40);
        let child = client
            .insert_object(
                state,
                crate::wayland_objects::wl_surface::WlSurface::new(
                    crate::protocol_generated::WlSurfaceId::from(83),
                    client.client_id(),
                    6,
                ),
            )
            .0;
        child
            .ref_mut(state)
            .test_map(crate::state::ObjectId::from(2usize), 0, 0, 10, 4);
        child
            .ref_mut(state)
            .test_mark_subsurface_at(surface, (28, 0));
        surface.ref_mut(state).test_push_current_child(child);
        let xdg_surface = client
            .insert_object(
                state,
                crate::wayland_objects::xdg_surface::XdgSurface::new(
                    crate::protocol_generated::XdgSurfaceId::from(81),
                    Resolved::new(
                        client.client_id(),
                        crate::protocol_generated::XdgWmBaseId::from(1u32),
                        crate::state::ObjectId::from(1usize),
                    ),
                    surface,
                    7,
                ),
            )
            .0;
        surface.ref_mut(state).set_xdg_surface_impl(xdg_surface);
        let toplevel = client
            .insert_object(
                state,
                crate::wayland_objects::xdg_toplevel::XdgToplevel::new(
                    crate::protocol_generated::XdgToplevelId::from(82),
                    xdg_surface,
                    7,
                ),
            )
            .0;
        ExtForeignToplevelListV1::attach(
            client,
            state,
            dispatcher,
            ExtForeignToplevelListV1Id::from(84),
        );
        map_xdg_toplevel(state, dispatcher, toplevel, "Terminal", "app");
        let key = state.capture.by_xdg[&toplevel];
        (surface, key)
    }

    #[test]
    fn a_toplevel_capture_uses_the_rendered_tree_without_consuming_it() {
        let (mut client, mut dispatcher, mut state) = client_state();
        let (surface, key) = mapped_window(&mut client, &mut state, &mut dispatcher);
        assert_eq!(
            toplevel_rendered_bounds(&state, key),
            crate::region::RectI::new(10, 20, 38, 40)
        );
        surface
            .ref_mut(&mut state)
            .test_push_frame_callback_and_feedback();
        let damage_before = surface.rref(&state).render_state().unwrap().damage.clone();
        let _ = peek_toplevel_snapshots(&state, key);
        assert_eq!(surface.rref(&state).test_frame_callback_count(), 1);
        assert_eq!(surface.rref(&state).test_presentation_feedback_count(), 1);
        assert_eq!(surface.rref(&state).render_state().unwrap().damage, damage_before);

        let too_small = shm_capture_buffer(
            &mut client,
            &mut state,
            85,
            2,
            2,
            8,
            vec![0; 16],
            crate::drm::ARGB8888,
        );
        assert!(matches!(
            submit_toplevel_capture(
                &mut state,
                &mut dispatcher,
                key,
                too_small,
                CaptureCursor::Hidden,
                true,
            ),
            Err(CaptureQueueError::Buffer(_))
        ));
        let hidden = shm_capture_buffer(
            &mut client,
            &mut state,
            86,
            38,
            40,
            152,
            vec![0xAB; 6080],
            crate::drm::ARGB8888,
        );
        let painted = shm_capture_buffer(
            &mut client,
            &mut state,
            87,
            38,
            40,
            152,
            vec![0xCD; 6080],
            crate::drm::ARGB8888,
        );
        let hidden_job = submit_toplevel_capture(
            &mut state,
            &mut dispatcher,
            key,
            hidden,
            CaptureCursor::Hidden,
            true,
        )
        .unwrap();
        let painted_job = submit_toplevel_capture(
            &mut state,
            &mut dispatcher,
            key,
            painted,
            CaptureCursor::Painted,
            true,
        )
        .unwrap();
        let bytes = vec![0x11; 6080];
        apply_rendered_toplevel(
            &mut state.capture,
            &state.objects,
            key,
            38,
            40,
            &[CapturePixelRect { x: 1, y: 0, width: 2, height: 1 }],
            RenderedOutputCopy {
                job_id: hidden_job,
                bytes: bytes.clone(),
                stride: 152,
                read_ok: true,
            },
            CaptureTimestamp { sec: 9, nsec: 3 },
        );
        apply_rendered_toplevel(
            &mut state.capture,
            &state.objects,
            key,
            38,
            40,
            &[CapturePixelRect { x: 1, y: 0, width: 2, height: 1 }],
            RenderedOutputCopy { job_id: painted_job, bytes, stride: 152, read_ok: true },
            CaptureTimestamp { sec: 9, nsec: 3 },
        );
        assert_eq!(capture_sample(&state, hidden_job).unwrap().flags, 0);
        assert_eq!(
            capture_sample(&state, hidden_job).unwrap().damage,
            vec![CapturePixelRect { x: 0, y: 0, width: 38, height: 40 }]
        );
        assert_eq!(
            capture_sample(&state, painted_job).unwrap().damage,
            vec![CapturePixelRect { x: 0, y: 0, width: 38, height: 40 }]
        );
        let again = shm_capture_buffer(
            &mut client,
            &mut state,
            88,
            38,
            40,
            152,
            vec![0; 6080],
            crate::drm::ARGB8888,
        );
        let later = submit_toplevel_capture(
            &mut state,
            &mut dispatcher,
            key,
            again,
            CaptureCursor::Hidden,
            true,
        )
        .unwrap();
        apply_rendered_toplevel(
            &mut state.capture,
            &state.objects,
            key,
            38,
            40,
            &[CapturePixelRect { x: 4, y: 5, width: 6, height: 7 }],
            RenderedOutputCopy {
                job_id: later,
                bytes: vec![0x22; 6080],
                stride: 152,
                read_ok: true,
            },
            CaptureTimestamp { sec: 10, nsec: 0 },
        );
        assert_eq!(
            capture_sample(&state, later).unwrap().damage,
            vec![CapturePixelRect { x: 4, y: 5, width: 6, height: 7 }]
        );
        assert_eq!(
            hidden
                .rref(&state)
                .begin_capture_write(38, 40)
                .unwrap()
                .as_slice()[..4],
            [0x11, 0x11, 0x11, 0x11]
        );
    }

    #[test]
    fn a_protected_session_stops_toplevel_capture_and_keeps_the_handle() {
        let (mut client, mut dispatcher, mut state) = client_state();
        let (_surface, key) = mapped_window(&mut client, &mut state, &mut dispatcher);
        test_deliver(&mut client, &mut state, &mut dispatcher);
        let handle = state.capture.handles_for(key)[0];
        let buffer = shm_capture_buffer(
            &mut client,
            &mut state,
            89,
            38,
            40,
            152,
            vec![0; 6080],
            crate::drm::ARGB8888,
        );
        let job = submit_toplevel_capture(
            &mut state,
            &mut dispatcher,
            key,
            buffer,
            CaptureCursor::Hidden,
            false,
        )
        .unwrap();
        let session = client
            .insert_object(
                &mut state,
                ExtImageCopyCaptureSessionV1::new(
                    ExtImageCopyCaptureSessionV1Id::from(90),
                    CaptureTarget::Toplevel(key),
                    false,
                    None,
                ),
            )
            .0;
        state
            .capture
            .track_session(session, CaptureTarget::Toplevel(key), None);
        let lock = client
            .insert_object(
                &mut state,
                crate::wayland_objects::ext_session_lock_v1::ExtSessionLockV1::new(
                    crate::protocol_generated::ExtSessionLockV1Id::from(91),
                    client.client_id(),
                ),
            )
            .0;
        state.session_lock.install(
            client.client_id(),
            lock,
            false,
            crate::session_lock::PreLockFocus::default(),
            [],
        );
        stop_protected_toplevel_captures(&mut state, &mut dispatcher);
        test_deliver(&mut client, &mut state, &mut dispatcher);
        assert_eq!(fail_capture(&mut state, job), CaptureFinish::AlreadyFinished);
        assert!(session.rref(&state).is_stopped());
        assert!(!handle.rref(&state).is_closed());
        assert_eq!(
            submit_toplevel_capture(
                &mut state,
                &mut dispatcher,
                key,
                buffer,
                CaptureCursor::Hidden,
                false,
            ),
            Err(CaptureQueueError::Protected)
        );
    }

    #[test]
    fn shell_output_and_parent_updates_stay_on_the_record() {
        let (mut client, mut dispatcher, mut state) = client_state();
        let (surface, key) = mapped_window(&mut client, &mut state, &mut dispatcher);
        let toplevel = state
            .capture
            .by_xdg
            .iter()
            .find(|(_, mapped)| **mapped == key)
            .map(|(toplevel, _)| *toplevel)
            .expect("mapped toplevel");
        note_xdg_shell_state(&mut state, &mut dispatcher, toplevel, true, false);
        test_deliver(&mut client, &mut state, &mut dispatcher);
        assert!(state.capture.toplevels[&key].maximized);
        assert!(!state.capture.toplevels[&key].fullscreen);
        assert!(!state.capture.toplevels[&key].closed);
        assert_eq!(state.capture.toplevels[&key].title, "Terminal");
        note_xdg_shell_state(&mut state, &mut dispatcher, toplevel, false, true);
        assert!(!state.capture.toplevels[&key].maximized);
        assert!(state.capture.toplevels[&key].fullscreen);

        apply_activated(&mut state, &mut dispatcher, Some(key));
        assert!(state.capture.toplevels[&key].activated);
        apply_activated(&mut state, &mut dispatcher, None);
        assert!(!state.capture.toplevels[&key].activated);

        let parent = announce_toplevel(&mut state, &mut dispatcher, "Parent", "parent");
        let child = announce_toplevel(&mut state, &mut dispatcher, "Child", "child");
        note_parent(&mut state, &mut dispatcher, child, Some(parent));
        assert_eq!(state.capture.toplevels[&child].parent, Some(parent));
        close_toplevel(&mut state, &mut dispatcher, parent);
        assert_eq!(state.capture.toplevels[&child].parent, None);
        assert!(!state.capture.toplevels[&child].closed);

        let output = insert_output(&mut client, &mut state);
        let xdg_surface = surface
            .ref_mut(&mut state)
            .xdg_surface()
            .expect("toplevel surface");
        state.register_xdg_toplevel_role(xdg_surface, toplevel);
        surface
            .ref_mut(&mut state)
            .update_output_membership(vec![output]);
        sync_toplevel_outputs(&mut state, &mut dispatcher, surface);
        assert_eq!(
            state.capture.toplevels[&key].outputs,
            std::collections::BTreeSet::from([output])
        );
        surface
            .ref_mut(&mut state)
            .update_output_membership(Vec::new());
        sync_toplevel_outputs(&mut state, &mut dispatcher, surface);
        assert!(state.capture.toplevels[&key].outputs.is_empty());
        assert!(!state.capture.handles_for(key).is_empty());
    }
}
