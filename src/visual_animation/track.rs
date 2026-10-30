use std::time::{Duration, Instant};

/// Normalized value in `[0, 1]`, stored as Q16 fixed point.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct UnitValue(u32);

impl UnitValue {
    pub const ZERO: Self = Self(0);
    pub const ONE: Self = Self(1 << 16);

    pub fn saturating_from_ratio(numerator: u128, denominator: u128) -> Self {
        if denominator == 0 || numerator >= denominator {
            return Self::ONE;
        }
        let scaled = numerator.saturating_mul(u128::from(Self::ONE.0)) / denominator;
        Self(
            u32::try_from(scaled)
                .unwrap_or(Self::ONE.0)
                .min(Self::ONE.0),
        )
    }

    pub fn saturating_sub(self, other: Self) -> Self {
        Self(self.0.saturating_sub(other.0))
    }

    pub fn lerp(self, from: Self, to: Self) -> Self {
        if self.0 == 0 {
            return from;
        }
        if self.0 >= Self::ONE.0 {
            return to;
        }
        let from = i64::from(from.0);
        let delta = i64::from(to.0) - from;
        let applied = delta * i64::from(self.0) / i64::from(Self::ONE.0);
        Self((from + applied).clamp(0, i64::from(Self::ONE.0)) as u32)
    }

    pub fn lerp_u8(self, from: u8, to: u8) -> u8 {
        if self.0 == 0 {
            return from;
        }
        if self.0 >= Self::ONE.0 {
            return to;
        }
        let from = i32::from(from);
        let delta = i32::from(to) - from;
        let applied = delta * self.0 as i32 / Self::ONE.0 as i32;
        (from + applied).clamp(0, i32::from(u8::MAX)) as u8
    }
}

/// Absolute progress of a timed segment. Time before the start stays at zero.
pub fn unit_progress_between(started_at: Instant, duration: Duration, now: Instant) -> UnitValue {
    if duration.is_zero() || now >= started_at + duration {
        return UnitValue::ONE;
    }
    let Some(elapsed) = now.checked_duration_since(started_at) else {
        return UnitValue::ZERO;
    };
    UnitValue::saturating_from_ratio(elapsed.as_nanos(), duration.as_nanos())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Easing {
    Linear,
}

pub fn apply_easing(easing: Easing, progress: UnitValue) -> UnitValue {
    match easing {
        Easing::Linear => progress,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TrackMode {
    Timed,
    Driven,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TrackStatus {
    Active,
    Settled,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TrackInterrupt {
    Retargeted,
    Cancelled,
    Snapped,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TrackSample {
    pub value: UnitValue,
    pub progress: UnitValue,
    pub status: TrackStatus,
}

#[derive(Clone, Copy, Debug)]
struct SettledTrack {
    value: UnitValue,
    progress: UnitValue,
}

#[derive(Clone, Debug)]
pub struct RetargetableTrack {
    mode: TrackMode,
    from: UnitValue,
    goal: UnitValue,
    started_at: Instant,
    duration: Duration,
    easing: Easing,
    driven_progress: UnitValue,
    settled: Option<SettledTrack>,
    last_interrupt: Option<TrackInterrupt>,
}

impl RetargetableTrack {
    pub fn settled(value: UnitValue) -> Self {
        Self {
            mode: TrackMode::Timed,
            from: value,
            goal: value,
            started_at: Instant::now(),
            duration: Duration::ZERO,
            easing: Easing::Linear,
            driven_progress: UnitValue::ONE,
            settled: Some(SettledTrack { value, progress: UnitValue::ONE }),
            last_interrupt: None,
        }
    }

    pub fn start_timed(
        from: UnitValue,
        goal: UnitValue,
        now: Instant,
        duration: Duration,
        easing: Easing,
    ) -> Self {
        let mut track = Self {
            mode: TrackMode::Timed,
            from,
            goal,
            started_at: now,
            duration,
            easing,
            driven_progress: UnitValue::ZERO,
            settled: None,
            last_interrupt: None,
        };
        track.record_if_settled(now);
        track
    }

    pub fn start_driven(from: UnitValue, goal: UnitValue, progress: UnitValue) -> Self {
        let mut track = Self {
            mode: TrackMode::Driven,
            from,
            goal,
            started_at: Instant::now(),
            duration: Duration::ZERO,
            easing: Easing::Linear,
            driven_progress: clamp_unit(progress),
            settled: None,
            last_interrupt: None,
        };
        track.record_if_settled(track.started_at);
        track
    }

    pub fn mode(&self) -> TrackMode {
        self.mode
    }

    pub fn from(&self) -> UnitValue {
        self.from
    }

    pub fn goal(&self) -> UnitValue {
        self.goal
    }

    pub fn duration(&self) -> Duration {
        self.duration
    }

    pub fn easing(&self) -> Easing {
        self.easing
    }

    pub fn driven_progress(&self) -> UnitValue {
        self.driven_progress
    }

    pub fn last_interrupt(&self) -> Option<TrackInterrupt> {
        self.last_interrupt
    }

    pub fn is_active(&self, now: Instant) -> bool {
        self.sample(now).status == TrackStatus::Active
    }

    /// Pure sample. Rendering may call this without advancing completion state.
    pub fn sample(&self, now: Instant) -> TrackSample {
        if let Some(settled) = self.settled {
            return TrackSample {
                value: settled.value,
                progress: settled.progress,
                status: TrackStatus::Settled,
            };
        }
        self.live_sample(now)
    }

    /// Records completion when a running track has reached its end.
    ///
    /// This is the only time-based mutation. Call it from the animation pump, not from render
    /// resolution.
    pub fn sample_mut(&mut self, now: Instant) -> TrackSample {
        let sample = self.sample(now);
        if self.settled.is_none() && sample.status == TrackStatus::Settled {
            self.from = sample.value;
            self.goal = sample.value;
            self.settle(sample.value, sample.progress, None);
        }
        sample
    }

    pub fn needs_frames(&self, now: Instant) -> bool {
        self.is_active(now)
    }

    pub fn retarget_timed(
        &mut self,
        goal: UnitValue,
        now: Instant,
        duration: Duration,
        easing: Easing,
    ) {
        self.restart(TrackMode::Timed, goal, now, duration, easing, UnitValue::ZERO);
    }

    pub fn retarget_driven(&mut self, goal: UnitValue, now: Instant, progress: UnitValue) {
        self.restart(TrackMode::Driven, goal, now, Duration::ZERO, self.easing, progress);
    }

    pub fn switch_to_timed(
        &mut self,
        goal: UnitValue,
        now: Instant,
        duration: Duration,
        easing: Easing,
    ) {
        self.retarget_timed(goal, now, duration, easing);
    }

    pub fn switch_to_driven(&mut self, goal: UnitValue, now: Instant, progress: UnitValue) {
        self.retarget_driven(goal, now, progress);
    }

    pub fn set_driven_progress(&mut self, progress: UnitValue) {
        if self.mode != TrackMode::Driven {
            return;
        }
        self.driven_progress = clamp_unit(progress);
        self.settled = None;
        self.record_if_settled(self.started_at);
    }

    pub fn cancel_at(&mut self, now: Instant) {
        let sample = self.sample(now);
        self.settle(sample.value, sample.progress, Some(TrackInterrupt::Cancelled));
    }

    pub fn snap_to(&mut self, value: UnitValue) {
        self.from = value;
        self.goal = value;
        self.settle(value, UnitValue::ONE, Some(TrackInterrupt::Snapped));
    }

    pub fn finish(&mut self) {
        let goal = self.goal;
        self.from = goal;
        self.settle(goal, UnitValue::ONE, None);
    }

    fn restart(
        &mut self,
        mode: TrackMode,
        goal: UnitValue,
        now: Instant,
        duration: Duration,
        easing: Easing,
        driven_progress: UnitValue,
    ) {
        let current = self.sample(now);
        let was_active = current.status == TrackStatus::Active;
        self.mode = mode;
        self.from = current.value;
        self.goal = goal;
        self.started_at = now;
        self.duration = duration;
        self.easing = easing;
        self.driven_progress = clamp_unit(driven_progress);
        self.settled = None;
        self.last_interrupt = was_active.then_some(TrackInterrupt::Retargeted);
        self.record_if_settled(now);
    }

    fn live_sample(&self, now: Instant) -> TrackSample {
        let raw_progress = match self.mode {
            TrackMode::Timed => {
                if self.duration.is_zero() || self.from == self.goal {
                    UnitValue::ONE
                } else {
                    unit_progress_between(self.started_at, self.duration, now)
                }
            }
            TrackMode::Driven => {
                if self.from == self.goal {
                    UnitValue::ONE
                } else {
                    self.driven_progress
                }
            }
        };
        let progress = apply_easing(self.easing, raw_progress);
        TrackSample {
            value: progress.lerp(self.from, self.goal),
            progress,
            status: if raw_progress == UnitValue::ONE {
                TrackStatus::Settled
            } else {
                TrackStatus::Active
            },
        }
    }

    fn record_if_settled(&mut self, now: Instant) {
        let sample = self.live_sample(now);
        if sample.status == TrackStatus::Settled {
            self.settle(sample.value, sample.progress, self.last_interrupt);
        }
    }

    fn settle(&mut self, value: UnitValue, progress: UnitValue, interrupt: Option<TrackInterrupt>) {
        self.settled = Some(SettledTrack { value, progress });
        if let Some(interrupt) = interrupt {
            self.last_interrupt = Some(interrupt);
        }
    }
}

fn clamp_unit(value: UnitValue) -> UnitValue {
    if value.0 > UnitValue::ONE.0 {
        UnitValue::ONE
    } else {
        value
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::{
        apply_easing, Easing, RetargetableTrack, TrackInterrupt, TrackMode, TrackStatus, UnitValue,
    };

    fn half() -> UnitValue {
        UnitValue::saturating_from_ratio(1, 2)
    }

    #[test]
    fn linear_easing_preserves_progress() {
        assert_eq!(apply_easing(Easing::Linear, UnitValue::ZERO), UnitValue::ZERO);
        assert_eq!(apply_easing(Easing::Linear, half()), half());
        assert_eq!(apply_easing(Easing::Linear, UnitValue::ONE), UnitValue::ONE);
    }

    #[test]
    fn timed_track_samples_endpoints_and_midpoint() {
        let now = Instant::now();
        let duration = Duration::from_millis(100);
        let track = RetargetableTrack::start_timed(
            UnitValue::ZERO,
            UnitValue::ONE,
            now,
            duration,
            Easing::Linear,
        );

        let start = track.sample(now);
        assert_eq!(start.value, UnitValue::ZERO);
        assert_eq!(start.progress, UnitValue::ZERO);
        assert_eq!(start.status, TrackStatus::Active);

        let middle = track.sample(now + Duration::from_millis(50));
        assert_eq!(middle.value, half());
        assert_eq!(middle.progress, half());
        assert_eq!(middle.status, TrackStatus::Active);

        let end = track.sample(now + duration);
        assert_eq!(end.value, UnitValue::ONE);
        assert_eq!(end.status, TrackStatus::Settled);
        let after = track.sample(now + duration + Duration::from_secs(5));
        assert_eq!(after, end);
    }

    #[test]
    fn zero_duration_and_equal_endpoints_settle_immediately() {
        let now = Instant::now();
        let snapped = RetargetableTrack::start_timed(
            UnitValue::ZERO,
            UnitValue::ONE,
            now,
            Duration::ZERO,
            Easing::Linear,
        );
        assert_eq!(snapped.sample(now).value, UnitValue::ONE);
        assert!(!snapped.needs_frames(now));

        let idle = RetargetableTrack::start_timed(
            half(),
            half(),
            now,
            Duration::from_millis(80),
            Easing::Linear,
        );
        assert_eq!(idle.sample(now).status, TrackStatus::Settled);
        assert!(!idle.needs_frames(now + Duration::from_secs(1)));
    }

    #[test]
    fn sample_does_not_record_completion() {
        let now = Instant::now();
        let mut track = RetargetableTrack::start_timed(
            UnitValue::ZERO,
            UnitValue::ONE,
            now,
            Duration::from_millis(40),
            Easing::Linear,
        );
        let end = now + Duration::from_millis(40);
        assert_eq!(track.sample(end), track.sample(end));
        assert_eq!(track.from(), UnitValue::ZERO);
        assert!(track.needs_frames(now));
        assert!(!track.needs_frames(end));

        let recorded = track.sample_mut(end);
        assert_eq!(recorded.value, UnitValue::ONE);
        assert_eq!(recorded.status, TrackStatus::Settled);
        assert_eq!(track.from(), UnitValue::ONE);
        assert_eq!(track.sample(now).value, UnitValue::ONE);
    }

    #[test]
    fn retarget_restarts_from_the_sampled_value() {
        let now = Instant::now();
        let mut track = RetargetableTrack::start_timed(
            UnitValue::ZERO,
            UnitValue::ONE,
            now,
            Duration::from_millis(100),
            Easing::Linear,
        );
        let mid = now + Duration::from_millis(50);
        track.retarget_timed(UnitValue::ZERO, mid, Duration::from_millis(100), Easing::Linear);
        track.retarget_timed(UnitValue::ONE, mid, Duration::from_millis(80), Easing::Linear);

        assert_eq!(track.from(), half());
        assert_eq!(track.goal(), UnitValue::ONE);
        assert_eq!(track.duration(), Duration::from_millis(80));
        assert_eq!(track.sample(mid).value, half());
        assert_eq!(track.last_interrupt(), Some(TrackInterrupt::Retargeted));
        assert!(track.needs_frames(mid));
    }

    #[test]
    fn retarget_of_a_settled_track_is_a_fresh_start() {
        let now = Instant::now();
        let mut track = RetargetableTrack::settled(UnitValue::ZERO);
        track.retarget_timed(UnitValue::ONE, now, Duration::from_millis(30), Easing::Linear);

        assert_eq!(track.last_interrupt(), None);
        assert_eq!(track.sample(now).value, UnitValue::ZERO);
        assert_eq!(track.sample(now).status, TrackStatus::Active);
    }

    #[test]
    fn driven_progress_reaches_the_goal_and_can_switch_modes() {
        let now = Instant::now();
        let mut track =
            RetargetableTrack::start_driven(UnitValue::ZERO, UnitValue::ONE, UnitValue::ZERO);
        assert_eq!(track.mode(), TrackMode::Driven);
        assert_eq!(track.sample(now).value, UnitValue::ZERO);
        assert!(track.needs_frames(now));

        track.set_driven_progress(half());
        assert_eq!(track.sample(now).value, half());
        track.set_driven_progress(UnitValue::ONE);
        assert_eq!(track.sample(now).value, UnitValue::ONE);
        assert!(!track.needs_frames(now));

        track.set_driven_progress(half());
        track.switch_to_timed(UnitValue::ZERO, now, Duration::from_millis(10), Easing::Linear);
        assert_eq!(track.mode(), TrackMode::Timed);
        assert_eq!(track.from(), half());
        assert_eq!(track.goal(), UnitValue::ZERO);

        track.switch_to_driven(UnitValue::ONE, now, UnitValue::ZERO);
        assert_eq!(track.mode(), TrackMode::Driven);
        assert_eq!(track.from(), half());
        assert_eq!(track.driven_progress(), UnitValue::ZERO);
        assert_eq!(track.sample(now).value, half());
    }

    #[test]
    fn cancel_snap_and_finish_settle_without_further_frames() {
        let now = Instant::now();
        let mut cancelled = RetargetableTrack::start_timed(
            UnitValue::ZERO,
            UnitValue::ONE,
            now,
            Duration::from_millis(100),
            Easing::Linear,
        );
        let mid = now + Duration::from_millis(50);
        cancelled.cancel_at(mid);
        assert_eq!(cancelled.sample(mid).value, half());
        assert_eq!(cancelled.sample(now + Duration::from_secs(3)).value, half());
        assert_eq!(cancelled.last_interrupt(), Some(TrackInterrupt::Cancelled));
        assert!(!cancelled.needs_frames(now));

        let mut snapped = RetargetableTrack::start_timed(
            UnitValue::ZERO,
            UnitValue::ONE,
            now,
            Duration::from_millis(100),
            Easing::Linear,
        );
        snapped.snap_to(half());
        assert_eq!(snapped.sample(now).value, half());
        assert_eq!(snapped.last_interrupt(), Some(TrackInterrupt::Snapped));

        let mut finished = RetargetableTrack::start_driven(half(), UnitValue::ONE, half());
        finished.finish();
        assert_eq!(finished.sample(now).value, UnitValue::ONE);
        assert_eq!(finished.last_interrupt(), None);
        assert!(!finished.needs_frames(now));
    }
}
