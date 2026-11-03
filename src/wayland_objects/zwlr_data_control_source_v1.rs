//! `zwlr_data_control_source_v1`.
//!
//! MIME types are offered before `set_selection`. Using the source for a selection, or
//! offering another type afterwards, disconnects the client. That matches wlroots and jay.

use crate::{
    clients::{Client, ClientDispatcher, EventWriter, RawRequest, Resolved},
    object::{Object, Parser, RequestResult},
    protocol_generated::{zwlr_data_control_source_v1, WaylandId, ZwlrDataControlSourceV1Id},
    state::{GrabbedObject, State},
    wayland_objects::wl_display::WlDisplay,
};

#[derive(Debug)]
pub struct ZwlrDataControlSourceV1 {
    id: ZwlrDataControlSourceV1Id,
    version: u32,
    mime_types: Vec<String>,
    /// Set once this source is given to `set_selection`.
    used: bool,
}

impl ZwlrDataControlSourceV1 {
    pub fn new(id: ZwlrDataControlSourceV1Id, version: u32) -> Self {
        Self { id, version, mime_types: Vec::new(), used: false }
    }

    pub fn version(&self) -> u32 {
        self.version
    }

    pub fn mime_types(&self) -> &[String] {
        &self.mime_types
    }

    pub fn is_used(&self) -> bool {
        self.used
    }

    /// Returns false when the source was already given to a selection request.
    pub fn try_use(&mut self) -> bool {
        if self.used {
            return false;
        }
        self.used = true;
        true
    }

    pub fn send_send(&self, mime_type: String, fd: i32, dispatcher: &mut ClientDispatcher) {
        self.send_event(zwlr_data_control_source_v1::Event::Send { mime_type, fd }, dispatcher);
    }

    pub fn send_cancelled(&self, dispatcher: &mut ClientDispatcher) {
        self.send_event(zwlr_data_control_source_v1::Event::Cancelled, dispatcher);
    }
}

impl Object for ZwlrDataControlSourceV1 {
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
        use zwlr_data_control_source_v1::Request::*;

        match client.parse::<Self>(req, dispatcher)? {
            Offer { mime_type } => {
                if self.used {
                    client.protocol_violation_disconnect(
                        dispatcher,
                        self.id.into(),
                        zwlr_data_control_source_v1::Error::InvalidOffer as u32,
                        "zwlr_data_control_source_v1.offer after set_selection",
                    );
                    return Ok(());
                }
                if !self.mime_types.contains(&mime_type) {
                    self.mime_types.push(mime_type);
                }
            }
            Destroy => {
                let source_id = self.id;
                drop(self);
                destroy_data_control_source(client, state, dispatcher, source_id, None)?;
                client.delete_id(&WlDisplay::default(), state, dispatcher, source_id.into());
            }
        }
        Ok(())
    }

    fn on_client_disconnect(
        &mut self,
        client: &mut Client,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
    ) {
        let _ = destroy_data_control_source(
            client,
            state,
            dispatcher,
            self.id,
            Some(client.client_id()),
        );
    }
}

pub(crate) fn destroy_data_control_source(
    client: &mut Client,
    state: &mut State,
    dispatcher: &mut ClientDispatcher,
    source_id: ZwlrDataControlSourceV1Id,
    skip_client: Option<crate::clients::ClientId>,
) -> RequestResult {
    let source = client.resolve_or_disconnect(source_id, state, dispatcher)?;
    for offer in state.take_data_offers_for_source(source.object_id()) {
        if let Some(offer_obj) = offer.try_ref_mut(state) {
            offer_obj.clear_source();
        }
    }
    if state
        .seat
        .selection_source()
        .is_some_and(|selection| selection.object_id() == source.object_id())
    {
        state
            .seat
            .replace_selection_source(None, state.seat.selection_serial());
        state
            .seat
            .publish_clipboard_selection(dispatcher, skip_client);
    }
    Ok(())
}
