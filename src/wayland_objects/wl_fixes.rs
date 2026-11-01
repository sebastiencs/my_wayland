use crate::{
    clients::{Client, ClientDispatcher, RawRequest, Resolved},
    globals::{Global, GlobalName, GlobalRemoveAckError},
    object::{Object, Parser, RequestResult},
    protocol_generated::{
        wl_display, wl_fixes, Constants, NewId, WaylandId, WlDisplayId, WlFixesId, WlRegistryId,
    },
    state::{GrabbedObject, State},
    wayland_objects::wl_display::WlDisplay,
};

#[derive(Debug)]
pub struct WlFixesGlobal;

impl WlFixesGlobal {
    pub fn new() -> Box<dyn Global> {
        Box::new(Self {})
    }
}

impl Global for WlFixesGlobal {
    fn interface(&self) -> (&'static str, u32) {
        (WlFixes::INTERFACE_NAME, WlFixes::INTERFACE_VERSION)
    }

    fn bind(
        &mut self,
        client: &mut Client,
        state: &mut State,
        id: NewId,
        version: u32,
        _dispatcher: &mut ClientDispatcher,
    ) {
        client.insert_object(state, WlFixes::new(WlFixesId::from(id), version));
    }
}

#[derive(Debug)]
pub struct WlFixes {
    id: WlFixesId,
    version: u32,
}

impl WlFixes {
    pub fn new(id: WlFixesId, version: u32) -> Self {
        Self { id, version }
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

impl Object for WlFixes {
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
        use wl_fixes::Request::*;

        match client.parse::<Self>(req, dispatcher)? {
            Destroy => self.delete_self(client, state, dispatcher),
            DestroyRegistry { registry } => {
                let registry = client.resolve_or_disconnect(registry, state, dispatcher)?;
                state.globals.release_registry(registry);
                client.delete_id(
                    &WlDisplay::default(),
                    state,
                    dispatcher,
                    registry.wayland_id().into(),
                );
            }
            AckGlobalRemove { registry, name } => {
                if self.version < wl_fixes::ACK_GLOBAL_REMOVE_SINCE {
                    client.protocol_violation_disconnect(
                        dispatcher,
                        self.id.into(),
                        wl_display::Error::InvalidMethod as u32,
                        "wl_fixes.ack_global_remove requires wl_fixes version >= 2",
                    );
                    return Ok(());
                }
                let registry: Resolved<WlRegistryId> =
                    client.resolve_or_disconnect(registry, state, dispatcher)?;
                if let Err(error) = state
                    .globals
                    .ack_global_remove(GlobalName::from_raw(name), registry)
                {
                    let message = match error {
                        GlobalRemoveAckError::Unknown => {
                            "wl_fixes.ack_global_remove names a global this registry was not offered"
                        }
                        GlobalRemoveAckError::NotRemoved => {
                            "wl_fixes.ack_global_remove names a global that is not removed"
                        }
                    };
                    client.protocol_violation_disconnect(
                        dispatcher,
                        self.id.into(),
                        wl_fixes::Error::InvalidAckRemove as u32,
                        message,
                    );
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use {
        super::*,
        crate::{
            clients::{Client, ClientAction, ClientId, ClientManagerAction, RawEvent},
            globals::Global,
            runner::{Dispatcher, Model},
            state::State,
            test_support::state_for_tests,
            wayland_objects::wl_display::WlDisplay,
        },
    };

    struct TestGlobal;

    impl Global for TestGlobal {
        fn interface(&self) -> (&'static str, u32) {
            ("test-global", 1)
        }

        fn bind(
            &mut self,
            _client: &mut Client,
            _state: &mut State,
            _id: NewId,
            _version: u32,
            _dispatcher: &mut ClientDispatcher,
        ) {
        }
    }

    fn request(object: u32, opcode: u16, words: u16) -> RawEvent {
        RawEvent::new(WaylandId::from(object), opcode, words.into())
    }

    fn dispatch_request<K>(
        client: &mut Client,
        state: &mut State,
        dispatcher: &mut Dispatcher,
        object: K,
        event: RawEvent,
    ) where
        K: crate::object::IdToObject + Into<WaylandId> + Copy,
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

    fn registry(
        client: &mut Client,
        state: &mut State,
        dispatcher: &mut Dispatcher,
    ) -> WlRegistryId {
        client.insert_object(state, WlDisplay::default());
        let mut event = request(1, 1, 1);
        event.id(WlRegistryId::from(2));
        dispatch_request(client, state, dispatcher, WlDisplayId::from(1u32), event);
        WlRegistryId::from(2)
    }

    #[test]
    fn wl_fixes_is_advertised_at_version_two() {
        let state = state_for_tests();
        assert_eq!(
            state
                .globals
                .name_for_interface(WlFixes::INTERFACE_NAME)
                .map(|_| {
                    state
                        .globals
                        .iter()
                        .find(|(_, global)| global.interface().0 == WlFixes::INTERFACE_NAME)
                        .unwrap()
                        .1
                        .interface()
                }),
            Some((WlFixes::INTERFACE_NAME, 2))
        );
    }

    #[test]
    fn removed_global_stays_bindable_until_every_registry_acks() {
        let mut state = state_for_tests();
        let mut client = Client::new_for_tests(ClientId::from_raw(4));
        let mut dispatcher = Dispatcher::default();
        let name = state.insert_global(Box::new(TestGlobal));
        let first = registry(&mut client, &mut state, &mut dispatcher);
        let mut second_event = request(1, 1, 1);
        second_event.id(WlRegistryId::from(3));
        dispatch_request(
            &mut client,
            &mut state,
            &mut dispatcher,
            WlDisplayId::from(1u32),
            second_event,
        );

        assert!(state.remove_global(name).is_none());
        assert!(state.globals.try_grab_global(name.as_u32()).is_some());
        client
            .process(
                &mut state,
                ClientAction::RegistryGlobalRemoved { name: name.as_u32() },
                &mut dispatcher,
            )
            .unwrap();

        client.insert_object(&mut state, WlFixes::new(WlFixesId::from(8), 2));
        let mut ack = request(8, 2, 2);
        ack.id(first);
        ack.uint(name.as_u32());
        dispatch_request(&mut client, &mut state, &mut dispatcher, WlFixesId::from(8u32), ack);
        assert!(state.globals.try_grab_global(name.as_u32()).is_some());

        let mut ack = request(8, 2, 2);
        ack.id(WlRegistryId::from(3));
        ack.uint(name.as_u32());
        dispatch_request(&mut client, &mut state, &mut dispatcher, WlFixesId::from(8u32), ack);
        assert!(state.globals.try_grab_global(name.as_u32()).is_none());
    }

    #[test]
    fn ack_of_a_live_global_is_invalid_and_destroying_the_registry_withdraws_it() {
        let mut state = state_for_tests();
        let mut client = Client::new_for_tests(ClientId::from_raw(5));
        let mut dispatcher = Dispatcher::default();
        let name = state.insert_global(Box::new(TestGlobal));
        let registry_id = registry(&mut client, &mut state, &mut dispatcher);
        client.insert_object(&mut state, WlFixes::new(WlFixesId::from(8), 2));

        let mut ack = request(8, 2, 2);
        ack.id(registry_id);
        ack.uint(name.as_u32());
        dispatch_request(&mut client, &mut state, &mut dispatcher, WlFixesId::from(8u32), ack);
        assert!(dispatcher.test_has_queued_action::<ClientManagerAction>(|action| {
            matches!(action, ClientManagerAction::Close { id } if *id == client.client_id())
        }));
        assert!(state.globals.try_grab_global(name.as_u32()).is_some());

        let mut state = state_for_tests();
        let mut client = Client::new_for_tests(ClientId::from_raw(6));
        let mut dispatcher = Dispatcher::default();
        let name = state.insert_global(Box::new(TestGlobal));
        let registry_id = registry(&mut client, &mut state, &mut dispatcher);
        assert!(state.remove_global(name).is_none());
        client.insert_object(&mut state, WlFixes::new(WlFixesId::from(8), 2));
        let mut destroy_registry = request(8, 1, 1);
        destroy_registry.id(registry_id);
        dispatch_request(
            &mut client,
            &mut state,
            &mut dispatcher,
            WlFixesId::from(8u32),
            destroy_registry,
        );
        assert!(!client.contains_object(registry_id, &state));
        assert!(state.globals.try_grab_global(name.as_u32()).is_none());
    }

    #[test]
    fn version_one_cannot_ack_and_destroy_deletes_only_wl_fixes() {
        let mut state = state_for_tests();
        let mut client = Client::new_for_tests(ClientId::from_raw(7));
        let mut dispatcher = Dispatcher::default();
        client.insert_object(&mut state, WlFixes::new(WlFixesId::from(8), 1));
        let mut ack = request(8, 2, 2);
        ack.id(WlRegistryId::from(2));
        ack.uint(1);
        dispatch_request(&mut client, &mut state, &mut dispatcher, WlFixesId::from(8u32), ack);
        assert!(dispatcher.test_has_queued_action::<ClientManagerAction>(|action| {
            matches!(action, ClientManagerAction::Close { id } if *id == client.client_id())
        }));

        let mut state = state_for_tests();
        let mut client = Client::new_for_tests(ClientId::from_raw(8));
        let mut dispatcher = Dispatcher::default();
        let registry_id = registry(&mut client, &mut state, &mut dispatcher);
        client.insert_object(&mut state, WlFixes::new(WlFixesId::from(8), 2));
        dispatch_request(
            &mut client,
            &mut state,
            &mut dispatcher,
            WlFixesId::from(8u32),
            request(8, 0, 0),
        );
        assert!(!client.contains_object(WlFixesId::from(8), &state));
        assert!(client.contains_object(registry_id, &state));
    }
}
