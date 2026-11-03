//! `zwp_primary_selection_source_v1`.
//!
//! The protocol defines no error for using a source twice. Clients such as Firefox
//! set the same source again, so a later `offer` still adds the MIME type.

use crate::{
    clients::{Client, ClientAction, ClientDispatcher, EventWriter, RawRequest, Resolved},
    object::{Object, Parser, RequestResult},
    protocol_generated::{
        zwp_primary_selection_source_v1, WaylandId, ZwpPrimarySelectionSourceV1Id,
    },
    state::{GrabbedObject, PrimarySelectionSourceState, State},
    wayland_objects::wl_display::WlDisplay,
};

#[derive(Debug)]
pub struct ZwpPrimarySelectionSourceV1 {
    id: ZwpPrimarySelectionSourceV1Id,
    version: u32,
    mime_types: Vec<String>,
}

impl ZwpPrimarySelectionSourceV1 {
    pub fn new(id: ZwpPrimarySelectionSourceV1Id, version: u32) -> Self {
        Self { id, version, mime_types: Vec::new() }
    }

    pub fn version(&self) -> u32 {
        self.version
    }

    pub fn mime_types(&self) -> &[String] {
        &self.mime_types
    }

    pub fn send_send(&self, mime_type: String, fd: i32, dispatcher: &mut ClientDispatcher) {
        self.send_event(zwp_primary_selection_source_v1::Event::Send { mime_type, fd }, dispatcher);
    }

    pub fn send_cancelled(&self, dispatcher: &mut ClientDispatcher) {
        self.send_event(zwp_primary_selection_source_v1::Event::Cancelled, dispatcher);
    }
}

impl Object for ZwpPrimarySelectionSourceV1 {
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
        use zwp_primary_selection_source_v1::Request::*;

        match client.parse::<Self>(req, dispatcher)? {
            Offer { mime_type } => {
                if !self.mime_types.contains(&mime_type) {
                    self.mime_types.push(mime_type);
                }
            }
            Destroy => {
                let source_id = self.id;
                drop(self);
                destroy_primary_selection_source(client, state, dispatcher, source_id, None)?;
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
        let _ = destroy_primary_selection_source(
            client,
            state,
            dispatcher,
            self.id,
            Some(client.client_id()),
        );
    }
}

pub(crate) fn destroy_primary_selection_source(
    client: &mut Client,
    state: &mut State,
    dispatcher: &mut ClientDispatcher,
    source_id: ZwpPrimarySelectionSourceV1Id,
    skip_client: Option<crate::clients::ClientId>,
) -> RequestResult {
    let source = client.resolve_or_disconnect(source_id, state, dispatcher)?;
    if state
        .seat
        .primary_selection_source()
        .is_some_and(|selection| selection.object_id() == source.object_id())
    {
        state
            .seat
            .replace_primary_selection_source(None, state.seat.primary_selection_serial());
        state
            .seat
            .publish_primary_selection(dispatcher, skip_client);
    }
    Ok(())
}

impl PrimarySelectionSourceState {
    pub fn mime_types(self, state: &State) -> Option<Vec<String>> {
        match self {
            Self::Zwp(source) => source
                .try_ref(state)
                .map(|source| source.mime_types().to_vec()),
            Self::DataControl(source) => source
                .try_ref(state)
                .map(|source| source.mime_types().to_vec()),
            Self::Ext(source) => source
                .try_ref(state)
                .map(|source| source.mime_types().to_vec()),
        }
    }

    pub fn send(self, mime_type: String, fd: i32, dispatcher: &mut ClientDispatcher) {
        match self {
            Self::Zwp(source) => {
                dispatcher.dispatch_to(
                    source.client_id(),
                    ClientAction::PrimarySelectionSourceEvent {
                        source,
                        event: crate::clients::PrimarySelectionSourceEvent::Send { mime_type, fd },
                    },
                );
            }
            Self::DataControl(source) => {
                dispatcher.dispatch_to(
                    source.client_id(),
                    ClientAction::DataControlSourceEvent {
                        source,
                        event: crate::clients::DataControlSourceEvent::Send { mime_type, fd },
                    },
                );
            }
            Self::Ext(source) => {
                dispatcher.dispatch_to(
                    source.client_id(),
                    ClientAction::ExtDataControlSourceEvent {
                        source,
                        event: crate::clients::ExtDataControlSourceEvent::Send { mime_type, fd },
                    },
                );
            }
        }
    }

    pub fn cancelled(self, dispatcher: &mut ClientDispatcher) {
        match self {
            Self::Zwp(source) => {
                dispatcher.dispatch_to(
                    source.client_id(),
                    ClientAction::PrimarySelectionSourceEvent {
                        source,
                        event: crate::clients::PrimarySelectionSourceEvent::Cancelled,
                    },
                );
            }
            Self::DataControl(source) => {
                dispatcher.dispatch_to(
                    source.client_id(),
                    ClientAction::DataControlSourceEvent {
                        source,
                        event: crate::clients::DataControlSourceEvent::Cancelled,
                    },
                );
            }
            Self::Ext(source) => {
                dispatcher.dispatch_to(
                    source.client_id(),
                    ClientAction::ExtDataControlSourceEvent {
                        source,
                        event: crate::clients::ExtDataControlSourceEvent::Cancelled,
                    },
                );
            }
        }
    }
}
