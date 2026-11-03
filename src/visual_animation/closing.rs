use std::{
    rc::Rc,
    time::{Duration, Instant},
};

#[cfg(test)]
use std::cell::Cell;

use crate::{
    clients::Resolved,
    protocol_generated::WlSurfaceId,
    region::RectI,
    render::{BlurVisualStyle, RoundedRing, Texture},
    state::{DrmId, WorkspaceId},
};

use super::track::{Easing, RetargetableTrack, TrackStatus, UnitValue};

/// Backdrop blur redrawn live behind a closing snapshot. The pixels are not baked into the texture.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FrozenClosingBlur {
    pub ring: RoundedRing,
    pub style: BlurVisualStyle,
    pub alpha: u8,
}

/// Progress for one retained close. `progress == 0` is full alpha until the first pump samples time.
#[derive(Clone, Debug)]
struct ClosingSnapshotAnimation {
    id: u64,
    source: Resolved<WlSurfaceId>,
    workspace_id: Option<WorkspaceId>,
    bounds: RectI,
    pose: Option<RectI>,
    factor: UnitValue,
    blur: Option<FrozenClosingBlur>,
    track: RetargetableTrack,
    cached_alpha: u8,
    active: bool,
    pending: bool,
}

impl ClosingSnapshotAnimation {
    fn start(
        id: u64,
        source: Resolved<WlSurfaceId>,
        workspace_id: Option<WorkspaceId>,
        bounds: RectI,
        blur: Option<FrozenClosingBlur>,
    ) -> Self {
        Self {
            id,
            source,
            workspace_id,
            bounds,
            pose: None,
            factor: UnitValue::ONE,
            blur,
            track: RetargetableTrack::settled(UnitValue::ONE),
            cached_alpha: u8::MAX,
            active: true,
            pending: true,
        }
    }

    fn advance(&mut self, now: Instant, duration: Duration, easing: Easing) -> bool {
        if !self.active {
            return false;
        }
        if duration.is_zero() {
            self.finish();
            return true;
        }
        if self.pending {
            self.track = RetargetableTrack::start_timed(
                UnitValue::ONE,
                UnitValue::ZERO,
                now,
                duration,
                easing,
            );
            self.pending = false;
        } else if self.track.duration() != duration || self.track.easing() != easing {
            self.track
                .retarget_timed(UnitValue::ZERO, now, duration, easing);
        }
        let sample = self.track.sample_mut(now);
        self.factor = sample.value;
        self.cached_alpha = sample.value.lerp_u8(0, u8::MAX);
        if sample.status == TrackStatus::Settled {
            self.finish();
            return true;
        }
        false
    }

    fn finish(&mut self) {
        self.active = false;
        self.pending = false;
        self.cached_alpha = 0;
    }
}

/// Canonical close animation. Capture freezes one managed toplevel into a window-local texture
/// before teardown. The visual-animation pump fades that texture, then dropping this value
/// releases it. There is no detached live-surface close path.
///
/// Capture is skipped when the toplevel is already snapshotted, the session is inactive, the close
/// duration is zero, the root is unmapped or has no buffer, the role is not an xdg toplevel, the
/// render plan is empty, or the window spans more than one DRM device.
///
/// Immutable window image retained after the real toplevel has left layout, input, and protocol state.
pub struct RetainedClosingSnapshot {
    animation: ClosingSnapshotAnimation,
    drm_id: DrmId,
    texture: Rc<dyn Texture>,
    #[cfg(test)]
    release_probe: Option<Rc<Cell<u32>>>,
}

impl Drop for RetainedClosingSnapshot {
    fn drop(&mut self) {
        #[cfg(test)]
        if let Some(probe) = &self.release_probe {
            probe.set(probe.get().saturating_add(1));
        }
    }
}

impl RetainedClosingSnapshot {
    pub(crate) fn new(
        id: u64,
        source: Resolved<WlSurfaceId>,
        workspace_id: Option<WorkspaceId>,
        bounds: RectI,
        blur: Option<FrozenClosingBlur>,
        drm_id: DrmId,
        texture: Rc<dyn Texture>,
    ) -> Self {
        Self {
            animation: ClosingSnapshotAnimation::start(id, source, workspace_id, bounds, blur),
            drm_id,
            texture,
            #[cfg(test)]
            release_probe: None,
        }
    }

    pub(crate) fn id(&self) -> u64 {
        self.animation.id
    }

    pub(crate) fn source(&self) -> Resolved<WlSurfaceId> {
        self.animation.source
    }

    pub(crate) fn workspace_id(&self) -> Option<WorkspaceId> {
        self.animation.workspace_id
    }

    pub(crate) fn bounds(&self) -> RectI {
        self.animation.bounds
    }

    /// Captured rectangle while the close clock is at the start, then the `windowsOut` pose.
    pub(crate) fn visual_bounds(&self) -> RectI {
        let Some(pose) = self.animation.pose else {
            return self.animation.bounds;
        };
        lerp_rect(self.animation.bounds, pose, self.animation.factor)
    }

    /// Union of the captured rectangle, the current sample, and the close pose.
    ///
    /// Scheduling keeps the snapshot alive on every output that rectangle still touches.
    pub(crate) fn motion_bounds(&self) -> RectI {
        let mut bounds = union_rect(self.bounds(), self.visual_bounds());
        if let Some(pose) = self.animation.pose {
            bounds = union_rect(bounds, pose);
        }
        bounds
    }

    pub(crate) fn arm_close_pose(&mut self, pose: RectI) {
        if self.animation.pose.is_none() {
            self.animation.pose = Some(pose);
        }
    }

    pub(crate) fn alpha(&self) -> u8 {
        self.animation.cached_alpha
    }

    pub(crate) fn is_active(&self) -> bool {
        self.animation.active
    }

    pub(crate) fn blur(&self) -> Option<FrozenClosingBlur> {
        self.animation.blur
    }

    pub(crate) fn drm_id(&self) -> DrmId {
        self.drm_id
    }

    pub(crate) fn texture(&self) -> Rc<dyn Texture> {
        Rc::clone(&self.texture)
    }

    pub(crate) fn requires_device_context_on_drop(&self) -> bool {
        self.texture.requires_device_context_on_drop()
    }

    /// Samples absolute time. `true` means the fade finished and the caller must drop this record.
    pub(crate) fn advance(&mut self, now: Instant, duration: Duration) -> bool {
        self.advance_with_easing(now, duration, Easing::Linear)
    }

    pub(crate) fn advance_with_easing(
        &mut self,
        now: Instant,
        duration: Duration,
        easing: Easing,
    ) -> bool {
        self.animation.advance(now, duration, easing)
    }
}

fn lerp_rect(from: RectI, to: RectI, remain: UnitValue) -> RectI {
    let remain = remain.as_f64();
    let mix = |start: i32, end: i32| -> i32 {
        round_i32(f64::from(start) * remain + f64::from(end) * (1.0 - remain))
    };
    RectI::new(
        mix(from.x, to.x),
        mix(from.y, to.y),
        mix(from.width, to.width).max(0),
        mix(from.height, to.height).max(0),
    )
    .unwrap_or(from)
}

fn union_rect(a: RectI, b: RectI) -> RectI {
    if a.is_empty() {
        return b;
    }
    if b.is_empty() {
        return a;
    }
    RectI::from_extents(a.x.min(b.x), a.y.min(b.y), a.x2().max(b.x2()), a.y2().max(b.y2()))
        .unwrap_or(a)
}

fn round_i32(value: f64) -> i32 {
    let rounded = value.round();
    if rounded >= f64::from(i32::MAX) {
        i32::MAX
    } else if rounded <= f64::from(i32::MIN) {
        i32::MIN
    } else {
        rounded as i32
    }
}

#[cfg(test)]
impl RetainedClosingSnapshot {
    pub(crate) fn test_pending(
        id: u64,
        source: Resolved<WlSurfaceId>,
        bounds: RectI,
        drm_id: DrmId,
    ) -> (Self, Rc<Cell<u32>>) {
        let probe = Rc::new(Cell::new(0));
        let snapshot = Self {
            animation: ClosingSnapshotAnimation::start(id, source, None, bounds, None),
            drm_id,
            texture: crate::render::inert_texture(),
            release_probe: Some(Rc::clone(&probe)),
        };
        (snapshot, probe)
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use crate::{
        clients::{ClientId, Resolved},
        protocol_generated::WlSurfaceId,
        region::RectI,
        state::{DrmId, ObjectId},
    };

    use super::RetainedClosingSnapshot;

    fn source() -> Resolved<WlSurfaceId> {
        Resolved::new(ClientId::from_raw(1), WlSurfaceId::from(4), ObjectId::from(4usize))
    }

    fn bounds() -> RectI {
        RectI::new(10, 20, 100, 80).unwrap()
    }

    #[test]
    fn pending_snapshot_is_fully_opaque_before_the_clock_starts() {
        let (snapshot, probe) =
            RetainedClosingSnapshot::test_pending(1, source(), bounds(), DrmId::from(0usize));
        assert!(snapshot.is_active());
        assert_eq!(snapshot.alpha(), 255);
        assert_eq!(snapshot.source(), source());
        assert_eq!(snapshot.bounds(), bounds());
        assert_eq!(probe.get(), 0);
        drop(snapshot);
        assert_eq!(probe.get(), 1);
    }

    #[test]
    fn midpoint_stays_translucent_and_completion_releases_once() {
        let (mut snapshot, probe) =
            RetainedClosingSnapshot::test_pending(7, source(), bounds(), DrmId::from(1usize));
        let now = Instant::now();
        let duration = Duration::from_millis(100);
        assert!(!snapshot.advance(now, duration));
        assert_eq!(snapshot.alpha(), 255);
        assert!(!snapshot.advance(now + Duration::from_millis(50), duration));
        assert!(snapshot.alpha() > 0 && snapshot.alpha() < 255);
        assert!(snapshot.advance(now + duration, duration));
        assert_eq!(snapshot.alpha(), 0);
        assert!(!snapshot.is_active());
        drop(snapshot);
        assert_eq!(probe.get(), 1);
    }

    #[test]
    fn zero_duration_completes_immediately() {
        let (mut snapshot, probe) =
            RetainedClosingSnapshot::test_pending(3, source(), bounds(), DrmId::from(0usize));
        assert!(snapshot.advance(Instant::now(), Duration::ZERO));
        assert_eq!(snapshot.alpha(), 0);
        drop(snapshot);
        assert_eq!(probe.get(), 1);
    }

    #[test]
    fn duration_retarget_keeps_the_current_alpha() {
        let (mut snapshot, _probe) =
            RetainedClosingSnapshot::test_pending(9, source(), bounds(), DrmId::from(0usize));
        let now = Instant::now();
        assert!(!snapshot.advance(now, Duration::from_millis(100)));
        assert!(!snapshot.advance(now + Duration::from_millis(50), Duration::from_millis(100)));
        let mid = snapshot.alpha();
        assert!(!snapshot.advance(now + Duration::from_millis(50), Duration::from_millis(400)));
        assert_eq!(snapshot.alpha(), mid);
        assert!(snapshot.is_active());
    }

    #[test]
    fn close_pose_moves_from_the_captured_bounds_and_completion_releases_once() {
        let (mut snapshot, probe) =
            RetainedClosingSnapshot::test_pending(4, source(), bounds(), DrmId::from(0usize));
        let pose = crate::visual_animation::lifecycle_pose(
            crate::animation_config::AnimationStyle::Popin { percent: 87 },
            bounds(),
            None,
        );
        snapshot.arm_close_pose(pose);
        let now = Instant::now();
        let duration = Duration::from_millis(100);
        assert!(!snapshot.advance(now, duration));
        assert_eq!(snapshot.visual_bounds(), bounds());
        assert_eq!(snapshot.bounds(), bounds());
        assert!(!snapshot.advance(now + Duration::from_millis(50), duration));
        let mid = snapshot.visual_bounds();
        assert_ne!(mid, bounds());
        assert_ne!(mid, pose);
        assert!(snapshot.motion_bounds().intersects(bounds()));
        assert!(snapshot.advance(now + duration, duration));
        drop(snapshot);
        assert_eq!(probe.get(), 1);
    }

    #[test]
    fn slide_close_pose_expands_the_motion_bounds_past_the_capture() {
        let (mut snapshot, _probe) =
            RetainedClosingSnapshot::test_pending(5, source(), bounds(), DrmId::from(0usize));
        let pose = RectI::new(-90, 20, 100, 80).unwrap();
        snapshot.arm_close_pose(pose);
        let motion = snapshot.motion_bounds();
        assert_eq!(motion.x, pose.x);
        assert_eq!(motion.x2(), bounds().x2());
        assert_eq!(snapshot.visual_bounds(), bounds());
    }

    #[test]
    fn snapshot_without_a_pose_stays_on_the_captured_bounds() {
        let (mut snapshot, _probe) =
            RetainedClosingSnapshot::test_pending(6, source(), bounds(), DrmId::from(0usize));
        let now = Instant::now();
        assert!(!snapshot.advance(now, Duration::from_millis(100)));
        assert!(!snapshot.advance(now + Duration::from_millis(50), Duration::from_millis(100)));
        assert_eq!(snapshot.visual_bounds(), bounds());
        assert_eq!(snapshot.motion_bounds(), bounds());
    }
}
