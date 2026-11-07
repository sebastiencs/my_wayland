//! `zwp_input_method_manager_v2`.
//!
//! One live input method per seat. A second `get_input_method` receives
//! `unavailable` and is not stored, so destroying the first one does not
//! promote it.

use crate::{
    clients::{Client, ClientDispatcher, RawRequest},
    globals::Global,
    input::ime,
    object::{Object, Parser, RequestResult},
    protocol_generated::{
        wl_display, zwp_input_method_manager_v2, Constants, NewId, WaylandId,
        ZwpInputMethodManagerV2Id, ZwpInputMethodV2Id,
    },
    state::{GrabbedObject, State},
    wayland_objects::{wl_display::WlDisplay, zwp_input_method_v2::ZwpInputMethodV2},
};

#[derive(Debug)]
pub struct ZwpInputMethodManagerV2Global;

impl ZwpInputMethodManagerV2Global {
    pub fn new() -> Box<dyn Global> {
        Box::new(Self)
    }
}

impl Global for ZwpInputMethodManagerV2Global {
    fn interface(&self) -> (&'static str, u32) {
        (ZwpInputMethodManagerV2::INTERFACE_NAME, ZwpInputMethodManagerV2::INTERFACE_VERSION)
    }

    fn bind(
        &mut self,
        client: &mut Client,
        state: &mut State,
        id: NewId,
        version: u32,
        dispatcher: &mut ClientDispatcher,
    ) {
        if !state.client_allows_input_method(client) {
            client.protocol_violation_disconnect(
                dispatcher,
                WaylandId::from(u32::from(id)),
                wl_display::Error::InvalidObject as u32,
                "client is not allowed to bind zwp_input_method_manager_v2",
            );
            return;
        }
        client.insert_object(
            state,
            ZwpInputMethodManagerV2::new(ZwpInputMethodManagerV2Id::from(id), version),
        );
    }
}

#[derive(Debug)]
pub struct ZwpInputMethodManagerV2 {
    id: ZwpInputMethodManagerV2Id,
    version: u32,
}

impl ZwpInputMethodManagerV2 {
    pub fn new(id: ZwpInputMethodManagerV2Id, version: u32) -> Self {
        Self { id, version }
    }
}

impl Object for ZwpInputMethodManagerV2 {
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
        use zwp_input_method_manager_v2::Request::*;

        match client.parse::<Self>(req, dispatcher)? {
            Destroy => {
                client.delete_id(&WlDisplay::default(), state, dispatcher, self.id.into());
            }
            GetInputMethod { seat, input_method } => {
                client.resolve_or_disconnect(seat, state, dispatcher)?;
                let live = state.seat.ime.input_method().is_none();
                let (resolved, _) = client
                    .insert_object(state, ZwpInputMethodV2::new(input_method, self.version, !live));
                if !live {
                    resolved.rref(state).send_unavailable(dispatcher);
                    return Ok(());
                }
                state.seat.ime.set_input_method(resolved);
                if let Some(text_input) = state.seat.ime.active_text_input() {
                    ime::relay_existing_activation(state, dispatcher, resolved, text_input);
                }
            }
        }
        Ok(())
    }
}
