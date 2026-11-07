use crate::{
    runner::Dispatcher,
    scheduler::presentation::{DiscardedFeedback, PresentEvent, PresentationSubsystem},
    state::OutputId,
};

use super::{
    frame_clock::{PresentPath, PresentTiming},
    PresentFlags, PresentSample,
};

#[derive(Clone, Copy, Debug)]
pub enum FrameCompletionOutcome {
    Presented(PresentSample),
    NoPresent,
    Discarded,
    MissedTarget,
}

#[derive(Clone, Copy, Debug)]
pub struct TimelinePresentInfo {
    pub timeline: OutputId,
    pub nominal_refresh_millihz: i32,
    pub refresh_is_constant: bool,
    pub tv_sec: u32,
    pub tv_usec: u32,
    pub sequence: u32,
}

#[derive(Clone, Copy, Debug)]
pub struct TimelineCompletedSubmission {
    pub submission_id: u64,
    pub output_id: Option<OutputId>,
    pub zero_copy: bool,
    pub damage_blob: Option<u64>,
    pub scene_build_duration: std::time::Duration,
    pub render_submit_duration: std::time::Duration,
    pub submitted_at: std::time::Instant,
}

pub fn discard_frame_for_timeline<Cleanup, Deliver>(
    dispatcher: &mut Dispatcher,
    discarded: &[DiscardedFeedback],
    damage_blob: Option<u64>,
    mut cleanup_blob: Cleanup,
    deliver_outcome: Deliver,
) -> FrameCompletionOutcome
where
    Cleanup: FnMut(u64),
    Deliver: FnOnce(&mut Dispatcher, FrameCompletionOutcome),
{
    if !discarded.is_empty() {
        PresentationSubsystem::dispatch_discarded(dispatcher, discarded);
    }
    if let Some(blob) = damage_blob {
        cleanup_blob(blob);
    }
    let outcome = FrameCompletionOutcome::Discarded;
    deliver_outcome(dispatcher, outcome);
    outcome
}

pub fn complete_frame_for_timeline<Advance, Cleanup, Deliver>(
    dispatcher: &mut Dispatcher,
    presentation: &mut PresentationSubsystem,
    present: TimelinePresentInfo,
    advance_pipeline: Advance,
    mut cleanup_blob: Cleanup,
    deliver_outcome: Deliver,
) -> FrameCompletionOutcome
where
    Advance: FnOnce() -> Option<TimelineCompletedSubmission>,
    Cleanup: FnMut(u64),
    Deliver: FnOnce(&mut Dispatcher, FrameCompletionOutcome),
{
    let Some(completed_submission) = advance_pipeline() else {
        let outcome = FrameCompletionOutcome::NoPresent;
        deliver_outcome(dispatcher, outcome);
        return outcome;
    };
    let presented = presentation.complete(
        present.timeline,
        completed_submission.submission_id,
        PresentEvent {
            tv_sec: present.tv_sec,
            tv_usec: present.tv_usec,
            sequence: present.sequence,
            refresh_is_constant: present.refresh_is_constant,
        },
    );
    if let Some(ref presented) = presented {
        PresentationSubsystem::dispatch_presented(
            dispatcher,
            present.nominal_refresh_millihz,
            presented,
        );
    }
    if let Some(blob) = completed_submission.damage_blob {
        cleanup_blob(blob);
    }
    let outcome = FrameCompletionOutcome::Presented(PresentSample {
        present_time: presented
            .as_ref()
            .map(|presented| presented.present_time)
            .unwrap_or_else(std::time::Instant::now),
        sequence: presented
            .as_ref()
            .map(|presented| presented.sequence)
            .unwrap_or_else(|| u64::from(present.sequence)),
        flags: PresentFlags {
            zero_copy: completed_submission.zero_copy,
            refresh_is_constant: present.refresh_is_constant,
        },
        timing: Some(PresentTiming {
            path: if completed_submission.zero_copy {
                PresentPath::DirectScanout
            } else {
                PresentPath::Composited
            },
            scene_build_duration: completed_submission.scene_build_duration,
            render_submit_duration: completed_submission.render_submit_duration,
            submit_to_present: Some(
                presented
                    .as_ref()
                    .map(|presented| presented.present_time)
                    .unwrap_or_else(std::time::Instant::now)
                    .saturating_duration_since(completed_submission.submitted_at),
            ),
        }),
    });
    deliver_outcome(dispatcher, outcome);
    outcome
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use crate::{
        clients::{ClientId, Resolved},
        protocol_generated::{WlSurfaceId, WpPresentationFeedbackId},
        scheduler::presentation::{sanitize_present_timestamp, SubmissionRecord},
        state::{ObjectId, OutputId},
    };

    use super::*;

    fn monotonic_event() -> (u32, u32) {
        let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
        assert_eq!(unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) }, 0);
        (
            u32::try_from(ts.tv_sec.max(0)).unwrap_or(0),
            u32::try_from(ts.tv_nsec.clamp(0, 999_999_999)).unwrap_or(0) / 1000,
        )
    }

    #[test]
    fn completion_uses_present_event_time_for_clock_sample() {
        let timeline = OutputId::from(1usize);
        let (tv_sec, tv_usec) = monotonic_event();
        let sanitized = sanitize_present_timestamp(tv_sec, tv_usec);
        let submitted_at = sanitized
            .present_time
            .checked_sub(Duration::from_millis(5))
            .unwrap_or_else(Instant::now);
        let mut presentation = PresentationSubsystem::default();
        presentation.register_submission(
            timeline,
            SubmissionRecord {
                submission_id: 7,
                output_id: Some(timeline),
                feedbacks: vec![crate::drm::SubmissionPresentationFeedback {
                    feedback: Resolved::new(
                        ClientId::from_raw(1),
                        WpPresentationFeedbackId::from(1u32),
                        ObjectId::from(1usize),
                    ),
                    version: 1,
                    surface: Some(Resolved::new(
                        ClientId::from_raw(1),
                        WlSurfaceId::from(2u32),
                        ObjectId::from(2usize),
                    )),
                }],
                zero_copy: false,
                async_flip: false,
            },
        );
        let mut dispatcher = crate::runner::Dispatcher::default();

        let outcome = complete_frame_for_timeline(
            &mut dispatcher,
            &mut presentation,
            TimelinePresentInfo {
                timeline,
                nominal_refresh_millihz: 60_000,
                refresh_is_constant: true,
                tv_sec,
                tv_usec,
                sequence: 9,
            },
            || {
                Some(TimelineCompletedSubmission {
                    submission_id: 7,
                    output_id: Some(timeline),
                    zero_copy: false,
                    damage_blob: None,
                    scene_build_duration: Duration::ZERO,
                    render_submit_duration: Duration::ZERO,
                    submitted_at,
                })
            },
            |_| {},
            |_dispatcher, _outcome| {},
        );

        let FrameCompletionOutcome::Presented(sample) = outcome else {
            panic!("expected presented outcome");
        };
        let present_error = if sample.present_time >= sanitized.present_time {
            sample.present_time.duration_since(sanitized.present_time)
        } else {
            sanitized.present_time.duration_since(sample.present_time)
        };
        assert!(present_error < Duration::from_millis(2));
        assert!(sample
            .timing
            .and_then(|timing| timing.submit_to_present)
            .is_some_and(
                |delta| delta >= Duration::from_millis(1) && delta < Duration::from_millis(20)
            ));
    }
}
