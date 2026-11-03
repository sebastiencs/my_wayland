use std::time::{Duration, Instant};

use crate::{
    animation_config::{AnimationStyle, SlideEdge},
    region::RectI,
};

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
        self.advance_with_easing(goal, duration, Easing::Linear, now)
    }

    pub fn advance_with_easing(
        &mut self,
        goal: WindowVisualBase,
        duration: Duration,
        easing: Easing,
        now: Instant,
    ) -> WindowVisualBase {
        let goal_changed = goal != self.to;
        let duration_changed = duration != self.track.duration();
        let easing_changed = easing != self.track.easing();
        if goal_changed || (self.active && (duration_changed || easing_changed)) {
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
                easing,
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
        self.advance_with_easing(duration, Easing::Linear, now)
    }

    pub fn advance_with_easing(
        &mut self,
        duration: Duration,
        easing: Easing,
        now: Instant,
    ) -> WindowLifecycleAnim {
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
                easing,
            );
        } else if duration != self.track.duration() || easing != self.track.easing() {
            self.track
                .retarget_timed(UnitValue::ONE, now, duration, easing);
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

/// Why the logical window rectangle changed.
///
/// Layout changes animate through the `windows` node. Moving a window onto another workspace or
/// output animates its position through `windowsMove` and its size through `windows`. Pointer
/// motion snaps both axes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WindowGeometryMotion {
    Layout,
    Relocation,
    Warp,
}

/// Duration and curve captured for one geometry axis.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GeometryTiming {
    pub duration: Duration,
    pub easing: Easing,
}

/// Pose and clock for the first sample of an opening window.
///
/// Passed into geometry advance only while the lifecycle sample is still at the start of
/// `windowsIn`. A pointer grab passes [`WindowGeometryMotion::Warp`] and skips this pose.
#[derive(Clone, Copy, Debug)]
pub struct OpeningIntroduction {
    pub style: AnimationStyle,
    pub output: Option<RectI>,
    pub timing: GeometryTiming,
}

/// Start pose for `windowsIn` and end pose for `windowsOut`.
///
/// Popin shrinks toward the center and stays at least 5px on each axis when the goal is at least
/// that large. Slide keeps the goal size. Top and bottom park fully outside `output`; left and
/// right shift by the window's own width. The nearest edge prefers bottom, then left, then right,
/// then top. Gnomed is a full-width line through the vertical center. A slide with no output, and
/// any style that is not a window pose, leaves `goal` unchanged. This matches Hyprland's
/// `animationPopin`, `animationSlide`, and `animationGnomed`.
pub fn lifecycle_pose(style: AnimationStyle, goal: RectI, output: Option<RectI>) -> RectI {
    match style {
        AnimationStyle::Popin { percent } => popin_pose(goal, percent),
        AnimationStyle::Slide { edge } => slide_pose(goal, edge, output),
        AnimationStyle::Gnomed => gnomed_pose(goal),
        AnimationStyle::None | AnimationStyle::Fade | AnimationStyle::SlideFade => goal,
    }
}

fn popin_pose(goal: RectI, percent: u8) -> RectI {
    let width = popin_axis(goal.width, percent);
    let height = popin_axis(goal.height, percent);
    let x = round_i32(f64::from(goal.x) + (f64::from(goal.width) - f64::from(width)) / 2.0);
    let y = round_i32(f64::from(goal.y) + (f64::from(goal.height) - f64::from(height)) / 2.0);
    RectI::new(x, y, width, height).unwrap_or(goal)
}

fn popin_axis(goal: i32, percent: u8) -> i32 {
    let goal = goal.max(0);
    if goal == 0 {
        return 0;
    }
    let scaled = (f64::from(goal) * f64::from(percent) / 100.0).round();
    round_i32(scaled.max(5.0).min(f64::from(goal))).clamp(0, goal)
}

fn slide_pose(goal: RectI, edge: Option<SlideEdge>, output: Option<RectI>) -> RectI {
    let Some(output) = output else {
        return goal;
    };
    let edge = edge.unwrap_or_else(|| nearest_slide_edge(goal, output));
    let (x, y) = match edge {
        SlideEdge::Left => (goal.x.checked_sub(goal.width).unwrap_or(i32::MIN), goal.y),
        SlideEdge::Right => (goal.x.checked_add(goal.width).unwrap_or(i32::MAX), goal.y),
        SlideEdge::Top => (goal.x, output.y.checked_sub(goal.height).unwrap_or(i32::MIN)),
        SlideEdge::Bottom => (goal.x, output.y.checked_add(output.height).unwrap_or(i32::MAX)),
    };
    RectI::new(x, y, goal.width, goal.height).unwrap_or(goal)
}

fn nearest_slide_edge(goal: RectI, output: RectI) -> SlideEdge {
    let mid_x = f64::from(goal.x) + f64::from(goal.width) / 2.0;
    let mid_y = f64::from(goal.y) + f64::from(goal.height) / 2.0;
    let top = mid_y - f64::from(output.y);
    let right = f64::from(output.x) + f64::from(output.width) - mid_x;
    let bottom = f64::from(output.y) + f64::from(output.height) - mid_y;
    let left = mid_x - f64::from(output.x);
    let min = top.min(right).min(bottom).min(left);
    if bottom == min {
        SlideEdge::Bottom
    } else if left == min {
        SlideEdge::Left
    } else if right == min {
        SlideEdge::Right
    } else {
        SlideEdge::Top
    }
}

fn gnomed_pose(goal: RectI) -> RectI {
    let y = round_i32(f64::from(goal.y) + f64::from(goal.height) / 2.0);
    RectI::new(goal.x, y, goal.width, 0).unwrap_or(goal)
}

fn style_has_pose(style: AnimationStyle) -> bool {
    matches!(
        style,
        AnimationStyle::Popin { .. } | AnimationStyle::Slide { .. } | AnimationStyle::Gnomed
    )
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct WindowBox {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

impl WindowBox {
    fn from_rect(rect: RectI) -> Self {
        Self {
            x: f64::from(rect.x),
            y: f64::from(rect.y),
            width: f64::from(rect.width),
            height: f64::from(rect.height),
        }
    }

    fn rounded(self) -> Option<RectI> {
        RectI::new(
            round_i32(self.x),
            round_i32(self.y),
            round_i32(self.width).max(0),
            round_i32(self.height).max(0),
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PositionClass {
    Layout,
    Relocation,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct PendingGeometry {
    rect: RectI,
    motion: WindowGeometryMotion,
}

#[derive(Clone, Copy, Debug)]
struct AxisAnimation {
    from_x: f64,
    from_y: f64,
    goal_x: f64,
    goal_y: f64,
    current_x: f64,
    current_y: f64,
    track: RetargetableTrack,
    active: bool,
}

impl AxisAnimation {
    fn settled(x: f64, y: f64) -> Self {
        Self {
            from_x: x,
            from_y: y,
            goal_x: x,
            goal_y: y,
            current_x: x,
            current_y: y,
            track: RetargetableTrack::settled(UnitValue::ONE),
            active: false,
        }
    }

    fn goal_is(self, x: f64, y: f64) -> bool {
        self.goal_x == x && self.goal_y == y
    }

    fn warp(&mut self, x: f64, y: f64) {
        *self = Self::settled(x, y);
    }

    fn retarget(&mut self, x: f64, y: f64, now: Instant, timing: GeometryTiming) {
        if timing.duration.is_zero() || (self.current_x == x && self.current_y == y) {
            self.warp(x, y);
            return;
        }
        self.from_x = self.current_x;
        self.from_y = self.current_y;
        self.goal_x = x;
        self.goal_y = y;
        self.track = RetargetableTrack::start_timed(
            UnitValue::ZERO,
            UnitValue::ONE,
            now,
            timing.duration,
            timing.easing,
        );
        self.active = !timing.duration.is_zero();
    }

    fn advance(&mut self, now: Instant, timing: GeometryTiming) {
        if !self.active {
            return;
        }
        if timing.duration.is_zero() {
            self.warp(self.goal_x, self.goal_y);
            return;
        }
        if timing.duration != self.track.duration() || timing.easing != self.track.easing() {
            self.track
                .retarget_timed(UnitValue::ONE, now, timing.duration, timing.easing);
        }
        let sample = self.track.sample_mut(now);
        self.current_x = lerp_axis(self.from_x, self.goal_x, sample.value);
        self.current_y = lerp_axis(self.from_y, self.goal_y, sample.value);
        if sample.status == TrackStatus::Settled {
            self.current_x = self.goal_x;
            self.current_y = self.goal_y;
            self.active = false;
        }
    }
}

/// Animated window position and size.
///
/// The logical layout rectangle is the goal. Each axis keeps its own sample, so a size change
/// leaves an in-flight move running. The first valid rectangle snaps, except an opening window,
/// which starts at [`lifecycle_pose`] and moves to that rectangle on the `windowsIn` clock.
/// Later goals are applied by [`Self::advance`] so the animation pump remains the clock.
#[derive(Clone, Copy, Debug)]
pub struct WindowGeometryAnimation {
    armed: bool,
    position: AxisAnimation,
    size: AxisAnimation,
    position_class: PositionClass,
    relocation_pending: bool,
    pending: Option<PendingGeometry>,
    introduced: Option<GeometryTiming>,
}

impl WindowGeometryAnimation {
    pub fn unarmed() -> Self {
        Self {
            armed: false,
            position: AxisAnimation::settled(0.0, 0.0),
            size: AxisAnimation::settled(0.0, 0.0),
            position_class: PositionClass::Layout,
            relocation_pending: false,
            pending: None,
            introduced: None,
        }
    }

    pub fn is_armed(self) -> bool {
        self.armed
    }

    pub fn is_active(self) -> bool {
        self.position.active || self.size.active
    }

    /// The next layout-driven goal should use the relocation position node.
    pub fn prefer_relocation(&mut self) {
        self.relocation_pending = true;
    }

    /// Record a new logical rectangle. Equal goals do not restart a running animation.
    pub fn set_goal(&mut self, rect: RectI, mut motion: WindowGeometryMotion) {
        if rect.width <= 0 || rect.height <= 0 {
            return;
        }
        if self.relocation_pending {
            self.relocation_pending = false;
            if motion == WindowGeometryMotion::Layout {
                motion = WindowGeometryMotion::Relocation;
            }
        }
        if self.armed && self.pending.is_none() && self.goal_is(rect) {
            let settled = !self.position.active && !self.size.active;
            if motion != WindowGeometryMotion::Warp || settled {
                return;
            }
        }
        self.pending = Some(PendingGeometry { rect, motion });
    }

    pub fn goal_rect(self) -> Option<RectI> {
        if let Some(pending) = self.pending {
            return Some(pending.rect);
        }
        self.armed.then(|| {
            RectI::new(
                round_i32(self.position.goal_x),
                round_i32(self.position.goal_y),
                round_i32(self.size.goal_x),
                round_i32(self.size.goal_y),
            )
            .expect("armed geometry goal")
        })
    }

    pub fn visual_rect(self) -> Option<RectI> {
        if !self.armed {
            return None;
        }
        WindowBox {
            x: self.position.current_x,
            y: self.position.current_y,
            width: self.size.current_x,
            height: self.size.current_y,
        }
        .rounded()
    }

    pub fn advance(&mut self, now: Instant, layout: GeometryTiming, moving: GeometryTiming) {
        self.advance_with_introduction(now, layout, moving, None);
    }

    pub fn advance_with_introduction(
        &mut self,
        now: Instant,
        layout: GeometryTiming,
        moving: GeometryTiming,
        opening: Option<OpeningIntroduction>,
    ) {
        if let Some(pending) = self.pending.take() {
            self.apply_goal(pending, now, layout, moving, opening);
        }
        let position_timing = if let Some(timing) = self.introduced {
            timing
        } else if self.position_class == PositionClass::Relocation {
            moving
        } else {
            layout
        };
        let size_timing = self.introduced.unwrap_or(layout);
        self.position.advance(now, position_timing);
        self.size.advance(now, size_timing);
        if !self.position.active && !self.size.active {
            self.introduced = None;
        }
    }

    fn goal_is(self, rect: RectI) -> bool {
        let goal = WindowBox::from_rect(rect);
        self.position.goal_is(goal.x, goal.y) && self.size.goal_is(goal.width, goal.height)
    }

    fn apply_goal(
        &mut self,
        pending: PendingGeometry,
        now: Instant,
        layout: GeometryTiming,
        moving: GeometryTiming,
        opening: Option<OpeningIntroduction>,
    ) {
        let goal = WindowBox::from_rect(pending.rect);
        if pending.motion != WindowGeometryMotion::Warp {
            if let Some(opening) = opening.filter(|opening| style_has_pose(opening.style)) {
                let pose = WindowBox::from_rect(lifecycle_pose(
                    opening.style,
                    pending.rect,
                    opening.output,
                ));
                self.position.warp(pose.x, pose.y);
                self.size.warp(pose.width, pose.height);
                self.position_class = PositionClass::Layout;
                self.armed = true;
                self.introduced = Some(opening.timing);
                self.position.retarget(goal.x, goal.y, now, opening.timing);
                self.size
                    .retarget(goal.width, goal.height, now, opening.timing);
                return;
            }
        }
        self.introduced = None;
        if !self.armed || pending.motion == WindowGeometryMotion::Warp {
            self.position.warp(goal.x, goal.y);
            self.size.warp(goal.width, goal.height);
            self.position_class = PositionClass::Layout;
            self.armed = true;
            return;
        }
        if !self.position.goal_is(goal.x, goal.y) {
            let (timing, class) = if pending.motion == WindowGeometryMotion::Relocation {
                (moving, PositionClass::Relocation)
            } else {
                (layout, PositionClass::Layout)
            };
            self.position_class = class;
            self.position.retarget(goal.x, goal.y, now, timing);
        }
        if !self.size.goal_is(goal.width, goal.height) {
            self.size.retarget(goal.width, goal.height, now, layout);
        }
        self.armed = true;
    }
}

fn lerp_axis(from: f64, to: f64, progress: UnitValue) -> f64 {
    from + (to - from) * progress.as_f64()
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
mod tests {
    use std::time::{Duration, Instant};

    use {
        super::{
            interpolate_window_visual, lifecycle_pose, resolve_window_visual, Easing,
            FocusVisualAnimation, GeometryTiming, OpeningIntroduction, WindowGeometryAnimation,
            WindowGeometryMotion, WindowLifecycleAnim, WindowLifecycleAnimation, WindowVisualBase,
        },
        crate::{
            animation_config::{AnimationStyle, SlideEdge},
            region::RectI,
            visual_animation::UnitValue,
        },
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

    fn linear(millis: u64) -> GeometryTiming {
        GeometryTiming {
            duration: Duration::from_millis(millis),
            easing: Easing::Linear,
        }
    }

    fn rect(x: i32, y: i32, width: i32, height: i32) -> RectI {
        RectI::new(x, y, width, height).unwrap()
    }

    #[test]
    fn first_rectangle_snaps_instead_of_starting_at_the_origin() {
        let now = Instant::now();
        let mut geometry = WindowGeometryAnimation::unarmed();
        geometry.set_goal(rect(40, 50, 200, 100), WindowGeometryMotion::Layout);
        assert!(geometry.visual_rect().is_none());
        assert_eq!(geometry.goal_rect(), Some(rect(40, 50, 200, 100)));

        geometry.advance(now, linear(400), linear(400));
        assert_eq!(geometry.visual_rect(), Some(rect(40, 50, 200, 100)));
        assert!(!geometry.is_active());
    }

    #[test]
    fn position_and_size_retarget_independently() {
        let now = Instant::now();
        let mut geometry = WindowGeometryAnimation::unarmed();
        geometry.set_goal(rect(0, 0, 100, 40), WindowGeometryMotion::Layout);
        geometry.advance(now, linear(100), linear(400));

        geometry.set_goal(rect(80, 0, 100, 40), WindowGeometryMotion::Layout);
        geometry.advance(now, linear(100), linear(400));
        assert_eq!(geometry.visual_rect(), Some(rect(0, 0, 100, 40)));

        geometry.advance(now + Duration::from_millis(50), linear(100), linear(400));
        assert_eq!(geometry.visual_rect(), Some(rect(40, 0, 100, 40)));

        geometry.set_goal(rect(80, 0, 140, 40), WindowGeometryMotion::Layout);
        geometry.advance(now + Duration::from_millis(50), linear(100), linear(400));
        geometry.advance(now + Duration::from_millis(100), linear(100), linear(400));
        let mid_size = geometry.visual_rect().unwrap();
        assert_eq!(mid_size.x, 80);
        assert_eq!(mid_size.width, 120);
    }

    #[test]
    fn relocation_uses_the_moving_duration_for_position_only() {
        let now = Instant::now();
        let mut geometry = WindowGeometryAnimation::unarmed();
        geometry.set_goal(rect(0, 0, 20, 20), WindowGeometryMotion::Layout);
        geometry.advance(now, linear(100), linear(200));

        geometry.set_goal(rect(40, 0, 60, 20), WindowGeometryMotion::Relocation);
        geometry.advance(now, linear(100), linear(200));
        geometry.advance(now + Duration::from_millis(100), linear(100), linear(200));
        let sampled = geometry.visual_rect().unwrap();
        assert_eq!(sampled.x, 20);
        assert_eq!(sampled.width, 60);
        assert!(geometry.is_active());
    }

    #[test]
    fn equal_goal_does_not_restart_and_warp_snaps() {
        let now = Instant::now();
        let mut geometry = WindowGeometryAnimation::unarmed();
        geometry.set_goal(rect(0, 0, 10, 10), WindowGeometryMotion::Layout);
        geometry.advance(now, linear(100), linear(100));
        geometry.set_goal(rect(30, 0, 10, 10), WindowGeometryMotion::Layout);
        geometry.advance(now, linear(100), linear(100));
        geometry.set_goal(rect(30, 0, 10, 10), WindowGeometryMotion::Layout);
        geometry.advance(now + Duration::from_millis(50), linear(100), linear(100));
        assert_eq!(geometry.visual_rect(), Some(rect(15, 0, 10, 10)));

        geometry.set_goal(rect(30, 0, 10, 10), WindowGeometryMotion::Warp);
        geometry.advance(now + Duration::from_millis(50), linear(100), linear(100));
        assert_eq!(geometry.visual_rect(), Some(rect(30, 0, 10, 10)));
        assert!(!geometry.is_active());
    }

    #[test]
    fn rounded_visual_rect_is_stable_at_the_same_sample() {
        let now = Instant::now();
        let mut geometry = WindowGeometryAnimation::unarmed();
        geometry.set_goal(rect(0, 0, 10, 10), WindowGeometryMotion::Layout);
        geometry.advance(now, linear(100), linear(100));
        geometry.set_goal(rect(1, 0, 11, 10), WindowGeometryMotion::Layout);
        geometry.advance(now, linear(100), linear(100));
        let mid = now + Duration::from_millis(50);
        geometry.advance(mid, linear(100), linear(100));
        let first = geometry.visual_rect();
        geometry.advance(mid, linear(100), linear(100));
        assert_eq!(first, geometry.visual_rect());
        assert_eq!(first, Some(rect(1, 0, 11, 10)));
        geometry.advance(now + Duration::from_millis(100), linear(100), linear(100));
        assert_eq!(geometry.visual_rect(), geometry.goal_rect());
    }

    fn goal() -> RectI {
        rect(100, 200, 80, 60)
    }

    fn output() -> RectI {
        rect(0, 0, 800, 600)
    }

    fn intro(style: AnimationStyle, output: Option<RectI>, millis: u64) -> OpeningIntroduction {
        OpeningIntroduction { style, output, timing: linear(millis) }
    }

    #[test]
    fn popin_pose_is_centered_and_clamped_to_five_pixels() {
        assert_eq!(
            lifecycle_pose(AnimationStyle::Popin { percent: 87 }, goal(), None),
            rect(105, 204, 70, 52)
        );
        assert_eq!(
            lifecycle_pose(AnimationStyle::Popin { percent: 0 }, goal(), None),
            rect(138, 228, 5, 5)
        );
        assert_eq!(lifecycle_pose(AnimationStyle::Popin { percent: 100 }, goal(), None), goal());
        assert_eq!(
            lifecycle_pose(AnimationStyle::Popin { percent: 0 }, rect(0, 0, 3, 4), None),
            rect(0, 0, 3, 4)
        );
    }

    #[test]
    fn slide_pose_uses_hyprland_edge_offsets() {
        let style = |edge| AnimationStyle::Slide { edge: Some(edge) };
        assert_eq!(
            lifecycle_pose(style(SlideEdge::Left), goal(), Some(output())),
            rect(20, 200, 80, 60)
        );
        assert_eq!(
            lifecycle_pose(style(SlideEdge::Right), goal(), Some(output())),
            rect(180, 200, 80, 60)
        );
        assert_eq!(
            lifecycle_pose(style(SlideEdge::Top), goal(), Some(output())),
            rect(100, -60, 80, 60)
        );
        assert_eq!(
            lifecycle_pose(style(SlideEdge::Bottom), goal(), Some(output())),
            rect(100, 600, 80, 60)
        );
        assert_eq!(lifecycle_pose(AnimationStyle::Slide { edge: None }, goal(), None), goal());
    }

    #[test]
    fn nearest_slide_prefers_bottom_then_left_then_right_then_top() {
        let style = AnimationStyle::Slide { edge: None };
        let tied = rect(350, 260, 100, 80);
        assert_eq!(lifecycle_pose(style, tied, Some(output())), rect(350, 600, 100, 80));
        let near_left = rect(10, 270, 100, 60);
        assert_eq!(lifecycle_pose(style, near_left, Some(output())), rect(-90, 270, 100, 60));
        let near_right = rect(700, 270, 80, 60);
        assert_eq!(lifecycle_pose(style, near_right, Some(output())), rect(780, 270, 80, 60));
        let near_top = rect(350, 10, 100, 40);
        assert_eq!(lifecycle_pose(style, near_top, Some(output())), rect(350, -40, 100, 40));
    }

    #[test]
    fn gnomed_pose_is_a_horizontal_line_through_the_center() {
        assert_eq!(lifecycle_pose(AnimationStyle::Gnomed, goal(), None), rect(100, 230, 80, 0));
        assert_eq!(lifecycle_pose(AnimationStyle::None, goal(), Some(output())), goal());
    }

    #[test]
    fn opening_pose_uses_windows_in_timing_when_layout_timing_is_disabled() {
        let now = Instant::now();
        let mut geometry = WindowGeometryAnimation::unarmed();
        geometry.prefer_relocation();
        geometry.set_goal(goal(), WindowGeometryMotion::Layout);
        geometry.advance_with_introduction(
            now,
            linear(0),
            linear(500),
            Some(intro(AnimationStyle::Popin { percent: 50 }, None, 100)),
        );
        assert_eq!(geometry.visual_rect(), Some(rect(120, 215, 40, 30)));
        assert_eq!(geometry.goal_rect(), Some(goal()));
        assert!(geometry.is_active());

        geometry.advance_with_introduction(
            now + Duration::from_millis(50),
            linear(0),
            linear(0),
            None,
        );
        let mid = geometry.visual_rect().unwrap();
        assert_ne!(mid, goal());
        assert!(geometry.is_active());

        geometry.advance_with_introduction(
            now + Duration::from_millis(100),
            linear(0),
            linear(0),
            None,
        );
        assert_eq!(geometry.visual_rect(), Some(goal()));
        assert!(!geometry.is_active());
    }

    #[test]
    fn warp_during_opening_snaps_to_the_pointer_goal() {
        let now = Instant::now();
        let mut geometry = WindowGeometryAnimation::unarmed();
        geometry.set_goal(goal(), WindowGeometryMotion::Layout);
        geometry.advance_with_introduction(
            now,
            linear(0),
            linear(0),
            Some(intro(AnimationStyle::Gnomed, None, 200)),
        );
        assert_eq!(geometry.visual_rect(), Some(rect(100, 230, 80, 0)));

        geometry.set_goal(rect(4, 5, 20, 10), WindowGeometryMotion::Warp);
        geometry.advance_with_introduction(
            now + Duration::from_millis(20),
            linear(0),
            linear(0),
            Some(intro(AnimationStyle::Gnomed, None, 200)),
        );
        assert_eq!(geometry.visual_rect(), Some(rect(4, 5, 20, 10)));
        assert!(!geometry.is_active());
    }

    #[test]
    fn later_layout_goal_leaves_the_opening_pose() {
        let now = Instant::now();
        let mut geometry = WindowGeometryAnimation::unarmed();
        geometry.set_goal(goal(), WindowGeometryMotion::Layout);
        geometry.advance_with_introduction(
            now,
            linear(200),
            linear(200),
            Some(intro(AnimationStyle::Slide { edge: Some(SlideEdge::Left) }, Some(output()), 200)),
        );
        assert_eq!(geometry.visual_rect(), Some(rect(20, 200, 80, 60)));

        geometry.set_goal(rect(300, 200, 80, 60), WindowGeometryMotion::Layout);
        geometry.advance_with_introduction(
            now + Duration::from_millis(100),
            linear(200),
            linear(200),
            None,
        );
        assert_eq!(geometry.visual_rect(), Some(rect(20, 200, 80, 60)));
        geometry.advance_with_introduction(
            now + Duration::from_millis(200),
            linear(200),
            linear(200),
            None,
        );
        assert_eq!(geometry.visual_rect(), Some(rect(160, 200, 80, 60)));
        assert_eq!(geometry.goal_rect(), Some(rect(300, 200, 80, 60)));
    }
}
