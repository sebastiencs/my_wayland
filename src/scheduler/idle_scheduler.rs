//! One monotonic timer for user-idle deadlines.
//!
//! wlroots, Smithay, and Hyprland each keep a timer per idle notification.
//! Hyprland also tracks elapsed time since the last activity and arms the
//! remainder. This scheduler stores the same absolute deadlines and arms one
//! `CLOCK_MONOTONIC` timerfd at the soonest of them, so a static desktop wakes
//! without a frame. The output frame clock is a separate timer and disarms on
//! session pause. Idle time keeps running across a VT switch: this timer does
//! not.
//!
//! A held key or button does not move a deadline. Weston postpones idle until
//! release; the activity clock follows the last real event instead, matching
//! wlroots, Smithay, and Hyprland. Phase 2 turns elapsed deadlines into
//! `idled`. Nothing here advertises a protocol global.

use std::{
    collections::BTreeMap,
    os::fd::{AsFd, AsRawFd},
    time::Instant,
};

use nix::sys::timerfd::{self, Expiration, TimerFd, TimerFlags, TimerSetTimeFlags};

use crate::{
    io_uring::{model::IoUringAction, ring::RingId},
    runner::{AnyAction, Dispatcher, Model, Redispatcher},
    state::State,
};

/// Identity of one registered deadline. Phase 2 mints one per notification.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct IdleDeadlineId(u64);

impl IdleDeadlineId {
    pub const fn from_raw(raw: u64) -> Self {
        Self(raw)
    }

    pub const fn raw(self) -> u64 {
        self.0
    }
}

/// Runtime shell around the idle deadline set.
///
/// Deadlines live here, not on the frame clock. Tests drive [`IdleSchedulerAction::Evaluate`]
/// with a chosen instant, or inject [`IdleSchedulerAction::Fired`], and never sleep.
#[derive(Debug)]
pub struct IdleScheduler {
    timer_fd: TimerFd,
    read_op_id: Option<RingId>,
    deadlines: BTreeMap<IdleDeadlineId, Instant>,
    armed_deadline: Option<Instant>,
    /// Deadlines reported by [`IdleScheduler::expire`], in (time, id) order.
    fired: Vec<IdleDeadlineId>,
}

#[derive(Debug)]
pub enum IdleSchedulerAction {
    Start,
    SetDeadline {
        id: IdleDeadlineId,
        at: Instant,
    },
    ClearDeadline {
        id: IdleDeadlineId,
    },
    /// Report deadlines at or before `now`, then rearm to whatever remains.
    Evaluate {
        now: Instant,
    },
    Fired,
    Failed {
        error: std::io::Error,
    },
    Stop,
}

impl IdleScheduler {
    pub fn new() -> Self {
        let timer_fd = TimerFd::new(
            timerfd::ClockId::CLOCK_MONOTONIC,
            TimerFlags::TFD_CLOEXEC | TimerFlags::TFD_NONBLOCK,
        )
        .expect("failed to create idle scheduler timerfd");
        Self {
            timer_fd,
            read_op_id: None,
            deadlines: BTreeMap::new(),
            armed_deadline: None,
            fired: Vec::new(),
        }
    }

    pub fn armed_deadline(&self) -> Option<Instant> {
        self.armed_deadline
    }

    pub fn fired(&self) -> &[IdleDeadlineId] {
        &self.fired
    }

    pub fn deadline(&self, id: IdleDeadlineId) -> Option<Instant> {
        self.deadlines.get(&id).copied()
    }

    pub fn timer_is_armed(&self) -> bool {
        self.timer_fd.get().ok().flatten().is_some()
    }

    fn expire(&mut self, now: Instant) {
        let mut due: Vec<(Instant, IdleDeadlineId)> = self
            .deadlines
            .iter()
            .filter(|(_, at)| **at <= now)
            .map(|(id, at)| (*at, *id))
            .collect();
        if due.is_empty() {
            return;
        }
        due.sort_unstable();
        for (_, id) in &due {
            self.deadlines.remove(id);
        }
        self.fired.extend(due.into_iter().map(|(_, id)| id));
    }

    /// Arm the soonest remaining deadline. A zero `it_value` disarms a
    /// timerfd, so a deadline that is already due is reported here instead of
    /// programmed as a zero timeout.
    fn arm(&mut self, dispatcher: &mut Dispatcher) {
        loop {
            let Some(deadline) = self.deadlines.values().copied().min() else {
                self.armed_deadline = None;
                let _ = self.timer_fd.unset();
                break;
            };
            let now = Instant::now();
            if deadline <= now {
                self.expire(now);
                continue;
            }
            self.armed_deadline = Some(deadline);
            let expiration = Expiration::OneShot(deadline.duration_since(now).into());
            if let Err(error) = self.timer_fd.set(expiration, TimerSetTimeFlags::empty()) {
                log::warn!("failed to arm idle scheduler timer: {error}");
            }
            break;
        }
        self.dispatch_read(dispatcher);
    }

    fn dispatch_read(&mut self, dispatcher: &mut Dispatcher) {
        if self.read_op_id.is_some() {
            return;
        }
        let read_op_id = RingId::fresh();
        self.read_op_id = Some(read_op_id);
        dispatcher.dispatch(IoUringAction::Read {
            id: Some(read_op_id),
            fd: self.timer_fd.as_fd().as_raw_fd(),
            offset: 0,
            length: std::mem::size_of::<u64>(),
            dispatch_to: Redispatcher::to_any(|result| match result {
                Ok(_) => AnyAction::from(IdleSchedulerAction::Fired),
                Err(error) => AnyAction::from(IdleSchedulerAction::Failed { error }),
            }),
        });
    }
}

impl Model for IdleScheduler {
    type Action = IdleSchedulerAction;

    const START: Option<Self::Action> = Some(IdleSchedulerAction::Start);
    const STOP: Option<Self::Action> = Some(IdleSchedulerAction::Stop);

    fn process(
        &mut self,
        state: &mut State,
        action: Self::Action,
        dispatcher: &mut Dispatcher,
    ) -> std::result::Result<(), crate::runner::ModelFailure> {
        match action {
            IdleSchedulerAction::Start => self.arm(dispatcher),
            IdleSchedulerAction::SetDeadline { id, at } => {
                self.deadlines.insert(id, at);
                self.arm(dispatcher);
                crate::wayland_objects::ext_idle_notification_v1::evaluate(
                    state,
                    dispatcher,
                    Instant::now(),
                );
            }
            IdleSchedulerAction::ClearDeadline { id } => {
                self.deadlines.remove(&id);
                self.arm(dispatcher);
            }
            IdleSchedulerAction::Evaluate { now } => {
                self.expire(now);
                self.arm(dispatcher);
                crate::wayland_objects::ext_idle_notification_v1::evaluate(state, dispatcher, now);
            }
            IdleSchedulerAction::Fired => {
                self.read_op_id = None;
                let now = Instant::now();
                self.expire(now);
                self.arm(dispatcher);
                crate::wayland_objects::ext_idle_notification_v1::evaluate(state, dispatcher, now);
            }
            IdleSchedulerAction::Failed { error } => {
                self.read_op_id = None;
                log::warn!("idle scheduler timer read failed: {error}");
                self.dispatch_read(dispatcher);
            }
            IdleSchedulerAction::Stop => {
                self.armed_deadline = None;
                let _ = self.timer_fd.unset();
                if let Some(read_op_id) = self.read_op_id.take() {
                    dispatcher.dispatch(IoUringAction::Cancel { target: read_op_id });
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use {
        super::*,
        crate::{
            io_uring::model::IoUringAction, runner::Dispatcher, test_support::state_for_tests,
        },
        std::time::Duration,
    };

    fn id(raw: u64) -> IdleDeadlineId {
        IdleDeadlineId::from_raw(raw)
    }

    fn later(secs: u64) -> Instant {
        Instant::now() + Duration::from_secs(secs)
    }

    #[test]
    fn start_with_no_deadlines_leaves_the_timer_disarmed() {
        let mut state = state_for_tests();
        let mut scheduler = IdleScheduler::new();
        let mut dispatcher = Dispatcher::default();
        scheduler
            .process(&mut state, IdleSchedulerAction::Start, &mut dispatcher)
            .unwrap();
        assert!(scheduler.armed_deadline().is_none());
        assert!(!scheduler.timer_is_armed());
        assert!(dispatcher.test_has_queued_action(|action: &IoUringAction| {
            matches!(action, IoUringAction::Read { length: 8, .. })
        }));
    }

    #[test]
    fn the_timer_arms_to_the_sooner_of_two_deadlines() {
        let mut state = state_for_tests();
        let mut scheduler = IdleScheduler::new();
        let mut dispatcher = Dispatcher::default();
        let sooner = later(3_600);
        let later_deadline = later(7_200);
        scheduler
            .process(
                &mut state,
                IdleSchedulerAction::SetDeadline { id: id(1), at: later_deadline },
                &mut dispatcher,
            )
            .unwrap();
        scheduler
            .process(
                &mut state,
                IdleSchedulerAction::SetDeadline { id: id(2), at: sooner },
                &mut dispatcher,
            )
            .unwrap();
        assert_eq!(scheduler.armed_deadline(), Some(sooner));
        assert!(scheduler.timer_is_armed());
        assert_eq!(
            dispatcher.test_count_queued_action(|action: &IoUringAction| {
                matches!(action, IoUringAction::Read { .. })
            }),
            1
        );
    }

    #[test]
    fn evaluate_reports_only_deadlines_at_or_before_now_and_rearms() {
        let mut state = state_for_tests();
        let mut scheduler = IdleScheduler::new();
        let mut dispatcher = Dispatcher::default();
        let sooner = later(3_600);
        let later_deadline = later(7_200);
        scheduler
            .process(
                &mut state,
                IdleSchedulerAction::SetDeadline { id: id(7), at: later_deadline },
                &mut dispatcher,
            )
            .unwrap();
        scheduler
            .process(
                &mut state,
                IdleSchedulerAction::SetDeadline { id: id(3), at: sooner },
                &mut dispatcher,
            )
            .unwrap();
        scheduler
            .process(&mut state, IdleSchedulerAction::Evaluate { now: sooner }, &mut dispatcher)
            .unwrap();
        assert_eq!(scheduler.fired(), &[id(3)]);
        assert_eq!(scheduler.deadline(id(3)), None);
        assert_eq!(scheduler.deadline(id(7)), Some(later_deadline));
        assert_eq!(scheduler.armed_deadline(), Some(later_deadline));
        assert!(scheduler.timer_is_armed());
    }

    #[test]
    fn equal_deadlines_fire_in_id_order_and_a_second_pass_does_not_repeat() {
        let mut state = state_for_tests();
        let mut scheduler = IdleScheduler::new();
        let mut dispatcher = Dispatcher::default();
        let at = later(3_600);
        scheduler
            .process(
                &mut state,
                IdleSchedulerAction::SetDeadline { id: id(2), at },
                &mut dispatcher,
            )
            .unwrap();
        scheduler
            .process(
                &mut state,
                IdleSchedulerAction::SetDeadline { id: id(1), at },
                &mut dispatcher,
            )
            .unwrap();
        scheduler
            .process(&mut state, IdleSchedulerAction::Evaluate { now: at }, &mut dispatcher)
            .unwrap();
        assert_eq!(scheduler.fired(), &[id(1), id(2)]);
        assert!(scheduler.armed_deadline().is_none());
        assert!(!scheduler.timer_is_armed());
        scheduler
            .process(&mut state, IdleSchedulerAction::Evaluate { now: at }, &mut dispatcher)
            .unwrap();
        assert_eq!(scheduler.fired(), &[id(1), id(2)]);
    }

    #[test]
    fn an_already_due_deadline_is_reported_without_arming() {
        let mut state = state_for_tests();
        let mut scheduler = IdleScheduler::new();
        let mut dispatcher = Dispatcher::default();
        let due = Instant::now() - Duration::from_secs(1);
        scheduler
            .process(
                &mut state,
                IdleSchedulerAction::SetDeadline { id: id(4), at: due },
                &mut dispatcher,
            )
            .unwrap();
        assert_eq!(scheduler.fired(), &[id(4)]);
        assert!(scheduler.armed_deadline().is_none());
        assert!(!scheduler.timer_is_armed());
    }

    #[test]
    fn clearing_the_sooner_deadline_rearms_the_later_one() {
        let mut state = state_for_tests();
        let mut scheduler = IdleScheduler::new();
        let mut dispatcher = Dispatcher::default();
        let sooner = later(3_600);
        let later_deadline = later(7_200);
        scheduler
            .process(
                &mut state,
                IdleSchedulerAction::SetDeadline { id: id(1), at: sooner },
                &mut dispatcher,
            )
            .unwrap();
        scheduler
            .process(
                &mut state,
                IdleSchedulerAction::SetDeadline { id: id(2), at: later_deadline },
                &mut dispatcher,
            )
            .unwrap();
        scheduler
            .process(&mut state, IdleSchedulerAction::ClearDeadline { id: id(1) }, &mut dispatcher)
            .unwrap();
        assert_eq!(scheduler.armed_deadline(), Some(later_deadline));
        assert!(scheduler.timer_is_armed());
        assert!(scheduler.fired().is_empty());
    }

    #[test]
    fn a_fired_timer_leaves_a_future_deadline_armed() {
        let mut state = state_for_tests();
        let mut scheduler = IdleScheduler::new();
        let mut dispatcher = Dispatcher::default();
        let deadline = later(3_600);
        scheduler
            .process(
                &mut state,
                IdleSchedulerAction::SetDeadline { id: id(1), at: deadline },
                &mut dispatcher,
            )
            .unwrap();
        scheduler
            .process(&mut state, IdleSchedulerAction::Fired, &mut dispatcher)
            .unwrap();
        assert!(scheduler.fired().is_empty());
        assert_eq!(scheduler.deadline(id(1)), Some(deadline));
        assert_eq!(scheduler.armed_deadline(), Some(deadline));
        assert!(scheduler.timer_is_armed());
    }

    #[test]
    fn session_pause_does_not_disarm_the_idle_timer() {
        let mut state = state_for_tests();
        let mut scheduler = IdleScheduler::new();
        let mut dispatcher = Dispatcher::default();
        let deadline = later(3_600);
        scheduler
            .process(
                &mut state,
                IdleSchedulerAction::SetDeadline { id: id(1), at: deadline },
                &mut dispatcher,
            )
            .unwrap();
        state.session_active = false;
        assert_eq!(scheduler.armed_deadline(), Some(deadline));
        assert!(scheduler.timer_is_armed());
        scheduler
            .process(&mut state, IdleSchedulerAction::Evaluate { now: deadline }, &mut dispatcher)
            .unwrap();
        assert_eq!(scheduler.fired(), &[id(1)]);
        assert!(!state.session_active);
    }

    #[test]
    fn stop_cancels_the_pending_timer_read() {
        let mut state = state_for_tests();
        let mut scheduler = IdleScheduler::new();
        let mut dispatcher = Dispatcher::default();
        scheduler
            .process(&mut state, IdleSchedulerAction::Start, &mut dispatcher)
            .unwrap();
        scheduler
            .process(&mut state, IdleSchedulerAction::Stop, &mut dispatcher)
            .unwrap();
        assert!(scheduler.armed_deadline().is_none());
        assert!(!scheduler.timer_is_armed());
        assert!(dispatcher.test_has_queued_action(|action: &IoUringAction| {
            matches!(action, IoUringAction::Cancel { .. })
        }));
    }

    #[test]
    fn ext_idle_notifier_is_advertised_without_idle_inhibit() {
        let state = state_for_tests();
        assert!(state.globals.contains_interface("ext_idle_notifier_v1"));
        assert!(!state
            .globals
            .contains_interface("zwp_idle_inhibit_manager_v1"));
    }
}
