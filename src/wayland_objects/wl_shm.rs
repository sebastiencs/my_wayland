use crate::{
    clients::{Client, ClientDispatcher, EventWriter, RawRequest},
    drm::{FORMATS, XRGB8888},
    globals::{Global, GlobalName},
    object::{Object, Parser},
    protocol_generated::{wl_shm, Constants, NewId, WaylandId, WlShmId},
    state::State,
};

#[derive(Debug)]
pub struct WlShmGlobal;

impl WlShmGlobal {
    pub fn new() -> Box<dyn Global> {
        Box::new(Self {})
    }
}

impl Global for WlShmGlobal {
    fn interface(&self) -> (&'static str, u32) {
        (WlShm::INTERFACE_NAME, WlShm::INTERFACE_VERSION)
    }
    fn bind(
        &mut self,
        client: &mut Client,
        state: &mut State,
        id: NewId,
        _version: u32,
        dispatcher: &mut ClientDispatcher,
    ) {
        let id = WlShmId::from(id);
        let (_, wl_shm) = client.insert_object(state, WlShm { id });

        // TODO: Send other supported formats
        let format = XRGB8888;
        wl_shm.send_event(wl_shm::Event::Format { format: format.wayland_id }, dispatcher);
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
