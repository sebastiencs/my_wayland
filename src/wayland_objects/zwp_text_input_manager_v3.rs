//! `zwp_text_input_manager_v3`, advertised at version 1.
//!
//! The installed XML is version 2. Version 2 requests can still be parsed, and
//! the text-input object ignores them.

use crate::{
    clients::{Client, ClientDispatcher, EventWriter, RawRequest},
    globals::Global,
    object::{Object, Parser, RequestResult},
    protocol_generated::{
        zwp_text_input_manager_v3, zwp_text_input_v3, Constants, NewId, WaylandId, WlSurfaceId,
        ZwpTextInputManagerV3Id, ZwpTextInputV3Id,
    },
    state::{GrabbedObject, State},
    wayland_objects::{wl_display::WlDisplay, zwp_text_input_v3::ZwpTextInputV3},
};

/// Clients only see version 1. Version 2 adds input-panel and action requests
/// this compositor does not relay.
const ADVERTISED_VERSION: u32 = 1;

#[derive(Debug)]
pub struct ZwpTextInputManagerV3Global;

impl ZwpTextInputManagerV3Global {
    pub fn new() -> Box<dyn Global> {
        Box::new(Self)
    }
}

impl Global for ZwpTextInputManagerV3Global {
    fn interface(&self) -> (&'static str, u32) {
        (ZwpTextInputManagerV3::INTERFACE_NAME, ADVERTISED_VERSION)
    }

    fn bind(
        &mut self,
        client: &mut Client,
        state: &mut State,
        id: NewId,
        version: u32,
        _dispatcher: &mut ClientDispatcher,
    ) {
        client.insert_object(
            state,
            ZwpTextInputManagerV3::new(ZwpTextInputManagerV3Id::from(id), version),
        );
    }
}

#[derive(Debug)]
pub struct ZwpTextInputManagerV3 {
    id: ZwpTextInputManagerV3Id,
    version: u32,
}

impl ZwpTextInputManagerV3 {
    pub fn new(id: ZwpTextInputManagerV3Id, version: u32) -> Self {
        Self { id, version }
    }
}

impl Object for ZwpTextInputManagerV3 {
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
        use zwp_text_input_manager_v3::Request::*;

        match client.parse::<Self>(req, dispatcher)? {
            Destroy => {
                client.delete_id(&WlDisplay::default(), state, dispatcher, self.id.into());
            }
            GetTextInput { id, seat } => {
                client.resolve_or_disconnect(seat, state, dispatcher)?;
                let text_input = client
                    .insert_object(state, ZwpTextInputV3::new(id, self.version))
                    .0;
                state
                    .seat
                    .ime
                    .register_text_input(client.client_id(), text_input);
                let focus = state
                    .seat
                    .keyboard_focus()
                    .filter(|focus| focus.client_id() == client.client_id());
                if let Some(focus) = focus {
                    let surface = focus.surface;
                    let object = text_input.ref_mut(state);
                    object.set_entered(Some(surface));
                    object.send_enter(surface.wayland_id(), dispatcher);
                    object.send_done(dispatcher);
                }
            }
        }
        Ok(())
    }
}

impl ZwpTextInputV3 {
    pub(crate) fn send_enter(&self, surface: WlSurfaceId, dispatcher: &mut ClientDispatcher) {
        self.send_event(zwp_text_input_v3::Event::Enter { surface }, dispatcher);
    }

    pub(crate) fn send_leave(&self, surface: WlSurfaceId, dispatcher: &mut ClientDispatcher) {
        self.send_event(zwp_text_input_v3::Event::Leave { surface }, dispatcher);
    }

    pub(crate) fn send_done(&self, dispatcher: &mut ClientDispatcher) {
        self.send_event(zwp_text_input_v3::Event::Done { serial: self.commit_count() }, dispatcher);
    }
}
