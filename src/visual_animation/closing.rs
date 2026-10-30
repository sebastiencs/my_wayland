use crate::{
    clients::Resolved, protocol_generated::WlSurfaceId, region::RectI, state::RootSurfaceClip,
};

use super::track::RetargetableTrack;

/// Immutable geometry captured for a close animation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ClosingFrame {
    pub bounds: RectI,
    pub clip: Option<RootSurfaceClip>,
}

/// Live-content close record. Phase 6 creates these at detach; Phase 3 only defines ownership.
#[derive(Clone, Debug)]
pub struct DetachedClosingWindow {
    surface: Resolved<WlSurfaceId>,
    frame: ClosingFrame,
    progress: RetargetableTrack,
}

impl DetachedClosingWindow {
    pub fn new(
        surface: Resolved<WlSurfaceId>,
        frame: ClosingFrame,
        progress: RetargetableTrack,
    ) -> Self {
        Self { surface, frame, progress }
    }

    pub fn surface(&self) -> Resolved<WlSurfaceId> {
        self.surface
    }

    pub fn frame(&self) -> ClosingFrame {
        self.frame
    }

    pub fn progress(&self) -> &RetargetableTrack {
        &self.progress
    }
}

/// Frozen close record. `T` is the retained image payload and is released with the record.
///
/// Phase 3 does not capture GPU contents. Callers can use a test payload or a later snapshot type
/// without changing this ownership boundary.
#[derive(Clone, Debug)]
pub struct RetainedClosingSnapshot<T> {
    surface: Resolved<WlSurfaceId>,
    frame: ClosingFrame,
    progress: RetargetableTrack,
    payload: T,
}

impl<T> RetainedClosingSnapshot<T> {
    pub fn new(
        surface: Resolved<WlSurfaceId>,
        frame: ClosingFrame,
        progress: RetargetableTrack,
        payload: T,
    ) -> Self {
        Self { surface, frame, progress, payload }
    }

    pub fn surface(&self) -> Resolved<WlSurfaceId> {
        self.surface
    }

    pub fn frame(&self) -> ClosingFrame {
        self.frame
    }

    pub fn progress(&self) -> &RetargetableTrack {
        &self.progress
    }

    pub fn payload(&self) -> &T {
        &self.payload
    }

    pub fn release(self) -> T {
        self.payload
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::Cell, rc::Rc, time::Instant};

    use {
        super::{ClosingFrame, DetachedClosingWindow, RetainedClosingSnapshot},
        crate::{
            clients::{ClientId, Resolved},
            config::RoundingPower,
            protocol_generated::WlSurfaceId,
            region::RectI,
            state::{ObjectId, RootSurfaceClip},
            visual_animation::RetargetableTrack,
        },
    };

    struct Payload(Rc<Cell<u32>>);

    impl Drop for Payload {
        fn drop(&mut self) {
            self.0.set(self.0.get().saturating_add(1));
        }
    }

    fn frame() -> ClosingFrame {
        let bounds = RectI::new(4, 8, 40, 24).unwrap();
        ClosingFrame {
            bounds,
            clip: Some(RootSurfaceClip::new(bounds, 6, RoundingPower::DEFAULT)),
        }
    }

    fn surface() -> Resolved<WlSurfaceId> {
        Resolved::new(ClientId::from_raw(1), WlSurfaceId::from(9u32), ObjectId::from(9usize))
    }

    #[test]
    fn detached_record_keeps_identity_geometry_and_progress() {
        let now = Instant::now();
        let record = DetachedClosingWindow::new(
            surface(),
            frame(),
            RetargetableTrack::settled(crate::visual_animation::UnitValue::ONE),
        );

        assert_eq!(record.surface(), surface());
        assert_eq!(record.frame(), frame());
        assert!(!record.progress().needs_frames(now));
    }

    #[test]
    fn retained_snapshot_owns_and_releases_its_payload() {
        let drops = Rc::new(Cell::new(0));
        let snapshot = RetainedClosingSnapshot::new(
            surface(),
            frame(),
            RetargetableTrack::settled(crate::visual_animation::UnitValue::ZERO),
            Payload(drops.clone()),
        );
        assert_eq!(snapshot.payload().0.get(), 0);

        let released = snapshot.release();
        assert_eq!(drops.get(), 0);
        drop(released);
        assert_eq!(drops.get(), 1);

        {
            let _snapshot = RetainedClosingSnapshot::new(
                surface(),
                frame(),
                RetargetableTrack::settled(crate::visual_animation::UnitValue::ZERO),
                Payload(drops.clone()),
            );
        }
        assert_eq!(drops.get(), 2);
    }
}
