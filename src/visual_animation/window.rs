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

/// Opening lifecycle owned separately from the focus style.
///
/// `start_opening` exposes progress zero immediately. The first animation-pump tick starts the
/// absolute-time track, and completion returns to [`WindowLifecycleAnim::Idle`]. Closing is not a
/// lifecycle state: [`crate::visual_animation::RetainedClosingSnapshot`] owns the fade.
#[derive(Clone, Copy, Debug)]
pub struct WindowLifecycleAnimation {
    track: RetargetableTrack,
    cached: WindowLifecycleAnim,
    active: bool,
    pending: bool,
}

impl WindowLifecycleAnimation {
    pub fn idle() -> Self {
        Self {
            track: RetargetableTrack::settled(UnitValue::ONE),
            cached: WindowLifecycleAnim::Idle,
            active: false,
            pending: false,
        }
    }

    pub fn sample(self) -> WindowLifecycleAnim {
        self.cached
    }

    pub fn is_active(self) -> bool {
        self.active
    }

    pub fn start_opening(&mut self) {
        self.cached = WindowLifecycleAnim::Opening { progress: UnitValue::ZERO };
        self.active = true;
        self.pending = true;
    }

    pub fn settle_idle(&mut self) {
        *self = Self::idle();
    }

    /// Sample the opening track. A zero duration settles immediately.
    pub fn advance(&mut self, duration: Duration, now: Instant) -> WindowLifecycleAnim {
        if !self.active {
            return self.cached;
        }
        if duration.is_zero() {
            self.settle_idle();
            return self.cached;
        }
        if self.pending {
            self.pending = false;
            self.track = RetargetableTrack::start_timed(
                UnitValue::ZERO,
                UnitValue::ONE,
                now,
                duration,
                Easing::Linear,
            );
        } else if duration != self.track.duration() {
            self.track
                .retarget_timed(UnitValue::ONE, now, duration, Easing::Linear);
        }
        let sample = self.track.sample_mut(now);
        if sample.status == TrackStatus::Settled {
            self.settle_idle();
        } else {
            self.cached = WindowLifecycleAnim::Opening { progress: sample.value };
            self.active = true;
        }
        self.cached
    }
}

/// Linear blend of every focus channel.
///
/// Progress zero returns `from` and progress one returns `to`.
fn interpolate_window_visual(
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
/// Production windows stay [`WindowLifecycleAnim::Idle`] until a managed map starts opening.
/// A closing toplevel leaves this lifecycle and fades as a retained snapshot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WindowLifecycleAnim {
    Idle,
    Opening { progress: UnitValue },
}

/// Alpha modifiers consumed by render assembly.
///
/// Workspace translation stays on `WorkspaceMotion` and is applied when scene items are placed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResolvedWindowVisual {
    pub content_alpha: u8,
    pub dim_alpha: u8,
    pub shadow_alpha: u8,
    pub border_alpha: u8,
}

pub fn resolve_window_visual(
    base: WindowVisualBase,
    lifecycle: WindowLifecycleAnim,
) -> ResolvedWindowVisual {
    let factor = match lifecycle {
        WindowLifecycleAnim::Idle => UnitValue::ONE,
        WindowLifecycleAnim::Opening { progress } => progress,
    };
    ResolvedWindowVisual {
        content_alpha: scale_alpha(base.content_alpha, factor),
        dim_alpha: scale_alpha(base.dim_alpha, factor),
        shadow_alpha: scale_alpha(base.shadow_alpha, factor),
        border_alpha: scale_alpha(base.border_alpha, factor),
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
            WindowLifecycleAnim, WindowLifecycleAnimation, WindowVisualBase,
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
    fn idle_resolution_preserves_every_alpha_channel() {
        let resolved = resolve_window_visual(base(), WindowLifecycleAnim::Idle);

        assert_eq!(resolved.content_alpha, 200);
        assert_eq!(resolved.dim_alpha, 40);
        assert_eq!(resolved.shadow_alpha, 180);
        assert_eq!(resolved.border_alpha, 160);
    }

    #[test]
    fn opening_scales_every_alpha_channel() {
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
        assert_eq!(shown.shadow_alpha, base().shadow_alpha);
        assert_eq!(shown.border_alpha, base().border_alpha);
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

    fn opening_progress(animation: WindowLifecycleAnimation) -> UnitValue {
        match animation.sample() {
            WindowLifecycleAnim::Opening { progress } => progress,
            WindowLifecycleAnim::Idle => panic!("expected opening progress"),
        }
    }

    #[test]
    fn opening_starts_pending_at_zero() {
        let mut lifecycle = WindowLifecycleAnimation::idle();
        lifecycle.start_opening();

        assert!(lifecycle.is_active());
        assert_eq!(opening_progress(lifecycle), UnitValue::ZERO);
        assert_eq!(resolve_window_visual(active(), lifecycle.sample()).content_alpha, 0);
    }

    #[test]
    fn opening_midpoint_and_completion_return_to_idle() {
        let now = Instant::now();
        let mut lifecycle = WindowLifecycleAnimation::idle();
        lifecycle.start_opening();
        assert_eq!(
            lifecycle.advance(Duration::from_millis(100), now),
            WindowLifecycleAnim::Opening { progress: UnitValue::ZERO }
        );

        let mid = lifecycle.advance(Duration::from_millis(100), now + Duration::from_millis(50));
        assert_eq!(mid, WindowLifecycleAnim::Opening { progress: half() });
        assert!(lifecycle.is_active());

        assert_eq!(
            lifecycle.advance(Duration::from_millis(100), now + Duration::from_millis(100)),
            WindowLifecycleAnim::Idle
        );
        assert!(!lifecycle.is_active());
    }

    #[test]
    fn zero_duration_opening_snaps_idle() {
        let mut lifecycle = WindowLifecycleAnimation::idle();
        lifecycle.start_opening();

        assert_eq!(lifecycle.advance(Duration::ZERO, Instant::now()), WindowLifecycleAnim::Idle);
        assert!(!lifecycle.is_active());
    }

    #[test]
    fn active_opening_duration_change_retargets_from_current_progress() {
        let now = Instant::now();
        let mut lifecycle = WindowLifecycleAnimation::idle();
        lifecycle.start_opening();
        lifecycle.advance(Duration::from_millis(100), now);
        let mid_time = now + Duration::from_millis(50);
        assert_eq!(
            lifecycle.advance(Duration::from_millis(100), mid_time),
            WindowLifecycleAnim::Opening { progress: half() }
        );

        assert_eq!(
            lifecycle.advance(Duration::from_millis(200), mid_time),
            WindowLifecycleAnim::Opening { progress: half() }
        );
        let later =
            lifecycle.advance(Duration::from_millis(200), mid_time + Duration::from_millis(100));
        let expected = half().lerp(half(), UnitValue::ONE);
        assert_eq!(later, WindowLifecycleAnim::Opening { progress: expected });
        assert_ne!(later, WindowLifecycleAnim::Idle);
    }

    #[test]
    fn remap_restarts_opening_from_zero() {
        let now = Instant::now();
        let mut lifecycle = WindowLifecycleAnimation::idle();
        lifecycle.start_opening();
        lifecycle.advance(Duration::from_millis(100), now);
        let mid_time = now + Duration::from_millis(50);
        lifecycle.advance(Duration::from_millis(100), mid_time);

        lifecycle.start_opening();
        assert_eq!(opening_progress(lifecycle), UnitValue::ZERO);
        assert_eq!(
            lifecycle.advance(Duration::from_millis(100), mid_time),
            WindowLifecycleAnim::Opening { progress: UnitValue::ZERO }
        );
        assert_eq!(
            lifecycle.advance(Duration::from_millis(100), mid_time + Duration::from_millis(50)),
            WindowLifecycleAnim::Opening { progress: half() }
        );
    }
}
