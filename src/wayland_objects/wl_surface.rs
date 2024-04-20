use crate::{
    clients::{Client, ClientDispatcher, RawRequest},
    object::{Object, Parser},
    protocol_generated::{xdg_wm_base, WaylandId, WlSurfaceId, XdgSurfaceId},
    state::State,
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
}

impl WlSurface {
    pub fn new(id: WlSurfaceId) -> Self {
        Self { id, role: Role::None, impl_id: None }
    }

    pub fn set_role(&mut self, new_role: Role) -> Result<(), xdg_wm_base::Error> {
        if self.role != Role::None && self.role != new_role {
            return Err(xdg_wm_base::Error::Role);
        }
        self.role = new_role;
        Ok(())
    }

    pub fn set_impl(&mut self, id: XdgSurfaceId) {
        self.impl_id = Some(id);
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

        match Self::parse(req).unwrap() {
            Destroy => todo!(),
            Attach { buffer, x, y } => todo!(),
            Damage { x, y, width, height } => todo!(),
            Frame { callback } => todo!(),
            SetOpaqueRegion { region } => todo!(),
            SetInputRegion { region } => todo!(),
            Commit => {
                let xdg_surface_id = self.impl_id.unwrap();
                client.with_object(xdg_surface_id, state, |xdg_surface, state, client| {
                    xdg_surface.on_commit(client, state, dispatcher);
                });
            }
            SetBufferTransform { transform } => todo!(),
            SetBufferScale { scale } => todo!(),
            DamageBuffer { x, y, width, height } => todo!(),
            Offset { x, y } => todo!(),
        }
    }

    fn as_any(&mut self) -> &mut dyn std::any::Any {
        self
    }
}
