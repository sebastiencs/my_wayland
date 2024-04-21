use crate::{
    clients::{Client, ClientDispatcher, EventWriter, RawRequest},
    globals::Global,
    object::{Object, Parser},
    protocol_generated::{wp_presentation, Constants, NewId, WaylandId, WpPresentationId},
    state::State,
};

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
        _version: u32,
        dispatcher: &mut ClientDispatcher,
    ) {
        let id = WpPresentationId::from(id);
        let wp_presentation = WpPresentation { id };

        wp_presentation.send_clock_id(dispatcher);

        client.insert_object(state, wp_presentation);
    }
}

#[derive(Debug)]
pub struct WpPresentation {
    id: WpPresentationId,
}

impl WpPresentation {
    pub fn new(id: WpPresentationId) -> Self {
        Self { id }
    }
}

impl Object for WpPresentation {
    fn id(&self) -> WaylandId {
        self.id.into()
    }

    fn on(&mut self, _: &mut Client, _: &mut State, req: RawRequest, _: &mut ClientDispatcher) {
        use wp_presentation::Request::*;

        match Self::parse(req).unwrap() {
            Destroy => todo!(),
            Feedback { surface, callback } => todo!(),
        }
    }
}

impl WpPresentation {
    pub fn send_clock_id(&self, dispatcher: &mut ClientDispatcher) {
        let clock_id = wp_presentation::Event::ClockId { clk_id: libc::CLOCK_MONOTONIC as u32 };
        self.send_event(clock_id, dispatcher);
    }
}
