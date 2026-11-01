//! Opaque `ext_image_capture_source_v1`. Destroying it leaves sessions valid.
//! The source stops only when its output or toplevel disappears.

use crate::{
    capture::CaptureTarget,
    clients::{Client, ClientDispatcher, RawRequest},
    object::{Object, Parser, RequestResult},
    protocol_generated::{ext_image_capture_source_v1, ExtImageCaptureSourceV1Id, WaylandId},
    state::{GrabbedObject, State},
    wayland_objects::wl_display::WlDisplay,
};

#[derive(Debug)]
pub struct ExtImageCaptureSourceV1 {
    id: ExtImageCaptureSourceV1Id,
    target: CaptureTarget,
}

impl ExtImageCaptureSourceV1 {
    pub(crate) fn new(id: ExtImageCaptureSourceV1Id, target: CaptureTarget) -> Self {
        Self { id, target }
    }

    pub(crate) fn target(&self) -> CaptureTarget {
        self.target
    }
}

impl Object for ExtImageCaptureSourceV1 {
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
        use ext_image_capture_source_v1::Request::*;
        match client.parse::<Self>(req, dispatcher)? {
            Destroy => {
                client.delete_id(&WlDisplay::default(), state, dispatcher, self.id.into());
            }
        }
        Ok(())
    }
}
