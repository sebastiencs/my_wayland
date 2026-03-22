use crate::{
    clients::{Client, ClientDispatcher, RawRequest},
    object::{Object, Parser},
    protocol_generated::{wl_data_source, WaylandId, WlDataSourceId},
    state::State,
    wayland_objects::wl_display::WlDisplay,
};

#[derive(Debug)]
pub struct WlDataSource {
    id: WlDataSourceId,
    version: u32,
}

impl WlDataSource {
    pub fn new(id: WlDataSourceId, version: u32) -> Self {
        Self { id, version }
    }
}

impl Object for WlDataSource {
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
        use wl_data_source::Request::*;

        match client.parse::<Self>(req).unwrap() {
            Offer { mime_type: _ } => {}
            Destroy => {
                client.delete_id(&WlDisplay::default(), state, dispatcher, self.id.into());
            }
            SetActions { dnd_actions: _ } => {
                // Action negotiation comes later.
            }
        }
    }
}
