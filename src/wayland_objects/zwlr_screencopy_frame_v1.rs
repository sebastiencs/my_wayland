//! One `zwlr_screencopy_frame_v1`. Copy is accepted once; pixels arrive in a later phase.
//! Losing the output fails the frame, as wlroots does in `wlr_screencopy_v1`.

use crate::{
    clients::{Client, ClientDispatcher, EventWriter, RawRequest, Resolved},
    object::{Object, Parser, RequestResult},
    protocol_generated::{
        wl_display, zwlr_screencopy_frame_v1, WaylandId, WlBufferId, ZwlrScreencopyFrameV1Id,
    },
    state::{GrabbedObject, OutputId, State},
    wayland_objects::wl_display::WlDisplay,
};

#[derive(Debug)]
pub struct ZwlrScreencopyFrameV1 {
    id: ZwlrScreencopyFrameV1Id,
    version: u32,
    output_id: OutputId,
    overlay_cursor: bool,
    region: Option<(i32, i32, i32, i32)>,
    used: bool,
    finished: bool,
    buffer: Option<Resolved<WlBufferId>>,
}

impl ZwlrScreencopyFrameV1 {
    pub(crate) fn new(
        id: ZwlrScreencopyFrameV1Id,
        version: u32,
        output_id: OutputId,
        overlay_cursor: bool,
        region: Option<(i32, i32, i32, i32)>,
    ) -> Self {
        Self {
            id,
            version,
            output_id,
            overlay_cursor,
            region,
            used: false,
            finished: false,
            buffer: None,
        }
    }

    pub(crate) fn overlay_cursor(&self) -> bool {
        self.overlay_cursor
    }

    pub(crate) fn region(&self) -> Option<(i32, i32, i32, i32)> {
        self.region
    }

    pub(crate) fn output_id(&self) -> OutputId {
        self.output_id
    }

    pub(crate) fn fail(&mut self, dispatcher: &mut ClientDispatcher) {
        if self.finished {
            return;
        }
        self.finished = true;
        self.send_event(zwlr_screencopy_frame_v1::Event::Failed, dispatcher);
    }

    fn delete_self(
        &self,
        client: &mut Client,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
    ) {
        if let Some(frame) = client.resolve(self.id, state) {
            state.capture.forget_screencopy(frame);
        }
        client.delete_id(&WlDisplay::default(), state, dispatcher, self.id.into());
    }

    fn copy(
        &mut self,
        client: &mut Client,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
        buffer: WlBufferId,
        with_damage: bool,
    ) {
        if with_damage && self.version < zwlr_screencopy_frame_v1::COPY_WITH_DAMAGE_SINCE {
            client.protocol_violation_disconnect(
                dispatcher,
                self.id.into(),
                wl_display::Error::InvalidMethod as u32,
                "zwlr_screencopy_frame_v1.copy_with_damage requires version 2",
            );
            return;
        }
        if self.used || self.finished {
            client.protocol_violation_disconnect(
                dispatcher,
                self.id.into(),
                zwlr_screencopy_frame_v1::Error::AlreadyUsed as u32,
                "zwlr_screencopy_frame_v1 copy was already requested",
            );
            return;
        }
        let Some(buffer) = client.resolve::<WlBufferId>(buffer, state) else {
            client.protocol_violation_disconnect(
                dispatcher,
                self.id.into(),
                wl_display::Error::InvalidObject as u32,
                "zwlr_screencopy_frame_v1 copy requires a live wl_buffer",
            );
            return;
        };
        let _ = buffer.rref(state);
        self.used = true;
        self.buffer = Some(buffer);
    }
}

impl Object for ZwlrScreencopyFrameV1 {
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
        use zwlr_screencopy_frame_v1::Request::*;
        match client.parse::<Self>(req, dispatcher)? {
            Destroy => self.delete_self(client, state, dispatcher),
            Copy { buffer } => self.copy(client, state, dispatcher, buffer, false),
            CopyWithDamage { buffer } => self.copy(client, state, dispatcher, buffer, true),
        }
        Ok(())
    }

    fn on_client_disconnect(
        &mut self,
        client: &mut Client,
        state: &mut State,
        _dispatcher: &mut ClientDispatcher,
    ) {
        if let Some(frame) = client.resolve(self.id, state) {
            state.capture.forget_screencopy(frame);
        }
    }
}
