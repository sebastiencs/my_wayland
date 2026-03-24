use crate::{
    clients::{Client, ClientDispatcher, RawRequest},
    globals::Global,
    object::{Object, Parser},
    protocol_generated::{wp_viewporter, Constants, NewId, WaylandId, WpViewporterId},
    state::State,
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
        &mut self,
        client: &mut Client,
        state: &mut State,
        req: RawRequest,
        dispatcher: &mut ClientDispatcher,
    ) {
        use wp_viewporter::Request::*;

        match client.parse::<Self>(req).unwrap() {
            Destroy => {
                use crate::wayland_objects::wl_display::WlDisplay;
                client.delete_id(&WlDisplay::default(), state, dispatcher, self.id.into());
            }
            GetViewport { id, surface } => {
                if !client.contains_object(surface, state) {
                    client.protocol_violation_disconnect(
                        dispatcher,
                        self.id.into(),
                        crate::protocol_generated::wl_display::Error::InvalidObject as u32,
                        "invalid wl_surface for wp_viewporter.get_viewport",
                    );
                    return;
                }
                let surface_oid = client.object_id(surface);
                {
                    let mut surface_obj = state.objects.grab(surface_oid);
                    let surface = surface_obj.downcast_mut::<WlSurface>().unwrap();
                    if surface.set_viewport_object(id).is_err() {
                        client.send_protocol_error(
                            dispatcher,
                            self.id.into(),
                            wp_viewporter::Error::ViewportExists as u32,
                            "wl_surface already has a wp_viewport object",
                        );
                        return;
                    }
                }
                client.insert_object(state, WpViewport::new(id, surface_oid));
            }
        }
    }
}
