//! `ext_image_copy_capture_cursor_session_v1`. `get_capture_session` may be sent once.
//! Destroying this object leaves the image-copy session and its frames in place.

use crate::{
    capture::{self, CaptureTarget},
    clients::{Client, ClientDispatcher, EventWriter, RawRequest, Resolved},
    object::{Object, Parser, RequestResult},
    protocol_generated::{
        ext_image_copy_capture_cursor_session_v1, ExtImageCopyCaptureCursorSessionV1Id,
        ExtImageCopyCaptureSessionV1Id, WaylandId, WlPointerId,
    },
    state::{GrabbedObject, State},
    wayland_objects::{
        ext_image_copy_capture_session_v1::ExtImageCopyCaptureSessionV1, wl_display::WlDisplay,
    },
};

#[derive(Debug)]
pub struct ExtImageCopyCaptureCursorSessionV1 {
    id: ExtImageCopyCaptureCursorSessionV1Id,
    target: CaptureTarget,
    pointer: Resolved<WlPointerId>,
    session: Option<Resolved<ExtImageCopyCaptureSessionV1Id>>,
    entered: bool,
    position: Option<(i32, i32)>,
    hotspot: Option<(i32, i32)>,
}

impl ExtImageCopyCaptureCursorSessionV1 {
    pub(crate) fn new(
        id: ExtImageCopyCaptureCursorSessionV1Id,
        target: CaptureTarget,
        pointer: Resolved<WlPointerId>,
    ) -> Self {
        Self {
            id,
            target,
            pointer,
            session: None,
            entered: false,
            position: None,
            hotspot: None,
        }
    }

    pub(crate) fn note_left(&mut self, dispatcher: &mut ClientDispatcher) {
        if !self.entered {
            return;
        }
        self.entered = false;
        self.send_event(ext_image_copy_capture_cursor_session_v1::Event::Leave, dispatcher);
    }

    pub(crate) fn note_cursor(
        &mut self,
        position: (i32, i32),
        hotspot: (i32, i32),
        dispatcher: &mut ClientDispatcher,
    ) {
        if !self.entered {
            self.entered = true;
            self.send_event(ext_image_copy_capture_cursor_session_v1::Event::Enter, dispatcher);
        }
        if self.position != Some(position) {
            self.position = Some(position);
            self.send_event(
                ext_image_copy_capture_cursor_session_v1::Event::Position {
                    x: position.0,
                    y: position.1,
                },
                dispatcher,
            );
        }
        if self.hotspot != Some(hotspot) {
            self.hotspot = Some(hotspot);
            self.send_event(
                ext_image_copy_capture_cursor_session_v1::Event::Hotspot {
                    x: hotspot.0,
                    y: hotspot.1,
                },
                dispatcher,
            );
        }
    }
}

impl Object for ExtImageCopyCaptureCursorSessionV1 {
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
        use ext_image_copy_capture_cursor_session_v1::Request::*;
        match client.parse::<Self>(req, dispatcher)? {
            Destroy => {
                client.delete_id(&WlDisplay::default(), state, dispatcher, self.id.into());
            }
            GetCaptureSession { session } => {
                if self.session.is_some() {
                    client.protocol_violation_disconnect(
                        dispatcher,
                        self.id.into(),
                        ext_image_copy_capture_cursor_session_v1::Error::DuplicateSession as u32,
                        "ext_image_copy_capture_cursor_session_v1.get_capture_session sent twice",
                    );
                    return Ok(());
                }
                if self.pointer.try_ref(state).is_none() {
                    client.protocol_violation_disconnect(
                        dispatcher,
                        self.id.into(),
                        crate::protocol_generated::wl_display::Error::InvalidObject as u32,
                        "pointer cursor capture session pointer is gone",
                    );
                    return Ok(());
                }
                let target = self.target;
                let pointer = self.pointer;
                let (resolved, _) = client.insert_object(
                    state,
                    ExtImageCopyCaptureSessionV1::new(session, target, false, Some(pointer)),
                );
                self.session = Some(resolved);
                state.capture.track_session(resolved, target, Some(pointer));
                let cursor_session = client.resolve_this(&self, state);
                let size = capture::image_copy_buffer_size(state, target, true);
                let placement = capture::cursor_placement(state, target);
                if let Some(session) = resolved.try_ref_mut(state) {
                    session.set_cursor_session(cursor_session);
                    session.send_constraints(size, dispatcher);
                }
                if let Some(placement) = placement {
                    if placement.inside {
                        self.note_cursor(
                            placement.position,
                            (placement.image.hotspot_x, placement.image.hotspot_y),
                            dispatcher,
                        );
                    }
                }
            }
        }
        Ok(())
    }
}
