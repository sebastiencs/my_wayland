use crate::{
    clients::{Client, ClientDispatcher, EventWriter, RawRequest, Resolved},
    object::{Object, Parser, RequestResult},
    protocol_generated::{wl_pointer, WaylandId, WlPointerId, WlSurfaceId},
    seat::{
        pointer_scroll_protocol_events, PointerScrollAxis, PointerScrollDirection,
        PointerScrollFrame, PointerScrollSource, PointerScrollWireEvent,
    },
    state::{GrabbedObject, State},
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
        surface: Resolved<WlSurfaceId>,
        x: f32,
        y: f32,
        dispatcher: &mut ClientDispatcher,
    ) {
        self.send_event(
            wl_pointer::Event::Enter {
                serial,
                surface: surface.wayland_id(),
                surface_x: x,
                surface_y: y,
            },
            dispatcher,
        );
    }

    pub fn send_leave(
        &self,
        serial: u32,
        surface: Resolved<WlSurfaceId>,
        dispatcher: &mut ClientDispatcher,
    ) {
        self.send_event(
            wl_pointer::Event::Leave { serial, surface: surface.wayland_id() },
            dispatcher,
        );
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

    pub fn send_scroll(&self, frame: &PointerScrollFrame, dispatcher: &mut ClientDispatcher) {
        for event in pointer_scroll_protocol_events(self.version, frame) {
            self.send_event(scroll_wire_event(event), dispatcher);
        }
    }
}

fn scroll_wire_event(event: PointerScrollWireEvent) -> wl_pointer::Event {
    use wl_pointer::Event::*;

    match event {
        PointerScrollWireEvent::Source(source) => {
            AxisSource { axis_source: scroll_source_code(source) }
        }
        PointerScrollWireEvent::Discrete { axis, discrete } => {
            AxisDiscrete { axis: scroll_axis_code(axis), discrete }
        }
        PointerScrollWireEvent::Value120 { axis, value120 } => {
            AxisValue120 { axis: scroll_axis_code(axis), value120 }
        }
        PointerScrollWireEvent::RelativeDirection { axis, direction } => AxisRelativeDirection {
            axis: scroll_axis_code(axis),
            direction: scroll_direction_code(direction),
        },
        PointerScrollWireEvent::Axis { time, axis, value_bits } => Axis {
            time,
            axis: scroll_axis_code(axis),
            value: f32::from_bits(value_bits),
        },
        PointerScrollWireEvent::Stop { time, axis } => {
            AxisStop { time, axis: scroll_axis_code(axis) }
        }
        PointerScrollWireEvent::Frame => Frame,
    }
}

fn scroll_axis_code(axis: PointerScrollAxis) -> u32 {
    match axis {
        PointerScrollAxis::Vertical => wl_pointer::Axis::VerticalScroll as u32,
        PointerScrollAxis::Horizontal => wl_pointer::Axis::HorizontalScroll as u32,
    }
}

fn scroll_source_code(source: PointerScrollSource) -> u32 {
    match source {
        PointerScrollSource::Wheel => wl_pointer::AxisSource::Wheel as u32,
        PointerScrollSource::Finger => wl_pointer::AxisSource::Finger as u32,
        PointerScrollSource::Continuous => wl_pointer::AxisSource::Continuous as u32,
        PointerScrollSource::WheelTilt => wl_pointer::AxisSource::WheelTilt as u32,
    }
}

fn scroll_direction_code(direction: PointerScrollDirection) -> u32 {
    match direction {
        PointerScrollDirection::Identical => wl_pointer::AxisRelativeDirection::Identical as u32,
        PointerScrollDirection::Inverted => wl_pointer::AxisRelativeDirection::Inverted as u32,
    }
}

impl Object for WlPointer {
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
                    crate::drm::request_outputs_for_points(state, [pointer_pos], dispatcher);
                }
            }
            Release => {
                if let Some(pointer) = client.resolve(self.id, state) {
                    state.seat.unregister_pointer(pointer);
                }
                client.delete_id(&WlDisplay::default(), state, dispatcher, self.id.into());
            }
        }
        Ok(())
    }
}
