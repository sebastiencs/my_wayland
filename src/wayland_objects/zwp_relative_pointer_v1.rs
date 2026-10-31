use crate::{
    clients::{Client, ClientDispatcher, EventWriter, RawRequest, Resolved},
    object::{Object, Parser, RequestResult},
    protocol_generated::{zwp_relative_pointer_v1, WaylandId, WlPointerId, ZwpRelativePointerV1Id},
    state::{GrabbedObject, State},
    wayland_objects::wl_display::WlDisplay,
};

#[derive(Debug)]
pub struct ZwpRelativePointerV1 {
    id: ZwpRelativePointerV1Id,
    pointer: Resolved<WlPointerId>,
}

impl ZwpRelativePointerV1 {
    pub(crate) fn new(id: ZwpRelativePointerV1Id, pointer: Resolved<WlPointerId>) -> Self {
        Self { id, pointer }
    }

    fn unregister(&self, state: &mut State) {
        state
            .seat
            .unregister_relative_pointer(self.pointer, self.id);
    }

    pub(crate) fn send_relative_motion(
        &self,
        utime_hi: u32,
        utime_lo: u32,
        dx: f32,
        dy: f32,
        dx_unaccel: f32,
        dy_unaccel: f32,
        dispatcher: &mut ClientDispatcher,
    ) {
        self.send_event(
            zwp_relative_pointer_v1::Event::RelativeMotion {
                utime_hi,
                utime_lo,
                dx,
                dy,
                dx_unaccel,
                dy_unaccel,
            },
            dispatcher,
        );
    }
}

impl Object for ZwpRelativePointerV1 {
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
        match client.parse::<Self>(req, dispatcher)? {
            zwp_relative_pointer_v1::Request::Destroy => {
                self.unregister(state);
                client.delete_id(&WlDisplay::default(), state, dispatcher, self.id.into());
            }
        }
        Ok(())
    }

    fn on_client_disconnect(
        &mut self,
        _client: &mut Client,
        state: &mut State,
        _dispatcher: &mut ClientDispatcher,
    ) {
        self.unregister(state);
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        clients::{Client, ClientAction, ClientId},
        io_uring::model::IoUringAction,
        protocol_generated::{WlPointerId, ZwpRelativePointerV1Id},
        runner::Model,
        seat::SeatClientEvent,
        test_support::{state_for_tests, test_client_and_dispatcher},
        wayland_objects::wl_pointer::WlPointer,
    };

    use super::ZwpRelativePointerV1;

    fn fixed_bits(value: f32) -> u32 {
        ((value * 256.0).round() as i32) as u32
    }

    fn words(buffer: &[u8]) -> Vec<u32> {
        buffer
            .chunks_exact(4)
            .map(|chunk| u32::from_ne_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
            .collect()
    }

    #[test]
    fn each_live_relative_pointer_receives_the_motion_event() {
        let mut state = state_for_tests();
        let (mut client, mut dispatcher) = test_client_and_dispatcher(7);
        let (pointer, _) =
            client.insert_object(&mut state, WlPointer::new(WlPointerId::from(5), 9));
        let (first, _) = client.insert_object(
            &mut state,
            ZwpRelativePointerV1::new(ZwpRelativePointerV1Id::from(7), pointer),
        );
        let (second, _) = client.insert_object(
            &mut state,
            ZwpRelativePointerV1::new(ZwpRelativePointerV1Id::from(8), pointer),
        );
        state.seat.register_relative_pointer(pointer, first);
        state.seat.register_relative_pointer(pointer, second);

        let motion = || SeatClientEvent::RelativePointerMotion {
            utime_hi: 1,
            utime_lo: 5,
            dx: 40.0,
            dy: -2.25,
            dx_unaccel: 3.0,
            dy_unaccel: -0.5,
        };
        client
            .process(&mut state, ClientAction::SeatEvent { event: motion() }, &mut dispatcher)
            .unwrap();

        let expected = |object_id: u32, buffer: &[u8]| {
            words(buffer)
                == vec![
                    object_id,
                    32 << 16,
                    1,
                    5,
                    fixed_bits(40.0),
                    fixed_bits(-2.25),
                    fixed_bits(3.0),
                    fixed_bits(-0.5),
                ]
        };
        assert_eq!(
            dispatcher.test_count_queued_action::<IoUringAction>(|action| matches!(
                action,
                IoUringAction::Sendmsg { buffer, .. } if expected(7, buffer) || expected(8, buffer)
            )),
            2
        );

        let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);
        state
            .objects
            .grab(first.object_id())
            .on(
                &mut client,
                &mut state,
                crate::clients::RawRequest {
                    object_id: 7,
                    request_id: 0,
                    arguments: crate::clients::Arguments::from_words(
                        std::collections::VecDeque::new(),
                    ),
                    fds: None,
                },
                &mut client_dispatcher,
            )
            .unwrap();
        drop(client_dispatcher);

        let mut dispatcher = crate::runner::Dispatcher::default();
        client
            .process(&mut state, ClientAction::SeatEvent { event: motion() }, &mut dispatcher)
            .unwrap();
        assert_eq!(
            dispatcher.test_count_queued_action::<IoUringAction>(|action| matches!(
                action,
                IoUringAction::Sendmsg { buffer, .. } if expected(8, buffer)
            )),
            1
        );
        assert_eq!(
            dispatcher.test_count_queued_action::<IoUringAction>(|action| matches!(
                action,
                IoUringAction::Sendmsg { buffer, .. } if expected(7, buffer)
            )),
            0
        );
    }
}
