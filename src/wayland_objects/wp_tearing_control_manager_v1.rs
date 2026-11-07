use crate::{
    clients::{Client, ClientDispatcher, RawRequest, Resolved},
    globals::Global,
    object::{Object, Parser, RequestResult},
    protocol_generated::{
        wp_tearing_control_manager_v1, Constants, NewId, WaylandId, WpTearingControlManagerV1Id,
    },
    state::{GrabbedObject, State},
    wayland_objects::{
        wl_display::WlDisplay, wl_surface::WlSurface, wp_tearing_control_v1::WpTearingControlV1,
    },
};

#[derive(Debug)]
pub struct WpTearingControlManagerV1Global;

impl WpTearingControlManagerV1Global {
    pub fn new() -> Box<dyn Global> {
        Box::new(Self)
    }
}

impl Global for WpTearingControlManagerV1Global {
    fn interface(&self) -> (&'static str, u32) {
        (WpTearingControlManagerV1::INTERFACE_NAME, WpTearingControlManagerV1::INTERFACE_VERSION)
    }

    fn bind(
        &mut self,
        client: &mut Client,
        state: &mut State,
        id: NewId,
        _version: u32,
        _dispatcher: &mut ClientDispatcher,
    ) {
        client.insert_object(state, WpTearingControlManagerV1 { id: id.into() });
    }
}

#[derive(Debug)]
pub struct WpTearingControlManagerV1 {
    id: WpTearingControlManagerV1Id,
}

impl Object for WpTearingControlManagerV1 {
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
        use wp_tearing_control_manager_v1::Request::*;

        match client.parse::<Self>(req, dispatcher)? {
            Destroy => {
                client.delete_id(&WlDisplay::default(), state, dispatcher, self.id.into());
            }
            GetTearingControl { id, surface } => {
                let surface = client.resolve_or_disconnect(surface, state, dispatcher)?;
                {
                    let surface = surface.rref(state);
                    if surface.has_tearing_control_object() {
                        client.send_protocol_error(
                            dispatcher,
                            self.id.into(),
                            wp_tearing_control_manager_v1::Error::TearingControlExists as u32,
                            "wl_surface already has a wp_tearing_control_v1 object",
                        );
                        return Ok(());
                    }
                }
                let (control, _) =
                    client.insert_object(state, WpTearingControlV1::new(id, surface));
                surface
                    .grab(state)
                    .set_tearing_control_object(control)
                    .expect("tearing control existence was checked before insertion");
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        clients::{Client, ClientId, RawEvent, Resolved},
        globals::Globals,
        io_uring::model::IoUringAction,
        object::{IdToObject, Object, RequestResult},
        protocol_generated::{
            wl_display, wp_tearing_control_v1, NewId, WaylandId, WlRegionId, WlSubsurfaceId,
            WlSurfaceId, WpTearingControlManagerV1Id, WpTearingControlV1Id,
        },
        runner::Dispatcher,
        test_support::{state_for_tests, test_client_and_dispatcher},
        wayland_objects::{
            wl_region::WlRegion,
            wl_subsurface::WlSubsurface,
            wl_surface::{PresentationHint, WlSurface},
        },
    };

    use super::*;

    fn request(
        object: WaylandId,
        opcode: u16,
        args: &[u32],
    ) -> crate::clients::RawRequest<'static> {
        let mut event = RawEvent::new(object, opcode, args.len());
        for arg in args {
            event.uint(*arg);
        }
        (&*event.into_bytes()).try_into().unwrap()
    }

    fn dispatch<K>(
        client: &mut Client,
        state: &mut State,
        dispatcher: &mut Dispatcher,
        id: K,
        opcode: u16,
        args: &[u32],
    ) -> RequestResult
    where
        K: IdToObject + Copy + Into<WaylandId>,
        K::Object: Object,
    {
        let wire: WaylandId = id.into();
        let mut client_dispatcher = client.test_dispatcher(dispatcher);
        client_dispatcher.set_requester_object_id_for_tests(wire);
        let mut object = client.grab_object(id, state);
        object.on(client, state, request(wire, opcode, args), &mut client_dispatcher)
    }

    fn display_error(dispatcher: &Dispatcher) -> Option<(u32, u32)> {
        let mut found = None;
        dispatcher.test_for_each_queued_action::<IoUringAction>(|action| {
            let IoUringAction::Sendmsg { buffer, .. } = action else {
                return;
            };
            if buffer.len() < 16 {
                return;
            }
            let object = u32::from_ne_bytes(buffer[0..4].try_into().unwrap());
            if object != 1 {
                return;
            }
            let target = u32::from_ne_bytes(buffer[8..12].try_into().unwrap());
            let code = u32::from_ne_bytes(buffer[12..16].try_into().unwrap());
            found = Some((target, code));
        });
        found
    }

    fn closed(dispatcher: &Dispatcher, client: &Client) -> bool {
        dispatcher.test_has_queued_action::<crate::clients::ClientManagerAction>(|action| {
            matches!(action, crate::clients::ClientManagerAction::Close { id } if *id == client.client_id())
        })
    }

    #[test]
    fn the_global_is_advertised_at_version_one() {
        let mut globals = Globals::new();
        globals.initialize();
        assert!(globals.contains_interface(WpTearingControlManagerV1::INTERFACE_NAME));

        let mut state = state_for_tests();
        let (mut client, mut dispatcher) = test_client_and_dispatcher(1);
        let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);
        let mut global = WpTearingControlManagerV1Global;
        global.bind(&mut client, &mut state, NewId::from(6), 1, &mut client_dispatcher);
        assert!(client.contains_object(WpTearingControlManagerV1Id::from(6), &state));
        assert_eq!(
            WpTearingControlManagerV1Global.interface(),
            ("wp_tearing_control_manager_v1", 1)
        );
    }

    #[test]
    fn a_second_control_on_the_same_surface_is_rejected() {
        let mut state = state_for_tests();
        let (mut client, mut dispatcher) = test_client_and_dispatcher(2);
        let manager = WpTearingControlManagerV1Id::from(3);
        let surface = WlSurfaceId::from(4);
        client.insert_object(&mut state, WpTearingControlManagerV1 { id: manager });
        client.insert_object(&mut state, WlSurface::new(surface, client.client_id(), 6));

        dispatch(&mut client, &mut state, &mut dispatcher, manager, 1, &[5, surface.into()])
            .unwrap();
        assert!(client.contains_object(WpTearingControlV1Id::from(5), &state));
        assert!(client
            .resolve(surface, &state)
            .unwrap()
            .rref(&state)
            .has_tearing_control_object());

        dispatch(&mut client, &mut state, &mut dispatcher, manager, 1, &[6, surface.into()])
            .unwrap();
        assert!(!client.contains_object(WpTearingControlV1Id::from(6), &state));
        assert_eq!(
            display_error(&dispatcher),
            Some((
                u32::from(manager),
                wp_tearing_control_manager_v1::Error::TearingControlExists as u32
            ))
        );
        assert!(!closed(&dispatcher, &client));
    }

    #[test]
    fn get_tearing_control_rejects_a_non_surface() {
        let mut state = state_for_tests();
        let (mut client, mut dispatcher) = test_client_and_dispatcher(3);
        let manager = WpTearingControlManagerV1Id::from(3);
        let region = WlRegionId::from(4);
        client.insert_object(&mut state, WpTearingControlManagerV1 { id: manager });
        client.insert_object(&mut state, WlRegion::new(region));

        let rejected =
            dispatch(&mut client, &mut state, &mut dispatcher, manager, 1, &[5, u32::from(region)]);
        assert!(rejected.is_err());
        assert!(!client.contains_object(WpTearingControlV1Id::from(5), &state));
        assert!(closed(&dispatcher, &client));
    }

    #[test]
    fn the_hint_applies_on_commit_and_stays_until_changed() {
        let mut state = state_for_tests();
        let (mut client, mut dispatcher) = test_client_and_dispatcher(4);
        let surface = WlSurfaceId::from(4);
        let control = WpTearingControlV1Id::from(5);
        client.insert_object(&mut state, WlSurface::new(surface, client.client_id(), 6));
        let surface_resolved = client.resolve(surface, &state).unwrap();
        client.insert_object(&mut state, WpTearingControlV1::new(control, surface_resolved));
        surface_resolved
            .grab(&mut state)
            .set_tearing_control_object(client.resolve(control, &state).unwrap())
            .unwrap();

        assert_eq!(
            surface_resolved.rref(&state).committed_presentation_hint(),
            PresentationHint::Vsync
        );
        dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            control,
            0,
            &[wp_tearing_control_v1::PresentationHint::Async as u32],
        )
        .unwrap();
        assert_eq!(
            surface_resolved.rref(&state).committed_presentation_hint(),
            PresentationHint::Vsync
        );

        dispatch(&mut client, &mut state, &mut dispatcher, surface, 6, &[]).unwrap();
        assert_eq!(
            surface_resolved.rref(&state).committed_presentation_hint(),
            PresentationHint::Async
        );
        dispatch(&mut client, &mut state, &mut dispatcher, surface, 6, &[]).unwrap();
        assert_eq!(
            surface_resolved.rref(&state).committed_presentation_hint(),
            PresentationHint::Async
        );
    }

    #[test]
    fn an_unknown_hint_is_an_implementation_error_and_leaves_pending_state() {
        let mut state = state_for_tests();
        let (mut client, mut dispatcher) = test_client_and_dispatcher(5);
        let surface = WlSurfaceId::from(4);
        let control = WpTearingControlV1Id::from(5);
        client.insert_object(&mut state, WlSurface::new(surface, client.client_id(), 6));
        let surface_resolved = client.resolve(surface, &state).unwrap();
        client.insert_object(&mut state, WpTearingControlV1::new(control, surface_resolved));
        surface_resolved
            .grab(&mut state)
            .set_tearing_control_object(client.resolve(control, &state).unwrap())
            .unwrap();
        dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            control,
            0,
            &[wp_tearing_control_v1::PresentationHint::Async as u32],
        )
        .unwrap();
        dispatch(&mut client, &mut state, &mut dispatcher, control, 0, &[9]).unwrap();
        assert_eq!(
            display_error(&dispatcher),
            Some((u32::from(control), wl_display::Error::Implementation as u32))
        );
        assert!(!closed(&dispatcher, &client));
        dispatch(&mut client, &mut state, &mut dispatcher, surface, 6, &[]).unwrap();
        assert_eq!(
            surface_resolved.rref(&state).committed_presentation_hint(),
            PresentationHint::Async
        );
    }

    #[test]
    fn destroy_reverts_to_vsync_on_the_next_commit() {
        let mut state = state_for_tests();
        let (mut client, mut dispatcher) = test_client_and_dispatcher(6);
        let manager = WpTearingControlManagerV1Id::from(3);
        let surface = WlSurfaceId::from(4);
        let control = WpTearingControlV1Id::from(5);
        client.insert_object(&mut state, WpTearingControlManagerV1 { id: manager });
        client.insert_object(&mut state, WlSurface::new(surface, client.client_id(), 6));
        dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            manager,
            1,
            &[u32::from(control), u32::from(surface)],
        )
        .unwrap();
        dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            control,
            0,
            &[wp_tearing_control_v1::PresentationHint::Async as u32],
        )
        .unwrap();
        dispatch(&mut client, &mut state, &mut dispatcher, surface, 6, &[]).unwrap();
        let surface_resolved = client.resolve(surface, &state).unwrap();
        assert_eq!(
            surface_resolved.rref(&state).committed_presentation_hint(),
            PresentationHint::Async
        );

        dispatch(&mut client, &mut state, &mut dispatcher, control, 1, &[]).unwrap();
        assert!(!client.contains_object(control, &state));
        assert!(!surface_resolved.rref(&state).has_tearing_control_object());
        assert_eq!(
            surface_resolved.rref(&state).committed_presentation_hint(),
            PresentationHint::Async
        );
        dispatch(&mut client, &mut state, &mut dispatcher, surface, 6, &[]).unwrap();
        assert_eq!(
            surface_resolved.rref(&state).committed_presentation_hint(),
            PresentationHint::Vsync
        );

        dispatch(&mut client, &mut state, &mut dispatcher, manager, 1, &[7, u32::from(surface)])
            .unwrap();
        assert!(client.contains_object(WpTearingControlV1Id::from(7), &state));
    }

    #[test]
    fn a_synchronized_subsurface_applies_the_hint_when_the_parent_commits() {
        let mut state = state_for_tests();
        let (mut client, mut dispatcher) = test_client_and_dispatcher(7);
        let parent = WlSurfaceId::from(4);
        let child = WlSurfaceId::from(5);
        let control = WpTearingControlV1Id::from(6);
        let (parent_resolved, _) =
            client.insert_object(&mut state, WlSurface::new(parent, client.client_id(), 6));
        state.register_root_surface(parent_resolved);
        let (child_resolved, _) =
            client.insert_object(&mut state, WlSurface::new(child, client.client_id(), 6));
        let (subsurface, _) = client.insert_object(
            &mut state,
            WlSubsurface::new(WlSubsurfaceId::from(8), child_resolved, parent_resolved),
        );
        child_resolved
            .grab(&mut state)
            .assign_subsurface(subsurface, parent_resolved);
        parent_resolved
            .grab(&mut state)
            .add_pending_child(child_resolved);
        client.insert_object(&mut state, WpTearingControlV1::new(control, child_resolved));
        child_resolved
            .grab(&mut state)
            .set_tearing_control_object(client.resolve(control, &state).unwrap())
            .unwrap();

        dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            control,
            0,
            &[wp_tearing_control_v1::PresentationHint::Async as u32],
        )
        .unwrap();
        dispatch(&mut client, &mut state, &mut dispatcher, child, 6, &[]).unwrap();
        assert_eq!(
            child_resolved.rref(&state).committed_presentation_hint(),
            PresentationHint::Vsync
        );
        dispatch(&mut client, &mut state, &mut dispatcher, parent, 6, &[]).unwrap();
        assert_eq!(
            child_resolved.rref(&state).committed_presentation_hint(),
            PresentationHint::Async
        );
    }

    #[test]
    fn client_disconnect_clears_the_surface_link() {
        let mut state = state_for_tests();
        let (mut client, mut dispatcher) = test_client_and_dispatcher(8);
        let surface = WlSurfaceId::from(4);
        let control = WpTearingControlV1Id::from(5);
        let (surface_resolved, _) =
            client.insert_object(&mut state, WlSurface::new(surface, client.client_id(), 6));
        client.insert_object(&mut state, WpTearingControlV1::new(control, surface_resolved));
        let control_resolved = client.resolve(control, &state).unwrap();
        surface_resolved
            .grab(&mut state)
            .set_tearing_control_object(control_resolved)
            .unwrap();
        surface_resolved
            .grab(&mut state)
            .set_pending_presentation_hint(PresentationHint::Async);

        {
            let mut control_object = client.grab_object(control, &mut state);
            let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);
            control_object.on_client_disconnect(&mut client, &mut state, &mut client_dispatcher);
        }
        assert!(!surface_resolved.rref(&state).has_tearing_control_object());
        dispatch(&mut client, &mut state, &mut dispatcher, surface, 6, &[]).unwrap();
        assert_eq!(
            surface_resolved.rref(&state).committed_presentation_hint(),
            PresentationHint::Vsync
        );
    }

    #[test]
    fn a_control_whose_surface_is_gone_ignores_the_hint() {
        let mut state = state_for_tests();
        let (mut client, mut dispatcher) = test_client_and_dispatcher(9);
        let surface = WlSurfaceId::from(4);
        let control = WpTearingControlV1Id::from(5);
        client.insert_object(&mut state, WlSurface::new(surface, ClientId::from_raw(9), 6));
        let surface_resolved = client.resolve(surface, &state).unwrap();
        client.insert_object(&mut state, WpTearingControlV1::new(control, surface_resolved));
        surface_resolved
            .grab(&mut state)
            .set_tearing_control_object(client.resolve(control, &state).unwrap())
            .unwrap();

        {
            let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);
            let mut surface_object = client.grab_object(surface, &mut state);
            surface_object.delete_self(&mut client, &mut state, &mut client_dispatcher);
        }
        assert!(!client.contains_object(surface, &state));
        assert!(client.contains_object(control, &state));
        dispatch(
            &mut client,
            &mut state,
            &mut dispatcher,
            control,
            0,
            &[wp_tearing_control_v1::PresentationHint::Async as u32],
        )
        .unwrap();
        assert!(display_error(&dispatcher).is_none());
        assert!(!closed(&dispatcher, &client));
    }
}
