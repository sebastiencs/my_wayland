use std::time::{Duration, Instant};

use crate::state::WorkspaceId;

use super::track::{unit_progress_between, UnitValue};

/// Direction of a group-scoped workspace switch.
///
/// Sampling uses absolute time only. Direction is retained for a later phase that derives
/// translation; Phase 3 does not move either workspace.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkspaceTransitionDirection {
    Next,
    Previous,
}

/// One logical transition shared by every output in a workspace group.
///
/// This value is not stored on `WorkspaceGroup` yet. Production composition stays on the steady
/// active-workspace policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WorkspaceGroupTransition {
    pub from: WorkspaceId,
    pub to: WorkspaceId,
    pub started_at: Instant,
    pub duration: Duration,
    pub direction: WorkspaceTransitionDirection,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WorkspaceTransitionSample {
    pub progress: UnitValue,
    pub outgoing_alpha: u8,
    pub incoming_alpha: u8,
    pub outgoing_translation: (i32, i32),
    pub incoming_translation: (i32, i32),
}

impl WorkspaceGroupTransition {
    /// Sample progress from the transition clock. The result does not depend on an output.
    pub fn sample(self, now: Instant) -> WorkspaceTransitionSample {
        let progress = unit_progress_between(self.started_at, self.duration, now);
        WorkspaceTransitionSample {
            progress,
            outgoing_alpha: UnitValue::ONE.saturating_sub(progress).lerp_u8(0, u8::MAX),
            incoming_alpha: progress.lerp_u8(0, u8::MAX),
            outgoing_translation: (0, 0),
            incoming_translation: (0, 0),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use {
        super::{WorkspaceGroupTransition, WorkspaceTransitionDirection},
        crate::{state::WorkspaceId, visual_animation::UnitValue},
    };

    fn transition(now: Instant) -> WorkspaceGroupTransition {
        WorkspaceGroupTransition {
            from: WorkspaceId::from(1usize),
            to: WorkspaceId::from(2usize),
            started_at: now,
            duration: Duration::from_millis(200),
            direction: WorkspaceTransitionDirection::Next,
        }
    }

    #[test]
    fn transition_endpoints_use_one_absolute_clock() {
        let now = Instant::now();
        let transition = transition(now);
        let start = transition.sample(now);
        let start_again = transition.sample(now);

        assert_eq!(start, start_again);
        assert_eq!(start.progress, UnitValue::ZERO);
        assert_eq!(start.outgoing_alpha, u8::MAX);
        assert_eq!(start.incoming_alpha, 0);
        assert_eq!(start.outgoing_translation, (0, 0));
        assert_eq!(start.incoming_translation, (0, 0));

        let end = transition.sample(now + Duration::from_millis(200));
        assert_eq!(end.progress, UnitValue::ONE);
        assert_eq!(end.outgoing_alpha, 0);
        assert_eq!(end.incoming_alpha, u8::MAX);
        assert_eq!(
            transition.sample(now + Duration::from_secs(2)),
            end,
            "progress stays settled after the shared duration"
        );
    }
}
