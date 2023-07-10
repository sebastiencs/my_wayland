use crate::{
    clients::{Client, RawRequest},
    object::{Object, Parser},
    protocol_generated::{wl_display, WaylandId, WlDisplayId},
    state::Dispatcher,
    wayland_objects::wl_callback::WlCallback,
};

#[derive(Debug)]
pub struct WlDisplay {
    pub id: WlDisplayId,
}

impl Default for WlDisplay {
    fn default() -> Self {
        Self { id: 1.into() }
    }
}

impl Object for WlDisplay {
    fn id(&self) -> WaylandId {
        self.id.into()
    }

    fn on(&mut self, client: &mut Client, request: RawRequest, dispatcher: &mut Dispatcher) {
        use wl_display::Request::*;

        match Self::parse_request(request).unwrap() {
            Sync { callback } => {
                let callback = WlCallback::new(callback);
                callback.done(dispatcher);
            }
            GetRegistry { registry } => {}
        }
    }
}
