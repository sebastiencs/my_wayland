use crate::{
    clients::{Client, ClientDispatcher, RawRequest},
    globals::Global,
    object::{Object, Parser, RequestResult},
    protocol_generated::{
        zwp_relative_pointer_manager_v1, Constants, NewId, WaylandId, WlPointerId,
        ZwpRelativePointerManagerV1Id, ZwpRelativePointerV1Id,
    },
    state::{GrabbedObject, State},
    wayland_objects::{wl_display::WlDisplay, zwp_relative_pointer_v1::ZwpRelativePointerV1},
};

#[derive(Debug)]
pub struct ZwpRelativePointerManagerV1Global;

impl ZwpRelativePointerManagerV1Global {
    pub fn new() -> Box<dyn Global> {
        Box::new(Self)
    }
}

impl Global for ZwpRelativePointerManagerV1Global {
    fn interface(&self) -> (&'static str, u32) {
        (
            ZwpRelativePointerManagerV1::INTERFACE_NAME,
            ZwpRelativePointerManagerV1::INTERFACE_VERSION,
        )
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
            ZwpRelativePointerManagerV1 { id: ZwpRelativePointerManagerV1Id::from(id) },
        );
    }
}

#[derive(Debug)]
pub struct ZwpRelativePointerManagerV1 {
    id: ZwpRelativePointerManagerV1Id,
}

impl Object for ZwpRelativePointerManagerV1 {
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
        use zwp_relative_pointer_manager_v1::Request::*;

        match client.parse::<Self>(req, dispatcher)? {
            Destroy => {
                client.delete_id(&WlDisplay::default(), state, dispatcher, self.id.into());
            }
            GetRelativePointer { id, pointer } => {
                let id = ZwpRelativePointerV1Id::from(id);
                let pointer =
                    client.resolve_or_disconnect(WlPointerId::from(pointer), state, dispatcher)?;
                let (relative, _) =
                    client.insert_object(state, ZwpRelativePointerV1::new(id, pointer));
                state.seat.register_relative_pointer(pointer, relative);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use crate::{
        clients::{Arguments, Client, ClientId, RawRequest},
        protocol_generated::{WlPointerId, ZwpRelativePointerManagerV1Id, ZwpRelativePointerV1Id},
        runner::Dispatcher,
        test_support::state_for_tests,
        wayland_objects::wl_pointer::WlPointer,
    };

    use super::ZwpRelativePointerManagerV1;

    #[test]
    fn relative_pointers_are_tracked_per_wl_pointer() {
        let mut state = state_for_tests();
        let mut client = Client::new_for_tests(ClientId::from_raw(1));
        let (pointer, _) =
            client.insert_object(&mut state, WlPointer::new(WlPointerId::from(5), 9));
        let (manager, _) = client.insert_object(
            &mut state,
            ZwpRelativePointerManagerV1 { id: ZwpRelativePointerManagerV1Id::from(6) },
        );
        let mut dispatcher = Dispatcher::default();
        let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);
        client_dispatcher.set_requester_object_id_for_tests(6.into());
        let request = RawRequest {
            object_id: 6,
            request_id: 1,
            arguments: Arguments::from_words(VecDeque::from([7, 5])),
            fds: None,
        };
        state
            .objects
            .grab(manager.object_id())
            .on(&mut client, &mut state, request, &mut client_dispatcher)
            .unwrap();
        let relative = client
            .resolve(ZwpRelativePointerV1Id::from(7), &state)
            .expect("relative pointer");
        assert_eq!(state.seat.relative_pointers(pointer), &[relative]);

        let destroy = RawRequest {
            object_id: 7,
            request_id: 0,
            arguments: Arguments::from_words(VecDeque::new()),
            fds: None,
        };
        state
            .objects
            .grab(relative.object_id())
            .on(&mut client, &mut state, destroy, &mut client_dispatcher)
            .unwrap();
        assert!(state.seat.relative_pointers(pointer).is_empty());
    }
}
