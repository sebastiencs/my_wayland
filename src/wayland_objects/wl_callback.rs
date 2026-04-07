use crate::{
    clients::{Client, ClientDispatcher, EventWriter, RawRequest},
    object::{Object, RequestResult},
    protocol_generated::{wl_callback, WaylandId, WlCallbackId},
    state::{GrabbedObject, State},
};

#[derive(Debug)]
pub struct WlCallback {
    id: WlCallbackId,
}

impl WlCallback {
    pub fn new(id: WlCallbackId) -> Self {
        Self { id }
    }

    pub fn done(&self, callback_data: u32, dispatcher: &mut ClientDispatcher) {
        self.send_event(wl_callback::Event::Done { callback_data }, dispatcher);
    }
}

impl Object for WlCallback {
    fn id(&self) -> WaylandId {
        self.id.into()
    }

    fn on(
        mut self: GrabbedObject<Self>,
        _: &mut Client,
        _: &mut State,
        _: RawRequest,
        _: &mut ClientDispatcher,
    ) -> RequestResult {
        Ok(())
    }
}
