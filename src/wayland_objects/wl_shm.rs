use crate::{
    clients::{Client, ClientDispatcher, RawRequest},
    globals::{Global, GlobalName},
    object::{Object, Parser},
    protocol_generated::{Constants, WaylandId, WlShmId},
    state::State,
};

#[derive(Debug)]
pub struct WlShmGlobal {
    name: GlobalName,
}

impl WlShmGlobal {
    pub fn new(name: GlobalName) -> Box<dyn Global> {
        Box::new(Self { name })
    }
}

impl Global for WlShmGlobal {
    fn global_name(&self) -> GlobalName {
        self.name.clone()
    }
    fn interface(&self) -> (&'static str, u32) {
        (WlShm::INTERFACE_NAME, WlShm::INTERFACE_VERSION)
    }
    fn bind(&mut self, client: &mut Client, state: &mut State, id: WaylandId, _version: u32) {
        let id = WlShmId::from(id);
        client.insert_object(state, WlShm { id });
    }
}

#[derive(Debug)]
pub struct WlShm {
    pub id: WlShmId,
}

impl WlShm {
    pub fn new(id: WlShmId) -> Self {
        Self { id }
    }
}

impl Object for WlShm {
    fn id(&self) -> WaylandId {
        self.id.into()
    }

    fn on(&mut self, _: &mut Client, _: &mut State, req: RawRequest, _: &mut ClientDispatcher) {
        use crate::protocol_generated::wl_shm::Request::*;

        match Self::parse(req).unwrap() {
            CreatePool { id, fd, size } => todo!(),
        }
    }
    fn as_any(&mut self) -> &mut dyn std::any::Any {
        self
    }
}
