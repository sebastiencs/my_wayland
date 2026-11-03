//! `ext_data_control_device_v1`.
//!
//! Clipboard and primary-selection updates are delivered with or without keyboard
//! focus. `set_selection` and `set_primary_selection` do not check a client serial;
//! each one advances the seat serial. A source that is already used disconnects the
//! client with `used_source`. `finished` is sent when the client's `wl_seat` goes
//! away.

use crate::{
    clients::{Client, ClientDispatcher, EventWriter, RawRequest, Resolved},
    object::{Object, Parser, RequestResult},
    protocol_generated::{
        ext_data_control_device_v1, ExtDataControlDeviceV1Id, ExtDataControlOfferV1Id,
        ExtDataControlSourceV1Id, WaylandId, WlSeatId,
    },
    state::{GrabbedObject, PrimarySelectionSourceState, SelectionSourceState, State},
    wayland_objects::{
        ext_data_control_offer_v1::inert_ext_data_control_offer, wl_display::WlDisplay,
    },
};

#[derive(Debug)]
pub struct ExtDataControlDeviceV1 {
    id: ExtDataControlDeviceV1Id,
    seat_id: WlSeatId,
    version: u32,
    selection_offer: Option<Resolved<ExtDataControlOfferV1Id>>,
    primary_offer: Option<Resolved<ExtDataControlOfferV1Id>>,
    inert: bool,
}

impl ExtDataControlDeviceV1 {
    pub fn new(id: ExtDataControlDeviceV1Id, seat_id: WlSeatId, version: u32) -> Self {
        Self {
            id,
            seat_id,
            version,
            selection_offer: None,
            primary_offer: None,
            inert: false,
        }
    }

    pub fn version(&self) -> u32 {
        self.version
    }

    pub fn seat_id(&self) -> WlSeatId {
        self.seat_id
    }

    pub fn is_inert(&self) -> bool {
        self.inert
    }

    pub fn selection_offer(&self) -> Option<Resolved<ExtDataControlOfferV1Id>> {
        self.selection_offer
    }

    pub fn take_selection_offer(&mut self) -> Option<Resolved<ExtDataControlOfferV1Id>> {
        self.selection_offer.take()
    }

    pub fn set_selection_offer(&mut self, offer: Option<Resolved<ExtDataControlOfferV1Id>>) {
        self.selection_offer = offer;
    }

    pub fn primary_offer(&self) -> Option<Resolved<ExtDataControlOfferV1Id>> {
        self.primary_offer
    }

    pub fn take_primary_offer(&mut self) -> Option<Resolved<ExtDataControlOfferV1Id>> {
        self.primary_offer.take()
    }

    pub fn set_primary_offer(&mut self, offer: Option<Resolved<ExtDataControlOfferV1Id>>) {
        self.primary_offer = offer;
    }

    pub fn send_data_offer(
        &self,
        offer: Resolved<ExtDataControlOfferV1Id>,
        dispatcher: &mut ClientDispatcher,
    ) {
        self.send_event(
            ext_data_control_device_v1::Event::DataOffer { id: offer.wayland_id() },
            dispatcher,
        );
    }

    pub fn send_selection(
        &self,
        offer: Option<Resolved<ExtDataControlOfferV1Id>>,
        dispatcher: &mut ClientDispatcher,
    ) {
        self.send_event(
            ext_data_control_device_v1::Event::Selection {
                id: offer
                    .map(|offer| offer.wayland_id())
                    .unwrap_or_else(|| ExtDataControlOfferV1Id::from(0u32)),
            },
            dispatcher,
        );
    }

    pub fn send_primary_selection(
        &self,
        offer: Option<Resolved<ExtDataControlOfferV1Id>>,
        dispatcher: &mut ClientDispatcher,
    ) {
        self.send_event(
            ext_data_control_device_v1::Event::PrimarySelection {
                id: offer
                    .map(|offer| offer.wayland_id())
                    .unwrap_or_else(|| ExtDataControlOfferV1Id::from(0u32)),
            },
            dispatcher,
        );
    }

    pub fn finish(&mut self, dispatcher: &mut ClientDispatcher) {
        if self.inert {
            return;
        }
        self.inert = true;
        self.send_event(ext_data_control_device_v1::Event::Finished, dispatcher);
    }
}

impl Object for ExtDataControlDeviceV1 {
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
        use ext_data_control_device_v1::Request::*;

        match client.parse::<Self>(req, dispatcher)? {
            SetSelection { source } => {
                if self.inert {
                    return Ok(());
                }
                set_selection(&mut self, client, state, dispatcher, source)?;
            }
            Destroy => {
                let device_id = self.id;
                let offer = self.selection_offer.take();
                let primary_offer = self.primary_offer.take();
                drop(self);
                if let Some(device) = client.resolve(device_id, state) {
                    state.seat.unregister_ext_data_control_device(device);
                }
                if let Some(offer) = offer {
                    inert_ext_data_control_offer(offer, state);
                }
                if let Some(offer) = primary_offer {
                    inert_ext_data_control_offer(offer, state);
                }
                client.delete_id(&WlDisplay::default(), state, dispatcher, device_id.into());
            }
            SetPrimarySelection { source } => {
                if self.inert {
                    return Ok(());
                }
                set_primary_selection(&mut self, client, state, dispatcher, source)?;
            }
        }
        Ok(())
    }

    fn on_client_disconnect(
        &mut self,
        client: &mut Client,
        state: &mut State,
        _dispatcher: &mut ClientDispatcher,
    ) {
        if let Some(device) = client.resolve(self.id, state) {
            state.seat.unregister_ext_data_control_device(device);
        }
    }
}

fn set_selection(
    device: &mut GrabbedObject<ExtDataControlDeviceV1>,
    client: &mut Client,
    state: &mut State,
    dispatcher: &mut ClientDispatcher,
    source: ExtDataControlSourceV1Id,
) -> RequestResult {
    let next = if u32::from(source) == 0 {
        None
    } else {
        let source = client.resolve_or_disconnect(source, state, dispatcher)?;
        let Some(mut source_obj) = source.try_ref_mut(state) else {
            return Ok(());
        };
        if !source_obj.try_use() {
            client.protocol_violation_disconnect(
                dispatcher,
                device.id.into(),
                ext_data_control_device_v1::Error::UsedSource as u32,
                "ext_data_control_source_v1 used more than once",
            );
            return Ok(());
        }
        Some(SelectionSourceState::Ext(source))
    };

    let serial = state.seat.next_serial();
    let previous = state.seat.replace_selection_source(next, serial);
    if let Some(previous) = previous {
        previous.cancelled(dispatcher);
    }
    state.seat.publish_clipboard_selection(dispatcher, None);
    Ok(())
}

fn set_primary_selection(
    device: &mut GrabbedObject<ExtDataControlDeviceV1>,
    client: &mut Client,
    state: &mut State,
    dispatcher: &mut ClientDispatcher,
    source: ExtDataControlSourceV1Id,
) -> RequestResult {
    let next = if u32::from(source) == 0 {
        None
    } else {
        let source = client.resolve_or_disconnect(source, state, dispatcher)?;
        let Some(mut source_obj) = source.try_ref_mut(state) else {
            return Ok(());
        };
        if !source_obj.try_use() {
            client.protocol_violation_disconnect(
                dispatcher,
                device.id.into(),
                ext_data_control_device_v1::Error::UsedSource as u32,
                "ext_data_control_source_v1 used more than once",
            );
            return Ok(());
        }
        Some(PrimarySelectionSourceState::Ext(source))
    };

    let serial = state.seat.next_serial();
    let previous = state.seat.replace_primary_selection_source(next, serial);
    if let Some(previous) = previous {
        previous.cancelled(dispatcher);
    }
    state.seat.publish_primary_selection(dispatcher, None);
    Ok(())
}

#[cfg(test)]
mod tests {
    use {
        super::*,
        crate::{
            capture,
            clients::{Client, ClientAction, ClientId, RawEvent, Resolved},
            globals::Global,
            protocol_generated::{
                Constants, ExtDataControlManagerV1Id, ExtDataControlSourceV1Id, WaylandId,
            },
            runner::Model,
            test_support::state_for_tests,
            wayland_objects::{
                ext_data_control_manager_v1::{
                    ExtDataControlManagerV1, ExtDataControlManagerV1Global,
                },
                ext_data_control_source_v1::ExtDataControlSourceV1,
                wl_seat::WlSeat,
            },
            Dispatcher,
        },
    };

    fn ids(object: impl Into<WaylandId>, opcode: u16, values: &[u32]) -> RawRequest<'static> {
        let mut event = RawEvent::new(object.into(), opcode, values.len());
        for value in values {
            event.uint(*value);
        }
        (&*event.into_bytes()).try_into().unwrap()
    }

    fn offer_mime(source: impl Into<WaylandId>, mime: &str) -> RawRequest<'static> {
        let mut event = RawEvent::new(source.into(), 0, 1);
        event.string(mime.to_string());
        (&*event.into_bytes()).try_into().unwrap()
    }

    fn seat_and_manager(client: &mut Client, state: &mut crate::state::State) {
        client.insert_object(state, WlSeat::new(WlSeatId::from(10)));
        client.insert_object(
            state,
            ExtDataControlManagerV1::new(ExtDataControlManagerV1Id::from(20), 1),
        );
    }

    #[test]
    fn advertised_manager_version_is_one() {
        assert_eq!(
            ExtDataControlManagerV1Global.interface().1,
            ExtDataControlManagerV1::INTERFACE_VERSION
        );
    }

    #[test]
    fn new_device_sends_null_selection_and_primary_selection() {
        let mut state = state_for_tests();
        let mut client = Client::new_for_tests(ClientId::from_raw(1));
        seat_and_manager(&mut client, &mut state);
        let mut dispatcher = Dispatcher::default();
        {
            let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);
            client
                .grab_object(ExtDataControlManagerV1Id::from(20), &mut state)
                .on(
                    &mut client,
                    &mut state,
                    ids(ExtDataControlManagerV1Id::from(20), 1, &[30, 10]),
                    &mut client_dispatcher,
                )
                .unwrap();
        }
        client
            .process(&mut state, ClientAction::RefreshExtDataControlSelection, &mut dispatcher)
            .unwrap();
        client
            .process(
                &mut state,
                ClientAction::RefreshExtDataControlPrimarySelection,
                &mut dispatcher,
            )
            .unwrap();
        let device_id = u32::from(ExtDataControlDeviceV1Id::from(30));
        assert!(capture::test_sent_arg(&dispatcher, device_id, 1, 0));
        assert!(capture::test_sent_arg(&dispatcher, device_id, 3, 0));
    }

    #[test]
    fn unfocused_set_selection_reaches_the_ext_device() {
        let mut state = state_for_tests();
        let mut client = Client::new_for_tests(ClientId::from_raw(1));
        seat_and_manager(&mut client, &mut state);
        client.insert_object(
            &mut state,
            ExtDataControlSourceV1::new(ExtDataControlSourceV1Id::from(60), 1),
        );
        let mut dispatcher = Dispatcher::default();
        {
            let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);
            client
                .grab_object(ExtDataControlSourceV1Id::from(60), &mut state)
                .on(
                    &mut client,
                    &mut state,
                    offer_mime(ExtDataControlSourceV1Id::from(60), "text/plain"),
                    &mut client_dispatcher,
                )
                .unwrap();
            client
                .grab_object(ExtDataControlManagerV1Id::from(20), &mut state)
                .on(
                    &mut client,
                    &mut state,
                    ids(ExtDataControlManagerV1Id::from(20), 1, &[30, 10]),
                    &mut client_dispatcher,
                )
                .unwrap();
            client
                .grab_object(ExtDataControlDeviceV1Id::from(30), &mut state)
                .on(
                    &mut client,
                    &mut state,
                    ids(ExtDataControlDeviceV1Id::from(30), 0, &[60]),
                    &mut client_dispatcher,
                )
                .unwrap();
        }
        client
            .process(&mut state, ClientAction::RefreshExtDataControlSelection, &mut dispatcher)
            .unwrap();
        let offer = client
            .ref_object(ExtDataControlDeviceV1Id::from(30), &state)
            .selection_offer()
            .unwrap();
        let device_id = u32::from(ExtDataControlDeviceV1Id::from(30));
        assert!(capture::test_sent(&dispatcher, device_id, 0));
        assert!(capture::test_sent_arg(&dispatcher, device_id, 1, u32::from(offer.wayland_id())));
        assert!(capture::test_sent(&dispatcher, u32::from(offer.wayland_id()), 0));
    }

    #[test]
    fn set_primary_selection_reaches_the_ext_device() {
        let mut state = state_for_tests();
        let mut client = Client::new_for_tests(ClientId::from_raw(1));
        seat_and_manager(&mut client, &mut state);
        client.insert_object(
            &mut state,
            ExtDataControlSourceV1::new(ExtDataControlSourceV1Id::from(60), 1),
        );
        let mut dispatcher = Dispatcher::default();
        {
            let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);
            client
                .grab_object(ExtDataControlSourceV1Id::from(60), &mut state)
                .on(
                    &mut client,
                    &mut state,
                    offer_mime(ExtDataControlSourceV1Id::from(60), "text/plain"),
                    &mut client_dispatcher,
                )
                .unwrap();
            client
                .grab_object(ExtDataControlManagerV1Id::from(20), &mut state)
                .on(
                    &mut client,
                    &mut state,
                    ids(ExtDataControlManagerV1Id::from(20), 1, &[30, 10]),
                    &mut client_dispatcher,
                )
                .unwrap();
            client
                .grab_object(ExtDataControlDeviceV1Id::from(30), &mut state)
                .on(
                    &mut client,
                    &mut state,
                    ids(ExtDataControlDeviceV1Id::from(30), 2, &[60]),
                    &mut client_dispatcher,
                )
                .unwrap();
        }
        client
            .process(
                &mut state,
                ClientAction::RefreshExtDataControlPrimarySelection,
                &mut dispatcher,
            )
            .unwrap();
        let offer = client
            .ref_object(ExtDataControlDeviceV1Id::from(30), &state)
            .primary_offer()
            .unwrap();
        assert!(capture::test_sent_arg(
            &dispatcher,
            u32::from(ExtDataControlDeviceV1Id::from(30)),
            3,
            u32::from(offer.wayland_id())
        ));
    }

    #[test]
    fn second_set_selection_disconnects() {
        let mut state = state_for_tests();
        let mut client = Client::new_for_tests(ClientId::from_raw(1));
        seat_and_manager(&mut client, &mut state);
        client.insert_object(
            &mut state,
            ExtDataControlSourceV1::new(ExtDataControlSourceV1Id::from(60), 1),
        );
        client.insert_object(
            &mut state,
            ExtDataControlDeviceV1::new(ExtDataControlDeviceV1Id::from(30), WlSeatId::from(10), 1),
        );
        let mut dispatcher = Dispatcher::default();
        let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);
        client
            .grab_object(ExtDataControlDeviceV1Id::from(30), &mut state)
            .on(
                &mut client,
                &mut state,
                ids(ExtDataControlDeviceV1Id::from(30), 0, &[60]),
                &mut client_dispatcher,
            )
            .unwrap();
        client
            .grab_object(ExtDataControlDeviceV1Id::from(30), &mut state)
            .on(
                &mut client,
                &mut state,
                ids(ExtDataControlDeviceV1Id::from(30), 0, &[60]),
                &mut client_dispatcher,
            )
            .unwrap();
        assert!(capture::test_closed(&dispatcher, &client));
    }

    #[test]
    fn offer_after_set_selection_disconnects() {
        let mut state = state_for_tests();
        let mut client = Client::new_for_tests(ClientId::from_raw(1));
        seat_and_manager(&mut client, &mut state);
        client.insert_object(
            &mut state,
            ExtDataControlSourceV1::new(ExtDataControlSourceV1Id::from(60), 1),
        );
        client.insert_object(
            &mut state,
            ExtDataControlDeviceV1::new(ExtDataControlDeviceV1Id::from(30), WlSeatId::from(10), 1),
        );
        let mut dispatcher = Dispatcher::default();
        let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);
        client
            .grab_object(ExtDataControlDeviceV1Id::from(30), &mut state)
            .on(
                &mut client,
                &mut state,
                ids(ExtDataControlDeviceV1Id::from(30), 0, &[60]),
                &mut client_dispatcher,
            )
            .unwrap();
        client
            .grab_object(ExtDataControlSourceV1Id::from(60), &mut state)
            .on(
                &mut client,
                &mut state,
                offer_mime(ExtDataControlSourceV1Id::from(60), "text/plain"),
                &mut client_dispatcher,
            )
            .unwrap();
        assert!(capture::test_closed(&dispatcher, &client));
    }

    #[test]
    fn a_primary_source_cannot_also_set_the_selection() {
        let mut state = state_for_tests();
        let mut client = Client::new_for_tests(ClientId::from_raw(1));
        seat_and_manager(&mut client, &mut state);
        client.insert_object(
            &mut state,
            ExtDataControlSourceV1::new(ExtDataControlSourceV1Id::from(60), 1),
        );
        client.insert_object(
            &mut state,
            ExtDataControlDeviceV1::new(ExtDataControlDeviceV1Id::from(30), WlSeatId::from(10), 1),
        );
        let mut dispatcher = Dispatcher::default();
        let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);
        client
            .grab_object(ExtDataControlDeviceV1Id::from(30), &mut state)
            .on(
                &mut client,
                &mut state,
                ids(ExtDataControlDeviceV1Id::from(30), 2, &[60]),
                &mut client_dispatcher,
            )
            .unwrap();
        client
            .grab_object(ExtDataControlDeviceV1Id::from(30), &mut state)
            .on(
                &mut client,
                &mut state,
                ids(ExtDataControlDeviceV1Id::from(30), 0, &[60]),
                &mut client_dispatcher,
            )
            .unwrap();
        assert!(capture::test_closed(&dispatcher, &client));
    }

    #[test]
    fn releasing_the_seat_finishes_the_device() {
        let mut state = state_for_tests();
        let mut client = Client::new_for_tests(ClientId::from_raw(1));
        seat_and_manager(&mut client, &mut state);
        let mut dispatcher = Dispatcher::default();
        {
            let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);
            client
                .grab_object(ExtDataControlManagerV1Id::from(20), &mut state)
                .on(
                    &mut client,
                    &mut state,
                    ids(ExtDataControlManagerV1Id::from(20), 1, &[30, 10]),
                    &mut client_dispatcher,
                )
                .unwrap();
            client
                .grab_object(WlSeatId::from(10), &mut state)
                .on(
                    &mut client,
                    &mut state,
                    ids(WlSeatId::from(10), 3, &[]),
                    &mut client_dispatcher,
                )
                .unwrap();
        }
        assert!(capture::test_sent(&dispatcher, u32::from(ExtDataControlDeviceV1Id::from(30)), 2));
        assert!(client
            .ref_object(ExtDataControlDeviceV1Id::from(30), &state)
            .is_inert());
    }

    #[test]
    fn an_inert_device_ignores_set_requests() {
        let mut state = state_for_tests();
        let mut client = Client::new_for_tests(ClientId::from_raw(1));
        seat_and_manager(&mut client, &mut state);
        client.insert_object(
            &mut state,
            ExtDataControlSourceV1::new(ExtDataControlSourceV1Id::from(60), 1),
        );
        client.insert_object(
            &mut state,
            ExtDataControlDeviceV1::new(ExtDataControlDeviceV1Id::from(30), WlSeatId::from(10), 1),
        );
        let mut dispatcher = Dispatcher::default();
        {
            let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);
            let mut device = client.grab_object(ExtDataControlDeviceV1Id::from(30), &mut state);
            device.finish(&mut client_dispatcher);
        }
        {
            let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);
            client
                .grab_object(ExtDataControlDeviceV1Id::from(30), &mut state)
                .on(
                    &mut client,
                    &mut state,
                    ids(ExtDataControlDeviceV1Id::from(30), 0, &[60]),
                    &mut client_dispatcher,
                )
                .unwrap();
        }
        assert!(state.seat.selection_source().is_none());
    }
}
