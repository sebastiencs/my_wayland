//! `zwp_virtual_keyboard_manager_v1`.
//!
//! Advertised to every client. [`State::client_allows_virtual_keyboard`] is
//! the gate, and today it returns true.

use crate::{
    clients::{Client, ClientDispatcher, RawRequest},
    globals::Global,
    object::{Object, Parser, RequestResult},
    protocol_generated::{
        zwp_virtual_keyboard_manager_v1, Constants, NewId, WaylandId, ZwpVirtualKeyboardManagerV1Id,
    },
    state::{GrabbedObject, State},
    wayland_objects::zwp_virtual_keyboard_v1::ZwpVirtualKeyboardV1,
};

#[derive(Debug)]
pub struct ZwpVirtualKeyboardManagerV1Global;

impl ZwpVirtualKeyboardManagerV1Global {
    pub fn new() -> Box<dyn Global> {
        Box::new(Self)
    }
}

impl Global for ZwpVirtualKeyboardManagerV1Global {
    fn interface(&self) -> (&'static str, u32) {
        (
            ZwpVirtualKeyboardManagerV1::INTERFACE_NAME,
            ZwpVirtualKeyboardManagerV1::INTERFACE_VERSION,
        )
    }

    fn bind(
        &mut self,
        client: &mut Client,
        state: &mut State,
        id: NewId,
        version: u32,
        dispatcher: &mut ClientDispatcher,
    ) {
        if !state.client_allows_virtual_keyboard(client) {
            client.protocol_violation_disconnect(
                dispatcher,
                WaylandId::from(u32::from(id)),
                zwp_virtual_keyboard_manager_v1::Error::Unauthorized as u32,
                "client is not allowed to bind zwp_virtual_keyboard_manager_v1",
            );
            return;
        }
        client.insert_object(
            state,
            ZwpVirtualKeyboardManagerV1::new(ZwpVirtualKeyboardManagerV1Id::from(id), version),
        );
    }
}

#[derive(Debug)]
pub struct ZwpVirtualKeyboardManagerV1 {
    id: ZwpVirtualKeyboardManagerV1Id,
    version: u32,
}

impl ZwpVirtualKeyboardManagerV1 {
    pub fn new(id: ZwpVirtualKeyboardManagerV1Id, version: u32) -> Self {
        Self { id, version }
    }
}

impl Object for ZwpVirtualKeyboardManagerV1 {
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
        use zwp_virtual_keyboard_manager_v1::Request::*;

        match client.parse::<Self>(req, dispatcher)? {
            CreateVirtualKeyboard { seat, id } => {
                client.resolve_or_disconnect(seat, state, dispatcher)?;
                client.insert_object(state, ZwpVirtualKeyboardV1::new(id, self.version));
            }
        }
        Ok(())
    }
}
