use crate::{
    clients::{Client, ClientDispatcher, RawRequest, Resolved},
    object::{Object, Parser, RequestResult},
    protocol_generated::{
        wl_display, wp_tearing_control_v1, WaylandId, WlSurfaceId, WpTearingControlV1Id,
    },
    state::{GrabbedObject, State},
    wayland_objects::{
        wl_display::WlDisplay,
        wl_surface::{PresentationHint, WlSurface},
    },
};

#[derive(Debug)]
pub struct WpTearingControlV1 {
    id: WpTearingControlV1Id,
    surface: Resolved<WlSurfaceId>,
}

impl WpTearingControlV1 {
    pub fn new(id: WpTearingControlV1Id, surface: Resolved<WlSurfaceId>) -> Self {
        Self { id, surface }
    }
}

impl Object for WpTearingControlV1 {
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
        use wp_tearing_control_v1::Request::*;

        match client.parse::<Self>(req, dispatcher)? {
            SetPresentationHint { hint } => {
                let Some(hint) = presentation_hint(hint) else {
                    client.send_protocol_error(
                        dispatcher,
                        self.id.into(),
                        wl_display::Error::Implementation as u32,
                        "unknown wp_tearing_control_v1 presentation hint",
                    );
                    return Ok(());
                };
                // The surface may already be gone. The protocol makes this object inert then.
                let Some(mut surface) = self.surface.try_grab(state) else {
                    return Ok(());
                };
                surface.set_pending_presentation_hint(hint);
            }
            Destroy => {
                let this = client.resolve(self.id, state).unwrap();
                if let Some(mut surface) = self.surface.try_grab(state) {
                    surface.clear_tearing_control_object(this);
                }
                client.delete_id(&WlDisplay::default(), state, dispatcher, self.id.into());
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
        let Some(object) = client.resolve(self.id, state) else {
            return;
        };
        if let Some(mut surface) = self.surface.try_grab(state) {
            surface.clear_tearing_control_object(object);
        }
    }
}

fn presentation_hint(hint: u32) -> Option<PresentationHint> {
    if hint == wp_tearing_control_v1::PresentationHint::Vsync as u32 {
        Some(PresentationHint::Vsync)
    } else if hint == wp_tearing_control_v1::PresentationHint::Async as u32 {
        Some(PresentationHint::Async)
    } else {
        None
    }
}
