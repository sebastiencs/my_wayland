//! `zwlr_data_control_manager_v1`, advertised at version 2.
//!
//! Version 1 binds still only see the clipboard. Version 2 also receives and
//! replaces the primary selection, without keyboard focus.

use crate::{
    clients::{Client, ClientAction, ClientDispatcher, RawRequest},
    globals::Global,
    object::{Object, Parser, RequestResult},
    protocol_generated::{
        zwlr_data_control_manager_v1, Constants, NewId, WaylandId, ZwlrDataControlDeviceV1Id,
        ZwlrDataControlManagerV1Id, ZwlrDataControlSourceV1Id,
    },
    state::{GrabbedObject, State},
    wayland_objects::{
        wl_display::WlDisplay, zwlr_data_control_device_v1::ZwlrDataControlDeviceV1,
        zwlr_data_control_source_v1::ZwlrDataControlSourceV1,
    },
};

/// Clipboard and primary selection. Version 1 clients still bind and never
/// receive `primary_selection`.
pub const ADVERTISED_VERSION: u32 = 2;

#[derive(Debug)]
pub struct ZwlrDataControlManagerV1Global;

impl ZwlrDataControlManagerV1Global {
    pub fn new() -> Box<dyn Global> {
        Box::new(Self)
    }
}

impl Global for ZwlrDataControlManagerV1Global {
    fn interface(&self) -> (&'static str, u32) {
        (ZwlrDataControlManagerV1::INTERFACE_NAME, ADVERTISED_VERSION)
    }

    fn bind(
        &mut self,
        client: &mut Client,
        state: &mut State,
        id: NewId,
        version: u32,
        _dispatcher: &mut ClientDispatcher,
    ) {
        let id = ZwlrDataControlManagerV1Id::from(id);
        client.insert_object(state, ZwlrDataControlManagerV1::new(id, version));
    }
}

#[derive(Debug)]
pub struct ZwlrDataControlManagerV1 {
    id: ZwlrDataControlManagerV1Id,
    version: u32,
}

impl ZwlrDataControlManagerV1 {
    pub fn new(id: ZwlrDataControlManagerV1Id, version: u32) -> Self {
        Self { id, version }
    }
}

impl Object for ZwlrDataControlManagerV1 {
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
        use zwlr_data_control_manager_v1::Request::*;

        match client.parse::<Self>(req, dispatcher)? {
            CreateDataSource { id } => {
                client.insert_object(state, ZwlrDataControlSourceV1::new(id, self.version.min(1)));
            }
            GetDataDevice { id, seat } => {
                client.resolve_or_disconnect(seat, state, dispatcher)?;
                let (device, _) = client
                    .insert_object(state, ZwlrDataControlDeviceV1::new(id, seat, self.version));
                state.seat.register_data_control_device(device);
                dispatcher
                    .dispatch_to(client.client_id(), ClientAction::RefreshDataControlSelection);
                dispatcher.dispatch_to(
                    client.client_id(),
                    ClientAction::RefreshDataControlPrimarySelection,
                );
            }
            Destroy => {
                client.delete_id(&WlDisplay::default(), state, dispatcher, self.id.into());
            }
        }
        Ok(())
    }
}
