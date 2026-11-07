//! `zwp_input_method_v2`.
//!
//! `commit` echoes the number of `done` events this object has sent. A
//! different serial drops the pending batch. The keyboard grab routes hardware
//! keys. Popup surfaces take the input-popup role and are placed in phase 5.

use crate::{
    clients::{Client, ClientDispatcher, RawRequest, Resolved},
    input::ime,
    object::{Object, Parser, RequestResult},
    protocol_generated::{wl_display, zwp_input_method_v2, WaylandId, ZwpInputMethodV2Id},
    state::{GrabbedObject, State},
    wayland_objects::{
        wl_display::WlDisplay, zwp_input_method_keyboard_grab_v2::ZwpInputMethodKeyboardGrabV2,
        zwp_input_popup_surface_v2::ZwpInputPopupSurfaceV2,
    },
};

const MAX_TEXT_BYTES: usize = 4000;

#[derive(Clone, Debug, Default)]
struct PendingText {
    commit_string: Option<String>,
    preedit: Option<(String, i32, i32)>,
    delete: Option<(u32, u32)>,
}

#[derive(Debug)]
pub struct ZwpInputMethodV2 {
    id: ZwpInputMethodV2Id,
    version: u32,
    /// Set when another input method already owned the seat. Not stored there.
    inert: bool,
    active: bool,
    done_count: u32,
    pending: PendingText,
}

impl ZwpInputMethodV2 {
    pub(crate) fn new(id: ZwpInputMethodV2Id, version: u32, inert: bool) -> Self {
        Self {
            id,
            version,
            inert,
            active: false,
            done_count: 0,
            pending: PendingText::default(),
        }
    }

    pub(crate) fn set_active(&mut self, active: bool) {
        self.active = active;
    }

    pub(crate) fn increment_done_count(&mut self) {
        self.done_count = self.done_count.wrapping_add(1);
    }

    fn resolved(&self, client: &Client, state: &State) -> Option<Resolved<ZwpInputMethodV2Id>> {
        client.resolve(self.id, state)
    }

    fn is_live(&self, client: &Client, state: &State) -> bool {
        if self.inert {
            return false;
        }
        self.resolved(client, state)
            .is_some_and(|input_method| state.seat.ime.input_method() == Some(input_method))
    }

    fn reject_text(
        &self,
        client: &mut Client,
        dispatcher: &mut ClientDispatcher,
        text: &str,
    ) -> bool {
        if text.len() <= MAX_TEXT_BYTES {
            return false;
        }
        client.protocol_violation_disconnect(
            dispatcher,
            self.id.into(),
            wl_display::Error::InvalidMethod as u32,
            format!(
                "zwp_input_method_v2 text is {} bytes; the maximum is {MAX_TEXT_BYTES}",
                text.len()
            ),
        );
        true
    }

    fn clear_seat(
        &self,
        client: &Client,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
        disconnecting: bool,
    ) {
        let Some(input_method) = self.resolved(client, state) else {
            return;
        };
        if state.seat.ime.input_method() != Some(input_method) {
            return;
        }
        state.seat.ime.clear_input_method();
        let skip = disconnecting.then_some(client.client_id());
        ime::release_keyboard_grab(state, dispatcher, skip);
        ime::retire_input_method_popups(state, dispatcher, skip);
        ime::clear_active_preedit(state, dispatcher, skip);
    }
}

impl Object for ZwpInputMethodV2 {
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
        use zwp_input_method_v2::Request::*;

        let live = self.is_live(client, state);
        match client.parse::<Self>(req, dispatcher)? {
            Destroy => {
                self.clear_seat(client, state, dispatcher, false);
                client.delete_id(&WlDisplay::default(), state, dispatcher, self.id.into());
            }
            CommitString { text } => {
                if !live {
                    return Ok(());
                }
                if self.reject_text(client, dispatcher, &text) {
                    return Ok(());
                }
                self.pending.commit_string = Some(text);
            }
            SetPreeditString { text, cursor_begin, cursor_end } => {
                if !live {
                    return Ok(());
                }
                if self.reject_text(client, dispatcher, &text) {
                    return Ok(());
                }
                self.pending.preedit = Some((text, cursor_begin, cursor_end));
            }
            DeleteSurroundingText { before_length, after_length } => {
                if live {
                    self.pending.delete = Some((before_length, after_length));
                }
            }
            Commit { serial } => {
                if !live {
                    return Ok(());
                }
                if serial != self.done_count {
                    log::debug!(
                        "input-method dropped commit serial={} done_count={}",
                        serial,
                        self.done_count
                    );
                    self.pending = PendingText::default();
                    return Ok(());
                }
                let pending = std::mem::take(&mut self.pending);
                let Some(text_input) = state.seat.ime.active_text_input() else {
                    return Ok(());
                };
                dispatcher.dispatch_to(
                    text_input.client_id(),
                    crate::clients::ClientAction::TextInputEvent {
                        text_input,
                        event: ime::TextInputClientEvent::ImeCommit {
                            preedit: pending.preedit,
                            commit: pending.commit_string,
                            delete: pending.delete,
                        },
                    },
                );
            }
            GetInputPopupSurface { id, surface } => {
                let surface = client.resolve_or_disconnect(surface, state, dispatcher)?;
                let accepted = surface
                    .try_ref_mut(state)
                    .is_some_and(|wl_surface| wl_surface.set_input_popup_role().is_ok());
                if !accepted {
                    client.protocol_violation_disconnect(
                        dispatcher,
                        self.id.into(),
                        zwp_input_method_v2::Error::Role as u32,
                        "zwp_input_method_v2.get_input_popup_surface surface has a role or a buffer",
                    );
                    return Ok(());
                }
                let popup = client
                    .insert_object(state, ZwpInputPopupSurfaceV2::new(id, surface, !live))
                    .0;
                if let Some(mut wl_surface) = surface.try_ref_mut(state) {
                    wl_surface.assign_input_popup(popup);
                }
                if !state.root_surfaces.contains(&surface) {
                    state.register_root_surface(surface);
                }
                if live {
                    state.seat.ime.add_popup(popup, surface);
                    dispatcher.dispatch_post_action(ime::RepositionInputPopups);
                }
            }
            GrabKeyboard { keyboard } => {
                if !live {
                    client.protocol_violation_disconnect(
                        dispatcher,
                        self.id.into(),
                        wl_display::Error::InvalidMethod as u32,
                        "zwp_input_method_v2.grab_keyboard requires the seat's input method",
                    );
                    return Ok(());
                }
                if state.seat.ime.keyboard_grab().is_some() {
                    client.protocol_violation_disconnect(
                        dispatcher,
                        self.id.into(),
                        wl_display::Error::InvalidMethod as u32,
                        "zwp_input_method_v2.grab_keyboard already has a grab",
                    );
                    return Ok(());
                }
                let (grab, _) =
                    client.insert_object(state, ZwpInputMethodKeyboardGrabV2::new(keyboard));
                state.seat.ime.set_keyboard_grab(grab);
                ime::announce_keyboard_grab(state, dispatcher, grab);
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
        self.clear_seat(client, state, dispatcher, true);
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use crate::{
        clients::{Arguments, Client, ClientAction, ClientId, RawRequest, Resolved},
        input::SurfaceFocus,
        io_uring::model::IoUringAction,
        object::{Object, RequestResult},
        protocol_generated::{
            WlSeatId, WlSurfaceId, ZwpInputMethodManagerV2Id, ZwpInputMethodV2Id,
            ZwpTextInputManagerV3Id, ZwpTextInputV3Id,
        },
        runner::{Dispatcher, Model},
        state::{ObjectId, State},
        test_support::state_for_tests,
        wayland_objects::{
            wl_seat::WlSeat, wl_surface::WlSurface,
            zwp_input_method_manager_v2::ZwpInputMethodManagerV2,
            zwp_text_input_manager_v3::ZwpTextInputManagerV3,
        },
    };

    use super::ZwpInputMethodV2;

    struct App {
        client: Client,
        surface: Resolved<WlSurfaceId>,
    }

    struct Fixture {
        state: State,
        dispatcher: Dispatcher,
        app: App,
        ime: Client,
    }

    fn new_fixture() -> Fixture {
        let mut state = state_for_tests();
        let mut dispatcher = Dispatcher::default();
        let mut app_client = Client::new_for_tests(ClientId::from_raw(1));
        let surface = insert_surface(&mut app_client, &mut state, 5);
        let _ = app_client
            .insert_object(&mut state, WlSeat::new(WlSeatId::from(4)))
            .0;
        let _ = app_client
            .insert_object(
                &mut state,
                ZwpTextInputManagerV3::new(ZwpTextInputManagerV3Id::from(6), 1),
            )
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
        let mut fixture = Fixture {
            state,
            dispatcher,
            app: App { client: app_client, surface },
            ime,
        };
        focus(&mut fixture);
        get_text_input(&mut fixture, 20);
        fixture
    }

    fn insert_surface(client: &mut Client, state: &mut State, id: u32) -> Resolved<WlSurfaceId> {
        let wl_id = WlSurfaceId::from(id);
        let (surface, _) =
            client.insert_object(state, WlSurface::new(wl_id, client.client_id(), 6));
        state.register_root_surface(surface);
        surface
    }

    fn request(object_id: u32, request_id: u16, words: &[u32]) -> RawRequest<'static> {
        RawRequest {
            object_id,
            request_id,
            arguments: Arguments::from_words(VecDeque::from(words.to_vec())),
            fds: None,
        }
    }

    fn string_words(text: &str) -> Vec<u32> {
        let mut bytes = text.as_bytes().to_vec();
        bytes.push(0);
        let len = bytes.len() as u32;
        while bytes.len() % 4 != 0 {
            bytes.push(0);
        }
        let mut words = vec![len];
        for chunk in bytes.chunks(4) {
            words.push(u32::from_ne_bytes(chunk.try_into().unwrap()));
        }
        words
    }

    fn call(
        client: &mut Client,
        state: &mut State,
        dispatcher: &mut Dispatcher,
        wire_id: u32,
        request_id: u16,
        words: &[u32],
    ) -> RequestResult {
        let object = client_object(client, state, wire_id);
        let request = request(wire_id, request_id, words);
        let mut client_dispatcher = client.test_dispatcher(dispatcher);
        client_dispatcher.set_requester_object_id_for_tests(wire_id.into());
        state
            .objects
            .grab(object)
            .on(client, state, request, &mut client_dispatcher)
    }

    fn client_object(client: &Client, _state: &State, wire_id: u32) -> ObjectId {
        client
            .object_id_for_wayland(wire_id)
            .unwrap_or_else(|| panic!("missing object {wire_id}"))
    }

    fn drain(client: &mut Client, state: &mut State, dispatcher: &mut Dispatcher) {
        let actions =
            dispatcher.test_take_actions_for::<ClientAction, ClientId>(client.client_id());
        for action in actions {
            match &action {
                ClientAction::TextInputEvent { .. } | ClientAction::InputMethodEvent { .. } => {
                    client.process(state, action, dispatcher).unwrap();
                }
                _ => {}
            }
        }
    }

    fn focus(fixture: &mut Fixture) {
        let surface = fixture.app.surface;
        crate::input::set_keyboard_focus(
            &mut fixture.state,
            &mut fixture.dispatcher,
            Some(SurfaceFocus::new(surface)),
        );
        drain(&mut fixture.app.client, &mut fixture.state, &mut fixture.dispatcher);
    }

    fn get_text_input(fixture: &mut Fixture, id: u32) {
        call(&mut fixture.app.client, &mut fixture.state, &mut fixture.dispatcher, 6, 1, &[id, 4])
            .unwrap();
    }

    fn app_request(fixture: &mut Fixture, id: u32, request_id: u16, words: &[u32]) {
        call(
            &mut fixture.app.client,
            &mut fixture.state,
            &mut fixture.dispatcher,
            id,
            request_id,
            words,
        )
        .unwrap();
    }

    fn ime_request(fixture: &mut Fixture, id: u32, request_id: u16, words: &[u32]) {
        call(&mut fixture.ime, &mut fixture.state, &mut fixture.dispatcher, id, request_id, words)
            .unwrap();
    }

    fn bind_input_method(
        client: &mut Client,
        state: &mut State,
        dispatcher: &mut Dispatcher,
        id: u32,
    ) {
        call(client, state, dispatcher, 6, 0, &[4, id]).unwrap();
    }

    fn opcodes(dispatcher: &Dispatcher, object: u32) -> Vec<u32> {
        let mut opcodes = Vec::new();
        dispatcher.test_for_each_queued_action::<IoUringAction>(|action| {
            if let IoUringAction::Sendmsg { buffer, .. } = action {
                if buffer.len() >= 8
                    && u32::from_ne_bytes(buffer[0..4].try_into().unwrap()) == object
                {
                    opcodes.push(u32::from_ne_bytes(buffer[4..8].try_into().unwrap()) & 0xffff);
                }
            }
        });
        opcodes
    }

    fn first_args(dispatcher: &Dispatcher, object: u32, opcode: u32) -> Vec<u32> {
        let mut args = Vec::new();
        dispatcher.test_for_each_queued_action::<IoUringAction>(|action| {
            if let IoUringAction::Sendmsg { buffer, .. } = action {
                if buffer.len() >= 12
                    && u32::from_ne_bytes(buffer[0..4].try_into().unwrap()) == object
                    && u32::from_ne_bytes(buffer[4..8].try_into().unwrap()) & 0xffff == opcode
                {
                    args.push(u32::from_ne_bytes(buffer[8..12].try_into().unwrap()));
                }
            }
        });
        args
    }

    fn enable_with_state(fixture: &mut Fixture) {
        app_request(fixture, 20, 1, &[]);
        let mut surrounding = string_words("hi");
        surrounding.extend([1, 1]);
        app_request(fixture, 20, 3, &surrounding);
        app_request(fixture, 20, 4, &[1]);
        app_request(fixture, 20, 5, &[4, 2]);
        app_request(fixture, 20, 7, &[]);
    }

    #[test]
    fn a_second_input_method_is_unavailable_and_is_not_promoted() {
        let mut fixture = new_fixture();
        bind_input_method(&mut fixture.ime, &mut fixture.state, &mut fixture.dispatcher, 40);
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
        bind_input_method(&mut other, &mut fixture.state, &mut fixture.dispatcher, 41);
        assert_eq!(opcodes(&fixture.dispatcher, 41), vec![6]);
        assert_eq!(
            fixture
                .state
                .seat
                .ime
                .input_method()
                .map(|input_method| input_method.wayland_id()),
            Some(ZwpInputMethodV2Id::from(40))
        );

        ime_request(&mut fixture, 40, 6, &[]);
        assert!(fixture.state.seat.ime.input_method().is_none());
        assert_ne!(
            fixture
                .state
                .seat
                .ime
                .input_method()
                .map(|input_method| u32::from(input_method.wayland_id())),
            Some(41)
        );
    }

    #[test]
    fn enable_update_and_disable_reach_the_input_method_in_order() {
        let mut fixture = new_fixture();
        bind_input_method(&mut fixture.ime, &mut fixture.state, &mut fixture.dispatcher, 40);
        enable_with_state(&mut fixture);
        drain(&mut fixture.ime, &mut fixture.state, &mut fixture.dispatcher);
        assert_eq!(opcodes(&fixture.dispatcher, 40), vec![0, 2, 3, 4, 5]);

        app_request(&mut fixture, 20, 5, &[8, 3]);
        app_request(&mut fixture, 20, 7, &[]);
        drain(&mut fixture.ime, &mut fixture.state, &mut fixture.dispatcher);
        let update = opcodes(&fixture.dispatcher, 40);
        assert_eq!(&update[5..], &[4, 5]);

        app_request(&mut fixture, 20, 2, &[]);
        app_request(&mut fixture, 20, 7, &[]);
        drain(&mut fixture.ime, &mut fixture.state, &mut fixture.dispatcher);
        let disabled = opcodes(&fixture.dispatcher, 40);
        assert_eq!(&disabled[disabled.len() - 2..], &[1, 5]);
        assert!(fixture.state.seat.ime.active_text_input().is_none());
    }

    #[test]
    fn reset_deactivates_and_activates_with_one_done() {
        let mut fixture = new_fixture();
        bind_input_method(&mut fixture.ime, &mut fixture.state, &mut fixture.dispatcher, 40);
        enable_with_state(&mut fixture);
        drain(&mut fixture.ime, &mut fixture.state, &mut fixture.dispatcher);
        app_request(&mut fixture, 20, 2, &[]);
        app_request(&mut fixture, 20, 1, &[]);
        let mut surrounding = string_words("again");
        surrounding.extend([0, 0]);
        app_request(&mut fixture, 20, 3, &surrounding);
        app_request(&mut fixture, 20, 7, &[]);
        drain(&mut fixture.ime, &mut fixture.state, &mut fixture.dispatcher);
        let opcodes = opcodes(&fixture.dispatcher, 40);
        assert_eq!(&opcodes[5..], &[1, 0, 2, 3, 5]);
    }

    #[test]
    fn the_input_method_commit_serial_must_match_and_forwards_once() {
        let mut fixture = new_fixture();
        bind_input_method(&mut fixture.ime, &mut fixture.state, &mut fixture.dispatcher, 40);
        enable_with_state(&mut fixture);
        drain(&mut fixture.ime, &mut fixture.state, &mut fixture.dispatcher);

        let mut stale = string_words("no");
        ime_request(&mut fixture, 40, 0, &stale);
        ime_request(&mut fixture, 40, 3, &[99]);
        drain(&mut fixture.app.client, &mut fixture.state, &mut fixture.dispatcher);
        assert!(first_args(&fixture.dispatcher, 20, 3).is_empty());

        stale = string_words("yo");
        ime_request(&mut fixture, 40, 0, &stale);
        ime_request(&mut fixture, 40, 3, &[1]);
        drain(&mut fixture.app.client, &mut fixture.state, &mut fixture.dispatcher);
        assert_eq!(first_args(&fixture.dispatcher, 20, 3), vec![3]);
        assert!(opcodes(&fixture.dispatcher, 20).contains(&5));
    }

    #[test]
    fn a_commit_after_focus_loss_does_not_reach_the_text_input() {
        let mut fixture = new_fixture();
        bind_input_method(&mut fixture.ime, &mut fixture.state, &mut fixture.dispatcher, 40);
        enable_with_state(&mut fixture);
        drain(&mut fixture.ime, &mut fixture.state, &mut fixture.dispatcher);
        let mut text = string_words("yo");
        ime_request(&mut fixture, 40, 0, &text);
        ime_request(&mut fixture, 40, 3, &[1]);
        drain(&mut fixture.app.client, &mut fixture.state, &mut fixture.dispatcher);

        crate::input::set_keyboard_focus(&mut fixture.state, &mut fixture.dispatcher, None);
        drain(&mut fixture.app.client, &mut fixture.state, &mut fixture.dispatcher);
        drain(&mut fixture.ime, &mut fixture.state, &mut fixture.dispatcher);
        let commit_lengths = first_args(&fixture.dispatcher, 20, 3);

        text = string_words("z");
        ime_request(&mut fixture, 40, 0, &text);
        ime_request(&mut fixture, 40, 3, &[1]);
        ime_request(&mut fixture, 40, 0, &text);
        ime_request(&mut fixture, 40, 3, &[2]);
        drain(&mut fixture.app.client, &mut fixture.state, &mut fixture.dispatcher);
        assert_eq!(first_args(&fixture.dispatcher, 20, 3), commit_lengths);
        assert!(commit_lengths.contains(&0));
        assert!(commit_lengths.contains(&3));
    }

    #[test]
    fn an_input_method_that_binds_while_active_is_activated() {
        let mut fixture = new_fixture();
        enable_with_state(&mut fixture);
        bind_input_method(&mut fixture.ime, &mut fixture.state, &mut fixture.dispatcher, 40);
        drain(&mut fixture.ime, &mut fixture.state, &mut fixture.dispatcher);
        assert_eq!(opcodes(&fixture.dispatcher, 40), vec![0, 2, 3, 4, 5]);
    }

    #[test]
    fn destroying_either_side_notifies_the_other() {
        let mut fixture = new_fixture();
        bind_input_method(&mut fixture.ime, &mut fixture.state, &mut fixture.dispatcher, 40);
        enable_with_state(&mut fixture);
        drain(&mut fixture.ime, &mut fixture.state, &mut fixture.dispatcher);

        app_request(&mut fixture, 20, 0, &[]);
        drain(&mut fixture.ime, &mut fixture.state, &mut fixture.dispatcher);
        let after_text_input = opcodes(&fixture.dispatcher, 40);
        assert_eq!(&after_text_input[after_text_input.len() - 2..], &[1, 5]);

        let mut fixture = new_fixture();
        bind_input_method(&mut fixture.ime, &mut fixture.state, &mut fixture.dispatcher, 40);
        enable_with_state(&mut fixture);
        drain(&mut fixture.ime, &mut fixture.state, &mut fixture.dispatcher);
        ime_request(&mut fixture, 40, 6, &[]);
        drain(&mut fixture.app.client, &mut fixture.state, &mut fixture.dispatcher);
        assert!(first_args(&fixture.dispatcher, 20, 2).contains(&0));
        assert!(first_args(&fixture.dispatcher, 20, 3).contains(&0));
        assert!(fixture.state.seat.ime.input_method().is_none());
        assert!(fixture.state.seat.ime.active_text_input().is_some());
    }
}
