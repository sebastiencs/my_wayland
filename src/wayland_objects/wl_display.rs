use crate::{
    clients::{Client, ClientDispatcher, RawRequest},
    object::{Object, Parser},
    protocol_generated::{wl_display, WaylandId, WlDisplayId},
    state::State,
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

    fn on(
        &mut self,
        _client: &mut Client,
        _state: &mut State,
        request: RawRequest,
        _dispatcher: &mut ClientDispatcher,
    ) {
        use wl_display::Request::*;

        match Self::parse_request(request).unwrap() {
            Sync { callback } => {
                let _callback = WlCallback::new(callback);
                // callback.done(dispatcher);
            }
            GetRegistry { registry: _ } => {}
        }
    }
}
