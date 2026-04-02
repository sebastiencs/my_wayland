use crate::{
    clients::{Client, ClientDispatcher, EventWriter, RawRequest},
    globals::Global,
    object::{Object, Parser},
    protocol_generated::{
        wl_display, wp_presentation, Constants, NewId, WaylandId, WlSurfaceId,
        WpPresentationFeedbackId, WpPresentationId,
    },
    state::State,
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
        &mut self,
        client: &mut Client,
        state: &mut State,
        req: RawRequest,
        dispatcher: &mut ClientDispatcher,
    ) -> crate::object::RequestResult {
        use wp_presentation::Request::*;

        match client.parse::<Self>(req).unwrap() {
            Destroy => {
                client.delete_id(&WlDisplay::default(), state, dispatcher, self.id.into());
            }
            Feedback { surface, callback } => {
                self.on_feedback(client, state, dispatcher, surface, callback);
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
    ) {
        if !client.object_is(surface, state) {
            client.protocol_violation_disconnect(
                dispatcher,
                self.id.into(),
                wl_display::Error::InvalidObject as u32,
                "wp_presentation.feedback requires a live wl_surface",
            );
            return;
        }

        client.insert_object(state, WpPresentationFeedback::new(callback, self.version));

        let mut surface = client.grab_object(surface, state);
        surface.queue_presentation_feedback(callback, self.version, None);
    }
}
