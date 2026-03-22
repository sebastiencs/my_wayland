use crate::{
    clients::{Client, ClientDispatcher, EventWriter, RawRequest},
    drm::schedule_active_output,
    object::{Object, Parser},
    protocol_generated::{
        wl_surface, xdg_wm_base, WaylandId, WlBufferId, WlCallbackId, WlSurfaceId, XdgSurfaceId,
    },
    state::{ObjectId, State},
    wayland_objects::{wl_callback::WlCallback, xdg_surface::XdgSurface},
};

use crate::protocol_generated::wl_display;

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum Role {
    None,
    XdgSurface,
}

#[derive(Debug)]
pub struct WlSurface {
    id: WlSurfaceId,
    role: Role,
    impl_id: Option<XdgSurfaceId>,
    version: u32,
    /// back buffer
    pending: Pending,
    /// front buffer
    on_screen: OnScreen,
    /// `wl_surface.frame` callbacks to complete on the next `commit`.
    pending_frame_callbacks: Vec<WlCallbackId>,
}

#[derive(Debug, Default)]
struct Pending {
    /// `None` = no attach this commit; `Some(None)` = detach; `Some(Some(id))` = attach.
    buffer: Option<Option<WlBufferId>>,
    offset: (i32, i32),
}

#[derive(Debug, Default)]
struct OnScreen {
    buffer: Option<WlBufferId>,
    buffer_object_id: Option<ObjectId>,
    x: i32,
    y: i32,
}

#[derive(Clone, Copy, Debug)]
pub struct SurfaceRenderState {
    pub buffer_object_id: ObjectId,
    pub x: i32,
    pub y: i32,
}

struct SurfaceChanges {
    buffer_changed: bool,
}

impl WlSurface {
    pub fn new(id: WlSurfaceId, version: u32) -> Self {
        Self {
            id,
            role: Role::None,
            impl_id: None,
            version,
            pending: Pending::default(),
            on_screen: OnScreen::default(),
            pending_frame_callbacks: Vec::new(),
        }
    }

    /// Whether this surface references `buffer` in pending or current state.
    pub fn references_wl_buffer(&self, buffer: WlBufferId) -> bool {
        self.on_screen.buffer == Some(buffer)
            || matches!(self.pending.buffer, Some(Some(b)) if b == buffer)
    }

    /// Clear references to a destroyed `wl_buffer`.
    pub fn detach_wl_buffer(&mut self, buffer: WlBufferId) {
        if self.on_screen.buffer == Some(buffer) {
            self.on_screen.buffer = None;
            self.on_screen.buffer_object_id = None;
        }
        if let Some(Some(b)) = self.pending.buffer {
            if b == buffer {
                self.pending.buffer = Some(None);
            }
        }
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

        self.pending.buffer = Some(Some(buffer_id));

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

        match self.pending.buffer.take() {
            None => {}
            Some(None) => {
                if let Some(buffer_id) = self.on_screen.buffer.take() {
                    self.on_screen.buffer_object_id = None;
                    let buffer = client.get_object(buffer_id, state);
                    buffer.send_release(dispatcher);
                }
            }
            Some(Some(buffer_id)) => {
                if let Some(old_id) = self.on_screen.buffer.take() {
                    self.on_screen.buffer_object_id = None;
                    let buffer = client.get_object(old_id, state);
                    buffer.send_release(dispatcher);
                }
                let (x, y) = self.pending.offset;
                self.pending.offset = (0, 0);
                self.on_screen.x += x;
                self.on_screen.y += y;

                let buffer = client.get_object(buffer_id, state);
                buffer.update_texture();

                self.on_screen.buffer = Some(buffer_id);
                self.on_screen.buffer_object_id = Some(client.object_id(buffer_id));
            }
        }

        self.flush_frame_callbacks(client, state, dispatcher);

        let SurfaceChanges { buffer_changed } = changes;
        if buffer_changed {
            schedule_active_output(state, dispatcher);
        }
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
        })
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
            Damage { x, y, width, height } => todo!(),
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
            DamageBuffer { x, y, width, height } => todo!(),
            Offset { x, y } => todo!(),
        }
    }
}
