use crate::{
    clients::{Client, ClientDispatcher, RawRequest},
    object::{Object, Parser, RequestResult},
    protocol_generated::{wl_touch, WaylandId, WlTouchId},
    state::{GrabbedObject, State},
    wayland_objects::wl_display::WlDisplay,
};

/// Inert `wl_touch`. The seat does not advertise the touch capability, and no
/// touch events are delivered. The object exists so `wl_seat.get_touch` has a
/// version-correct id that the client can release.
#[derive(Debug)]
pub struct WlTouch {
    id: WlTouchId,
    version: u32,
}

impl WlTouch {
    pub fn new(id: WlTouchId, version: u32) -> Self {
        Self { id, version }
    }
}

impl Object for WlTouch {
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
        use wl_touch::Request::*;

        match client.parse::<Self>(req, dispatcher)? {
            Release => {
                if self.version >= wl_touch::RELEASE_SINCE {
                    client.delete_id(&WlDisplay::default(), state, dispatcher, self.id.into());
                }
            }
        }
        Ok(())
    }
}
