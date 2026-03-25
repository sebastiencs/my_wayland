use crate::{
    clients::{Client, ClientDispatcher, EventWriter, RawRequest},
    object::Object,
    protocol_generated::{
        wp_presentation_feedback, WaylandId, WlOutputId, WpPresentationFeedbackId,
    },
    state::State,
};

#[derive(Debug)]
pub struct WpPresentationFeedback {
    id: WpPresentationFeedbackId,
    version: u32,
}

impl WpPresentationFeedback {
    pub fn new(id: WpPresentationFeedbackId, version: u32) -> Self {
        Self { id, version }
    }

    pub fn send_sync_output(&self, output: WlOutputId, dispatcher: &mut ClientDispatcher) {
        self.send_event(wp_presentation_feedback::Event::SyncOutput { output }, dispatcher);
    }

    pub fn send_presented(
        &self,
        tv_sec_hi: u32,
        tv_sec_lo: u32,
        tv_nsec: u32,
        refresh: u32,
        seq_hi: u32,
        seq_lo: u32,
        flags: u32,
        dispatcher: &mut ClientDispatcher,
    ) {
        self.send_event(
            wp_presentation_feedback::Event::Presented {
                tv_sec_hi,
                tv_sec_lo,
                tv_nsec,
                refresh,
                seq_hi,
                seq_lo,
                flags,
            },
            dispatcher,
        );
    }

    pub fn send_discarded(&self, dispatcher: &mut ClientDispatcher) {
        self.send_event(wp_presentation_feedback::Event::Discarded, dispatcher);
    }

    pub fn version(&self) -> u32 {
        self.version
    }
}

impl Object for WpPresentationFeedback {
    fn id(&self) -> WaylandId {
        self.id.into()
    }

    fn on(&mut self, _: &mut Client, _: &mut State, _: RawRequest, _: &mut ClientDispatcher) {}
}
