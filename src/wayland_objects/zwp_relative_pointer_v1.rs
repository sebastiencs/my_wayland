use crate::{
    clients::{Client, ClientDispatcher, RawRequest, Resolved},
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
