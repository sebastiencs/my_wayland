use crate::{
    clients::{Client, ClientDispatcher},
    protocol_generated::NewId,
    state::{IndexMap, State},
    wayland_objects::{wl_compositor::WlCompositorGlobal, wl_shm::WlShmGlobal},
};

pub struct Globals {
    pub map: IndexMap<GlobalName, Box<dyn Global>>,
    current_name: u32,
}

index_map_id!(GlobalName);

impl GlobalName {
    pub fn as_u32(&self) -> u32 {
        self.0
    }
}

pub trait Global {
    /// Returns `(interface_name, interface_version)`
    fn interface(&self) -> (&'static str, u32);
    fn bind(
        &mut self,
        client: &mut Client,
        state: &mut State,
        id: NewId,
        version: u32,
        dispatcher: &mut ClientDispatcher,
    );
}

impl Globals {
    pub fn new() -> Self {
        Self { map: IndexMap::default(), current_name: 1 }
    }

    pub fn initialize(&mut self) {
        for fun in [WlCompositorGlobal::new, WlShmGlobal::new] {
            self.map.insert(fun());
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = (GlobalName, &Box<dyn Global>)> {
        self.map.iter()
    }

    fn add_global<G: Global + 'static>(&mut self, global: G) {
        self.map.insert(Box::new(global));
    }
}
