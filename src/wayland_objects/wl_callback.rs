use crate::{
    clients::{Client, ClientDispatcher, EventWriter, RawRequest},
    object::Object,
    protocol_generated::{wl_callback, WaylandId, WlCallbackId},
    state::State,
};

#[derive(Debug)]
pub struct WlCallback {
    id: WlCallbackId,
}

impl WlCallback {
    pub fn new(id: WlCallbackId) -> Self {
        Self { id }
    }

    pub fn done(&self, dispatcher: &mut ClientDispatcher) {
        let callback_data = self.id.into();
        self.send_event(wl_callback::Event::Done { callback_data }, dispatcher);
    }
}

impl Object for WlCallback {
    fn id(&self) -> WaylandId {
        self.id.into()
    }

    fn on(&mut self, _: &mut Client, _: &mut State, _: RawRequest, _: &mut ClientDispatcher) {}
}
