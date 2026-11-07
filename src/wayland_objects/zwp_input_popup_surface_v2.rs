//! `zwp_input_popup_surface_v2`.
//!
//! The surface is placed next to the text cursor. `text_input_rectangle` reports
//! that cursor in popup-local coordinates. The popup is not a pointer target and
//! never takes keyboard focus.

use crate::{
    clients::{Client, ClientDispatcher, EventWriter, RawRequest, Resolved},
    object::{Object, Parser, RequestResult},
    protocol_generated::{
        zwp_input_popup_surface_v2, WaylandId, WlSurfaceId, ZwpInputPopupSurfaceV2Id,
    },
    region::RectI,
    state::{GrabbedObject, State},
    wayland_objects::wl_display::WlDisplay,
};

#[derive(Debug)]
pub struct ZwpInputPopupSurfaceV2 {
    id: ZwpInputPopupSurfaceV2Id,
    surface: Resolved<WlSurfaceId>,
    /// Set when the input method that created this popup is gone.
    inert: bool,
}

impl ZwpInputPopupSurfaceV2 {
    pub(crate) fn new(
        id: ZwpInputPopupSurfaceV2Id,
        surface: Resolved<WlSurfaceId>,
        inert: bool,
    ) -> Self {
        Self { id, surface, inert }
    }

    pub(crate) fn surface(&self) -> Resolved<WlSurfaceId> {
        self.surface
    }

    pub(crate) fn set_inert(&mut self) {
        self.inert = true;
    }

    fn dismiss(
        &self,
        client: &Client,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
        disconnecting: bool,
    ) {
        let Some(popup) = client.resolve(self.id, state) else {
            return;
        };
        let record = state.seat.ime.take_popup(popup);
        if let Some(mut surface) = self.surface.try_ref_mut(state) {
            surface.clear_input_popup_role();
        }
        if let Some(global) = record.and_then(|record| record.global) {
            crate::drm::request_outputs_for_rects(state, [global], dispatcher);
        }
        if !disconnecting {
            dispatcher.dispatch_to(
                client.client_id(),
                crate::clients::ClientAction::RefreshSurfaceOutputs,
            );
        }
    }
}

impl Object for ZwpInputPopupSurfaceV2 {
    fn id(&self) -> WaylandId {
        self.id.into()
    }

    fn on(
        self: GrabbedObject<Self>,
        client: &mut Client,
        state: &mut State,
        req: RawRequest,
        dispatcher: &mut ClientDispatcher,
    ) -> RequestResult {
        match client.parse::<Self>(req, dispatcher)? {
            zwp_input_popup_surface_v2::Request::Destroy => {
                self.dismiss(client, state, dispatcher, false);
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
        self.dismiss(client, state, dispatcher, true);
    }
}

pub(crate) fn deliver_input_popup_event(
    state: &mut State,
    dispatcher: &mut ClientDispatcher,
    popup: Resolved<ZwpInputPopupSurfaceV2Id>,
    cursor: RectI,
) {
    let Some(popup) = popup.try_ref(state) else {
        return;
    };
    if popup.inert {
        return;
    }
    popup.send_event(
        zwp_input_popup_surface_v2::Event::TextInputRectangle {
            x: cursor.x,
            y: cursor.y,
            width: cursor.width,
            height: cursor.height,
        },
        dispatcher,
    );
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use crate::{
        clients::{Arguments, Client, ClientAction, ClientId, RawRequest, Resolved},
        input::{ime, SurfaceFocus},
        io_uring::model::IoUringAction,
        object::{Object, RequestResult},
        protocol_generated::{
            zwp_input_method_v2, WlSeatId, WlSurfaceId, ZwpInputMethodManagerV2Id,
            ZwpInputMethodV2Id, ZwpInputPopupSurfaceV2Id, ZwpTextInputManagerV3Id,
            ZwpTextInputV3Id,
        },
        region::RectI,
        runner::{Dispatcher, Model},
        session_lock::SessionLockPhase,
        state::{BackendOutputRef, DrmId, ObjectId, State},
        test_support::{
            insert_root_surface, resolved_wl_surface, state_for_tests, test_output_with_crtc_name,
        },
        wayland_objects::{
            wl_seat::WlSeat, wl_surface::WlSurface,
            zwp_input_method_manager_v2::ZwpInputMethodManagerV2,
            zwp_input_method_v2::ZwpInputMethodV2,
            zwp_text_input_manager_v3::ZwpTextInputManagerV3, zwp_text_input_v3::ZwpTextInputV3,
        },
    };

    use super::ZwpInputPopupSurfaceV2;

    fn output(state: &mut State, width: i32, height: i32) {
        let backend = BackendOutputRef::new(DrmId::from(0usize));
        state.insert_output(test_output_with_crtc_name(backend, 1, 0, 0, width, height));
    }

    fn map_surface(
        state: &mut State,
        id: u32,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
    ) -> Resolved<WlSurfaceId> {
        let object_id = insert_root_surface(state, id);
        let surface = resolved_wl_surface(state, object_id);
        surface
            .ref_mut(state)
            .test_map(ObjectId::from(id as usize), x, y, width, height);
        surface
    }

    struct Placement {
        state: State,
        focus: Resolved<WlSurfaceId>,
        popup: Resolved<ZwpInputPopupSurfaceV2Id>,
    }

    fn placement_fixture(
        focus_at: (i32, i32),
        popup_size: (i32, i32),
        output_size: (i32, i32),
        cursor: Option<RectI>,
    ) -> Placement {
        let mut state = state_for_tests();
        output(&mut state, output_size.0, output_size.1);
        let focus = map_surface(&mut state, 5, focus_at.0, focus_at.1, 80, 40);
        state.seat.keyboard_focus = Some(SurfaceFocus::new(focus));
        if cursor.is_some() {
            let (_, object_id) =
                state.insert_object(ZwpTextInputV3::new(ZwpTextInputV3Id::from(9), 1));
            let text_input =
                Resolved::new(ClientId::from_raw(1), ZwpTextInputV3Id::from(9), object_id);
            {
                let mut text_input = text_input.ref_mut(&mut state);
                text_input.set_entered(Some(focus));
                text_input.set_cursor_rect_for_tests(cursor);
            }
            state.seat.ime.note_commit(
                text_input,
                ime::TextInputCommitKind::Enable,
                Some(focus),
                Some(focus),
            );
        }
        let popup_object_id = insert_root_surface(&mut state, 7);
        let popup_surface = resolved_wl_surface(&mut state, popup_object_id);
        popup_surface
            .ref_mut(&mut state)
            .set_input_popup_role()
            .unwrap();
        popup_surface.ref_mut(&mut state).test_map(
            ObjectId::from(7usize),
            0,
            0,
            popup_size.0,
            popup_size.1,
        );
        let (_, object_id) = state.insert_object(ZwpInputPopupSurfaceV2::new(
            ZwpInputPopupSurfaceV2Id::from(8),
            popup_surface,
            false,
        ));
        let popup =
            Resolved::new(ClientId::from_raw(1), ZwpInputPopupSurfaceV2Id::from(8), object_id);
        popup_surface.ref_mut(&mut state).assign_input_popup(popup);
        Placement { state, focus, popup }
    }

    #[test]
    fn the_popup_sits_below_the_cursor_rectangle() {
        let cursor = RectI::new(10, 20, 8, 16).unwrap();
        let fixture = placement_fixture((100, 100), (40, 30), (1920, 1080), Some(cursor));
        let (global, cursor_in_popup) =
            ime::popup_placement(&fixture.state, fixture.popup).expect("placement");
        assert_eq!(global, RectI { x: 110, y: 136, width: 40, height: 30 });
        assert_eq!(cursor_in_popup, RectI { x: 0, y: -16, width: 8, height: 16 });
    }

    #[test]
    fn the_popup_flips_above_the_cursor_when_it_would_leave_the_output() {
        let cursor = RectI::new(0, 0, 10, 20).unwrap();
        let fixture = placement_fixture((100, 1000), (40, 100), (1920, 1080), Some(cursor));
        let (global, cursor_in_popup) =
            ime::popup_placement(&fixture.state, fixture.popup).expect("placement");
        assert_eq!(global, RectI { x: 100, y: 900, width: 40, height: 100 });
        assert_eq!(cursor_in_popup, RectI { x: 0, y: 100, width: 10, height: 20 });
    }

    #[test]
    fn the_popup_is_clamped_inside_the_output() {
        let cursor = RectI::new(0, 0, 10, 10).unwrap();
        let fixture = placement_fixture((1900, 100), (200, 40), (1920, 1080), Some(cursor));
        let (global, cursor_in_popup) =
            ime::popup_placement(&fixture.state, fixture.popup).expect("placement");
        assert_eq!(global, RectI { x: 1720, y: 110, width: 200, height: 40 });
        assert_eq!(cursor_in_popup, RectI { x: 180, y: -10, width: 10, height: 10 });
    }

    #[test]
    fn a_missing_cursor_rectangle_anchors_at_the_surface_origin() {
        let fixture = placement_fixture((50, 60), (20, 20), (800, 600), None);
        let (global, cursor_in_popup) =
            ime::popup_placement(&fixture.state, fixture.popup).expect("placement");
        assert_eq!(global, RectI { x: 50, y: 60, width: 20, height: 20 });
        assert_eq!(cursor_in_popup, RectI { x: 0, y: 0, width: 0, height: 0 });
    }

    #[test]
    fn a_subsurface_offset_moves_the_anchor() {
        let mut fixture = placement_fixture((100, 100), (20, 20), (800, 600), None);
        let (_, child_object_id) = fixture.state.insert_object(WlSurface::new(
            WlSurfaceId::from(12),
            ClientId::from_raw(1),
            6,
        ));
        let child = Resolved::new(ClientId::from_raw(1), WlSurfaceId::from(12), child_object_id);
        child
            .ref_mut(&mut fixture.state)
            .test_mark_subsurface_at(fixture.focus, (5, 7));
        fixture.state.seat.keyboard_focus = Some(SurfaceFocus::new(child));
        let (global, _) = ime::popup_placement(&fixture.state, fixture.popup).expect("placement");
        assert_eq!(global.x, 105);
        assert_eq!(global.y, 107);
    }

    #[test]
    fn visibility_needs_a_buffer_an_input_method_an_active_text_input_and_an_unlocked_session() {
        let mut fixture =
            placement_fixture((0, 0), (10, 10), (100, 100), Some(RectI::new(0, 0, 1, 1).unwrap()));
        let popup_surface = fixture.popup.ref_mut(&mut fixture.state).surface();
        assert!(!ime::input_popup_visible(&fixture.state, popup_surface));

        let (_, object_id) = fixture.state.insert_object(ZwpInputMethodV2::new(
            ZwpInputMethodV2Id::from(40),
            1,
            false,
        ));
        let input_method =
            Resolved::new(ClientId::from_raw(1), ZwpInputMethodV2Id::from(40), object_id);
        fixture.state.seat.ime.set_input_method(input_method);
        assert!(ime::input_popup_visible(&fixture.state, popup_surface));

        popup_surface
            .ref_mut(&mut fixture.state)
            .test_clear_committed_buffer();
        assert!(!ime::input_popup_visible(&fixture.state, popup_surface));
        popup_surface
            .ref_mut(&mut fixture.state)
            .test_map(ObjectId::from(7usize), 0, 0, 10, 10);

        fixture
            .state
            .session_lock
            .set_phase_for_tests(SessionLockPhase::Locked);
        assert!(!ime::input_popup_visible(&fixture.state, popup_surface));
        fixture
            .state
            .session_lock
            .set_phase_for_tests(SessionLockPhase::Unlocked);
        assert!(ime::input_popup_visible(&fixture.state, popup_surface));

        let active = fixture.state.seat.ime.active_text_input().unwrap();
        fixture.state.seat.ime.note_commit(
            active,
            ime::TextInputCommitKind::Disable,
            Some(fixture.focus),
            Some(fixture.focus),
        );
        assert!(!ime::input_popup_visible(&fixture.state, popup_surface));
    }

    struct Fixture {
        state: State,
        dispatcher: Dispatcher,
        app: Client,
        ime: Client,
        surface: Resolved<WlSurfaceId>,
    }

    fn new_fixture() -> Fixture {
        let mut state = state_for_tests();
        output(&mut state, 800, 600);
        let mut dispatcher = Dispatcher::default();
        let mut app = Client::new_for_tests(ClientId::from_raw(1));
        let surface = {
            let (surface, _) = app.insert_object(
                &mut state,
                WlSurface::new(WlSurfaceId::from(5), app.client_id(), 6),
            );
            state.register_root_surface(surface);
            surface
                .ref_mut(&mut state)
                .test_map(ObjectId::from(5usize), 100, 200, 80, 40);
            surface
        };
        let _ = app
            .insert_object(&mut state, WlSeat::new(WlSeatId::from(4)))
            .0;
        let _ = app
            .insert_object(
                &mut state,
                ZwpTextInputManagerV3::new(ZwpTextInputManagerV3Id::from(3), 1),
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
        let mut fixture = Fixture { state, dispatcher, app, ime, surface };
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
        let result = state
            .objects
            .grab(object)
            .on(client, state, request, &mut client_dispatcher);
        client_dispatcher.drain_post_actions(client, state);
        result
    }

    fn drain(client: &mut Client, state: &mut State, dispatcher: &mut Dispatcher) {
        let actions =
            dispatcher.test_take_actions_for::<ClientAction, ClientId>(client.client_id());
        for action in actions {
            match &action {
                ClientAction::SeatEvent { .. }
                | ClientAction::InputMethodEvent { .. }
                | ClientAction::InputPopupEvent { .. }
                | ClientAction::TextInputEvent { .. }
                | ClientAction::RefreshSurfaceOutputs => {
                    client.process(state, action, dispatcher).unwrap();
                }
                _ => {}
            }
        }
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

    fn rectangles(dispatcher: &Dispatcher, object: u32) -> Vec<(i32, i32, i32, i32)> {
        events(dispatcher, object)
            .into_iter()
            .filter_map(|(opcode, args)| {
                if opcode == 0 && args.len() >= 4 {
                    Some((args[0] as i32, args[1] as i32, args[2] as i32, args[3] as i32))
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

    fn popup_surface(fixture: &mut Fixture, id: u32) -> Resolved<WlSurfaceId> {
        let (surface, _) = fixture.ime.insert_object(
            &mut fixture.state,
            WlSurface::new(WlSurfaceId::from(id), fixture.ime.client_id(), 6),
        );
        surface
    }

    fn role_error(fixture: &Fixture) -> (u32, u32) {
        let (_, args) = events(&fixture.dispatcher, 1)
            .into_iter()
            .find(|(_, args)| args.len() >= 2)
            .expect("display error");
        (args[0], args[1])
    }

    #[test]
    fn a_surface_with_a_role_or_a_buffer_is_rejected() {
        let mut fixture = new_fixture();
        let cursor = popup_surface(&mut fixture, 11);
        {
            let mut surface = cursor.ref_mut(&mut fixture.state);
            let mut client_dispatcher = fixture.ime.test_dispatcher(&mut fixture.dispatcher);
            surface.set_cursor_role(&mut client_dispatcher).unwrap();
        }
        call(&mut fixture.ime, &mut fixture.state, &mut fixture.dispatcher, 40, 4, &[70, 11])
            .unwrap();
        assert!(closed(&fixture.dispatcher, fixture.ime.client_id()));
        assert_eq!(role_error(&fixture), (40, zwp_input_method_v2::Error::Role as u32));
        assert!(cursor.rref(&fixture.state).has_cursor_role());

        let mut fixture = new_fixture();
        let buffered = popup_surface(&mut fixture, 12);
        buffered
            .ref_mut(&mut fixture.state)
            .test_map(ObjectId::from(12usize), 0, 0, 4, 4);
        call(&mut fixture.ime, &mut fixture.state, &mut fixture.dispatcher, 40, 4, &[71, 12])
            .unwrap();
        assert!(closed(&fixture.dispatcher, fixture.ime.client_id()));
        assert_eq!(role_error(&fixture), (40, zwp_input_method_v2::Error::Role as u32));
        assert_eq!(
            buffered.rref(&fixture.state).role(),
            crate::wayland_objects::wl_surface::Role::None
        );
    }

    #[test]
    fn text_input_rectangle_follows_the_cursor_and_hides_when_focus_leaves() {
        let mut fixture = new_fixture();
        let popup = popup_surface(&mut fixture, 7);
        call(&mut fixture.ime, &mut fixture.state, &mut fixture.dispatcher, 40, 4, &[70, 7])
            .unwrap();
        popup
            .ref_mut(&mut fixture.state)
            .test_map(ObjectId::from(7usize), 0, 0, 40, 30);
        call(&mut fixture.app, &mut fixture.state, &mut fixture.dispatcher, 3, 1, &[9, 4]).unwrap();
        call(&mut fixture.app, &mut fixture.state, &mut fixture.dispatcher, 9, 1, &[]).unwrap();
        call(&mut fixture.app, &mut fixture.state, &mut fixture.dispatcher, 9, 6, &[10, 20, 8, 16])
            .unwrap();
        call(&mut fixture.app, &mut fixture.state, &mut fixture.dispatcher, 9, 7, &[]).unwrap();
        drain(&mut fixture.ime, &mut fixture.state, &mut fixture.dispatcher);
        assert_eq!(rectangles(&fixture.dispatcher, 70), vec![(0, -16, 8, 16)]);
        assert_eq!(
            fixture.state.seat.ime.popup_global(popup),
            Some(RectI { x: 110, y: 236, width: 40, height: 30 })
        );
        assert_eq!(
            popup
                .rref(&fixture.state)
                .input_popup()
                .map(|popup| u32::from(popup.wayland_id())),
            Some(70)
        );

        let sent = rectangles(&fixture.dispatcher, 70).len();
        call(&mut fixture.app, &mut fixture.state, &mut fixture.dispatcher, 9, 7, &[]).unwrap();
        drain(&mut fixture.ime, &mut fixture.state, &mut fixture.dispatcher);
        assert_eq!(rectangles(&fixture.dispatcher, 70).len(), sent);

        call(&mut fixture.app, &mut fixture.state, &mut fixture.dispatcher, 9, 6, &[0, 0, 4, 4])
            .unwrap();
        call(&mut fixture.app, &mut fixture.state, &mut fixture.dispatcher, 9, 7, &[]).unwrap();
        drain(&mut fixture.ime, &mut fixture.state, &mut fixture.dispatcher);
        assert_eq!(rectangles(&fixture.dispatcher, 70).last().copied(), Some((0, -4, 4, 4)));

        crate::input::set_keyboard_focus(&mut fixture.state, &mut fixture.dispatcher, None);
        assert_eq!(fixture.state.seat.ime.popup_global(popup), None);
    }
}
