use crate::{
    clients::Resolved,
    protocol_generated::{
        zwp_pointer_constraints_v1, WlPointerId, WlSurfaceId, ZwpConfinedPointerV1Id,
        ZwpLockedPointerV1Id,
    },
    region::{RegionOp, RegionValue},
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

    pub(crate) fn set_status(&mut self, status: ConstraintStatus) {
        self.status = status;
    }
}

/// A point is in the effective region when it lies in the constraint region and the surface
/// input region. A null constraint region means the input region alone.
pub(crate) fn point_in_effective_region(
    constraint_region: Option<&RegionValue>,
    input_region: &RegionValue,
    x: i32,
    y: i32,
) -> bool {
    let in_constraint = constraint_region.is_none_or(|region| region.contains(x, y));
    in_constraint && input_region.contains(x, y)
}

/// One integer point inside the effective region, used to warp a fullscreen pointer in.
pub(crate) fn sample_effective_point(
    constraint_region: Option<&RegionValue>,
    input_region: &RegionValue,
) -> Option<(i32, i32)> {
    let region = constraint_region.unwrap_or(input_region);
    match region {
        RegionValue::Empty => None,
        RegionValue::Infinite => {
            point_in_effective_region(constraint_region, input_region, 0, 0).then_some((0, 0))
        }
        RegionValue::Spec(spec) => {
            for op in spec.ops() {
                let RegionOp::Add(rect) = op else {
                    continue;
                };
                let x = rect.x.saturating_add(rect.width / 2);
                let y = rect.y.saturating_add(rect.height / 2);
                if point_in_effective_region(constraint_region, input_region, x, y) {
                    return Some((x, y));
                }
            }
            None
        }
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
