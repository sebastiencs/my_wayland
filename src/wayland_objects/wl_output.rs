//! `wl_output` global backed by compositor-facing logical outputs. The DRM backend still owns
//! presentation/runtime state, but protocol-facing geometry and metadata live in
//! [`crate::state::Output`].

use crate::{
    clients::{Client, ClientDispatcher, EventWriter, RawRequest},
    globals::Global,
    object::{Object, Parser, RequestResult},
    protocol_generated::{wl_output, Constants, NewId, WaylandId, WlOutputId},
    state::{GrabbedObject, Output, OutputId, State},
};

#[derive(Debug)]
pub struct WlOutputGlobal {
    output_id: OutputId,
}

impl WlOutputGlobal {
    pub fn new(output_id: OutputId) -> Box<dyn Global> {
        Box::new(Self { output_id })
    }
}

impl Global for WlOutputGlobal {
    fn interface(&self) -> (&'static str, u32) {
        (WlOutput::INTERFACE_NAME, WlOutput::INTERFACE_VERSION)
    }
    fn bind(
        &mut self,
        client: &mut Client,
        state: &mut State,
        id: NewId,
        version: u32,
        dispatcher: &mut ClientDispatcher,
    ) {
        let id = WlOutputId::from(id);
        let _ =
            client.insert_object(state, WlOutput { id, output_id: Some(self.output_id), version });
        let Some(wl_output) = client.resolve(id, state) else {
            return;
        };
        let wl_output = wl_output.rref(state);
        wl_output.send_state(state, dispatcher);
        client.replay_surface_output_for_output(state, dispatcher, self.output_id);
        client.refresh_workspace_protocols(state, dispatcher);
    }
}

#[derive(Debug)]
pub struct WlOutput {
    id: WlOutputId,
    output_id: Option<OutputId>,
    version: u32,
}

impl Object for WlOutput {
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
        use wl_output::Request::*;

        match client.parse::<Self>(req).unwrap() {
            Release => {
                use crate::wayland_objects::wl_display::WlDisplay;
                client.delete_id(&WlDisplay::default(), state, dispatcher, self.id.into());
            }
        }
        Ok(())
    }
}

impl WlOutput {
    pub fn wl_output_id(&self) -> WlOutputId {
        self.id
    }

    pub fn output_id(&self) -> Option<OutputId> {
        self.output_id
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
        self.send_geometry(state, dispatcher);
        self.send_mode(state, dispatcher);
        self.send_scale(state, dispatcher);
        self.send_name(state, dispatcher);
        self.send_description(state, dispatcher);
        self.send_done(dispatcher);
    }

    pub fn send_geometry(&self, state: &State, dispatcher: &mut ClientDispatcher) {
        let Some(output) = self.output(state) else {
            return;
        };

        let geometry = wl_output::Event::Geometry {
            x: output.x,
            y: output.y,
            physical_width: output.physical_width,
            physical_height: output.physical_height,
            subpixel: wl_output::Subpixel::Unknown as i32,
            make: output.make.clone(),
            model: output.model.clone(),
            transform: output.transform,
        };
        self.send_event(geometry, dispatcher);
    }

    pub fn send_mode(&self, state: &State, dispatcher: &mut ClientDispatcher) {
        let Some(output) = self.output(state) else {
            return;
        };
        let mode = &output.current_mode;
        let mut flags = wl_output::Mode::Current as u32;
        if output
            .preferred_mode
            .as_ref()
            .is_some_and(|preferred| preferred == mode)
        {
            flags |= wl_output::Mode::Preferred as u32;
        }

        let mode = wl_output::Event::Mode {
            flags,
            width: mode.width,
            height: mode.height,
            refresh: mode.refresh_millihz,
        };
        self.send_event(mode, dispatcher);
    }

    pub fn send_scale(&self, state: &State, dispatcher: &mut ClientDispatcher) {
        if self.version < wl_output::SCALE_SINCE {
            return;
        }
        let scale = wl_output::Event::Scale {
            factor: self
                .output(state)
                .map(|output| output.legacy_scale())
                .unwrap_or(1),
        };
        self.send_event(scale, dispatcher);
    }

    pub fn send_name(&self, state: &State, dispatcher: &mut ClientDispatcher) {
        if self.version < wl_output::NAME_SINCE {
            return;
        }
        let Some(output) = self.output(state) else {
            return;
        };
        let name = output.name.clone();
        self.send_event(wl_output::Event::Name { name }, dispatcher);
    }

    pub fn send_description(&self, state: &State, dispatcher: &mut ClientDispatcher) {
        if self.version < wl_output::DESCRIPTION_SINCE {
            return;
        }
        let Some(output) = self.output(state) else {
            return;
        };
        let description = output.description.clone();
        self.send_event(wl_output::Event::Description { description }, dispatcher);
    }

    pub fn send_done(&self, dispatcher: &mut ClientDispatcher) {
        if self.version < wl_output::DONE_SINCE {
            return;
        }
        self.send_event(wl_output::Event::Done, dispatcher);
    }
}
