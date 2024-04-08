use crate::{
    clients::{Client, ClientDispatcher, EventWriter, RawRequest},
    object::Object,
    protocol_generated::{WaylandId, WlCallbackId},
    state::State,
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

    pub fn done(&self, dispatcher: &mut ClientDispatcher) {
        let callback_data = self.id.into();
        self.send_event(Event::Done { callback_data }, dispatcher);
    }
}

impl Object for WlCallback {
    fn id(&self) -> WaylandId {
        self.id.into()
    }

    fn on(&mut self, _: &mut Client, _: &mut State, _: RawRequest, _: &mut ClientDispatcher) {}

    fn as_any(&mut self) -> &mut dyn std::any::Any {
        self
    }
}
