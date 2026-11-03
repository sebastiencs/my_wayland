//! `zwp_primary_selection_device_v1`.
//!
//! `set_selection` needs a serial that this client was given and that is not older
//! than the current primary selection. Keyboard focus is not required to set it.
//! Offers are delivered only to the client that has keyboard focus.

use crate::{
    clients::{Client, ClientDispatcher, EventWriter, RawRequest, Resolved},
    object::{Object, Parser, RequestResult},
    protocol_generated::{
        zwp_primary_selection_device_v1, WaylandId, ZwpPrimarySelectionDeviceV1Id,
        ZwpPrimarySelectionOfferV1Id, ZwpPrimarySelectionSourceV1Id,
    },
    state::{GrabbedObject, PrimarySelectionSourceState, State},
    wayland_objects::{
        wl_display::WlDisplay, zwp_primary_selection_offer_v1::destroy_primary_selection_offer,
    },
};

#[derive(Debug)]
pub struct ZwpPrimarySelectionDeviceV1 {
    id: ZwpPrimarySelectionDeviceV1Id,
    seat_id: crate::protocol_generated::WlSeatId,
    version: u32,
    selection_offer: Option<Resolved<ZwpPrimarySelectionOfferV1Id>>,
}

impl ZwpPrimarySelectionDeviceV1 {
    pub fn new(
        id: ZwpPrimarySelectionDeviceV1Id,
        seat_id: crate::protocol_generated::WlSeatId,
        version: u32,
    ) -> Self {
        Self { id, seat_id, version, selection_offer: None }
    }

    pub fn version(&self) -> u32 {
        self.version
    }

    pub fn seat_id(&self) -> crate::protocol_generated::WlSeatId {
        self.seat_id
    }

    pub fn selection_offer(&self) -> Option<Resolved<ZwpPrimarySelectionOfferV1Id>> {
        self.selection_offer
    }

    pub fn take_selection_offer(&mut self) -> Option<Resolved<ZwpPrimarySelectionOfferV1Id>> {
        self.selection_offer.take()
    }

    pub fn set_selection_offer(&mut self, offer: Option<Resolved<ZwpPrimarySelectionOfferV1Id>>) {
        self.selection_offer = offer;
    }

    pub fn send_data_offer(
        &self,
        offer: Resolved<ZwpPrimarySelectionOfferV1Id>,
        dispatcher: &mut ClientDispatcher,
    ) {
        self.send_event(
            zwp_primary_selection_device_v1::Event::DataOffer { offer: offer.wayland_id() },
            dispatcher,
        );
    }

    pub fn send_selection(
        &self,
        offer: Option<Resolved<ZwpPrimarySelectionOfferV1Id>>,
        dispatcher: &mut ClientDispatcher,
    ) {
        self.send_event(
            zwp_primary_selection_device_v1::Event::Selection {
                id: offer
                    .map(|offer| offer.wayland_id())
                    .unwrap_or_else(|| ZwpPrimarySelectionOfferV1Id::from(0u32)),
            },
            dispatcher,
        );
    }
}

impl Object for ZwpPrimarySelectionDeviceV1 {
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
        use zwp_primary_selection_device_v1::Request::*;

        match client.parse::<Self>(req, dispatcher)? {
            SetSelection { source, serial } => {
                set_selection(&mut self, client, state, dispatcher, source, serial)?;
            }
            Destroy => {
                let device_id = self.id;
                let offer = self.selection_offer.take();
                drop(self);
                if let Some(device) = client.resolve(device_id, state) {
                    state.seat.unregister_primary_selection_device(device);
                }
                if let Some(offer) = offer {
                    destroy_primary_selection_offer(client, state, dispatcher, offer);
                }
                client.delete_id(&WlDisplay::default(), state, dispatcher, device_id.into());
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
            state.seat.unregister_primary_selection_device(device);
        }
    }
}

fn set_selection(
    _device: &mut GrabbedObject<ZwpPrimarySelectionDeviceV1>,
    client: &mut Client,
    state: &mut State,
    dispatcher: &mut ClientDispatcher,
    source: ZwpPrimarySelectionSourceV1Id,
    serial: u32,
) -> RequestResult {
    if !state.seat.validate_primary_selection_serial(serial)
        || !state
            .seat
            .validate_client_serial(client.client_id(), serial)
    {
        return Ok(());
    }

    let next = match source.to_opt() {
        None => None,
        Some(source) => {
            let source = client.resolve_or_disconnect(source, state, dispatcher)?;
            Some(PrimarySelectionSourceState::Zwp(source))
        }
    };
    if let (Some(next), Some(current)) = (next, state.seat.primary_selection_source()) {
        if next.object_id() == current.object_id() {
            state.seat.set_primary_selection_serial(serial);
            return Ok(());
        }
    }

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
            clients::{
                Client, ClientAction, ClientId, PrimarySelectionSourceEvent, RawEvent, Resolved,
            },
            protocol_generated::{
                WaylandId, WlSeatId, WlSurfaceId, ZwpPrimarySelectionDeviceManagerV1Id,
                ZwpPrimarySelectionDeviceV1Id, ZwpPrimarySelectionSourceV1Id,
            },
            runner::Model,
            seat::{set_keyboard_focus_from_surface, SurfaceFocus},
            test_support::state_for_tests,
            wayland_objects::{
                wl_seat::WlSeat, wl_surface::WlSurface,
                zwp_primary_selection_device_manager_v1::ZwpPrimarySelectionDeviceManagerV1,
                zwp_primary_selection_source_v1::ZwpPrimarySelectionSourceV1,
            },
            Dispatcher,
        },
    };

    fn request(object: impl Into<WaylandId>, opcode: u16, values: &[u32]) -> RawRequest<'static> {
        let mut event = RawEvent::new(object.into(), opcode, values.len());
        for value in values {
            event.uint(*value);
        }
        (&*event.into_bytes()).try_into().unwrap()
    }

    fn offer_mime(source: ZwpPrimarySelectionSourceV1Id, mime: &str) -> RawRequest<'static> {
        let mut event = RawEvent::new(source.into(), 0, 1);
        event.string(mime.to_string());
        (&*event.into_bytes()).try_into().unwrap()
    }

    fn focus(client: &mut Client, state: &mut crate::state::State, surface: WlSurfaceId) {
        client.insert_object(state, WlSurface::new(surface, client.client_id(), 6));
        let mut dispatcher = Dispatcher::default();
        let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);
        set_keyboard_focus_from_surface(
            state,
            &mut client_dispatcher,
            Some(SurfaceFocus::new(Resolved::new(
                client.client_id(),
                surface,
                client.object_id(surface),
            ))),
        );
    }

    fn insert_source(client: &mut Client, state: &mut crate::state::State) {
        client.insert_object(
            state,
            ZwpPrimarySelectionSourceV1::new(ZwpPrimarySelectionSourceV1Id::from(60), 1),
        );
    }

    fn insert_device(client: &mut Client, state: &mut crate::state::State) {
        client.insert_object(
            state,
            ZwpPrimarySelectionDeviceV1::new(
                ZwpPrimarySelectionDeviceV1Id::from(30),
                WlSeatId::from(10),
                1,
            ),
        );
        state.seat.register_primary_selection_device(Resolved::new(
            client.client_id(),
            ZwpPrimarySelectionDeviceV1Id::from(30),
            client.object_id(ZwpPrimarySelectionDeviceV1Id::from(30)),
        ));
    }

    #[test]
    fn unfocused_client_with_a_valid_serial_sets_the_primary_selection() {
        let mut state = state_for_tests();
        let mut setter = Client::new_for_tests(ClientId::from_raw(1));
        let mut focused = Client::new_for_tests(ClientId::from_raw(2));
        insert_source(&mut setter, &mut state);
        insert_device(&mut setter, &mut state);
        insert_device(&mut focused, &mut state);
        focus(&mut focused, &mut state, WlSurfaceId::from(40));
        state.seat.record_client_serial(setter.client_id(), 10);
        let mut dispatcher = Dispatcher::default();
        {
            let mut client_dispatcher = setter.test_dispatcher(&mut dispatcher);
            setter
                .grab_object(ZwpPrimarySelectionSourceV1Id::from(60), &mut state)
                .on(
                    &mut setter,
                    &mut state,
                    offer_mime(ZwpPrimarySelectionSourceV1Id::from(60), "text/plain"),
                    &mut client_dispatcher,
                )
                .unwrap();
            setter
                .grab_object(ZwpPrimarySelectionDeviceV1Id::from(30), &mut state)
                .on(
                    &mut setter,
                    &mut state,
                    request(ZwpPrimarySelectionDeviceV1Id::from(30), 0, &[60, 10]),
                    &mut client_dispatcher,
                )
                .unwrap();
        }
        assert!(dispatcher.test_has_queued_action_for(
            focused.client_id(),
            |action: &ClientAction| matches!(action, ClientAction::RefreshPrimarySelection)
        ));
        assert!(!dispatcher.test_has_queued_action_for(
            setter.client_id(),
            |action: &ClientAction| matches!(action, ClientAction::RefreshPrimarySelection)
        ));
        focused
            .process(&mut state, ClientAction::RefreshPrimarySelection, &mut dispatcher)
            .unwrap();
        let device_id = u32::from(ZwpPrimarySelectionDeviceV1Id::from(30));
        assert!(capture::test_sent(&dispatcher, device_id, 0));
        assert!(capture::test_sent(&dispatcher, device_id, 1));
        let offer = focused
            .ref_object(ZwpPrimarySelectionDeviceV1Id::from(30), &state)
            .selection_offer()
            .unwrap();
        assert!(capture::test_sent(&dispatcher, u32::from(offer.wayland_id()), 0));
        assert!(setter
            .ref_object(ZwpPrimarySelectionDeviceV1Id::from(30), &state)
            .selection_offer()
            .is_none());
    }

    #[test]
    fn an_unissued_serial_is_ignored() {
        let mut state = state_for_tests();
        let mut client = Client::new_for_tests(ClientId::from_raw(1));
        insert_source(&mut client, &mut state);
        insert_device(&mut client, &mut state);
        let mut dispatcher = Dispatcher::default();
        let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);
        client
            .grab_object(ZwpPrimarySelectionDeviceV1Id::from(30), &mut state)
            .on(
                &mut client,
                &mut state,
                request(ZwpPrimarySelectionDeviceV1Id::from(30), 0, &[60, 10]),
                &mut client_dispatcher,
            )
            .unwrap();
        assert!(state.seat.primary_selection_source().is_none());
    }

    #[test]
    fn setting_the_same_source_again_does_not_cancel_it() {
        let mut state = state_for_tests();
        let mut client = Client::new_for_tests(ClientId::from_raw(1));
        insert_source(&mut client, &mut state);
        insert_device(&mut client, &mut state);
        state.seat.record_client_serial(client.client_id(), 10);
        state.seat.record_client_serial(client.client_id(), 11);
        let mut dispatcher = Dispatcher::default();
        let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);
        client
            .grab_object(ZwpPrimarySelectionDeviceV1Id::from(30), &mut state)
            .on(
                &mut client,
                &mut state,
                request(ZwpPrimarySelectionDeviceV1Id::from(30), 0, &[60, 10]),
                &mut client_dispatcher,
            )
            .unwrap();
        client
            .grab_object(ZwpPrimarySelectionDeviceV1Id::from(30), &mut state)
            .on(
                &mut client,
                &mut state,
                request(ZwpPrimarySelectionDeviceV1Id::from(30), 0, &[60, 11]),
                &mut client_dispatcher,
            )
            .unwrap();
        assert_eq!(state.seat.primary_selection_serial(), 11);
        assert!(!dispatcher.test_has_queued_action_for(
            client.client_id(),
            |action: &ClientAction| matches!(
                action,
                ClientAction::PrimarySelectionSourceEvent {
                    event: PrimarySelectionSourceEvent::Cancelled,
                    ..
                }
            )
        ));
    }

    #[test]
    fn replacing_the_source_cancels_the_previous_one() {
        let mut state = state_for_tests();
        let mut client = Client::new_for_tests(ClientId::from_raw(1));
        insert_source(&mut client, &mut state);
        client.insert_object(
            &mut state,
            ZwpPrimarySelectionSourceV1::new(ZwpPrimarySelectionSourceV1Id::from(61), 1),
        );
        insert_device(&mut client, &mut state);
        state.seat.record_client_serial(client.client_id(), 10);
        state.seat.record_client_serial(client.client_id(), 11);
        let mut dispatcher = Dispatcher::default();
        let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);
        client
            .grab_object(ZwpPrimarySelectionDeviceV1Id::from(30), &mut state)
            .on(
                &mut client,
                &mut state,
                request(ZwpPrimarySelectionDeviceV1Id::from(30), 0, &[60, 10]),
                &mut client_dispatcher,
            )
            .unwrap();
        client
            .grab_object(ZwpPrimarySelectionDeviceV1Id::from(30), &mut state)
            .on(
                &mut client,
                &mut state,
                request(ZwpPrimarySelectionDeviceV1Id::from(30), 0, &[61, 11]),
                &mut client_dispatcher,
            )
            .unwrap();
        assert!(dispatcher.test_has_queued_action_for(
            client.client_id(),
            |action: &ClientAction| matches!(
                action,
                ClientAction::PrimarySelectionSourceEvent {
                    event: PrimarySelectionSourceEvent::Cancelled,
                    ..
                }
            )
        ));
    }

    #[test]
    fn offer_after_set_selection_adds_the_mime_type() {
        let mut state = state_for_tests();
        let mut client = Client::new_for_tests(ClientId::from_raw(1));
        insert_source(&mut client, &mut state);
        insert_device(&mut client, &mut state);
        state.seat.record_client_serial(client.client_id(), 10);
        let mut dispatcher = Dispatcher::default();
        let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);
        let source = ZwpPrimarySelectionSourceV1Id::from(60);
        client
            .grab_object(source, &mut state)
            .on(&mut client, &mut state, offer_mime(source, "text/plain"), &mut client_dispatcher)
            .unwrap();
        client
            .grab_object(ZwpPrimarySelectionDeviceV1Id::from(30), &mut state)
            .on(
                &mut client,
                &mut state,
                request(ZwpPrimarySelectionDeviceV1Id::from(30), 0, &[60, 10]),
                &mut client_dispatcher,
            )
            .unwrap();
        client
            .grab_object(source, &mut state)
            .on(&mut client, &mut state, offer_mime(source, "text/html"), &mut client_dispatcher)
            .unwrap();
        assert_eq!(
            client.ref_object(source, &state).mime_types(),
            ["text/plain".to_string(), "text/html".to_string()]
        );
        assert!(!capture::test_closed(&dispatcher, &client));
    }

    #[test]
    fn focused_get_device_sends_a_null_selection() {
        let mut state = state_for_tests();
        let mut client = Client::new_for_tests(ClientId::from_raw(1));
        client.insert_object(&mut state, WlSeat::new(WlSeatId::from(10)));
        client.insert_object(
            &mut state,
            ZwpPrimarySelectionDeviceManagerV1::new(
                ZwpPrimarySelectionDeviceManagerV1Id::from(20),
                1,
            ),
        );
        focus(&mut client, &mut state, WlSurfaceId::from(40));
        let mut dispatcher = Dispatcher::default();
        let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);
        client
            .grab_object(ZwpPrimarySelectionDeviceManagerV1Id::from(20), &mut state)
            .on(
                &mut client,
                &mut state,
                request(ZwpPrimarySelectionDeviceManagerV1Id::from(20), 1, &[30, 10]),
                &mut client_dispatcher,
            )
            .unwrap();
        drop(client_dispatcher);
        client
            .process(&mut state, ClientAction::RefreshPrimarySelection, &mut dispatcher)
            .unwrap();
        let device_id = u32::from(ZwpPrimarySelectionDeviceV1Id::from(30));
        assert!(capture::test_sent(&dispatcher, device_id, 1));
        assert!(capture::test_sent_arg(&dispatcher, device_id, 1, 0));
    }

    #[test]
    fn losing_keyboard_focus_clears_the_offer() {
        let mut state = state_for_tests();
        let mut setter = Client::new_for_tests(ClientId::from_raw(1));
        let mut focused = Client::new_for_tests(ClientId::from_raw(2));
        let mut other = Client::new_for_tests(ClientId::from_raw(3));
        insert_source(&mut setter, &mut state);
        insert_device(&mut setter, &mut state);
        insert_device(&mut focused, &mut state);
        focus(&mut focused, &mut state, WlSurfaceId::from(40));
        state.seat.record_client_serial(setter.client_id(), 10);
        let mut dispatcher = Dispatcher::default();
        {
            let mut client_dispatcher = setter.test_dispatcher(&mut dispatcher);
            setter
                .grab_object(ZwpPrimarySelectionSourceV1Id::from(60), &mut state)
                .on(
                    &mut setter,
                    &mut state,
                    offer_mime(ZwpPrimarySelectionSourceV1Id::from(60), "text/plain"),
                    &mut client_dispatcher,
                )
                .unwrap();
            setter
                .grab_object(ZwpPrimarySelectionDeviceV1Id::from(30), &mut state)
                .on(
                    &mut setter,
                    &mut state,
                    request(ZwpPrimarySelectionDeviceV1Id::from(30), 0, &[60, 10]),
                    &mut client_dispatcher,
                )
                .unwrap();
        }
        focused
            .process(&mut state, ClientAction::RefreshPrimarySelection, &mut dispatcher)
            .unwrap();
        focus(&mut other, &mut state, WlSurfaceId::from(41));
        focused
            .process(&mut state, ClientAction::RefreshPrimarySelection, &mut dispatcher)
            .unwrap();
        assert!(focused
            .ref_object(ZwpPrimarySelectionDeviceV1Id::from(30), &state)
            .selection_offer()
            .is_none());
        assert!(capture::test_sent_arg(
            &dispatcher,
            u32::from(ZwpPrimarySelectionDeviceV1Id::from(30)),
            1,
            0
        ));
    }

    #[test]
    fn destroying_the_current_source_clears_the_selection() {
        let mut state = state_for_tests();
        let mut setter = Client::new_for_tests(ClientId::from_raw(1));
        let mut focused = Client::new_for_tests(ClientId::from_raw(2));
        insert_source(&mut setter, &mut state);
        insert_device(&mut setter, &mut state);
        focus(&mut focused, &mut state, WlSurfaceId::from(40));
        state.seat.record_client_serial(setter.client_id(), 10);
        let mut dispatcher = Dispatcher::default();
        {
            let mut client_dispatcher = setter.test_dispatcher(&mut dispatcher);
            setter
                .grab_object(ZwpPrimarySelectionDeviceV1Id::from(30), &mut state)
                .on(
                    &mut setter,
                    &mut state,
                    request(ZwpPrimarySelectionDeviceV1Id::from(30), 0, &[60, 10]),
                    &mut client_dispatcher,
                )
                .unwrap();
            setter
                .grab_object(ZwpPrimarySelectionSourceV1Id::from(60), &mut state)
                .on(
                    &mut setter,
                    &mut state,
                    request(ZwpPrimarySelectionSourceV1Id::from(60), 1, &[]),
                    &mut client_dispatcher,
                )
                .unwrap();
        }
        assert!(state.seat.primary_selection_source().is_none());
        assert!(dispatcher.test_has_queued_action_for(
            focused.client_id(),
            |action: &ClientAction| matches!(action, ClientAction::RefreshPrimarySelection)
        ));
        assert!(!dispatcher.test_has_queued_action_for(
            setter.client_id(),
            |action: &ClientAction| matches!(
                action,
                ClientAction::PrimarySelectionSourceEvent {
                    event: PrimarySelectionSourceEvent::Cancelled,
                    ..
                }
            )
        ));
    }
}
