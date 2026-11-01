//! `ext_foreign_toplevel_image_capture_source_manager_v1`, advertised at version 1.
//! A closed handle cannot become a source.

use crate::{
    capture::{self, CaptureTarget},
    clients::{Client, ClientDispatcher, RawRequest},
    globals::Global,
    object::{Object, Parser, RequestResult},
    protocol_generated::{
        ext_foreign_toplevel_image_capture_source_manager_v1, wl_display, Constants,
        ExtForeignToplevelImageCaptureSourceManagerV1Id, NewId, WaylandId,
    },
    state::{GrabbedObject, State},
    wayland_objects::{
        ext_image_capture_source_v1::ExtImageCaptureSourceV1, wl_display::WlDisplay,
    },
};

#[derive(Debug)]
pub struct ExtForeignToplevelImageCaptureSourceManagerV1Global;

impl ExtForeignToplevelImageCaptureSourceManagerV1Global {
    pub fn new() -> Box<dyn Global> {
        Box::new(Self)
    }
}

impl Global for ExtForeignToplevelImageCaptureSourceManagerV1Global {
    fn interface(&self) -> (&'static str, u32) {
        (
            ExtForeignToplevelImageCaptureSourceManagerV1::INTERFACE_NAME,
            ExtForeignToplevelImageCaptureSourceManagerV1::INTERFACE_VERSION,
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
        let id = ExtForeignToplevelImageCaptureSourceManagerV1Id::from(id);
        let _ = client.insert_object(state, ExtForeignToplevelImageCaptureSourceManagerV1::new(id));
    }
}

#[derive(Debug)]
pub struct ExtForeignToplevelImageCaptureSourceManagerV1 {
    id: ExtForeignToplevelImageCaptureSourceManagerV1Id,
}

impl ExtForeignToplevelImageCaptureSourceManagerV1 {
    pub fn new(id: ExtForeignToplevelImageCaptureSourceManagerV1Id) -> Self {
        Self { id }
    }
}

impl Object for ExtForeignToplevelImageCaptureSourceManagerV1 {
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
        use ext_foreign_toplevel_image_capture_source_manager_v1::Request::*;
        match client.parse::<Self>(req, dispatcher)? {
            Destroy => {
                client.delete_id(&WlDisplay::default(), state, dispatcher, self.id.into());
            }
            CreateSource { source, toplevel_handle } => {
                let Some(handle) = client.resolve(toplevel_handle, state) else {
                    client.protocol_violation_disconnect(
                        dispatcher,
                        self.id.into(),
                        wl_display::Error::InvalidObject as u32,
                        "toplevel capture source requires a live foreign toplevel handle",
                    );
                    return Ok(());
                };
                let key = {
                    let handle = handle.rref(state);
                    if handle.is_closed() {
                        client.protocol_violation_disconnect(
                            dispatcher,
                            self.id.into(),
                            wl_display::Error::InvalidObject as u32,
                            "toplevel capture source handle is closed",
                        );
                        return Ok(());
                    }
                    handle.key()
                };
                if !capture::target_is_live(state, CaptureTarget::Toplevel(key)) {
                    client.protocol_violation_disconnect(
                        dispatcher,
                        self.id.into(),
                        wl_display::Error::InvalidObject as u32,
                        "toplevel capture source is no longer mapped",
                    );
                    return Ok(());
                }
                let _ = client.insert_object(
                    state,
                    ExtImageCaptureSourceV1::new(source, CaptureTarget::Toplevel(key)),
                );
            }
        }
        Ok(())
    }
}
