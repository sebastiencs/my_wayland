use crate::{
    clients::{Client, ClientDispatcher, RawRequest},
    globals::Global,
    object::{Object, Parser},
    protocol_generated::{wl_subcompositor, Constants, NewId, WaylandId, WlSubcompositorId},
    state::State,
};

#[derive(Debug)]
pub struct WlSubcompositorGlobal;

impl WlSubcompositorGlobal {
    pub fn new() -> Box<dyn Global> {
        Box::new(Self {})
    }
}

impl Global for WlSubcompositorGlobal {
    fn interface(&self) -> (&'static str, u32) {
        (WlSubcompositor::INTERFACE_NAME, WlSubcompositor::INTERFACE_VERSION)
    }
    fn bind(
        &mut self,
        client: &mut Client,
        state: &mut State,
        id: NewId,
        _version: u32,
        _dispatcher: &mut ClientDispatcher,
    ) {
        let id = WlSubcompositorId::from(id);
        client.insert_object(state, WlSubcompositor { id });
    }
}

#[derive(Debug)]
pub struct WlSubcompositor {
    id: WlSubcompositorId,
}

impl WlSubcompositor {
    pub fn new(id: WlSubcompositorId) -> Self {
        Self { id }
    }
}

impl Object for WlSubcompositor {
    fn id(&self) -> WaylandId {
        self.id.into()
    }

    fn on(&mut self, _: &mut Client, _: &mut State, req: RawRequest, _: &mut ClientDispatcher) {
        use wl_subcompositor::Request::*;

        match Self::parse(req).unwrap() {
            Destroy => todo!(),
            GetSubsurface { id, surface, parent } => todo!(),
        }
    }
}
