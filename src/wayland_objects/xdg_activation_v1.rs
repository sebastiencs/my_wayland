use crate::{
    clients::{Client, ClientDispatcher, RawRequest},
    drm::{schedule_active_output, schedule_outputs_for_rect},
    globals::Global,
    object::{Object, Parser},
    protocol_generated::{
        wl_display, xdg_activation_v1, Constants, NewId, WaylandId, XdgActivationV1Id,
    },
    seat::{resolve_activation_target, set_surface_focus},
    state::State,
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
        &mut self,
        client: &mut Client,
        state: &mut State,
        req: RawRequest,
        dispatcher: &mut ClientDispatcher,
    ) {
        use xdg_activation_v1::Request::*;

        match client.parse::<Self>(req).unwrap() {
            Destroy => {
                client.delete_id(&WlDisplay::default(), state, dispatcher, self.id.into());
            }
            GetActivationToken { id } => {
                client.insert_object(state, XdgActivationTokenV1::new(id));
            }
            Activate { token, surface } => {
                if !client.object_is(surface, state) {
                    client.protocol_violation_disconnect(
                        dispatcher,
                        self.id.into(),
                        wl_display::Error::InvalidObject as u32,
                        "xdg_activation_v1.activate requires a live wl_surface",
                    );
                    return;
                }
                let Some(_) = state.activation_tokens.consume(&token) else {
                    log::debug!(
                        "ignoring activation request with unknown or expired token from client {:?}",
                        client.client_id()
                    );
                    return;
                };
                let Some((root_surface_object_id, focus)) =
                    resolve_activation_target(state, surface)
                else {
                    log::debug!(
                        "ignoring activation request for non-activatable surface {:?}",
                        surface
                    );
                    return;
                };
                if !state.raise_root_surface(root_surface_object_id) {
                    return;
                }
                set_surface_focus(state, dispatcher, Some(focus));
                if let Some(bounds) = state
                    .objects
                    .try_ref_object::<crate::wayland_objects::wl_surface::WlSurface>(
                        root_surface_object_id,
                    )
                    .and_then(|surface| surface.global_logical_bounds())
                {
                    schedule_outputs_for_rect(state, bounds, dispatcher);
                } else {
                    schedule_active_output(state, dispatcher);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use {
        super::{XdgActivationV1, XdgActivationV1Global},
        crate::{
            clients::{Client, ClientId, RawEvent},
            egl::EGL,
            globals::Globals,
            object::Object,
            protocol_generated::{
                Constants, WlSeatId, WlSurfaceId, XdgActivationTokenV1Id, XdgActivationV1Id,
            },
            runner::Dispatcher,
            seat::{handle_keyboard_key, set_surface_focus, SurfaceFocus},
            state::{ObjectId, State},
            wayland_objects::{wl_seat::WlSeat, wl_surface::WlSurface},
            xkb::Xkb,
        },
    };

    fn make_state() -> State {
        let xkb = Xkb::new();
        let keyboard = xkb.create_keyboard_state();
        let mut globals = Globals::new();
        globals.initialize();
        State {
            objects: Default::default(),
            root_surfaces: Default::default(),
            layer_surface_objects: Default::default(),
            inputs: Default::default(),
            drm: Default::default(),
            outputs: Default::default(),
            active_output: None,
            backend_syncobj_globals: Default::default(),
            globals,
            activation_tokens: Default::default(),
            egl: EGL::new(),
            seat: Default::default(),
            keyboard,
            xkb,
        }
    }

    fn make_client(id: u64) -> (Client, Dispatcher) {
        (Client::new_for_tests(ClientId::from_raw(id)), Dispatcher::default())
    }

    fn insert_mapped_surface(
        state: &mut State,
        client: &mut Client,
        surface_id: WlSurfaceId,
        buffer_object_id: usize,
    ) -> ObjectId {
        let ((_, object_id), _) =
            client.insert_object(state, WlSurface::new(surface_id, client.client_id(), 6));
        state.register_root_surface(object_id);
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
        let ((_, object_id), _) = client.insert_object(state, XdgActivationV1::new(manager_id));
        object_id
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
        let mut state = make_state();
        let (mut client, mut dispatcher) = make_client(1);
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
            set_surface_focus(
                &mut state,
                &mut client_dispatcher,
                Some(SurfaceFocus {
                    client_id: client.client_id(),
                    surface_id: requester_surface_id,
                }),
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
            let mut token = state
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
            token.on(
                &mut client,
                &mut state,
                set_serial_request(token_id, serial, seat_id),
                &mut client_dispatcher,
            );
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
        assert_eq!(state.root_surface_ids(), vec![requester_object_id, target_object_id]);
        assert_eq!(
            state.seat.keyboard_focus(),
            Some(SurfaceFocus { client_id: client.client_id(), surface_id: target_surface_id })
        );
    }

    #[test]
    fn invalid_serial_commit_returns_untracked_token() {
        let mut state = make_state();
        let (mut client, mut dispatcher) = make_client(2);
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
            let mut token = state
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
            token.on(
                &mut client,
                &mut state,
                set_serial_request(token_id, 9999, seat_id),
                &mut client_dispatcher,
            );
            token.on(&mut client, &mut state, commit_request(token_id), &mut client_dispatcher);
        }

        assert_eq!(state.activation_tokens.tracked_count(), 0);
    }

    #[test]
    fn double_commit_keeps_single_tracked_token() {
        let mut state = make_state();
        let (mut client, mut dispatcher) = make_client(3);
        let surface_id = WlSurfaceId::from(40u32);
        let manager_id = XdgActivationV1Id::from(41u32);
        let token_id = XdgActivationTokenV1Id::from(42u32);
        let seat_id = WlSeatId::from(43u32);

        insert_mapped_surface(&mut state, &mut client, surface_id, 300);
        insert_manager(&mut state, &mut client, manager_id);
        insert_seat(&mut state, &mut client, seat_id);
        {
            let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);
            set_surface_focus(
                &mut state,
                &mut client_dispatcher,
                Some(SurfaceFocus { client_id: client.client_id(), surface_id }),
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
            let mut token = state
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
            token.on(
                &mut client,
                &mut state,
                set_serial_request(token_id, serial, seat_id),
                &mut client_dispatcher,
            );
            token.on(&mut client, &mut state, commit_request(token_id), &mut client_dispatcher);
            token.on(&mut client, &mut state, commit_request(token_id), &mut client_dispatcher);
        }

        assert_eq!(state.activation_tokens.tracked_count(), 1);
    }

    #[test]
    fn expired_token_is_ignored_on_activate() {
        let mut state = make_state();
        let (mut client, mut dispatcher) = make_client(4);
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
            set_surface_focus(
                &mut state,
                &mut client_dispatcher,
                Some(SurfaceFocus {
                    client_id: client.client_id(),
                    surface_id: requester_surface_id,
                }),
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
            let mut token = state
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
            token.on(
                &mut client,
                &mut state,
                set_serial_request(token_id, serial, seat_id),
                &mut client_dispatcher,
            );
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

        assert_eq!(state.root_surface_ids(), vec![requester_object_id, target_object_id]);
        assert_eq!(
            state.seat.keyboard_focus(),
            Some(SurfaceFocus {
                client_id: client.client_id(),
                surface_id: requester_surface_id
            })
        );
    }

    #[test]
    fn unknown_token_is_ignored_on_activate() {
        let mut state = make_state();
        let (mut client, mut dispatcher) = make_client(5);
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
            set_surface_focus(
                &mut state,
                &mut client_dispatcher,
                Some(SurfaceFocus {
                    client_id: client.client_id(),
                    surface_id: requester_surface_id,
                }),
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

        assert_eq!(state.root_surface_ids(), vec![requester_object_id, target_object_id]);
        assert_eq!(
            state.seat.keyboard_focus(),
            Some(SurfaceFocus {
                client_id: client.client_id(),
                surface_id: requester_surface_id
            })
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
