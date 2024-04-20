use crate::{
    clients::{Client, ClientDispatcher, RawRequest},
    object::{Object, Parser},
    protocol_generated::{wl_display, WaylandId, WlDisplayId},
    state::State,
    wayland_objects::{wl_callback::WlCallback, wl_registry::WlRegistry},
};

#[derive(Debug)]
pub struct WlDisplay {
    id: WlDisplayId,
}

impl Default for WlDisplay {
    fn default() -> Self {
        Self { id: 1.into() }
    }
}

impl Object for WlDisplay {
    fn id(&self) -> WaylandId {
        self.id.into()
    }

    fn on(
        &mut self,
        client: &mut Client,
        state: &mut State,
        request: RawRequest,
        dispatcher: &mut ClientDispatcher,
    ) {
        use wl_display::Request::*;

        match Self::parse(request).unwrap() {
            Sync { callback } => {
                let ((id, _), callback) = client.insert_object(state, WlCallback::new(callback));
                callback.done(dispatcher);
                client.delete_id(self, state, dispatcher, id);
            }
            GetRegistry { registry } => {
                let registry = WlRegistry::new(registry);
                registry.list_global_objects(state, dispatcher);
                client.insert_object(state, registry);
            }
        }
    }

    fn as_any(&mut self) -> &mut dyn std::any::Any {
        self
    }
}
