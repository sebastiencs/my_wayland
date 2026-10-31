use crate::{
    clients::{Client, ClientDispatcher, RawRequest},
    globals::Global,
    object::{Object, Parser, RequestResult},
    protocol_generated::{Constants, NewId, WaylandId, WlCompositorId},
    state::{GrabbedObject, State},
    wayland_objects::{wl_region::WlRegion, wl_surface::WlSurface},
};

#[derive(Debug)]
pub struct WlCompositorGlobal;

impl WlCompositorGlobal {
    pub fn new() -> Box<dyn Global> {
        Box::new(Self {})
    }
}

impl Global for WlCompositorGlobal {
    fn interface(&self) -> (&'static str, u32) {
        (WlCompositor::INTERFACE_NAME, WlCompositor::INTERFACE_VERSION)
    }
    fn bind(
        &mut self,
        client: &mut Client,
        state: &mut State,
        id: NewId,
        version: u32,
        _dispatcher: &mut ClientDispatcher,
    ) {
        let id = WlCompositorId::from(id);
        client.insert_object(state, WlCompositor { id, version });
    }
}

#[derive(Debug)]
pub struct WlCompositor {
    id: WlCompositorId,
    version: u32,
}

impl Object for WlCompositor {
    fn id(&self) -> WaylandId {
        self.id.into()
    }

    fn on(
        mut self: GrabbedObject<Self>,
        client: &mut Client,
        state: &mut State,
        req: RawRequest,
        dispatcher: &mut ClientDispatcher,
    ) -> RequestResult {
        use crate::protocol_generated::wl_compositor::Request::*;

        match client.parse::<Self>(req, dispatcher)? {
            CreateSurface { id } => {
                let (resolved, _) = client
                    .insert_object(state, WlSurface::new(id, client.client_id(), self.version));
                state.register_root_surface(resolved);
            }
            CreateRegion { id } => {
                let _ = client.insert_object(state, WlRegion::new(id));
            }
        }
        Ok(())
    }
}
