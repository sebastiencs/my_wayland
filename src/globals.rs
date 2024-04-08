use std::collections::BTreeMap;

use crate::{
    clients::Client,
    protocol_generated::WaylandId,
    state::State,
    wayland_objects::{wl_compositor::WlCompositorGlobal, wl_shm::WlShmGlobal},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct GlobalName(u32);

impl GlobalName {
    pub fn as_u32(&self) -> u32 {
        self.0
    }
}

pub struct Globals {
    map: BTreeMap<GlobalName, Box<dyn Global>>,
    current_name: u32,
}

pub trait Global {
    fn global_name(&self) -> GlobalName;
    /// Returns `(interface_name, interface_version)`
    fn interface(&self) -> (&'static str, u32);
    fn bind(&mut self, client: &mut Client, state: &mut State, id: WaylandId, version: u32);
}

impl Globals {
    pub fn new() -> Self {
        Self { map: BTreeMap::default(), current_name: 1 }
    }

    pub fn initialize(&mut self) {
        self.map = [WlCompositorGlobal::new, WlShmGlobal::new]
            .into_iter()
            .map(|fun| {
                let name = self.next_name();
                (name, fun(name))
            })
            .collect();
    }

    pub fn iter(&self) -> std::collections::btree_map::Iter<'_, GlobalName, Box<dyn Global>> {
        self.map.iter()
    }

    fn next_name(&mut self) -> GlobalName {
        let next = self.current_name;
        self.current_name += 1;
        GlobalName(next)
    }
    fn add_global<G: Global + 'static>(&mut self, global: G) {
        let name = self.next_name();
        self.map.insert(name, Box::new(global));
    }
}
