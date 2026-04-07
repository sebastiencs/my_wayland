use crate::{
    clients::{Client, ClientDispatcher, RawRequest, Resolved},
    globals::Global,
    object::{Object, Parser, RequestResult},
    protocol_generated::{wp_viewporter, Constants, NewId, WaylandId, WpViewporterId},
    state::{GrabbedObject, State},
    wayland_objects::{wl_surface::WlSurface, wp_viewport::WpViewport},
};

#[derive(Debug)]
pub struct WpViewporterGlobal;

impl WpViewporterGlobal {
    pub fn new() -> Box<dyn Global> {
        Box::new(Self)
    }
}

impl Global for WpViewporterGlobal {
    fn interface(&self) -> (&'static str, u32) {
        (WpViewporter::INTERFACE_NAME, WpViewporter::INTERFACE_VERSION)
    }

    fn bind(
        &mut self,
        client: &mut Client,
        state: &mut State,
        id: NewId,
        _version: u32,
        _dispatcher: &mut ClientDispatcher,
    ) {
        client.insert_object(state, WpViewporter { id: id.into() });
    }
}

#[derive(Debug)]
pub struct WpViewporter {
    id: WpViewporterId,
}

impl Object for WpViewporter {
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
        use wp_viewporter::Request::*;

        match client.parse::<Self>(req).unwrap() {
            Destroy => {
                use crate::wayland_objects::wl_display::WlDisplay;
                client.delete_id(&WlDisplay::default(), state, dispatcher, self.id.into());
            }
            GetViewport { id, surface } => {
                let surface = client.resolve_or_disconnect(surface, state, dispatcher)?;
                {
                    let surface = surface.rref(state);
                    if surface.has_viewport_object() {
                        client.send_protocol_error(
                            dispatcher,
                            self.id.into(),
                            wp_viewporter::Error::ViewportExists as u32,
                            "wl_surface already has a wp_viewport object",
                        );
                        return Ok(());
                    }
                }
                let (viewport, _) = client.insert_object(state, WpViewport::new(id, surface));
                surface
                    .grab(state)
                    .set_viewport_object(viewport)
                    .expect("viewport existence was checked before insertion");
            }
        }
        Ok(())
    }
}
