use std::{
    collections::{BTreeMap, BTreeSet},
    time::{Duration, Instant},
};

use crate::{
    clients::{ClientAction, ClientId},
    protocol_generated::WlSurfaceId,
    runner::Dispatcher,
    state::OutputId,
};

use crate::drm::SubmissionPresentationFeedback;

pub(crate) type OutputTimelineId = OutputId;

#[derive(Debug, Default)]
pub struct PresentationSubsystem {
    timelines: BTreeMap<OutputTimelineId, TimelinePresentationState>,
    surface_sequences: BTreeMap<crate::clients::Resolved<WlSurfaceId>, u64>,
}

#[derive(Debug, Default)]
struct TimelinePresentationState {
    submissions: BTreeMap<u64, SubmissionRecord>,
}

#[derive(Clone, Debug)]
pub(crate) struct SubmissionRecord {
    pub submission_id: u64,
    pub output_id: Option<OutputId>,
    pub feedbacks: Vec<SubmissionPresentationFeedback>,
    pub zero_copy: bool,
    /// The commit that landed used `DRM_MODE_PAGE_FLIP_ASYNC`.
    pub async_flip: bool,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct PresentEvent {
    pub tv_sec: u32,
    pub tv_usec: u32,
    pub sequence: u32,
    pub refresh_is_constant: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct PresentedSubmission {
    pub output_id: Option<OutputId>,
    pub feedbacks: Vec<SubmissionPresentationFeedback>,
    pub zero_copy: bool,
    pub async_flip: bool,
    pub present_time: Instant,
    pub sequence: u64,
    pub tv_sec_hi: u32,
    pub tv_sec_lo: u32,
    pub tv_nsec: u32,
    pub seq_hi: u32,
    pub seq_lo: u32,
    pub refresh_is_constant: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct DiscardedFeedback {
    pub feedbacks: Vec<SubmissionPresentationFeedback>,
}

impl PresentationSubsystem {
    pub(crate) fn register_submission(
        &mut self,
        timeline: OutputTimelineId,
        submission: SubmissionRecord,
    ) {
        self.timelines
            .entry(timeline)
            .or_default()
            .submissions
            .insert(submission.submission_id, submission);
    }

    pub(crate) fn complete(
        &mut self,
        timeline: OutputTimelineId,
        submission_id: u64,
        event: PresentEvent,
    ) -> Option<PresentedSubmission> {
        let submission = {
            let timeline_state = self.timelines.get_mut(&timeline)?;
            timeline_state.submissions.remove(&submission_id)?
        };
        let timestamp = sanitize_present_timestamp(event.tv_sec, event.tv_usec);
        let sequence = self.normalize_sequence(event.sequence, &submission.feedbacks);
        Some(PresentedSubmission {
            output_id: submission.output_id,
            feedbacks: submission.feedbacks,
            zero_copy: submission.zero_copy,
            async_flip: submission.async_flip,
            present_time: timestamp.present_time,
            sequence,
            tv_sec_hi: (timestamp.tv_sec >> 32) as u32,
            tv_sec_lo: timestamp.tv_sec as u32,
            tv_nsec: timestamp.tv_nsec,
            seq_hi: (sequence >> 32) as u32,
            seq_lo: sequence as u32,
            refresh_is_constant: event.refresh_is_constant,
        })
    }

    pub(crate) fn discard_submission(
        &mut self,
        timeline: OutputTimelineId,
        submission_id: u64,
    ) -> Vec<DiscardedFeedback> {
        let Some(timeline_state) = self.timelines.get_mut(&timeline) else {
            return Vec::new();
        };
        let Some(submission) = timeline_state.submissions.remove(&submission_id) else {
            return Vec::new();
        };
        vec![DiscardedFeedback { feedbacks: submission.feedbacks }]
    }

    pub(crate) fn discard_timeline(
        &mut self,
        timeline: OutputTimelineId,
    ) -> Vec<DiscardedFeedback> {
        let Some(timeline_state) = self.timelines.remove(&timeline) else {
            return Vec::new();
        };
        timeline_state
            .submissions
            .into_values()
            .map(|submission| DiscardedFeedback { feedbacks: submission.feedbacks })
            .collect()
    }

    pub(crate) fn dispatch_discarded(dispatcher: &mut Dispatcher, discarded: &[DiscardedFeedback]) {
        let mut per_client = BTreeMap::<ClientId, Vec<_>>::new();
        for entry in discarded {
            for feedback in &entry.feedbacks {
                per_client
                    .entry(feedback.feedback.client_id())
                    .or_default()
                    .push(feedback.feedback);
            }
        }
        for (client_id, feedbacks) in per_client {
            dispatcher
                .dispatch_to(client_id, ClientAction::PresentationFeedbackDiscarded { feedbacks });
        }
    }

    pub(crate) fn dispatch_presented(
        dispatcher: &mut Dispatcher,
        nominal_refresh_millihz: i32,
        presented: &PresentedSubmission,
    ) {
        if presented.feedbacks.is_empty() {
            return;
        }
        let flags = presentation_feedback_flags(presented.zero_copy, !presented.async_flip);

        let mut per_client = BTreeMap::<(ClientId, u32), Vec<_>>::new();
        for feedback in &presented.feedbacks {
            per_client
                .entry((feedback.feedback.client_id(), feedback.version))
                .or_default()
                .push(feedback.feedback);
        }
        for ((client_id, feedback_version), feedbacks) in per_client {
            let refresh = presentation_refresh_interval_ns(
                nominal_refresh_millihz,
                feedback_version,
                presented.refresh_is_constant,
            );
            dispatcher.dispatch_to(
                client_id,
                ClientAction::PresentationFeedbackPresented {
                    feedbacks,
                    output_id: presented.output_id,
                    tv_sec_hi: presented.tv_sec_hi,
                    tv_sec_lo: presented.tv_sec_lo,
                    tv_nsec: presented.tv_nsec,
                    refresh,
                    seq_hi: presented.seq_hi,
                    seq_lo: presented.seq_lo,
                    flags,
                },
            );
        }
    }

    fn normalize_sequence(
        &mut self,
        raw_sequence: u32,
        feedbacks: &[SubmissionPresentationFeedback],
    ) -> u64 {
        let mut normalized = u64::from(raw_sequence);
        let mut surfaces = BTreeSet::new();
        for feedback in feedbacks {
            if let Some(surface) = feedback.surface {
                normalized = normalized.max(
                    self.surface_sequences
                        .get(&surface)
                        .copied()
                        .unwrap_or(0)
                        .saturating_add(1),
                );
                surfaces.insert(surface);
            }
        }
        if normalized == 0 && !surfaces.is_empty() {
            normalized = 1;
        }
        for surface in surfaces {
            self.surface_sequences.insert(surface, normalized);
        }
        normalized
    }
}

#[cfg(test)]
mod tests {
    use {
        super::*,
        crate::{
            clients::{ClientId, Resolved},
            protocol_generated::{WlSurfaceId, WpPresentationFeedbackId},
            state::OutputId,
        },
    };

    fn feedback(id: u32, version: u32, surface_id: u32) -> SubmissionPresentationFeedback {
        SubmissionPresentationFeedback {
            feedback: Resolved::new(
                ClientId::from_raw(1),
                WpPresentationFeedbackId::from(id),
                crate::state::ObjectId::from(id as usize),
            ),
            version,
            surface: Some(Resolved::new(
                ClientId::from_raw(1),
                WlSurfaceId::from(surface_id),
                crate::state::ObjectId::from(surface_id as usize),
            )),
        }
    }

    #[test]
    fn composited_fallback_submission_is_presented_as_non_zero_copy() {
        let mut presentation = PresentationSubsystem::default();
        let timeline = OutputId::from(1usize);
        presentation.register_submission(
            timeline,
            SubmissionRecord {
                submission_id: 7,
                output_id: Some(timeline),
                feedbacks: vec![feedback(1, 1, 10)],
                zero_copy: false,
                async_flip: false,
            },
        );

        let presented = presentation
            .complete(
                timeline,
                7,
                PresentEvent {
                    tv_sec: 10,
                    tv_usec: 100,
                    sequence: 9,
                    refresh_is_constant: true,
                },
            )
            .expect("presented submission");

        assert!(!presented.zero_copy);
        assert_eq!(presented.output_id, Some(timeline));
    }

    #[test]
    fn presentation_sequences_stay_monotonic_when_surface_moves_outputs() {
        let mut presentation = PresentationSubsystem::default();
        let first_timeline = OutputId::from(1usize);
        let second_timeline = OutputId::from(2usize);
        presentation.register_submission(
            first_timeline,
            SubmissionRecord {
                submission_id: 1,
                output_id: Some(first_timeline),
                feedbacks: vec![feedback(1, 1, 42)],
                zero_copy: false,
                async_flip: false,
            },
        );
        let first = presentation
            .complete(
                first_timeline,
                1,
                PresentEvent {
                    tv_sec: 10,
                    tv_usec: 100,
                    sequence: 100,
                    refresh_is_constant: true,
                },
            )
            .expect("first presented submission");
        presentation.register_submission(
            second_timeline,
            SubmissionRecord {
                submission_id: 2,
                output_id: Some(second_timeline),
                feedbacks: vec![feedback(2, 1, 42)],
                zero_copy: false,
                async_flip: false,
            },
        );
        let second = presentation
            .complete(
                second_timeline,
                2,
                PresentEvent {
                    tv_sec: 11,
                    tv_usec: 100,
                    sequence: 3,
                    refresh_is_constant: true,
                },
            )
            .expect("second presented submission");

        assert!(second.sequence > first.sequence);
    }

    fn presented_flags(async_flip: bool, zero_copy: bool) -> u32 {
        let mut presentation = PresentationSubsystem::default();
        let timeline = OutputId::from(1usize);
        presentation.register_submission(
            timeline,
            SubmissionRecord {
                submission_id: 1,
                output_id: Some(timeline),
                feedbacks: vec![feedback(1, 1, 10)],
                zero_copy,
                async_flip,
            },
        );
        let presented = presentation
            .complete(
                timeline,
                1,
                PresentEvent {
                    tv_sec: 10,
                    tv_usec: 100,
                    sequence: 4,
                    refresh_is_constant: true,
                },
            )
            .expect("presented submission");
        let mut dispatcher = crate::runner::Dispatcher::default();
        PresentationSubsystem::dispatch_presented(&mut dispatcher, 60_000, &presented);
        let mut flags = None;
        dispatcher.test_for_each_queued_action(|action: &crate::clients::ClientAction| {
            if let crate::clients::ClientAction::PresentationFeedbackPresented {
                flags: presented_flags,
                ..
            } = action
            {
                flags = Some(*presented_flags);
            }
        });
        flags.expect("presented feedback")
    }

    #[test]
    fn async_submission_drops_vsync_and_a_sync_fallback_keeps_it() {
        use crate::protocol_generated::wp_presentation_feedback::Kind;

        let vsync = Kind::Vsync as u32;
        let hw_completion = Kind::HwCompletion as u32;
        let zero_copy = Kind::ZeroCopy as u32;

        let async_flags = presented_flags(true, false);
        assert_eq!(async_flags & vsync, 0);
        assert_ne!(async_flags & hw_completion, 0);
        assert_eq!(async_flags & zero_copy, 0);

        let async_scanout = presented_flags(true, true);
        assert_eq!(async_scanout & vsync, 0);
        assert_ne!(async_scanout & hw_completion, 0);
        assert_ne!(async_scanout & zero_copy, 0);

        let sync_flags = presented_flags(false, false);
        assert_ne!(sync_flags & vsync, 0);
        assert_ne!(sync_flags & hw_completion, 0);
        assert_eq!(sync_flags & zero_copy, 0);
    }

    #[test]
    fn presentation_refresh_is_zero_for_v1_variable_rate_feedback() {
        assert_eq!(presentation_refresh_interval_ns(60_000, 1, false), 0);
        assert_ne!(presentation_refresh_interval_ns(60_000, 2, false), 0);
    }

    #[test]
    fn sanitize_present_timestamp_falls_back_for_suspicious_values() {
        let timestamp = sanitize_present_timestamp(1, 2_000_000);
        assert!(timestamp.tv_sec > 0);
        assert!(timestamp.tv_nsec < 1_000_000_000);
    }
}

pub const WP_PRESENTATION_FEEDBACK_PRESENTED_REFRESH_RELAXED_SINCE: u32 = 2;

pub fn presentation_refresh_interval_ns(
    refresh_millihz: i32,
    feedback_version: u32,
    refresh_is_constant: bool,
) -> u32 {
    if refresh_millihz <= 0 {
        return 0;
    }
    if feedback_version < WP_PRESENTATION_FEEDBACK_PRESENTED_REFRESH_RELAXED_SINCE
        && !refresh_is_constant
    {
        return 0;
    }
    let refresh = u64::try_from(refresh_millihz).unwrap_or(0);
    let refresh_ns = 1_000_000_000_000u64 / refresh;
    refresh_ns.min(u64::from(u32::MAX)) as u32
}

pub fn presentation_feedback_flags(zero_copy: bool, vsync: bool) -> u32 {
    use crate::protocol_generated::wp_presentation_feedback::Kind;

    let mut flags = Kind::HwCompletion as u32;
    if vsync {
        flags |= Kind::Vsync as u32;
    }
    if zero_copy {
        flags |= Kind::ZeroCopy as u32;
    }
    flags
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct SanitizedPresentTimestamp {
    pub tv_sec: u64,
    pub tv_nsec: u32,
    pub present_time: Instant,
}

pub(crate) fn sanitize_present_timestamp(tv_sec: u32, tv_usec: u32) -> SanitizedPresentTimestamp {
    let sampled_now = sample_monotonic_now();
    if tv_usec < 1_000_000 && tv_sec > 2 {
        let tv_sec = u64::from(tv_sec);
        let tv_nsec = tv_usec.saturating_mul(1000);
        let present_time = sampled_now
            .and_then(|(now_instant, now_sec, now_nsec)| {
                monotonic_timestamp_to_instant(now_instant, now_sec, now_nsec, tv_sec, tv_nsec)
            })
            .unwrap_or_else(Instant::now);
        return SanitizedPresentTimestamp { tv_sec, tv_nsec, present_time };
    }

    let Some((present_time, tv_sec, tv_nsec)) = sampled_now else {
        return SanitizedPresentTimestamp {
            tv_sec: u64::from(tv_sec),
            tv_nsec: tv_usec.min(999_999).saturating_mul(1000),
            present_time: Instant::now(),
        };
    };
    SanitizedPresentTimestamp { tv_sec, tv_nsec, present_time }
}

fn sample_monotonic_now() -> Option<(Instant, u64, u32)> {
    let present_time = Instant::now();
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) } != 0 {
        return None;
    }
    Some((
        present_time,
        u64::try_from(ts.tv_sec.max(0)).unwrap_or(0),
        u32::try_from(ts.tv_nsec.clamp(0, 999_999_999)).unwrap_or(0),
    ))
}

fn monotonic_timestamp_to_instant(
    now_instant: Instant,
    now_sec: u64,
    now_nsec: u32,
    event_sec: u64,
    event_nsec: u32,
) -> Option<Instant> {
    let now =
        Duration::from_secs(now_sec).checked_add(Duration::from_nanos(u64::from(now_nsec)))?;
    let event =
        Duration::from_secs(event_sec).checked_add(Duration::from_nanos(u64::from(event_nsec)))?;
    if now >= event {
        now_instant.checked_sub(now - event)
    } else {
        now_instant.checked_add(event - now)
    }
}
