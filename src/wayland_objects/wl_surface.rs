use crate::{
    clients::{Client, ClientDispatcher, ClientId, EventWriter, RawRequest},
    drm::schedule_active_output,
    object::{Object, Parser},
    protocol_generated::{
        wl_surface, wp_linux_drm_syncobj_surface_v1, xdg_wm_base, WaylandId, WlBufferId,
        WlCallbackId, WlSurfaceId, XdgSurfaceId,
    },
    state::{ObjectId, State},
    sync::{self, AcquireReadyWaiter, RetiredBufferCandidate, SurfaceSyncState},
    wayland_objects::{wl_callback::WlCallback, xdg_surface::XdgSurface},
};

use {super::wl_buffer::WlBuffer, crate::protocol_generated::wl_display};

static LOG_WL_SURFACE_DAMAGE_IGNORED: std::sync::Once = std::sync::Once::new();

fn log_wl_surface_damage_ignored_once() {
    LOG_WL_SURFACE_DAMAGE_IGNORED.call_once(|| {
        log::warn!(
            "wl_surface::damage and wl_surface::damage_buffer are ignored; repainting full buffers on commit until damage tracking is implemented"
        );
    });
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum Role {
    None,
    XdgSurface,
}

#[derive(Debug)]
pub struct WlSurface {
    id: WlSurfaceId,
    client_id: ClientId,
    role: Role,
    impl_id: Option<XdgSurfaceId>,
    version: u32,
    /// back buffer
    pending: Pending,
    /// front buffer
    on_screen: OnScreen,
    blocked: Option<BlockedSurfaceState>,
    blocked_waiter_id: Option<u64>,
    syncobj_surface_object: Option<crate::protocol_generated::WpLinuxDrmSyncobjSurfaceV1Id>,
    retired_buffers: Vec<RetiredBufferCandidate>,
    /// `wl_surface.frame` callbacks to complete on the next `commit`.
    pending_frame_callbacks: Vec<WlCallbackId>,
}

#[derive(Debug, Default)]
struct Pending {
    /// `None` = no attach this commit; `Some(None)` = detach; `Some(Some(id))` = attach.
    buffer: Option<Option<ObjectId>>,
    offset: (i32, i32),
    sync: SurfaceSyncState,
}

#[derive(Clone, Debug, Default)]
struct OnScreen {
    buffer_object_id: Option<ObjectId>,
    x: i32,
    y: i32,
    sync: SurfaceSyncState,
}

#[derive(Clone, Debug)]
pub struct SurfaceRenderState {
    pub buffer_object_id: ObjectId,
    pub x: i32,
    pub y: i32,
    pub sync: SurfaceSyncState,
}

#[derive(Clone, Debug)]
struct BlockedSurfaceState {
    buffer_object_id: ObjectId,
    x: i32,
    y: i32,
    sync: SurfaceSyncState,
}

struct SurfaceChanges {
    buffer_changed: bool,
}

impl WlSurface {
    pub fn new(id: WlSurfaceId, client_id: ClientId, version: u32) -> Self {
        Self {
            id,
            client_id,
            role: Role::None,
            impl_id: None,
            version,
            pending: Pending::default(),
            on_screen: OnScreen::default(),
            blocked: None,
            blocked_waiter_id: None,
            syncobj_surface_object: None,
            retired_buffers: Vec::new(),
            pending_frame_callbacks: Vec::new(),
        }
    }

    pub fn set_syncobj_surface_object(
        &mut self,
        object: crate::protocol_generated::WpLinuxDrmSyncobjSurfaceV1Id,
        dispatcher: &mut ClientDispatcher,
    ) -> Result<(), wp_linux_drm_syncobj_surface_v1::Error> {
        if self.syncobj_surface_object.is_some() {
            self.send_error(
                dispatcher,
                wp_linux_drm_syncobj_surface_v1::Error::NoSurface as u32,
                "surface already has a syncobj surface object",
            );
            return Err(wp_linux_drm_syncobj_surface_v1::Error::NoSurface);
        }
        self.syncobj_surface_object = Some(object);
        Ok(())
    }

    pub fn clear_syncobj_surface_object(
        &mut self,
        object: crate::protocol_generated::WpLinuxDrmSyncobjSurfaceV1Id,
    ) {
        if self.syncobj_surface_object == Some(object) {
            self.syncobj_surface_object = None;
        }
    }

    pub fn pending_sync_mut(&mut self) -> &mut SurfaceSyncState {
        &mut self.pending.sync
    }

    pub fn set_role(
        &mut self,
        new_role: Role,
        dispatcher: &mut ClientDispatcher,
    ) -> Result<(), xdg_wm_base::Error> {
        if self.role != Role::None && self.role != new_role {
            self.send_error(
                dispatcher,
                xdg_wm_base::Error::Role as u32,
                "invalid xdg_surface role",
            );
            return Err(xdg_wm_base::Error::Role);
        }
        self.role = new_role;
        Ok(())
    }

    pub fn set_impl(&mut self, id: XdgSurfaceId) {
        self.impl_id = Some(id);
    }

    fn on_attach(
        &mut self,
        client: &mut Client,
        state: &mut State,
        buffer_id: WlBufferId,
        x: i32,
        y: i32,
        dispatcher: &mut ClientDispatcher,
    ) -> Result<(), wl_surface::Error> {
        if self.version >= 5 {
            if x != 0 || y != 0 {
                self.send_error(
                    dispatcher,
                    wl_surface::Error::InvalidOffset as u32,
                    "non-zero attach offset not allowed",
                );
                return Err(wl_surface::Error::InvalidOffset);
            }
        } else {
            self.pending.offset = (x, y);
        }

        let raw: u32 = buffer_id.into();
        if raw == 0 {
            self.pending.buffer = Some(None);
            return Ok(());
        }

        if !client.contains_object(buffer_id, state) {
            client.protocol_violation_disconnect(
                dispatcher,
                self.id.into(),
                wl_display::Error::InvalidObject as u32,
                "invalid wl_buffer for wl_surface.attach",
            );
            return Ok(());
        }

        self.pending.buffer = Some(Some(client.object_id(buffer_id)));

        Ok(())
    }

    fn get_changes(&self) -> SurfaceChanges {
        SurfaceChanges { buffer_changed: self.pending.buffer.is_some() }
    }

    fn on_commit(
        &mut self,
        client: &mut Client,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
    ) {
        let changes = self.get_changes();

        if let Some(xdg_surface_id) = self.impl_id {
            let mut xdg_surface = client.grab_object(xdg_surface_id, state);
            xdg_surface.on_commit(client, state, dispatcher);
        }

        if !self.validate_pending_sync(state, dispatcher) {
            self.pending.buffer = None;
            self.pending.sync = SurfaceSyncState::default();
            self.pending.offset = (0, 0);
            return;
        }

        let pending_sync = std::mem::take(&mut self.pending.sync);
        match self.pending.buffer.take() {
            None => {}
            Some(None) => {
                self.cancel_blocked(client, state, dispatcher);
                self.retire_current();
                self.on_screen = OnScreen {
                    buffer_object_id: None,
                    x: self.on_screen.x,
                    y: self.on_screen.y,
                    sync: SurfaceSyncState::default(),
                };
            }
            Some(Some(buffer_object_id)) => {
                let (x, y) = self.pending.offset;
                self.pending.offset = (0, 0);
                let next = BlockedSurfaceState {
                    buffer_object_id,
                    x: self.on_screen.x + x,
                    y: self.on_screen.y + y,
                    sync: pending_sync,
                };

                let should_block =
                    self.syncobj_surface_object.is_some() && !next.sync.acquire.is_ready_now();
                if should_block {
                    if let Err(error) = self.install_blocked_surface(
                        next.clone(),
                        client.object_id(self.id),
                        dispatcher,
                    ) {
                        log::warn!(
                            "failed to install acquire waiter for surface {:?}: {:?}; activating immediately",
                            self.id,
                            error
                        );
                        self.cancel_blocked(client, state, dispatcher);
                        self.activate_blocked_state(next);
                    }
                } else {
                    self.cancel_blocked(client, state, dispatcher);
                    self.activate_blocked_state(next);
                }
            }
        }

        self.flush_frame_callbacks(client, state, dispatcher);

        let SurfaceChanges { buffer_changed } = changes;
        if buffer_changed {
            schedule_active_output(state, dispatcher);
        }
    }

    fn validate_pending_sync(
        &mut self,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
    ) -> bool {
        if self.syncobj_surface_object.is_none() {
            return true;
        }

        let pending_sync = &self.pending.sync;
        let sync_present = !pending_sync.acquire.is_none() || !pending_sync.release.is_none();
        let Some(pending_buffer) = self.pending.buffer else {
            if sync_present {
                self.send_error(
                    dispatcher,
                    wp_linux_drm_syncobj_surface_v1::Error::NoBuffer as u32,
                    "explicit sync points require a pending non-null buffer",
                );
                return false;
            }
            return true;
        };

        let Some(buffer_object_id) = pending_buffer else {
            if sync_present {
                self.send_error(
                    dispatcher,
                    wp_linux_drm_syncobj_surface_v1::Error::NoBuffer as u32,
                    "explicit sync points require a pending non-null buffer",
                );
                return false;
            }
            return true;
        };

        let Some(object) = state.objects.try_get(buffer_object_id) else {
            self.send_error(
                dispatcher,
                wl_display::Error::InvalidObject as u32,
                "pending wl_buffer disappeared before commit",
            );
            return false;
        };
        let Some(buffer) = object.downcast_ref::<WlBuffer>() else {
            self.send_error(
                dispatcher,
                wl_display::Error::InvalidObject as u32,
                "pending buffer object is not a wl_buffer",
            );
            return false;
        };
        if !buffer.supports_explicit_sync() {
            self.send_error(
                dispatcher,
                wp_linux_drm_syncobj_surface_v1::Error::UnsupportedBuffer as u32,
                "explicit sync currently only supports dma-buf wl_buffers",
            );
            return false;
        }
        if pending_sync.acquire.is_none() {
            self.send_error(
                dispatcher,
                wp_linux_drm_syncobj_surface_v1::Error::NoAcquirePoint as u32,
                "missing acquire point for explicit-sync surface commit",
            );
            return false;
        }
        if pending_sync.release.is_none() {
            self.send_error(
                dispatcher,
                wp_linux_drm_syncobj_surface_v1::Error::NoReleasePoint as u32,
                "missing release point for explicit-sync surface commit",
            );
            return false;
        }
        if let SurfaceSyncState {
            acquire:
                sync::AcquireSync::TimelinePoint { timeline: acquire_timeline, point: acquire_point },
            release:
                sync::ReleasePoint::TimelinePoint { timeline: release_timeline, point: release_point },
        } = pending_sync
        {
            if acquire_timeline.handle() == release_timeline.handle()
                && acquire_point >= release_point
            {
                self.send_error(
                    dispatcher,
                    wp_linux_drm_syncobj_surface_v1::Error::ConflictingPoints as u32,
                    "acquire timeline point must be strictly less than release point",
                );
                return false;
            }
        }

        true
    }

    fn flush_frame_callbacks(
        &mut self,
        client: &mut Client,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
    ) {
        use crate::wayland_objects::wl_display::WlDisplay;
        let display = WlDisplay::default();
        let callbacks = std::mem::take(&mut self.pending_frame_callbacks);
        for cb in callbacks {
            {
                let callback = client.grab_object(cb, state);
                callback.done(dispatcher);
            }
            client.delete_id(&display, state, dispatcher, cb.into());
        }
    }

    pub fn render_state(&self) -> Option<SurfaceRenderState> {
        Some(SurfaceRenderState {
            buffer_object_id: self.on_screen.buffer_object_id?,
            x: self.on_screen.x,
            y: self.on_screen.y,
            sync: self.on_screen.sync.clone(),
        })
    }

    pub fn take_retired_buffers(&mut self) -> Vec<RetiredBufferCandidate> {
        std::mem::take(&mut self.retired_buffers)
    }

    pub fn activate_blocked_if_waiter_ready(&mut self, waiter_id: u64) -> bool {
        if self.blocked_waiter_id != Some(waiter_id) {
            return false;
        }
        let Some(blocked) = self.blocked.take() else {
            return false;
        };
        self.blocked_waiter_id = None;
        self.activate_blocked_state(blocked);
        true
    }

    fn install_blocked_surface(
        &mut self,
        next: BlockedSurfaceState,
        surface_object_id: ObjectId,
        dispatcher: &mut ClientDispatcher,
    ) -> std::io::Result<()> {
        self.blocked = Some(next);
        let acquire = self.blocked.as_ref().unwrap().sync.acquire.clone();
        let Some(poll_fd) = acquire.prepare_waiter_fd()? else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "missing acquire fd for blocked surface",
            ));
        };
        if let Some(waiter_id) = self.blocked_waiter_id.take() {
            dispatcher.stop_model::<_, AcquireReadyWaiter>(waiter_id);
        }
        let waiter_id = sync::fresh_waiter_id();
        self.blocked_waiter_id = Some(waiter_id);
        dispatcher
            .start_model(waiter_id, AcquireReadyWaiter::new(waiter_id, surface_object_id, poll_fd));
        Ok(())
    }

    fn activate_blocked_state(&mut self, next: BlockedSurfaceState) {
        self.retire_current();
        self.on_screen = OnScreen {
            buffer_object_id: Some(next.buffer_object_id),
            x: next.x,
            y: next.y,
            sync: next.sync,
        };
    }

    fn retire_current(&mut self) {
        if let Some(buffer_object_id) = self.on_screen.buffer_object_id.take() {
            self.retired_buffers.push(RetiredBufferCandidate {
                buffer_object_id,
                sync: self.on_screen.sync.clone(),
            });
        }
        self.on_screen.sync = SurfaceSyncState::default();
    }

    fn cancel_blocked(
        &mut self,
        client: &mut Client,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
    ) {
        if let Some(waiter_id) = self.blocked_waiter_id.take() {
            dispatcher.stop_model::<_, AcquireReadyWaiter>(waiter_id);
        }
        let Some(blocked) = self.blocked.take() else {
            return;
        };
        sync::resolve_unused_candidate_immediately(
            client,
            state,
            dispatcher,
            RetiredBufferCandidate {
                buffer_object_id: blocked.buffer_object_id,
                sync: blocked.sync,
            },
        );
    }

    pub fn client_id(&self) -> ClientId {
        self.client_id
    }

    pub fn surface_id(&self) -> WlSurfaceId {
        self.id
    }
}

impl Object for WlSurface {
    fn id(&self) -> WaylandId {
        self.id.into()
    }

    fn on(
        &mut self,
        client: &mut Client,
        state: &mut State,
        req: RawRequest,
        dispatcher: &mut ClientDispatcher,
    ) {
        use wl_surface::Request::*;

        match client.parse::<Self>(req).unwrap() {
            Destroy => todo!(),
            Attach { buffer, x, y } => {
                self.on_attach(client, state, buffer, x, y, dispatcher);
            }
            Damage { x: _, y: _, width: _, height: _ } => {
                log_wl_surface_damage_ignored_once();
            }
            Frame { callback } => {
                client.insert_object(state, WlCallback::new(callback));
                self.pending_frame_callbacks.push(callback);
            }
            SetOpaqueRegion { region } => todo!(),
            SetInputRegion { region } => todo!(),
            Commit => {
                self.on_commit(client, state, dispatcher);
            }
            SetBufferTransform { transform } => todo!(),
            SetBufferScale { scale } => todo!(),
            DamageBuffer { x: _, y: _, width: _, height: _ } => {
                log_wl_surface_damage_ignored_once();
            }
            Offset { x, y } => todo!(),
        }
    }
}
