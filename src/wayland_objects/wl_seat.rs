use crate::{
    clients::{Client, ClientDispatcher, EventWriter, RawRequest},
    globals::Global,
    object::{Object, Parser, RequestResult},
    protocol_generated::{
        wl_keyboard, wl_seat, Constants as ProtocolConstants, NewId, WaylandId, WlPointerId,
        WlSeatId, WlTouchId,
    },
    state::{GrabbedObject, State},
    wayland_objects::{
        wl_display::WlDisplay, wl_keyboard::WlKeyboard, wl_pointer::WlPointer, wl_touch::WlTouch,
    },
};

#[derive(Debug)]
pub struct WlSeatGlobal;

impl WlSeatGlobal {
    pub fn new() -> Box<dyn Global> {
        Box::new(Self {})
    }
}

impl Global for WlSeatGlobal {
    fn interface(&self) -> (&'static str, u32) {
        (WlSeat::INTERFACE_NAME, WlSeat::INTERFACE_VERSION)
    }
    fn bind(
        &mut self,
        client: &mut Client,
        state: &mut State,
        id: NewId,
        version: u32,
        dispatcher: &mut ClientDispatcher,
    ) {
        let id = WlSeatId::from(id);
        let wl_seat = WlSeat { id, version };

        wl_seat.send_capabilities(dispatcher);
        // `name` exists since wl_seat version 2; v1 clients (e.g. weston simple-egl) only
        // register the capabilities listener and libwayland aborts if we send opcode 1.
        if version >= wl_seat::NAME_SINCE {
            wl_seat.send_name(dispatcher);
        }

        client.insert_object(state, wl_seat);
    }
}

#[derive(Debug)]
pub struct WlSeat {
    id: WlSeatId,
    /// Version the client bound on `wl_seat`. Child objects (`wl_keyboard`, etc.) inherit it.
    version: u32,
}

impl WlSeat {
    pub fn new(id: WlSeatId) -> Self {
        Self { id, version: <Self as ProtocolConstants>::INTERFACE_VERSION }
    }
}

impl Object for WlSeat {
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
        use wl_seat::Request::*;

        match client.parse::<Self>(req, dispatcher)? {
            GetPointer { id } => {
                let id = WlPointerId::from(id);
                let wl_pointer = WlPointer::new(id, self.version);
                let (pointer, _) = client.insert_object(state, wl_pointer);
                state.seat.register_pointer(pointer);
            }
            GetKeyboard { id } => {
                let wl_keyboard = WlKeyboard::new(id);
                let keymap = state
                    .seat
                    .client_keymap(client.client_id())
                    .unwrap_or_else(|| state.xkb.keymap_blob());
                wl_keyboard.send_keymap_blob(&keymap, dispatcher);
                state
                    .seat
                    .remember_client_keymap(client.client_id(), keymap);
                // `repeat_info` exists since wl_keyboard version 4; v1–v3 clients (e.g. weston
                // simple-egl with wl_seat v1) have no listener slot and libwayland aborts.
                if self.version >= wl_keyboard::REPEAT_INFO_SINCE {
                    wl_keyboard.send_repeat_info(dispatcher);
                }

                let (keyboard, _) = client.insert_object(state, wl_keyboard);
                state.seat.register_keyboard(keyboard);
            }
            GetTouch { id } => {
                // Touch is not advertised. The object stays inert so a client
                // that asks anyway still has a valid id to release.
                let version = self
                    .version
                    .min(<WlTouch as ProtocolConstants>::INTERFACE_VERSION);
                client.insert_object(state, WlTouch::new(id, version));
            }
            Release => {
                finish_data_control_devices(client, state, dispatcher, self.id);
                finish_ext_data_control_devices(client, state, dispatcher, self.id);
                client.delete_id(&WlDisplay::default(), state, dispatcher, self.id.into());
            }
        }
        Ok(())
    }
}

fn finish_data_control_devices(
    client: &mut Client,
    state: &mut State,
    dispatcher: &mut ClientDispatcher,
    seat_id: WlSeatId,
) {
    use crate::wayland_objects::zwlr_data_control_offer_v1::inert_data_control_offer;

    for device in state.seat.data_control_devices(client.client_id()) {
        let finish = device
            .try_ref(state)
            .is_some_and(|device_obj| device_obj.seat_id() == seat_id && !device_obj.is_inert());
        if !finish {
            continue;
        }
        let (offer, primary_offer) = {
            let mut device_obj = device.ref_mut(state);
            device_obj.finish(dispatcher);
            (device_obj.take_selection_offer(), device_obj.take_primary_offer())
        };
        state.seat.unregister_data_control_device(device);
        if let Some(offer) = offer {
            inert_data_control_offer(offer, state);
        }
        if let Some(offer) = primary_offer {
            inert_data_control_offer(offer, state);
        }
    }
}

fn finish_ext_data_control_devices(
    client: &mut Client,
    state: &mut State,
    dispatcher: &mut ClientDispatcher,
    seat_id: WlSeatId,
) {
    use crate::wayland_objects::ext_data_control_offer_v1::inert_ext_data_control_offer;

    for device in state.seat.ext_data_control_devices(client.client_id()) {
        let finish = device
            .try_ref(state)
            .is_some_and(|device_obj| device_obj.seat_id() == seat_id && !device_obj.is_inert());
        if !finish {
            continue;
        }
        let (offer, primary_offer) = {
            let mut device_obj = device.ref_mut(state);
            device_obj.finish(dispatcher);
            (device_obj.take_selection_offer(), device_obj.take_primary_offer())
        };
        state.seat.unregister_ext_data_control_device(device);
        if let Some(offer) = offer {
            inert_ext_data_control_offer(offer, state);
        }
        if let Some(offer) = primary_offer {
            inert_ext_data_control_offer(offer, state);
        }
    }
}

impl WlSeat {
    fn advertised_capabilities() -> u32 {
        use wl_seat::Capability::{Keyboard, Pointer};
        Keyboard as u32 | Pointer as u32
    }

    pub fn send_capabilities(&self, dispatcher: &mut ClientDispatcher) {
        let capabilites =
            wl_seat::Event::Capabilities { capabilities: Self::advertised_capabilities() };
        self.send_event(capabilites, dispatcher);
    }

    pub fn send_name(&self, dispatcher: &mut ClientDispatcher) {
        let name = wl_seat::Event::Name { name: "my_seat".to_string() };
        self.send_event(name, dispatcher);
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use crate::{
        clients::{Arguments, Client, ClientId, RawRequest},
        protocol_generated::{wl_seat, WlSeatId, WlTouchId},
        runner::Dispatcher,
        test_support::state_for_tests,
        wayland_objects::wl_touch::WlTouch,
    };

    use super::WlSeat;

    #[test]
    fn advertised_capabilities_omit_touch() {
        let caps = WlSeat::advertised_capabilities();
        assert_eq!(caps & wl_seat::Capability::Touch as u32, 0);
        assert_ne!(caps & wl_seat::Capability::Pointer as u32, 0);
        assert_ne!(caps & wl_seat::Capability::Keyboard as u32, 0);
    }

    #[test]
    fn get_touch_creates_an_object_the_client_can_release() {
        let mut state = state_for_tests();
        let mut client = Client::new_for_tests(ClientId::from_raw(1));
        let (seat, _) = client.insert_object(&mut state, WlSeat::new(WlSeatId::from(3)));
        let mut dispatcher = Dispatcher::default();
        let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);
        let request = RawRequest {
            object_id: 3,
            request_id: 2,
            arguments: Arguments::from_words(VecDeque::from([8])),
            fds: None,
        };
        state
            .objects
            .grab(seat.object_id())
            .on(&mut client, &mut state, request, &mut client_dispatcher)
            .unwrap();
        assert!(client.contains_object(WlTouchId::from(8), &state));

        let touch = state
            .objects
            .iter()
            .find_map(|(id, object)| object.downcast_ref::<WlTouch>().map(|_| id));
        let touch = touch.expect("inert touch object");
        let release = RawRequest {
            object_id: 8,
            request_id: 0,
            arguments: Arguments::from_words(VecDeque::new()),
            fds: None,
        };
        state
            .objects
            .grab(touch)
            .on(&mut client, &mut state, release, &mut client_dispatcher)
            .unwrap();
        assert!(!client.contains_object(WlTouchId::from(8), &state));
    }
}
