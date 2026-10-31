use crate::{
    clients::{Client, ClientDispatcher, RawRequest},
    drm::{request_active_output, request_outputs_for_rect},
    globals::Global,
    object::{Object, Parser, RequestResult},
    protocol_generated::{
        wl_display, xdg_activation_v1, Constants, NewId, WaylandId, XdgActivationV1Id,
    },
    seat::{resolve_activation_target, set_keyboard_focus_from_surface},
    state::{GrabbedObject, State},
    wayland_objects::{wl_display::WlDisplay, xdg_activation_token_v1::XdgActivationTokenV1},
};

#[derive(Debug)]
pub struct XdgActivationV1Global;

impl XdgActivationV1Global {
    pub fn new() -> Box<dyn Global> {
        Box::new(Self {})
    }
}

impl Global for XdgActivationV1Global {
    fn interface(&self) -> (&'static str, u32) {
        (XdgActivationV1::INTERFACE_NAME, XdgActivationV1::INTERFACE_VERSION)
    }
    fn bind(
        &mut self,
        client: &mut Client,
        state: &mut State,
        id: NewId,
        _version: u32,
        dispatcher: &mut ClientDispatcher,
    ) {
        let id = XdgActivationV1Id::from(id);
        client.insert_object(state, XdgActivationV1 { id });
    }
}

#[derive(Debug)]
pub struct XdgActivationV1 {
    id: XdgActivationV1Id,
}

impl XdgActivationV1 {
    pub fn new(id: XdgActivationV1Id) -> Self {
        Self { id }
    }
}

impl Object for XdgActivationV1 {
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
        use xdg_activation_v1::Request::*;

        match client.parse::<Self>(req, dispatcher)? {
            Destroy => {
                client.delete_id(&WlDisplay::default(), state, dispatcher, self.id.into());
            }
            GetActivationToken { id } => {
                client.insert_object(state, XdgActivationTokenV1::new(id));
            }
            Activate { token, surface } => {
                let surface = client.resolve_or_disconnect(surface, state, dispatcher)?;
                let Some(_) = state.activation_tokens.consume(&token) else {
                    log::debug!(
                        "ignoring activation request with unknown or expired token from client {:?}",
                        client.client_id()
                    );
                    return Ok(());
                };
                let Some(focus) = resolve_activation_target(state, surface) else {
                    log::debug!(
                        "ignoring activation request for non-activatable surface {:?}",
                        surface
                    );
                    return Ok(());
                };
                if !state.raise_root_surface(focus.surface) {
                    return Ok(());
                }
                set_keyboard_focus_from_surface(state, dispatcher, Some(focus));
                if let Some(bounds) = state
                    .root_surface_for_surface(focus.surface)
                    .and_then(|root_surface| state.root_surface_surface_bounds_global(root_surface))
                {
                    request_outputs_for_rect(state, bounds, dispatcher);
                } else {
                    request_active_output(state, dispatcher);
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use {
        super::{XdgActivationV1, XdgActivationV1Global},
        crate::{
            clients::{Client, ClientId, RawEvent},
            object::{Object, RequestResult},
            protocol_generated::{
                Constants, WlSeatId, WlSurfaceId, XdgActivationTokenV1Id, XdgActivationV1Id,
            },
            runner::Dispatcher,
            seat::{handle_keyboard_key, set_keyboard_focus_from_surface},
            state::{GrabbedObject, ObjectId, State},
            test_support::{focus_for_surface, state_for_tests, test_client_and_dispatcher},
            wayland_objects::{wl_seat::WlSeat, wl_surface::WlSurface},
        },
    };

    fn insert_mapped_surface(
        state: &mut State,
        client: &mut Client,
        surface_id: WlSurfaceId,
        buffer_object_id: usize,
    ) -> ObjectId {
        let (surface_resolved, _) =
            client.insert_object(state, WlSurface::new(surface_id, client.client_id(), 6));
        let object_id = surface_resolved.object_id();
        state.register_root_surface(surface_resolved);
        state
            .objects
            .get_mut(object_id)
            .downcast_mut::<WlSurface>()
            .unwrap()
            .test_map(ObjectId::from(buffer_object_id), 0, 0, 100, 100);
        object_id
    }

    fn insert_manager(
        state: &mut State,
        client: &mut Client,
        manager_id: XdgActivationV1Id,
    ) -> ObjectId {
        let (manager_resolved, _) = client.insert_object(state, XdgActivationV1::new(manager_id));
        manager_resolved.object_id()
    }

    fn insert_seat(state: &mut State, client: &mut Client, seat_id: WlSeatId) {
        client.insert_object(state, WlSeat::new(seat_id));
    }

    fn get_activation_token_request(
        manager_id: XdgActivationV1Id,
        token_id: XdgActivationTokenV1Id,
    ) -> crate::clients::RawRequest<'static> {
        let mut event = RawEvent::new(manager_id.into(), 1, 1);
        event.id(token_id);
        (&*event.into_bytes()).try_into().unwrap()
    }

    fn activate_request(
        manager_id: XdgActivationV1Id,
        token: &str,
        surface_id: WlSurfaceId,
    ) -> crate::clients::RawRequest<'static> {
        let mut event = RawEvent::new(manager_id.into(), 2, 2);
        event.string(token.to_string());
        event.id(surface_id);
        (&*event.into_bytes()).try_into().unwrap()
    }

    fn set_serial_request(
        token_id: XdgActivationTokenV1Id,
        serial: u32,
        seat_id: WlSeatId,
    ) -> crate::clients::RawRequest<'static> {
        let mut event = RawEvent::new(token_id.into(), 0, 2);
        event.uint(serial);
        event.id(seat_id);
        (&*event.into_bytes()).try_into().unwrap()
    }

    fn set_surface_request(
        token_id: XdgActivationTokenV1Id,
        surface_id: WlSurfaceId,
    ) -> crate::clients::RawRequest<'static> {
        let mut event = RawEvent::new(token_id.into(), 2, 1);
        event.id(surface_id);
        (&*event.into_bytes()).try_into().unwrap()
    }

    fn commit_request(token_id: XdgActivationTokenV1Id) -> crate::clients::RawRequest<'static> {
        let event = RawEvent::new(token_id.into(), 3, 0);
        (&*event.into_bytes()).try_into().unwrap()
    }

    #[test]
    fn valid_token_commit_and_activate_raises_and_focuses_target() {
        let mut state = state_for_tests();
        let (mut client, mut dispatcher) = test_client_and_dispatcher(1);
        let requester_surface_id = WlSurfaceId::from(10u32);
        let target_surface_id = WlSurfaceId::from(11u32);
        let target_object_id =
            insert_mapped_surface(&mut state, &mut client, target_surface_id, 101);
        let requester_object_id =
            insert_mapped_surface(&mut state, &mut client, requester_surface_id, 100);
        let manager_id = XdgActivationV1Id::from(20u32);
        let token_id = XdgActivationTokenV1Id::from(21u32);
        let seat_id = WlSeatId::from(22u32);

        insert_manager(&mut state, &mut client, manager_id);
        insert_seat(&mut state, &mut client, seat_id);
        {
            let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);
            set_keyboard_focus_from_surface(
                &mut state,
                &mut client_dispatcher,
                Some(focus_for_surface(
                    client.client_id(),
                    requester_surface_id,
                    requester_object_id,
                )),
            );
            handle_keyboard_key(&mut state, &mut client_dispatcher, 30, 1, 100);
        }
        let serial = state.seat.last_keyboard_trigger_serial().unwrap();

        {
            let mut manager = state
                .objects
                .grab_object::<XdgActivationV1>(client.object_id(manager_id));
            let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);
            manager.on(
                &mut client,
                &mut state,
                get_activation_token_request(manager_id, token_id),
                &mut client_dispatcher,
            );
        }
        {
            let token = state
                .objects
                .grab_object::<crate::wayland_objects::xdg_activation_token_v1::XdgActivationTokenV1>(
                    client.object_id(token_id),
                );
            let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);
            token.on(
                &mut client,
                &mut state,
                set_surface_request(token_id, requester_surface_id),
                &mut client_dispatcher,
            );
        }
        {
            let token = state
                .objects
                .grab_object::<crate::wayland_objects::xdg_activation_token_v1::XdgActivationTokenV1>(
                    client.object_id(token_id),
                );
            let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);
            token.on(
                &mut client,
                &mut state,
                set_serial_request(token_id, serial, seat_id),
                &mut client_dispatcher,
            );
        }
        {
            let token = state
                .objects
                .grab_object::<crate::wayland_objects::xdg_activation_token_v1::XdgActivationTokenV1>(
                    client.object_id(token_id),
                );
            let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);
            token.on(&mut client, &mut state, commit_request(token_id), &mut client_dispatcher);
        }

        let token = state.activation_tokens.first_token().unwrap();
        {
            let mut manager = state
                .objects
                .grab_object::<XdgActivationV1>(client.object_id(manager_id));
            let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);
            manager.on(
                &mut client,
                &mut state,
                activate_request(manager_id, &token, target_surface_id),
                &mut client_dispatcher,
            );
        }

        assert_eq!(state.activation_tokens.tracked_count(), 0);
        assert_eq!(
            state
                .root_surfaces()
                .into_iter()
                .map(|surface| surface.object_id())
                .collect::<Vec<_>>(),
            vec![requester_object_id, target_object_id]
        );
        assert_eq!(
            state.seat.keyboard_focus(),
            Some(focus_for_surface(client.client_id(), target_surface_id, target_object_id,))
        );
    }

    #[test]
    fn invalid_serial_commit_returns_untracked_token() {
        let mut state = state_for_tests();
        let (mut client, mut dispatcher) = test_client_and_dispatcher(2);
        let surface_id = WlSurfaceId::from(30u32);
        let manager_id = XdgActivationV1Id::from(31u32);
        let token_id = XdgActivationTokenV1Id::from(32u32);
        let seat_id = WlSeatId::from(33u32);

        insert_mapped_surface(&mut state, &mut client, surface_id, 200);
        insert_manager(&mut state, &mut client, manager_id);
        insert_seat(&mut state, &mut client, seat_id);
        {
            let mut manager = state
                .objects
                .grab_object::<XdgActivationV1>(client.object_id(manager_id));
            let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);
            manager.on(
                &mut client,
                &mut state,
                get_activation_token_request(manager_id, token_id),
                &mut client_dispatcher,
            );
        }
        {
            let token = state
                .objects
                .grab_object::<crate::wayland_objects::xdg_activation_token_v1::XdgActivationTokenV1>(
                    client.object_id(token_id),
                );
            let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);
            token.on(
                &mut client,
                &mut state,
                set_surface_request(token_id, surface_id),
                &mut client_dispatcher,
            );
        }
        {
            let token = state
                .objects
                .grab_object::<crate::wayland_objects::xdg_activation_token_v1::XdgActivationTokenV1>(
                    client.object_id(token_id),
                );
            let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);
            token.on(
                &mut client,
                &mut state,
                set_serial_request(token_id, 9999, seat_id),
                &mut client_dispatcher,
            );
        }
        {
            let token = state
                .objects
                .grab_object::<crate::wayland_objects::xdg_activation_token_v1::XdgActivationTokenV1>(
                    client.object_id(token_id),
                );
            let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);
            token.on(&mut client, &mut state, commit_request(token_id), &mut client_dispatcher);
        }

        assert_eq!(state.activation_tokens.tracked_count(), 0);
    }

    #[test]
    fn double_commit_keeps_single_tracked_token() {
        let mut state = state_for_tests();
        let (mut client, mut dispatcher) = test_client_and_dispatcher(3);
        let surface_id = WlSurfaceId::from(40u32);
        let manager_id = XdgActivationV1Id::from(41u32);
        let token_id = XdgActivationTokenV1Id::from(42u32);
        let seat_id = WlSeatId::from(43u32);

        insert_mapped_surface(&mut state, &mut client, surface_id, 300);
        insert_manager(&mut state, &mut client, manager_id);
        insert_seat(&mut state, &mut client, seat_id);
        {
            let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);
            set_keyboard_focus_from_surface(
                &mut state,
                &mut client_dispatcher,
                Some(focus_for_surface(
                    client.client_id(),
                    surface_id,
                    client.object_id(surface_id),
                )),
            );
            handle_keyboard_key(&mut state, &mut client_dispatcher, 30, 1, 100);
        }
        let serial = state.seat.last_keyboard_trigger_serial().unwrap();

        {
            let mut manager = state
                .objects
                .grab_object::<XdgActivationV1>(client.object_id(manager_id));
            let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);
            manager.on(
                &mut client,
                &mut state,
                get_activation_token_request(manager_id, token_id),
                &mut client_dispatcher,
            );
        }
        {
            let token = state
                .objects
                .grab_object::<crate::wayland_objects::xdg_activation_token_v1::XdgActivationTokenV1>(
                    client.object_id(token_id),
                );
            let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);
            token.on(
                &mut client,
                &mut state,
                set_surface_request(token_id, surface_id),
                &mut client_dispatcher,
            );
        }
        {
            let token = state
                .objects
                .grab_object::<crate::wayland_objects::xdg_activation_token_v1::XdgActivationTokenV1>(
                    client.object_id(token_id),
                );
            let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);
            token.on(
                &mut client,
                &mut state,
                set_serial_request(token_id, serial, seat_id),
                &mut client_dispatcher,
            );
        }
        {
            let token = state
                .objects
                .grab_object::<crate::wayland_objects::xdg_activation_token_v1::XdgActivationTokenV1>(
                    client.object_id(token_id),
                );
            let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);
            token.on(&mut client, &mut state, commit_request(token_id), &mut client_dispatcher);
        }
        {
            let token = state
                .objects
                .grab_object::<crate::wayland_objects::xdg_activation_token_v1::XdgActivationTokenV1>(
                    client.object_id(token_id),
                );
            let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);
            token.on(&mut client, &mut state, commit_request(token_id), &mut client_dispatcher);
        }

        assert_eq!(state.activation_tokens.tracked_count(), 1);
    }

    #[test]
    fn expired_token_is_ignored_on_activate() {
        let mut state = state_for_tests();
        let (mut client, mut dispatcher) = test_client_and_dispatcher(4);
        let requester_surface_id = WlSurfaceId::from(50u32);
        let target_surface_id = WlSurfaceId::from(51u32);
        let requester_object_id =
            insert_mapped_surface(&mut state, &mut client, requester_surface_id, 400);
        let target_object_id =
            insert_mapped_surface(&mut state, &mut client, target_surface_id, 401);
        let manager_id = XdgActivationV1Id::from(52u32);
        let token_id = XdgActivationTokenV1Id::from(53u32);
        let seat_id = WlSeatId::from(54u32);

        insert_manager(&mut state, &mut client, manager_id);
        insert_seat(&mut state, &mut client, seat_id);
        {
            let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);
            set_keyboard_focus_from_surface(
                &mut state,
                &mut client_dispatcher,
                Some(focus_for_surface(
                    client.client_id(),
                    requester_surface_id,
                    requester_object_id,
                )),
            );
            handle_keyboard_key(&mut state, &mut client_dispatcher, 30, 1, 100);
        }
        let serial = state.seat.last_keyboard_trigger_serial().unwrap();

        {
            let mut manager = state
                .objects
                .grab_object::<XdgActivationV1>(client.object_id(manager_id));
            let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);
            manager.on(
                &mut client,
                &mut state,
                get_activation_token_request(manager_id, token_id),
                &mut client_dispatcher,
            );
        }
        {
            let token = state
                .objects
                .grab_object::<crate::wayland_objects::xdg_activation_token_v1::XdgActivationTokenV1>(
                    client.object_id(token_id),
                );
            let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);
            token.on(
                &mut client,
                &mut state,
                set_surface_request(token_id, requester_surface_id),
                &mut client_dispatcher,
            );
        }
        {
            let token = state
                .objects
                .grab_object::<crate::wayland_objects::xdg_activation_token_v1::XdgActivationTokenV1>(
                    client.object_id(token_id),
                );
            let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);
            token.on(
                &mut client,
                &mut state,
                set_serial_request(token_id, serial, seat_id),
                &mut client_dispatcher,
            );
        }
        {
            let token = state
                .objects
                .grab_object::<crate::wayland_objects::xdg_activation_token_v1::XdgActivationTokenV1>(
                    client.object_id(token_id),
                );
            let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);
            token.on(&mut client, &mut state, commit_request(token_id), &mut client_dispatcher);
        }

        let token = state.activation_tokens.first_token().unwrap();
        assert!(state.activation_tokens.expire_now(&token));
        {
            let mut manager = state
                .objects
                .grab_object::<XdgActivationV1>(client.object_id(manager_id));
            let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);
            manager.on(
                &mut client,
                &mut state,
                activate_request(manager_id, &token, target_surface_id),
                &mut client_dispatcher,
            );
        }

        assert_eq!(
            state
                .root_surfaces()
                .into_iter()
                .map(|surface| surface.object_id())
                .collect::<Vec<_>>(),
            vec![requester_object_id, target_object_id]
        );
        assert_eq!(
            state.seat.keyboard_focus(),
            Some(focus_for_surface(client.client_id(), requester_surface_id, requester_object_id,))
        );
    }

    #[test]
    fn unknown_token_is_ignored_on_activate() {
        let mut state = state_for_tests();
        let (mut client, mut dispatcher) = test_client_and_dispatcher(5);
        let requester_surface_id = WlSurfaceId::from(60u32);
        let target_surface_id = WlSurfaceId::from(61u32);
        let requester_object_id =
            insert_mapped_surface(&mut state, &mut client, requester_surface_id, 500);
        let target_object_id =
            insert_mapped_surface(&mut state, &mut client, target_surface_id, 501);
        let manager_id = XdgActivationV1Id::from(62u32);

        insert_manager(&mut state, &mut client, manager_id);
        {
            let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);
            set_keyboard_focus_from_surface(
                &mut state,
                &mut client_dispatcher,
                Some(focus_for_surface(
                    client.client_id(),
                    requester_surface_id,
                    requester_object_id,
                )),
            );
        }
        {
            let mut manager = state
                .objects
                .grab_object::<XdgActivationV1>(client.object_id(manager_id));
            let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);
            manager.on(
                &mut client,
                &mut state,
                activate_request(manager_id, "does-not-exist", target_surface_id),
                &mut client_dispatcher,
            );
        }

        assert_eq!(
            state
                .root_surfaces()
                .into_iter()
                .map(|surface| surface.object_id())
                .collect::<Vec<_>>(),
            vec![requester_object_id, target_object_id]
        );
        assert_eq!(
            state.seat.keyboard_focus(),
            Some(focus_for_surface(client.client_id(), requester_surface_id, requester_object_id,))
        );
    }

    #[test]
    fn global_is_registered() {
        let global = XdgActivationV1Global::new();
        assert_eq!(
            global.interface(),
            (XdgActivationV1::INTERFACE_NAME, XdgActivationV1::INTERFACE_VERSION)
        );
    }
}
