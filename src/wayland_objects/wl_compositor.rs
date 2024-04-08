use crate::{
    clients::{Client, ClientDispatcher, RawRequest},
    globals::{Global, GlobalName},
    object::{Object, Parser},
    protocol_generated::{Constants, WaylandId, WlCompositorId},
    state::State,
};

#[derive(Debug)]
pub struct WlCompositorGlobal {
    name: GlobalName,
}

impl WlCompositorGlobal {
    pub fn new(name: GlobalName) -> Box<dyn Global> {
        Box::new(Self { name })
    }
}

impl Global for WlCompositorGlobal {
    fn global_name(&self) -> GlobalName {
        self.name.clone()
    }
    fn interface(&self) -> (&'static str, u32) {
        (WlCompositor::INTERFACE_NAME, WlCompositor::INTERFACE_VERSION)
    }
    fn bind(&mut self, client: &mut Client, state: &mut State, id: WaylandId, _version: u32) {
        let id = WlCompositorId::from(id);
        client.insert_object(state, WlCompositor { id });
    }
}

#[derive(Debug)]
pub struct WlCompositor {
    pub id: WlCompositorId,
}

impl WlCompositor {
    pub fn new(id: WlCompositorId) -> Self {
        Self { id }
    }
}

impl Object for WlCompositor {
    fn id(&self) -> WaylandId {
        self.id.into()
    }

    fn on(&mut self, _: &mut Client, _: &mut State, req: RawRequest, _: &mut ClientDispatcher) {
        use crate::protocol_generated::wl_compositor::Request::*;

        match Self::parse(req).unwrap() {
            CreateSurface { id } => todo!(),
            CreateRegion { id } => todo!(),
        }
    }
    fn as_any(&mut self) -> &mut dyn std::any::Any {
        self
    }
}
