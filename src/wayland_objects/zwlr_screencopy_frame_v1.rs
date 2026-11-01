//! One `zwlr_screencopy_frame_v1`. Copy is accepted once and queued on the shared capture backend.
//! Losing the output fails the frame, as wlroots does in `wlr_screencopy_v1`.

use crate::{
    capture::{self, CaptureCursor, CaptureQueueError, CaptureSample},
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

    /// wlroots sends one preferred SHM format. Both formats this compositor can write are
    /// advertised, ARGB8888 first, and `buffer_done` follows at version 3. DMA-BUF stays deferred.
    pub(crate) fn advertise(&mut self, width: i32, height: i32, dispatcher: &mut ClientDispatcher) {
        let (Ok(width), Ok(height)) = (u32::try_from(width), u32::try_from(height)) else {
            self.fail(dispatcher);
            return;
        };
        let stride = width.saturating_mul(4);
        for format in [crate::drm::ARGB8888.wayland_id, crate::drm::XRGB8888.wayland_id] {
            self.send_event(
                zwlr_screencopy_frame_v1::Event::Buffer { format, width, height, stride },
                dispatcher,
            );
        }
        if self.version >= zwlr_screencopy_frame_v1::BUFFER_DONE_SINCE {
            self.send_event(zwlr_screencopy_frame_v1::Event::BufferDone, dispatcher);
        }
    }

    /// Protocol order is `flags`, then `damage` when the copy asked for it, then `ready`.
    pub(crate) fn ready(&mut self, sample: &CaptureSample, dispatcher: &mut ClientDispatcher) {
        if self.finished {
            return;
        }
        self.finished = true;
        self.send_event(zwlr_screencopy_frame_v1::Event::Flags { flags: sample.flags }, dispatcher);
        if self.version >= zwlr_screencopy_frame_v1::DAMAGE_SINCE {
            for rect in &sample.damage {
                if rect.width <= 0 || rect.height <= 0 || rect.x < 0 || rect.y < 0 {
                    continue;
                }
                self.send_event(
                    zwlr_screencopy_frame_v1::Event::Damage {
                        x: rect.x as u32,
                        y: rect.y as u32,
                        width: rect.width as u32,
                        height: rect.height as u32,
                    },
                    dispatcher,
                );
            }
        }
        self.send_event(
            zwlr_screencopy_frame_v1::Event::Ready {
                tv_sec_hi: sample.tv_sec_hi,
                tv_sec_lo: sample.tv_sec_lo,
                tv_nsec: sample.tv_nsec,
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
        let rect = state.outputs.try_get(self.output_id).and_then(|output| {
            capture::output_capture_rect(
                output.current_mode.width,
                output.current_mode.height,
                output.scale,
                output.transform,
                self.region,
            )
        });
        let Some(rect) = rect else {
            self.used = true;
            self.fail(dispatcher);
            return;
        };
        let stride_matches = {
            let buffer_ref = buffer.rref(state);
            match buffer_ref.begin_capture_write(rect.width, rect.height) {
                Ok(write) => write.stride_bytes() == rect.width as usize * 4,
                Err(_) => false,
            }
        };
        if !stride_matches {
            client.protocol_violation_disconnect(
                dispatcher,
                self.id.into(),
                zwlr_screencopy_frame_v1::Error::InvalidBuffer as u32,
                "zwlr_screencopy_frame_v1 copy buffer has the wrong size, format, or stride",
            );
            return;
        }
        let cursor =
            if self.overlay_cursor { CaptureCursor::Painted } else { CaptureCursor::Hidden };
        match capture::submit_output_capture(
            state,
            dispatcher,
            self.output_id,
            buffer,
            cursor,
            self.region,
            with_damage,
        ) {
            Ok(job) => {
                self.used = true;
                self.buffer = Some(buffer);
                state.capture.bind_screencopy_job(self.id, job);
            }
            Err(CaptureQueueError::Buffer(_)) => {
                client.protocol_violation_disconnect(
                    dispatcher,
                    self.id.into(),
                    zwlr_screencopy_frame_v1::Error::InvalidBuffer as u32,
                    "zwlr_screencopy_frame_v1 copy buffer has the wrong size, format, or stride",
                );
            }
            Err(
                CaptureQueueError::SourceGone
                | CaptureQueueError::EmptyRegion
                | CaptureQueueError::BufferBusy
                | CaptureQueueError::Protected,
            ) => {
                self.used = true;
                self.fail(dispatcher);
            }
        }
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
