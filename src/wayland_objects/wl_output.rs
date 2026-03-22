//! `wl_output` global: one instance per DRM device in this compositor, describing the
//! currently selected connector (see [`crate::drm::DrmOutput::in_use`]). A full multi-output
//! setup should register one global per logical connector.

use crate::{
    clients::{Client, ClientDispatcher, EventWriter, RawRequest},
    drm::Drm,
    globals::Global,
    object::{Object, Parser},
    protocol_generated::{wl_output, Constants, NewId, WaylandId, WlOutputId},
    state::{DrmId, State},
};

#[derive(Debug)]
pub struct WlOutputGlobal {
    drm_id: DrmId,
}

impl WlOutputGlobal {
    pub fn new(drm_id: DrmId) -> Box<dyn Global> {
        Box::new(Self { drm_id })
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
        _version: u32,
        dispatcher: &mut ClientDispatcher,
    ) {
        let id = WlOutputId::from(id);
        let wl_output = WlOutput { id, drm_id: self.drm_id };

        wl_output.send_geometry(state, dispatcher);
        wl_output.send_mode(state, dispatcher);
        // TODO: Check versions
        wl_output.send_scale(dispatcher);
        wl_output.send_name(state, dispatcher);
        wl_output.send_done(dispatcher);

        client.insert_object(state, wl_output);
    }
}

#[derive(Debug)]
pub struct WlOutput {
    id: WlOutputId,
    drm_id: DrmId,
}

impl Object for WlOutput {
    fn id(&self) -> WaylandId {
        self.id.into()
    }

    fn on(
        &mut self,
        client: &mut Client,
        _: &mut State,
        req: RawRequest,
        _: &mut ClientDispatcher,
    ) {
        use wl_output::Request::*;

        match client.parse::<Self>(req).unwrap() {
            Release => todo!(),
        }
    }
}

impl WlOutput {
    fn get_drm<'a>(&self, state: &'a State) -> &'a Drm {
        state.drm.get(self.drm_id)
    }

    pub fn send_geometry(&self, state: &State, dispatcher: &mut ClientDispatcher) {
        let drm = self.get_drm(state);
        let in_use = &drm.output.in_use;

        let geometry = wl_output::Event::Geometry {
            x: 0,
            y: 0,
            physical_width: in_use.connector.mm_width as i32,
            physical_height: in_use.connector.mm_height as i32,
            subpixel: wl_output::Subpixel::Unknown as i32,
            make: in_use.connector.manufacturer_id.clone(),
            model: in_use.connector.model_name.clone().unwrap_or_default(),
            transform: wl_output::Transform::Normal as i32,
        };
        self.send_event(geometry, dispatcher);
    }

    pub fn send_mode(&self, state: &State, dispatcher: &mut ClientDispatcher) {
        let drm = self.get_drm(state);
        let in_use = &drm.output.in_use;

        let mode = in_use.connector.preferred_mode.as_ref().unwrap();
        let (width, height) = mode.size();

        let refresh = {
            let clock_milli_hertz = mode.clock() as u64 * 1_000_000;
            let (_, _, htotal) = mode.hsync();
            let (_, _, vtotal) = mode.vsync();
            ((clock_milli_hertz / htotal as u64) + (vtotal as u64 / 2)) / vtotal as u64
        };

        let mode = wl_output::Event::Mode {
            flags: wl_output::Mode::Current as u32,
            width: width as i32,
            height: height as i32,
            refresh: refresh as i32,
        };
        self.send_event(mode, dispatcher);
    }

    pub fn send_scale(&self, dispatcher: &mut ClientDispatcher) {
        let scale = wl_output::Event::Scale { factor: 1 }; // TODO: Support other
        self.send_event(scale, dispatcher);
    }

    pub fn send_name(&self, state: &State, dispatcher: &mut ClientDispatcher) {
        let drm = self.get_drm(state);
        let name = drm.output.in_use.connector.interface_name.clone();
        self.send_event(wl_output::Event::Name { name }, dispatcher);
    }

    pub fn send_done(&self, dispatcher: &mut ClientDispatcher) {
        self.send_event(wl_output::Event::Done, dispatcher);
    }
}
