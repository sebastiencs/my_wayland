use crate::{
    clients::{Client, ClientDispatcher, EventWriter, RawRequest},
    object::{Object, Parser, RequestResult},
    protocol_generated::{wl_display, wl_registry, WaylandId, WlRegistryId},
    state::{GrabbedObject, State},
};

#[derive(Debug)]
pub struct WlRegistry {
    id: WlRegistryId,
}

impl WlRegistry {
    pub fn new(id: WlRegistryId) -> Self {
        Self { id }
    }

    pub fn send_global(
        &self,
        dispatcher: &mut ClientDispatcher,
        name: u32,
        interface: String,
        version: u32,
    ) {
        self.send_event(wl_registry::Event::Global { name, interface, version }, dispatcher);
    }

    pub fn send_global_remove(&self, dispatcher: &mut ClientDispatcher, name: u32) {
        self.send_event(wl_registry::Event::GlobalRemove { name }, dispatcher);
    }

    pub fn list_global_objects(&self, state: &mut State, dispatcher: &mut ClientDispatcher) {
        for (name, global) in state.globals.iter() {
            let (interface_name, interface_version) = global.interface();
            self.send_global(
                dispatcher,
                name.as_u32(),
                interface_name.to_string(),
                interface_version,
            );
        }
    }
}

impl Object for WlRegistry {
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
        match client.parse::<Self>(req).unwrap() {
            wl_registry::Request::Bind { name, id, requested_version } => {
                let Some(mut global) = state.globals.try_grab_global(name) else {
                    client.protocol_violation_disconnect(
                        dispatcher,
                        self.id.into(),
                        wl_display::Error::InvalidObject as u32,
                        "invalid global name",
                    );
                    return Ok(());
                };

                let (_, max_version) = global.interface();
                let version = requested_version.min(max_version);
                global.bind(client, state, id, version, dispatcher);
            }
        }
        Ok(())
    }
}
