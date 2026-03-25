use crate::{
    clients::{Client, ClientDispatcher, EventWriter, RawRequest},
    object::{Object, Parser},
    protocol_generated::{zxdg_output_v1, WaylandId, ZxdgOutputV1Id},
    state::{Output, OutputId, State},
};

#[derive(Debug)]
pub struct ZxdgOutputV1 {
    id: ZxdgOutputV1Id,
    output_id: Option<OutputId>,
    version: u32,
}

impl ZxdgOutputV1 {
    pub fn new(id: ZxdgOutputV1Id, output_id: OutputId, version: u32) -> Self {
        Self { id, output_id: Some(output_id), version }
    }

    pub fn matches_output(&self, output_id: OutputId) -> bool {
        self.output_id == Some(output_id)
    }

    pub fn mark_removed(&mut self) {
        self.output_id = None;
    }

    fn output<'a>(&self, state: &'a State) -> Option<&'a Output> {
        self.output_id
            .and_then(|output_id| state.outputs.try_get(output_id))
    }

    pub fn send_state(&self, state: &State, dispatcher: &mut ClientDispatcher) {
        if self.output(state).is_none() {
            return;
        }
        self.send_logical_position(state, dispatcher);
        self.send_logical_size(state, dispatcher);
        self.send_name(state, dispatcher);
        self.send_description(state, dispatcher);
        self.send_done(dispatcher);
    }

    pub fn send_logical_position(&self, state: &State, dispatcher: &mut ClientDispatcher) {
        let Some(output) = self.output(state) else {
            return;
        };
        self.send_event(
            zxdg_output_v1::Event::LogicalPosition { x: output.x, y: output.y },
            dispatcher,
        );
    }

    pub fn send_logical_size(&self, state: &State, dispatcher: &mut ClientDispatcher) {
        let Some(output) = self.output(state) else {
            return;
        };
        let (width, height) = output.logical_size();
        self.send_event(zxdg_output_v1::Event::LogicalSize { width, height }, dispatcher);
    }

    pub fn send_name(&self, state: &State, dispatcher: &mut ClientDispatcher) {
        if self.version < 2 {
            return;
        }
        let Some(output) = self.output(state) else {
            return;
        };
        let name = output.name.clone();
        self.send_event(zxdg_output_v1::Event::Name { name }, dispatcher);
    }

    pub fn send_description(&self, state: &State, dispatcher: &mut ClientDispatcher) {
        if self.version < 2 {
            return;
        }
        let Some(output) = self.output(state) else {
            return;
        };
        let description = output.description.clone();
        self.send_event(zxdg_output_v1::Event::Description { description }, dispatcher);
    }

    pub fn send_done(&self, dispatcher: &mut ClientDispatcher) {
        if self.version >= 3 {
            return;
        }
        self.send_event(zxdg_output_v1::Event::Done, dispatcher);
    }
}

impl Object for ZxdgOutputV1 {
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
        use zxdg_output_v1::Request::*;

        match client.parse::<Self>(req).unwrap() {
            Destroy => {
                use crate::wayland_objects::wl_display::WlDisplay;
                client.delete_id(&WlDisplay::default(), state, dispatcher, self.id.into());
            }
        }
    }
}
