//! One `ext_image_copy_capture_session_v1`. At most one frame exists at a time.
//! Constraint events arrive with the capture backend. `stopped` is sent once.

use crate::{
    capture::CaptureTarget,
    clients::{Client, ClientDispatcher, EventWriter, RawRequest, Resolved},
    object::{Object, Parser, RequestResult},
    protocol_generated::{
        ext_image_copy_capture_session_v1, ExtImageCopyCaptureFrameV1Id,
        ExtImageCopyCaptureSessionV1Id, WaylandId, WlPointerId,
    },
    state::{GrabbedObject, State},
    wayland_objects::{
        ext_image_copy_capture_frame_v1::ExtImageCopyCaptureFrameV1, wl_display::WlDisplay,
    },
};

#[derive(Debug)]
pub struct ExtImageCopyCaptureSessionV1 {
    id: ExtImageCopyCaptureSessionV1Id,
    target: CaptureTarget,
    paint_cursors: bool,
    pointer: Option<Resolved<WlPointerId>>,
    frame: Option<Resolved<ExtImageCopyCaptureFrameV1Id>>,
    stopped: bool,
}

impl ExtImageCopyCaptureSessionV1 {
    pub(crate) fn new(
        id: ExtImageCopyCaptureSessionV1Id,
        target: CaptureTarget,
        paint_cursors: bool,
        pointer: Option<Resolved<WlPointerId>>,
    ) -> Self {
        Self {
            id,
            target,
            paint_cursors,
            pointer,
            frame: None,
            stopped: false,
        }
    }

    pub(crate) fn target(&self) -> CaptureTarget {
        self.target
    }

    pub(crate) fn paint_cursors(&self) -> bool {
        self.paint_cursors
    }

    pub(crate) fn pointer(&self) -> Option<Resolved<WlPointerId>> {
        self.pointer
    }

    pub(crate) fn is_stopped(&self) -> bool {
        self.stopped
    }

    pub(crate) fn mark_stopped(&mut self, dispatcher: &mut ClientDispatcher) {
        if self.stopped {
            return;
        }
        self.stopped = true;
        self.send_event(ext_image_copy_capture_session_v1::Event::Stopped, dispatcher);
    }

    pub(crate) fn clear_frame(&mut self, frame: ExtImageCopyCaptureFrameV1Id) {
        if self
            .frame
            .is_some_and(|existing| existing.wayland_id() == frame)
        {
            self.frame = None;
        }
    }

    fn delete_self(
        &self,
        client: &mut Client,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
    ) {
        if let Some(session) = client.resolve(self.id, state) {
            state.capture.forget_session(session);
        }
        client.delete_id(&WlDisplay::default(), state, dispatcher, self.id.into());
    }
}

impl Object for ExtImageCopyCaptureSessionV1 {
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
        use ext_image_copy_capture_session_v1::Request::*;
        match client.parse::<Self>(req, dispatcher)? {
            Destroy => self.delete_self(client, state, dispatcher),
            CreateFrame { frame } => {
                if self
                    .frame
                    .is_some_and(|existing| existing.try_ref(state).is_some())
                {
                    client.protocol_violation_disconnect(
                        dispatcher,
                        self.id.into(),
                        ext_image_copy_capture_session_v1::Error::DuplicateFrame as u32,
                        "ext_image_copy_capture_session_v1.create_frame while a frame exists",
                    );
                    return Ok(());
                }
                let target = self.target;
                let pointer = self.pointer;
                let session = client.resolve_this(&self, state);
                let (resolved, _) =
                    client.insert_object(state, ExtImageCopyCaptureFrameV1::new(frame, session));
                self.frame = Some(resolved);
                state.capture.track_ext_frame(resolved, target, pointer);
            }
        }
        Ok(())
    }

    fn on_client_disconnect(
        &mut self,
        client: &mut Client,
        state: &mut State,
        _dispatcher: &mut ClientDispatcher,
    ) {
        if let Some(session) = client.resolve(self.id, state) {
            state.capture.forget_session(session);
        }
    }
}
