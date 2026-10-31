use crate::{
    clients::Resolved,
    protocol_generated::{
        zwp_pointer_constraints_v1, WlPointerId, WlSurfaceId, ZwpConfinedPointerV1Id,
        ZwpLockedPointerV1Id,
    },
    region::RegionValue,
};

/// One pointer constraint for a surface. This compositor has one seat, so one slot per surface
/// is one constraint per surface per seat.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum SurfacePointerConstraint {
    Lock(Resolved<ZwpLockedPointerV1Id>),
    Confine(Resolved<ZwpConfinedPointerV1Id>),
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum ConstraintLifetime {
    Oneshot,
    Persistent,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum ConstraintStatus {
    Inactive,
    Active,
    Terminal,
}

#[derive(Clone, Debug)]
pub(crate) struct PointerConstraintCore {
    pub(crate) surface: Resolved<WlSurfaceId>,
    pub(crate) pointer: Resolved<WlPointerId>,
    pub(crate) lifetime: ConstraintLifetime,
    pub(crate) status: ConstraintStatus,
    /// `None` means the surface input region. The factory region is stored here immediately.
    /// `set_region` replaces it on `wl_surface.commit`.
    pub(crate) region: Option<RegionValue>,
    pub(crate) committed_hint: Option<(f32, f32)>,
}

impl PointerConstraintCore {
    pub(crate) fn new(
        surface: Resolved<WlSurfaceId>,
        pointer: Resolved<WlPointerId>,
        lifetime: ConstraintLifetime,
        region: Option<RegionValue>,
    ) -> Self {
        Self {
            surface,
            pointer,
            lifetime,
            status: ConstraintStatus::Inactive,
            region,
            committed_hint: None,
        }
    }

    pub(crate) fn apply_region(&mut self, region: Option<RegionValue>) {
        self.region = region;
    }

    pub(crate) fn apply_hint(&mut self, hint: (f32, f32)) {
        self.committed_hint = Some(hint);
    }

    pub(crate) fn mark_terminal(&mut self) {
        self.status = ConstraintStatus::Terminal;
    }
}

pub(crate) fn constraint_lifetime(value: u32) -> Option<ConstraintLifetime> {
    if value == zwp_pointer_constraints_v1::Lifetime::Oneshot as u32 {
        Some(ConstraintLifetime::Oneshot)
    } else if value == zwp_pointer_constraints_v1::Lifetime::Persistent as u32 {
        Some(ConstraintLifetime::Persistent)
    } else {
        None
    }
}
