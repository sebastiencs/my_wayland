use crate::{
    clients::{Client, EventWriter, RawRequest},
    object::Object,
    protocol_generated::{WaylandId, WlCallbackId},
    state::Dispatcher,
};

use crate::protocol_generated::wl_callback::*;

#[derive(Debug)]
pub struct WlCallback {
    pub id: WlCallbackId,
}

impl WlCallback {
    pub fn new(id: WlCallbackId) -> Self {
        Self { id }
    }

    pub fn done(&self, dispatcher: &mut Dispatcher) {
        let callback_data = self.id.into();
        self.send_event(Event::Done { callback_data }, dispatcher);
    }
}

impl Object for WlCallback {
    fn id(&self) -> WaylandId {
        self.id.into()
    }

    fn on(&mut self, _: &mut Client, _: RawRequest, _: &mut Dispatcher) {}
}
