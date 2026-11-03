//! `zwlr_data_control_offer_v1`.
//!
//! `receive` forwards to the source captured when the offer was created. A clipboard
//! offer stores a selection source. A primary offer stores a primary-selection source.

use std::os::fd::FromRawFd;

use crate::{
    clients::{Client, ClientDispatcher, EventWriter, RawRequest, Resolved},
    object::{Object, Parser, RequestResult},
    protocol_generated::{
        zwlr_data_control_offer_v1, WaylandId, ZwlrDataControlDeviceV1Id, ZwlrDataControlOfferV1Id,
    },
    state::{GrabbedObject, PrimarySelectionSourceState, SelectionSourceState, State},
    wayland_objects::wl_display::WlDisplay,
};

#[derive(Clone, Copy, Debug)]
enum OfferSource {
    Clipboard(SelectionSourceState),
    Primary(PrimarySelectionSourceState),
}

#[derive(Debug)]
pub struct ZwlrDataControlOfferV1 {
    id: ZwlrDataControlOfferV1Id,
    version: u32,
    source: Option<OfferSource>,
}

impl ZwlrDataControlOfferV1 {
    pub fn new(id: ZwlrDataControlOfferV1Id, version: u32, source: SelectionSourceState) -> Self {
        Self { id, version, source: Some(OfferSource::Clipboard(source)) }
    }

    pub fn from_primary(
        id: ZwlrDataControlOfferV1Id,
        version: u32,
        source: PrimarySelectionSourceState,
    ) -> Self {
        Self { id, version, source: Some(OfferSource::Primary(source)) }
    }

    pub fn version(&self) -> u32 {
        self.version
    }

    pub fn send_offer(&self, mime_type: String, dispatcher: &mut ClientDispatcher) {
        self.send_event(zwlr_data_control_offer_v1::Event::Offer { mime_type }, dispatcher);
    }

    fn clear_source(&mut self) {
        self.source = None;
    }
}

impl Object for ZwlrDataControlOfferV1 {
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
        use zwlr_data_control_offer_v1::Request::*;

        match client.parse::<Self>(req, dispatcher)? {
            Receive { mime_type, fd } => match self.source {
                Some(OfferSource::Clipboard(source)) => source.send(mime_type, fd, dispatcher),
                Some(OfferSource::Primary(source)) => source.send(mime_type, fd, dispatcher),
                None => {
                    let _ = unsafe { std::os::fd::OwnedFd::from_raw_fd(fd) };
                }
            },
            Destroy => {
                let offer_id = self.id;
                drop(self);
                if let Some(offer) = client.resolve(offer_id, state) {
                    destroy_data_control_offer(client, state, dispatcher, offer);
                }
            }
        }
        Ok(())
    }
}

pub(crate) fn create_data_control_offer(
    client: &mut Client,
    state: &mut State,
    dispatcher: &mut ClientDispatcher,
    device: Resolved<ZwlrDataControlDeviceV1Id>,
    selection: SelectionSourceState,
) -> Option<Resolved<ZwlrDataControlOfferV1Id>> {
    if !device.exists(state) {
        return None;
    }
    let mime_types = selection.mime_types(state)?;
    let version = device.rref(state).version();
    let offer_id = client.allocate_server_object_id::<ZwlrDataControlOfferV1Id>();
    let offer = ZwlrDataControlOfferV1::new(offer_id, version, selection);
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

pub(crate) fn create_data_control_primary_offer(
    client: &mut Client,
    state: &mut State,
    dispatcher: &mut ClientDispatcher,
    device: Resolved<ZwlrDataControlDeviceV1Id>,
    selection: PrimarySelectionSourceState,
) -> Option<Resolved<ZwlrDataControlOfferV1Id>> {
    if !device.exists(state) {
        return None;
    }
    let mime_types = selection.mime_types(state)?;
    let version = device.rref(state).version();
    let offer_id = client.allocate_server_object_id::<ZwlrDataControlOfferV1Id>();
    let offer = ZwlrDataControlOfferV1::from_primary(offer_id, version, selection);
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

pub(crate) fn destroy_data_control_offer(
    client: &mut Client,
    state: &mut State,
    dispatcher: &mut ClientDispatcher,
    offer: Resolved<ZwlrDataControlOfferV1Id>,
) {
    if offer.client_id() != client.client_id() || !offer.exists(state) {
        return;
    }
    if let Some(mut offer_obj) = offer.try_grab(state) {
        offer_obj.clear_source();
    }
    client.delete_id(&WlDisplay::default(), state, dispatcher, offer.wayland_id().into());
}
