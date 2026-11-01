use crate::{
    clients::{Client, ClientDispatcher, RawRequest, Resolved},
    object::{Object, Parser, RequestResult},
    protocol_generated::{zwp_idle_inhibitor_v1, WaylandId, WlSurfaceId, ZwpIdleInhibitorV1Id},
    state::{GrabbedObject, State},
    wayland_objects::wl_display::WlDisplay,
};

/// One `zwp_idle_inhibitor_v1` created by a single `create_inhibitor` request.
///
/// Surface destruction drops the association and leaves this object in place.
/// wlroots does the same: `idle_inhibitor_v1_destroy` clears the resource user
/// data without `wl_resource_destroy`, so a later client destroy is inert.
/// Destroying the object here would send `wl_display.delete_id` for an id the
/// client still owns.
#[derive(Debug)]
pub struct ZwpIdleInhibitorV1 {
    id: ZwpIdleInhibitorV1Id,
    surface: Option<Resolved<WlSurfaceId>>,
}

impl ZwpIdleInhibitorV1 {
    pub(crate) fn new(id: ZwpIdleInhibitorV1Id, surface: Resolved<WlSurfaceId>) -> Self {
        Self { id, surface: Some(surface) }
    }

    pub(crate) fn surface(&self) -> Option<Resolved<WlSurfaceId>> {
        self.surface
    }

    fn detach(&mut self, client: &Client, state: &mut State) {
        let Some(surface) = self.surface.take() else {
            return;
        };
        let Some(inhibitor) = client.resolve(self.id, state) else {
            return;
        };
        if let Some(mut surface) = surface.try_ref_mut(state) {
            surface.remove_idle_inhibitor(inhibitor);
        }
    }
}

/// The surface is going away. Drop every association without destroying the
/// client-owned inhibitor objects.
pub(crate) fn detach_destroyed_surface(
    inhibitors: Vec<Resolved<ZwpIdleInhibitorV1Id>>,
    state: &mut State,
) {
    for inhibitor in inhibitors {
        if let Some(mut inhibitor) = inhibitor.try_ref_mut(state) {
            inhibitor.surface = None;
        }
    }
}

impl Object for ZwpIdleInhibitorV1 {
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
        match client.parse::<Self>(req, dispatcher)? {
            zwp_idle_inhibitor_v1::Request::Destroy => {
                self.detach(client, state);
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
        self.detach(client, state);
    }
}
