//! `zwp_primary_selection_device_manager_v1`.
//!
//! The global is version 1. A device created for the keyboard-focused client
//! receives the current primary selection immediately. Other clients receive it
//! when they gain keyboard focus.

use crate::{
    clients::{Client, ClientAction, ClientDispatcher, RawRequest},
    globals::Global,
    object::{Object, Parser, RequestResult},
    protocol_generated::{
        zwp_primary_selection_device_manager_v1, Constants, NewId, WaylandId,
        ZwpPrimarySelectionDeviceManagerV1Id, ZwpPrimarySelectionDeviceV1Id,
        ZwpPrimarySelectionSourceV1Id,
    },
    state::{GrabbedObject, State},
    wayland_objects::{
        wl_display::WlDisplay, zwp_primary_selection_device_v1::ZwpPrimarySelectionDeviceV1,
        zwp_primary_selection_source_v1::ZwpPrimarySelectionSourceV1,
    },
};

#[derive(Debug)]
pub struct ZwpPrimarySelectionDeviceManagerV1Global;

impl ZwpPrimarySelectionDeviceManagerV1Global {
    pub fn new() -> Box<dyn Global> {
        Box::new(Self)
    }
}

impl Global for ZwpPrimarySelectionDeviceManagerV1Global {
    fn interface(&self) -> (&'static str, u32) {
        (
            ZwpPrimarySelectionDeviceManagerV1::INTERFACE_NAME,
            ZwpPrimarySelectionDeviceManagerV1::INTERFACE_VERSION,
        )
    }

    fn bind(
        &mut self,
        client: &mut Client,
        state: &mut State,
        id: NewId,
        version: u32,
        _dispatcher: &mut ClientDispatcher,
    ) {
        let id = ZwpPrimarySelectionDeviceManagerV1Id::from(id);
        client.insert_object(state, ZwpPrimarySelectionDeviceManagerV1::new(id, version));
    }
}

#[derive(Debug)]
pub struct ZwpPrimarySelectionDeviceManagerV1 {
    id: ZwpPrimarySelectionDeviceManagerV1Id,
    version: u32,
}

impl ZwpPrimarySelectionDeviceManagerV1 {
    pub fn new(id: ZwpPrimarySelectionDeviceManagerV1Id, version: u32) -> Self {
        Self { id, version }
    }
}

impl Object for ZwpPrimarySelectionDeviceManagerV1 {
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
        use zwp_primary_selection_device_manager_v1::Request::*;

        match client.parse::<Self>(req, dispatcher)? {
            CreateSource { id } => {
                client.insert_object(
                    state,
                    ZwpPrimarySelectionSourceV1::new(id, self.version.min(1)),
                );
            }
            GetDevice { id, seat } => {
                client.resolve_or_disconnect(seat, state, dispatcher)?;
                let (device, _) = client
                    .insert_object(state, ZwpPrimarySelectionDeviceV1::new(id, seat, self.version));
                state.seat.register_primary_selection_device(device);
                if state
                    .seat
                    .keyboard_focus()
                    .is_some_and(|focus| focus.client_id() == client.client_id())
                {
                    dispatcher
                        .dispatch_to(client.client_id(), ClientAction::RefreshPrimarySelection);
                }
            }
            Destroy => {
                client.delete_id(&WlDisplay::default(), state, dispatcher, self.id.into());
            }
        }
        Ok(())
    }
}
