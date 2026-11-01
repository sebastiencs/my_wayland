//! `ext_session_lock_manager_v1`. Advertised at version 1.

use crate::{
    clients::{Client, ClientDispatcher, RawRequest},
    globals::Global,
    object::{Object, Parser, RequestResult},
    protocol_generated::{
        ext_session_lock_manager_v1, Constants, ExtSessionLockManagerV1Id, ExtSessionLockV1Id,
        NewId, WaylandId,
    },
    state::{GrabbedObject, State},
    wayland_objects::{ext_session_lock_v1::ExtSessionLockV1, wl_display::WlDisplay},
};

#[derive(Debug)]
pub struct ExtSessionLockManagerV1 {
    id: ExtSessionLockManagerV1Id,
}

impl Object for ExtSessionLockManagerV1 {
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
        use ext_session_lock_manager_v1::Request::*;
        match client.parse::<Self>(req, dispatcher)? {
            Destroy => {
                client.delete_id(&WlDisplay::default(), state, dispatcher, self.id.into());
            }
            Lock { id } => {
                let id = ExtSessionLockV1Id::from(id);
                let client_id = client.client_id();
                let (resolved, _) =
                    client.insert_object(state, ExtSessionLockV1::new(id, client_id));
                if !crate::wayland_objects::ext_session_lock_v1::accept_lock(
                    state, client_id, resolved,
                ) {
                    resolved.ref_mut(state).deny(dispatcher);
                } else {
                    crate::capture::stop_protected_toplevel_captures(state, dispatcher);
                    let outputs = state.session_lock.covered_outputs();
                    crate::drm::request_outputs(state, outputs, dispatcher);
                    crate::seat::gate_for_session_lock(state, dispatcher);
                    crate::wayland_objects::ext_session_lock_v1::publish_locked_hint(
                        state, dispatcher,
                    );
                }
            }
        }
        Ok(())
    }
}

/// Version 1 matches the published session-lock protocol. Sway, wlroots, and Jay
/// advertise this same version; live lock cycles stay with the later validation.
#[derive(Debug)]
pub struct ExtSessionLockManagerV1Global;

impl ExtSessionLockManagerV1Global {
    pub fn new() -> Box<dyn Global> {
        Box::new(Self)
    }
}

impl Global for ExtSessionLockManagerV1Global {
    fn interface(&self) -> (&'static str, u32) {
        (ExtSessionLockManagerV1::INTERFACE_NAME, ExtSessionLockManagerV1::INTERFACE_VERSION)
    }

    fn bind(
        &mut self,
        client: &mut Client,
        state: &mut State,
        id: NewId,
        _version: u32,
        _dispatcher: &mut ClientDispatcher,
    ) {
        client.insert_object(
            state,
            ExtSessionLockManagerV1::new(ExtSessionLockManagerV1Id::from(id)),
        );
    }
}

impl ExtSessionLockManagerV1 {
    pub fn new(id: ExtSessionLockManagerV1Id) -> Self {
        Self { id }
    }
}

#[cfg(test)]
mod tests {
    use {
        super::*,
        crate::{
            clients::{Client, ClientAction, ClientId, ClientManagerAction, RawEvent},
            config::{Binding, CompositorAction, KeyBinding},
            dbus::DbusAction,
            input::types::SurfaceFocus,
            object::IdToObject,
            protocol_generated::{ExtSessionLockSurfaceV1Id, WlOutputId, WlSurfaceId},
            runner::{Dispatcher, Model},
            seat::handle_keyboard_key,
            session_lock::{LockBlank, SessionLockPhase},
            state::ObjectId,
            test_support::{state_for_tests, test_output_drm1},
            wayland_objects::{
                ext_session_lock_v1::{confirm_lock, on_protected_frame_presented},
                wl_output::WlOutput,
                wl_surface::{Role, WlSurface},
            },
        },
        xkbcommon::xkb,
    };

    const MANAGER_ID: u32 = 8;
    const LOCK_ID: u32 = 9;
    const OTHER_LOCK_ID: u32 = 10;

    fn request(object: u32, opcode: u16, words: u16) -> RawEvent {
        RawEvent::new(WaylandId::from(object), opcode, words.into())
    }

    fn dispatch<K>(
        client: &mut Client,
        state: &mut State,
        dispatcher: &mut Dispatcher,
        object: K,
        event: RawEvent,
    ) where
        K: IdToObject + Into<WaylandId> + Copy,
    {
        let mut client_dispatcher = client.test_dispatcher(dispatcher);
        client_dispatcher.set_requester_object_id_for_tests(object.into());
        client.grab_object(object, state).on(
            client,
            state,
            (&*event.into_bytes()).try_into().unwrap(),
            &mut client_dispatcher,
        );
    }

    fn insert_manager(client: &mut Client, state: &mut State) {
        client.insert_object(
            state,
            ExtSessionLockManagerV1::new(ExtSessionLockManagerV1Id::from(MANAGER_ID)),
        );
    }

    fn lock(client: &mut Client, state: &mut State, dispatcher: &mut Dispatcher, id: u32) {
        let mut event = request(MANAGER_ID, 1, 1);
        event.id(ExtSessionLockV1Id::from(id));
        dispatch(client, state, dispatcher, ExtSessionLockManagerV1Id::from(MANAGER_ID), event);
    }

    fn closed(dispatcher: &Dispatcher, client: &Client) -> bool {
        dispatcher.test_has_queued_action::<ClientManagerAction>(|action| {
            matches!(action, ClientManagerAction::Close { id } if *id == client.client_id())
        })
    }

    #[test]
    fn a_lock_drops_desktop_input_and_ignores_bindings() {
        let mut state = state_for_tests();
        let mut client = Client::new_for_tests(ClientId::from_raw(1));
        let mut dispatcher = Dispatcher::default();
        insert_manager(&mut client, &mut state);
        let surface = client
            .insert_object(&mut state, WlSurface::new(WlSurfaceId::from(20), client.client_id(), 6))
            .0;
        let focus = SurfaceFocus::new(surface);
        state.seat.keyboard_focus = Some(focus);
        state.seat.remember_keyboard_focus(focus);
        handle_keyboard_key(&mut state, &mut dispatcher, 30, 1, 0);
        assert!(state.seat.last_keyboard_trigger_serial().is_some());

        let keysym = xkb::Keysym::from(xkb::keysyms::KEY_a);
        assert_eq!(state.keyboard.key_get_one_sym(30), keysym);
        state.config.bindings.push(Binding {
            trigger: KeyBinding { description: "quit".into(), keysym, modifiers: 0 },
            action: CompositorAction::Quit,
        });
        lock(&mut client, &mut state, &mut dispatcher, LOCK_ID);

        assert!(state.seat.keyboard_focus().is_none());
        assert!(state.seat.last_keyboard_trigger_serial().is_none());
        assert_eq!(state.seat.keyboard_focus_history(), vec![focus]);
        handle_keyboard_key(&mut state, &mut dispatcher, 30, 1, 0);
        assert!(!dispatcher.shutdown_requested());
    }

    #[test]
    fn the_manager_is_advertised_at_version_one() {
        let state = state_for_tests();
        let name = state
            .globals
            .name_for_interface(ExtSessionLockManagerV1::INTERFACE_NAME)
            .expect("session lock manager");
        let version = state
            .globals
            .iter()
            .find(|(candidate, _)| *candidate == name)
            .map(|(_, global)| global.interface().1);
        assert_eq!(version, Some(ExtSessionLockManagerV1::INTERFACE_VERSION));
    }

    #[test]
    fn the_first_lock_stays_locking_and_a_second_is_finished() {
        let mut state = state_for_tests();
        let mut client = Client::new_for_tests(ClientId::from_raw(1));
        let mut dispatcher = Dispatcher::default();
        insert_manager(&mut client, &mut state);
        let surface = client
            .insert_object(&mut state, WlSurface::new(WlSurfaceId::from(20), client.client_id(), 6))
            .0;
        state.seat.keyboard_focus = Some(SurfaceFocus::new(surface));

        lock(&mut client, &mut state, &mut dispatcher, LOCK_ID);
        assert_eq!(state.session_lock.phase(), SessionLockPhase::Locking);
        assert!(!state.session_lock.locked_sent());
        assert!(state.seat.keyboard_focus().is_none());
        assert_eq!(state.session_lock.pre_lock_focus().keyboard, Some(SurfaceFocus::new(surface)));
        assert!(!dispatcher.test_has_queued_action::<ClientAction>(|action| {
            matches!(action, ClientAction::SendSessionLockLocked)
        }));

        lock(&mut client, &mut state, &mut dispatcher, OTHER_LOCK_ID);
        assert_eq!(
            state.session_lock.lock().unwrap().wayland_id(),
            ExtSessionLockV1Id::from(LOCK_ID)
        );
        assert!(client
            .resolve(ExtSessionLockV1Id::from(OTHER_LOCK_ID), &state)
            .unwrap()
            .rref(&state)
            .finished_sent());
        dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ExtSessionLockV1Id::from(OTHER_LOCK_ID),
            request(OTHER_LOCK_ID, 0, 0),
        );
        assert_eq!(state.session_lock.phase(), SessionLockPhase::Locking);
        assert_eq!(
            state.session_lock.lock().unwrap().wayland_id(),
            ExtSessionLockV1Id::from(LOCK_ID)
        );
    }

    #[test]
    fn destroy_before_locked_returns_to_unlocked() {
        let mut state = state_for_tests();
        let mut client = Client::new_for_tests(ClientId::from_raw(1));
        let mut dispatcher = Dispatcher::default();
        insert_manager(&mut client, &mut state);
        lock(&mut client, &mut state, &mut dispatcher, LOCK_ID);
        dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ExtSessionLockV1Id::from(LOCK_ID),
            request(LOCK_ID, 0, 0),
        );
        assert_eq!(state.session_lock.phase(), SessionLockPhase::Unlocked);
        assert!(!client.contains_object(ExtSessionLockV1Id::from(LOCK_ID), &state));
    }

    #[test]
    fn unlock_before_locked_disconnects_and_the_close_cancels() {
        let mut state = state_for_tests();
        let mut client = Client::new_for_tests(ClientId::from_raw(1));
        let mut dispatcher = Dispatcher::default();
        insert_manager(&mut client, &mut state);
        lock(&mut client, &mut state, &mut dispatcher, LOCK_ID);
        dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ExtSessionLockV1Id::from(LOCK_ID),
            request(LOCK_ID, 2, 0),
        );
        assert!(closed(&dispatcher, &client));
        assert_eq!(state.session_lock.phase(), SessionLockPhase::Locking);
        client
            .process(&mut state, ClientAction::Close, &mut dispatcher)
            .unwrap();
        assert_eq!(state.session_lock.phase(), SessionLockPhase::Unlocked);
    }

    #[test]
    fn destroy_after_locked_is_invalid_and_disconnect_abandons() {
        let mut state = state_for_tests();
        let mut client = Client::new_for_tests(ClientId::from_raw(1));
        let mut dispatcher = Dispatcher::default();
        insert_manager(&mut client, &mut state);
        lock(&mut client, &mut state, &mut dispatcher, LOCK_ID);
        state.session_lock.mark_locked_for_tests();
        client
            .resolve(ExtSessionLockV1Id::from(LOCK_ID), &state)
            .unwrap()
            .ref_mut(&mut state)
            .mark_locked_sent();
        dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ExtSessionLockV1Id::from(LOCK_ID),
            request(LOCK_ID, 0, 0),
        );
        assert!(closed(&dispatcher, &client));
        assert_eq!(state.session_lock.phase(), SessionLockPhase::Locked);
        client
            .process(&mut state, ClientAction::Close, &mut dispatcher)
            .unwrap();
        assert_eq!(state.session_lock.phase(), SessionLockPhase::Abandoned);
    }

    #[test]
    fn unlock_after_locked_returns_to_unlocked() {
        let mut state = state_for_tests();
        let mut client = Client::new_for_tests(ClientId::from_raw(1));
        let mut dispatcher = Dispatcher::default();
        insert_manager(&mut client, &mut state);
        lock(&mut client, &mut state, &mut dispatcher, LOCK_ID);
        state.session_lock.mark_locked_for_tests();
        client
            .resolve(ExtSessionLockV1Id::from(LOCK_ID), &state)
            .unwrap()
            .ref_mut(&mut state)
            .mark_locked_sent();
        dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ExtSessionLockV1Id::from(LOCK_ID),
            request(LOCK_ID, 2, 0),
        );
        assert_eq!(state.session_lock.phase(), SessionLockPhase::Unlocked);
        assert!(!closed(&dispatcher, &client));
    }

    fn locked_hints(dispatcher: &Dispatcher, locked: bool) -> usize {
        dispatcher.test_count_queued_action(|action: &DbusAction| {
            matches!(action, DbusAction::SetLockedHint { locked: value } if *value == locked)
        })
    }

    #[test]
    fn unlock_restores_a_mapped_window_and_clears_the_locked_hint() {
        let mut state = state_for_tests();
        let output = state.insert_output(test_output_drm1(0, 0, 640, 480));
        let mut client = Client::new_for_tests(ClientId::from_raw(1));
        let mut dispatcher = Dispatcher::default();
        insert_manager(&mut client, &mut state);
        let surface = client
            .insert_object(&mut state, WlSurface::new(WlSurfaceId::from(20), client.client_id(), 6))
            .0;
        surface
            .ref_mut(&mut state)
            .test_map(ObjectId::from(40usize), 0, 0, 20, 20);
        let focus = SurfaceFocus::new(surface);
        state.seat.keyboard_focus = Some(focus);

        lock(&mut client, &mut state, &mut dispatcher, LOCK_ID);
        assert!(state.seat.keyboard_focus().is_none());
        assert_eq!(locked_hints(&dispatcher, true), 1);
        state.session_lock.consume_full_damage(output);
        state.session_lock.consume_full_damage(output);
        state.session_lock.note_session_resumed();
        assert!(state.session_lock.consume_full_damage(output));

        state.session_lock.mark_locked_for_tests();
        client
            .resolve(ExtSessionLockV1Id::from(LOCK_ID), &state)
            .unwrap()
            .ref_mut(&mut state)
            .mark_locked_sent();
        dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ExtSessionLockV1Id::from(LOCK_ID),
            request(LOCK_ID, 2, 0),
        );
        assert_eq!(state.session_lock.phase(), SessionLockPhase::Unlocked);
        assert_eq!(state.seat.keyboard_focus(), Some(focus));
        assert_eq!(locked_hints(&dispatcher, false), 1);
    }

    #[test]
    fn a_disconnect_after_locked_accepts_one_replacement() {
        let mut state = state_for_tests();
        let mut client = Client::new_for_tests(ClientId::from_raw(1));
        let mut dispatcher = Dispatcher::default();
        insert_manager(&mut client, &mut state);
        lock(&mut client, &mut state, &mut dispatcher, LOCK_ID);
        state.session_lock.mark_locked_for_tests();
        client
            .resolve(ExtSessionLockV1Id::from(LOCK_ID), &state)
            .unwrap()
            .ref_mut(&mut state)
            .mark_locked_sent();
        client
            .process(&mut state, ClientAction::Close, &mut dispatcher)
            .unwrap();
        assert_eq!(state.session_lock.phase(), SessionLockPhase::Abandoned);

        let mut replacement = Client::new_for_tests(ClientId::from_raw(2));
        let mut replacement_dispatcher = Dispatcher::default();
        insert_manager(&mut replacement, &mut state);
        lock(&mut replacement, &mut state, &mut replacement_dispatcher, LOCK_ID);
        assert_eq!(state.session_lock.phase(), SessionLockPhase::Locking);
        assert_eq!(state.session_lock.owner(), Some(ClientId::from_raw(2)));
        assert!(!state.session_lock.locked_sent());

        lock(&mut replacement, &mut state, &mut replacement_dispatcher, OTHER_LOCK_ID);
        assert_eq!(state.session_lock.owner(), Some(ClientId::from_raw(2)));
        assert!(replacement
            .resolve(ExtSessionLockV1Id::from(OTHER_LOCK_ID), &state)
            .unwrap()
            .rref(&state)
            .finished_sent());
    }

    #[test]
    fn destroying_the_manager_leaves_the_lock() {
        let mut state = state_for_tests();
        let mut client = Client::new_for_tests(ClientId::from_raw(1));
        let mut dispatcher = Dispatcher::default();
        insert_manager(&mut client, &mut state);
        lock(&mut client, &mut state, &mut dispatcher, LOCK_ID);
        dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ExtSessionLockManagerV1Id::from(MANAGER_ID),
            request(MANAGER_ID, 0, 0),
        );
        assert!(!client.contains_object(ExtSessionLockManagerV1Id::from(MANAGER_ID), &state));
        assert_eq!(state.session_lock.phase(), SessionLockPhase::Locking);
        assert!(client.contains_object(ExtSessionLockV1Id::from(LOCK_ID), &state));
    }

    #[test]
    fn an_empty_output_set_does_not_confirm_the_lock() {
        let mut state = state_for_tests();
        let mut client = Client::new_for_tests(ClientId::from_raw(1));
        let mut dispatcher = Dispatcher::default();
        insert_manager(&mut client, &mut state);
        lock(&mut client, &mut state, &mut dispatcher, LOCK_ID);
        let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);
        assert!(!confirm_lock(&mut state, &mut client_dispatcher));
        assert_eq!(state.session_lock.phase(), SessionLockPhase::Locking);
    }

    #[test]
    fn every_active_output_must_be_presented_before_locked() {
        let mut state = state_for_tests();
        let output = state.insert_output(test_output_drm1(0, 0, 800, 600));
        let mut client = Client::new_for_tests(ClientId::from_raw(1));
        let mut dispatcher = Dispatcher::default();
        insert_manager(&mut client, &mut state);
        lock(&mut client, &mut state, &mut dispatcher, LOCK_ID);
        assert!(!state.session_lock.barrier_satisfied());
        {
            let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);
            assert!(!confirm_lock(&mut state, &mut client_dispatcher));
        }
        state.session_lock.mark_output_presented(output);
        let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);
        assert!(confirm_lock(&mut state, &mut client_dispatcher));
        assert_eq!(state.session_lock.phase(), SessionLockPhase::Locked);
        assert!(client
            .resolve(ExtSessionLockV1Id::from(LOCK_ID), &state)
            .unwrap()
            .rref(&state)
            .locked_notified());
    }

    #[test]
    fn get_lock_surface_configures_the_output_size_and_acks_once() {
        let mut state = state_for_tests();
        let output_id = state.insert_output(test_output_drm1(0, 0, 640, 480));
        let mut client = Client::new_for_tests(ClientId::from_raw(1));
        let mut dispatcher = Dispatcher::default();
        insert_manager(&mut client, &mut state);
        lock(&mut client, &mut state, &mut dispatcher, LOCK_ID);
        client.insert_object(
            &mut state,
            WlSurface::new(WlSurfaceId::from(20), client.client_id(), 6),
        );
        client.insert_object(&mut state, WlOutput::new(WlOutputId::from(21), output_id, 4));
        let mut event = request(LOCK_ID, 1, 3);
        event.id(ExtSessionLockSurfaceV1Id::from(30));
        event.id(WlSurfaceId::from(20));
        event.id(WlOutputId::from(21));
        dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ExtSessionLockV1Id::from(LOCK_ID),
            event,
        );
        let configures = client
            .resolve(ExtSessionLockSurfaceV1Id::from(30), &state)
            .unwrap()
            .rref(&state)
            .configures()
            .to_vec();
        assert_eq!(configures, [(1, 640, 480)]);
        assert_eq!(
            state
                .session_lock
                .surface_for_output(output_id)
                .unwrap()
                .wayland_id(),
            ExtSessionLockSurfaceV1Id::from(30)
        );
        assert_eq!(
            client
                .resolve(WlSurfaceId::from(20), &state)
                .unwrap()
                .rref(&state)
                .role(),
            Role::LockSurface
        );

        let mut ack = request(30, 1, 1);
        ack.uint(1);
        dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ExtSessionLockSurfaceV1Id::from(30),
            ack,
        );
        assert!(!closed(&dispatcher, &client));
        let mut ack = request(30, 1, 1);
        ack.uint(1);
        dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ExtSessionLockSurfaceV1Id::from(30),
            ack,
        );
        assert!(closed(&dispatcher, &client));
    }

    #[test]
    fn a_second_surface_for_the_same_output_is_a_protocol_error() {
        let mut state = state_for_tests();
        let output_id = state.insert_output(test_output_drm1(0, 0, 100, 80));
        let mut client = Client::new_for_tests(ClientId::from_raw(1));
        let mut dispatcher = Dispatcher::default();
        insert_manager(&mut client, &mut state);
        lock(&mut client, &mut state, &mut dispatcher, LOCK_ID);
        client.insert_object(
            &mut state,
            WlSurface::new(WlSurfaceId::from(20), client.client_id(), 6),
        );
        client.insert_object(
            &mut state,
            WlSurface::new(WlSurfaceId::from(22), client.client_id(), 6),
        );
        client.insert_object(&mut state, WlOutput::new(WlOutputId::from(21), output_id, 4));
        let mut event = request(LOCK_ID, 1, 3);
        event.id(ExtSessionLockSurfaceV1Id::from(30));
        event.id(WlSurfaceId::from(20));
        event.id(WlOutputId::from(21));
        dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ExtSessionLockV1Id::from(LOCK_ID),
            event,
        );
        let mut event = request(LOCK_ID, 1, 3);
        event.id(ExtSessionLockSurfaceV1Id::from(31));
        event.id(WlSurfaceId::from(22));
        event.id(WlOutputId::from(21));
        dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ExtSessionLockV1Id::from(LOCK_ID),
            event,
        );
        assert!(closed(&dispatcher, &client));
        assert!(!client.contains_object(ExtSessionLockSurfaceV1Id::from(31), &state));
    }

    #[test]
    fn a_surface_that_already_has_a_role_is_rejected() {
        let mut state = state_for_tests();
        let output_id = state.insert_output(test_output_drm1(0, 0, 100, 80));
        let mut client = Client::new_for_tests(ClientId::from_raw(1));
        let mut dispatcher = Dispatcher::default();
        insert_manager(&mut client, &mut state);
        lock(&mut client, &mut state, &mut dispatcher, LOCK_ID);
        client.insert_object(
            &mut state,
            WlSurface::new(WlSurfaceId::from(20), client.client_id(), 6),
        );
        assert!(client
            .resolve(WlSurfaceId::from(20), &state)
            .unwrap()
            .ref_mut(&mut state)
            .assign_lock_surface_role());
        client.insert_object(&mut state, WlOutput::new(WlOutputId::from(21), output_id, 4));
        let mut event = request(LOCK_ID, 1, 3);
        event.id(ExtSessionLockSurfaceV1Id::from(30));
        event.id(WlSurfaceId::from(20));
        event.id(WlOutputId::from(21));
        dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ExtSessionLockV1Id::from(LOCK_ID),
            event,
        );
        assert!(closed(&dispatcher, &client));
    }

    #[test]
    fn disconnect_while_locking_cancels() {
        let mut state = state_for_tests();
        let mut client = Client::new_for_tests(ClientId::from_raw(1));
        let mut dispatcher = Dispatcher::default();
        insert_manager(&mut client, &mut state);
        lock(&mut client, &mut state, &mut dispatcher, LOCK_ID);
        client
            .process(&mut state, ClientAction::Close, &mut dispatcher)
            .unwrap();
        assert_eq!(state.session_lock.phase(), SessionLockPhase::Unlocked);
    }

    #[test]
    fn the_blank_is_black_until_the_locker_is_gone() {
        let mut state = state_for_tests();
        let output = state.insert_output(test_output_drm1(0, 0, 640, 480));
        let mut client = Client::new_for_tests(ClientId::from_raw(1));
        let mut dispatcher = Dispatcher::default();
        insert_manager(&mut client, &mut state);
        assert!(state.session_lock.blank().is_none());
        lock(&mut client, &mut state, &mut dispatcher, LOCK_ID);
        assert_eq!(state.session_lock.blank(), Some(LockBlank::Black));
        assert!(state.session_lock.consume_full_damage(output));
        assert!(state.session_lock.consume_full_damage(output));
        assert!(!state.session_lock.consume_full_damage(output));

        state.session_lock.mark_locked_for_tests();
        client
            .resolve(ExtSessionLockV1Id::from(LOCK_ID), &state)
            .unwrap()
            .ref_mut(&mut state)
            .mark_locked_sent();
        client
            .process(&mut state, ClientAction::Close, &mut dispatcher)
            .unwrap();
        assert_eq!(state.session_lock.phase(), SessionLockPhase::Abandoned);
        assert_eq!(state.session_lock.blank(), Some(LockBlank::Red));
        assert!(state.session_lock.consume_full_damage(output));
        assert!(state.session_lock.consume_full_damage(output));
        assert!(!state.session_lock.consume_full_damage(output));
    }

    #[test]
    fn a_protected_flip_sends_locked_only_after_every_output() {
        let mut state = state_for_tests();
        let first = state.insert_output(test_output_drm1(0, 0, 640, 480));
        let second = state.insert_output(test_output_drm1(640, 0, 640, 480));
        let mut client = Client::new_for_tests(ClientId::from_raw(1));
        let mut dispatcher = Dispatcher::default();
        insert_manager(&mut client, &mut state);
        lock(&mut client, &mut state, &mut dispatcher, LOCK_ID);

        on_protected_frame_presented(&mut state, first, &mut dispatcher);
        assert_eq!(state.session_lock.phase(), SessionLockPhase::Locking);
        assert!(!dispatcher.test_has_queued_action::<ClientAction>(|action| {
            matches!(action, ClientAction::SendSessionLockLocked)
        }));

        on_protected_frame_presented(&mut state, second, &mut dispatcher);
        assert_eq!(state.session_lock.phase(), SessionLockPhase::Locked);
        let lock = client
            .resolve(ExtSessionLockV1Id::from(LOCK_ID), &state)
            .unwrap();
        assert!(lock.rref(&state).locked_notified());
        assert!(!lock.rref(&state).locked_event_sent());
        client
            .process(&mut state, ClientAction::SendSessionLockLocked, &mut dispatcher)
            .unwrap();
        assert!(lock.rref(&state).locked_event_sent());
        client
            .process(&mut state, ClientAction::SendSessionLockLocked, &mut dispatcher)
            .unwrap();
        assert!(lock.rref(&state).locked_event_sent());
    }

    #[test]
    fn removing_an_output_does_not_invent_a_presentation() {
        let mut state = state_for_tests();
        let first = state.insert_output(test_output_drm1(0, 0, 640, 480));
        let second = state.insert_output(test_output_drm1(640, 0, 640, 480));
        let mut client = Client::new_for_tests(ClientId::from_raw(1));
        let mut dispatcher = Dispatcher::default();
        insert_manager(&mut client, &mut state);
        lock(&mut client, &mut state, &mut dispatcher, LOCK_ID);

        assert!(state.session_lock.detach_output(second).is_none());
        assert!(!state.session_lock.barrier_satisfied());
        on_protected_frame_presented(&mut state, second, &mut dispatcher);
        assert_eq!(state.session_lock.phase(), SessionLockPhase::Locking);

        on_protected_frame_presented(&mut state, first, &mut dispatcher);
        assert_eq!(state.session_lock.phase(), SessionLockPhase::Locked);
    }

    #[test]
    fn dropping_an_unpresented_output_does_not_release_the_barrier() {
        let mut state = state_for_tests();
        let first = state.insert_output(test_output_drm1(0, 0, 640, 480));
        let second = state.insert_output(test_output_drm1(640, 0, 640, 480));
        let mut client = Client::new_for_tests(ClientId::from_raw(1));
        let mut dispatcher = Dispatcher::default();
        insert_manager(&mut client, &mut state);
        lock(&mut client, &mut state, &mut dispatcher, LOCK_ID);

        on_protected_frame_presented(&mut state, first, &mut dispatcher);
        assert!(state.session_lock.detach_output(second).is_none());
        assert_eq!(state.session_lock.phase(), SessionLockPhase::Locking);
        assert!(!dispatcher.test_has_queued_action::<ClientAction>(|action| {
            matches!(action, ClientAction::SendSessionLockLocked)
        }));
    }

    #[test]
    fn an_empty_output_set_stays_unconfirmed() {
        let mut state = state_for_tests();
        let mut client = Client::new_for_tests(ClientId::from_raw(1));
        let mut dispatcher = Dispatcher::default();
        insert_manager(&mut client, &mut state);
        lock(&mut client, &mut state, &mut dispatcher, LOCK_ID);
        let output = state.insert_output(test_output_drm1(0, 0, 640, 480));
        on_protected_frame_presented(&mut state, output, &mut dispatcher);
        assert_eq!(state.session_lock.phase(), SessionLockPhase::Locking);
        assert!(!dispatcher.test_has_queued_action::<ClientAction>(|action| {
            matches!(action, ClientAction::SendSessionLockLocked)
        }));
    }
}
