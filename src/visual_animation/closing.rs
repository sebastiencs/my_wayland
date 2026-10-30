use std::time::{Duration, Instant};

use khronos_types::gl;

#[cfg(test)]
use std::{cell::Cell, rc::Rc};

use crate::{
    clients::Resolved,
    egl::GlTexture,
    protocol_generated::WlSurfaceId,
    region::RectI,
    render::{BlurVisualStyle, RoundedRing},
    state::DrmId,
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
    bounds: RectI,
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
        bounds: RectI,
        blur: Option<FrozenClosingBlur>,
    ) -> Self {
        Self {
            id,
            source,
            bounds,
            blur,
            track: RetargetableTrack::settled(UnitValue::ONE),
            cached_alpha: u8::MAX,
            active: true,
            pending: true,
        }
    }

    fn advance(&mut self, now: Instant, duration: Duration) -> bool {
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
                Easing::Linear,
            );
            self.pending = false;
        } else if self.track.duration() != duration {
            self.track
                .retarget_timed(UnitValue::ZERO, now, duration, Easing::Linear);
        }
        let sample = self.track.sample_mut(now);
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

/// Immutable window image retained after the real toplevel has left layout, input, and protocol state.
pub struct RetainedClosingSnapshot {
    animation: ClosingSnapshotAnimation,
    drm_id: DrmId,
    texture: GlTexture,
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
        bounds: RectI,
        blur: Option<FrozenClosingBlur>,
        drm_id: DrmId,
        texture: GlTexture,
    ) -> Self {
        Self {
            animation: ClosingSnapshotAnimation::start(id, source, bounds, blur),
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

    pub(crate) fn bounds(&self) -> RectI {
        self.animation.bounds
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

    pub(crate) fn texture_id(&self) -> gl::Uint {
        self.texture.texture_id()
    }

    /// Samples absolute time. `true` means the fade finished and the caller must drop this record.
    pub(crate) fn advance(&mut self, now: Instant, duration: Duration) -> bool {
        self.animation.advance(now, duration)
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
            animation: ClosingSnapshotAnimation::start(id, source, bounds, None),
            drm_id,
            texture: GlTexture::test_inert(),
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
}
