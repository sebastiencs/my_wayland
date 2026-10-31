use crate::{
    clients::{Client, ClientDispatcher, RawRequest, Resolved},
    object::{Object, Parser, RequestResult},
    protocol_generated::{
        zwp_locked_pointer_v1, WlPointerId, WlRegionId, WlSurfaceId, ZwpLockedPointerV1Id,
    },
    region::RegionValue,
    state::{GrabbedObject, State},
    wayland_objects::{
        pointer_constraint::{ConstraintLifetime, PointerConstraintCore, SurfacePointerConstraint},
        wl_display::WlDisplay,
    },
};

#[derive(Debug)]
pub struct ZwpLockedPointerV1 {
    id: ZwpLockedPointerV1Id,
    core: PointerConstraintCore,
}

impl ZwpLockedPointerV1 {
    pub(crate) fn new(
        id: ZwpLockedPointerV1Id,
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

    pub(crate) fn region(&self) -> Option<&RegionValue> {
        self.core.region.as_ref()
    }

    pub(crate) fn committed_hint(&self) -> Option<(f32, f32)> {
        self.core.committed_hint
    }

    pub(crate) fn lifetime(&self) -> ConstraintLifetime {
        self.core.lifetime
    }

    pub(crate) fn status(&self) -> crate::wayland_objects::pointer_constraint::ConstraintStatus {
        self.core.status
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
        let region = client.resolve_or_disconnect::<WlRegionId>(region, state, dispatcher)?;
        Ok(Some(region.rref(state).snapshot()))
    }
}

impl Object for ZwpLockedPointerV1 {
    fn id(&self) -> crate::protocol_generated::WaylandId {
        self.id.into()
    }

    fn on(
        mut self: GrabbedObject<Self>,
        client: &mut Client,
        state: &mut State,
        req: RawRequest,
        dispatcher: &mut ClientDispatcher,
    ) -> RequestResult {
        use zwp_locked_pointer_v1::Request::*;

        match client.parse::<Self>(req, dispatcher)? {
            Destroy => {
                if let Some(resolved) = client.resolve(self.id, state) {
                    if let Some(mut surface) = self.core.surface.try_ref_mut(state) {
                        surface.clear_pointer_constraint(SurfacePointerConstraint::Lock(resolved));
                    }
                }
                client.delete_id(&WlDisplay::default(), state, dispatcher, self.id.into());
            }
            SetCursorPositionHint { surface_x, surface_y } => {
                if self.core.status
                    == crate::wayland_objects::pointer_constraint::ConstraintStatus::Terminal
                {
                    return Ok(());
                }
                if let Some(mut surface) = self.core.surface.try_ref_mut(state) {
                    surface.set_pending_pointer_constraint_hint((surface_x, surface_y));
                }
            }
            SetRegion { region } => {
                if self.core.status
                    == crate::wayland_objects::pointer_constraint::ConstraintStatus::Terminal
                {
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
            if let Some(mut surface) = self.core.surface.try_ref_mut(state) {
                surface.clear_pointer_constraint(SurfacePointerConstraint::Lock(resolved));
            }
        }
        self.core.mark_terminal();
    }
}
