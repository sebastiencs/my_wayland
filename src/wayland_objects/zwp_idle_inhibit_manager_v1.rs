use crate::{
    clients::{Client, ClientDispatcher, RawRequest},
    globals::Global,
    object::{Object, Parser, RequestResult},
    protocol_generated::{
        zwp_idle_inhibit_manager_v1, Constants, NewId, WaylandId, WlSurfaceId,
        ZwpIdleInhibitManagerV1Id, ZwpIdleInhibitorV1Id,
    },
    state::{GrabbedObject, State},
    wayland_objects::{wl_display::WlDisplay, zwp_idle_inhibitor_v1::ZwpIdleInhibitorV1},
};

/// Present so a later phase can publish the global. `Globals::initialize` does
/// not insert it: inhibition is not honored until visibility policy exists.
#[derive(Debug)]
pub struct ZwpIdleInhibitManagerV1Global;

impl ZwpIdleInhibitManagerV1Global {
    pub fn new() -> Box<dyn Global> {
        Box::new(Self)
    }
}

impl Global for ZwpIdleInhibitManagerV1Global {
    fn interface(&self) -> (&'static str, u32) {
        (ZwpIdleInhibitManagerV1::INTERFACE_NAME, ZwpIdleInhibitManagerV1::INTERFACE_VERSION)
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
            ZwpIdleInhibitManagerV1 { id: ZwpIdleInhibitManagerV1Id::from(id) },
        );
    }
}

#[derive(Debug)]
pub struct ZwpIdleInhibitManagerV1 {
    id: ZwpIdleInhibitManagerV1Id,
}

impl Object for ZwpIdleInhibitManagerV1 {
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
        use zwp_idle_inhibit_manager_v1::Request::*;

        match client.parse::<Self>(req, dispatcher)? {
            Destroy => {
                client.delete_id(&WlDisplay::default(), state, dispatcher, self.id.into());
            }
            CreateInhibitor { id, surface } => {
                let id = ZwpIdleInhibitorV1Id::from(id);
                let surface =
                    client.resolve_or_disconnect(WlSurfaceId::from(surface), state, dispatcher)?;
                let (inhibitor, _) =
                    client.insert_object(state, ZwpIdleInhibitorV1::new(id, surface));
                surface
                    .try_ref_mut(state)
                    .expect("idle inhibitor surface")
                    .add_idle_inhibitor(inhibitor);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use crate::{
        clients::{Arguments, Client, ClientAction, ClientId, ClientManagerAction, RawRequest},
        globals::{Global, Globals},
        object::{Object, RequestResult},
        protocol_generated::{
            Constants, NewId, WlSurfaceId, ZwpIdleInhibitManagerV1Id, ZwpIdleInhibitorV1Id,
        },
        runner::Model,
        state::ObjectId,
        test_support::{state_for_tests, test_client_and_dispatcher},
        wayland_objects::{wl_surface::WlSurface, zwp_idle_inhibitor_v1::ZwpIdleInhibitorV1},
    };

    use super::{ZwpIdleInhibitManagerV1, ZwpIdleInhibitManagerV1Global};

    struct Fixture {
        state: crate::state::State,
        client: Client,
        dispatcher: crate::runner::Dispatcher,
        surface: crate::clients::Resolved<WlSurfaceId>,
        manager: crate::clients::Resolved<ZwpIdleInhibitManagerV1Id>,
    }

    fn new_fixture() -> Fixture {
        let mut state = state_for_tests();
        let (mut client, dispatcher) = test_client_and_dispatcher(1);
        let (surface, _) = client
            .insert_object(&mut state, WlSurface::new(WlSurfaceId::from(5), client.client_id(), 6));
        let (manager, _) = client.insert_object(
            &mut state,
            ZwpIdleInhibitManagerV1 { id: ZwpIdleInhibitManagerV1Id::from(6) },
        );
        Fixture { state, client, dispatcher, surface, manager }
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

    fn create(fixture: &mut Fixture, id: u32) -> RequestResult {
        let manager = fixture.manager.object_id();
        dispatch(fixture, manager, 6, 1, &[id, 5])
    }

    fn closed(fixture: &Fixture) -> bool {
        fixture.dispatcher.test_has_queued_action(|action: &ClientManagerAction| {
            matches!(action, ClientManagerAction::Close { id } if *id == ClientId::from_raw(1))
        })
    }

    #[test]
    fn the_inhibit_global_stays_unpublished() {
        let mut globals = Globals::new();
        globals.initialize();
        assert!(!globals.contains_interface(ZwpIdleInhibitManagerV1::INTERFACE_NAME));

        let mut state = state_for_tests();
        let (mut client, mut dispatcher) = test_client_and_dispatcher(1);
        let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);
        let mut global = ZwpIdleInhibitManagerV1Global;
        global.bind(&mut client, &mut state, NewId::from(6), 1, &mut client_dispatcher);
        assert!(client.contains_object(ZwpIdleInhibitManagerV1Id::from(6), &state));
        assert_eq!(ZwpIdleInhibitManagerV1Global.interface(), ("zwp_idle_inhibit_manager_v1", 1));
    }

    #[test]
    fn each_request_adds_another_inhibitor_on_the_same_surface() {
        let mut fixture = new_fixture();
        create(&mut fixture, 7).unwrap();
        create(&mut fixture, 8).unwrap();
        let inhibitors = fixture.surface.rref(&fixture.state).idle_inhibitors();
        assert_eq!(inhibitors.len(), 2);
        assert!(fixture
            .client
            .contains_object(ZwpIdleInhibitorV1Id::from(7), &fixture.state));
        assert!(fixture
            .client
            .contains_object(ZwpIdleInhibitorV1Id::from(8), &fixture.state));
        assert!(fixture.state.idle_notifications.is_empty());
        assert!(!closed(&fixture));

        let first = fixture
            .client
            .resolve(ZwpIdleInhibitorV1Id::from(7), &fixture.state)
            .unwrap()
            .object_id();
        dispatch(&mut fixture, first, 7, 0, &[]).unwrap();
        let inhibitors = fixture.surface.rref(&fixture.state).idle_inhibitors();
        assert_eq!(inhibitors.len(), 1);
        assert_eq!(
            inhibitors[0],
            fixture
                .client
                .resolve(ZwpIdleInhibitorV1Id::from(8), &fixture.state)
                .unwrap()
        );
        assert!(!fixture
            .client
            .contains_object(ZwpIdleInhibitorV1Id::from(7), &fixture.state));
    }

    #[test]
    fn an_unknown_surface_disconnects_without_an_inhibitor() {
        let mut fixture = new_fixture();
        let manager = fixture.manager.object_id();
        let result = dispatch(&mut fixture, manager, 6, 1, &[7, 99]);
        assert!(result.is_err());
        assert!(!fixture
            .client
            .contains_object(ZwpIdleInhibitorV1Id::from(7), &fixture.state));
        assert!(fixture
            .surface
            .rref(&fixture.state)
            .idle_inhibitors()
            .is_empty());
        assert!(closed(&fixture));
    }

    #[test]
    fn destroying_the_manager_leaves_inhibitors() {
        let mut fixture = new_fixture();
        create(&mut fixture, 7).unwrap();
        let manager = fixture.manager.object_id();
        dispatch(&mut fixture, manager, 6, 0, &[]).unwrap();
        assert!(!fixture
            .client
            .contains_object(ZwpIdleInhibitManagerV1Id::from(6), &fixture.state));
        assert!(fixture
            .client
            .contains_object(ZwpIdleInhibitorV1Id::from(7), &fixture.state));
        assert_eq!(fixture.surface.rref(&fixture.state).idle_inhibitors().len(), 1);
        assert!(!closed(&fixture));
    }

    #[test]
    fn destroying_the_surface_leaves_an_inert_inhibitor() {
        let mut fixture = new_fixture();
        create(&mut fixture, 7).unwrap();
        {
            let mut surface = fixture
                .state
                .objects
                .grab_object::<WlSurface>(fixture.surface.object_id());
            let mut client_dispatcher = fixture.client.test_dispatcher(&mut fixture.dispatcher);
            surface.delete_self(&mut fixture.client, &mut fixture.state, &mut client_dispatcher);
        }
        let inhibitor = fixture
            .client
            .resolve(ZwpIdleInhibitorV1Id::from(7), &fixture.state)
            .expect("inhibitor stays until the client destroys it");
        assert!(inhibitor.rref(&fixture.state).surface().is_none());
        let inhibitor = fixture
            .client
            .resolve(ZwpIdleInhibitorV1Id::from(7), &fixture.state)
            .unwrap()
            .object_id();
        dispatch(&mut fixture, inhibitor, 7, 0, &[]).unwrap();
        assert!(!fixture
            .client
            .contains_object(ZwpIdleInhibitorV1Id::from(7), &fixture.state));
        assert!(!closed(&fixture));
    }

    #[test]
    fn client_disconnect_drops_the_association_and_the_objects() {
        let mut fixture = new_fixture();
        create(&mut fixture, 7).unwrap();
        let inhibitor = fixture
            .client
            .resolve(ZwpIdleInhibitorV1Id::from(7), &fixture.state)
            .unwrap();
        {
            let mut inhibitor = fixture
                .state
                .objects
                .try_grab_object::<ZwpIdleInhibitorV1>(inhibitor.object_id())
                .unwrap();
            let mut client_dispatcher = fixture.client.test_dispatcher(&mut fixture.dispatcher);
            inhibitor.on_client_disconnect(
                &mut fixture.client,
                &mut fixture.state,
                &mut client_dispatcher,
            );
        }
        assert!(fixture
            .surface
            .rref(&fixture.state)
            .idle_inhibitors()
            .is_empty());

        fixture
            .client
            .process(&mut fixture.state, ClientAction::Close, &mut fixture.dispatcher)
            .unwrap();
        assert!(!fixture
            .client
            .contains_object(ZwpIdleInhibitorV1Id::from(7), &fixture.state));
        assert!(!fixture
            .client
            .contains_object(WlSurfaceId::from(5), &fixture.state));
    }
}
