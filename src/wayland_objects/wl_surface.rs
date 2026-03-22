use crate::{
    clients::{Client, ClientDispatcher, EventWriter, RawRequest},
    drm::schedule_active_output,
    object::{Object, Parser},
    protocol_generated::{xdg_wm_base, WaylandId, WlBufferId, WlSurfaceId, XdgSurfaceId},
    state::{ObjectId, State},
    utils::MemHelper,
    wayland_objects::xdg_surface::XdgSurface,
};

use crate::protocol_generated::wl_surface;

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
}

#[derive(Debug, Default)]
struct Pending {
    buffer: Option<WlBufferId>,
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
    is_new_buffer: bool,
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
        }
    }

    pub fn set_role(
        &mut self,
        new_role: Role,
        dispatcher: &mut ClientDispatcher,
    ) -> Result<(), xdg_wm_base::Error> {
        if self.role != Role::None && self.role != new_role {
            self.send_error(xdg_wm_base::Error::Role, dispatcher);
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
                self.send_error(wl_surface::Error::InvalidOffset, dispatcher);
                return Err(wl_surface::Error::InvalidOffset);
            }
        } else {
            self.pending.offset = (x, y);
        }

        if client.contains_object(buffer_id, state) {
            self.pending.buffer = Some(buffer_id);
        }

        Ok(())
    }

    fn get_changes(&self) -> SurfaceChanges {
        SurfaceChanges { is_new_buffer: self.pending.buffer.is_some() }
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

        if let Some(buffer_id) = self.pending.buffer.take() {
            if let Some(buffer_id) = self.on_screen.buffer.take() {
                self.on_screen.buffer_object_id = None;
                let buffer = client.get_object(buffer_id, state);
                buffer.send_release(dispatcher);
            }
            let (x, y) = self.pending.offset.take();
            self.on_screen.x += x;
            self.on_screen.y += y;

            let buffer = client.get_object(buffer_id, state);
            buffer.update_texture();

            self.on_screen.buffer = Some(buffer_id);
            self.on_screen.buffer_object_id = Some(client.object_id(buffer_id));
        };

        let SurfaceChanges { is_new_buffer } = changes;
        if is_new_buffer {
            schedule_active_output(state, dispatcher);
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
            Frame { callback } => todo!(),
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
