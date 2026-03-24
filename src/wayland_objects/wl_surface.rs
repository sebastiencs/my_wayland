use crate::{
    clients::{Client, ClientDispatcher, ClientId, EventWriter, RawRequest},
    drm::schedule_active_output,
    object::{Object, Parser},
    protocol_generated::{
        wl_surface, wp_linux_drm_syncobj_surface_v1, wp_viewport, xdg_wm_base, WaylandId,
        WlBufferId, WlCallbackId, WlSurfaceId, XdgSurfaceId,
    },
    state::{ObjectId, State},
    sync::{self, AcquireReadyWaiter, RetiredBufferCandidate, SurfaceSyncState},
    transform::{self, BufferTransform, SizeI, SurfaceViewport},
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
    viewport_object: Option<crate::protocol_generated::WpViewportId>,
    retired_buffers: Vec<RetiredBufferCandidate>,
    /// `wl_surface.frame` callbacks to complete on the next `commit`.
    pending_frame_callbacks: Vec<WlCallbackId>,
}

#[derive(Debug)]
struct Pending {
    /// `None` = no attach this commit; `Some(None)` = detach; `Some(Some(id))` = attach.
    buffer: Option<Option<ObjectId>>,
    offset: (i32, i32),
    sync: SurfaceSyncState,
    buffer_transform: BufferTransform,
    buffer_scale: i32,
    viewport: SurfaceViewport,
    damage_surface: Vec<DamageRect>,
    damage_buffer: Vec<DamageRect>,
}

impl Default for Pending {
    fn default() -> Self {
        Self {
            buffer: None,
            offset: (0, 0),
            sync: SurfaceSyncState::default(),
            buffer_transform: BufferTransform::Normal,
            buffer_scale: 1,
            viewport: SurfaceViewport::default(),
            damage_surface: Vec::new(),
            damage_buffer: Vec::new(),
        }
    }
}

#[derive(Clone, Debug)]
struct OnScreen {
    buffer_object_id: Option<ObjectId>,
    x: i32,
    y: i32,
    sync: SurfaceSyncState,
    buffer_transform: BufferTransform,
    buffer_scale: i32,
    viewport: SurfaceViewport,
    logical_size: Option<SizeI>,
    sample_transform: transform::Mat3,
}

impl Default for OnScreen {
    fn default() -> Self {
        Self {
            buffer_object_id: None,
            x: 0,
            y: 0,
            sync: SurfaceSyncState::default(),
            buffer_transform: BufferTransform::Normal,
            buffer_scale: 1,
            viewport: SurfaceViewport::default(),
            logical_size: None,
            sample_transform: transform::Mat3::identity(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct SurfaceRenderState {
    pub buffer_object_id: ObjectId,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub tex_transform: transform::Mat3,
    pub sync: SurfaceSyncState,
}

#[derive(Clone, Debug)]
struct BlockedSurfaceState {
    buffer_object_id: ObjectId,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    tex_transform: transform::Mat3,
    sync: SurfaceSyncState,
    buffer_transform: BufferTransform,
    buffer_scale: i32,
    viewport: SurfaceViewport,
}

struct SurfaceChanges {
    buffer_changed: bool,
    mapping_changed: bool,
}

#[derive(Clone, Copy, Debug)]
struct DamageRect {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
}

#[derive(Debug)]
struct DamageScaffold {
    surface_to_buffer: Vec<transform::RectF>,
    buffer_to_surface: Vec<transform::RectF>,
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
            viewport_object: None,
            retired_buffers: Vec::new(),
            pending_frame_callbacks: Vec::new(),
        }
    }

    pub fn set_syncobj_surface_object(
        &mut self,
        object: crate::protocol_generated::WpLinuxDrmSyncobjSurfaceV1Id,
    ) -> Result<(), ()> {
        if self.syncobj_surface_object.is_some() {
            return Err(());
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

    pub fn set_viewport_object(
        &mut self,
        object: crate::protocol_generated::WpViewportId,
    ) -> Result<(), ()> {
        if self.viewport_object.is_some() {
            return Err(());
        }
        self.viewport_object = Some(object);
        Ok(())
    }

    pub fn clear_viewport_object(&mut self, object: crate::protocol_generated::WpViewportId) {
        if self.viewport_object == Some(object) {
            self.viewport_object = None;
            self.pending.viewport = SurfaceViewport::default();
        }
    }

    fn send_viewport_error(
        &self,
        client: &Client,
        dispatcher: &mut ClientDispatcher,
        code: u32,
        message: impl AsRef<str>,
    ) {
        let Some(viewport) = self.viewport_object else {
            log::warn!(
                "viewport protocol error without viewport object on wl_surface {:?}: {}",
                self.id,
                message.as_ref()
            );
            return;
        };
        client.send_protocol_error(dispatcher, viewport.into(), code, message);
    }

    pub fn pending_sync_mut(&mut self) -> &mut SurfaceSyncState {
        &mut self.pending.sync
    }

    pub fn pending_viewport_mut(&mut self) -> &mut SurfaceViewport {
        &mut self.pending.viewport
    }

    pub fn has_syncobj_surface_object(&self) -> bool {
        self.syncobj_surface_object.is_some()
    }

    fn send_syncobj_surface_error(
        &self,
        client: &Client,
        dispatcher: &mut ClientDispatcher,
        code: u32,
        message: impl AsRef<str>,
    ) {
        let Some(syncobj_surface) = self.syncobj_surface_object else {
            log::warn!(
                "syncobj protocol error without syncobj surface object on wl_surface {:?}: {}",
                self.id,
                message.as_ref()
            );
            return;
        };
        client.send_protocol_error(dispatcher, syncobj_surface.into(), code, message);
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
        SurfaceChanges {
            buffer_changed: self.pending.buffer.is_some(),
            mapping_changed: self.pending.buffer_transform != self.on_screen.buffer_transform
                || self.pending.buffer_scale != self.on_screen.buffer_scale
                || self.pending.viewport != self.on_screen.viewport,
        }
    }

    fn validate_pending_mapping(
        &self,
        client: &Client,
        state: &State,
        dispatcher: &mut ClientDispatcher,
    ) -> bool {
        let Some(buffer_object_id) = self.pending.buffer.flatten().or(self.on_screen.buffer_object_id) else {
            return true;
        };
        let Some(object) = state.objects.try_get(buffer_object_id) else {
            return true;
        };
        let Some(buffer) = object.downcast_ref::<WlBuffer>() else {
            return true;
        };

        let buffer_size = buffer.dimensions();
        let buffer_size = SizeI { width: buffer_size.0, height: buffer_size.1 };
        let base = transform::pre_viewport_logical_size(
            buffer_size,
            self.pending.buffer_transform,
            self.pending.buffer_scale,
        );
        if let Some(src) = self.pending.viewport.src {
            if src.width <= 0.0 || src.height <= 0.0 || src.x < 0.0 || src.y < 0.0 {
                self.send_viewport_error(
                    client,
                    dispatcher,
                    wp_viewport::Error::BadValue as u32,
                    "viewport source must have positive size and non-negative origin",
                );
                return false;
            }
            if self.pending.viewport.dst.is_none()
                && (src.width.fract() != 0.0 || src.height.fract() != 0.0)
            {
                self.send_viewport_error(
                    client,
                    dispatcher,
                    wp_viewport::Error::BadSize as u32,
                    "viewport source must have integer size when destination is unset",
                );
                return false;
            }
            let content = transform::RectF { x: 0.0, y: 0.0, width: base.width, height: base.height };
            if !transform::rect_contains(content, src) {
                self.send_viewport_error(
                    client,
                    dispatcher,
                    wp_viewport::Error::OutOfBuffer as u32,
                    "viewport source rectangle extends outside the transformed/scaled buffer",
                );
                return false;
            }
        }
        true
    }

    fn mapped_state_for_buffer(
        &self,
        state: &State,
        buffer_object_id: ObjectId,
        x: i32,
        y: i32,
        sync: SurfaceSyncState,
    ) -> Option<BlockedSurfaceState> {
        let object = state.objects.try_get(buffer_object_id)?;
        let buffer = object.downcast_ref::<WlBuffer>()?;
        let buffer_size = buffer.dimensions();
        let buffer_size = SizeI { width: buffer_size.0, height: buffer_size.1 };
        let logical_size = transform::derive_logical_size(
            buffer_size,
            self.pending.buffer_transform,
            self.pending.buffer_scale,
            self.pending.viewport,
        );
        let tex_transform = transform::surface_sample_transform(
            buffer_size,
            self.pending.buffer_transform,
            self.pending.buffer_scale,
            self.pending.viewport,
        );
        Some(BlockedSurfaceState {
            buffer_object_id,
            x,
            y,
            width: logical_size.width,
            height: logical_size.height,
            tex_transform,
            sync,
            buffer_transform: self.pending.buffer_transform,
            buffer_scale: self.pending.buffer_scale,
            viewport: self.pending.viewport,
        })
    }

    fn pending_damage_scaffold(
        &self,
        state: &State,
        buffer_object_id: ObjectId,
        logical_size: SizeI,
    ) -> Option<DamageScaffold> {
        let object = state.objects.try_get(buffer_object_id)?;
        let buffer = object.downcast_ref::<WlBuffer>()?;
        let raw_size = buffer.dimensions();
        let raw_size = SizeI { width: raw_size.0, height: raw_size.1 };
        let surface_to_buffer = self
            .pending
            .damage_surface
            .iter()
            .map(|rect| {
                transform::surface_rect_to_buffer_rect(
                    transform::RectF {
                        x: rect.x as f32,
                        y: rect.y as f32,
                        width: rect.width as f32,
                        height: rect.height as f32,
                    },
                    logical_size,
                    raw_size,
                    self.pending.buffer_transform,
                    self.pending.buffer_scale,
                    self.pending.viewport,
                )
            })
            .collect();
        let buffer_to_surface = self
            .pending
            .damage_buffer
            .iter()
            .filter_map(|rect| {
                transform::buffer_rect_to_surface_rect(
                    transform::RectF {
                        x: rect.x as f32,
                        y: rect.y as f32,
                        width: rect.width as f32,
                        height: rect.height as f32,
                    },
                    logical_size,
                    raw_size,
                    self.pending.buffer_transform,
                    self.pending.buffer_scale,
                    self.pending.viewport,
                )
            })
            .collect();
        Some(DamageScaffold { surface_to_buffer, buffer_to_surface })
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

        if !self.validate_pending_mapping(client, state, dispatcher) {
            self.pending.buffer = None;
            self.pending.offset = (0, 0);
            self.pending.damage_surface.clear();
            self.pending.damage_buffer.clear();
            return;
        }

        if !self.validate_pending_sync(client, state, dispatcher) {
            self.pending.buffer = None;
            self.pending.sync = SurfaceSyncState::default();
            self.pending.offset = (0, 0);
            self.pending.damage_surface.clear();
            self.pending.damage_buffer.clear();
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
                    buffer_transform: self.pending.buffer_transform,
                    buffer_scale: self.pending.buffer_scale,
                    viewport: self.pending.viewport,
                    logical_size: None,
                    sample_transform: transform::Mat3::identity(),
                };
            }
            Some(Some(buffer_object_id)) => {
                let (x, y) = self.pending.offset;
                self.pending.offset = (0, 0);
                let Some(next) = self.mapped_state_for_buffer(
                    state,
                    buffer_object_id,
                    self.on_screen.x + x,
                    self.on_screen.y + y,
                    pending_sync,
                ) else {
                    log::warn!("failed to derive mapped state for buffer {:?}", buffer_object_id);
                    return;
                };

                let should_block =
                    self.syncobj_surface_object.is_some() && !next.sync.acquire.is_ready_now();
                if should_block {
                    if let Err(error) = self.install_blocked_surface(
                        next.clone(),
                        self.client_id,
                        client.object_id(self.id),
                        dispatcher,
                    ) {
                        log::warn!(
                            "failed to install acquire waiter for surface {:?}: {:?}; keeping previous contents active",
                            self.id,
                            error
                        );
                        self.cancel_blocked(client, state, dispatcher);
                        return;
                    }
                } else {
                    self.cancel_blocked(client, state, dispatcher);
                    self.activate_blocked_state(next);
                }
            }
        }

        if !changes.buffer_changed && changes.mapping_changed && self.on_screen.buffer_object_id.is_some()
        {
            let Some(next) = self.mapped_state_for_buffer(
                state,
                self.on_screen.buffer_object_id.unwrap(),
                self.on_screen.x,
                self.on_screen.y,
                self.on_screen.sync.clone(),
            ) else {
                log::warn!("failed to recompute mapped state for current surface {:?}", self.id);
                return;
            };
            self.on_screen.buffer_transform = next.buffer_transform;
            self.on_screen.buffer_scale = next.buffer_scale;
            self.on_screen.viewport = next.viewport;
            self.on_screen.logical_size = Some(SizeI { width: next.width, height: next.height });
            self.on_screen.sample_transform = next.tex_transform;
        }

        let SurfaceChanges { buffer_changed, mapping_changed } = changes;
        if buffer_changed || mapping_changed {
            log::debug!(
                "wl_surface commit applied surface={:?} buffer={:?} logical_size={:?} pos=({}, {}) transform={:?} scale={} viewport={:?}",
                self.id,
                self.on_screen.buffer_object_id,
                self.on_screen.logical_size,
                self.on_screen.x,
                self.on_screen.y,
                self.on_screen.buffer_transform,
                self.on_screen.buffer_scale,
                self.on_screen.viewport
            );
        }

        let damage_scaffold = self
            .on_screen
            .buffer_object_id
            .zip(self.on_screen.logical_size)
            .and_then(|(buffer_object_id, logical_size)| {
                self.pending_damage_scaffold(state, buffer_object_id, logical_size)
            });
        if let Some(damage) = damage_scaffold.as_ref() {
            if !damage.surface_to_buffer.is_empty() || !damage.buffer_to_surface.is_empty() {
                log::debug!(
                    "wl_surface commit damage scaffold surface={:?} transform={:?} scale={} viewport={:?} surface_to_buffer={:?} buffer_to_surface={:?}",
                    self.id,
                    self.on_screen.buffer_transform,
                    self.on_screen.buffer_scale,
                    self.on_screen.viewport,
                    damage.surface_to_buffer,
                    damage.buffer_to_surface
                );
            }
        }

        self.pending.damage_surface.clear();
        self.pending.damage_buffer.clear();

        self.flush_frame_callbacks(client, state, dispatcher);

        if buffer_changed || mapping_changed {
            schedule_active_output(state, dispatcher);
        }
    }

    fn validate_pending_sync(
        &mut self,
        client: &mut Client,
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
                self.send_syncobj_surface_error(
                    client,
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
                self.send_syncobj_surface_error(
                    client,
                    dispatcher,
                    wp_linux_drm_syncobj_surface_v1::Error::NoBuffer as u32,
                    "explicit sync points require a pending non-null buffer",
                );
                return false;
            }
            return true;
        };

        let Some(object) = state.objects.try_get(buffer_object_id) else {
            client.protocol_violation_disconnect(
                dispatcher,
                self.id.into(),
                wl_display::Error::InvalidObject as u32,
                "pending wl_buffer disappeared before commit",
            );
            return false;
        };
        let Some(buffer) = object.downcast_ref::<WlBuffer>() else {
            client.protocol_violation_disconnect(
                dispatcher,
                self.id.into(),
                wl_display::Error::InvalidObject as u32,
                "pending buffer object is not a wl_buffer",
            );
            return false;
        };
        if !buffer.supports_explicit_sync() {
            self.send_syncobj_surface_error(
                client,
                dispatcher,
                wp_linux_drm_syncobj_surface_v1::Error::UnsupportedBuffer as u32,
                "explicit sync currently only supports dma-buf wl_buffers",
            );
            return false;
        }
        if pending_sync.acquire.is_none() {
            self.send_syncobj_surface_error(
                client,
                dispatcher,
                wp_linux_drm_syncobj_surface_v1::Error::NoAcquirePoint as u32,
                "missing acquire point for explicit-sync surface commit",
            );
            return false;
        }
        if pending_sync.release.is_none() {
            self.send_syncobj_surface_error(
                client,
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
                self.send_syncobj_surface_error(
                    client,
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
        let logical_size = self.on_screen.logical_size?;
        Some(SurfaceRenderState {
            buffer_object_id: self.on_screen.buffer_object_id?,
            x: self.on_screen.x,
            y: self.on_screen.y,
            width: logical_size.width,
            height: logical_size.height,
            tex_transform: self.on_screen.sample_transform,
            sync: self.on_screen.sync.clone(),
        })
    }

    pub fn logical_size(&self) -> Option<SizeI> {
        self.on_screen.logical_size
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
        client_id: ClientId,
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
        dispatcher.start_model(
            waiter_id,
            AcquireReadyWaiter::new(waiter_id, client_id, surface_object_id, poll_fd),
        );
        Ok(())
    }

    fn activate_blocked_state(&mut self, next: BlockedSurfaceState) {
        self.retire_current();
        self.on_screen = OnScreen {
            buffer_object_id: Some(next.buffer_object_id),
            x: next.x,
            y: next.y,
            sync: next.sync,
            buffer_transform: next.buffer_transform,
            buffer_scale: next.buffer_scale,
            viewport: next.viewport,
            logical_size: Some(SizeI { width: next.width, height: next.height }),
            sample_transform: next.tex_transform,
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
        self.on_screen.logical_size = None;
        self.on_screen.sample_transform = transform::Mat3::identity();
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

    pub fn cancel_blocked_if_waiter_matches(
        &mut self,
        waiter_id: u64,
        client: &mut Client,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
    ) {
        if self.blocked_waiter_id != Some(waiter_id) {
            return;
        }
        self.cancel_blocked(client, state, dispatcher);
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
            Damage { x, y, width, height } => {
                self.pending.damage_surface.push(DamageRect { x, y, width, height });
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
            SetBufferTransform { transform } => {
                let Some(transform) = BufferTransform::from_wayland(transform) else {
                    self.send_error(
                        dispatcher,
                        wl_surface::Error::InvalidTransform as u32,
                        "invalid wl_surface buffer transform",
                    );
                    return;
                };
                self.pending.buffer_transform = transform;
            }
            SetBufferScale { scale } => {
                if scale < 1 {
                    self.send_error(
                        dispatcher,
                        wl_surface::Error::InvalidScale as u32,
                        "wl_surface buffer scale must be at least 1",
                    );
                    return;
                }
                self.pending.buffer_scale = scale;
            }
            DamageBuffer { x, y, width, height } => {
                self.pending.damage_buffer.push(DamageRect { x, y, width, height });
                log_wl_surface_damage_ignored_once();
            }
            Offset { x, y } => todo!(),
        }
    }
}
