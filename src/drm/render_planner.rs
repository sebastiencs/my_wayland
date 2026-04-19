use crate::{
    scene::SceneModel,
    state::{DrmId, OutputId, State},
};

pub(super) fn collect_output_render_plan(
    state: &mut State,
    drm_id: DrmId,
    logical_output_id: Option<OutputId>,
    frame_time_msec: u32,
    scene: &SceneModel,
) -> super::OutputRenderPlan {
    let logical_output_id = logical_output_id.expect("missing logical output for DRM render plan");
    let drm_crtc_id = state
        .output_crtc_id(logical_output_id)
        .expect("missing DRM CRTC for logical output");
    let global_bounds = state.output_bounds(logical_output_id).unwrap_or_else(|| {
        state
            .drm
            .get(drm_id)
            .output(drm_crtc_id)
            .unwrap()
            .output_bounds()
    });
    let view = state.compose_output_view(scene, logical_output_id, None);
    let (
        mut items,
        mut software_cursor_items,
        mut frame_elements,
        deferred_releases,
        cursor_plane,
        cursor_animation_delay_msec,
    ) = super::collect_render_items_for_output_view(
        state,
        drm_id,
        drm_crtc_id,
        logical_output_id,
        global_bounds,
        frame_time_msec,
        &view,
    );
    super::translate_scene_items(&mut items, -global_bounds.x, -global_bounds.y);
    super::translate_scene_items(&mut software_cursor_items, -global_bounds.x, -global_bounds.y);
    super::translate_recorded_frame_elements(
        &mut frame_elements,
        -global_bounds.x,
        -global_bounds.y,
    );
    let frame = {
        let drm = state.drm.get(drm_id);
        build_frame_plan(
            drm.output(drm_crtc_id)
                .expect("missing DRM output pipeline for frame plan"),
            items,
            software_cursor_items,
            frame_elements,
            cursor_plane,
        )
    };
    let output_id = Some(logical_output_id)
        .or_else(|| select_presentation_output_for_scene(state, drm_id, &frame.scene));
    super::OutputRenderPlan {
        output_id,
        global_bounds,
        local_origin: (global_bounds.x, global_bounds.y),
        deferred_releases,
        cursor_animation_delay_msec,
        frame,
    }
}

pub(super) fn build_frame_plan(
    output: &super::DrmOutput,
    items: Vec<super::SceneItem>,
    software_cursor_items: Vec<super::SceneItem>,
    frame_elements: Vec<super::RecordedFrameElement>,
    cursor_plane: Option<super::CursorPlaneCandidate>,
) -> super::FramePlan {
    let output_bounds = output.output_bounds();
    let mut base_damage = crate::region::DamageRegion::new();
    let mut items = items;
    let mut software_cursor_items = software_cursor_items;
    if cursor_plane.is_none() {
        items.append(&mut software_cursor_items);
    }
    let (scene, frame_callback_surfaces, presentation_feedbacks) =
        super::collect_submission_metadata(&items, &software_cursor_items);
    let last_scene = output.pipeline.last_scene();
    let order_changed = scene_order(last_scene) != scene_order(&scene);
    let has_effects = frame_elements
        .iter()
        .any(|element| matches!(element, super::RecordedFrameElement::Effect { .. }));

    let had_effects_last_frame = output.pipeline.last_submission_had_effects();
    if super::effect_transition_requires_full_redraw(has_effects, had_effects_last_frame)
        || !crate::ENABLE_DAMAGE
        || output.pipeline.force_full_redraw()
        || order_changed
        || scene.len() != last_scene.len()
    {
        base_damage.add_rect(output_bounds);
    } else {
        for (current, previous) in items.iter().zip(last_scene.iter()) {
            if current.state.key != previous.key {
                base_damage.add_rect(output_bounds);
                break;
            }
            if current.state.bounds != previous.bounds
                || current.state.content_key != previous.content_key
                || current.state.tex_transform != previous.tex_transform
            {
                base_damage.add_rect(previous.bounds);
                base_damage.add_rect(current.state.bounds);
            } else {
                base_damage.add_region(&current.damage);
            }
        }
    }

    base_damage = output.accumulate_damage_from_age(&base_damage);
    base_damage.intersect_rect(output_bounds);
    if !presentation_feedbacks.is_empty()
        && cursor_plane.is_none()
        && !scene.is_empty()
        && base_damage.is_empty()
    {
        base_damage.add_rect(output_bounds);
    }
    for item in &items {
        if item.sample_damage_padding <= 0 {
            continue;
        }
        let mut padded_item_damage = base_damage.clone();
        padded_item_damage.intersect_rect(item.state.bounds);
        if padded_item_damage.is_empty() {
            continue;
        }
        padded_item_damage.expand(item.sample_damage_padding);
        padded_item_damage.intersect_rect(item.state.bounds);
        base_damage.add_region(&padded_item_damage);
    }

    if has_effects {
        for item in &mut items {
            let mut visible_damage = base_damage.clone();
            visible_damage.intersect_rect(item.state.bounds);
            item.damage = visible_damage;
        }
    } else {
        let mut occluded = crate::region::DamageRegion::new();
        for item in items.iter_mut().rev() {
            let mut visible_damage = base_damage.clone();
            visible_damage.intersect_rect(item.state.bounds);
            visible_damage.subtract_region(&occluded);
            item.damage = visible_damage;
            if item.opaque {
                occluded.add_region(&item.opaque_region);
            }
        }
    }

    let direct_scanout = if !has_effects
        && items.len() == 1
        && items[0].state.bounds == output_bounds
        && !super::scene_item_requires_compositor_compositing(&items[0])
        && !output.pipeline.is_waiting_for_flip()
    {
        let bounds = items[0].state.bounds;
        let sync = items[0].sync.clone();
        items[0]
            .direct_scanout
            .take()
            .map(|framebuffer| super::DirectScanoutCandidate { bounds, sync, framebuffer })
    } else {
        None
    };

    super::FramePlan {
        items,
        software_cursor_items,
        frame_elements,
        scene,
        frame_callback_surfaces,
        presentation_feedbacks,
        damage: base_damage,
        direct_scanout,
        cursor_plane,
        preserve_contents: crate::ENABLE_DAMAGE
            && output.preserve_render_buffer_contents()
            && !output.pipeline.force_full_redraw()
            && !has_effects,
    }
}

fn output_overlap_area(scene: &[super::SceneItemState], output_rect: crate::region::RectI) -> i64 {
    scene
        .iter()
        .map(|item| {
            let x1 = item.bounds.x.max(output_rect.x);
            let y1 = item.bounds.y.max(output_rect.y);
            let x2 = item.bounds.x2().min(output_rect.x2());
            let y2 = item.bounds.y2().min(output_rect.y2());
            let width = (x2 - x1).max(0) as i64;
            let height = (y2 - y1).max(0) as i64;
            width * height
        })
        .sum()
}

pub(super) fn select_presentation_output_for_scene(
    state: &State,
    drm_id: DrmId,
    scene: &[super::SceneItemState],
) -> Option<OutputId> {
    let backend = crate::state::BackendOutputRef::new(drm_id);
    let layout = state.output_layout();
    let output_ids = state.output_ids_for_backend(backend);
    let preferred_output = state.active_output_id().filter(|output_id| {
        layout
            .entry(*output_id)
            .is_some_and(|entry| entry.backend == backend)
    });
    let mut best = None;
    let mut best_area = -1i64;
    for output_id in output_ids {
        let Some(output_rect) = layout.bounds(output_id) else {
            continue;
        };
        let area = output_overlap_area(scene, output_rect);
        let choose = if area > best_area {
            true
        } else if area == best_area {
            let best_is_preferred = best == preferred_output;
            let current_is_preferred = Some(output_id) == preferred_output;
            if current_is_preferred != best_is_preferred {
                current_is_preferred
            } else {
                best.is_none_or(|best_output| output_id < best_output)
            }
        } else {
            false
        };
        if choose {
            best_area = area;
            best = Some(output_id);
        }
    }
    best.or(preferred_output)
}

fn scene_order(scene: &[super::SceneItemState]) -> Vec<super::SceneItemKey> {
    scene.iter().map(|item| item.key).collect()
}
