use std::{
    os::fd::{AsFd, AsRawFd},
    time::{Duration, Instant},
};

use nix::sys::timerfd::{self, Expiration, TimerFd, TimerFlags, TimerSetTimeFlags};

use crate::{
    drm::DrmAction,
    io_uring::{model::IoUringAction, ring::RingId},
    runner::{Dispatcher, Model, ModelInstanceId},
    state::{DrmId, OutputId, State},
};

use super::{
    frame_clock::{
        ClockDisposition, FrameClock, FrameClockMode, FrameClockStateSnapshot, RequestFrameReason,
        TimingHints,
    },
    FrameCompletionOutcome,
};

#[derive(Clone, Debug)]
pub struct OutputSchedulerSnapshot {
    pub clock: FrameClockStateSnapshot,
    pub blocked_on_pipeline: bool,
    pub flip_watchdog_deadline: Option<Instant>,
    pub render_dispatched: bool,
    pub session_active: bool,
    pub presented_frames: u64,
    pub discarded_frames: u64,
    pub watchdog_timeouts: u64,
    pub last_predicted_present: Option<Instant>,
    pub last_actual_present: Option<Instant>,
    pub last_present_error_ns: Option<i128>,
    pub mode_transition_count: u64,
    pub vrr_transition_count: u64,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, derive_more::From)]
pub struct OutputSchedulerId(u64);

impl ModelInstanceId for OutputSchedulerId {
    type Model = OutputScheduler;

    fn into_raw(self) -> u64 {
        self.0
    }
}

#[derive(Debug)]
pub struct OutputScheduler {
    drm_id: DrmId,
    output_id: OutputId,
    clock: FrameClock,
    timer_fd: TimerFd,
    read_op_id: Option<RingId>,
    render_dispatched: bool,
    blocked_on_pipeline: bool,
    flip_watchdog_deadline: Option<Instant>,
    session_active: bool,
    presented_frames: u64,
    discarded_frames: u64,
    watchdog_timeouts: u64,
    last_predicted_present: Option<Instant>,
    last_actual_present: Option<Instant>,
    last_present_error_ns: Option<i128>,
    mode_transition_count: u64,
    vrr_transition_count: u64,
}

#[derive(Debug)]
pub enum OutputSchedulerAction {
    Start,
    RequestFrame {
        reason: RequestFrameReason,
    },
    Arm,
    RenderAttempted {
        started: bool,
    },
    ScheduleAnimation {
        delay_msec: Option<u32>,
    },
    PipelineBlocked {
        watchdog_deadline: Instant,
    },
    FrameCompleted(FrameCompletionOutcome),
    ModeUpdated,
    SessionChanged {
        active: bool,
    },
    /// Cancel pending frame and watchdog work while a device pause is in progress.
    Disarm,
    Fired,
    Failed {
        error: std::io::Error,
    },
    Stop,
}

pub fn output_scheduler_id(output_id: OutputId) -> OutputSchedulerId {
    OutputSchedulerId::from(output_id.as_u64())
}

impl OutputScheduler {
    pub fn new(drm_id: DrmId, output_id: OutputId) -> Self {
        let timer_fd = TimerFd::new(
            timerfd::ClockId::CLOCK_MONOTONIC,
            TimerFlags::TFD_CLOEXEC | TimerFlags::TFD_NONBLOCK,
        )
        .expect("failed to create output scheduler timerfd");
        Self {
            drm_id,
            output_id,
            clock: FrameClock::new(),
            timer_fd,
            read_op_id: None,
            render_dispatched: false,
            blocked_on_pipeline: false,
            flip_watchdog_deadline: None,
            session_active: true,
            presented_frames: 0,
            discarded_frames: 0,
            watchdog_timeouts: 0,
            last_predicted_present: None,
            last_actual_present: None,
            last_present_error_ns: None,
            mode_transition_count: 0,
            vrr_transition_count: 0,
        }
    }

    fn snapshot(&self) -> OutputSchedulerSnapshot {
        OutputSchedulerSnapshot {
            clock: self.clock.snapshot(),
            blocked_on_pipeline: self.blocked_on_pipeline,
            flip_watchdog_deadline: self.flip_watchdog_deadline,
            render_dispatched: self.render_dispatched,
            session_active: self.session_active,
            presented_frames: self.presented_frames,
            discarded_frames: self.discarded_frames,
            watchdog_timeouts: self.watchdog_timeouts,
            last_predicted_present: self.last_predicted_present,
            last_actual_present: self.last_actual_present,
            last_present_error_ns: self.last_present_error_ns,
            mode_transition_count: self.mode_transition_count,
            vrr_transition_count: self.vrr_transition_count,
        }
    }

    fn persist_snapshot(&self, state: &mut State) {
        state
            .frame_scheduler
            .insert(self.output_id, self.snapshot());
    }

    fn sync_mode_from_state(&mut self, state: &State) {
        let previous_mode = self.clock.mode();
        let previous_refresh_is_constant = self.clock.refresh_is_constant();
        if !self.session_active || !state.session_active {
            self.clock.update_mode(FrameClockMode::Passive, 60_000);
            if previous_mode != FrameClockMode::Passive {
                self.mode_transition_count = self.mode_transition_count.saturating_add(1);
                log::debug!(
                    "output scheduler {:?}: mode transition {:?} -> Passive",
                    self.output_id,
                    previous_mode
                );
            }
            return;
        }
        let (nominal_refresh_millihz, vrr_capable, vrr_policy_enabled, vrr_range) = state
            .outputs
            .try_get(self.output_id)
            .map(|output| {
                (
                    output.current_mode.refresh_millihz.max(0) as u32,
                    output.vrr_capable,
                    output.vrr_policy_enabled,
                    output.vrr_range,
                )
            })
            .unwrap_or((60_000, false, false, None));
        let mode = if nominal_refresh_millihz == 0 {
            FrameClockMode::Passive
        } else if vrr_capable && vrr_policy_enabled && vrr_range.is_some() {
            FrameClockMode::Variable
        } else {
            FrameClockMode::Fixed
        };
        self.clock.update_mode(mode, nominal_refresh_millihz);
        self.clock
            .update_vrr_state(vrr_range, vrr_capable && vrr_policy_enabled && vrr_range.is_some());
        if previous_mode != mode || previous_refresh_is_constant != self.clock.refresh_is_constant()
        {
            if previous_mode != mode {
                self.mode_transition_count = self.mode_transition_count.saturating_add(1);
            }
            if previous_refresh_is_constant != self.clock.refresh_is_constant() {
                self.vrr_transition_count = self.vrr_transition_count.saturating_add(1);
            }
            log::debug!(
                "output scheduler {:?}: mode transition {:?} -> {:?}, refresh_constant={} nominal_refresh_millihz={}",
                self.output_id,
                previous_mode,
                mode,
                self.clock.refresh_is_constant(),
                nominal_refresh_millihz
            );
        }
    }

    fn request_frame(
        &mut self,
        state: &State,
        dispatcher: &mut Dispatcher,
        reason: RequestFrameReason,
    ) {
        self.sync_mode_from_state(state);
        self.clock.request_frame(reason);
        self.try_dispatch_render(dispatcher);
        self.arm_from_clock(dispatcher);
    }

    fn schedule_animation(
        &mut self,
        state: &State,
        dispatcher: &mut Dispatcher,
        delay_msec: Option<u32>,
    ) {
        let Some(delay_msec) = delay_msec else {
            return;
        };
        self.sync_mode_from_state(state);
        let delay = Duration::from_millis(u64::from(delay_msec));
        let target = Instant::now() + delay;
        self.clock.update_timing_hints(TimingHints {
            min_next_present: Some(target),
            max_next_present: Some(target),
            cadence_hint: Some(delay),
            ..TimingHints::default()
        });
        self.clock.request_frame(RequestFrameReason::Animation);
        self.try_dispatch_render(dispatcher);
        self.arm_from_clock(dispatcher);
    }

    fn arm_from_clock(&mut self, dispatcher: &mut Dispatcher) {
        if self.render_dispatched {
            let _ = self.set_expiration(None);
            return;
        }
        let expiration = if self.blocked_on_pipeline {
            match self.flip_watchdog_deadline {
                Some(deadline) => {
                    let now = Instant::now();
                    if deadline <= now {
                        Expiration::OneShot(Duration::ZERO.into())
                    } else {
                        Expiration::OneShot(deadline.duration_since(now).into())
                    }
                }
                None => {
                    let _ = self.set_expiration(None);
                    return;
                }
            }
        } else {
            match self.clock.next_deadline() {
                Some(deadline) => {
                    let now = Instant::now();
                    if deadline <= now {
                        Expiration::OneShot(Duration::ZERO.into())
                    } else {
                        Expiration::OneShot(deadline.duration_since(now).into())
                    }
                }
                None => Expiration::OneShot(Duration::ZERO.into()),
            }
        };
        if let Err(error) = self.timer_fd.set(expiration, TimerSetTimeFlags::empty()) {
            log::warn!("failed to arm output scheduler timer: {:?}", error);
        }
        self.dispatch_read(dispatcher);
    }

    fn disarm_clock(&mut self) {
        self.session_active = false;
        self.blocked_on_pipeline = false;
        self.flip_watchdog_deadline = None;
        self.clock.update_mode(FrameClockMode::Passive, 60_000);
        let _ = self.set_expiration(None);
    }

    fn try_dispatch_render(&mut self, dispatcher: &mut Dispatcher) {
        if self.render_dispatched || self.blocked_on_pipeline || !self.clock.pending_work() {
            return;
        }
        let decision = self.clock.decide(Instant::now());
        match decision.disposition {
            ClockDisposition::RenderNow | ClockDisposition::MissedTarget => {
                if crate::logger::ENABLE_NOISY_LOG {
                    log::debug!(
                        "output scheduler {:?}: dispatch render mode={:?} target_present={:?} latest_render_start={:?} disposition={:?}",
                        self.output_id,
                        decision.mode,
                        decision.target_present,
                        decision.latest_render_start,
                        decision.disposition
                    );
                }
                self.render_dispatched = true;
                self.last_predicted_present = decision.target_present;
                dispatcher.dispatch_to(self.output_id, DrmAction::RenderFrame { decision });
            }
            ClockDisposition::WaitUntil(_) | ClockDisposition::Skip => {}
        }
    }

    fn on_render_attempted(&mut self, dispatcher: &mut Dispatcher, started: bool) {
        self.render_dispatched = false;
        if started {
            self.blocked_on_pipeline = false;
            self.flip_watchdog_deadline = None;
            self.clock.consume_pending_work();
        } else {
            self.blocked_on_pipeline = true;
        }
        self.arm_from_clock(dispatcher);
    }

    fn on_pipeline_blocked(&mut self, dispatcher: &mut Dispatcher, watchdog_deadline: Instant) {
        self.render_dispatched = false;
        self.blocked_on_pipeline = true;
        self.flip_watchdog_deadline = Some(watchdog_deadline);
        self.arm_from_clock(dispatcher);
    }

    fn on_frame_completed(
        &mut self,
        state: &State,
        dispatcher: &mut Dispatcher,
        outcome: FrameCompletionOutcome,
    ) {
        self.sync_mode_from_state(state);
        self.blocked_on_pipeline = false;
        self.flip_watchdog_deadline = None;
        match outcome {
            FrameCompletionOutcome::Presented(sample) => {
                self.presented_frames = self.presented_frames.saturating_add(1);
                self.last_actual_present = Some(sample.present_time);
                self.last_present_error_ns = self.last_predicted_present.map(|predicted| {
                    if sample.present_time >= predicted {
                        sample.present_time.duration_since(predicted).as_nanos() as i128
                    } else {
                        -(predicted.duration_since(sample.present_time).as_nanos() as i128)
                    }
                });
                if crate::logger::ENABLE_NOISY_LOG {
                    log::debug!(
                        "output scheduler {:?}: presented sequence={} refresh_constant={} total_presented={} present_error_ns={:?} render_budget_us={} submit_budget_us={}",
                        self.output_id,
                        sample.sequence,
                        sample.flags.refresh_is_constant,
                        self.presented_frames,
                        self.last_present_error_ns,
                        self.clock.snapshot().render_budget.as_micros(),
                        self.clock.snapshot().submit_budget.as_micros(),
                    );
                }
                self.clock.notify_presented(sample);
            }
            FrameCompletionOutcome::MissedTarget => {
                self.clock.note_missed_frame();
            }
            FrameCompletionOutcome::Discarded => {
                self.discarded_frames = self.discarded_frames.saturating_add(1);
                self.clock
                    .request_frame(RequestFrameReason::RetryAfterPresent);
            }
            FrameCompletionOutcome::NoPresent => {}
        }
        dispatcher.dispatch(crate::drm::VisualAnimationPumpAction::KickOutput {
            output_id: self.output_id,
        });
        self.try_dispatch_render(dispatcher);
        self.arm_from_clock(dispatcher);
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
            dispatch_to: crate::runner::Redispatcher::to_instance(
                output_scheduler_id(self.output_id),
                |result| match result {
                    Ok(_) => OutputSchedulerAction::Fired,
                    Err(error) => OutputSchedulerAction::Failed { error },
                },
            ),
        });
    }

    fn set_expiration(&mut self, expiration: Option<Expiration>) -> std::io::Result<()> {
        self.timer_fd
            .set(
                expiration.unwrap_or_else(|| Expiration::OneShot(Duration::ZERO.into())),
                TimerSetTimeFlags::empty(),
            )
            .map_err(std::io::Error::other)
    }
}

impl Model for OutputScheduler {
    type Action = OutputSchedulerAction;

    const START: Option<Self::Action> = Some(OutputSchedulerAction::Start);
    const STOP: Option<Self::Action> = Some(OutputSchedulerAction::Stop);

    fn process(
        &mut self,
        state: &mut State,
        action: Self::Action,
        dispatcher: &mut Dispatcher,
    ) -> std::result::Result<(), crate::runner::ModelFailure> {
        match action {
            OutputSchedulerAction::Start => {
                self.sync_mode_from_state(state);
                self.dispatch_read(dispatcher);
            }
            OutputSchedulerAction::RequestFrame { reason } => {
                self.request_frame(state, dispatcher, reason);
            }
            OutputSchedulerAction::Arm | OutputSchedulerAction::ModeUpdated => {
                self.sync_mode_from_state(state);
                self.try_dispatch_render(dispatcher);
                self.arm_from_clock(dispatcher);
            }
            OutputSchedulerAction::RenderAttempted { started } => {
                self.on_render_attempted(dispatcher, started);
            }
            OutputSchedulerAction::ScheduleAnimation { delay_msec } => {
                self.schedule_animation(state, dispatcher, delay_msec);
            }
            OutputSchedulerAction::PipelineBlocked { watchdog_deadline } => {
                self.on_pipeline_blocked(dispatcher, watchdog_deadline);
            }
            OutputSchedulerAction::FrameCompleted(outcome) => {
                self.on_frame_completed(state, dispatcher, outcome);
            }
            OutputSchedulerAction::SessionChanged { active } => {
                self.session_active = active;
                dispatcher.dispatch_to(self.output_id, DrmAction::SessionChanged { active });
                if active {
                    self.sync_mode_from_state(state);
                    self.blocked_on_pipeline = false;
                    self.flip_watchdog_deadline = None;
                    self.clock.request_frame(RequestFrameReason::ModeChange);
                    self.try_dispatch_render(dispatcher);
                    self.arm_from_clock(dispatcher);
                } else {
                    self.disarm_clock();
                }
            }
            OutputSchedulerAction::Disarm => self.disarm_clock(),
            OutputSchedulerAction::Fired => {
                self.read_op_id = None;
                if self.blocked_on_pipeline
                    && self
                        .flip_watchdog_deadline
                        .is_some_and(|deadline| Instant::now() >= deadline)
                {
                    self.watchdog_timeouts = self.watchdog_timeouts.saturating_add(1);
                    log::warn!(
                        "output scheduler {:?}: flip watchdog timeout count={}",
                        self.output_id,
                        self.watchdog_timeouts
                    );
                    dispatcher.dispatch_to(self.output_id, DrmAction::FlipWatchdog);
                } else {
                    self.try_dispatch_render(dispatcher);
                    self.arm_from_clock(dispatcher);
                }
            }
            OutputSchedulerAction::Failed { error } => {
                self.read_op_id = None;
                log::warn!("output scheduler timer read failed: {:?}", error);
                self.dispatch_read(dispatcher);
            }
            OutputSchedulerAction::Stop => {
                let _ = self.set_expiration(None);
                if let Some(read_op_id) = self.read_op_id.take() {
                    dispatcher.dispatch(IoUringAction::Cancel { target: read_op_id });
                }
                state.frame_scheduler.remove(&self.output_id);
                return Ok(());
            }
        }
        self.persist_snapshot(state);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use {
        super::*,
        crate::{
            drm::DrmAction,
            runner::Dispatcher,
            state::{BackendOutputRef, DrmId},
            test_support::{state_for_tests, test_output},
        },
    };

    #[test]
    fn vrr_capable_output_dispatches_variable_mode_render() {
        let mut state = state_for_tests();
        let output_id = state.insert_output(test_output(
            BackendOutputRef::new(DrmId::from(0usize)),
            1,
            0,
            0,
            1920,
            1080,
            "test-output",
        ));
        {
            let output = state.outputs.try_get_mut(output_id).unwrap();
            output.vrr_capable = true;
            output.vrr_policy_enabled = true;
            output.vrr_enabled = true;
            output.vrr_range = Some(crate::scheduler::frame_clock::VrrRange {
                min_refresh_millihz: 48_000,
                max_refresh_millihz: 60_000,
            });
        }

        let mut scheduler = OutputScheduler::new(DrmId::from(0usize), output_id);
        let mut dispatcher = Dispatcher::default();
        scheduler
            .process(
                &mut state,
                OutputSchedulerAction::RequestFrame { reason: RequestFrameReason::Input },
                &mut dispatcher,
            )
            .unwrap();

        assert!(dispatcher.test_has_queued_action_for::<DrmAction, _>(output_id, |action| {
            matches!(
                action,
                DrmAction::RenderFrame { decision }
                    if decision.mode == FrameClockMode::Variable
            )
        }));
        let snapshot = state.frame_scheduler.get(&output_id).expect("snapshot");
        assert_eq!(
            snapshot.clock.vrr,
            Some(crate::scheduler::frame_clock::VrrRange {
                min_refresh_millihz: 48_000,
                max_refresh_millihz: 60_000,
            })
        );
    }

    #[test]
    fn non_vrr_output_dispatches_fixed_mode_render() {
        let mut state = state_for_tests();
        let output_id = state.insert_output(test_output(
            BackendOutputRef::new(DrmId::from(0usize)),
            1,
            0,
            0,
            1920,
            1080,
            "test-output",
        ));

        let mut scheduler = OutputScheduler::new(DrmId::from(0usize), output_id);
        let mut dispatcher = Dispatcher::default();
        scheduler
            .process(
                &mut state,
                OutputSchedulerAction::RequestFrame { reason: RequestFrameReason::Input },
                &mut dispatcher,
            )
            .unwrap();

        assert!(dispatcher.test_has_queued_action_for::<DrmAction, _>(output_id, |action| {
            matches!(
                action,
                DrmAction::RenderFrame { decision }
                    if decision.mode == FrameClockMode::Fixed
            )
        }));
    }

    #[test]
    fn blocked_pipeline_arms_flip_watchdog() {
        let mut state = state_for_tests();
        let output_id = state.insert_output(test_output(
            BackendOutputRef::new(DrmId::from(0usize)),
            1,
            0,
            0,
            1920,
            1080,
            "test-output",
        ));

        let mut scheduler = OutputScheduler::new(DrmId::from(0usize), output_id);
        let mut dispatcher = Dispatcher::default();
        scheduler
            .process(
                &mut state,
                OutputSchedulerAction::PipelineBlocked {
                    watchdog_deadline: Instant::now() - Duration::from_millis(1),
                },
                &mut dispatcher,
            )
            .unwrap();
        scheduler
            .process(&mut state, OutputSchedulerAction::Fired, &mut dispatcher)
            .unwrap();

        assert!(dispatcher.test_has_queued_action_for::<DrmAction, _>(output_id, |action| {
            matches!(action, DrmAction::FlipWatchdog)
        }));
    }

    #[test]
    fn request_while_pipeline_blocked_keeps_work_queued_without_redispatch() {
        let mut state = state_for_tests();
        let output_id = state.insert_output(test_output(
            BackendOutputRef::new(DrmId::from(0usize)),
            1,
            0,
            0,
            1920,
            1080,
            "test-output",
        ));

        let mut scheduler = OutputScheduler::new(DrmId::from(0usize), output_id);
        let mut dispatcher = Dispatcher::default();
        let watchdog_deadline = Instant::now() + Duration::from_millis(10);
        scheduler
            .process(
                &mut state,
                OutputSchedulerAction::PipelineBlocked { watchdog_deadline },
                &mut dispatcher,
            )
            .unwrap();
        scheduler
            .process(
                &mut state,
                OutputSchedulerAction::RequestFrame { reason: RequestFrameReason::Damage },
                &mut dispatcher,
            )
            .unwrap();

        assert!(!dispatcher.test_has_queued_action_for::<DrmAction, _>(output_id, |action| {
            matches!(action, DrmAction::RenderFrame { .. })
        }));
        let snapshot = state.frame_scheduler.get(&output_id).expect("snapshot");
        assert!(snapshot.blocked_on_pipeline);
        assert!(snapshot.clock.pending_work);
        assert_eq!(snapshot.flip_watchdog_deadline, Some(watchdog_deadline));
    }

    #[test]
    fn scheduler_persists_debug_snapshot_into_state() {
        let mut state = state_for_tests();
        let output_id = state.insert_output(test_output(
            BackendOutputRef::new(DrmId::from(0usize)),
            1,
            0,
            0,
            1920,
            1080,
            "test-output",
        ));
        let mut scheduler = OutputScheduler::new(DrmId::from(0usize), output_id);
        let mut dispatcher = Dispatcher::default();

        scheduler
            .process(
                &mut state,
                OutputSchedulerAction::RequestFrame { reason: RequestFrameReason::Damage },
                &mut dispatcher,
            )
            .unwrap();

        let snapshot = state.frame_scheduler.get(&output_id).expect("snapshot");
        assert!(snapshot.clock.pending_work);
        assert_eq!(snapshot.clock.mode, FrameClockMode::Fixed);
    }

    #[test]
    fn disarm_drops_pending_schedule_work() {
        let mut state = state_for_tests();
        let output_id = state.insert_output(test_output(
            BackendOutputRef::new(DrmId::from(0usize)),
            1,
            0,
            0,
            1920,
            1080,
            "test-output",
        ));
        let mut scheduler = OutputScheduler::new(DrmId::from(0usize), output_id);
        let mut dispatcher = Dispatcher::default();
        scheduler
            .process(
                &mut state,
                OutputSchedulerAction::RequestFrame { reason: RequestFrameReason::Damage },
                &mut dispatcher,
            )
            .unwrap();
        scheduler.blocked_on_pipeline = true;
        scheduler.flip_watchdog_deadline = Some(std::time::Instant::now());
        scheduler
            .process(&mut state, OutputSchedulerAction::Disarm, &mut dispatcher)
            .unwrap();
        assert!(!scheduler.session_active);
        assert!(!scheduler.blocked_on_pipeline);
        assert!(scheduler.flip_watchdog_deadline.is_none());
        assert_eq!(scheduler.clock.mode(), FrameClockMode::Passive);
    }
}
