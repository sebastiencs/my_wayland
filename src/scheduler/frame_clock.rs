use std::{
    cmp::max,
    collections::VecDeque,
    time::{Duration, Instant},
};

const DEFAULT_RENDER_BUDGET: Duration = Duration::from_millis(3);
const DEFAULT_SUBMIT_BUDGET: Duration = Duration::from_millis(1);
const DEFAULT_SAFETY_MARGIN: Duration = Duration::from_millis(1);
const PRESENT_HISTORY_LIMIT: usize = 8;
const SHORT_BUDGET_WINDOW: usize = 8;
const LONG_BUDGET_WINDOW: usize = 64;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FrameClockMode {
    Fixed,
    Variable,
    Passive,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RequestFrameReason {
    Damage,
    Animation,
    Input,
    SurfaceCommit,
    ModeChange,
    RetryAfterPresent,
    Explicit,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct VrrRange {
    pub min_refresh_millihz: u32,
    pub max_refresh_millihz: u32,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TimingHints {
    pub wants_low_latency: bool,
    pub min_next_present: Option<Instant>,
    pub max_next_present: Option<Instant>,
    pub cadence_hint: Option<Duration>,
    pub allow_immediate: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PresentFlags {
    pub zero_copy: bool,
    pub refresh_is_constant: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PresentPath {
    Composited,
    DirectScanout,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PresentTiming {
    pub path: PresentPath,
    pub scene_build_duration: Duration,
    pub render_submit_duration: Duration,
    pub submit_to_present: Option<Duration>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PresentSample {
    pub present_time: Instant,
    pub sequence: u64,
    pub flags: PresentFlags,
    pub timing: Option<PresentTiming>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ClockDecision {
    pub target_present: Option<Instant>,
    pub latest_render_start: Instant,
    pub latest_submit_or_commit: Instant,
    pub mode: FrameClockMode,
    pub disposition: ClockDisposition,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClockDisposition {
    RenderNow,
    WaitUntil(Instant),
    MissedTarget,
    Skip,
}

#[derive(Clone, Debug)]
pub struct FrameClockStateSnapshot {
    pub mode: FrameClockMode,
    pub nominal_refresh_millihz: u32,
    pub vrr: Option<VrrRange>,
    pub vrr_enabled: bool,
    pub pending_work: bool,
    pub target_present: Option<Instant>,
    pub latest_render_start: Option<Instant>,
    pub latest_submit_or_commit: Option<Instant>,
    pub last_present: Option<PresentSample>,
    pub render_budget: Duration,
    pub scene_build_budget: Duration,
    pub render_submit_budget: Duration,
    pub submit_budget: Duration,
    pub safety_margin: Duration,
    pub missed_frames: u64,
    pub last_deadline_miss_ns: Option<i128>,
    pub refresh_is_constant: bool,
}

#[derive(Clone, Debug)]
pub struct FrameClock {
    mode: FrameClockMode,
    nominal_refresh_millihz: u32,
    vrr: Option<VrrRange>,
    vrr_enabled: bool,
    last_present: Option<PresentSample>,
    present_intervals: VecDeque<Duration>,
    next_target_present: Option<Instant>,
    latest_render_start: Option<Instant>,
    latest_submit_or_commit: Option<Instant>,
    pending_work: bool,
    timing_hints: TimingHints,
    render_budget: Duration,
    scene_build_budget: Duration,
    render_submit_budget: Duration,
    submit_budget: Duration,
    safety_margin: Duration,
    missed_frames: u64,
    last_deadline_miss_ns: Option<i128>,
    composited_scene_build_history: RollingDurationHistory,
    direct_scanout_scene_build_history: RollingDurationHistory,
    composited_render_submit_history: RollingDurationHistory,
    direct_scanout_render_submit_history: RollingDurationHistory,
    composited_submit_history: RollingDurationHistory,
    direct_scanout_submit_history: RollingDurationHistory,
}

impl Default for FrameClock {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameClock {
    pub fn new() -> Self {
        Self {
            mode: FrameClockMode::Fixed,
            nominal_refresh_millihz: 60_000,
            vrr: None,
            vrr_enabled: false,
            last_present: None,
            present_intervals: VecDeque::with_capacity(PRESENT_HISTORY_LIMIT),
            next_target_present: None,
            latest_render_start: None,
            latest_submit_or_commit: None,
            pending_work: false,
            timing_hints: TimingHints::default(),
            render_budget: DEFAULT_RENDER_BUDGET,
            scene_build_budget: Duration::from_millis(1),
            render_submit_budget: Duration::from_millis(2),
            submit_budget: DEFAULT_SUBMIT_BUDGET,
            safety_margin: DEFAULT_SAFETY_MARGIN,
            missed_frames: 0,
            last_deadline_miss_ns: None,
            composited_scene_build_history: RollingDurationHistory::new(),
            direct_scanout_scene_build_history: RollingDurationHistory::new(),
            composited_render_submit_history: RollingDurationHistory::new(),
            direct_scanout_render_submit_history: RollingDurationHistory::new(),
            composited_submit_history: RollingDurationHistory::new(),
            direct_scanout_submit_history: RollingDurationHistory::new(),
        }
    }

    pub fn mode(&self) -> FrameClockMode {
        self.mode
    }

    pub fn pending_work(&self) -> bool {
        self.pending_work
    }

    pub fn request_frame(&mut self, reason: RequestFrameReason) {
        self.pending_work = true;
        self.merge_reason(reason);
        self.recompute_targets();
    }

    pub fn consume_pending_work(&mut self) {
        self.pending_work = false;
        self.timing_hints = TimingHints::default();
        self.recompute_targets();
    }

    pub fn update_mode(&mut self, mode: FrameClockMode, nominal_refresh_millihz: u32) {
        self.mode = mode;
        if nominal_refresh_millihz > 0 {
            self.nominal_refresh_millihz = nominal_refresh_millihz;
        }
        self.recompute_targets();
    }

    pub fn update_vrr_state(&mut self, vrr: Option<VrrRange>, enabled: bool) {
        self.vrr = vrr;
        self.vrr_enabled = enabled;
        self.recompute_targets();
    }

    pub fn update_timing_hints(&mut self, hints: TimingHints) {
        self.timing_hints = self.timing_hints.merge(hints);
        self.recompute_targets();
    }

    pub fn notify_presented(&mut self, sample: PresentSample) {
        if let Some(timing) = sample.timing {
            match timing.path {
                PresentPath::Composited => {
                    self.composited_scene_build_history
                        .push(timing.scene_build_duration);
                    self.composited_render_submit_history
                        .push(timing.render_submit_duration);
                    if let Some(submit_to_present) = timing.submit_to_present {
                        self.composited_submit_history.push(submit_to_present);
                    }
                }
                PresentPath::DirectScanout => {
                    self.direct_scanout_scene_build_history
                        .push(timing.scene_build_duration);
                    self.direct_scanout_render_submit_history
                        .push(timing.render_submit_duration);
                    if let Some(submit_to_present) = timing.submit_to_present {
                        self.direct_scanout_submit_history.push(submit_to_present);
                    }
                }
            }
            self.refresh_budgets_from_measurements();
        }
        if let Some(last_present) = self.last_present {
            if let Some(delta) = sample
                .present_time
                .checked_duration_since(last_present.present_time)
            {
                self.push_present_interval(delta);
            }
        }
        self.last_present = Some(sample);
        self.recompute_targets();
    }

    pub fn note_missed_frame(&mut self) {
        self.missed_frames = self.missed_frames.saturating_add(1);
        self.render_budget = max(self.render_budget, DEFAULT_RENDER_BUDGET + DEFAULT_SAFETY_MARGIN);
    }

    pub fn next_deadline(&self) -> Option<Instant> {
        if !self.pending_work {
            return None;
        }
        self.latest_render_start
    }

    pub fn refresh_is_constant(&self) -> bool {
        !matches!(self.mode, FrameClockMode::Variable) || !self.vrr_enabled
    }

    pub fn snapshot(&self) -> FrameClockStateSnapshot {
        FrameClockStateSnapshot {
            mode: self.mode,
            nominal_refresh_millihz: self.nominal_refresh_millihz,
            vrr: self.vrr,
            vrr_enabled: self.vrr_enabled,
            pending_work: self.pending_work,
            target_present: self.next_target_present,
            latest_render_start: self.latest_render_start,
            latest_submit_or_commit: self.latest_submit_or_commit,
            last_present: self.last_present,
            render_budget: self.render_budget,
            scene_build_budget: self.scene_build_budget,
            render_submit_budget: self.render_submit_budget,
            submit_budget: self.submit_budget,
            safety_margin: self.safety_margin,
            missed_frames: self.missed_frames,
            last_deadline_miss_ns: self.last_deadline_miss_ns,
            refresh_is_constant: self.refresh_is_constant(),
        }
    }

    pub fn decide(&mut self, now: Instant) -> ClockDecision {
        self.recompute_targets();
        let Some(latest_render_start) = self.latest_render_start else {
            return ClockDecision {
                target_present: self.next_target_present,
                latest_render_start: now,
                latest_submit_or_commit: now,
                mode: self.mode,
                disposition: ClockDisposition::Skip,
            };
        };
        let latest_submit_or_commit = self
            .latest_submit_or_commit
            .unwrap_or(latest_render_start + DEFAULT_SUBMIT_BUDGET);
        let disposition = if !self.pending_work {
            ClockDisposition::Skip
        } else if self.timing_hints.allow_immediate || self.timing_hints.wants_low_latency {
            ClockDisposition::RenderNow
        } else if let Some(target_present) = self.next_target_present {
            if now >= target_present {
                self.last_deadline_miss_ns =
                    Some(now.duration_since(target_present).as_nanos() as i128);
                self.note_missed_frame();
                ClockDisposition::MissedTarget
            } else if now >= latest_render_start {
                ClockDisposition::RenderNow
            } else {
                ClockDisposition::WaitUntil(latest_render_start)
            }
        } else {
            ClockDisposition::RenderNow
        };
        ClockDecision {
            target_present: self.next_target_present,
            latest_render_start,
            latest_submit_or_commit,
            mode: self.mode,
            disposition,
        }
    }

    fn merge_reason(&mut self, reason: RequestFrameReason) {
        let hints = match reason {
            RequestFrameReason::Damage | RequestFrameReason::SurfaceCommit => {
                TimingHints::default()
            }
            RequestFrameReason::Animation => TimingHints {
                cadence_hint: Some(self.nominal_interval()),
                ..TimingHints::default()
            },
            RequestFrameReason::Input => {
                TimingHints { wants_low_latency: true, ..TimingHints::default() }
            }
            RequestFrameReason::ModeChange => {
                TimingHints { allow_immediate: true, ..TimingHints::default() }
            }
            RequestFrameReason::RetryAfterPresent | RequestFrameReason::Explicit => {
                TimingHints { allow_immediate: true, ..TimingHints::default() }
            }
        };
        self.timing_hints = self.timing_hints.merge(hints);
    }

    fn recompute_targets(&mut self) {
        if !self.pending_work {
            self.next_target_present = None;
            self.latest_render_start = None;
            self.latest_submit_or_commit = None;
            return;
        }
        let now = Instant::now();
        if matches!(self.mode, FrameClockMode::Passive) {
            if self.timing_hints.allow_immediate || self.timing_hints.wants_low_latency {
                self.next_target_present = None;
                self.latest_render_start = Some(now);
                self.latest_submit_or_commit = Some(now);
            } else {
                self.next_target_present = None;
                self.latest_render_start = None;
                self.latest_submit_or_commit = None;
            }
            return;
        }
        let cadence = self
            .timing_hints
            .cadence_hint
            .unwrap_or_else(|| self.default_cadence());
        let bootstrap_without_present_history = self.last_present.is_none();
        let earliest_base = self
            .last_present
            .map(|sample| sample.present_time)
            .unwrap_or(now);
        let mut target_present = if bootstrap_without_present_history
            || self.timing_hints.allow_immediate
            || self.timing_hints.wants_low_latency
        {
            now
        } else {
            earliest_base.checked_add(cadence).unwrap_or(now)
        };
        if let Some(min_next_present) = self.timing_hints.min_next_present {
            target_present = max_instant(target_present, min_next_present);
        }
        if let Some(max_next_present) = self.timing_hints.max_next_present {
            target_present = target_present.min(max_next_present);
        }
        if !bootstrap_without_present_history
            && matches!(self.mode, FrameClockMode::Variable)
            && self.vrr_enabled
        {
            if let Some(vrr) = self.vrr {
                let min_interval = refresh_interval(vrr.max_refresh_millihz);
                let max_interval = refresh_interval(vrr.min_refresh_millihz);
                let lower_bound = earliest_base
                    .checked_add(min_interval)
                    .unwrap_or(target_present);
                let upper_bound = earliest_base
                    .checked_add(max_interval)
                    .unwrap_or(target_present);
                target_present = target_present.clamp(lower_bound, upper_bound);
            }
        }
        let latest_submit_or_commit = target_present
            .checked_sub(self.submit_budget + self.safety_margin)
            .unwrap_or(now);
        let latest_render_start = target_present
            .checked_sub(self.render_budget + self.submit_budget + self.safety_margin)
            .unwrap_or(now);
        self.next_target_present = Some(target_present);
        self.latest_submit_or_commit = Some(latest_submit_or_commit);
        self.latest_render_start = Some(latest_render_start);
    }

    fn nominal_interval(&self) -> Duration {
        refresh_interval(self.nominal_refresh_millihz)
    }

    fn default_cadence(&self) -> Duration {
        match self.mode {
            FrameClockMode::Fixed => self.nominal_interval(),
            FrameClockMode::Variable => self
                .recent_present_interval_hint()
                .unwrap_or_else(|| self.nominal_interval()),
            FrameClockMode::Passive => self.nominal_interval(),
        }
    }

    fn recent_present_interval_hint(&self) -> Option<Duration> {
        self.present_intervals.back().copied()
    }

    fn push_present_interval(&mut self, interval: Duration) {
        if self.present_intervals.len() == PRESENT_HISTORY_LIMIT {
            self.present_intervals.pop_front();
        }
        self.present_intervals.push_back(interval);
    }

    fn refresh_budgets_from_measurements(&mut self) {
        self.scene_build_budget = self
            .composited_scene_build_history
            .budget(Duration::from_millis(1))
            .max(
                self.direct_scanout_scene_build_history
                    .budget(Duration::from_millis(1)),
            );
        self.render_submit_budget = self
            .composited_render_submit_history
            .budget(Duration::from_millis(2))
            .max(
                self.direct_scanout_render_submit_history
                    .budget(Duration::ZERO),
            );
        self.render_budget = self.scene_build_budget + self.render_submit_budget;
        self.submit_budget = self
            .composited_submit_history
            .budget(DEFAULT_SUBMIT_BUDGET)
            .max(
                self.direct_scanout_submit_history
                    .budget(DEFAULT_SUBMIT_BUDGET),
            );
    }
}

#[derive(Clone, Debug)]
struct RollingDurationHistory {
    short: VecDeque<Duration>,
    long: VecDeque<Duration>,
}

impl RollingDurationHistory {
    fn new() -> Self {
        Self {
            short: VecDeque::with_capacity(SHORT_BUDGET_WINDOW),
            long: VecDeque::with_capacity(LONG_BUDGET_WINDOW),
        }
    }

    fn push(&mut self, duration: Duration) {
        if self.short.len() == SHORT_BUDGET_WINDOW {
            self.short.pop_front();
        }
        if self.long.len() == LONG_BUDGET_WINDOW {
            self.long.pop_front();
        }
        self.short.push_back(duration);
        self.long.push_back(duration);
    }

    fn budget(&self, default: Duration) -> Duration {
        let short = self.short.iter().copied().max().unwrap_or(default);
        let long = self.long.iter().copied().max().unwrap_or(default);
        short.max(long)
    }
}

fn refresh_interval(refresh_millihz: u32) -> Duration {
    if refresh_millihz == 0 {
        return Duration::from_millis(16);
    }
    Duration::from_nanos((1_000_000_000_000u64 / u64::from(refresh_millihz)).max(1))
}

fn max_instant(lhs: Instant, rhs: Instant) -> Instant {
    if lhs >= rhs {
        lhs
    } else {
        rhs
    }
}

impl TimingHints {
    pub fn merge(self, other: Self) -> Self {
        Self {
            wants_low_latency: self.wants_low_latency || other.wants_low_latency,
            min_next_present: merge_min_present(self.min_next_present, other.min_next_present),
            max_next_present: merge_max_present(self.max_next_present, other.max_next_present),
            cadence_hint: other.cadence_hint.or(self.cadence_hint),
            allow_immediate: self.allow_immediate || other.allow_immediate,
        }
    }
}

fn merge_min_present(lhs: Option<Instant>, rhs: Option<Instant>) -> Option<Instant> {
    match (lhs, rhs) {
        (Some(lhs), Some(rhs)) => Some(max_instant(lhs, rhs)),
        (Some(lhs), None) => Some(lhs),
        (None, Some(rhs)) => Some(rhs),
        (None, None) => None,
    }
}

fn merge_max_present(lhs: Option<Instant>, rhs: Option<Instant>) -> Option<Instant> {
    match (lhs, rhs) {
        (Some(lhs), Some(rhs)) => Some(lhs.min(rhs)),
        (Some(lhs), None) => Some(lhs),
        (None, Some(rhs)) => Some(rhs),
        (None, None) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_mode_waits_until_render_deadline() {
        let mut clock = FrameClock::new();
        clock.update_mode(FrameClockMode::Fixed, 60_000);
        let now = Instant::now();
        clock.notify_presented(PresentSample {
            present_time: now,
            sequence: 1,
            flags: PresentFlags { zero_copy: false, refresh_is_constant: true },
            timing: None,
        });
        clock.request_frame(RequestFrameReason::Damage);
        let decision = clock.decide(now);
        assert!(matches!(
            decision.disposition,
            ClockDisposition::WaitUntil(_) | ClockDisposition::RenderNow
        ));
        assert!(decision.target_present.is_some());
    }

    #[test]
    fn input_requests_can_render_immediately() {
        let mut clock = FrameClock::new();
        clock.request_frame(RequestFrameReason::Input);
        let decision = clock.decide(Instant::now());
        assert_eq!(decision.disposition, ClockDisposition::RenderNow);
    }

    #[test]
    fn mode_change_renders_immediately_on_a_variable_output() {
        let mut clock = FrameClock::new();
        clock.update_mode(FrameClockMode::Variable, 60_000);
        clock.update_vrr_state(
            Some(VrrRange { min_refresh_millihz: 48_000, max_refresh_millihz: 144_000 }),
            true,
        );
        clock.request_frame(RequestFrameReason::ModeChange);
        assert_eq!(clock.decide(Instant::now()).disposition, ClockDisposition::RenderNow);
    }

    #[test]
    fn first_frame_bootstraps_immediately_without_present_history() {
        let now = Instant::now();
        let mut clock = FrameClock::new();
        clock.update_mode(FrameClockMode::Fixed, 60_000);
        clock.request_frame(RequestFrameReason::Damage);
        let decision = clock.decide(now);
        assert_eq!(decision.disposition, ClockDisposition::RenderNow);
        assert!(decision.target_present.is_some_and(|target| {
            target >= now && target.duration_since(now) < Duration::from_millis(2)
        }));
    }

    #[test]
    fn variable_mode_reports_non_constant_refresh() {
        let mut clock = FrameClock::new();
        clock.update_mode(FrameClockMode::Variable, 144_000);
        clock.update_vrr_state(
            Some(VrrRange { min_refresh_millihz: 48_000, max_refresh_millihz: 144_000 }),
            true,
        );
        assert!(!clock.refresh_is_constant());
    }

    #[test]
    fn measured_histories_raise_budgets() {
        let mut clock = FrameClock::new();
        clock.notify_presented(PresentSample {
            present_time: Instant::now(),
            sequence: 1,
            flags: PresentFlags { zero_copy: false, refresh_is_constant: true },
            timing: Some(PresentTiming {
                path: PresentPath::Composited,
                scene_build_duration: Duration::from_millis(2),
                render_submit_duration: Duration::from_millis(5),
                submit_to_present: Some(Duration::from_millis(3)),
            }),
        });

        assert!(clock.render_budget >= Duration::from_millis(7));
        assert!(clock.scene_build_budget >= Duration::from_millis(2));
        assert!(clock.render_submit_budget >= Duration::from_millis(5));
        assert!(clock.submit_budget >= Duration::from_millis(3));
    }

    #[test]
    fn mixed_refresh_outputs_compute_independent_deadlines() {
        let base = Instant::now();
        let mut clock_60 = FrameClock::new();
        let mut clock_144 = FrameClock::new();
        clock_60.update_mode(FrameClockMode::Fixed, 60_000);
        clock_144.update_mode(FrameClockMode::Fixed, 144_000);
        for clock in [&mut clock_60, &mut clock_144] {
            clock.notify_presented(PresentSample {
                present_time: base,
                sequence: 1,
                flags: PresentFlags { zero_copy: false, refresh_is_constant: true },
                timing: None,
            });
            clock.request_frame(RequestFrameReason::Damage);
        }

        let d60 = clock_60.decide(base);
        let d144 = clock_144.decide(base);
        assert!(d60.target_present.is_some() && d144.target_present.is_some());
        assert!(d144.target_present.unwrap() < d60.target_present.unwrap());
        assert!(d144.latest_render_start < d60.latest_render_start);
    }

    #[test]
    fn fixed_mode_ignores_long_present_gap_for_cadence() {
        let base = Instant::now();
        let mut clock = FrameClock::new();
        clock.update_mode(FrameClockMode::Fixed, 60_000);
        clock.notify_presented(PresentSample {
            present_time: base,
            sequence: 1,
            flags: PresentFlags { zero_copy: false, refresh_is_constant: true },
            timing: None,
        });
        clock.notify_presented(PresentSample {
            present_time: base + Duration::from_secs(1),
            sequence: 2,
            flags: PresentFlags { zero_copy: false, refresh_is_constant: true },
            timing: None,
        });
        clock.request_frame(RequestFrameReason::Damage);

        let decision = clock.decide(base + Duration::from_secs(1));
        let target = decision.target_present.expect("target present");
        assert!(target <= base + Duration::from_secs(1) + refresh_interval(60_000));
    }

    #[test]
    fn variable_mode_clamps_targets_into_vrr_window() {
        let base = Instant::now();
        let mut clock = FrameClock::new();
        clock.update_mode(FrameClockMode::Variable, 144_000);
        clock.update_vrr_state(
            Some(VrrRange { min_refresh_millihz: 48_000, max_refresh_millihz: 144_000 }),
            true,
        );
        clock.notify_presented(PresentSample {
            present_time: base,
            sequence: 1,
            flags: PresentFlags { zero_copy: false, refresh_is_constant: false },
            timing: None,
        });
        clock.update_timing_hints(TimingHints {
            cadence_hint: Some(Duration::from_millis(100)),
            ..TimingHints::default()
        });
        clock.request_frame(RequestFrameReason::Animation);

        let decision = clock.decide(base);
        let target = decision.target_present.expect("target present");
        assert!(target >= base + refresh_interval(144_000));
        assert!(target <= base + refresh_interval(48_000));
    }

    #[test]
    fn passive_mode_only_renders_for_explicit_immediate_work() {
        let now = Instant::now();
        let mut clock = FrameClock::new();
        clock.update_mode(FrameClockMode::Passive, 60_000);
        clock.request_frame(RequestFrameReason::Damage);
        assert_eq!(clock.decide(now).disposition, ClockDisposition::Skip);

        let mut explicit_clock = FrameClock::new();
        explicit_clock.update_mode(FrameClockMode::Passive, 60_000);
        explicit_clock.request_frame(RequestFrameReason::Explicit);
        assert_eq!(explicit_clock.decide(now).disposition, ClockDisposition::RenderNow);
    }

    #[test]
    fn missed_target_widens_budget_and_reports_miss_amount() {
        let base = Instant::now();
        let mut clock = FrameClock::new();
        clock.update_mode(FrameClockMode::Fixed, 60_000);
        clock.notify_presented(PresentSample {
            present_time: base,
            sequence: 1,
            flags: PresentFlags { zero_copy: false, refresh_is_constant: true },
            timing: None,
        });
        let before = clock.snapshot().render_budget;
        clock.request_frame(RequestFrameReason::Damage);
        let decision = clock.decide(base + Duration::from_millis(50));
        assert_eq!(decision.disposition, ClockDisposition::MissedTarget);
        let after = clock.snapshot();
        assert!(after.render_budget >= before);
        assert!(after.last_deadline_miss_ns.is_some());
    }
}
