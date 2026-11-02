//! `ext_session_lock_surface_v1`. The first configure is sent on creation.
//! A commit must follow an ack, carry a non-null buffer, and match the last acked size.

use crate::{
    clients::{Client, ClientDispatcher, EventWriter, RawRequest, Resolved},
    object::{Object, Parser, RequestResult},
    protocol_generated::{
        ext_session_lock_surface_v1, Constants, ExtSessionLockSurfaceV1Id, ExtSessionLockV1Id,
        WaylandId, WlSurfaceId,
    },
    state::{GrabbedObject, OutputId, State},
    wayland_objects::wl_display::WlDisplay,
};

#[derive(Debug)]
pub struct ExtSessionLockSurfaceV1 {
    id: ExtSessionLockSurfaceV1Id,
    lock: Resolved<ExtSessionLockV1Id>,
    surface: Resolved<WlSurfaceId>,
    output_id: OutputId,
    inert: bool,
    configures: Vec<(u32, u32, u32)>,
    acked_index: Option<usize>,
    /// Width and height of the last `ack_configure`. The next commit must match it.
    acked_size: Option<(u32, u32)>,
    mapped: bool,
}

impl ExtSessionLockSurfaceV1 {
    pub fn new(
        id: ExtSessionLockSurfaceV1Id,
        lock: Resolved<ExtSessionLockV1Id>,
        surface: Resolved<WlSurfaceId>,
        output_id: OutputId,
    ) -> Self {
        Self {
            id,
            lock,
            surface,
            output_id,
            inert: false,
            configures: Vec::new(),
            acked_index: None,
            acked_size: None,
            mapped: false,
        }
    }

    pub fn output_id(&self) -> OutputId {
        self.output_id
    }

    pub fn wl_surface(&self) -> Resolved<WlSurfaceId> {
        self.surface
    }

    pub fn configures(&self) -> &[(u32, u32, u32)] {
        &self.configures
    }

    pub fn configured_size(&self) -> Option<(u32, u32)> {
        self.configures
            .last()
            .map(|(_, width, height)| (*width, *height))
    }

    pub fn make_inert(&mut self) {
        self.inert = true;
    }

    pub fn is_inert(&self) -> bool {
        self.inert
    }

    pub fn is_mapped(&self) -> bool {
        self.mapped
    }

    /// The last acked configure is the size a non-null buffer must match.
    pub fn check_commit(
        &self,
        logical_size: Option<crate::transform::SizeI>,
    ) -> Result<(), (u32, &'static str)> {
        if self.inert {
            return Ok(());
        }
        let Some((width, height)) = self.acked_size else {
            return Err((
                ext_session_lock_surface_v1::Error::CommitBeforeFirstAck as u32,
                "ext_session_lock_surface_v1 commit before ack_configure",
            ));
        };
        let Some(logical_size) = logical_size else {
            return Err((
                ext_session_lock_surface_v1::Error::NullBuffer as u32,
                "ext_session_lock_surface_v1 commit with a null buffer",
            ));
        };
        if logical_size.width != width as i32 || logical_size.height != height as i32 {
            return Err((
                ext_session_lock_surface_v1::Error::DimensionsMismatch as u32,
                "ext_session_lock_surface_v1 commit size does not match the last ack",
            ));
        }
        Ok(())
    }

    pub fn mark_mapped_if_ready(&mut self) -> Option<OutputId> {
        if self.inert || self.acked_size.is_none() {
            return None;
        }
        self.mapped = true;
        Some(self.output_id)
    }

    pub fn configure(
        &mut self,
        serial: u32,
        width: u32,
        height: u32,
        dispatcher: &mut ClientDispatcher,
    ) {
        self.configures.push((serial, width, height));
        self.send_event(
            ext_session_lock_surface_v1::Event::Configure { serial, width, height },
            dispatcher,
        );
    }

    fn delete_self(
        &self,
        client: &mut Client,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
    ) {
        client.delete_id(&WlDisplay::default(), state, dispatcher, self.id.into());
    }
}

impl Object for ExtSessionLockSurfaceV1 {
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
        use ext_session_lock_surface_v1::Request::*;
        match client.parse::<Self>(req, dispatcher)? {
            Destroy => {
                let output_id = self.output_id;
                let repaint = !self.inert && state.session_lock.clear_surface(output_id);
                self.delete_self(client, state, dispatcher);
                if repaint {
                    crate::drm::request_output(state, output_id, dispatcher);
                    dispatcher.dispatch_post_action(crate::input::focus::RefreshLockFocus);
                }
            }
            AckConfigure { serial } => {
                let Some(index) = self
                    .configures
                    .iter()
                    .position(|(sent, _, _)| *sent == serial)
                else {
                    client.protocol_violation_disconnect(
                        dispatcher,
                        self.id.into(),
                        ext_session_lock_surface_v1::Error::InvalidSerial as u32,
                        "ext_session_lock_surface_v1.ack_configure serial was not sent",
                    );
                    return Ok(());
                };
                if self.acked_index.is_some_and(|acked| index <= acked) {
                    client.protocol_violation_disconnect(
                        dispatcher,
                        self.id.into(),
                        ext_session_lock_surface_v1::Error::InvalidSerial as u32,
                        "ext_session_lock_surface_v1.ack_configure serial was already consumed",
                    );
                    return Ok(());
                }
                let (_, width, height) = self.configures[index];
                self.acked_index = Some(index);
                self.acked_size = Some((width, height));
            }
        }
        Ok(())
    }

    fn on_client_disconnect(
        &mut self,
        _client: &mut Client,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
    ) {
        let output_id = self.output_id;
        if !self.inert && state.session_lock.clear_surface(output_id) {
            crate::drm::request_output(state, output_id, dispatcher);
            dispatcher.dispatch_post_action(crate::input::focus::RefreshLockFocus);
        }
    }
}

#[cfg(test)]
mod tests {
    use {
        super::*,
        crate::{
            clients::{Client, ClientAction, ClientId, ClientManagerAction, RawEvent, Resolved},
            drm::ARGB8888,
            input::types::SurfaceFocus,
            object::IdToObject,
            protocol_generated::{
                ExtSessionLockManagerV1Id, ExtSessionLockV1Id, WaylandId, WlBufferId, WlOutputId,
                WlSurfaceId, XdgPopupId, XdgToplevelId,
            },
            runner::{Dispatcher, Model},
            session_lock::SessionLockPhase,
            state::State,
            test_support::{state_for_tests, test_output_drm1},
            transform::SizeI,
            wayland_objects::{
                ext_session_lock_manager_v1::ExtSessionLockManagerV1,
                wl_buffer::{NewWlBufferParams, WlBuffer},
                wl_output::WlOutput,
                wl_shm_pool::shared_memory_from_bytes,
                wl_surface::{Role, WlSurface},
            },
        },
    };

    const SURFACE_ID: u32 = 20;
    const OUTPUT_OBJECT_ID: u32 = 21;
    const LOCK_SURFACE_ID: u32 = 30;

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
        client_dispatcher.drain_post_actions(client, state);
    }

    fn closed(dispatcher: &Dispatcher, client: &Client) -> bool {
        dispatcher.test_has_queued_action::<ClientManagerAction>(|action| {
            matches!(action, ClientManagerAction::Close { id } if *id == client.client_id())
        })
    }

    struct Fixture {
        state: State,
        client: Client,
        dispatcher: Dispatcher,
        output_id: OutputId,
        surface: Resolved<WlSurfaceId>,
    }

    fn fixture() -> Fixture {
        let mut state = state_for_tests();
        let output_id = state.insert_output(test_output_drm1(0, 0, 640, 480));
        let mut client = Client::new_for_tests(ClientId::from_raw(1));
        let mut dispatcher = Dispatcher::default();
        client.insert_object(
            &mut state,
            ExtSessionLockManagerV1::new(ExtSessionLockManagerV1Id::from(8)),
        );
        let mut lock = request(8, 1, 1);
        lock.id(ExtSessionLockV1Id::from(9));
        dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            ExtSessionLockManagerV1Id::from(8),
            lock,
        );
        let (surface, _) = client.insert_object(
            &mut state,
            WlSurface::new(WlSurfaceId::from(SURFACE_ID), client.client_id(), 6),
        );
        state.register_root_surface(surface);
        client.insert_object(
            &mut state,
            WlOutput::new(WlOutputId::from(OUTPUT_OBJECT_ID), output_id, 4),
        );
        let mut get = request(9, 1, 3);
        get.id(ExtSessionLockSurfaceV1Id::from(LOCK_SURFACE_ID));
        get.id(WlSurfaceId::from(SURFACE_ID));
        get.id(WlOutputId::from(OUTPUT_OBJECT_ID));
        dispatch(&mut client, &mut state, &mut dispatcher, ExtSessionLockV1Id::from(9), get);
        Fixture { state, client, dispatcher, output_id, surface }
    }

    fn ack(fixture: &mut Fixture, serial: u32) {
        let mut event = request(LOCK_SURFACE_ID, 1, 1);
        event.uint(serial);
        dispatch(
            &mut fixture.client,
            &mut fixture.state,
            &mut fixture.dispatcher,
            ExtSessionLockSurfaceV1Id::from(LOCK_SURFACE_ID),
            event,
        );
    }

    fn attach(fixture: &mut Fixture, buffer: WlBufferId) {
        let mut event = request(SURFACE_ID, 1, 3);
        event.id(buffer);
        event.int(0);
        event.int(0);
        dispatch(
            &mut fixture.client,
            &mut fixture.state,
            &mut fixture.dispatcher,
            WlSurfaceId::from(SURFACE_ID),
            event,
        );
    }

    fn commit(fixture: &mut Fixture) {
        dispatch(
            &mut fixture.client,
            &mut fixture.state,
            &mut fixture.dispatcher,
            WlSurfaceId::from(SURFACE_ID),
            request(SURFACE_ID, 6, 0),
        );
    }

    fn insert_buffer(fixture: &mut Fixture, id: u32, width: i32, height: i32) -> WlBufferId {
        let bytes = vec![0u8; (width as usize) * (height as usize) * 4];
        let buffer_id = WlBufferId::from(id);
        fixture.client.insert_object(
            &mut fixture.state,
            WlBuffer::new(NewWlBufferParams {
                id: buffer_id,
                owner_client_id: fixture.client.client_id(),
                offset: 0,
                width,
                height,
                stride: width.saturating_mul(4),
                format: &ARGB8888,
                mem: shared_memory_from_bytes(&bytes),
            })
            .unwrap(),
        );
        buffer_id
    }

    #[test]
    fn a_commit_before_the_first_ack_is_rejected() {
        let mut fixture = fixture();
        let buffer = insert_buffer(&mut fixture, 40, 640, 480);
        attach(&mut fixture, buffer);
        commit(&mut fixture);
        assert!(closed(&fixture.dispatcher, &fixture.client));
        assert!(fixture
            .surface
            .rref(&fixture.state)
            .logical_size()
            .is_none());
        assert!(!fixture
            .state
            .session_lock
            .shows_lock_surface(fixture.output_id));
    }

    #[test]
    fn a_null_buffer_after_ack_is_rejected() {
        let mut fixture = fixture();
        ack(&mut fixture, 1);
        attach(&mut fixture, WlBufferId::from(0));
        commit(&mut fixture);
        assert!(closed(&fixture.dispatcher, &fixture.client));
        assert!(!fixture
            .state
            .session_lock
            .shows_lock_surface(fixture.output_id));
    }

    #[test]
    fn a_matching_commit_maps_the_lock_surface_outside_the_workspace() {
        let mut fixture = fixture();
        assert!(fixture
            .state
            .root_surface_workspace(fixture.surface)
            .is_none());
        assert!(!fixture.state.root_surfaces().contains(&fixture.surface));
        ack(&mut fixture, 1);
        let buffer = insert_buffer(&mut fixture, 40, 640, 480);
        attach(&mut fixture, buffer);
        commit(&mut fixture);
        assert!(!closed(&fixture.dispatcher, &fixture.client));
        assert_eq!(
            fixture.surface.rref(&fixture.state).logical_size(),
            Some(SizeI { width: 640, height: 480 })
        );
        assert!(fixture
            .state
            .session_lock
            .shows_lock_surface(fixture.output_id));
        assert_eq!(fixture.surface.rref(&fixture.state).role(), Role::LockSurface);
    }

    #[test]
    fn scale_transform_and_viewport_destination_decide_the_size() {
        let mut fixture = fixture();
        ack(&mut fixture, 1);
        let mut scale = request(SURFACE_ID, 8, 1);
        scale.int(2);
        dispatch(
            &mut fixture.client,
            &mut fixture.state,
            &mut fixture.dispatcher,
            WlSurfaceId::from(SURFACE_ID),
            scale,
        );
        let buffer = insert_buffer(&mut fixture, 40, 1280, 960);
        attach(&mut fixture, buffer);
        commit(&mut fixture);
        assert_eq!(
            fixture.surface.rref(&fixture.state).logical_size(),
            Some(SizeI { width: 640, height: 480 })
        );

        let mut scale = request(SURFACE_ID, 8, 1);
        scale.int(1);
        dispatch(
            &mut fixture.client,
            &mut fixture.state,
            &mut fixture.dispatcher,
            WlSurfaceId::from(SURFACE_ID),
            scale,
        );
        let mut transform = request(SURFACE_ID, 7, 1);
        transform.int(1);
        dispatch(
            &mut fixture.client,
            &mut fixture.state,
            &mut fixture.dispatcher,
            WlSurfaceId::from(SURFACE_ID),
            transform,
        );
        let rotated = insert_buffer(&mut fixture, 41, 480, 640);
        attach(&mut fixture, rotated);
        commit(&mut fixture);
        assert!(!closed(&fixture.dispatcher, &fixture.client));
        assert_eq!(
            fixture.surface.rref(&fixture.state).logical_size(),
            Some(SizeI { width: 640, height: 480 })
        );

        fixture
            .surface
            .ref_mut(&mut fixture.state)
            .pending_viewport_mut()
            .dst = Some(SizeI { width: 320, height: 240 });
        commit(&mut fixture);
        assert!(closed(&fixture.dispatcher, &fixture.client));
    }

    #[test]
    fn the_last_ack_before_commit_selects_the_size() {
        let mut fixture = fixture();
        let lock_surface = fixture
            .client
            .resolve(ExtSessionLockSurfaceV1Id::from(LOCK_SURFACE_ID), &fixture.state)
            .unwrap();
        {
            let mut client_dispatcher = fixture.client.test_dispatcher(&mut fixture.dispatcher);
            lock_surface
                .ref_mut(&mut fixture.state)
                .configure(2, 800, 600, &mut client_dispatcher);
        }
        ack(&mut fixture, 1);
        let buffer = insert_buffer(&mut fixture, 40, 640, 480);
        attach(&mut fixture, buffer);
        commit(&mut fixture);
        assert!(fixture
            .state
            .session_lock
            .shows_lock_surface(fixture.output_id));
        ack(&mut fixture, 2);
        commit(&mut fixture);
        assert!(closed(&fixture.dispatcher, &fixture.client));
    }

    #[test]
    fn a_mapped_lock_surface_takes_input_without_entering_focus_history() {
        let mut fixture = fixture();
        let desktop = fixture
            .client
            .insert_object(
                &mut fixture.state,
                WlSurface::new(WlSurfaceId::from(21), fixture.client.client_id(), 6),
            )
            .0;
        let desktop_focus = SurfaceFocus::new(desktop);
        fixture.state.seat.remember_keyboard_focus(desktop_focus);
        fixture.state.seat.keyboard_focus = Some(desktop_focus);

        ack(&mut fixture, 1);
        let buffer = insert_buffer(&mut fixture, 40, 640, 480);
        attach(&mut fixture, buffer);
        commit(&mut fixture);

        assert_eq!(
            fixture
                .state
                .seat
                .keyboard_focus()
                .map(|focus| focus.surface),
            Some(fixture.surface)
        );
        assert_eq!(
            fixture
                .state
                .seat
                .pointer_focus
                .map(|focus| focus.target.surface),
            Some(fixture.surface)
        );
        assert_eq!(fixture.state.seat.keyboard_focus_history(), vec![desktop_focus]);

        fixture.state.seat.keyboard_focus = Some(desktop_focus);
        crate::seat::refresh_scene_focus(&mut fixture.state, &mut fixture.dispatcher);
        assert_eq!(
            fixture
                .state
                .seat
                .keyboard_focus()
                .map(|focus| focus.surface),
            Some(fixture.surface)
        );
        assert_eq!(fixture.state.seat.keyboard_focus_history(), vec![desktop_focus]);

        let group = fixture
            .state
            .workspace_group_for_output(fixture.output_id)
            .unwrap();
        let second = fixture
            .state
            .workspace_groups
            .try_get(group)
            .unwrap()
            .workspaces[1];
        assert!(!fixture.state.activate_workspace(second));
        assert!(!crate::seat::request_popup_grab(
            &mut fixture.state,
            &mut fixture.dispatcher,
            fixture.client.client_id(),
            fixture.surface,
            1,
            Resolved::new(
                fixture.client.client_id(),
                XdgPopupId::from(99),
                fixture.surface.object_id()
            ),
            fixture.surface,
        ));

        fixture.state.seat.keyboard_focus = None;
        crate::input::focus::focus_after_client_disconnect(
            &mut fixture.state,
            &mut fixture.dispatcher,
        );
        assert_eq!(
            fixture
                .state
                .seat
                .keyboard_focus()
                .map(|focus| focus.surface),
            Some(fixture.surface)
        );

        dispatch(
            &mut fixture.client,
            &mut fixture.state,
            &mut fixture.dispatcher,
            ExtSessionLockSurfaceV1Id::from(LOCK_SURFACE_ID),
            request(LOCK_SURFACE_ID, 0, 0),
        );
        assert!(fixture.state.seat.keyboard_focus().is_none());
        assert!(fixture.state.seat.pointer_focus.is_none());
        assert_eq!(fixture.state.seat.keyboard_focus_history(), vec![desktop_focus]);
    }

    #[test]
    fn destroying_a_mapped_lock_surface_exposes_the_fallback_and_keeps_the_role() {
        let mut fixture = fixture();
        ack(&mut fixture, 1);
        let buffer = insert_buffer(&mut fixture, 40, 640, 480);
        attach(&mut fixture, buffer);
        commit(&mut fixture);
        dispatch(
            &mut fixture.client,
            &mut fixture.state,
            &mut fixture.dispatcher,
            ExtSessionLockSurfaceV1Id::from(LOCK_SURFACE_ID),
            request(LOCK_SURFACE_ID, 0, 0),
        );
        assert!(!fixture
            .state
            .session_lock
            .shows_lock_surface(fixture.output_id));
        assert_eq!(fixture.state.session_lock.phase(), SessionLockPhase::Locking);
        assert_eq!(fixture.surface.rref(&fixture.state).role(), Role::LockSurface);
        assert!(!fixture.state.root_surfaces().contains(&fixture.surface));
    }

    #[test]
    fn a_logical_size_change_configures_the_lock_surface_again() {
        let mut fixture = fixture();
        let lock_surface = fixture
            .client
            .resolve(ExtSessionLockSurfaceV1Id::from(LOCK_SURFACE_ID), &fixture.state)
            .unwrap();
        assert_eq!(lock_surface.rref(&fixture.state).configured_size(), Some((640, 480)));
        {
            let output = fixture
                .state
                .outputs
                .try_get_mut(fixture.output_id)
                .unwrap();
            output.current_mode.width = 800;
            output.current_mode.height = 600;
        }
        crate::wayland_objects::ext_session_lock_v1::on_output_state_changed(
            &mut fixture.state,
            &mut fixture.dispatcher,
            fixture.output_id,
        );
        fixture
            .client
            .process(
                &mut fixture.state,
                ClientAction::ReconfigureLockOutput { output_id: fixture.output_id },
                &mut fixture.dispatcher,
            )
            .unwrap();
        assert_eq!(lock_surface.rref(&fixture.state).configured_size(), Some((800, 600)));
        assert_eq!(fixture.state.session_lock.phase(), SessionLockPhase::Locking);
    }

    #[test]
    fn disabling_an_output_drops_its_lock_surface_without_confirming() {
        let mut fixture = fixture();
        fixture
            .state
            .outputs
            .try_get_mut(fixture.output_id)
            .unwrap()
            .enabled = false;
        crate::wayland_objects::ext_session_lock_v1::on_output_state_changed(
            &mut fixture.state,
            &mut fixture.dispatcher,
            fixture.output_id,
        );
        assert!(fixture
            .client
            .resolve(ExtSessionLockSurfaceV1Id::from(LOCK_SURFACE_ID), &fixture.state)
            .unwrap()
            .rref(&fixture.state)
            .is_inert());
        assert_eq!(fixture.state.session_lock.phase(), SessionLockPhase::Locking);
        assert!(!fixture.state.session_lock.locked_sent());
    }

    #[test]
    fn removing_the_output_makes_the_lock_surface_inert() {
        let mut fixture = fixture();
        assert!(fixture
            .state
            .remove_output(fixture.output_id, &mut fixture.dispatcher)
            .is_some());
        assert!(fixture
            .client
            .resolve(ExtSessionLockSurfaceV1Id::from(LOCK_SURFACE_ID), &fixture.state)
            .unwrap()
            .rref(&fixture.state)
            .is_inert());
        assert!(!fixture
            .state
            .session_lock
            .shows_lock_surface(fixture.output_id));
    }

    #[test]
    fn a_commit_after_the_lock_disappears_is_ignored() {
        let mut fixture = fixture();
        dispatch(
            &mut fixture.client,
            &mut fixture.state,
            &mut fixture.dispatcher,
            ExtSessionLockV1Id::from(9),
            request(9, 0, 0),
        );
        assert_eq!(fixture.state.session_lock.phase(), SessionLockPhase::Unlocked);
        ack(&mut fixture, 1);
        let buffer = insert_buffer(&mut fixture, 40, 640, 480);
        attach(&mut fixture, buffer);
        commit(&mut fixture);
        assert!(!closed(&fixture.dispatcher, &fixture.client));
        assert!(!fixture
            .state
            .session_lock
            .shows_lock_surface(fixture.output_id));
    }

    #[test]
    fn only_a_mapped_lock_surface_joins_the_protected_plan() {
        let mut fixture = fixture();
        assert!(fixture
            .state
            .session_lock
            .paint_order(&fixture.state, fixture.output_id)
            .is_empty());
        ack(&mut fixture, 1);
        let buffer = insert_buffer(&mut fixture, 40, 640, 480);
        attach(&mut fixture, buffer);
        commit(&mut fixture);
        assert_eq!(
            fixture
                .state
                .session_lock
                .paint_order(&fixture.state, fixture.output_id),
            vec![fixture.surface]
        );
    }

    #[test]
    fn the_protected_plan_keeps_the_desktop_hidden_across_resume() {
        let mut fixture = fixture();
        let desktop = fixture
            .client
            .insert_object(
                &mut fixture.state,
                WlSurface::new(WlSurfaceId::from(50), fixture.client.client_id(), 6),
            )
            .0;
        fixture.state.register_root_surface(desktop);
        desktop.ref_mut(&mut fixture.state).test_map(
            crate::state::ObjectId::from(41usize),
            0,
            0,
            100,
            80,
        );

        ack(&mut fixture, 1);
        let buffer = insert_buffer(&mut fixture, 40, 640, 480);
        attach(&mut fixture, buffer);
        commit(&mut fixture);

        assert_eq!(
            fixture
                .state
                .session_lock
                .paint_order(&fixture.state, fixture.output_id),
            vec![fixture.surface]
        );
        fixture.state.session_lock.note_session_resumed();
        assert_eq!(fixture.state.session_lock.blank(), Some(crate::session_lock::LockBlank::Black));
        assert_eq!(fixture.state.session_lock.phase(), SessionLockPhase::Locking);
        assert_eq!(
            fixture
                .state
                .session_lock
                .paint_order(&fixture.state, fixture.output_id),
            vec![fixture.surface]
        );
    }

    #[test]
    fn locked_input_cannot_leave_the_locker() {
        let mut fixture = fixture();
        ack(&mut fixture, 1);
        let buffer = insert_buffer(&mut fixture, 40, 640, 480);
        attach(&mut fixture, buffer);
        commit(&mut fixture);

        let mut other = Client::new_for_tests(ClientId::from_raw(2));
        let other_surface = other
            .insert_object(
                &mut fixture.state,
                WlSurface::new(WlSurfaceId::from(31), other.client_id(), 6),
            )
            .0;
        other_surface.ref_mut(&mut fixture.state).test_map(
            crate::state::ObjectId::from(42usize),
            0,
            0,
            40,
            40,
        );
        crate::input::focus::set_keyboard_focus_from_surface(
            &mut fixture.state,
            &mut fixture.dispatcher,
            Some(crate::input::SurfaceFocus::new(other_surface)),
        );
        assert_eq!(
            fixture
                .state
                .seat
                .keyboard_focus()
                .map(|focus| focus.surface),
            Some(fixture.surface)
        );
        assert!(!crate::seat::request_toplevel_move(
            &mut fixture.state,
            &mut fixture.dispatcher,
            other.client_id(),
            other_surface,
            1,
            other_surface,
            Resolved::new(other.client_id(), XdgToplevelId::from(50), other_surface.object_id(),),
            crate::wayland_objects::xdg_surface::xdg_toplevel::ToplevelDecorationMode::ClientSide,
            crate::wayland_objects::xdg_surface::xdg_toplevel::InteractiveSizeConstraints::default(
            ),
            false,
            false,
        ));

        crate::seat::handle_keyboard_key(&mut fixture.state, &mut fixture.dispatcher, 30, 1, 0);
        let mut other_actions = 0;
        fixture
            .dispatcher
            .test_matching_actions_for::<crate::clients::ClientAction, _>(
                other.client_id(),
                |_| other_actions += 1,
            );
        assert_eq!(other_actions, 0);

        let mut locker_offers = 0;
        fixture
            .dispatcher
            .test_matching_actions_for::<crate::clients::ClientAction, _>(
                fixture.client.client_id(),
                |action| {
                    if matches!(action, crate::clients::ClientAction::RefreshDataDeviceSelection) {
                        locker_offers += 1;
                    }
                },
            );
        assert_eq!(locker_offers, 1);
        assert!(fixture.state.seat.selection_source().is_none());
    }
}
