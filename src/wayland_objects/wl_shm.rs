use crate::{
    clients::{Client, ClientDispatcher, EventWriter, RawRequest},
    drm::{Format, ARGB8888, FORMATS, XRGB8888},
    globals::{Global, GlobalName},
    object::{Object, Parser},
    protocol_generated::{wl_shm, Constants, NewId, WaylandId, WlShmId},
    state::State,
    wayland_objects::wl_shm_pool::WlShmPool,
};

#[derive(Debug)]
pub struct WlShmGlobal;

impl WlShmGlobal {
    pub fn new() -> Box<dyn Global> {
        Box::new(Self {})
    }
}

pub static SHM_FORMATS: [&'static Format; 2] = [XRGB8888, ARGB8888];

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
        for format in SHM_FORMATS {
            wl_shm.send_event(wl_shm::Event::Format { format: format.wayland_id }, dispatcher);
        }
    }
}

#[derive(Debug)]
pub struct WlShm {
    id: WlShmId,
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

    fn on(
        &mut self,
        client: &mut Client,
        state: &mut State,
        req: RawRequest,
        _: &mut ClientDispatcher,
    ) {
        use crate::protocol_generated::wl_shm::Request::*;

        match client.parse::<Self>(req).unwrap() {
            CreatePool { id, fd, size } => {
                let pool = WlShmPool::new(id, fd, size as usize);
                client.insert_object(state, pool);
            }
        }
    }
}
