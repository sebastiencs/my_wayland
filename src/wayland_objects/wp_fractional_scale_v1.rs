use crate::{
    clients::{Client, ClientDispatcher, EventWriter, RawRequest, Resolved},
    object::{Object, Parser},
    protocol_generated::{wp_fractional_scale_v1, WaylandId, WlSurfaceId, WpFractionalScaleV1Id},
    state::State,
    wayland_objects::{wl_display::WlDisplay, wl_surface::WlSurface},
};

#[derive(Debug)]
pub struct WpFractionalScaleV1 {
    id: WpFractionalScaleV1Id,
    surface: Resolved<WlSurfaceId>,
}

impl WpFractionalScaleV1 {
    pub fn new(id: WpFractionalScaleV1Id, surface: Resolved<WlSurfaceId>) -> Self {
        Self { id, surface }
    }

    pub fn send_preferred_scale(&self, scale: u32, dispatcher: &mut ClientDispatcher) {
        self.send_event(wp_fractional_scale_v1::Event::PreferredScale { scale }, dispatcher);
    }
}

impl Object for WpFractionalScaleV1 {
    fn id(&self) -> WaylandId {
        self.id.into()
    }

    fn on(
        &mut self,
        client: &mut Client,
        state: &mut State,
        req: RawRequest,
        dispatcher: &mut ClientDispatcher,
    ) -> crate::object::RequestResult {
        use wp_fractional_scale_v1::Request::*;

        match client.parse::<Self>(req).unwrap() {
            Destroy => {
                let object = client.resolve(self.id, state).unwrap();
                if let Some(mut surface) = self.surface.try_grab(state) {
                    surface.clear_fractional_scale_object(object);
                }
                client.delete_id(&WlDisplay::default(), state, dispatcher, self.id.into());
            }
        }
        Ok(())
    }

    fn on_client_disconnect(
        &mut self,
        client: &mut Client,
        state: &mut State,
        _dispatcher: &mut ClientDispatcher,
    ) {
        let Some(object_id) = client.resolve(self.id, state) else {
            return;
        };
        if let Some(mut surface) = self.surface.try_grab(state) {
            surface.clear_fractional_scale_object(object_id);
        }
    }
}
