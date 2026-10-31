use crate::{
    clients::{Client, ClientDispatcher, RawRequest, Resolved},
    object::{Object, Parser, RequestResult},
    protocol_generated::{wp_viewport, WaylandId, WlSurfaceId, WpViewportId},
    state::{GrabbedObject, State},
    transform::{RectF, SizeI},
    wayland_objects::wl_surface::WlSurface,
};

#[derive(Debug)]
pub struct WpViewport {
    id: WpViewportId,
    surface: Resolved<WlSurfaceId>,
}

impl WpViewport {
    pub fn new(id: WpViewportId, surface: Resolved<WlSurfaceId>) -> Self {
        Self { id, surface }
    }
}

impl Object for WpViewport {
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
        use wp_viewport::Request::*;

        match client.parse::<Self>(req, dispatcher)? {
            Destroy => {
                let this = client.resolve(self.id, state).unwrap();
                if let Some(mut surface) = self.surface.try_grab(state) {
                    surface.clear_viewport_object(this);
                }
                use crate::wayland_objects::wl_display::WlDisplay;
                client.delete_id(&WlDisplay::default(), state, dispatcher, self.id.into());
            }
            SetSource { x, y, width, height } => {
                let Some(mut surface) = self.surface.try_grab(state) else {
                    client.send_protocol_error(
                        dispatcher,
                        self.id.into(),
                        wp_viewport::Error::NoSurface as u32,
                        "wp_viewport surface no longer exists",
                    );
                    return Ok(());
                };
                if x == -1.0 && y == -1.0 && width == -1.0 && height == -1.0 {
                    surface.pending_viewport_mut().src = None;
                    return Ok(());
                }
                if x < 0.0 || y < 0.0 || width <= 0.0 || height <= 0.0 {
                    client.send_protocol_error(
                        dispatcher,
                        self.id.into(),
                        wp_viewport::Error::BadValue as u32,
                        "wp_viewport source must have positive size and non-negative origin",
                    );
                    return Ok(());
                }
                surface.pending_viewport_mut().src = Some(RectF { x, y, width, height });
            }
            SetDestination { width, height } => {
                let Some(mut surface) = self.surface.try_grab(state) else {
                    client.send_protocol_error(
                        dispatcher,
                        self.id.into(),
                        wp_viewport::Error::NoSurface as u32,
                        "wp_viewport surface no longer exists",
                    );
                    return Ok(());
                };
                if width == -1 && height == -1 {
                    surface.pending_viewport_mut().dst = None;
                    return Ok(());
                }
                if width <= 0 || height <= 0 {
                    client.send_protocol_error(
                        dispatcher,
                        self.id.into(),
                        wp_viewport::Error::BadValue as u32,
                        "wp_viewport destination must have positive size",
                    );
                    return Ok(());
                }
                surface.pending_viewport_mut().dst = Some(SizeI { width, height });
            }
        }
        Ok(())
    }
}
