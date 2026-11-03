//! `ext_data_control_manager_v1`, advertised at version 1.
//!
//! Version 1 already covers the clipboard and the primary selection. The objects
//! share the seat selection state with `wl_data_device`, `zwp_primary_selection_v1`,
//! and `zwlr_data_control_v1`.

use crate::{
    clients::{Client, ClientAction, ClientDispatcher, RawRequest},
    globals::Global,
    object::{Object, Parser, RequestResult},
    protocol_generated::{
        ext_data_control_manager_v1, Constants, ExtDataControlDeviceV1Id,
        ExtDataControlManagerV1Id, ExtDataControlSourceV1Id, NewId, WaylandId,
    },
    state::{GrabbedObject, State},
    wayland_objects::{
        ext_data_control_device_v1::ExtDataControlDeviceV1,
        ext_data_control_source_v1::ExtDataControlSourceV1, wl_display::WlDisplay,
    },
};

#[derive(Debug)]
pub struct ExtDataControlManagerV1Global;

impl ExtDataControlManagerV1Global {
    pub fn new() -> Box<dyn Global> {
        Box::new(Self)
    }
}

impl Global for ExtDataControlManagerV1Global {
    fn interface(&self) -> (&'static str, u32) {
        (ExtDataControlManagerV1::INTERFACE_NAME, ExtDataControlManagerV1::INTERFACE_VERSION)
    }

    fn bind(
        &mut self,
        client: &mut Client,
        state: &mut State,
        id: NewId,
        version: u32,
        _dispatcher: &mut ClientDispatcher,
    ) {
        let id = ExtDataControlManagerV1Id::from(id);
        client.insert_object(state, ExtDataControlManagerV1::new(id, version));
    }
}

#[derive(Debug)]
pub struct ExtDataControlManagerV1 {
    id: ExtDataControlManagerV1Id,
    version: u32,
}

impl ExtDataControlManagerV1 {
    pub fn new(id: ExtDataControlManagerV1Id, version: u32) -> Self {
        Self { id, version }
    }
}

impl Object for ExtDataControlManagerV1 {
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
        use ext_data_control_manager_v1::Request::*;

        match client.parse::<Self>(req, dispatcher)? {
            CreateDataSource { id } => {
                client.insert_object(state, ExtDataControlSourceV1::new(id, self.version.min(1)));
            }
            GetDataDevice { id, seat } => {
                client.resolve_or_disconnect(seat, state, dispatcher)?;
                let (device, _) = client
                    .insert_object(state, ExtDataControlDeviceV1::new(id, seat, self.version));
                state.seat.register_ext_data_control_device(device);
                dispatcher
                    .dispatch_to(client.client_id(), ClientAction::RefreshExtDataControlSelection);
                dispatcher.dispatch_to(
                    client.client_id(),
                    ClientAction::RefreshExtDataControlPrimarySelection,
                );
            }
            Destroy => {
                client.delete_id(&WlDisplay::default(), state, dispatcher, self.id.into());
            }
        }
        Ok(())
    }
}
