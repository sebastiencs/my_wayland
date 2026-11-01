//! `ext_image_copy_capture_manager_v1`. Unpublished until capture validation.
//! Unknown option bits raise `invalid_option`. `paint_cursors` selects a cursor-painted copy.

use crate::{
    capture::{self, CaptureTarget},
    clients::{Client, ClientDispatcher, RawRequest},
    globals::Global,
    object::{Object, Parser, RequestResult},
    protocol_generated::{
        ext_image_copy_capture_manager_v1, wl_display, Constants, ExtImageCaptureSourceV1Id,
        ExtImageCopyCaptureManagerV1Id, NewId, WaylandId, WlPointerId,
    },
    state::{GrabbedObject, State},
    wayland_objects::{
        ext_image_copy_capture_cursor_session_v1::ExtImageCopyCaptureCursorSessionV1,
        ext_image_copy_capture_session_v1::ExtImageCopyCaptureSessionV1, wl_display::WlDisplay,
    },
};

#[derive(Debug)]
pub struct ExtImageCopyCaptureManagerV1Global;

impl ExtImageCopyCaptureManagerV1Global {
    pub fn new() -> Box<dyn Global> {
        Box::new(Self)
    }
}

impl Global for ExtImageCopyCaptureManagerV1Global {
    fn interface(&self) -> (&'static str, u32) {
        (
            ExtImageCopyCaptureManagerV1::INTERFACE_NAME,
            ExtImageCopyCaptureManagerV1::INTERFACE_VERSION,
        )
    }

    fn bind(
        &mut self,
        client: &mut Client,
        state: &mut State,
        id: NewId,
        _version: u32,
        _dispatcher: &mut ClientDispatcher,
    ) {
        let id = ExtImageCopyCaptureManagerV1Id::from(id);
        let _ = client.insert_object(state, ExtImageCopyCaptureManagerV1::new(id));
    }
}

#[derive(Debug)]
pub struct ExtImageCopyCaptureManagerV1 {
    id: ExtImageCopyCaptureManagerV1Id,
}

impl ExtImageCopyCaptureManagerV1 {
    pub fn new(id: ExtImageCopyCaptureManagerV1Id) -> Self {
        Self { id }
    }

    fn live_source(
        &self,
        client: &mut Client,
        state: &State,
        dispatcher: &mut ClientDispatcher,
        source: ExtImageCaptureSourceV1Id,
    ) -> Option<CaptureTarget> {
        let Some(source) = client.resolve(source, state) else {
            client.protocol_violation_disconnect(
                dispatcher,
                self.id.into(),
                wl_display::Error::InvalidObject as u32,
                "ext_image_copy_capture requires a live image capture source",
            );
            return None;
        };
        let target = source.rref(state).target();
        if !capture::target_is_live(state, target) {
            client.protocol_violation_disconnect(
                dispatcher,
                self.id.into(),
                wl_display::Error::InvalidObject as u32,
                "ext_image_copy_capture source is no longer available",
            );
            return None;
        }
        Some(target)
    }
}

impl Object for ExtImageCopyCaptureManagerV1 {
    fn id(&self) -> WaylandId {
        self.id.into()
    }

    fn on(
        self: GrabbedObject<Self>,
        client: &mut Client,
        state: &mut State,
        req: RawRequest,
        dispatcher: &mut ClientDispatcher,
    ) -> RequestResult {
        use ext_image_copy_capture_manager_v1::Request::*;
        match client.parse::<Self>(req, dispatcher)? {
            Destroy => {
                client.delete_id(&WlDisplay::default(), state, dispatcher, self.id.into());
            }
            CreateSession { session, source, options } => {
                let paint = ext_image_copy_capture_manager_v1::Options::PaintCursors as u32;
                if options & !paint != 0 {
                    client.protocol_violation_disconnect(
                        dispatcher,
                        self.id.into(),
                        ext_image_copy_capture_manager_v1::Error::InvalidOption as u32,
                        "ext_image_copy_capture_manager_v1.create_session options are invalid",
                    );
                    return Ok(());
                }
                let Some(target) = self.live_source(client, state, dispatcher, source) else {
                    return Ok(());
                };
                let (resolved, _) = client.insert_object(
                    state,
                    ExtImageCopyCaptureSessionV1::new(session, target, options & paint != 0, None),
                );
                state.capture.track_session(resolved, target, None);
                let size = capture::image_copy_buffer_size(state, target, false);
                if let Some(session) = resolved.try_ref_mut(state) {
                    session.send_constraints(size, dispatcher);
                }
            }
            CreatePointerCursorSession { session, source, pointer } => {
                let Some(target) = self.live_source(client, state, dispatcher, source) else {
                    return Ok(());
                };
                let Some(pointer) = client.resolve::<WlPointerId>(pointer, state) else {
                    client.protocol_violation_disconnect(
                        dispatcher,
                        self.id.into(),
                        wl_display::Error::InvalidObject as u32,
                        "pointer cursor capture requires a live wl_pointer",
                    );
                    return Ok(());
                };
                if pointer.try_ref(state).is_none() {
                    client.protocol_violation_disconnect(
                        dispatcher,
                        self.id.into(),
                        wl_display::Error::InvalidObject as u32,
                        "pointer cursor capture requires a live wl_pointer",
                    );
                    return Ok(());
                }
                let _ = client.insert_object(
                    state,
                    ExtImageCopyCaptureCursorSessionV1::new(session, target, pointer),
                );
            }
        }
        Ok(())
    }
}
