//! `ext_output_image_capture_source_manager_v1`. Unpublished until capture validation.

use crate::{
    capture::CaptureTarget,
    clients::{Client, ClientDispatcher, RawRequest},
    globals::Global,
    object::{Object, Parser, RequestResult},
    protocol_generated::{
        ext_output_image_capture_source_manager_v1, Constants,
        ExtOutputImageCaptureSourceManagerV1Id, NewId, WaylandId,
    },
    state::{GrabbedObject, State},
    wayland_objects::{
        ext_image_capture_source_v1::ExtImageCaptureSourceV1, wl_display::WlDisplay,
        zwlr_screencopy_manager_v1::live_output,
    },
};

#[derive(Debug)]
pub struct ExtOutputImageCaptureSourceManagerV1Global;

impl ExtOutputImageCaptureSourceManagerV1Global {
    pub fn new() -> Box<dyn Global> {
        Box::new(Self)
    }
}

impl Global for ExtOutputImageCaptureSourceManagerV1Global {
    fn interface(&self) -> (&'static str, u32) {
        (
            ExtOutputImageCaptureSourceManagerV1::INTERFACE_NAME,
            ExtOutputImageCaptureSourceManagerV1::INTERFACE_VERSION,
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
        let id = ExtOutputImageCaptureSourceManagerV1Id::from(id);
        let _ = client.insert_object(state, ExtOutputImageCaptureSourceManagerV1::new(id));
    }
}

#[derive(Debug)]
pub struct ExtOutputImageCaptureSourceManagerV1 {
    id: ExtOutputImageCaptureSourceManagerV1Id,
}

impl ExtOutputImageCaptureSourceManagerV1 {
    pub fn new(id: ExtOutputImageCaptureSourceManagerV1Id) -> Self {
        Self { id }
    }
}

impl Object for ExtOutputImageCaptureSourceManagerV1 {
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
        use ext_output_image_capture_source_manager_v1::Request::*;
        match client.parse::<Self>(req, dispatcher)? {
            Destroy => {
                client.delete_id(&WlDisplay::default(), state, dispatcher, self.id.into());
            }
            CreateSource { source, output } => {
                let Some(output_id) =
                    live_output(client, state, dispatcher, self.id.into(), output)
                else {
                    return Ok(());
                };
                let _ = client.insert_object(
                    state,
                    ExtImageCaptureSourceV1::new(source, CaptureTarget::Output(output_id)),
                );
            }
        }
        Ok(())
    }
}
