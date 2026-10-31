use crate::{
    clients::{Client, ClientDispatcher, EventWriter, RawRequest, Resolved},
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

    pub(crate) fn surface(&self) -> Resolved<WlSurfaceId> {
        self.core.surface
    }

    pub(crate) fn pointer(&self) -> Resolved<WlPointerId> {
        self.core.pointer
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

    pub(crate) fn set_status(
        &mut self,
        status: crate::wayland_objects::pointer_constraint::ConstraintStatus,
    ) {
        self.core.set_status(status);
    }

    pub(crate) fn send_locked(&self, dispatcher: &mut ClientDispatcher) {
        self.send_event(zwp_locked_pointer_v1::Event::Locked, dispatcher);
    }

    pub(crate) fn send_unlocked(&self, dispatcher: &mut ClientDispatcher) {
        self.send_event(zwp_locked_pointer_v1::Event::Unlocked, dispatcher);
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
                    // The client destroyed the lock. Release the freeze and warp to a valid
                    // hint, but do not send `unlocked`: the object is going away.
                    crate::input::pointer_lock::release_grabbed_pointer_lock(
                        &self,
                        resolved,
                        state,
                        dispatcher,
                        crate::input::pointer_lock::PointerLockRelease {
                            send_unlocked: false,
                            apply_lifetime: false,
                            warp: true,
                        },
                    );
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
            crate::input::pointer_lock::release_grabbed_pointer_lock(
                self,
                resolved,
                state,
                _dispatcher,
                crate::input::pointer_lock::PointerLockRelease {
                    send_unlocked: false,
                    apply_lifetime: false,
                    warp: false,
                },
            );
            if let Some(mut surface) = self.core.surface.try_ref_mut(state) {
                surface.clear_pointer_constraint(SurfacePointerConstraint::Lock(resolved));
            }
        }
        self.core.mark_terminal();
    }
}
