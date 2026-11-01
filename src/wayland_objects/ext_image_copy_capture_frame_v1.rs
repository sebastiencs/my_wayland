//! One `ext_image_copy_capture_frame_v1`. Capture is accepted once after a buffer is attached.
//! A stopped source fails an in-flight capture with `failure_reason.stopped`.

use crate::{
    clients::{Client, ClientDispatcher, EventWriter, RawRequest, Resolved},
    object::{Object, Parser, RequestResult},
    protocol_generated::{
        ext_image_copy_capture_frame_v1, wl_display, ExtImageCopyCaptureFrameV1Id,
        ExtImageCopyCaptureSessionV1Id, WaylandId, WlBufferId,
    },
    state::{GrabbedObject, State},
    wayland_objects::wl_display::WlDisplay,
};

#[derive(Debug)]
pub struct ExtImageCopyCaptureFrameV1 {
    id: ExtImageCopyCaptureFrameV1Id,
    session: Resolved<ExtImageCopyCaptureSessionV1Id>,
    buffer: Option<Resolved<WlBufferId>>,
    captured: bool,
    finished: bool,
}

impl ExtImageCopyCaptureFrameV1 {
    pub(crate) fn new(
        id: ExtImageCopyCaptureFrameV1Id,
        session: Resolved<ExtImageCopyCaptureSessionV1Id>,
    ) -> Self {
        Self { id, session, buffer: None, captured: false, finished: false }
    }

    pub(crate) fn fail_stopped(&mut self, dispatcher: &mut ClientDispatcher) {
        if !self.captured || self.finished {
            return;
        }
        self.finished = true;
        self.send_event(
            ext_image_copy_capture_frame_v1::Event::Failed {
                reason: ext_image_copy_capture_frame_v1::FailureReason::Stopped as u32,
            },
            dispatcher,
        );
    }

    fn delete_self(
        &self,
        client: &mut Client,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
    ) {
        if let Some(mut session) = self.session.try_ref_mut(state) {
            session.clear_frame(self.id);
        }
        if let Some(frame) = client.resolve(self.id, state) {
            state.capture.forget_ext_frame(frame);
        }
        client.delete_id(&WlDisplay::default(), state, dispatcher, self.id.into());
    }

    fn reject_after_capture(&self, client: &mut Client, dispatcher: &mut ClientDispatcher) -> bool {
        if self.captured || self.finished {
            client.protocol_violation_disconnect(
                dispatcher,
                self.id.into(),
                ext_image_copy_capture_frame_v1::Error::AlreadyCaptured as u32,
                "ext_image_copy_capture_frame_v1 request after capture",
            );
            true
        } else {
            false
        }
    }
}

impl Object for ExtImageCopyCaptureFrameV1 {
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
        use ext_image_copy_capture_frame_v1::Request::*;
        match client.parse::<Self>(req, dispatcher)? {
            Destroy => self.delete_self(client, state, dispatcher),
            AttachBuffer { buffer } => {
                if self.reject_after_capture(client, dispatcher) {
                    return Ok(());
                }
                let Some(buffer) = client.resolve(buffer, state) else {
                    client.protocol_violation_disconnect(
                        dispatcher,
                        self.id.into(),
                        wl_display::Error::InvalidObject as u32,
                        "ext_image_copy_capture_frame_v1.attach_buffer requires a live wl_buffer",
                    );
                    return Ok(());
                };
                self.buffer = Some(buffer);
            }
            DamageBuffer { x, y, width, height } => {
                if self.reject_after_capture(client, dispatcher) {
                    return Ok(());
                }
                if x < 0 || y < 0 || width <= 0 || height <= 0 {
                    client.protocol_violation_disconnect(
                        dispatcher,
                        self.id.into(),
                        ext_image_copy_capture_frame_v1::Error::InvalidBufferDamage as u32,
                        "ext_image_copy_capture_frame_v1.damage_buffer region is invalid",
                    );
                    return Ok(());
                }
            }
            Capture => {
                if self.reject_after_capture(client, dispatcher) {
                    return Ok(());
                }
                if self.buffer.is_none() {
                    client.protocol_violation_disconnect(
                        dispatcher,
                        self.id.into(),
                        ext_image_copy_capture_frame_v1::Error::NoBuffer as u32,
                        "ext_image_copy_capture_frame_v1.capture without attach_buffer",
                    );
                    return Ok(());
                }
                let stopped = self
                    .session
                    .try_ref(state)
                    .is_some_and(|session| session.is_stopped());
                self.captured = true;
                if stopped {
                    self.fail_stopped(dispatcher);
                }
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
        if let Some(frame) = client.resolve(self.id, state) {
            state.capture.forget_ext_frame(frame);
        }
    }
}
