//! `zwlr_data_control_offer_v1`.
//!
//! `receive` forwards to the selection source captured when the offer was created.
//! That source is either a `wl_data_source` or a `zwlr_data_control_source_v1`.

use std::os::fd::FromRawFd;

use crate::{
    clients::{Client, ClientDispatcher, EventWriter, RawRequest, Resolved},
    object::{Object, Parser, RequestResult},
    protocol_generated::{
        zwlr_data_control_offer_v1, WaylandId, ZwlrDataControlDeviceV1Id, ZwlrDataControlOfferV1Id,
    },
    state::{GrabbedObject, SelectionSourceState, State},
    wayland_objects::wl_display::WlDisplay,
};

#[derive(Debug)]
pub struct ZwlrDataControlOfferV1 {
    id: ZwlrDataControlOfferV1Id,
    version: u32,
    source: Option<SelectionSourceState>,
}

impl ZwlrDataControlOfferV1 {
    pub fn new(id: ZwlrDataControlOfferV1Id, version: u32, source: SelectionSourceState) -> Self {
        Self { id, version, source: Some(source) }
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
            Receive { mime_type, fd } => {
                if let Some(source) = self.source {
                    source.send(mime_type, fd, dispatcher);
                } else {
                    let _ = unsafe { std::os::fd::OwnedFd::from_raw_fd(fd) };
                }
            }
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
