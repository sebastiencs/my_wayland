//! One `ext_image_copy_capture_frame_v1`. Capture is accepted once after a buffer is attached.
//! A stopped source fails an in-flight capture with `failure_reason.stopped`.

use crate::{
    capture::{self, CaptureCursor, CaptureQueueError, CaptureSample},
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
        self.fail_reason(
            ext_image_copy_capture_frame_v1::FailureReason::Stopped as u32,
            dispatcher,
        );
    }

    pub(crate) fn fail_reason(&mut self, reason: u32, dispatcher: &mut ClientDispatcher) {
        if !self.captured || self.finished {
            return;
        }
        self.finished = true;
        self.send_event(ext_image_copy_capture_frame_v1::Event::Failed { reason }, dispatcher);
    }

    /// Pixels are already top-left and scanout-oriented, so the transform is normal.
    /// wlroots can send the output transform because it copies the swapchain as stored.
    pub(crate) fn ready(&mut self, sample: &CaptureSample, dispatcher: &mut ClientDispatcher) {
        if self.finished {
            return;
        }
        self.finished = true;
        for rect in &sample.damage {
            if rect.width <= 0 || rect.height <= 0 || rect.x < 0 || rect.y < 0 {
                continue;
            }
            self.send_event(
                ext_image_copy_capture_frame_v1::Event::Damage {
                    x: rect.x,
                    y: rect.y,
                    width: rect.width,
                    height: rect.height,
                },
                dispatcher,
            );
        }
        self.send_event(
            ext_image_copy_capture_frame_v1::Event::Transform { transform: 0 },
            dispatcher,
        );
        self.send_event(
            ext_image_copy_capture_frame_v1::Event::PresentationTime {
                tv_sec_hi: sample.tv_sec_hi,
                tv_sec_lo: sample.tv_sec_lo,
                tv_nsec: sample.tv_nsec,
            },
            dispatcher,
        );
        self.send_event(ext_image_copy_capture_frame_v1::Event::Ready, dispatcher);
    }

    fn queue_capture(
        &mut self,
        _client: &mut Client,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
    ) {
        use crate::capture::CaptureTarget;
        let Some(session) = self.session.try_ref(state) else {
            self.fail_stopped(dispatcher);
            return;
        };
        let target = session.target();
        let cursor_capture = session.pointer().is_some();
        let paint = session.paint_cursors();
        let cursor_session = session.cursor_session();
        let advertised = session.advertised_size();
        let size = capture::image_copy_buffer_size(state, target, cursor_capture);
        if advertised != size {
            if let Some(session) = self.session.try_ref_mut(state) {
                session.send_constraints(size, dispatcher);
            }
        }
        let Some((width, height)) = size else {
            self.fail_reason(
                ext_image_copy_capture_frame_v1::FailureReason::Unknown as u32,
                dispatcher,
            );
            return;
        };
        let Some(buffer) = self.buffer else {
            self.fail_reason(
                ext_image_copy_capture_frame_v1::FailureReason::BufferConstraints as u32,
                dispatcher,
            );
            return;
        };
        let matches = {
            let buffer_ref = buffer.rref(state);
            match buffer_ref.begin_capture_write(width, height) {
                Ok(write) => write.stride_bytes() == (width as usize).saturating_mul(4),
                Err(_) => false,
            }
        };
        if !matches {
            self.fail_reason(
                ext_image_copy_capture_frame_v1::FailureReason::BufferConstraints as u32,
                dispatcher,
            );
            return;
        }
        if cursor_capture {
            self.capture_cursor(state, dispatcher, target, cursor_session, buffer, width, height);
            return;
        }
        let cursor = if paint { CaptureCursor::Painted } else { CaptureCursor::Hidden };
        let queued = match target {
            CaptureTarget::Output(output_id) => capture::submit_output_capture(
                state, dispatcher, output_id, buffer, cursor, None, true,
            ),
            CaptureTarget::Toplevel(key) => {
                capture::submit_toplevel_capture(state, dispatcher, key, buffer, cursor, true)
            }
        };
        match queued {
            Ok(job) => state.capture.bind_ext_frame_job(self.id, job),
            Err(CaptureQueueError::Protected | CaptureQueueError::SourceGone) => {
                self.fail_stopped(dispatcher);
            }
            Err(CaptureQueueError::Buffer(_) | CaptureQueueError::EmptyRegion) => {
                self.fail_reason(
                    ext_image_copy_capture_frame_v1::FailureReason::BufferConstraints as u32,
                    dispatcher,
                );
            }
            Err(CaptureQueueError::BufferBusy) => {
                self.fail_reason(
                    ext_image_copy_capture_frame_v1::FailureReason::Unknown as u32,
                    dispatcher,
                );
            }
        }
    }

    fn capture_cursor(
        &mut self,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
        target: capture::CaptureTarget,
        cursor_session: Option<
            Resolved<crate::protocol_generated::ExtImageCopyCaptureCursorSessionV1Id>,
        >,
        buffer: Resolved<WlBufferId>,
        width: i32,
        height: i32,
    ) {
        let Some(placement) = capture::cursor_placement(state, target) else {
            if let Some(cursor) = cursor_session.and_then(|cursor| cursor.try_ref_mut(state)) {
                cursor.note_left(dispatcher);
            }
            self.fail_reason(
                ext_image_copy_capture_frame_v1::FailureReason::Unknown as u32,
                dispatcher,
            );
            return;
        };
        if !placement.inside || placement.image.width != width || placement.image.height != height {
            if let Some(cursor) = cursor_session.and_then(|cursor| cursor.try_ref_mut(state)) {
                if !placement.inside {
                    cursor.note_left(dispatcher);
                }
            }
            let reason = if placement.inside {
                ext_image_copy_capture_frame_v1::FailureReason::BufferConstraints
            } else {
                ext_image_copy_capture_frame_v1::FailureReason::Unknown
            };
            self.fail_reason(reason as u32, dispatcher);
            return;
        }
        let written = {
            let buffer_ref = buffer.rref(state);
            buffer_ref
                .begin_capture_write(width, height)
                .and_then(|write| {
                    write.write_top_left(&placement.image.argb, placement.image.stride)
                })
                .is_ok()
        };
        if !written {
            self.fail_reason(
                ext_image_copy_capture_frame_v1::FailureReason::Unknown as u32,
                dispatcher,
            );
            return;
        }
        if let Some(cursor) = cursor_session.and_then(|cursor| cursor.try_ref_mut(state)) {
            cursor.note_cursor(
                placement.position,
                (placement.image.hotspot_x, placement.image.hotspot_y),
                dispatcher,
            );
        }
        let now = capture::capture_timestamp_now();
        self.ready(
            &CaptureSample {
                rect: capture::CapturePixelRect { x: 0, y: 0, width, height },
                stride: (width as usize).saturating_mul(4),
                flags: 0,
                tv_sec_hi: (now.sec >> 32) as u32,
                tv_sec_lo: now.sec as u32,
                tv_nsec: now.nsec,
                damage: vec![capture::CapturePixelRect { x: 0, y: 0, width, height }],
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
                    return Ok(());
                }
                self.queue_capture(client, state, dispatcher);
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
