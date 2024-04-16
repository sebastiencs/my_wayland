use crate::{
    clients::{Client, ClientDispatcher, RawRequest},
    globals::Global,
    object::{Object, Parser},
    protocol_generated::{Constants, NewId, WaylandId, WlCompositorId},
    state::State,
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
        _version: u32,
        _dispatcher: &mut ClientDispatcher,
    ) {
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
