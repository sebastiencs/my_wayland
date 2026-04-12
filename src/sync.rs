use std::{
    io,
    os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd},
    rc::Rc,
    sync::atomic::{AtomicU64, Ordering},
};

use {
    drm::control::{syncobj, Device},
    nix::unistd::dup,
};

use crate::{
    clients::{ClientDispatcher, Resolved},
    drm::{schedule_active_output, schedule_outputs_for_rect, DrmFd},
    egl::EglContext,
    io_uring::{model::IoUringAction, ring::RingId},
    protocol_generated::{WlBufferId, WlSurfaceId},
    runner::{Dispatcher, Model, Redispatcher},
    state::State,
    wayland_objects::wl_buffer::WlBuffer,
};

#[derive(Debug)]
struct SyncTimelineInner {
    drm: DrmFd,
    handle: syncobj::Handle,
}

impl Drop for SyncTimelineInner {
    fn drop(&mut self) {
        if let Err(error) = self.drm.destroy_syncobj(self.handle) {
            log::warn!(
                "failed to destroy imported syncobj timeline handle={:?}: {:?}",
                self.handle,
                error
            );
        }
    }
}

#[derive(Debug, Clone)]
pub struct SyncTimeline {
    inner: Rc<SyncTimelineInner>,
}

impl SyncTimeline {
    pub fn new(drm: DrmFd, handle: syncobj::Handle) -> Self {
        Self { inner: Rc::new(SyncTimelineInner { drm, handle }) }
    }

    pub fn handle(&self) -> syncobj::Handle {
        self.inner.handle
    }

    pub fn drm(&self) -> &DrmFd {
        &self.inner.drm
    }
}

#[derive(Debug, Clone)]
pub enum AcquireSync {
    None,
    SyncFile(Rc<OwnedFd>),
    TimelinePoint { timeline: SyncTimeline, point: u64 },
}

impl Default for AcquireSync {
    fn default() -> Self {
        Self::None
    }
}

#[derive(Debug, Clone)]
pub enum ReleasePoint {
    None,
    TimelinePoint { timeline: SyncTimeline, point: u64 },
}

impl Default for ReleasePoint {
    fn default() -> Self {
        Self::None
    }
}

#[derive(Debug, Clone, Default)]
pub struct SurfaceSyncState {
    pub acquire: AcquireSync,
    pub release: ReleasePoint,
}

#[derive(Debug, Clone)]
pub struct RetiredBufferCandidate {
    pub buffer: Resolved<WlBufferId>,
    pub sync: SurfaceSyncState,
}

#[derive(Debug, Clone)]
pub struct DeferredRelease {
    pub buffer: Resolved<WlBufferId>,
    pub plain_release: Option<PlainRelease>,
    pub release: ReleasePoint,
}

pub type PlainRelease = Resolved<WlBufferId>;

impl AcquireSync {
    pub fn from_sync_file(fd: OwnedFd) -> Self {
        Self::SyncFile(Rc::new(fd))
    }

    pub fn is_none(&self) -> bool {
        matches!(self, Self::None)
    }

    pub fn is_ready_now(&self) -> bool {
        match self {
            Self::None => true,
            Self::SyncFile(fd) => poll_fd_ready(fd.as_fd(), 0),
            Self::TimelinePoint { timeline, point } => match timeline.drm().syncobj_timeline_wait(
                &[timeline.handle()],
                &[*point],
                0,
                false,
                false,
                true,
            ) {
                Ok(_) => true,
                Err(error) => {
                    let raw = error.raw_os_error();
                    #[cfg(target_os = "linux")]
                    let is_timeout = raw == Some(libc::ETIME) || raw == Some(libc::ETIMEDOUT);
                    #[cfg(not(target_os = "linux"))]
                    let is_timeout = raw == Some(libc::ETIMEDOUT);

                    if is_timeout {
                        false
                    } else {
                        log::warn!(
                            "explicit acquire readiness check failed for syncobj timeline: {:?}",
                            error
                        );
                        false
                    }
                }
            },
        }
    }

    pub fn prepare_wait_fd(&self) -> io::Result<Option<OwnedFd>> {
        match self {
            Self::None => Ok(None),
            _ => self.export_sync_file().map(Some),
        }
    }

    pub fn prepare_waiter_fd(&self) -> io::Result<Option<OwnedFd>> {
        match self {
            Self::None => Ok(None),
            Self::SyncFile(_) => Ok(None),
            Self::TimelinePoint { timeline, point } => {
                let raw = unsafe { libc::eventfd(0, libc::EFD_CLOEXEC | libc::EFD_NONBLOCK) };
                if raw < 0 {
                    return Err(io::Error::last_os_error());
                }
                let eventfd = unsafe { OwnedFd::from_raw_fd(raw) };
                timeline
                    .drm()
                    .syncobj_eventfd(timeline.handle(), *point, eventfd.as_fd(), true)?;
                Ok(Some(eventfd))
            }
        }
    }

    pub fn wait_for_renderer(&self, egl: &EglContext) {
        let Ok(Some(sync_file)) = self.prepare_wait_fd() else {
            return;
        };

        if let Err(error) = egl.wait_on_native_fence(sync_file) {
            log::warn!(
                "renderer explicit acquire wait failed, falling back to CPU wait: {:?}",
                error
            );
            if let Ok(Some(sync_file)) = self.prepare_wait_fd() {
                if let Err(wait_error) = wait_sync_file_cpu(sync_file.as_fd()) {
                    log::warn!("renderer explicit acquire CPU wait also failed: {:?}", wait_error);
                }
            }
        }
    }

    fn export_sync_file(&self) -> io::Result<OwnedFd> {
        match self {
            Self::None => Err(io::Error::new(io::ErrorKind::InvalidInput, "no acquire sync")),
            Self::SyncFile(fd) => {
                let raw = dup(fd.as_fd().as_raw_fd())
                    .map_err(|error| io::Error::from_raw_os_error(error as i32))?;
                Ok(unsafe { OwnedFd::from_raw_fd(raw) })
            }
            Self::TimelinePoint { timeline, point } => {
                export_timeline_point_sync_file(timeline.drm(), timeline.handle(), *point)
            }
        }
    }
}

impl ReleasePoint {
    pub fn is_none(&self) -> bool {
        matches!(self, Self::None)
    }

    pub fn signal(&self) {
        match self {
            Self::None => {}
            Self::TimelinePoint { timeline, point } => {
                if let Err(error) = timeline
                    .drm()
                    .syncobj_timeline_signal(&[timeline.handle()], &[*point])
                {
                    log::warn!(
                        "failed to signal release timeline point handle={:?} point={}: {:?}",
                        timeline.handle(),
                        point,
                        error
                    );
                } else {
                    log::debug!(
                        "signaled release timeline point handle={:?} point={}",
                        timeline.handle(),
                        point
                    );
                }
            }
        }
    }
}

impl DeferredRelease {
    pub fn from_candidate(state: &State, candidate: RetiredBufferCandidate) -> Self {
        let plain_release = state
            .objects
            .try_ref_object::<WlBuffer>(candidate.buffer.object_id())
            .and_then(|buffer| buffer.plain_release_target(candidate.buffer.object_id()));

        Self {
            buffer: candidate.buffer,
            plain_release,
            release: candidate.sync.release,
        }
    }

    pub fn resolve(self, state: &mut State, dispatcher: &mut Dispatcher) {
        self.release.signal();

        if let Some(buffer) = self.plain_release {
            dispatcher.dispatch_to(
                buffer.client_id(),
                crate::clients::ClientAction::ReleaseBuffer { buffer_id: buffer.wayland_id() },
            );
        }

        let _ = state;
    }
}

fn export_timeline_point_sync_file(
    drm: &DrmFd,
    handle: syncobj::Handle,
    point: u64,
) -> io::Result<OwnedFd> {
    let tmp = drm.create_syncobj(false)?;
    let result = (|| {
        drm.syncobj_timeline_transfer(handle, tmp, point, 0)?;
        drm.syncobj_to_fd(tmp, true)
    })();
    if let Err(error) = drm.destroy_syncobj(tmp) {
        log::warn!("failed to destroy temporary syncobj {:?}: {:?}", tmp, error);
    }
    result
}

fn poll_fd_ready(fd: std::os::fd::BorrowedFd<'_>, timeout_ms: i32) -> bool {
    let mut pfd = libc::pollfd { fd: fd.as_raw_fd(), events: libc::POLLIN, revents: 0 };
    let ready = unsafe { libc::poll(&mut pfd, 1, timeout_ms) };
    ready > 0 && (pfd.revents & libc::POLLIN) != 0
}

// Last-resort correctness fallback for sync_file acquires when the renderer
// cannot import/wait on the native fence through EGL.
fn wait_sync_file_cpu(fd: std::os::fd::BorrowedFd<'_>) -> io::Result<()> {
    loop {
        let mut pfd = libc::pollfd { fd: fd.as_raw_fd(), events: libc::POLLIN, revents: 0 };
        let ready = unsafe { libc::poll(&mut pfd, 1, -1) };
        if ready < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
        if (pfd.revents & libc::POLLIN) != 0 {
            return Ok(());
        }
    }
}

model_instance_id!(pub AcquireReadyWaiterId => AcquireReadyWaiter);

fn next_waiter_id() -> AcquireReadyWaiterId {
    static NEXT_ID: AtomicU64 = AtomicU64::new(1);
    AcquireReadyWaiterId::from(NEXT_ID.fetch_add(1, Ordering::Relaxed))
}

pub fn fresh_waiter_id() -> AcquireReadyWaiterId {
    next_waiter_id()
}

pub struct AcquireReadyWaiter {
    waiter_id: AcquireReadyWaiterId,
    surface: Resolved<WlSurfaceId>,
    poll_fd: OwnedFd,
    read_op_id: Option<RingId>,
}

#[derive(Debug)]
pub enum AcquireReadyWaiterAction {
    Start,
    Ready,
    Failed,
    Stop,
}

impl Model for AcquireReadyWaiter {
    type Action = AcquireReadyWaiterAction;

    const START: Option<Self::Action> = Some(Self::Action::Start);
    const STOP: Option<Self::Action> = Some(Self::Action::Stop);

    fn process(
        &mut self,
        state: &mut State,
        action: Self::Action,
        dispatcher: &mut Dispatcher,
    ) -> Result<(), ()> {
        match action {
            AcquireReadyWaiterAction::Start => {
                let read_op_id = RingId::fresh();
                self.read_op_id = Some(read_op_id);
                dispatcher.dispatch(IoUringAction::Read {
                    id: Some(read_op_id),
                    fd: self.poll_fd.as_raw_fd(),
                    offset: 0,
                    length: std::mem::size_of::<u64>(),
                    dispatch_to: Redispatcher::to_instance(self.waiter_id, |result| match result {
                        Ok(_) => AcquireReadyWaiterAction::Ready,
                        Err(error) => {
                            log::warn!("surface acquire waiter poll failed: {:?}", error);
                            AcquireReadyWaiterAction::Failed
                        }
                    }),
                });
            }
            AcquireReadyWaiterAction::Ready => {
                self.read_op_id = None;
                let became_active = {
                    let Some(mut surface) = self.surface.try_grab(state) else {
                        dispatcher.stop_model(self.waiter_id);
                        return Ok(());
                    };
                    surface.activate_blocked_if_waiter_ready(self.waiter_id)
                };

                if became_active {
                    log::debug!(
                        "surface acquire waiter fired: waiter_id={:?} surface={:?}",
                        self.waiter_id,
                        self.surface
                    );
                    if let Some(bounds) =
                        state
                            .root_surface_for_surface(self.surface)
                            .and_then(|root_surface| {
                                state.root_surface_surface_bounds_global(root_surface)
                            })
                    {
                        schedule_outputs_for_rect(state, bounds, dispatcher);
                    } else {
                        schedule_active_output(state, dispatcher);
                    }
                }
                dispatcher.stop_model(self.waiter_id);
            }
            AcquireReadyWaiterAction::Failed => {
                self.read_op_id = None;
                dispatcher.dispatch_to(
                    self.surface.client_id(),
                    crate::clients::ClientAction::CancelBlockedSurface {
                        surface: self.surface,
                        waiter_id: self.waiter_id,
                    },
                );
                dispatcher.stop_model(self.waiter_id);
            }
            AcquireReadyWaiterAction::Stop => {
                if let Some(read_op_id) = self.read_op_id.take() {
                    dispatcher.dispatch(IoUringAction::Cancel { target: read_op_id });
                }
            }
        }

        Ok(())
    }
}

impl AcquireReadyWaiter {
    pub fn new(
        waiter_id: AcquireReadyWaiterId,
        surface: Resolved<WlSurfaceId>,
        poll_fd: OwnedFd,
    ) -> Self {
        Self { waiter_id, surface, poll_fd, read_op_id: None }
    }
}

pub fn resolve_unused_candidate_immediately(
    state: &mut State,
    dispatcher: &mut ClientDispatcher,
    candidate: RetiredBufferCandidate,
) {
    candidate.sync.release.signal();
    let Some(buffer) = candidate.buffer.try_ref_mut(state) else {
        return;
    };
    if buffer
        .plain_release_target(candidate.buffer.object_id())
        .is_some()
    {
        buffer.send_release(dispatcher);
    }
}
