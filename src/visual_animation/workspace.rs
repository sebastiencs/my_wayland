use std::time::{Duration, Instant};

use crate::state::WorkspaceId;

use super::track::{Easing, RetargetableTrack, TrackStatus, UnitValue};

/// Direction of a group-scoped workspace switch.
///
/// `Next` moves toward a later workspace in the group: the incoming workspace enters from the
/// right and the outgoing workspace leaves to the left. `Previous` mirrors that.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkspaceTransitionDirection {
    Next,
    Previous,
}

/// Output-local slide and fade for one workspace. Logical layout is unchanged.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WorkspaceMotion {
    pub translation_x: i32,
    pub alpha: u8,
}

impl WorkspaceMotion {
    pub const IDENTITY: Self = Self { translation_x: 0, alpha: u8::MAX };

    pub fn is_identity(self) -> bool {
        self.translation_x == 0 && self.alpha == u8::MAX
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TransitionPhase {
    Pending { goal: UnitValue },
    Running,
    Paused { goal: UnitValue },
}

/// One live slide-and-fade shared by every output in a workspace group.
///
/// Progress `0` shows `from` in place. Progress `1` shows `to` in place. The animation pump is
/// the only caller that starts or samples the clock. Hyprland owns the same live offset and alpha
/// on the workspace; this compositor keeps one group clock instead of a per-monitor timeline.
#[derive(Clone, Copy, Debug)]
pub struct WorkspaceGroupTransition {
    from: WorkspaceId,
    to: WorkspaceId,
    direction: WorkspaceTransitionDirection,
    full_duration: Duration,
    track: RetargetableTrack,
    cached_progress: UnitValue,
    phase: TransitionPhase,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkspaceTransitionAdvance {
    Unchanged,
    Started,
    Progressed,
    Finished,
}

impl WorkspaceGroupTransition {
    pub fn pending(
        from: WorkspaceId,
        to: WorkspaceId,
        direction: WorkspaceTransitionDirection,
        duration: Duration,
    ) -> Self {
        Self {
            from,
            to,
            direction,
            full_duration: duration,
            track: RetargetableTrack::settled(UnitValue::ZERO),
            cached_progress: UnitValue::ZERO,
            phase: TransitionPhase::Pending { goal: UnitValue::ONE },
        }
    }

    pub fn from_workspace(self) -> WorkspaceId {
        self.from
    }

    pub fn to_workspace(self) -> WorkspaceId {
        self.to
    }

    pub fn direction(self) -> WorkspaceTransitionDirection {
        self.direction
    }

    pub fn progress(self) -> UnitValue {
        self.cached_progress
    }

    pub fn involves(self, workspace_id: WorkspaceId) -> bool {
        workspace_id == self.from || workspace_id == self.to
    }

    pub fn needs_frames(self) -> bool {
        !matches!(self.phase, TransitionPhase::Paused { .. })
    }

    /// Aim the existing pair at `goal` without moving the cached progress.
    ///
    /// `true` means the transition has already reached that goal and can be dropped.
    pub fn request_goal(&mut self, goal: UnitValue, full_duration: Duration) -> bool {
        self.full_duration = full_duration;
        if self.cached_progress == goal || full_duration.is_zero() {
            self.cached_progress = goal;
            self.phase = TransitionPhase::Pending { goal };
            return true;
        }
        self.phase = TransitionPhase::Pending { goal };
        false
    }

    pub fn pause(&mut self) {
        if matches!(self.phase, TransitionPhase::Paused { .. }) {
            return;
        }
        let goal = match self.phase {
            TransitionPhase::Pending { goal } | TransitionPhase::Paused { goal } => goal,
            TransitionPhase::Running => self.track.goal(),
        };
        self.phase = TransitionPhase::Paused { goal };
    }

    pub fn resume(&mut self) {
        if let TransitionPhase::Paused { goal } = self.phase {
            self.phase = TransitionPhase::Pending { goal };
        }
    }

    pub fn motion_for(self, workspace_id: WorkspaceId, output_width: i32) -> WorkspaceMotion {
        let width = output_width.max(0);
        let sign = match self.direction {
            WorkspaceTransitionDirection::Next => 1,
            WorkspaceTransitionDirection::Previous => -1,
        };
        if workspace_id == self.from {
            return WorkspaceMotion {
                translation_x: -sign * self.cached_progress.scale_i32(width),
                alpha: UnitValue::ONE
                    .saturating_sub(self.cached_progress)
                    .lerp_u8(0, u8::MAX),
            };
        }
        if workspace_id == self.to {
            return WorkspaceMotion {
                translation_x: sign
                    * UnitValue::ONE
                        .saturating_sub(self.cached_progress)
                        .scale_i32(width),
                alpha: self.cached_progress.lerp_u8(0, u8::MAX),
            };
        }
        WorkspaceMotion::IDENTITY
    }

    pub fn advance(&mut self, now: Instant, full_duration: Duration) -> WorkspaceTransitionAdvance {
        self.advance_with_easing(now, full_duration, Easing::Linear)
    }

    pub fn advance_with_easing(
        &mut self,
        now: Instant,
        full_duration: Duration,
        easing: Easing,
    ) -> WorkspaceTransitionAdvance {
        if matches!(self.phase, TransitionPhase::Paused { .. }) {
            return WorkspaceTransitionAdvance::Unchanged;
        }
        let duration_changed = self.full_duration != full_duration;
        self.full_duration = full_duration;
        let goal = match self.phase {
            TransitionPhase::Pending { goal } | TransitionPhase::Paused { goal } => goal,
            TransitionPhase::Running => self.track.goal(),
        };
        if full_duration.is_zero() {
            self.cached_progress = goal;
            return WorkspaceTransitionAdvance::Finished;
        }
        if matches!(self.phase, TransitionPhase::Pending { .. }) {
            let distance = if goal >= self.cached_progress {
                goal.saturating_sub(self.cached_progress)
            } else {
                self.cached_progress.saturating_sub(goal)
            };
            let duration = distance.scale_duration(full_duration);
            if duration.is_zero() {
                self.cached_progress = goal;
                return WorkspaceTransitionAdvance::Finished;
            }
            self.track =
                RetargetableTrack::start_timed(self.cached_progress, goal, now, duration, easing);
            self.phase = TransitionPhase::Running;
            return WorkspaceTransitionAdvance::Started;
        }
        if duration_changed || self.track.easing() != easing {
            let duration = remaining_duration(self.cached_progress, goal, full_duration);
            self.track.retarget_timed(goal, now, duration, easing);
        }
        let sample = self.track.sample_mut(now);
        let changed = sample.value != self.cached_progress;
        self.cached_progress = sample.value;
        if sample.status == TrackStatus::Settled {
            return WorkspaceTransitionAdvance::Finished;
        }
        if changed {
            WorkspaceTransitionAdvance::Progressed
        } else {
            WorkspaceTransitionAdvance::Unchanged
        }
    }
}

fn remaining_duration(progress: UnitValue, goal: UnitValue, full_duration: Duration) -> Duration {
    let distance = if goal >= progress {
        goal.saturating_sub(progress)
    } else {
        progress.saturating_sub(goal)
    };
    distance.scale_duration(full_duration)
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use {
        super::{
            WorkspaceGroupTransition, WorkspaceMotion, WorkspaceTransitionAdvance,
            WorkspaceTransitionDirection,
        },
        crate::{state::WorkspaceId, visual_animation::UnitValue},
    };

    fn pair() -> (WorkspaceId, WorkspaceId) {
        (WorkspaceId::from(1usize), WorkspaceId::from(2usize))
    }

    fn pending(direction: WorkspaceTransitionDirection) -> WorkspaceGroupTransition {
        let (from, to) = pair();
        WorkspaceGroupTransition::pending(from, to, direction, Duration::from_millis(200))
    }

    #[test]
    fn pending_transition_is_fully_outgoing_before_the_clock_starts() {
        let (from, to) = pair();
        let transition = pending(WorkspaceTransitionDirection::Next);
        assert_eq!(transition.progress(), UnitValue::ZERO);
        assert_eq!(
            transition.motion_for(from, 800),
            WorkspaceMotion { translation_x: 0, alpha: 255 }
        );
        assert_eq!(
            transition.motion_for(to, 800),
            WorkspaceMotion { translation_x: 800, alpha: 0 }
        );
        assert!(transition.needs_frames());
    }

    #[test]
    fn previous_direction_enters_from_the_left() {
        let (from, to) = pair();
        let transition = pending(WorkspaceTransitionDirection::Previous);
        assert_eq!(transition.motion_for(to, 500).translation_x, -500);
        assert_eq!(transition.motion_for(from, 500).translation_x, 0);
    }

    #[test]
    fn midpoint_slides_and_fades_both_workspaces() {
        let (from, to) = pair();
        let mut transition = pending(WorkspaceTransitionDirection::Next);
        let now = Instant::now();
        assert_eq!(
            transition.advance(now, Duration::from_millis(200)),
            WorkspaceTransitionAdvance::Started
        );
        assert_eq!(transition.progress(), UnitValue::ZERO);
        assert_eq!(
            transition.advance(now + Duration::from_millis(100), Duration::from_millis(200)),
            WorkspaceTransitionAdvance::Progressed
        );
        let outgoing = transition.motion_for(from, 200);
        let incoming = transition.motion_for(to, 200);
        assert!(outgoing.translation_x < 0 && outgoing.translation_x > -200);
        assert!(incoming.translation_x > 0 && incoming.translation_x < 200);
        assert!(outgoing.alpha > 0 && outgoing.alpha < 255);
        assert!(incoming.alpha > 0 && incoming.alpha < 255);
        assert_eq!(outgoing.translation_x, -incoming.translation_x);
    }

    #[test]
    fn completion_settles_on_the_incoming_workspace() {
        let (from, to) = pair();
        let mut transition = pending(WorkspaceTransitionDirection::Next);
        let now = Instant::now();
        assert_eq!(
            transition.advance(now, Duration::from_millis(200)),
            WorkspaceTransitionAdvance::Started
        );
        assert_eq!(
            transition.advance(now + Duration::from_millis(200), Duration::from_millis(200)),
            WorkspaceTransitionAdvance::Finished
        );
        assert_eq!(transition.progress(), UnitValue::ONE);
        assert_eq!(transition.motion_for(from, 100).alpha, 0);
        assert_eq!(
            transition.motion_for(to, 100),
            WorkspaceMotion { translation_x: 0, alpha: 255 }
        );
    }

    #[test]
    fn reverse_keeps_the_current_progress_and_uses_the_remaining_distance() {
        let (from, to) = pair();
        let mut transition = pending(WorkspaceTransitionDirection::Next);
        let now = Instant::now();
        let duration = Duration::from_millis(200);
        transition.advance(now, duration);
        transition.advance(now + Duration::from_millis(100), duration);
        let mid = transition.progress();
        assert!(!transition.request_goal(UnitValue::ZERO, duration));
        assert_eq!(transition.progress(), mid);
        assert_eq!(
            transition.advance(now + Duration::from_millis(100), duration),
            WorkspaceTransitionAdvance::Started
        );
        assert_eq!(transition.progress(), mid);
        assert_eq!(
            transition.advance(now + Duration::from_millis(200), duration),
            WorkspaceTransitionAdvance::Finished
        );
        assert_eq!(transition.progress(), UnitValue::ZERO);
        assert_eq!(transition.motion_for(from, 80).translation_x, 0);
        assert_eq!(transition.motion_for(to, 80).alpha, 0);
    }

    #[test]
    fn forward_retarget_continues_toward_the_incoming_workspace() {
        let mut transition = pending(WorkspaceTransitionDirection::Next);
        let now = Instant::now();
        let duration = Duration::from_millis(100);
        transition.advance(now, duration);
        transition.advance(now + Duration::from_millis(40), duration);
        transition.request_goal(UnitValue::ZERO, duration);
        transition.advance(now + Duration::from_millis(40), duration);
        transition.advance(now + Duration::from_millis(60), duration);
        let reversed = transition.progress();
        assert!(reversed < UnitValue::ONE);
        assert!(!transition.request_goal(UnitValue::ONE, duration));
        assert_eq!(transition.progress(), reversed);
        transition.advance(now + Duration::from_millis(60), duration);
        assert_eq!(
            transition.advance(now + duration + Duration::from_millis(60), duration),
            WorkspaceTransitionAdvance::Finished
        );
        assert_eq!(transition.progress(), UnitValue::ONE);
    }

    #[test]
    fn zero_duration_finishes_on_the_current_goal() {
        let mut transition = pending(WorkspaceTransitionDirection::Next);
        assert_eq!(
            transition.advance(Instant::now(), Duration::ZERO),
            WorkspaceTransitionAdvance::Finished
        );
        assert_eq!(transition.progress(), UnitValue::ONE);
    }

    #[test]
    fn pause_and_resume_do_not_consume_inactive_time() {
        let mut transition = pending(WorkspaceTransitionDirection::Next);
        let now = Instant::now();
        let duration = Duration::from_millis(200);
        transition.advance(now, duration);
        transition.advance(now + Duration::from_millis(50), duration);
        let paused = transition.progress();
        transition.pause();
        assert!(!transition.needs_frames());
        assert_eq!(
            transition.advance(now + Duration::from_millis(10_000), duration),
            WorkspaceTransitionAdvance::Unchanged
        );
        assert_eq!(transition.progress(), paused);
        transition.resume();
        assert_eq!(
            transition.advance(now + Duration::from_millis(10_000), duration),
            WorkspaceTransitionAdvance::Started
        );
        assert_eq!(transition.progress(), paused);
        assert_eq!(
            transition.advance(now + Duration::from_millis(10_100), duration),
            WorkspaceTransitionAdvance::Progressed
        );
        assert!(transition.progress() > paused);
        assert!(transition.progress() < UnitValue::ONE);
    }
}
