use crate::{
    clients::{Client, ClientDispatcher, EventWriter, RawRequest},
    object::{Object, Parser},
    protocol_generated::{wl_pointer, WaylandId, WlPointerId, WlSurfaceId},
    state::State,
    wayland_objects::wl_display::WlDisplay,
};

#[derive(Debug)]
pub struct WlPointer {
    id: WlPointerId,
    /// `wl_seat` version the client used when creating this object (`get_pointer`).
    version: u32,
}

impl WlPointer {
    pub fn new(id: WlPointerId, version: u32) -> Self {
        Self { id, version }
    }

    pub fn send_enter(
        &self,
        serial: u32,
        surface: WlSurfaceId,
        x: f32,
        y: f32,
        dispatcher: &mut ClientDispatcher,
    ) {
        self.send_event(
            wl_pointer::Event::Enter { serial, surface, surface_x: x, surface_y: y },
            dispatcher,
        );
    }

    pub fn send_leave(&self, serial: u32, surface: WlSurfaceId, dispatcher: &mut ClientDispatcher) {
        self.send_event(wl_pointer::Event::Leave { serial, surface }, dispatcher);
    }

    pub fn send_motion(&self, time: u32, x: f32, y: f32, dispatcher: &mut ClientDispatcher) {
        self.send_event(wl_pointer::Event::Motion { time, surface_x: x, surface_y: y }, dispatcher);
    }

    pub fn send_button(
        &self,
        serial: u32,
        time: u32,
        button: u32,
        state: u32,
        dispatcher: &mut ClientDispatcher,
    ) {
        self.send_event(wl_pointer::Event::Button { serial, time, button, state }, dispatcher);
    }

    pub fn send_frame(&self, dispatcher: &mut ClientDispatcher) {
        if self.version < wl_pointer::FRAME_SINCE {
            return;
        }
        self.send_event(wl_pointer::Event::Frame, dispatcher);
    }
}

impl Object for WlPointer {
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
        use wl_pointer::Request::*;

        match client.parse::<Self>(req).unwrap() {
            SetCursor { serial, surface, hotspot_x, hotspot_y } => {
                let surface = match surface.to_opt() {
                    Some(surface) => {
                        Some(client.resolve_or_disconnect(surface, state, dispatcher)?)
                    }
                    None => None,
                };
                if !state
                    .seat
                    .pointer_cursor_request_allowed(client.client_id(), serial)
                {
                    return Ok(());
                }
                let cursor_changed = match surface {
                    None => state.seat.clear_cursor_for_client(client.client_id()),
                    Some(surface) => {
                        {
                            let mut surface_object = surface.ref_mut(state);
                            if surface_object.set_cursor_role(dispatcher).is_err() {
                                return Ok(());
                            }
                        }
                        state.seat.set_cursor_surface(surface, hotspot_x, hotspot_y)
                    }
                };
                if cursor_changed {
                    dispatcher.dispatch_to(
                        client.client_id(),
                        crate::clients::ClientAction::RefreshSurfaceOutputs,
                    );
                    let pointer_pos = state.seat.pointer_position();
                    crate::drm::schedule_outputs_for_points(state, [pointer_pos], dispatcher);
                }
            }
            Release => {
                state.seat.unregister_pointer(client.client_id(), self.id);
                client.delete_id(&WlDisplay::default(), state, dispatcher, self.id.into());
            }
        }
        Ok(())
    }
}
