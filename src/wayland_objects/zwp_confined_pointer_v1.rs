use crate::{
    clients::{Client, ClientDispatcher, EventWriter, RawRequest, Resolved},
    object::{Object, Parser, RequestResult},
    protocol_generated::{
        zwp_confined_pointer_v1, WaylandId, WlPointerId, WlRegionId, WlSurfaceId,
        ZwpConfinedPointerV1Id,
    },
    region::RegionValue,
    state::{GrabbedObject, State},
    wayland_objects::{
        pointer_constraint::{
            ConstraintLifetime, ConstraintStatus, PointerConstraintCore, SurfacePointerConstraint,
        },
        wl_display::WlDisplay,
    },
};

#[derive(Debug)]
pub struct ZwpConfinedPointerV1 {
    id: ZwpConfinedPointerV1Id,
    core: PointerConstraintCore,
}

impl ZwpConfinedPointerV1 {
    pub(crate) fn new(
        id: ZwpConfinedPointerV1Id,
        surface: Resolved<WlSurfaceId>,
        pointer: Resolved<WlPointerId>,
        lifetime: ConstraintLifetime,
        region: Option<RegionValue>,
    ) -> Self {
        Self {
            id,
            core: PointerConstraintCore::new(surface, pointer, lifetime, region),
        }
    }

    pub(crate) fn core_mut(&mut self) -> &mut PointerConstraintCore {
        &mut self.core
    }

    pub(crate) fn surface(&self) -> Resolved<WlSurfaceId> {
        self.core.surface
    }

    pub(crate) fn region(&self) -> Option<&RegionValue> {
        self.core.region.as_ref()
    }

    pub(crate) fn lifetime(&self) -> ConstraintLifetime {
        self.core.lifetime
    }

    pub(crate) fn status(&self) -> ConstraintStatus {
        self.core.status
    }

    pub(crate) fn set_status(&mut self, status: ConstraintStatus) {
        self.core.set_status(status);
    }

    pub(crate) fn send_confined(&self, dispatcher: &mut ClientDispatcher) {
        self.send_event(zwp_confined_pointer_v1::Event::Confined, dispatcher);
    }

    pub(crate) fn send_unconfined(&self, dispatcher: &mut ClientDispatcher) {
        self.send_event(zwp_confined_pointer_v1::Event::Unconfined, dispatcher);
    }

    fn snapshot_region(
        &self,
        client: &mut Client,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
        region: WlRegionId,
    ) -> Result<Option<RegionValue>, crate::object::AbortRequest> {
        let Some(region) = region.to_opt() else {
            return Ok(None);
        };
        let region = client.resolve_or_disconnect(region, state, dispatcher)?;
        Ok(Some(region.rref(state).snapshot()))
    }
}

impl Object for ZwpConfinedPointerV1 {
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
        use zwp_confined_pointer_v1::Request::*;

        match client.parse::<Self>(req, dispatcher)? {
            Destroy => {
                if let Some(resolved) = client.resolve(self.id, state) {
                    crate::input::pointer_confine::release_grabbed_pointer_confine(
                        &self, resolved, state, dispatcher, false,
                    );
                    if let Some(mut surface) = self.core.surface.try_ref_mut(state) {
                        surface
                            .clear_pointer_constraint(SurfacePointerConstraint::Confine(resolved));
                    }
                }
                client.delete_id(&WlDisplay::default(), state, dispatcher, self.id.into());
            }
            SetRegion { region } => {
                if self.core.status == ConstraintStatus::Terminal {
                    return Ok(());
                }
                let region = self.snapshot_region(client, state, dispatcher, region)?;
                if let Some(mut surface) = self.core.surface.try_ref_mut(state) {
                    surface.set_pending_pointer_constraint_region(region);
                }
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
        if let Some(resolved) = client.resolve(self.id, state) {
            crate::input::pointer_confine::release_grabbed_pointer_confine(
                self,
                resolved,
                state,
                _dispatcher,
                false,
            );
            if let Some(mut surface) = self.core.surface.try_ref_mut(state) {
                surface.clear_pointer_constraint(SurfacePointerConstraint::Confine(resolved));
            }
        }
        self.core.mark_terminal();
    }
}
