use std::{collections::VecDeque, time::Instant};

use super::{
    CompletedSubmission, DamageRegion, DeferredRelease, DrmFrameBuffer, OutputSubmission, RectI,
    RenderBuffer, SceneItemState, SubmissionPresentationFeedback, SubmittedFrame, SurfaceKey,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum PipelinePhase {
    Idle,
    Rendering,
    WaitingForFlip,
}

impl Default for PipelinePhase {
    fn default() -> Self {
        Self::Idle
    }
}

#[derive(Debug)]
pub(super) struct OutputPipeline {
    buffers: [RenderBuffer; 2],
    core: OutputPipelineCore,
}

#[derive(Debug, Default)]
struct OutputPipelineCore {
    phase: PipelinePhase,
    displayed_buffer_index: usize,
    render_buffer_index: usize,
    in_flight_releases: Vec<DeferredRelease>,
    present_count: u64,
    damage_history: VecDeque<DamageRegion>,
    last_scene: Vec<SceneItemState>,
    last_submission_had_effects: bool,
    force_full_redraw: bool,
    commits_stopped: bool,
    rendered_buffer_present_id: [Option<u64>; 2],
    in_flight_frame: Option<SubmittedFrame>,
    displayed_external_fb: Option<DrmFrameBuffer>,
    retiring_external_fb: Option<DrmFrameBuffer>,
    displayed_cursor_slot: Option<usize>,
    retiring_cursor_slot: Option<usize>,
    next_submission_id: u64,
    waiting_for_flip_since: Option<Instant>,
    pending_damage: Vec<PendingDamage>,
    open_damage_generation: u64,
    planned_damage_through: Option<u64>,
    submitted_damage_through: Option<u64>,
}

#[derive(Clone, Debug)]
struct PendingDamage {
    generation: u64,
    region: DamageRegion,
}

impl OutputPipeline {
    pub(super) fn new(buffers: [RenderBuffer; 2]) -> Self {
        Self {
            buffers,
            core: OutputPipelineCore {
                phase: PipelinePhase::Idle,
                displayed_buffer_index: 0,
                render_buffer_index: 1,
                in_flight_releases: Vec::new(),
                present_count: 0,
                damage_history: VecDeque::new(),
                last_scene: Vec::new(),
                last_submission_had_effects: false,
                force_full_redraw: true,
                commits_stopped: false,
                rendered_buffer_present_id: [None, None],
                in_flight_frame: None,
                displayed_external_fb: None,
                retiring_external_fb: None,
                displayed_cursor_slot: None,
                retiring_cursor_slot: None,
                next_submission_id: 1,
                waiting_for_flip_since: None,
                pending_damage: Vec::new(),
                open_damage_generation: 0,
                planned_damage_through: None,
                submitted_damage_through: None,
            },
        }
    }

    pub(super) fn phase(&self) -> PipelinePhase {
        self.core.phase()
    }

    pub(super) fn begin_render(&mut self) -> bool {
        self.core.begin_render()
    }

    pub(super) fn mark_render_failed(&mut self) {
        self.core.mark_render_failed();
    }

    pub(super) fn mark_frame_idle(&mut self) {
        self.core.mark_frame_idle();
    }

    pub(super) fn render_buffer(&self) -> &RenderBuffer {
        &self.buffers[self.core.render_buffer_index]
    }

    pub(super) fn render_buffer_mut(&mut self) -> &mut RenderBuffer {
        &mut self.buffers[self.core.render_buffer_index]
    }

    pub(super) fn render_buffer_index(&self) -> usize {
        self.core.render_buffer_index
    }

    pub(super) fn buffer_age(&self) -> Option<usize> {
        self.core.buffer_age()
    }

    pub(super) fn last_scene(&self) -> &[SceneItemState] {
        self.core.last_scene()
    }

    pub(super) fn last_submission_had_effects(&self) -> bool {
        self.core.last_submission_had_effects
    }

    pub(super) fn force_full_redraw(&self) -> bool {
        self.core.force_full_redraw
    }

    pub(super) fn set_force_full_redraw(&mut self, force: bool) {
        self.core.force_full_redraw = force;
    }

    pub(super) fn accumulate_damage_from_age(
        &self,
        damage: DamageRegion,
        output_bounds: RectI,
    ) -> DamageRegion {
        self.core.accumulate_damage_from_age(damage, output_bounds)
    }

    pub(super) fn queue_pending_damage(&mut self, rect: RectI) {
        self.core.queue_pending_damage(rect);
    }

    pub(super) fn snapshot_pending_damage(&mut self) -> DamageRegion {
        self.core.snapshot_pending_damage()
    }

    pub(super) fn preserve_render_buffer_contents(&self) -> bool {
        super::ENABLE_DAMAGE && self.core.buffer_age().is_some() && !self.core.force_full_redraw
    }

    pub(super) fn displayed_cursor_slot(&self) -> Option<usize> {
        self.core.displayed_cursor_slot
    }

    pub(super) fn has_displayed_cursor_slot(&self) -> bool {
        self.core.displayed_cursor_slot.is_some()
    }

    pub(super) fn is_waiting_for_flip(&self) -> bool {
        self.core.is_waiting_for_flip()
    }

    pub(super) fn waiting_for_flip_since(&self) -> Option<Instant> {
        self.core.waiting_for_flip_since
    }

    pub(super) fn set_in_flight_releases(&mut self, releases: Vec<DeferredRelease>) {
        self.core.in_flight_releases = releases;
    }

    pub(super) fn take_in_flight_releases(&mut self) -> Vec<DeferredRelease> {
        std::mem::take(&mut self.core.in_flight_releases)
    }

    pub(super) fn allocate_submission(
        &mut self,
        output_id: Option<super::OutputId>,
        scene: Vec<SceneItemState>,
        frame_callback_surfaces: Vec<SurfaceKey>,
        presentation_feedbacks: Vec<SubmissionPresentationFeedback>,
        zero_copy: bool,
        had_effects: bool,
        scene_build_duration: std::time::Duration,
        render_submit_duration: std::time::Duration,
        submitted_at: Instant,
        protected: bool,
        async_flip: bool,
    ) -> OutputSubmission {
        let id = self.core.next_submission_id;
        self.core.next_submission_id = self.core.next_submission_id.saturating_add(1);
        OutputSubmission {
            id,
            output_id,
            scene,
            frame_callback_surfaces,
            presentation_feedbacks,
            zero_copy,
            had_effects,
            scene_build_duration,
            render_submit_duration,
            submitted_at,
            protected,
            async_flip,
        }
    }

    pub(super) fn mark_frame_submitted(&mut self, frame: SubmittedFrame) {
        self.core.mark_frame_submitted(frame);
    }

    pub(super) fn finish_page_flip(&mut self) -> Option<CompletedSubmission> {
        self.core.finish_page_flip()
    }

    pub(super) fn take_in_flight_presentation_feedbacks_for_discard(
        &mut self,
    ) -> (Vec<SubmissionPresentationFeedback>, Option<u64>) {
        self.core
            .take_in_flight_presentation_feedbacks_for_discard()
    }

    pub(super) fn reset_after_session_resume(
        &mut self,
    ) -> (Vec<DeferredRelease>, Vec<SubmissionPresentationFeedback>, Option<u64>) {
        let releases = self.take_in_flight_releases();
        let (feedbacks, damage_blob) = self.take_in_flight_presentation_feedbacks_for_discard();
        self.mark_render_failed();
        (releases, feedbacks, damage_blob)
    }

    /// Stop new commits, drop in-flight bookkeeping, and keep the next frame full.
    pub(super) fn quiesce(
        &mut self,
    ) -> (Vec<DeferredRelease>, Vec<SubmissionPresentationFeedback>, Option<u64>) {
        self.core.commits_stopped = true;
        self.reset_after_session_resume()
    }

    pub(super) fn resume_commits(&mut self) {
        self.core.commits_stopped = false;
    }
}

impl OutputPipelineCore {
    fn phase(&self) -> PipelinePhase {
        self.phase
    }

    fn begin_render(&mut self) -> bool {
        if self.commits_stopped || !matches!(self.phase, PipelinePhase::Idle) {
            return false;
        }
        self.phase = PipelinePhase::Rendering;
        true
    }

    fn mark_render_failed(&mut self) {
        self.phase = PipelinePhase::Idle;
        self.force_full_redraw = true;
        self.in_flight_frame = None;
        self.waiting_for_flip_since = None;
        self.submitted_damage_through = None;
    }

    fn queue_pending_damage(&mut self, rect: RectI) {
        if rect.is_empty() {
            return;
        }
        if let Some(open) = self.pending_damage.last_mut() {
            if open.generation == self.open_damage_generation {
                open.region.add_rect(rect);
                return;
            }
        }
        self.pending_damage.push(PendingDamage {
            generation: self.open_damage_generation,
            region: DamageRegion::from_rect(rect),
        });
    }

    fn snapshot_pending_damage(&mut self) -> DamageRegion {
        let through = self.open_damage_generation;
        let mut region = DamageRegion::new();
        for entry in &self.pending_damage {
            if entry.generation <= through {
                region.add_region(&entry.region);
            }
        }
        self.open_damage_generation = self.open_damage_generation.saturating_add(1);
        self.planned_damage_through = Some(through);
        region
    }

    fn retire_submitted_pending_damage(&mut self) {
        let Some(through) = self.submitted_damage_through.take() else {
            return;
        };
        self.pending_damage
            .retain(|entry| entry.generation > through);
        if self
            .planned_damage_through
            .is_some_and(|planned| planned <= through)
        {
            self.planned_damage_through = None;
        }
    }

    fn accumulate_damage_from_age(
        &self,
        mut damage: DamageRegion,
        output_bounds: RectI,
    ) -> DamageRegion {
        if !super::ENABLE_DAMAGE {
            damage.add_rect(output_bounds);
            return damage;
        }
        let Some(age) = self.buffer_age() else {
            damage.add_rect(output_bounds);
            return damage;
        };
        if age == 0 || self.force_full_redraw {
            damage.add_rect(output_bounds);
            return damage;
        }
        if age.saturating_sub(1) > self.damage_history.len() {
            damage.add_rect(output_bounds);
            return damage;
        }
        for previous in self.damage_history.iter().take(age.saturating_sub(1)) {
            damage.add_region(previous);
        }
        damage
    }

    fn mark_frame_idle(&mut self) {
        self.phase = PipelinePhase::Idle;
    }

    fn buffer_age(&self) -> Option<usize> {
        let last_presented = self.rendered_buffer_present_id[self.render_buffer_index]?;
        let age = self
            .present_count
            .checked_sub(last_presented)?
            .saturating_add(1);
        usize::try_from(age).ok()
    }

    fn last_scene(&self) -> &[SceneItemState] {
        &self.last_scene
    }

    fn is_waiting_for_flip(&self) -> bool {
        matches!(self.phase, PipelinePhase::WaitingForFlip)
    }

    fn mark_frame_submitted(&mut self, frame: SubmittedFrame) {
        self.phase = PipelinePhase::WaitingForFlip;
        self.waiting_for_flip_since = Some(Instant::now());
        self.in_flight_frame = Some(frame);
        self.submitted_damage_through = self.planned_damage_through;
        self.retiring_external_fb = self.displayed_external_fb.take();
        self.retiring_cursor_slot = self.displayed_cursor_slot.take();
    }

    fn finish_page_flip(&mut self) -> Option<CompletedSubmission> {
        if !matches!(self.phase, PipelinePhase::WaitingForFlip) {
            return None;
        }

        self.phase = PipelinePhase::Idle;
        self.waiting_for_flip_since = None;
        self.present_count = self.present_count.saturating_add(1);
        let completed_submission = match self.in_flight_frame.take() {
            Some(SubmittedFrame::Composited {
                buffer_index,
                damage,
                submission,
                damage_blob,
                cursor_slot,
            }) => {
                self.displayed_buffer_index = buffer_index;
                self.render_buffer_index = if buffer_index == 0 { 1 } else { 0 };
                self.rendered_buffer_present_id[buffer_index] = Some(self.present_count);
                self.damage_history.push_front(damage);
                while self.damage_history.len() > 4 {
                    self.damage_history.pop_back();
                }
                self.last_scene = submission.scene.clone();
                self.last_submission_had_effects = submission.had_effects;
                self.displayed_external_fb = None;
                self.displayed_cursor_slot = cursor_slot;
                self.force_full_redraw = false;
                self.retire_submitted_pending_damage();
                Some(CompletedSubmission { submission, damage_blob })
            }
            Some(SubmittedFrame::DirectScanout { submission, framebuffer, cursor_slot }) => {
                self.force_full_redraw = true;
                self.retire_submitted_pending_damage();
                self.last_scene = submission.scene.clone();
                self.last_submission_had_effects = false;
                self.displayed_external_fb = Some(framebuffer);
                self.displayed_cursor_slot = cursor_slot;
                self.rendered_buffer_present_id = [None, None];
                self.damage_history.clear();
                Some(CompletedSubmission { submission, damage_blob: None })
            }
            None => None,
        };
        self.retiring_external_fb = None;
        self.retiring_cursor_slot = None;
        completed_submission
    }

    fn take_in_flight_presentation_feedbacks_for_discard(
        &mut self,
    ) -> (Vec<SubmissionPresentationFeedback>, Option<u64>) {
        self.phase = PipelinePhase::Idle;
        self.waiting_for_flip_since = None;
        let Some(in_flight) = self.in_flight_frame.take() else {
            return (Vec::new(), None);
        };
        super::submitted_frame_feedbacks_for_discard(in_flight)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_submission(id: u64) -> OutputSubmission {
        OutputSubmission {
            id,
            output_id: None,
            scene: Vec::new(),
            frame_callback_surfaces: Vec::new(),
            presentation_feedbacks: Vec::new(),
            zero_copy: false,
            had_effects: false,
            scene_build_duration: std::time::Duration::ZERO,
            render_submit_duration: std::time::Duration::ZERO,
            submitted_at: Instant::now(),
            protected: false,
            async_flip: false,
        }
    }

    #[test]
    fn one_flip_in_flight_blocks_second_render_start() {
        let mut core = OutputPipelineCore {
            phase: PipelinePhase::Idle,
            render_buffer_index: 1,
            force_full_redraw: true,
            next_submission_id: 1,
            ..Default::default()
        };
        assert!(core.begin_render());
        core.mark_frame_submitted(SubmittedFrame::Composited {
            buffer_index: 1,
            damage: DamageRegion::new(),
            submission: test_submission(1),
            damage_blob: None,
            cursor_slot: None,
        });
        assert_eq!(core.phase(), PipelinePhase::WaitingForFlip);
        assert!(!core.begin_render());
    }

    #[test]
    fn stopping_commits_blocks_render_and_marks_a_full_redraw() {
        let mut core = OutputPipelineCore::default();
        core.phase = PipelinePhase::WaitingForFlip;
        core.force_full_redraw = false;
        core.commits_stopped = true;
        core.mark_render_failed();
        assert_eq!(core.phase(), PipelinePhase::Idle);
        assert!(core.force_full_redraw);
        assert!(!core.begin_render());
    }

    #[test]
    fn begin_render_starts_immediately_when_pipeline_is_idle() {
        let mut core = OutputPipelineCore {
            phase: PipelinePhase::Idle,
            render_buffer_index: 1,
            force_full_redraw: true,
            next_submission_id: 1,
            ..Default::default()
        };
        assert!(core.begin_render());
        assert_eq!(core.phase(), PipelinePhase::Rendering);
    }

    fn rect(x: i32, y: i32, width: i32, height: i32) -> RectI {
        RectI::new(x, y, width, height).unwrap()
    }

    fn covers(core: &OutputPipelineCore, area: RectI) -> bool {
        core.pending_damage
            .iter()
            .any(|entry| entry.region.intersects_rect(area))
    }

    fn submit(core: &mut OutputPipelineCore) {
        core.mark_frame_submitted(SubmittedFrame::Composited {
            buffer_index: 0,
            damage: DamageRegion::new(),
            submission: test_submission(1),
            damage_blob: None,
            cursor_slot: None,
        });
    }

    #[test]
    fn queued_damage_stays_on_its_output_pipeline() {
        let left = rect(90, 0, 10, 20);
        let right = rect(0, 0, 20, 20);
        let mut left_core = OutputPipelineCore::default();
        let mut right_core = OutputPipelineCore::default();
        left_core.queue_pending_damage(left);
        right_core.queue_pending_damage(right);
        assert!(covers(&left_core, left));
        assert!(!covers(&left_core, right));
        assert!(covers(&right_core, right));
        assert!(!covers(&right_core, left));
        assert!(left_core.last_scene.is_empty());
        assert!(right_core.last_scene.is_empty());
    }

    #[test]
    fn damage_queued_during_flight_survives_successful_retirement() {
        let presented = rect(0, 0, 10, 10);
        let newer = rect(20, 0, 10, 10);
        let mut core = OutputPipelineCore::default();
        core.queue_pending_damage(presented);
        let snapshot = core.snapshot_pending_damage();
        assert!(snapshot.intersects_rect(presented));
        core.queue_pending_damage(newer);
        submit(&mut core);
        assert!(core.finish_page_flip().is_some());
        assert!(!covers(&core, presented));
        assert!(covers(&core, newer));
        assert!(core.last_scene.is_empty());
    }

    #[test]
    fn render_and_submission_failure_keep_queued_damage() {
        let area = rect(4, 4, 12, 12);
        let mut core = OutputPipelineCore::default();
        core.queue_pending_damage(area);
        let snapshot = core.snapshot_pending_damage();
        assert!(snapshot.intersects_rect(area));
        core.mark_render_failed();
        assert!(covers(&core, area));
        assert!(core.snapshot_pending_damage().intersects_rect(area));

        let mut submitted = OutputPipelineCore::default();
        submitted.queue_pending_damage(area);
        submitted.snapshot_pending_damage();
        submit(&mut submitted);
        submitted.queue_pending_damage(rect(40, 0, 8, 8));
        submitted.mark_render_failed();
        assert!(covers(&submitted, area));
        assert!(covers(&submitted, rect(40, 0, 8, 8)));

        let mut discarded = OutputPipelineCore::default();
        discarded.queue_pending_damage(area);
        discarded.snapshot_pending_damage();
        submit(&mut discarded);
        let _ = discarded.take_in_flight_presentation_feedbacks_for_discard();
        assert!(covers(&discarded, area));
        assert!(discarded.finish_page_flip().is_none());
        assert!(covers(&discarded, area));
    }

    #[test]
    fn successful_submission_retires_the_snapshotted_generation() {
        let area = rect(1, 2, 3, 4);
        let mut core = OutputPipelineCore::default();
        core.queue_pending_damage(area);
        assert!(core.snapshot_pending_damage().intersects_rect(area));
        submit(&mut core);
        assert!(core.finish_page_flip().is_some());
        assert!(!covers(&core, area));
        assert!(core.pending_damage.is_empty());
    }

    #[test]
    fn pending_damage_is_unioned_before_buffer_age_accumulation() {
        let output = rect(0, 0, 100, 100);
        let scene = rect(0, 0, 10, 10);
        let pending = rect(20, 0, 10, 10);
        let history = rect(40, 0, 10, 10);
        let mut core = OutputPipelineCore {
            force_full_redraw: false,
            present_count: 2,
            rendered_buffer_present_id: [Some(1), None],
            ..Default::default()
        };
        core.damage_history
            .push_front(DamageRegion::from_rect(history));
        core.queue_pending_damage(pending);
        let mut base = DamageRegion::from_rect(scene);
        base.add_region(&core.snapshot_pending_damage());
        let accumulated = core.accumulate_damage_from_age(base, output);
        assert!(accumulated.intersects_rect(scene));
        assert!(accumulated.intersects_rect(pending));
        assert!(accumulated.intersects_rect(history));
        assert!(!accumulated.intersects_rect(rect(70, 70, 10, 10)));
    }
}
