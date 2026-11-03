//! `zwlr_data_control_device_v1`.
//!
//! Clipboard updates are delivered with or without keyboard focus. Version 2 also
//! delivers the primary selection that way. `set_selection` and `set_primary_selection`
//! do not check a client serial; each one advances the seat serial. A source that is
//! already used disconnects the client with `used_source`. `finished` is sent when the
//! client's `wl_seat` goes away.

use crate::{
    clients::{Client, ClientDispatcher, EventWriter, RawRequest, Resolved},
    object::{Object, Parser, RequestResult},
    protocol_generated::{
        wl_display, zwlr_data_control_device_v1, WaylandId, WlSeatId, ZwlrDataControlDeviceV1Id,
        ZwlrDataControlOfferV1Id, ZwlrDataControlSourceV1Id,
    },
    state::{GrabbedObject, PrimarySelectionSourceState, SelectionSourceState, State},
    wayland_objects::{
        wl_display::WlDisplay, zwlr_data_control_offer_v1::destroy_data_control_offer,
    },
};

#[derive(Debug)]
pub struct ZwlrDataControlDeviceV1 {
    id: ZwlrDataControlDeviceV1Id,
    seat_id: WlSeatId,
    version: u32,
    selection_offer: Option<Resolved<ZwlrDataControlOfferV1Id>>,
    primary_offer: Option<Resolved<ZwlrDataControlOfferV1Id>>,
    inert: bool,
}

impl ZwlrDataControlDeviceV1 {
    pub fn new(id: ZwlrDataControlDeviceV1Id, seat_id: WlSeatId, version: u32) -> Self {
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

    pub fn selection_offer(&self) -> Option<Resolved<ZwlrDataControlOfferV1Id>> {
        self.selection_offer
    }

    pub fn take_selection_offer(&mut self) -> Option<Resolved<ZwlrDataControlOfferV1Id>> {
        self.selection_offer.take()
    }

    pub fn set_selection_offer(&mut self, offer: Option<Resolved<ZwlrDataControlOfferV1Id>>) {
        self.selection_offer = offer;
    }

    pub fn primary_offer(&self) -> Option<Resolved<ZwlrDataControlOfferV1Id>> {
        self.primary_offer
    }

    pub fn take_primary_offer(&mut self) -> Option<Resolved<ZwlrDataControlOfferV1Id>> {
        self.primary_offer.take()
    }

    pub fn set_primary_offer(&mut self, offer: Option<Resolved<ZwlrDataControlOfferV1Id>>) {
        self.primary_offer = offer;
    }

    pub fn send_data_offer(
        &self,
        offer: Resolved<ZwlrDataControlOfferV1Id>,
        dispatcher: &mut ClientDispatcher,
    ) {
        self.send_event(
            zwlr_data_control_device_v1::Event::DataOffer { id: offer.wayland_id() },
            dispatcher,
        );
    }

    pub fn send_selection(
        &self,
        offer: Option<Resolved<ZwlrDataControlOfferV1Id>>,
        dispatcher: &mut ClientDispatcher,
    ) {
        self.send_event(
            zwlr_data_control_device_v1::Event::Selection {
                id: offer
                    .map(|offer| offer.wayland_id())
                    .unwrap_or_else(|| ZwlrDataControlOfferV1Id::from(0u32)),
            },
            dispatcher,
        );
    }

    pub fn send_primary_selection(
        &self,
        offer: Option<Resolved<ZwlrDataControlOfferV1Id>>,
        dispatcher: &mut ClientDispatcher,
    ) {
        self.send_event(
            zwlr_data_control_device_v1::Event::PrimarySelection {
                id: offer
                    .map(|offer| offer.wayland_id())
                    .unwrap_or_else(|| ZwlrDataControlOfferV1Id::from(0u32)),
            },
            dispatcher,
        );
    }

    pub fn finish(&mut self, dispatcher: &mut ClientDispatcher) {
        if self.inert {
            return;
        }
        self.inert = true;
        self.send_event(zwlr_data_control_device_v1::Event::Finished, dispatcher);
    }
}

impl Object for ZwlrDataControlDeviceV1 {
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
        use zwlr_data_control_device_v1::Request::*;

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
                    state.seat.unregister_data_control_device(device);
                }
                if let Some(offer) = offer {
                    destroy_data_control_offer(client, state, dispatcher, offer);
                }
                if let Some(offer) = primary_offer {
                    destroy_data_control_offer(client, state, dispatcher, offer);
                }
                client.delete_id(&WlDisplay::default(), state, dispatcher, device_id.into());
            }
            SetPrimarySelection { source } => {
                if self.version < zwlr_data_control_device_v1::SET_PRIMARY_SELECTION_SINCE {
                    client.protocol_violation_disconnect(
                        dispatcher,
                        self.id.into(),
                        wl_display::Error::InvalidMethod as u32,
                        "zwlr_data_control_device_v1.set_primary_selection requires version 2",
                    );
                    return Ok(());
                }
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
            state.seat.unregister_data_control_device(device);
        }
    }
}

fn set_selection(
    device: &mut GrabbedObject<ZwlrDataControlDeviceV1>,
    client: &mut Client,
    state: &mut State,
    dispatcher: &mut ClientDispatcher,
    source: ZwlrDataControlSourceV1Id,
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
                zwlr_data_control_device_v1::Error::UsedSource as u32,
                "zwlr_data_control_source_v1 used more than once",
            );
            return Ok(());
        }
        Some(SelectionSourceState::DataControl(source))
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
    device: &mut GrabbedObject<ZwlrDataControlDeviceV1>,
    client: &mut Client,
    state: &mut State,
    dispatcher: &mut ClientDispatcher,
    source: ZwlrDataControlSourceV1Id,
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
                zwlr_data_control_device_v1::Error::UsedSource as u32,
                "zwlr_data_control_source_v1 used more than once",
            );
            return Ok(());
        }
        Some(PrimarySelectionSourceState::DataControl(source))
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
    use std::{
        collections::VecDeque,
        os::fd::{FromRawFd, OwnedFd},
    };

    use {
        super::*,
        crate::{
            capture,
            clients::{
                Client, ClientAction, ClientId, DataControlSourceEvent, DataSourceClientEvent,
                PrimarySelectionSourceEvent, RawEvent, Resolved,
            },
            globals::Global,
            protocol_generated::{
                WaylandId, WlDataDeviceId, WlDataSourceId, WlSeatId, WlSurfaceId,
                ZwlrDataControlDeviceV1Id, ZwlrDataControlManagerV1Id, ZwlrDataControlSourceV1Id,
                ZwpPrimarySelectionDeviceV1Id, ZwpPrimarySelectionSourceV1Id,
            },
            runner::Model,
            seat::{set_keyboard_focus_from_surface, SurfaceFocus},
            test_support::state_for_tests,
            wayland_objects::{
                wl_data_device::WlDataDevice,
                wl_data_source::WlDataSource,
                wl_seat::WlSeat,
                wl_surface::WlSurface,
                zwlr_data_control_manager_v1::{
                    ZwlrDataControlManagerV1, ZwlrDataControlManagerV1Global, ADVERTISED_VERSION,
                },
                zwlr_data_control_source_v1::ZwlrDataControlSourceV1,
                zwp_primary_selection_device_v1::ZwpPrimarySelectionDeviceV1,
                zwp_primary_selection_source_v1::ZwpPrimarySelectionSourceV1,
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

    fn receive(
        offer: impl Into<WaylandId>,
        opcode: u16,
        mime: &str,
        fd: i32,
    ) -> RawRequest<'static> {
        let mut event = RawEvent::new(offer.into(), opcode, 1);
        event.string(mime.to_string());
        let mut request: RawRequest<'static> = (&*event.into_bytes()).try_into().unwrap();
        request.fds =
            Some(Box::leak(Box::new(VecDeque::from([unsafe { OwnedFd::from_raw_fd(fd) }]))));
        request
    }

    fn dev_null() -> i32 {
        unsafe { libc::open(c"/dev/null".as_ptr(), libc::O_WRONLY) }
    }

    fn seat_and_manager(client: &mut Client, state: &mut crate::state::State) {
        client.insert_object(state, WlSeat::new(WlSeatId::from(10)));
        client.insert_object(
            state,
            ZwlrDataControlManagerV1::new(ZwlrDataControlManagerV1Id::from(20), 1),
        );
    }

    #[test]
    fn advertised_manager_version_is_two() {
        assert_eq!(ZwlrDataControlManagerV1Global.interface().1, ADVERTISED_VERSION);
        assert_eq!(ADVERTISED_VERSION, 2);
    }

    #[test]
    fn new_device_sends_null_selection_without_keyboard_focus() {
        let mut state = state_for_tests();
        let mut client = Client::new_for_tests(ClientId::from_raw(1));
        seat_and_manager(&mut client, &mut state);
        let mut dispatcher = Dispatcher::default();
        let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);
        client
            .grab_object(ZwlrDataControlManagerV1Id::from(20), &mut state)
            .on(
                &mut client,
                &mut state,
                ids(ZwlrDataControlManagerV1Id::from(20), 1, &[30, 10]),
                &mut client_dispatcher,
            )
            .unwrap();
        drop(client_dispatcher);
        assert!(dispatcher.test_has_queued_action_for(
            client.client_id(),
            |action: &ClientAction| matches!(action, ClientAction::RefreshDataControlSelection)
        ));
        client
            .process(&mut state, ClientAction::RefreshDataControlSelection, &mut dispatcher)
            .unwrap();
        let device_id = u32::from(ZwlrDataControlDeviceV1Id::from(30));
        assert!(capture::test_sent(&dispatcher, device_id, 1));
        assert!(capture::test_sent_arg(&dispatcher, device_id, 1, 0));
        client
            .process(&mut state, ClientAction::RefreshDataControlPrimarySelection, &mut dispatcher)
            .unwrap();
        assert!(!capture::test_sent(&dispatcher, device_id, 3));
    }

    #[test]
    fn unfocused_set_selection_reaches_data_control_and_focused_client() {
        let mut state = state_for_tests();
        let mut clipboard = Client::new_for_tests(ClientId::from_raw(1));
        let mut focused = Client::new_for_tests(ClientId::from_raw(2));
        seat_and_manager(&mut clipboard, &mut state);
        focused.insert_object(
            &mut state,
            WlSurface::new(WlSurfaceId::from(40), focused.client_id(), 6),
        );
        focused.insert_object(
            &mut state,
            WlDataDevice::new(WlDataDeviceId::from(50), WlSeatId::from(10), 3),
        );
        state.seat.register_data_device(Resolved::new(
            focused.client_id(),
            WlDataDeviceId::from(50),
            focused.object_id(WlDataDeviceId::from(50)),
        ));
        {
            let mut setup = Dispatcher::default();
            let mut setup_dispatcher = focused.test_dispatcher(&mut setup);
            set_keyboard_focus_from_surface(
                &mut state,
                &mut setup_dispatcher,
                Some(SurfaceFocus::new(Resolved::new(
                    focused.client_id(),
                    WlSurfaceId::from(40),
                    focused.object_id(WlSurfaceId::from(40)),
                ))),
            );
        }
        clipboard.insert_object(
            &mut state,
            ZwlrDataControlSourceV1::new(ZwlrDataControlSourceV1Id::from(60), 1),
        );
        let mut dispatcher = Dispatcher::default();
        {
            let mut client_dispatcher = clipboard.test_dispatcher(&mut dispatcher);
            clipboard
                .grab_object(ZwlrDataControlSourceV1Id::from(60), &mut state)
                .on(
                    &mut clipboard,
                    &mut state,
                    offer_mime(ZwlrDataControlSourceV1Id::from(60), "text/plain"),
                    &mut client_dispatcher,
                )
                .unwrap();
            clipboard
                .grab_object(ZwlrDataControlManagerV1Id::from(20), &mut state)
                .on(
                    &mut clipboard,
                    &mut state,
                    ids(ZwlrDataControlManagerV1Id::from(20), 1, &[30, 10]),
                    &mut client_dispatcher,
                )
                .unwrap();
            clipboard
                .grab_object(ZwlrDataControlDeviceV1Id::from(30), &mut state)
                .on(
                    &mut clipboard,
                    &mut state,
                    ids(ZwlrDataControlDeviceV1Id::from(30), 0, &[60]),
                    &mut client_dispatcher,
                )
                .unwrap();
        }
        assert!(dispatcher.test_has_queued_action_for(
            focused.client_id(),
            |action: &ClientAction| matches!(action, ClientAction::RefreshDataDeviceSelection)
        ));
        clipboard
            .process(&mut state, ClientAction::RefreshDataControlSelection, &mut dispatcher)
            .unwrap();
        focused
            .process(&mut state, ClientAction::RefreshDataDeviceSelection, &mut dispatcher)
            .unwrap();
        let device = clipboard.ref_object(ZwlrDataControlDeviceV1Id::from(30), &state);
        let offer = device.selection_offer().unwrap();
        let device_id = u32::from(ZwlrDataControlDeviceV1Id::from(30));
        assert!(capture::test_sent(&dispatcher, device_id, 0));
        assert!(capture::test_sent(&dispatcher, device_id, 1));
        assert!(capture::test_sent(&dispatcher, u32::from(offer.wayland_id()), 0));
        let wl_offer = focused
            .ref_object(WlDataDeviceId::from(50), &state)
            .selection_offer()
            .unwrap();
        let mut focused_dispatcher = focused.test_dispatcher(&mut dispatcher);
        focused
            .grab_object(wl_offer.wayland_id(), &mut state)
            .on(
                &mut focused,
                &mut state,
                receive(wl_offer.wayland_id(), 1, "text/plain", dev_null()),
                &mut focused_dispatcher,
            )
            .unwrap();
        assert!(dispatcher.test_has_queued_action_for(
            clipboard.client_id(),
            |action: &ClientAction| matches!(
                action,
                ClientAction::DataControlSourceEvent {
                    event: DataControlSourceEvent::Send { mime_type, .. },
                    ..
                } if mime_type == "text/plain"
            )
        ));
    }

    #[test]
    fn second_set_selection_disconnects() {
        let mut state = state_for_tests();
        let mut client = Client::new_for_tests(ClientId::from_raw(1));
        seat_and_manager(&mut client, &mut state);
        client.insert_object(
            &mut state,
            ZwlrDataControlSourceV1::new(ZwlrDataControlSourceV1Id::from(60), 1),
        );
        client.insert_object(
            &mut state,
            ZwlrDataControlDeviceV1::new(
                ZwlrDataControlDeviceV1Id::from(30),
                WlSeatId::from(10),
                1,
            ),
        );
        state.seat.register_data_control_device(Resolved::new(
            client.client_id(),
            ZwlrDataControlDeviceV1Id::from(30),
            client.object_id(ZwlrDataControlDeviceV1Id::from(30)),
        ));
        let mut dispatcher = Dispatcher::default();
        let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);
        client
            .grab_object(ZwlrDataControlDeviceV1Id::from(30), &mut state)
            .on(
                &mut client,
                &mut state,
                ids(ZwlrDataControlDeviceV1Id::from(30), 0, &[60]),
                &mut client_dispatcher,
            )
            .unwrap();
        client
            .grab_object(ZwlrDataControlDeviceV1Id::from(30), &mut state)
            .on(
                &mut client,
                &mut state,
                ids(ZwlrDataControlDeviceV1Id::from(30), 0, &[60]),
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
            ZwlrDataControlSourceV1::new(ZwlrDataControlSourceV1Id::from(60), 1),
        );
        client.insert_object(
            &mut state,
            ZwlrDataControlDeviceV1::new(
                ZwlrDataControlDeviceV1Id::from(30),
                WlSeatId::from(10),
                1,
            ),
        );
        let mut dispatcher = Dispatcher::default();
        let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);
        client
            .grab_object(ZwlrDataControlDeviceV1Id::from(30), &mut state)
            .on(
                &mut client,
                &mut state,
                ids(ZwlrDataControlDeviceV1Id::from(30), 0, &[60]),
                &mut client_dispatcher,
            )
            .unwrap();
        client
            .grab_object(ZwlrDataControlSourceV1Id::from(60), &mut state)
            .on(
                &mut client,
                &mut state,
                offer_mime(ZwlrDataControlSourceV1Id::from(60), "text/plain"),
                &mut client_dispatcher,
            )
            .unwrap();
        assert!(capture::test_closed(&dispatcher, &client));
    }

    #[test]
    fn set_primary_selection_is_rejected_at_version_one() {
        let mut state = state_for_tests();
        let mut client = Client::new_for_tests(ClientId::from_raw(1));
        seat_and_manager(&mut client, &mut state);
        client.insert_object(
            &mut state,
            ZwlrDataControlDeviceV1::new(
                ZwlrDataControlDeviceV1Id::from(30),
                WlSeatId::from(10),
                1,
            ),
        );
        let mut dispatcher = Dispatcher::default();
        let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);
        client
            .grab_object(ZwlrDataControlDeviceV1Id::from(30), &mut state)
            .on(
                &mut client,
                &mut state,
                ids(ZwlrDataControlDeviceV1Id::from(30), 2, &[0]),
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
        let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);
        client
            .grab_object(ZwlrDataControlManagerV1Id::from(20), &mut state)
            .on(
                &mut client,
                &mut state,
                ids(ZwlrDataControlManagerV1Id::from(20), 1, &[30, 10]),
                &mut client_dispatcher,
            )
            .unwrap();
        client
            .grab_object(WlSeatId::from(10), &mut state)
            .on(&mut client, &mut state, ids(WlSeatId::from(10), 3, &[]), &mut client_dispatcher)
            .unwrap();
        assert!(capture::test_sent(&dispatcher, u32::from(ZwlrDataControlDeviceV1Id::from(30)), 2));
        assert!(client
            .ref_object(ZwlrDataControlDeviceV1Id::from(30), &state)
            .is_inert());
    }

    #[test]
    fn version_two_bind_sends_a_null_primary_selection() {
        let mut state = state_for_tests();
        let mut client = Client::new_for_tests(ClientId::from_raw(1));
        client.insert_object(&mut state, WlSeat::new(WlSeatId::from(10)));
        client.insert_object(
            &mut state,
            ZwlrDataControlManagerV1::new(ZwlrDataControlManagerV1Id::from(20), 2),
        );
        let mut dispatcher = Dispatcher::default();
        {
            let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);
            client
                .grab_object(ZwlrDataControlManagerV1Id::from(20), &mut state)
                .on(
                    &mut client,
                    &mut state,
                    ids(ZwlrDataControlManagerV1Id::from(20), 1, &[30, 10]),
                    &mut client_dispatcher,
                )
                .unwrap();
        }
        client
            .process(&mut state, ClientAction::RefreshDataControlSelection, &mut dispatcher)
            .unwrap();
        client
            .process(&mut state, ClientAction::RefreshDataControlPrimarySelection, &mut dispatcher)
            .unwrap();
        let device_id = u32::from(ZwlrDataControlDeviceV1Id::from(30));
        assert!(capture::test_sent_arg(&dispatcher, device_id, 1, 0));
        assert!(capture::test_sent_arg(&dispatcher, device_id, 3, 0));
    }

    #[test]
    fn set_primary_selection_reaches_version_two_devices_and_the_focused_client() {
        let mut state = state_for_tests();
        let mut clipboard = Client::new_for_tests(ClientId::from_raw(1));
        let mut focused = Client::new_for_tests(ClientId::from_raw(2));
        clipboard.insert_object(&mut state, WlSeat::new(WlSeatId::from(10)));
        clipboard.insert_object(
            &mut state,
            ZwlrDataControlManagerV1::new(ZwlrDataControlManagerV1Id::from(20), 2),
        );
        clipboard.insert_object(
            &mut state,
            ZwlrDataControlSourceV1::new(ZwlrDataControlSourceV1Id::from(60), 1),
        );
        {
            let mut setup = Dispatcher::default();
            let mut client_dispatcher = clipboard.test_dispatcher(&mut setup);
            clipboard
                .grab_object(ZwlrDataControlSourceV1Id::from(60), &mut state)
                .on(
                    &mut clipboard,
                    &mut state,
                    offer_mime(ZwlrDataControlSourceV1Id::from(60), "text/plain"),
                    &mut client_dispatcher,
                )
                .unwrap();
            clipboard
                .grab_object(ZwlrDataControlManagerV1Id::from(20), &mut state)
                .on(
                    &mut clipboard,
                    &mut state,
                    ids(ZwlrDataControlManagerV1Id::from(20), 1, &[30, 10]),
                    &mut client_dispatcher,
                )
                .unwrap();
        }
        clipboard
            .process(
                &mut state,
                ClientAction::RefreshDataControlSelection,
                &mut Dispatcher::default(),
            )
            .unwrap();
        clipboard
            .process(
                &mut state,
                ClientAction::RefreshDataControlPrimarySelection,
                &mut Dispatcher::default(),
            )
            .unwrap();
        focused.insert_object(
            &mut state,
            WlSurface::new(WlSurfaceId::from(40), focused.client_id(), 6),
        );
        focused.insert_object(
            &mut state,
            ZwpPrimarySelectionDeviceV1::new(
                ZwpPrimarySelectionDeviceV1Id::from(30),
                WlSeatId::from(10),
                1,
            ),
        );
        state.seat.register_primary_selection_device(Resolved::new(
            focused.client_id(),
            ZwpPrimarySelectionDeviceV1Id::from(30),
            focused.object_id(ZwpPrimarySelectionDeviceV1Id::from(30)),
        ));
        {
            let mut setup = Dispatcher::default();
            let mut client_dispatcher = focused.test_dispatcher(&mut setup);
            set_keyboard_focus_from_surface(
                &mut state,
                &mut client_dispatcher,
                Some(SurfaceFocus::new(Resolved::new(
                    focused.client_id(),
                    WlSurfaceId::from(40),
                    focused.object_id(WlSurfaceId::from(40)),
                ))),
            );
        }
        let mut dispatcher = Dispatcher::default();
        {
            let mut client_dispatcher = clipboard.test_dispatcher(&mut dispatcher);
            clipboard
                .grab_object(ZwlrDataControlDeviceV1Id::from(30), &mut state)
                .on(
                    &mut clipboard,
                    &mut state,
                    ids(ZwlrDataControlDeviceV1Id::from(30), 2, &[60]),
                    &mut client_dispatcher,
                )
                .unwrap();
        }
        assert_ne!(state.seat.primary_selection_serial(), 0);
        clipboard
            .process(&mut state, ClientAction::RefreshDataControlPrimarySelection, &mut dispatcher)
            .unwrap();
        focused
            .process(&mut state, ClientAction::RefreshPrimarySelection, &mut dispatcher)
            .unwrap();
        let offer = clipboard
            .ref_object(ZwlrDataControlDeviceV1Id::from(30), &state)
            .primary_offer()
            .unwrap();
        let device_id = u32::from(ZwlrDataControlDeviceV1Id::from(30));
        assert!(capture::test_sent_arg(&dispatcher, device_id, 3, u32::from(offer.wayland_id())));
        let zwp_offer = focused
            .ref_object(ZwpPrimarySelectionDeviceV1Id::from(30), &state)
            .selection_offer()
            .unwrap();
        assert!(capture::test_sent_arg(
            &dispatcher,
            u32::from(ZwpPrimarySelectionDeviceV1Id::from(30)),
            1,
            u32::from(zwp_offer.wayland_id())
        ));
    }

    #[test]
    fn a_clipboard_source_cannot_also_set_the_primary_selection() {
        let mut state = state_for_tests();
        let mut client = Client::new_for_tests(ClientId::from_raw(1));
        seat_and_manager(&mut client, &mut state);
        client.insert_object(
            &mut state,
            ZwlrDataControlSourceV1::new(ZwlrDataControlSourceV1Id::from(60), 1),
        );
        client.insert_object(
            &mut state,
            ZwlrDataControlDeviceV1::new(
                ZwlrDataControlDeviceV1Id::from(30),
                WlSeatId::from(10),
                2,
            ),
        );
        state.seat.register_data_control_device(Resolved::new(
            client.client_id(),
            ZwlrDataControlDeviceV1Id::from(30),
            client.object_id(ZwlrDataControlDeviceV1Id::from(30)),
        ));
        let mut dispatcher = Dispatcher::default();
        let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);
        client
            .grab_object(ZwlrDataControlDeviceV1Id::from(30), &mut state)
            .on(
                &mut client,
                &mut state,
                ids(ZwlrDataControlDeviceV1Id::from(30), 0, &[60]),
                &mut client_dispatcher,
            )
            .unwrap();
        client
            .grab_object(ZwlrDataControlDeviceV1Id::from(30), &mut state)
            .on(
                &mut client,
                &mut state,
                ids(ZwlrDataControlDeviceV1Id::from(30), 2, &[60]),
                &mut client_dispatcher,
            )
            .unwrap();
        assert!(capture::test_closed(&dispatcher, &client));
    }

    #[test]
    fn an_older_primary_serial_cannot_replace_the_data_control_selection() {
        let mut state = state_for_tests();
        let mut clipboard = Client::new_for_tests(ClientId::from_raw(1));
        let mut app = Client::new_for_tests(ClientId::from_raw(2));
        clipboard.insert_object(&mut state, WlSeat::new(WlSeatId::from(10)));
        clipboard.insert_object(
            &mut state,
            ZwlrDataControlDeviceV1::new(
                ZwlrDataControlDeviceV1Id::from(30),
                WlSeatId::from(10),
                2,
            ),
        );
        state.seat.register_data_control_device(Resolved::new(
            clipboard.client_id(),
            ZwlrDataControlDeviceV1Id::from(30),
            clipboard.object_id(ZwlrDataControlDeviceV1Id::from(30)),
        ));
        clipboard.insert_object(
            &mut state,
            ZwlrDataControlSourceV1::new(ZwlrDataControlSourceV1Id::from(60), 1),
        );
        app.insert_object(
            &mut state,
            ZwpPrimarySelectionDeviceV1::new(
                ZwpPrimarySelectionDeviceV1Id::from(30),
                WlSeatId::from(10),
                1,
            ),
        );
        app.insert_object(
            &mut state,
            ZwpPrimarySelectionSourceV1::new(ZwpPrimarySelectionSourceV1Id::from(70), 1),
        );
        let mut dispatcher = Dispatcher::default();
        {
            let mut client_dispatcher = clipboard.test_dispatcher(&mut dispatcher);
            clipboard
                .grab_object(ZwlrDataControlDeviceV1Id::from(30), &mut state)
                .on(
                    &mut clipboard,
                    &mut state,
                    ids(ZwlrDataControlDeviceV1Id::from(30), 2, &[60]),
                    &mut client_dispatcher,
                )
                .unwrap();
        }
        let serial = state.seat.primary_selection_serial();
        state.seat.record_client_serial(app.client_id(), serial);
        {
            let mut client_dispatcher = app.test_dispatcher(&mut dispatcher);
            app.grab_object(ZwpPrimarySelectionDeviceV1Id::from(30), &mut state)
                .on(
                    &mut app,
                    &mut state,
                    ids(ZwpPrimarySelectionDeviceV1Id::from(30), 0, &[70, serial]),
                    &mut client_dispatcher,
                )
                .unwrap();
        }
        assert!(matches!(
            state.seat.primary_selection_source(),
            Some(crate::state::PrimarySelectionSourceState::DataControl(_))
        ));
        let newer = serial.wrapping_add(1);
        state.seat.record_client_serial(app.client_id(), newer);
        {
            let mut client_dispatcher = app.test_dispatcher(&mut dispatcher);
            app.grab_object(ZwpPrimarySelectionDeviceV1Id::from(30), &mut state)
                .on(
                    &mut app,
                    &mut state,
                    ids(ZwpPrimarySelectionDeviceV1Id::from(30), 0, &[70, newer]),
                    &mut client_dispatcher,
                )
                .unwrap();
        }
        assert!(matches!(
            state.seat.primary_selection_source(),
            Some(crate::state::PrimarySelectionSourceState::Zwp(_))
        ));
    }

    #[test]
    fn clearing_the_primary_selection_advances_the_serial() {
        let mut state = state_for_tests();
        let mut client = Client::new_for_tests(ClientId::from_raw(1));
        client.insert_object(&mut state, WlSeat::new(WlSeatId::from(10)));
        client.insert_object(
            &mut state,
            ZwlrDataControlDeviceV1::new(
                ZwlrDataControlDeviceV1Id::from(30),
                WlSeatId::from(10),
                2,
            ),
        );
        client.insert_object(
            &mut state,
            ZwlrDataControlSourceV1::new(ZwlrDataControlSourceV1Id::from(60), 1),
        );
        let mut dispatcher = Dispatcher::default();
        let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);
        client
            .grab_object(ZwlrDataControlDeviceV1Id::from(30), &mut state)
            .on(
                &mut client,
                &mut state,
                ids(ZwlrDataControlDeviceV1Id::from(30), 2, &[60]),
                &mut client_dispatcher,
            )
            .unwrap();
        let first = state.seat.primary_selection_serial();
        client
            .grab_object(ZwlrDataControlDeviceV1Id::from(30), &mut state)
            .on(
                &mut client,
                &mut state,
                ids(ZwlrDataControlDeviceV1Id::from(30), 2, &[0]),
                &mut client_dispatcher,
            )
            .unwrap();
        assert_ne!(state.seat.primary_selection_serial(), first);
        assert!(state.seat.primary_selection_source().is_none());
    }

    fn register_data_control(client: &mut Client, state: &mut crate::state::State, version: u32) {
        client.insert_object(
            state,
            ZwlrDataControlDeviceV1::new(
                ZwlrDataControlDeviceV1Id::from(30),
                WlSeatId::from(10),
                version,
            ),
        );
        state.seat.register_data_control_device(Resolved::new(
            client.client_id(),
            ZwlrDataControlDeviceV1Id::from(30),
            client.object_id(ZwlrDataControlDeviceV1Id::from(30)),
        ));
    }

    fn focus_client(client: &mut Client, state: &mut crate::state::State) {
        client.insert_object(state, WlSurface::new(WlSurfaceId::from(40), client.client_id(), 6));
        let mut setup = Dispatcher::default();
        let mut client_dispatcher = client.test_dispatcher(&mut setup);
        set_keyboard_focus_from_surface(
            state,
            &mut client_dispatcher,
            Some(SurfaceFocus::new(Resolved::new(
                client.client_id(),
                WlSurfaceId::from(40),
                client.object_id(WlSurfaceId::from(40)),
            ))),
        );
    }

    #[test]
    fn focused_wl_selection_reaches_data_control_devices() {
        let mut state = state_for_tests();
        let mut app = Client::new_for_tests(ClientId::from_raw(1));
        let mut clipboard = Client::new_for_tests(ClientId::from_raw(2));
        let mut unfocused = Client::new_for_tests(ClientId::from_raw(3));
        app.insert_object(&mut state, WlSeat::new(WlSeatId::from(10)));
        app.insert_object(
            &mut state,
            WlDataDevice::new(WlDataDeviceId::from(50), WlSeatId::from(10), 3),
        );
        state.seat.register_data_device(Resolved::new(
            app.client_id(),
            WlDataDeviceId::from(50),
            app.object_id(WlDataDeviceId::from(50)),
        ));
        app.insert_object(
            &mut state,
            WlDataSource::new(WlDataSourceId::from(61), app.client_id(), 3),
        );
        focus_client(&mut app, &mut state);
        register_data_control(&mut clipboard, &mut state, 2);
        unfocused.insert_object(
            &mut state,
            WlDataDevice::new(WlDataDeviceId::from(50), WlSeatId::from(10), 3),
        );
        state.seat.register_data_device(Resolved::new(
            unfocused.client_id(),
            WlDataDeviceId::from(50),
            unfocused.object_id(WlDataDeviceId::from(50)),
        ));
        state.seat.record_client_serial(app.client_id(), 7);
        let mut dispatcher = Dispatcher::default();
        {
            let mut client_dispatcher = app.test_dispatcher(&mut dispatcher);
            app.grab_object(WlDataDeviceId::from(50), &mut state)
                .on(
                    &mut app,
                    &mut state,
                    ids(WlDataDeviceId::from(50), 1, &[61, 7]),
                    &mut client_dispatcher,
                )
                .unwrap();
        }
        assert!(dispatcher.test_has_queued_action_for(
            clipboard.client_id(),
            |action: &ClientAction| matches!(action, ClientAction::RefreshDataControlSelection)
        ));
        assert!(!dispatcher.test_has_queued_action_for(
            unfocused.client_id(),
            |action: &ClientAction| matches!(action, ClientAction::RefreshDataDeviceSelection)
        ));
        clipboard
            .process(&mut state, ClientAction::RefreshDataControlSelection, &mut dispatcher)
            .unwrap();
        assert!(clipboard
            .ref_object(ZwlrDataControlDeviceV1Id::from(30), &state)
            .selection_offer()
            .is_some());
    }

    #[test]
    fn data_control_selection_cancels_the_previous_wl_source() {
        let mut state = state_for_tests();
        let mut app = Client::new_for_tests(ClientId::from_raw(1));
        let mut clipboard = Client::new_for_tests(ClientId::from_raw(2));
        app.insert_object(&mut state, WlSeat::new(WlSeatId::from(10)));
        app.insert_object(
            &mut state,
            WlDataDevice::new(WlDataDeviceId::from(50), WlSeatId::from(10), 3),
        );
        app.insert_object(
            &mut state,
            WlDataSource::new(WlDataSourceId::from(61), app.client_id(), 3),
        );
        focus_client(&mut app, &mut state);
        state.seat.record_client_serial(app.client_id(), 7);
        register_data_control(&mut clipboard, &mut state, 2);
        clipboard.insert_object(
            &mut state,
            ZwlrDataControlSourceV1::new(ZwlrDataControlSourceV1Id::from(60), 1),
        );
        let mut dispatcher = Dispatcher::default();
        {
            let mut client_dispatcher = app.test_dispatcher(&mut dispatcher);
            app.grab_object(WlDataDeviceId::from(50), &mut state)
                .on(
                    &mut app,
                    &mut state,
                    ids(WlDataDeviceId::from(50), 1, &[61, 7]),
                    &mut client_dispatcher,
                )
                .unwrap();
            let mut client_dispatcher = clipboard.test_dispatcher(&mut dispatcher);
            clipboard
                .grab_object(ZwlrDataControlDeviceV1Id::from(30), &mut state)
                .on(
                    &mut clipboard,
                    &mut state,
                    ids(ZwlrDataControlDeviceV1Id::from(30), 0, &[60]),
                    &mut client_dispatcher,
                )
                .unwrap();
        }
        assert!(dispatcher.test_has_queued_action_for(
            app.client_id(),
            |action: &ClientAction| matches!(
                action,
                ClientAction::DataSourceEvent { event: DataSourceClientEvent::Cancelled, .. }
            )
        ));
        assert!(matches!(
            state.seat.selection_source(),
            Some(crate::state::SelectionSourceState::DataControl(_))
        ));
    }

    #[test]
    fn wl_selection_cancels_the_previous_data_control_source() {
        let mut state = state_for_tests();
        let mut clipboard = Client::new_for_tests(ClientId::from_raw(1));
        let mut app = Client::new_for_tests(ClientId::from_raw(2));
        clipboard.insert_object(&mut state, WlSeat::new(WlSeatId::from(10)));
        register_data_control(&mut clipboard, &mut state, 2);
        clipboard.insert_object(
            &mut state,
            ZwlrDataControlSourceV1::new(ZwlrDataControlSourceV1Id::from(60), 1),
        );
        app.insert_object(
            &mut state,
            WlDataDevice::new(WlDataDeviceId::from(50), WlSeatId::from(10), 3),
        );
        app.insert_object(
            &mut state,
            WlDataSource::new(WlDataSourceId::from(61), app.client_id(), 3),
        );
        focus_client(&mut app, &mut state);
        let mut dispatcher = Dispatcher::default();
        {
            let mut client_dispatcher = clipboard.test_dispatcher(&mut dispatcher);
            clipboard
                .grab_object(ZwlrDataControlDeviceV1Id::from(30), &mut state)
                .on(
                    &mut clipboard,
                    &mut state,
                    ids(ZwlrDataControlDeviceV1Id::from(30), 0, &[60]),
                    &mut client_dispatcher,
                )
                .unwrap();
        }
        let serial = state.seat.selection_serial().wrapping_add(1);
        state.seat.record_client_serial(app.client_id(), serial);
        {
            let mut client_dispatcher = app.test_dispatcher(&mut dispatcher);
            app.grab_object(WlDataDeviceId::from(50), &mut state)
                .on(
                    &mut app,
                    &mut state,
                    ids(WlDataDeviceId::from(50), 1, &[61, serial]),
                    &mut client_dispatcher,
                )
                .unwrap();
        }
        assert!(dispatcher.test_has_queued_action_for(
            clipboard.client_id(),
            |action: &ClientAction| matches!(
                action,
                ClientAction::DataControlSourceEvent {
                    event: DataControlSourceEvent::Cancelled,
                    ..
                }
            )
        ));
        assert!(matches!(
            state.seat.selection_source(),
            Some(crate::state::SelectionSourceState::WlData(_))
        ));
    }

    #[test]
    fn zwp_primary_selection_reaches_version_two_devices_only() {
        let mut state = state_for_tests();
        let mut app = Client::new_for_tests(ClientId::from_raw(1));
        let mut version_two = Client::new_for_tests(ClientId::from_raw(2));
        let mut version_one = Client::new_for_tests(ClientId::from_raw(3));
        app.insert_object(&mut state, WlSeat::new(WlSeatId::from(10)));
        app.insert_object(
            &mut state,
            ZwpPrimarySelectionDeviceV1::new(
                ZwpPrimarySelectionDeviceV1Id::from(30),
                WlSeatId::from(10),
                1,
            ),
        );
        app.insert_object(
            &mut state,
            ZwpPrimarySelectionSourceV1::new(ZwpPrimarySelectionSourceV1Id::from(70), 1),
        );
        register_data_control(&mut version_two, &mut state, 2);
        register_data_control(&mut version_one, &mut state, 1);
        state.seat.record_client_serial(app.client_id(), 10);
        let mut dispatcher = Dispatcher::default();
        {
            let mut client_dispatcher = app.test_dispatcher(&mut dispatcher);
            app.grab_object(ZwpPrimarySelectionDeviceV1Id::from(30), &mut state)
                .on(
                    &mut app,
                    &mut state,
                    ids(ZwpPrimarySelectionDeviceV1Id::from(30), 0, &[70, 10]),
                    &mut client_dispatcher,
                )
                .unwrap();
        }
        version_one
            .process(&mut state, ClientAction::RefreshDataControlPrimarySelection, &mut dispatcher)
            .unwrap();
        assert!(!capture::test_sent(
            &dispatcher,
            u32::from(ZwlrDataControlDeviceV1Id::from(30)),
            3
        ));
        version_two
            .process(&mut state, ClientAction::RefreshDataControlPrimarySelection, &mut dispatcher)
            .unwrap();
        let offer = version_two
            .ref_object(ZwlrDataControlDeviceV1Id::from(30), &state)
            .primary_offer()
            .unwrap();
        assert!(capture::test_sent_arg(
            &dispatcher,
            u32::from(ZwlrDataControlDeviceV1Id::from(30)),
            3,
            u32::from(offer.wayland_id())
        ));
    }

    #[test]
    fn data_control_primary_selection_cancels_the_previous_zwp_source() {
        let mut state = state_for_tests();
        let mut app = Client::new_for_tests(ClientId::from_raw(1));
        let mut clipboard = Client::new_for_tests(ClientId::from_raw(2));
        app.insert_object(&mut state, WlSeat::new(WlSeatId::from(10)));
        app.insert_object(
            &mut state,
            ZwpPrimarySelectionDeviceV1::new(
                ZwpPrimarySelectionDeviceV1Id::from(30),
                WlSeatId::from(10),
                1,
            ),
        );
        app.insert_object(
            &mut state,
            ZwpPrimarySelectionSourceV1::new(ZwpPrimarySelectionSourceV1Id::from(70), 1),
        );
        register_data_control(&mut clipboard, &mut state, 2);
        clipboard.insert_object(
            &mut state,
            ZwlrDataControlSourceV1::new(ZwlrDataControlSourceV1Id::from(60), 1),
        );
        state.seat.record_client_serial(app.client_id(), 10);
        let mut dispatcher = Dispatcher::default();
        {
            let mut client_dispatcher = app.test_dispatcher(&mut dispatcher);
            app.grab_object(ZwpPrimarySelectionDeviceV1Id::from(30), &mut state)
                .on(
                    &mut app,
                    &mut state,
                    ids(ZwpPrimarySelectionDeviceV1Id::from(30), 0, &[70, 10]),
                    &mut client_dispatcher,
                )
                .unwrap();
            let mut client_dispatcher = clipboard.test_dispatcher(&mut dispatcher);
            clipboard
                .grab_object(ZwlrDataControlDeviceV1Id::from(30), &mut state)
                .on(
                    &mut clipboard,
                    &mut state,
                    ids(ZwlrDataControlDeviceV1Id::from(30), 2, &[60]),
                    &mut client_dispatcher,
                )
                .unwrap();
        }
        assert!(dispatcher.test_has_queued_action_for(
            app.client_id(),
            |action: &ClientAction| matches!(
                action,
                ClientAction::PrimarySelectionSourceEvent {
                    event: PrimarySelectionSourceEvent::Cancelled,
                    ..
                }
            )
        ));
        assert!(matches!(
            state.seat.primary_selection_source(),
            Some(crate::state::PrimarySelectionSourceState::DataControl(_))
        ));
    }

    #[test]
    fn data_control_source_disconnect_clears_the_clipboard_and_skips_the_owner() {
        let mut state = state_for_tests();
        let mut owner = Client::new_for_tests(ClientId::from_raw(1));
        let mut watcher = Client::new_for_tests(ClientId::from_raw(2));
        let mut focused = Client::new_for_tests(ClientId::from_raw(3));
        owner.insert_object(&mut state, WlSeat::new(WlSeatId::from(10)));
        register_data_control(&mut owner, &mut state, 2);
        owner.insert_object(
            &mut state,
            ZwlrDataControlSourceV1::new(ZwlrDataControlSourceV1Id::from(60), 1),
        );
        register_data_control(&mut watcher, &mut state, 2);
        focused.insert_object(
            &mut state,
            WlDataDevice::new(WlDataDeviceId::from(50), WlSeatId::from(10), 3),
        );
        state.seat.register_data_device(Resolved::new(
            focused.client_id(),
            WlDataDeviceId::from(50),
            focused.object_id(WlDataDeviceId::from(50)),
        ));
        focus_client(&mut focused, &mut state);
        {
            let mut setup = Dispatcher::default();
            let mut client_dispatcher = owner.test_dispatcher(&mut setup);
            owner
                .grab_object(ZwlrDataControlDeviceV1Id::from(30), &mut state)
                .on(
                    &mut owner,
                    &mut state,
                    ids(ZwlrDataControlDeviceV1Id::from(30), 0, &[60]),
                    &mut client_dispatcher,
                )
                .unwrap();
        }
        let mut dispatcher = Dispatcher::default();
        {
            let mut client_dispatcher = owner.test_dispatcher(&mut dispatcher);
            owner
                .grab_object(ZwlrDataControlSourceV1Id::from(60), &mut state)
                .on_client_disconnect(&mut owner, &mut state, &mut client_dispatcher);
        }
        assert!(state.seat.selection_source().is_none());
        assert!(dispatcher.test_has_queued_action_for(
            watcher.client_id(),
            |action: &ClientAction| matches!(action, ClientAction::RefreshDataControlSelection)
        ));
        assert!(dispatcher.test_has_queued_action_for(
            focused.client_id(),
            |action: &ClientAction| matches!(action, ClientAction::RefreshDataDeviceSelection)
        ));
        assert!(!dispatcher.test_has_queued_action_for(
            owner.client_id(),
            |action: &ClientAction| matches!(action, ClientAction::RefreshDataControlSelection)
        ));
    }

    #[test]
    fn primary_source_disconnect_clears_the_primary_selection_and_skips_the_owner() {
        let mut state = state_for_tests();
        let mut owner = Client::new_for_tests(ClientId::from_raw(1));
        let mut watcher = Client::new_for_tests(ClientId::from_raw(2));
        let mut focused = Client::new_for_tests(ClientId::from_raw(3));
        owner.insert_object(&mut state, WlSeat::new(WlSeatId::from(10)));
        owner.insert_object(
            &mut state,
            ZwpPrimarySelectionDeviceV1::new(
                ZwpPrimarySelectionDeviceV1Id::from(30),
                WlSeatId::from(10),
                1,
            ),
        );
        owner.insert_object(
            &mut state,
            ZwpPrimarySelectionSourceV1::new(ZwpPrimarySelectionSourceV1Id::from(70), 1),
        );
        register_data_control(&mut watcher, &mut state, 2);
        focused.insert_object(
            &mut state,
            ZwpPrimarySelectionDeviceV1::new(
                ZwpPrimarySelectionDeviceV1Id::from(30),
                WlSeatId::from(10),
                1,
            ),
        );
        state.seat.register_primary_selection_device(Resolved::new(
            focused.client_id(),
            ZwpPrimarySelectionDeviceV1Id::from(30),
            focused.object_id(ZwpPrimarySelectionDeviceV1Id::from(30)),
        ));
        focus_client(&mut focused, &mut state);
        state.seat.record_client_serial(owner.client_id(), 10);
        {
            let mut setup = Dispatcher::default();
            let mut client_dispatcher = owner.test_dispatcher(&mut setup);
            owner
                .grab_object(ZwpPrimarySelectionDeviceV1Id::from(30), &mut state)
                .on(
                    &mut owner,
                    &mut state,
                    ids(ZwpPrimarySelectionDeviceV1Id::from(30), 0, &[70, 10]),
                    &mut client_dispatcher,
                )
                .unwrap();
        }
        let mut dispatcher = Dispatcher::default();
        {
            let mut client_dispatcher = owner.test_dispatcher(&mut dispatcher);
            owner
                .grab_object(ZwpPrimarySelectionSourceV1Id::from(70), &mut state)
                .on_client_disconnect(&mut owner, &mut state, &mut client_dispatcher);
        }
        assert!(state.seat.primary_selection_source().is_none());
        assert!(dispatcher.test_has_queued_action_for(
            watcher.client_id(),
            |action: &ClientAction| matches!(
                action,
                ClientAction::RefreshDataControlPrimarySelection
            )
        ));
        assert!(dispatcher.test_has_queued_action_for(
            focused.client_id(),
            |action: &ClientAction| matches!(action, ClientAction::RefreshPrimarySelection)
        ));
        assert!(!dispatcher.test_has_queued_action_for(
            owner.client_id(),
            |action: &ClientAction| {
                matches!(
                    action,
                    ClientAction::RefreshPrimarySelection
                        | ClientAction::RefreshDataControlPrimarySelection
                )
            }
        ));
    }
}
