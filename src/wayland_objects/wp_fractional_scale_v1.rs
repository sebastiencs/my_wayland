use crate::{
    clients::{Client, ClientDispatcher, EventWriter, RawRequest},
    object::{Object, Parser},
    protocol_generated::{wp_fractional_scale_v1, WaylandId, WpFractionalScaleV1Id},
    state::{ObjectId, State},
    wayland_objects::{wl_display::WlDisplay, wl_surface::WlSurface},
};

#[derive(Debug)]
pub struct WpFractionalScaleV1 {
    id: WpFractionalScaleV1Id,
    surface_object_id: ObjectId,
}

impl WpFractionalScaleV1 {
    pub fn new(id: WpFractionalScaleV1Id, surface_object_id: ObjectId) -> Self {
        Self { id, surface_object_id }
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
    ) {
        use wp_fractional_scale_v1::Request::*;

        match client.parse::<Self>(req).unwrap() {
            Destroy => {
                if let Some(mut surface) = state
                    .objects
                    .try_grab_object::<WlSurface>(self.surface_object_id)
                {
                    surface.clear_fractional_scale_object(client.object_id(self.id));
                }
                client.delete_id(&WlDisplay::default(), state, dispatcher, self.id.into());
            }
        }
    }

    fn on_client_disconnect(
        &mut self,
        client: &mut Client,
        state: &mut State,
        _dispatcher: &mut ClientDispatcher,
    ) {
        let Some(object_id) = client.try_object_id(self.id, state) else {
            return;
        };
        if let Some(mut surface) = state
            .objects
            .try_grab_object::<WlSurface>(self.surface_object_id)
        {
            surface.clear_fractional_scale_object(object_id);
        }
    }
}
