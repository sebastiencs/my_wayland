//! `ext_image_copy_capture_cursor_session_v1`. `get_capture_session` may be sent once.
//! Destroying this object leaves the image-copy session and its frames in place.

use crate::{
    capture::CaptureTarget,
    clients::{Client, ClientDispatcher, RawRequest, Resolved},
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
}

impl ExtImageCopyCaptureCursorSessionV1 {
    pub(crate) fn new(
        id: ExtImageCopyCaptureCursorSessionV1Id,
        target: CaptureTarget,
        pointer: Resolved<WlPointerId>,
    ) -> Self {
        Self { id, target, pointer, session: None }
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
            }
        }
        Ok(())
    }
}
