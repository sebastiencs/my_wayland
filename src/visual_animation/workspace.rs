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

/// One live slide shared by every output in a workspace group.
///
/// The outgoing workspace and the incoming workspace each keep a progress value. Progress `0`
/// leaves `from` in place and `to` offscreen. Progress `1` leaves `to` in place and `from`
/// offscreen. The two values share a direction, and each one is sampled on its own clock. The
/// animation pump is the only caller that starts or samples those clocks.
#[derive(Clone, Copy, Debug)]
pub struct WorkspaceGroupTransition {
    from: WorkspaceId,
    to: WorkspaceId,
    direction: WorkspaceTransitionDirection,
    outgoing: UnitValue,
    incoming: UnitValue,
    outgoing_track: RetargetableTrack,
    incoming_track: RetargetableTrack,
    outgoing_duration: Duration,
    incoming_duration: Duration,
    goal: UnitValue,
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
    ) -> Self {
        Self {
            from,
            to,
            direction,
            outgoing: UnitValue::ZERO,
            incoming: UnitValue::ZERO,
            outgoing_track: RetargetableTrack::settled(UnitValue::ZERO),
            incoming_track: RetargetableTrack::settled(UnitValue::ZERO),
            outgoing_duration: Duration::ZERO,
            incoming_duration: Duration::ZERO,
            goal: UnitValue::ONE,
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

    /// Outgoing progress. It matches the incoming progress while both clocks use the same timing.
    pub fn progress(self) -> UnitValue {
        self.outgoing
    }

    pub fn incoming_progress(self) -> UnitValue {
        self.incoming
    }

    pub fn goal(self) -> UnitValue {
        self.goal
    }

    pub fn involves(self, workspace_id: WorkspaceId) -> bool {
        workspace_id == self.from || workspace_id == self.to
    }

    pub fn needs_frames(self) -> bool {
        !matches!(self.phase, TransitionPhase::Paused { .. })
    }

    /// Aim both workspaces at `goal` without moving the sampled progress.
    ///
    /// `true` means both workspaces already show that goal and the caller can drop the transition.
    pub fn request_goal(&mut self, goal: UnitValue) -> bool {
        self.goal = goal;
        self.phase = TransitionPhase::Pending { goal };
        self.outgoing == goal && self.incoming == goal
    }

    pub fn pause(&mut self) {
        if matches!(self.phase, TransitionPhase::Paused { .. }) {
            return;
        }
        self.phase = TransitionPhase::Paused { goal: self.goal };
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
                translation_x: -sign * self.outgoing.scale_i32(width),
                alpha: UnitValue::ONE
                    .saturating_sub(self.outgoing)
                    .lerp_u8(0, u8::MAX),
            };
        }
        if workspace_id == self.to {
            return WorkspaceMotion {
                translation_x: sign
                    * UnitValue::ONE
                        .saturating_sub(self.incoming)
                        .scale_i32(width),
                alpha: self.incoming.lerp_u8(0, u8::MAX),
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
        self.advance_sides(now, full_duration, easing, full_duration, easing)
    }

    /// Sample the outgoing workspace on `(from_duration, from_easing)` and the incoming workspace
    /// on `(to_duration, to_easing)`.
    pub fn advance_sides(
        &mut self,
        now: Instant,
        from_duration: Duration,
        from_easing: Easing,
        to_duration: Duration,
        to_easing: Easing,
    ) -> WorkspaceTransitionAdvance {
        if matches!(self.phase, TransitionPhase::Paused { .. }) {
            return WorkspaceTransitionAdvance::Unchanged;
        }
        let goal = match self.phase {
            TransitionPhase::Pending { goal } | TransitionPhase::Paused { goal } => goal,
            TransitionPhase::Running => self.goal,
        };
        self.goal = goal;
        if matches!(self.phase, TransitionPhase::Pending { .. }) {
            let outgoing_done = start_side(
                &mut self.outgoing,
                &mut self.outgoing_track,
                &mut self.outgoing_duration,
                goal,
                now,
                from_duration,
                from_easing,
            );
            let incoming_done = start_side(
                &mut self.incoming,
                &mut self.incoming_track,
                &mut self.incoming_duration,
                goal,
                now,
                to_duration,
                to_easing,
            );
            if outgoing_done && incoming_done {
                return WorkspaceTransitionAdvance::Finished;
            }
            self.phase = TransitionPhase::Running;
            return WorkspaceTransitionAdvance::Started;
        }
        let outgoing = sample_side(
            &mut self.outgoing,
            &mut self.outgoing_track,
            &mut self.outgoing_duration,
            goal,
            now,
            from_duration,
            from_easing,
        );
        let incoming = sample_side(
            &mut self.incoming,
            &mut self.incoming_track,
            &mut self.incoming_duration,
            goal,
            now,
            to_duration,
            to_easing,
        );
        if self.outgoing == goal && self.incoming == goal {
            return WorkspaceTransitionAdvance::Finished;
        }
        if outgoing || incoming {
            WorkspaceTransitionAdvance::Progressed
        } else {
            WorkspaceTransitionAdvance::Unchanged
        }
    }
}

fn start_side(
    progress: &mut UnitValue,
    track: &mut RetargetableTrack,
    applied_duration: &mut Duration,
    goal: UnitValue,
    now: Instant,
    full_duration: Duration,
    easing: Easing,
) -> bool {
    *applied_duration = full_duration;
    if *progress == goal || full_duration.is_zero() {
        *progress = goal;
        return true;
    }
    let duration = remaining_duration(*progress, goal, full_duration);
    if duration.is_zero() {
        *progress = goal;
        return true;
    }
    *track = RetargetableTrack::start_timed(*progress, goal, now, duration, easing);
    false
}

fn sample_side(
    progress: &mut UnitValue,
    track: &mut RetargetableTrack,
    applied_duration: &mut Duration,
    goal: UnitValue,
    now: Instant,
    full_duration: Duration,
    easing: Easing,
) -> bool {
    if *progress == goal {
        return false;
    }
    if full_duration.is_zero() {
        *progress = goal;
        *applied_duration = full_duration;
        return true;
    }
    if *applied_duration != full_duration || track.goal() != goal || track.easing() != easing {
        *applied_duration = full_duration;
        let duration = remaining_duration(*progress, goal, full_duration);
        track.retarget_timed(goal, now, duration, easing);
    }
    let sample = track.sample_mut(now);
    let changed = sample.value != *progress;
    *progress = if sample.status == TrackStatus::Settled { goal } else { sample.value };
    changed || *progress == goal
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
        WorkspaceGroupTransition::pending(from, to, direction)
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
        assert!(!transition.request_goal(UnitValue::ZERO));
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
        transition.request_goal(UnitValue::ZERO);
        transition.advance(now + Duration::from_millis(40), duration);
        transition.advance(now + Duration::from_millis(60), duration);
        let reversed = transition.progress();
        assert!(reversed < UnitValue::ONE);
        assert!(!transition.request_goal(UnitValue::ONE));
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

    #[test]
    fn incoming_clock_finishes_before_the_outgoing_clock() {
        let (from, to) = pair();
        let mut transition = pending(WorkspaceTransitionDirection::Next);
        let now = Instant::now();
        let linear = crate::visual_animation::Easing::Linear;
        assert_eq!(
            transition.advance_sides(
                now,
                Duration::from_millis(200),
                linear,
                Duration::from_millis(100),
                linear,
            ),
            WorkspaceTransitionAdvance::Started
        );
        assert_eq!(
            transition.advance_sides(
                now + Duration::from_millis(100),
                Duration::from_millis(200),
                linear,
                Duration::from_millis(100),
                linear,
            ),
            WorkspaceTransitionAdvance::Progressed
        );
        assert_eq!(transition.incoming_progress(), UnitValue::ONE);
        assert!(transition.progress() > UnitValue::ZERO && transition.progress() < UnitValue::ONE);
        assert_eq!(transition.motion_for(to, 200).translation_x, 0);
        assert_eq!(transition.motion_for(to, 200).alpha, 255);
        assert!(transition.motion_for(from, 200).translation_x < 0);
        assert_eq!(
            transition.advance_sides(
                now + Duration::from_millis(200),
                Duration::from_millis(200),
                linear,
                Duration::from_millis(100),
                linear,
            ),
            WorkspaceTransitionAdvance::Finished
        );
    }
}
