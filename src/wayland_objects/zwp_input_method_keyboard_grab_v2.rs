//! `zwp_input_method_keyboard_grab_v2`.
//!
//! Hardware keys reach this object after compositor bindings, and not the
//! focused client. A locked session bypasses the grab.

use crate::{
    clients::{Client, ClientDispatcher, EventWriter, RawRequest, Resolved},
    input::ime::{self, KeyboardGrabClientEvent},
    object::{Object, Parser, RequestResult},
    protocol_generated::{
        wl_keyboard, zwp_input_method_keyboard_grab_v2, WaylandId, ZwpInputMethodKeyboardGrabV2Id,
    },
    state::{GrabbedObject, State},
    wayland_objects::{
        wl_display::WlDisplay,
        wl_keyboard::{KEY_REPEAT_DELAY, KEY_REPEAT_RATE},
    },
    xkb::KeyboardModifiers,
};

#[derive(Debug)]
pub struct ZwpInputMethodKeyboardGrabV2 {
    id: ZwpInputMethodKeyboardGrabV2Id,
}

impl ZwpInputMethodKeyboardGrabV2 {
    pub(crate) fn new(id: ZwpInputMethodKeyboardGrabV2Id) -> Self {
        Self { id }
    }

    fn send_keymap(&self, keymap: &crate::xkb::KeymapBlob, dispatcher: &mut ClientDispatcher) {
        self.send_event(
            zwp_input_method_keyboard_grab_v2::Event::Keymap {
                format: wl_keyboard::KeymapFormat::XkbV1 as u32,
                fd: keymap.fd.rc(),
                size: keymap.fd.len() as u32,
            },
            dispatcher,
        );
    }

    fn send_key(
        &self,
        serial: u32,
        time: u32,
        key: u32,
        state: u32,
        dispatcher: &mut ClientDispatcher,
    ) {
        self.send_event(
            zwp_input_method_keyboard_grab_v2::Event::Key { serial, time, key, state },
            dispatcher,
        );
    }

    fn send_modifiers(
        &self,
        serial: u32,
        modifiers: KeyboardModifiers,
        dispatcher: &mut ClientDispatcher,
    ) {
        self.send_event(
            zwp_input_method_keyboard_grab_v2::Event::Modifiers {
                serial,
                mods_depressed: modifiers.mods_depressed,
                mods_latched: modifiers.mods_latched,
                mods_locked: modifiers.mods_locked,
                group: modifiers.group,
            },
            dispatcher,
        );
    }

    fn send_repeat_info(&self, dispatcher: &mut ClientDispatcher) {
        self.send_event(
            zwp_input_method_keyboard_grab_v2::Event::RepeatInfo {
                rate: KEY_REPEAT_RATE,
                delay: KEY_REPEAT_DELAY,
            },
            dispatcher,
        );
    }
}

impl Object for ZwpInputMethodKeyboardGrabV2 {
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
        match client.parse::<Self>(req, dispatcher)? {
            zwp_input_method_keyboard_grab_v2::Request::Release => {
                if let Some(grab) = client.resolve(self.id, state) {
                    if state.seat.ime.keyboard_grab() == Some(grab) {
                        ime::release_keyboard_grab(state, dispatcher, None);
                    }
                }
                client.delete_id(&WlDisplay::default(), state, dispatcher, self.id.into());
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
        if let Some(grab) = client.resolve(self.id, state) {
            if state.seat.ime.keyboard_grab() == Some(grab) {
                ime::release_keyboard_grab(state, dispatcher, Some(client.client_id()));
            }
        }
    }
}

pub(crate) fn deliver_keyboard_grab_event(
    state: &mut State,
    dispatcher: &mut ClientDispatcher,
    grab: Resolved<ZwpInputMethodKeyboardGrabV2Id>,
    event: KeyboardGrabClientEvent,
) {
    match event {
        KeyboardGrabClientEvent::Initial { modifiers } => {
            if state.seat.ime.keyboard_grab() != Some(grab) {
                return;
            }
            let keymap = state.xkb.keymap_blob();
            let serial = state.seat.next_serial();
            state.seat.record_client_serial(grab.client_id(), serial);
            let Some(object) = grab.try_ref(state) else {
                return;
            };
            object.send_keymap(&keymap, dispatcher);
            object.send_modifiers(serial, modifiers, dispatcher);
            object.send_repeat_info(dispatcher);
        }
        KeyboardGrabClientEvent::Key { serial, time, key, state: key_state } => {
            state.seat.record_client_serial(grab.client_id(), serial);
            let Some(object) = grab.try_ref(state) else {
                return;
            };
            object.send_key(serial, time, key, key_state, dispatcher);
        }
        KeyboardGrabClientEvent::Modifiers { serial, modifiers } => {
            state.seat.record_client_serial(grab.client_id(), serial);
            let Some(object) = grab.try_ref(state) else {
                return;
            };
            object.send_modifiers(serial, modifiers, dispatcher);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use xkbcommon::xkb;

    use crate::{
        clients::{Arguments, Client, ClientAction, ClientId, RawRequest},
        config::{Binding, CompositorAction, KeyBinding},
        input::SurfaceFocus,
        io_uring::model::IoUringAction,
        object::{Object, RequestResult},
        protocol_generated::{wl_display, WlSeatId, WlSurfaceId, ZwpInputMethodManagerV2Id},
        runner::{Dispatcher, Model},
        session_lock::SessionLockPhase,
        state::State,
        test_support::state_for_tests,
        wayland_objects::{
            wl_seat::WlSeat, wl_surface::WlSurface,
            zwp_input_method_manager_v2::ZwpInputMethodManagerV2,
        },
    };

    struct Fixture {
        state: State,
        dispatcher: Dispatcher,
        app: Client,
        ime: Client,
    }

    fn new_fixture() -> Fixture {
        let mut state = state_for_tests();
        let mut dispatcher = Dispatcher::default();
        let mut app = Client::new_for_tests(ClientId::from_raw(1));
        let surface = {
            let (surface, _) = app.insert_object(
                &mut state,
                WlSurface::new(WlSurfaceId::from(5), app.client_id(), 6),
            );
            state.register_root_surface(surface);
            surface
        };
        let _ = app
            .insert_object(&mut state, WlSeat::new(WlSeatId::from(4)))
            .0;
        let mut ime = Client::new_for_tests(ClientId::from_raw(2));
        let _ = ime
            .insert_object(&mut state, WlSeat::new(WlSeatId::from(4)))
            .0;
        let _ = ime
            .insert_object(
                &mut state,
                ZwpInputMethodManagerV2::new(ZwpInputMethodManagerV2Id::from(6), 1),
            )
            .0;
        let mut fixture = Fixture { state, dispatcher, app, ime };
        call(&mut fixture.app, &mut fixture.state, &mut fixture.dispatcher, 4, 1, &[8]).unwrap();
        crate::input::set_keyboard_focus(
            &mut fixture.state,
            &mut fixture.dispatcher,
            Some(SurfaceFocus::new(surface)),
        );
        drain(&mut fixture.app, &mut fixture.state, &mut fixture.dispatcher);
        call(&mut fixture.ime, &mut fixture.state, &mut fixture.dispatcher, 6, 0, &[4, 40])
            .unwrap();
        fixture
    }

    fn request(object_id: u32, request_id: u16, words: &[u32]) -> RawRequest<'static> {
        RawRequest {
            object_id,
            request_id,
            arguments: Arguments::from_words(VecDeque::from(words.to_vec())),
            fds: None,
        }
    }

    fn call(
        client: &mut Client,
        state: &mut State,
        dispatcher: &mut Dispatcher,
        wire_id: u32,
        request_id: u16,
        words: &[u32],
    ) -> RequestResult {
        let object = client
            .object_id_for_wayland(wire_id)
            .unwrap_or_else(|| panic!("missing object {wire_id}"));
        let request = request(wire_id, request_id, words);
        let mut client_dispatcher = client.test_dispatcher(dispatcher);
        client_dispatcher.set_requester_object_id_for_tests(wire_id.into());
        state
            .objects
            .grab(object)
            .on(client, state, request, &mut client_dispatcher)
    }

    fn drain(client: &mut Client, state: &mut State, dispatcher: &mut Dispatcher) {
        let actions =
            dispatcher.test_take_actions_for::<ClientAction, ClientId>(client.client_id());
        for action in actions {
            match &action {
                ClientAction::SeatEvent { .. }
                | ClientAction::KeyboardGrabEvent { .. }
                | ClientAction::InputMethodEvent { .. } => {
                    client.process(state, action, dispatcher).unwrap();
                }
                _ => {}
            }
        }
    }

    fn grab(fixture: &mut Fixture) {
        call(&mut fixture.ime, &mut fixture.state, &mut fixture.dispatcher, 40, 5, &[50]).unwrap();
        drain(&mut fixture.ime, &mut fixture.state, &mut fixture.dispatcher);
    }

    fn events(dispatcher: &Dispatcher, object: u32) -> Vec<(u32, Vec<u32>)> {
        let mut events = Vec::new();
        dispatcher.test_for_each_queued_action::<IoUringAction>(|action| {
            if let IoUringAction::Sendmsg { buffer, .. } = action {
                if buffer.len() >= 8
                    && u32::from_ne_bytes(buffer[0..4].try_into().unwrap()) == object
                {
                    let opcode = u32::from_ne_bytes(buffer[4..8].try_into().unwrap()) & 0xffff;
                    let mut args = Vec::new();
                    let mut offset = 8;
                    while offset + 4 <= buffer.len() {
                        args.push(u32::from_ne_bytes(
                            buffer[offset..offset + 4].try_into().unwrap(),
                        ));
                        offset += 4;
                    }
                    events.push((opcode, args));
                }
            }
        });
        events
    }

    fn opcodes(dispatcher: &Dispatcher, object: u32) -> Vec<u32> {
        events(dispatcher, object)
            .into_iter()
            .map(|(opcode, _)| opcode)
            .collect()
    }

    fn key_events(dispatcher: &Dispatcher, object: u32, opcode: u32) -> Vec<(u32, u32)> {
        events(dispatcher, object)
            .into_iter()
            .filter_map(|(event_opcode, args)| {
                if event_opcode == opcode && args.len() >= 4 {
                    Some((args[2], args[3]))
                } else {
                    None
                }
            })
            .collect()
    }

    fn closed(dispatcher: &Dispatcher, client: ClientId) -> bool {
        dispatcher.test_has_queued_action(|action: &crate::clients::ClientManagerAction| {
            matches!(action, crate::clients::ClientManagerAction::Close { id } if *id == client)
        })
    }

    #[test]
    fn the_grab_receives_the_hardware_keymap_and_then_keys_and_modifiers() {
        let mut fixture = new_fixture();
        grab(&mut fixture);
        assert_eq!(opcodes(&fixture.dispatcher, 50), vec![0, 2, 3]);

        let app_keys = key_events(&fixture.dispatcher, 8, 3);
        let before = opcodes(&fixture.dispatcher, 50).len();
        crate::seat::handle_keyboard_key(&mut fixture.state, &mut fixture.dispatcher, 42, 1, 9);
        drain(&mut fixture.ime, &mut fixture.state, &mut fixture.dispatcher);
        drain(&mut fixture.app, &mut fixture.state, &mut fixture.dispatcher);
        assert_eq!(key_events(&fixture.dispatcher, 50, 1), vec![(42, 1)]);
        assert_eq!(&opcodes(&fixture.dispatcher, 50)[before..], &[1, 2]);
        assert_eq!(key_events(&fixture.dispatcher, 8, 3), app_keys);
    }

    #[test]
    fn a_compositor_binding_wins_over_the_grab() {
        let mut fixture = new_fixture();
        grab(&mut fixture);
        fixture.state.config.bindings.push(Binding {
            trigger: KeyBinding {
                description: "quit".to_string(),
                keysym: xkb::Keysym::from(xkb::keysyms::KEY_Escape),
                modifiers: 0,
            },
            action: CompositorAction::Quit,
        });
        let grab_before = opcodes(&fixture.dispatcher, 50);
        let app_before = key_events(&fixture.dispatcher, 8, 3);
        crate::seat::handle_keyboard_key(&mut fixture.state, &mut fixture.dispatcher, 1, 1, 4);
        crate::seat::handle_keyboard_key(&mut fixture.state, &mut fixture.dispatcher, 1, 0, 5);
        drain(&mut fixture.ime, &mut fixture.state, &mut fixture.dispatcher);
        drain(&mut fixture.app, &mut fixture.state, &mut fixture.dispatcher);
        assert_eq!(opcodes(&fixture.dispatcher, 50), grab_before);
        assert_eq!(key_events(&fixture.dispatcher, 8, 3), app_before);
    }

    #[test]
    fn a_press_from_before_the_grab_is_released_to_the_client() {
        let mut fixture = new_fixture();
        crate::seat::handle_keyboard_key(&mut fixture.state, &mut fixture.dispatcher, 30, 1, 3);
        drain(&mut fixture.app, &mut fixture.state, &mut fixture.dispatcher);
        grab(&mut fixture);
        crate::seat::handle_keyboard_key(&mut fixture.state, &mut fixture.dispatcher, 30, 0, 4);
        drain(&mut fixture.app, &mut fixture.state, &mut fixture.dispatcher);
        drain(&mut fixture.ime, &mut fixture.state, &mut fixture.dispatcher);
        assert_eq!(key_events(&fixture.dispatcher, 8, 3), vec![(30, 1), (30, 0)]);
        assert!(key_events(&fixture.dispatcher, 50, 1).is_empty());
    }

    #[test]
    fn a_press_during_the_grab_is_swallowed_after_release() {
        let mut fixture = new_fixture();
        grab(&mut fixture);
        crate::seat::handle_keyboard_key(&mut fixture.state, &mut fixture.dispatcher, 16, 1, 6);
        drain(&mut fixture.ime, &mut fixture.state, &mut fixture.dispatcher);
        assert_eq!(key_events(&fixture.dispatcher, 50, 1), vec![(16, 1)]);
        call(&mut fixture.ime, &mut fixture.state, &mut fixture.dispatcher, 50, 0, &[]).unwrap();
        drain(&mut fixture.app, &mut fixture.state, &mut fixture.dispatcher);
        assert!(opcodes(&fixture.dispatcher, 8).contains(&4));
        let app_keys = key_events(&fixture.dispatcher, 8, 3);
        let grab_keys = key_events(&fixture.dispatcher, 50, 1);
        crate::seat::handle_keyboard_key(&mut fixture.state, &mut fixture.dispatcher, 16, 0, 7);
        drain(&mut fixture.app, &mut fixture.state, &mut fixture.dispatcher);
        drain(&mut fixture.ime, &mut fixture.state, &mut fixture.dispatcher);
        assert_eq!(key_events(&fixture.dispatcher, 8, 3), app_keys);
        assert_eq!(key_events(&fixture.dispatcher, 50, 1), grab_keys);
        assert!(fixture.state.seat.ime.keyboard_grab().is_none());
    }

    #[test]
    fn a_locked_session_bypasses_the_grab() {
        let mut fixture = new_fixture();
        grab(&mut fixture);
        let surface = SurfaceFocus::new(
            fixture
                .app
                .resolve(WlSurfaceId::from(5), &fixture.state)
                .expect("surface"),
        );
        fixture
            .state
            .session_lock
            .set_phase_for_tests(SessionLockPhase::Locked);
        let grab_keys = key_events(&fixture.dispatcher, 50, 1);
        crate::seat::handle_keyboard_key(&mut fixture.state, &mut fixture.dispatcher, 20, 1, 8);
        drain(&mut fixture.app, &mut fixture.state, &mut fixture.dispatcher);
        drain(&mut fixture.ime, &mut fixture.state, &mut fixture.dispatcher);
        assert_eq!(key_events(&fixture.dispatcher, 50, 1), grab_keys);

        fixture
            .state
            .session_lock
            .set_phase_for_tests(SessionLockPhase::Unlocked);
        crate::input::set_keyboard_focus(
            &mut fixture.state,
            &mut fixture.dispatcher,
            Some(surface),
        );
        drain(&mut fixture.app, &mut fixture.state, &mut fixture.dispatcher);
        crate::seat::handle_keyboard_key(&mut fixture.state, &mut fixture.dispatcher, 20, 0, 9);
        drain(&mut fixture.app, &mut fixture.state, &mut fixture.dispatcher);
        drain(&mut fixture.ime, &mut fixture.state, &mut fixture.dispatcher);
        assert_eq!(key_events(&fixture.dispatcher, 8, 3).last().copied(), Some((20, 0)));
        assert_eq!(key_events(&fixture.dispatcher, 50, 1), grab_keys);
    }

    #[test]
    fn a_second_grab_and_a_grab_on_an_inert_input_method_are_errors() {
        let mut fixture = new_fixture();
        grab(&mut fixture);
        call(&mut fixture.ime, &mut fixture.state, &mut fixture.dispatcher, 40, 5, &[51]).unwrap();
        assert!(closed(&fixture.dispatcher, fixture.ime.client_id()));
        let code = events(&fixture.dispatcher, 1)
            .into_iter()
            .find_map(|(opcode, args)| (opcode == 0 && args.len() >= 2).then_some(args[1]));
        assert_eq!(code, Some(wl_display::Error::InvalidMethod as u32));

        let mut fixture = new_fixture();
        grab(&mut fixture);
        let mut other = Client::new_for_tests(ClientId::from_raw(3));
        let _ = other
            .insert_object(&mut fixture.state, WlSeat::new(WlSeatId::from(4)))
            .0;
        let _ = other
            .insert_object(
                &mut fixture.state,
                ZwpInputMethodManagerV2::new(ZwpInputMethodManagerV2Id::from(6), 1),
            )
            .0;
        call(&mut other, &mut fixture.state, &mut fixture.dispatcher, 6, 0, &[4, 41]).unwrap();
        call(&mut other, &mut fixture.state, &mut fixture.dispatcher, 41, 5, &[60]).unwrap();
        assert!(closed(&fixture.dispatcher, other.client_id()));
        assert!(fixture.state.seat.ime.keyboard_grab().is_some());
    }
}
