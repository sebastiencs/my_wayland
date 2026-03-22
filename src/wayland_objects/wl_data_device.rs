use crate::{
    clients::{Client, ClientDispatcher, RawRequest},
    object::{Object, Parser},
    protocol_generated::{wl_data_device, WaylandId, WlDataDeviceId},
    state::State,
    wayland_objects::wl_display::WlDisplay,
};

#[derive(Debug)]
pub struct WlDataDevice {
    id: WlDataDeviceId,
    version: u32,
}

impl WlDataDevice {
    pub fn new(id: WlDataDeviceId, version: u32) -> Self {
        Self { id, version }
    }
}

impl Object for WlDataDevice {
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
        use wl_data_device::Request::*;

        match client.parse::<Self>(req).unwrap() {
            StartDrag { source: _, origin: _, icon: _, serial: _ } => {
                // Drag-and-drop comes later.
            }
            SetSelection { source: _, serial: _ } => {
                // Clipboard ownership comes later.
            }
            Release => {
                client.delete_id(&WlDisplay::default(), state, dispatcher, self.id.into());
            }
        }
    }
}
