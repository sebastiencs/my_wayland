use crate::{region::DamageRegion, state::OutputId};

use super::{
    CursorPlaneCandidate, DirectScanoutCandidate, Drm, SceneItemState,
    SubmissionPresentationFeedback, SubmittedFrameRegistration, SurfaceKey,
};

pub(super) fn submit_render_buffer(
    drm: &mut Drm,
    drm_crtc_id: u32,
    render_completion: &crate::egl::RenderCompletion,
    damage: &DamageRegion,
    output_id: Option<OutputId>,
    scene: Vec<SceneItemState>,
    frame_callback_surfaces: Vec<SurfaceKey>,
    presentation_feedbacks: Vec<SubmissionPresentationFeedback>,
    cursor_plane: Option<CursorPlaneCandidate>,
    had_effects: bool,
    scene_build_duration: std::time::Duration,
    render_started_at: std::time::Instant,
    protected: bool,
) -> std::io::Result<SubmittedFrameRegistration> {
    drm.submit_render_buffer(
        drm_crtc_id,
        render_completion,
        damage,
        output_id,
        scene,
        frame_callback_surfaces,
        presentation_feedbacks,
        cursor_plane,
        had_effects,
        scene_build_duration,
        render_started_at,
        protected,
    )
}

pub(super) fn submit_direct_scanout(
    drm: &mut Drm,
    drm_crtc_id: u32,
    candidate: DirectScanoutCandidate,
    output_id: Option<OutputId>,
    scene: Vec<SceneItemState>,
    frame_callback_surfaces: Vec<SurfaceKey>,
    presentation_feedbacks: Vec<SubmissionPresentationFeedback>,
    cursor_plane: Option<CursorPlaneCandidate>,
    scene_build_duration: std::time::Duration,
    render_started_at: std::time::Instant,
) -> std::io::Result<SubmittedFrameRegistration> {
    drm.submit_direct_scanout(
        drm_crtc_id,
        candidate,
        output_id,
        scene,
        frame_callback_surfaces,
        presentation_feedbacks,
        cursor_plane,
        scene_build_duration,
        render_started_at,
    )
}
