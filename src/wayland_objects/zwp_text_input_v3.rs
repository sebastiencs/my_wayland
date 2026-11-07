//! `zwp_text_input_v3`.
//!
//! Requests double-buffer until `commit`. `done.serial` is how many commits this
//! object has applied. Leaving keyboard focus does not clear `enabled`.

use crate::{
    clients::{Client, ClientDispatcher, EventWriter, RawEvent, RawRequest, Resolved},
    input::ime::{self, TextInputCommitKind, TextInputRelayChanges, TextInputRelayState},
    object::{Object, Parser, RequestResult},
    protocol_generated::{wl_display, zwp_text_input_v3, WaylandId, WlSurfaceId, ZwpTextInputV3Id},
    region::RectI,
    state::{GrabbedObject, State},
    wayland_objects::wl_display::WlDisplay,
};

const MAX_SURROUNDING_BYTES: usize = 4000;

#[derive(Clone, Debug)]
struct TextInputState {
    enabled: bool,
    surrounding: Option<(String, u32, u32)>,
    change_cause: u32,
    content_type: Option<(u32, u32)>,
    cursor_rect: Option<RectI>,
}

impl Default for TextInputState {
    fn default() -> Self {
        Self {
            enabled: false,
            surrounding: None,
            change_cause: zwp_text_input_v3::ChangeCause::InputMethod as u32,
            content_type: None,
            cursor_rect: None,
        }
    }
}

#[derive(Debug)]
pub struct ZwpTextInputV3 {
    id: ZwpTextInputV3Id,
    version: u32,
    pending: TextInputState,
    current: TextInputState,
    enable_requested: bool,
    disable_requested: bool,
    surrounding_set: bool,
    cause_set: bool,
    content_type_set: bool,
    commit_count: u32,
    entered: Option<Resolved<WlSurfaceId>>,
}

impl ZwpTextInputV3 {
    pub(crate) fn new(id: ZwpTextInputV3Id, version: u32) -> Self {
        Self {
            id,
            version,
            pending: TextInputState::default(),
            current: TextInputState::default(),
            enable_requested: false,
            disable_requested: false,
            surrounding_set: false,
            cause_set: false,
            content_type_set: false,
            commit_count: 0,
            entered: None,
        }
    }

    pub(crate) fn commit_count(&self) -> u32 {
        self.commit_count
    }

    pub(crate) fn entered(&self) -> Option<Resolved<WlSurfaceId>> {
        self.entered
    }

    pub(crate) fn set_entered(&mut self, entered: Option<Resolved<WlSurfaceId>>) {
        self.entered = entered;
    }

    pub(crate) fn is_enabled(&self) -> bool {
        self.current.enabled
    }

    pub(crate) fn surrounding(&self) -> Option<&(String, u32, u32)> {
        self.current.surrounding.as_ref()
    }

    pub(crate) fn cursor_rect(&self) -> Option<RectI> {
        self.current.cursor_rect
    }

    #[cfg(test)]
    pub(crate) fn set_cursor_rect_for_tests(&mut self, cursor_rect: Option<RectI>) {
        self.current.cursor_rect = cursor_rect;
    }

    pub(crate) fn relay_state(&self) -> TextInputRelayState {
        TextInputRelayState {
            surrounding: self.current.surrounding.clone(),
            change_cause: self.current.change_cause,
            content_type: self.current.content_type,
        }
    }

    pub(crate) fn send_preedit(
        &self,
        text: Option<String>,
        cursor_begin: i32,
        cursor_end: i32,
        dispatcher: &mut ClientDispatcher,
    ) {
        match text {
            Some(text) => self.send_event(
                zwp_text_input_v3::Event::PreeditString { text, cursor_begin, cursor_end },
                dispatcher,
            ),
            None => self.send_nullable_text(2, None, Some((cursor_begin, cursor_end)), dispatcher),
        }
    }

    pub(crate) fn send_commit_string(
        &self,
        text: Option<String>,
        dispatcher: &mut ClientDispatcher,
    ) {
        match text {
            Some(text) => {
                self.send_event(zwp_text_input_v3::Event::CommitString { text }, dispatcher);
            }
            None => self.send_nullable_text(3, None, None, dispatcher),
        }
    }

    pub(crate) fn send_delete_surrounding(
        &self,
        before_length: u32,
        after_length: u32,
        dispatcher: &mut ClientDispatcher,
    ) {
        self.send_event(
            zwp_text_input_v3::Event::DeleteSurroundingText { before_length, after_length },
            dispatcher,
        );
    }

    fn send_nullable_text(
        &self,
        opcode: u16,
        text: Option<String>,
        cursors: Option<(i32, i32)>,
        dispatcher: &mut ClientDispatcher,
    ) {
        let nargs = 1 + usize::from(cursors.is_some()) * 2;
        let mut raw = RawEvent::new(self.id(), opcode, nargs);
        match text {
            Some(text) => raw.string(text),
            None => raw.null_string(),
        }
        if let Some((begin, end)) = cursors {
            raw.int(begin);
            raw.int(end);
        }
        dispatcher.send_event(raw);
    }

    fn unregister(
        &self,
        client: &Client,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
        disconnecting: bool,
    ) {
        let Some(text_input) = client.resolve(self.id, state) else {
            return;
        };
        if state.seat.ime.unregister_text_input(text_input) {
            let skip = disconnecting.then_some(client.client_id());
            ime::deactivate_input_method(state, dispatcher, skip);
        }
    }
}

impl Object for ZwpTextInputV3 {
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
        use zwp_text_input_v3::Request::*;

        match client.parse::<Self>(req, dispatcher)? {
            Destroy => {
                self.unregister(client, state, dispatcher, false);
                client.delete_id(&WlDisplay::default(), state, dispatcher, self.id.into());
            }
            Enable => {
                self.pending = TextInputState::default();
                self.pending.enabled = true;
                self.enable_requested = true;
                self.surrounding_set = false;
                self.cause_set = false;
                self.content_type_set = false;
            }
            Disable => {
                self.pending.enabled = false;
                self.disable_requested = true;
            }
            SetSurroundingText { text, cursor, anchor } => {
                if let Err(message) = check_surrounding(&text, cursor, anchor) {
                    client.protocol_violation_disconnect(
                        dispatcher,
                        self.id.into(),
                        wl_display::Error::InvalidMethod as u32,
                        message,
                    );
                    return Ok(());
                }
                self.pending.surrounding = Some((text, cursor as u32, anchor as u32));
                self.surrounding_set = true;
            }
            SetTextChangeCause { cause } => {
                self.pending.change_cause = cause;
                self.cause_set = true;
            }
            SetContentType { hint, purpose } => {
                self.pending.content_type = Some((hint, purpose));
                self.content_type_set = true;
            }
            SetCursorRectangle { x, y, width, height } => {
                let Some(rect) = RectI::new(x, y, width, height) else {
                    client.protocol_violation_disconnect(
                        dispatcher,
                        self.id.into(),
                        wl_display::Error::InvalidMethod as u32,
                        "zwp_text_input_v3.set_cursor_rectangle width and height must be non-negative",
                    );
                    return Ok(());
                };
                self.pending.cursor_rect = Some(rect);
            }
            Commit => self.commit(client, state, dispatcher),
            SetAvailableActions { .. } | ShowInputPanel | HideInputPanel => {
                log::debug!(
                    "text-input ignored version 2 request on version {} object",
                    self.version
                );
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
        self.unregister(client, state, dispatcher, true);
    }
}

impl ZwpTextInputV3 {
    fn commit(
        self: &mut GrabbedObject<Self>,
        client: &Client,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
    ) {
        let was_enabled = self.current.enabled;
        self.commit_count = self.commit_count.wrapping_add(1);
        self.current = self.pending.clone();
        let enable_requested = self.enable_requested;
        let disable_requested = self.disable_requested;
        let changes = TextInputRelayChanges {
            surrounding: self.surrounding_set,
            change_cause: self.cause_set,
            content_type: self.content_type_set,
        };
        self.enable_requested = false;
        self.disable_requested = false;
        self.surrounding_set = false;
        self.cause_set = false;
        self.content_type_set = false;
        self.pending.change_cause = zwp_text_input_v3::ChangeCause::InputMethod as u32;

        let kind = ime::commit_kind(
            was_enabled,
            self.current.enabled,
            enable_requested,
            disable_requested,
        );
        log::debug!(
            "text-input commit id={} count={} kind={:?} enabled={}",
            u32::from(WaylandId::from(self.id)),
            self.commit_count,
            kind,
            self.current.enabled
        );
        let Some(text_input) = client.resolve(self.id, state) else {
            return;
        };
        let was_active = state.seat.ime.active_text_input() == Some(text_input);
        let focused = state.seat.keyboard_focus().map(|focus| focus.surface);
        let relay = self.relay_state();
        state
            .seat
            .ime
            .note_commit(text_input, kind, self.entered, focused);
        ime::relay_text_input_commit(
            state, dispatcher, text_input, kind, was_active, &relay, changes,
        );
        dispatcher.dispatch_post_action(ime::RepositionInputPopups);
    }
}

fn check_surrounding(text: &str, cursor: i32, anchor: i32) -> Result<(), String> {
    if text.len() > MAX_SURROUNDING_BYTES {
        return Err(format!(
            "zwp_text_input_v3.set_surrounding_text text is {} bytes; the maximum is {MAX_SURROUNDING_BYTES}",
            text.len()
        ));
    }
    for (name, offset) in [("cursor", cursor), ("anchor", anchor)] {
        if offset < 0 || offset as usize > text.len() || !text.is_char_boundary(offset as usize) {
            return Err(format!(
                "zwp_text_input_v3.set_surrounding_text {name} {offset} is not a byte offset on a code point boundary"
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use crate::{
        capture::test_sent_opcodes,
        clients::{Arguments, Client, ClientAction, ClientId, RawRequest, Resolved},
        input::{ime::TextInputCommitKind, SurfaceFocus},
        object::{Object, RequestResult},
        protocol_generated::{WlSeatId, WlSurfaceId, ZwpTextInputManagerV3Id, ZwpTextInputV3Id},
        runner::{Dispatcher, Model},
        state::ObjectId,
        test_support::state_for_tests,
        wayland_objects::{
            wl_seat::WlSeat, wl_surface::WlSurface,
            zwp_text_input_manager_v3::ZwpTextInputManagerV3,
        },
    };

    use super::ZwpTextInputV3;

    struct Fixture {
        state: crate::state::State,
        client: Client,
        dispatcher: Dispatcher,
        surface: Resolved<WlSurfaceId>,
        manager: Resolved<ZwpTextInputManagerV3Id>,
    }

    fn new_fixture() -> Fixture {
        let mut state = state_for_tests();
        let mut client = Client::new_for_tests(ClientId::from_raw(1));
        let dispatcher = Dispatcher::default();
        let surface = insert_surface(&mut client, &mut state, 5);
        let _seat = client
            .insert_object(&mut state, WlSeat::new(WlSeatId::from(4)))
            .0;
        let manager = client
            .insert_object(
                &mut state,
                ZwpTextInputManagerV3::new(ZwpTextInputManagerV3Id::from(6), 1),
            )
            .0;
        Fixture { state, client, dispatcher, surface, manager }
    }

    fn insert_surface(
        client: &mut Client,
        state: &mut crate::state::State,
        id: u32,
    ) -> Resolved<WlSurfaceId> {
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

    fn dispatch(
        fixture: &mut Fixture,
        object: ObjectId,
        wire_id: u32,
        request_id: u16,
        words: &[u32],
    ) -> RequestResult {
        let request = request(wire_id, request_id, words);
        let mut client_dispatcher = fixture.client.test_dispatcher(&mut fixture.dispatcher);
        client_dispatcher.set_requester_object_id_for_tests(wire_id.into());
        fixture.state.objects.grab(object).on(
            &mut fixture.client,
            &mut fixture.state,
            request,
            &mut client_dispatcher,
        )
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

    fn get_text_input(fixture: &mut Fixture, id: u32) -> RequestResult {
        let manager = fixture.manager.object_id();
        dispatch(fixture, manager, 6, 1, &[id, 4])
    }

    fn text_input(fixture: &Fixture, id: u32) -> Resolved<ZwpTextInputV3Id> {
        fixture
            .client
            .resolve(ZwpTextInputV3Id::from(id), &fixture.state)
            .unwrap()
    }

    fn enable(fixture: &mut Fixture, id: u32) {
        let object = text_input(fixture, id).object_id();
        dispatch(fixture, object, id, 1, &[]).unwrap();
    }

    fn disable(fixture: &mut Fixture, id: u32) {
        let object = text_input(fixture, id).object_id();
        dispatch(fixture, object, id, 2, &[]).unwrap();
    }

    fn set_surrounding(fixture: &mut Fixture, id: u32, text: &str, cursor: i32, anchor: i32) {
        let object = text_input(fixture, id).object_id();
        let mut words = string_words(text);
        words.push(cursor as u32);
        words.push(anchor as u32);
        dispatch(fixture, object, id, 3, &words).unwrap();
    }

    fn commit(fixture: &mut Fixture, id: u32) {
        let object = text_input(fixture, id).object_id();
        dispatch(fixture, object, id, 7, &[]).unwrap();
    }

    fn focus_surface(fixture: &mut Fixture, surface: Resolved<WlSurfaceId>) {
        crate::input::set_keyboard_focus(
            &mut fixture.state,
            &mut fixture.dispatcher,
            Some(SurfaceFocus::new(surface)),
        );
        drain(fixture);
    }

    fn drain(fixture: &mut Fixture) {
        let actions = fixture
            .dispatcher
            .test_take_actions_for::<ClientAction, ClientId>(fixture.client.client_id());
        for action in actions {
            if let ClientAction::TextInputEvent { .. } = &action {
                fixture
                    .client
                    .process(&mut fixture.state, action, &mut fixture.dispatcher)
                    .unwrap();
            }
        }
    }

    fn opcodes(fixture: &Fixture, object: u32) -> Vec<u32> {
        test_sent_opcodes(&fixture.dispatcher, object)
    }

    #[test]
    fn commit_kinds_follow_the_enabled_edge() {
        assert_eq!(
            crate::input::ime::commit_kind(false, true, true, false),
            TextInputCommitKind::Enable
        );
        assert_eq!(
            crate::input::ime::commit_kind(true, false, false, true),
            TextInputCommitKind::Disable
        );
        assert_eq!(
            crate::input::ime::commit_kind(true, true, true, true),
            TextInputCommitKind::Reset
        );
        assert_eq!(
            crate::input::ime::commit_kind(true, true, false, false),
            TextInputCommitKind::Update
        );
        assert_eq!(
            crate::input::ime::commit_kind(false, true, true, true),
            TextInputCommitKind::Enable
        );
    }

    #[test]
    fn enable_wipes_pending_state_and_commit_counts() {
        let mut fixture = new_fixture();
        let surface = fixture.surface;
        focus_surface(&mut fixture, surface);
        get_text_input(&mut fixture, 7).unwrap();
        set_surrounding(&mut fixture, 7, "before", 0, 0);
        enable(&mut fixture, 7);
        set_surrounding(&mut fixture, 7, "after", 1, 1);
        commit(&mut fixture, 7);

        let text_input = text_input(&fixture, 7);
        let object = text_input.rref(&fixture.state);
        assert_eq!(object.commit_count(), 1);
        assert!(object.is_enabled());
        assert_eq!(object.surrounding().map(|text| text.0.as_str()), Some("after"));
        assert_eq!(fixture.state.seat.ime.active_text_input(), Some(text_input));

        commit(&mut fixture, 7);
        assert_eq!(text_input.rref(&fixture.state).commit_count(), 2);
    }

    #[test]
    fn disable_clears_only_the_active_text_input() {
        let mut fixture = new_fixture();
        let surface = fixture.surface;
        focus_surface(&mut fixture, surface);
        get_text_input(&mut fixture, 7).unwrap();
        get_text_input(&mut fixture, 8).unwrap();
        enable(&mut fixture, 7);
        commit(&mut fixture, 7);
        enable(&mut fixture, 8);
        commit(&mut fixture, 8);
        assert_eq!(fixture.state.seat.ime.active_text_input(), Some(text_input(&fixture, 8)));

        disable(&mut fixture, 7);
        commit(&mut fixture, 7);
        assert_eq!(fixture.state.seat.ime.active_text_input(), Some(text_input(&fixture, 8)));

        disable(&mut fixture, 8);
        commit(&mut fixture, 8);
        assert!(fixture.state.seat.ime.active_text_input().is_none());
        assert!(!text_input(&fixture, 8).rref(&fixture.state).is_enabled());
    }

    #[test]
    fn reset_keeps_the_text_input_active() {
        let mut fixture = new_fixture();
        let surface = fixture.surface;
        focus_surface(&mut fixture, surface);
        get_text_input(&mut fixture, 7).unwrap();
        enable(&mut fixture, 7);
        commit(&mut fixture, 7);
        disable(&mut fixture, 7);
        enable(&mut fixture, 7);
        set_surrounding(&mut fixture, 7, "again", 0, 0);
        commit(&mut fixture, 7);

        let text_input = text_input(&fixture, 7);
        assert_eq!(fixture.state.seat.ime.active_text_input(), Some(text_input));
        assert!(text_input.rref(&fixture.state).is_enabled());
        assert_eq!(
            text_input
                .rref(&fixture.state)
                .surrounding()
                .map(|text| text.0.as_str()),
            Some("again")
        );
    }

    #[test]
    fn enter_before_any_commit_uses_serial_zero() {
        let mut fixture = new_fixture();
        let surface = fixture.surface;
        get_text_input(&mut fixture, 7).unwrap();
        focus_surface(&mut fixture, surface);
        assert_eq!(opcodes(&fixture, 7), vec![0, 5]);
        assert!(crate::capture::test_sent_arg(&fixture.dispatcher, 7, 5, 0));
        assert_eq!(text_input(&fixture, 7).rref(&fixture.state).entered(), Some(surface));
    }

    #[test]
    fn get_text_input_while_focused_enters_immediately() {
        let mut fixture = new_fixture();
        let surface = fixture.surface;
        focus_surface(&mut fixture, surface);
        get_text_input(&mut fixture, 7).unwrap();
        assert_eq!(opcodes(&fixture, 7), vec![0, 5]);
        assert_eq!(text_input(&fixture, 7).rref(&fixture.state).entered(), Some(surface));
    }

    #[test]
    fn focus_change_between_two_surfaces_leaves_then_enters() {
        let mut fixture = new_fixture();
        let first = fixture.surface;
        let other = insert_surface(&mut fixture.client, &mut fixture.state, 9);
        get_text_input(&mut fixture, 7).unwrap();
        focus_surface(&mut fixture, first);
        let first = opcodes(&fixture, 7);
        focus_surface(&mut fixture, other);
        let second = opcodes(&fixture, 7);
        assert_eq!(&second[first.len()..], &[1, 5, 0, 5]);
        assert_eq!(text_input(&fixture, 7).rref(&fixture.state).entered(), Some(other));
        assert!(fixture.state.seat.ime.active_text_input().is_none());
    }

    #[test]
    fn focus_change_between_clients_leaves_the_old_client() {
        let mut fixture = new_fixture();
        let surface = fixture.surface;
        get_text_input(&mut fixture, 20).unwrap();
        focus_surface(&mut fixture, surface);
        enable(&mut fixture, 20);
        commit(&mut fixture, 20);

        let mut other_client = Client::new_for_tests(ClientId::from_raw(2));
        let other_surface = insert_surface(&mut other_client, &mut fixture.state, 11);
        let _seat = other_client
            .insert_object(&mut fixture.state, WlSeat::new(WlSeatId::from(4)))
            .0;
        let _manager = other_client
            .insert_object(
                &mut fixture.state,
                ZwpTextInputManagerV3::new(ZwpTextInputManagerV3Id::from(6), 1),
            )
            .0;
        let manager = other_client
            .resolve(ZwpTextInputManagerV3Id::from(6), &fixture.state)
            .unwrap()
            .object_id();
        let request = request(6, 1, &[30, 4]);
        let mut client_dispatcher = other_client.test_dispatcher(&mut fixture.dispatcher);
        client_dispatcher.set_requester_object_id_for_tests(6.into());
        fixture
            .state
            .objects
            .grab(manager)
            .on(&mut other_client, &mut fixture.state, request, &mut client_dispatcher)
            .unwrap();
        crate::input::set_keyboard_focus(
            &mut fixture.state,
            &mut fixture.dispatcher,
            Some(SurfaceFocus::new(other_surface)),
        );
        drain(&mut fixture);
        drain_client(&mut other_client, &mut fixture);

        let left = opcodes(&fixture, 20);
        assert_eq!(&left[left.len() - 2..], &[1, 5]);
        assert_eq!(opcodes(&fixture, 30), vec![0, 5]);
        assert!(fixture.state.seat.ime.active_text_input().is_none());
        assert!(text_input(&fixture, 20).rref(&fixture.state).is_enabled());
        assert_eq!(
            other_client
                .resolve(ZwpTextInputV3Id::from(30), &fixture.state)
                .unwrap()
                .rref(&fixture.state)
                .entered(),
            Some(other_surface)
        );
    }

    fn drain_client(client: &mut Client, fixture: &mut Fixture) {
        let actions = fixture
            .dispatcher
            .test_take_actions_for::<ClientAction, ClientId>(client.client_id());
        for action in actions {
            if let ClientAction::TextInputEvent { .. } = &action {
                client
                    .process(&mut fixture.state, action, &mut fixture.dispatcher)
                    .unwrap();
            }
        }
    }

    #[test]
    fn destroying_the_active_text_input_clears_it() {
        let mut fixture = new_fixture();
        let surface = fixture.surface;
        focus_surface(&mut fixture, surface);
        get_text_input(&mut fixture, 7).unwrap();
        enable(&mut fixture, 7);
        commit(&mut fixture, 7);
        let object = text_input(&fixture, 7).object_id();
        dispatch(&mut fixture, object, 7, 0, &[]).unwrap();
        assert!(fixture.state.seat.ime.active_text_input().is_none());
        assert!(fixture
            .client
            .resolve(ZwpTextInputV3Id::from(7), &fixture.state)
            .is_none());
    }

    #[test]
    fn a_cursor_inside_a_code_point_disconnects() {
        let mut fixture = new_fixture();
        get_text_input(&mut fixture, 7).unwrap();
        let object = text_input(&fixture, 7).object_id();
        let mut words = string_words("é");
        words.push(1);
        words.push(1);
        dispatch(&mut fixture, object, 7, 3, &words).unwrap();
        assert!(fixture.dispatcher.test_has_queued_action(|action: &crate::clients::ClientManagerAction| {
            matches!(action, crate::clients::ClientManagerAction::Close { id } if *id == ClientId::from_raw(1))
        }));
    }
}
