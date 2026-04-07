use crate::{
    clients::{Client, ClientDispatcher, EventWriter, RawRequest, Resolved},
    globals::Global,
    object::{Object, Parser, RequestResult},
    protocol_generated::{
        wl_display, wp_presentation, Constants, NewId, WaylandId, WlSurfaceId,
        WpPresentationFeedbackId, WpPresentationId,
    },
    state::{GrabbedObject, State},
};

use super::{wl_display::WlDisplay, wp_presentation_feedback::WpPresentationFeedback};

#[derive(Debug)]
pub struct WpPresentationGlobal;

impl WpPresentationGlobal {
    pub fn new() -> Box<dyn Global> {
        Box::new(Self {})
    }
}

impl Global for WpPresentationGlobal {
    fn interface(&self) -> (&'static str, u32) {
        (WpPresentation::INTERFACE_NAME, WpPresentation::INTERFACE_VERSION)
    }
    fn bind(
        &mut self,
        client: &mut Client,
        state: &mut State,
        id: NewId,
        version: u32,
        dispatcher: &mut ClientDispatcher,
    ) {
        let id = WpPresentationId::from(id);
        let wp_presentation = WpPresentation { id, version };

        wp_presentation.send_clock_id(dispatcher);

        client.insert_object(state, wp_presentation);
    }
}

#[derive(Debug)]
pub struct WpPresentation {
    id: WpPresentationId,
    version: u32,
}

impl WpPresentation {
    pub fn new(id: WpPresentationId, version: u32) -> Self {
        Self { id, version }
    }
}

impl Object for WpPresentation {
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
        use wp_presentation::Request::*;

        match client.parse::<Self>(req).unwrap() {
            Destroy => {
                client.delete_id(&WlDisplay::default(), state, dispatcher, self.id.into());
            }
            Feedback { surface, callback } => {
                self.on_feedback(client, state, dispatcher, surface, callback)?;
            }
        }
        Ok(())
    }
}

impl WpPresentation {
    pub fn send_clock_id(&self, dispatcher: &mut ClientDispatcher) {
        let clock_id = wp_presentation::Event::ClockId { clk_id: libc::CLOCK_MONOTONIC as u32 };
        self.send_event(clock_id, dispatcher);
    }

    fn on_feedback(
        &mut self,
        client: &mut Client,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
        surface: WlSurfaceId,
        callback: WpPresentationFeedbackId,
    ) -> RequestResult {
        let surface = client.resolve_or_disconnect(surface, state, dispatcher)?;

        client.insert_object(state, WpPresentationFeedback::new(callback, self.version));
        let feedback = client.resolve(callback, state).unwrap();

        let mut surface = surface.grab(state);
        surface.queue_presentation_feedback(feedback, self.version, None);

        Ok(())
    }
}
