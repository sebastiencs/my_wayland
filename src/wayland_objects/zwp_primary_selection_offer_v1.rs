//! `zwp_primary_selection_offer_v1`.
//!
//! `receive` forwards to the primary-selection source captured when the offer was created.

use std::os::fd::{FromRawFd, OwnedFd};

use crate::{
    clients::{Client, ClientDispatcher, EventWriter, RawRequest, Resolved},
    object::{Object, Parser, RequestResult},
    protocol_generated::{
        zwp_primary_selection_offer_v1, WaylandId, ZwpPrimarySelectionDeviceV1Id,
        ZwpPrimarySelectionOfferV1Id,
    },
    state::{GrabbedObject, PrimarySelectionSourceState, State},
    wayland_objects::wl_display::WlDisplay,
};

#[derive(Debug)]
pub struct ZwpPrimarySelectionOfferV1 {
    id: ZwpPrimarySelectionOfferV1Id,
    version: u32,
    source: Option<PrimarySelectionSourceState>,
}

impl ZwpPrimarySelectionOfferV1 {
    pub fn new(
        id: ZwpPrimarySelectionOfferV1Id,
        version: u32,
        source: PrimarySelectionSourceState,
    ) -> Self {
        Self { id, version, source: Some(source) }
    }

    pub fn version(&self) -> u32 {
        self.version
    }

    pub fn send_offer(&self, mime_type: String, dispatcher: &mut ClientDispatcher) {
        self.send_event(zwp_primary_selection_offer_v1::Event::Offer { mime_type }, dispatcher);
    }

    fn clear_source(&mut self) {
        self.source = None;
    }
}

/// Detach the offer without destroying the Wayland object. The client destroys
/// the previous offer itself after the next `selection` event, so the id stays
/// valid until that request arrives. `receive` on an inert offer closes the fd.
pub(crate) fn inert_primary_selection_offer(
    offer: Resolved<ZwpPrimarySelectionOfferV1Id>,
    state: &mut State,
) {
    if let Some(mut offer_obj) = offer.try_ref_mut(state) {
        offer_obj.clear_source();
    }
}

impl Object for ZwpPrimarySelectionOfferV1 {
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
        use zwp_primary_selection_offer_v1::Request::*;

        match client.parse::<Self>(req, dispatcher)? {
            Receive { mime_type, fd } => {
                if let Some(source) = self.source {
                    source.send(mime_type, fd, dispatcher);
                } else {
                    drop(fd);
                }
            }
            Destroy => {
                let offer_id = self.id;
                drop(self);
                if let Some(offer) = client.resolve(offer_id, state) {
                    destroy_primary_selection_offer(client, state, dispatcher, offer);
                }
            }
        }
        Ok(())
    }
}

pub(crate) fn create_primary_selection_offer(
    client: &mut Client,
    state: &mut State,
    dispatcher: &mut ClientDispatcher,
    device: Resolved<ZwpPrimarySelectionDeviceV1Id>,
    selection: PrimarySelectionSourceState,
) -> Option<Resolved<ZwpPrimarySelectionOfferV1Id>> {
    if !device.exists(state) {
        return None;
    }
    let mime_types = selection.mime_types(state)?;
    let version = device.rref(state).version();
    let offer_id = client.allocate_server_object_id::<ZwpPrimarySelectionOfferV1Id>();
    let offer = ZwpPrimarySelectionOfferV1::new(offer_id, version, selection);
    let (resolved, _) = client.insert_object(state, offer);
    {
        let device = device.rref(state);
        device.send_data_offer(resolved, dispatcher);
    }
    for mime_type in mime_types {
        client
            .ref_object(offer_id, state)
            .send_offer(mime_type, dispatcher);
    }
    Some(resolved)
}

pub(crate) fn destroy_primary_selection_offer(
    client: &mut Client,
    state: &mut State,
    dispatcher: &mut ClientDispatcher,
    offer: Resolved<ZwpPrimarySelectionOfferV1Id>,
) {
    if offer.client_id() != client.client_id() || !offer.exists(state) {
        return;
    }
    if let Some(mut offer_obj) = offer.try_grab(state) {
        offer_obj.clear_source();
    }
    client.delete_id(&WlDisplay::default(), state, dispatcher, offer.wayland_id().into());
}
