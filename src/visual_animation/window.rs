use std::time::{Duration, Instant};

use super::track::{Easing, RetargetableTrack, TrackStatus, UnitValue};

/// Appearance sampled from logical window state before lifecycle scaling.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WindowVisualBase {
    pub content_alpha: u8,
    pub dim_alpha: u8,
    pub shadow_alpha: u8,
    pub border_alpha: u8,
}

/// Focus appearance owned by one scalar timed track.
///
/// The track interpolates from the sampled style to the latest focus style. A focus or duration
/// change samples that style first, then starts a new segment, so the fade continues from the
/// pixels already shown. Opening and closing stay outside this owner and scale the result later.
#[derive(Clone, Copy, Debug)]
pub struct FocusVisualAnimation {
    from: WindowVisualBase,
    to: WindowVisualBase,
    track: RetargetableTrack,
    current: WindowVisualBase,
    active: bool,
}

impl FocusVisualAnimation {
    pub fn settled(style: WindowVisualBase) -> Self {
        Self {
            from: style,
            to: style,
            track: RetargetableTrack::settled(UnitValue::ONE),
            current: style,
            active: false,
        }
    }

    pub fn current(self) -> WindowVisualBase {
        self.current
    }

    pub fn is_active(self) -> bool {
        self.active
    }

    /// Sample `goal` over `duration`, retargeting only when the goal or active duration changes.
    pub fn advance(
        &mut self,
        goal: WindowVisualBase,
        duration: Duration,
        now: Instant,
    ) -> WindowVisualBase {
        let goal_changed = goal != self.to;
        let duration_changed = duration != self.track.duration();
        if goal_changed || (self.active && duration_changed) {
            let sampled =
                interpolate_window_visual(self.from, self.to, self.track.sample(now).value);
            if duration.is_zero() || sampled == goal {
                self.from = goal;
                self.to = goal;
                self.current = goal;
                self.track = RetargetableTrack::settled(UnitValue::ONE);
                self.active = false;
                return self.current;
            }
            self.from = sampled;
            self.to = goal;
            self.track = RetargetableTrack::start_timed(
                UnitValue::ZERO,
                UnitValue::ONE,
                now,
                duration,
                Easing::Linear,
            );
        }
        let sample = self.track.sample_mut(now);
        self.current = interpolate_window_visual(self.from, self.to, sample.value);
        self.active = sample.status == TrackStatus::Active;
        self.current
    }
}

/// Linear blend of every focus channel.
///
/// Progress zero returns `from` and progress one returns `to`.
pub fn interpolate_window_visual(
    from: WindowVisualBase,
    to: WindowVisualBase,
    progress: UnitValue,
) -> WindowVisualBase {
    WindowVisualBase {
        content_alpha: progress.lerp_u8(from.content_alpha, to.content_alpha),
        dim_alpha: progress.lerp_u8(from.dim_alpha, to.dim_alpha),
        shadow_alpha: progress.lerp_u8(from.shadow_alpha, to.shadow_alpha),
        border_alpha: progress.lerp_u8(from.border_alpha, to.border_alpha),
    }
}

/// Lifecycle contribution composed onto [`WindowVisualBase`] at sample time.
///
/// Phase 3 resolves production windows as [`WindowLifecycleAnim::Idle`]. Opening and closing exist
/// so later phases can scale the same channels without a second visual model.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WindowLifecycleAnim {
    Idle,
    Opening { progress: UnitValue },
    Closing { progress: UnitValue },
}

/// Draw modifiers consumed by render assembly.
///
/// `translation` is reserved for later workspace and window motion. Phase 3 resolution keeps it at
/// the origin and does not move layout or scene geometry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResolvedWindowVisual {
    pub content_alpha: u8,
    pub dim_alpha: u8,
    pub shadow_alpha: u8,
    pub border_alpha: u8,
    pub translation: (i32, i32),
}

pub fn resolve_window_visual(
    base: WindowVisualBase,
    lifecycle: WindowLifecycleAnim,
) -> ResolvedWindowVisual {
    let factor = match lifecycle {
        WindowLifecycleAnim::Idle => UnitValue::ONE,
        WindowLifecycleAnim::Opening { progress } => progress,
        WindowLifecycleAnim::Closing { progress } => UnitValue::ONE.saturating_sub(progress),
    };
    ResolvedWindowVisual {
        content_alpha: scale_alpha(base.content_alpha, factor),
        dim_alpha: scale_alpha(base.dim_alpha, factor),
        shadow_alpha: scale_alpha(base.shadow_alpha, factor),
        border_alpha: scale_alpha(base.border_alpha, factor),
        translation: (0, 0),
    }
}

fn scale_alpha(alpha: u8, factor: UnitValue) -> u8 {
    factor.lerp_u8(0, alpha)
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use {
        super::{
            interpolate_window_visual, resolve_window_visual, FocusVisualAnimation,
            WindowLifecycleAnim, WindowVisualBase,
        },
        crate::visual_animation::UnitValue,
    };

    fn inactive() -> WindowVisualBase {
        WindowVisualBase {
            content_alpha: 100,
            dim_alpha: 40,
            shadow_alpha: 80,
            border_alpha: 60,
        }
    }

    fn active() -> WindowVisualBase {
        WindowVisualBase {
            content_alpha: 200,
            dim_alpha: 0,
            shadow_alpha: 220,
            border_alpha: 180,
        }
    }

    fn half() -> UnitValue {
        UnitValue::saturating_from_ratio(1, 2)
    }

    fn base() -> WindowVisualBase {
        WindowVisualBase {
            content_alpha: 200,
            dim_alpha: 40,
            shadow_alpha: 180,
            border_alpha: 160,
        }
    }

    #[test]
    fn idle_resolution_preserves_alpha_and_origin() {
        let resolved = resolve_window_visual(base(), WindowLifecycleAnim::Idle);

        assert_eq!(resolved.content_alpha, 200);
        assert_eq!(resolved.dim_alpha, 40);
        assert_eq!(resolved.shadow_alpha, 180);
        assert_eq!(resolved.border_alpha, 160);
        assert_eq!(resolved.translation, (0, 0));
    }

    #[test]
    fn opening_and_closing_scale_every_alpha_channel() {
        let hidden = resolve_window_visual(
            base(),
            WindowLifecycleAnim::Opening { progress: UnitValue::ZERO },
        );
        assert_eq!(hidden.content_alpha, 0);
        assert_eq!(hidden.dim_alpha, 0);
        assert_eq!(hidden.shadow_alpha, 0);
        assert_eq!(hidden.border_alpha, 0);

        let shown = resolve_window_visual(
            base(),
            WindowLifecycleAnim::Opening { progress: UnitValue::ONE },
        );
        assert_eq!(shown.content_alpha, base().content_alpha);
        assert_eq!(shown.dim_alpha, base().dim_alpha);

        let closing_start = resolve_window_visual(
            base(),
            WindowLifecycleAnim::Closing { progress: UnitValue::ZERO },
        );
        assert_eq!(closing_start.content_alpha, base().content_alpha);
        let closing_end = resolve_window_visual(
            base(),
            WindowLifecycleAnim::Closing { progress: UnitValue::ONE },
        );
        assert_eq!(closing_end.content_alpha, 0);
        assert_eq!(closing_end.shadow_alpha, 0);
        assert_eq!(closing_end.border_alpha, 0);
        assert_eq!(closing_end.translation, (0, 0));
    }

    #[test]
    fn settled_focus_animation_stays_on_its_style() {
        let now = Instant::now();
        let mut animation = FocusVisualAnimation::settled(inactive());

        assert_eq!(animation.current(), inactive());
        assert!(!animation.is_active());
        assert_eq!(animation.advance(inactive(), Duration::from_millis(100), now), inactive());
        assert!(!animation.is_active());
    }

    #[test]
    fn focus_midpoint_interpolates_every_channel() {
        let now = Instant::now();
        let mut animation = FocusVisualAnimation::settled(inactive());
        assert_eq!(animation.advance(active(), Duration::from_millis(100), now), inactive());

        let mid = animation.advance(
            active(),
            Duration::from_millis(100),
            now + Duration::from_millis(50),
        );

        assert_eq!(mid, interpolate_window_visual(inactive(), active(), half()));
        assert_eq!(mid.content_alpha, 150);
        assert_eq!(mid.dim_alpha, 20);
        assert_eq!(mid.shadow_alpha, 150);
        assert_eq!(mid.border_alpha, 120);
        assert!(animation.is_active());
    }

    #[test]
    fn rapid_focus_reversal_continues_from_the_sampled_style() {
        let now = Instant::now();
        let mut animation = FocusVisualAnimation::settled(inactive());
        animation.advance(active(), Duration::from_millis(100), now);
        let mid_time = now + Duration::from_millis(50);
        let mid = animation.advance(active(), Duration::from_millis(100), mid_time);

        let reversed = animation.advance(inactive(), Duration::from_millis(100), mid_time);

        assert_eq!(reversed, mid);
        assert!(animation.is_active());
        let later = animation.advance(
            inactive(),
            Duration::from_millis(100),
            mid_time + Duration::from_millis(50),
        );
        assert!(later.content_alpha < mid.content_alpha);
        assert!(later.dim_alpha > mid.dim_alpha);
        assert_eq!(later, interpolate_window_visual(mid, inactive(), half()));
    }

    #[test]
    fn zero_duration_focus_change_snaps() {
        let now = Instant::now();
        let mut animation = FocusVisualAnimation::settled(inactive());

        assert_eq!(animation.advance(active(), Duration::ZERO, now), active());
        assert!(!animation.is_active());
        assert_eq!(animation.advance(active(), Duration::from_millis(80), now), active());
        assert!(!animation.is_active());
    }

    #[test]
    fn active_duration_change_retargets_from_the_current_style() {
        let now = Instant::now();
        let mut animation = FocusVisualAnimation::settled(inactive());
        animation.advance(active(), Duration::from_millis(100), now);
        let mid_time = now + Duration::from_millis(50);
        let mid = animation.advance(active(), Duration::from_millis(100), mid_time);

        let retargeted = animation.advance(active(), Duration::from_millis(200), mid_time);

        assert_eq!(retargeted, mid);
        assert!(animation.is_active());
        let later = animation.advance(
            active(),
            Duration::from_millis(200),
            mid_time + Duration::from_millis(100),
        );
        assert_eq!(later, interpolate_window_visual(mid, active(), half()));
        assert_ne!(later, active());
    }
}
