//! `zwlr_screencopy_manager_v1`. Unpublished until capture pixels and security tests pass.
//! Buffer parameters and `ready` are sent by a later phase; this object only creates frames.

use crate::{
    capture::{self, CaptureTarget},
    clients::{Client, ClientDispatcher, RawRequest},
    globals::Global,
    object::{Object, Parser, RequestResult},
    protocol_generated::{
        wl_display, zwlr_screencopy_manager_v1, Constants, NewId, WaylandId, WlOutputId,
        ZwlrScreencopyFrameV1Id, ZwlrScreencopyManagerV1Id,
    },
    state::{GrabbedObject, OutputId, State},
    wayland_objects::{
        wl_display::WlDisplay, wl_output::WlOutput, zwlr_screencopy_frame_v1::ZwlrScreencopyFrameV1,
    },
};

#[derive(Debug)]
pub struct ZwlrScreencopyManagerV1Global;

impl ZwlrScreencopyManagerV1Global {
    pub fn new() -> Box<dyn Global> {
        Box::new(Self)
    }
}

impl Global for ZwlrScreencopyManagerV1Global {
    fn interface(&self) -> (&'static str, u32) {
        (ZwlrScreencopyManagerV1::INTERFACE_NAME, ZwlrScreencopyManagerV1::INTERFACE_VERSION)
    }

    fn bind(
        &mut self,
        client: &mut Client,
        state: &mut State,
        id: NewId,
        version: u32,
        _dispatcher: &mut ClientDispatcher,
    ) {
        let id = ZwlrScreencopyManagerV1Id::from(id);
        let _ = client.insert_object(state, ZwlrScreencopyManagerV1::new(id, version));
    }
}

#[derive(Debug)]
pub struct ZwlrScreencopyManagerV1 {
    id: ZwlrScreencopyManagerV1Id,
    version: u32,
}

impl ZwlrScreencopyManagerV1 {
    pub fn new(id: ZwlrScreencopyManagerV1Id, version: u32) -> Self {
        Self { id, version }
    }

    fn delete_self(
        &self,
        client: &mut Client,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
    ) {
        client.delete_id(&WlDisplay::default(), state, dispatcher, self.id.into());
    }
}

impl Object for ZwlrScreencopyManagerV1 {
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
        use zwlr_screencopy_manager_v1::Request::*;
        match client.parse::<Self>(req, dispatcher)? {
            Destroy => self.delete_self(client, state, dispatcher),
            CaptureOutput { frame, overlay_cursor, output } => {
                start_frame(&self, client, state, dispatcher, frame, overlay_cursor, output, None);
            }
            CaptureOutputRegion { frame, overlay_cursor, output, x, y, width, height } => {
                start_frame(
                    &self,
                    client,
                    state,
                    dispatcher,
                    frame,
                    overlay_cursor,
                    output,
                    Some((x, y, width, height)),
                );
            }
        }
        Ok(())
    }
}

fn start_frame(
    manager: &ZwlrScreencopyManagerV1,
    client: &mut Client,
    state: &mut State,
    dispatcher: &mut ClientDispatcher,
    frame: ZwlrScreencopyFrameV1Id,
    overlay_cursor: i32,
    output: WlOutputId,
    region: Option<(i32, i32, i32, i32)>,
) {
    let Some(output_id) = live_output(client, state, dispatcher, manager.id.into(), output) else {
        return;
    };
    let (resolved, _) = client.insert_object(
        state,
        ZwlrScreencopyFrameV1::new(frame, manager.version, output_id, overlay_cursor != 0, region),
    );
    state.capture.track_screencopy(resolved, output_id);
}

pub(crate) fn live_output(
    client: &mut Client,
    state: &State,
    dispatcher: &mut ClientDispatcher,
    object_id: WaylandId,
    output: WlOutputId,
) -> Option<OutputId> {
    let Some(output) = client.resolve(output, state) else {
        client.protocol_violation_disconnect(
            dispatcher,
            object_id,
            wl_display::Error::InvalidObject as u32,
            "capture output is not a live wl_output",
        );
        return None;
    };
    let Some(output_id) = output.rref(state).output_id() else {
        client.protocol_violation_disconnect(
            dispatcher,
            object_id,
            wl_display::Error::InvalidObject as u32,
            "capture output has been removed",
        );
        return None;
    };
    if !capture::target_is_live(state, CaptureTarget::Output(output_id)) {
        client.protocol_violation_disconnect(
            dispatcher,
            object_id,
            wl_display::Error::InvalidObject as u32,
            "capture output is no longer in the layout",
        );
        return None;
    }
    Some(output_id)
}
