use crate::{
    clients::{Client, ClientDispatcher, EventWriter, RawRequest},
    object::{Object, Parser},
    protocol_generated::{wl_registry, WaylandId, WlRegistryId},
    state::State,
};

#[derive(Debug)]
pub struct WlRegistry {
    id: WlRegistryId,
}

impl WlRegistry {
    pub fn new(id: WlRegistryId) -> Self {
        Self { id }
    }

    pub fn list_global_objects(&self, state: &mut State, dispatcher: &mut ClientDispatcher) {
        for (name, global) in state.globals.iter() {
            let (interface_name, interface_version) = global.interface();
            let global = wl_registry::Event::Global {
                name: name.as_u32(),
                interface: interface_name.to_string(),
                version: interface_version,
            };
            self.send_event(global, dispatcher);
        }
    }
}

impl Object for WlRegistry {
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
        match Self::parse(req).unwrap() {
            wl_registry::Request::Bind { name, id } => {
                state.with_global(name, |global, state| {
                    let (_, version) = global.interface();
                    global.bind(client, state, id, version, dispatcher);
                });
            }
        }
    }

    fn as_any(&mut self) -> &mut dyn std::any::Any {
        self
    }
}
