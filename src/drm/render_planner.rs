use crate::{
    region::{DamageRegion, RectI},
    scene::SceneModel,
    state::{DrmId, OutputId, State},
};

// Output capture and any debug view of a physical output must consume this same
// frame plan, including both participants of a live workspace transition. A session
// lock replaces that view with the opaque blank and the lock surface. Retained closing
// capture is window-local and is not an output or screencopy path.
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
    if state.session_lock.consume_full_damage(logical_output_id) {
        let drm = state.drm.get_mut(drm_id);
        if let Some(output) = drm.output_mut(drm_crtc_id) {
            output.pipeline.set_force_full_redraw(true);
        }
    }
    if state.session_lock.blank().is_some() {
        return collect_protected_output_render_plan(
            state,
            drm_id,
            drm_crtc_id,
            logical_output_id,
            global_bounds,
        );
    }
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
    let mut frame = {
        let drm = state.drm.get_mut(drm_id);
        build_frame_plan(
            drm.output_mut(drm_crtc_id)
                .expect("missing DRM output pipeline for frame plan"),
            items,
            software_cursor_items,
            frame_elements,
            cursor_plane,
            false,
        )
    };
    if state.output_has_live_workspace_transition(logical_output_id) {
        let drm = state.drm.get(drm_id);
        if let Some(bounds) = drm.output(drm_crtc_id).map(|output| output.output_bounds()) {
            damage_live_workspace_transition(&mut frame.damage, true, bounds);
        }
    }
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
    output: &mut super::DrmOutput,
    items: Vec<super::SceneItem>,
    software_cursor_items: Vec<super::SceneItem>,
    frame_elements: Vec<super::RecordedFrameElement>,
    cursor_plane: Option<super::CursorPlaneCandidate>,
    protected: bool,
) -> super::FramePlan {
    let output_bounds = output.output_bounds();
    let mut items = items;
    let mut software_cursor_items = software_cursor_items;
    let mut frame_elements = frame_elements;
    super::commit_visible_scene_state(&mut items);
    super::commit_visible_scene_state(&mut software_cursor_items);
    incorporate_software_cursor(
        &mut items,
        &mut software_cursor_items,
        &mut frame_elements,
        cursor_plane.is_some(),
    );
    let (scene, frame_callback_surfaces, presentation_feedbacks) =
        super::collect_submission_metadata(&items, &software_cursor_items);
    let has_effects = frame_elements
        .iter()
        .any(|element| matches!(element, super::RecordedFrameElement::Effect { .. }));
    let mut base_damage = scene_change_damage(
        output_bounds,
        &items,
        output.pipeline.last_scene(),
        has_effects,
        output.pipeline.last_submission_had_effects(),
        output.pipeline.force_full_redraw(),
    );
    // Animation invalidation is snapshotted before buffer-age accumulation so a later
    // queue during this frame stays in the next generation.
    base_damage.add_region(&output.pipeline.snapshot_pending_damage());
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

    let direct_scanout =
        if super::direct_scanout_blocked(output.pending_modeset.is_some(), protected) {
            None
        } else if !has_effects
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
        protected,
    }
}

/// A software cursor is both a scene item and the last overlay texture. A live
/// hardware cursor keeps those items out of the recipe so the cursor is not drawn twice.
pub(super) fn incorporate_software_cursor(
    items: &mut Vec<super::SceneItem>,
    software_cursor_items: &mut Vec<super::SceneItem>,
    frame_elements: &mut Vec<super::RecordedFrameElement>,
    cursor_plane_active: bool,
) {
    if cursor_plane_active {
        return;
    }
    let first_index = items.len();
    let count = software_cursor_items.len();
    frame_elements.extend((0..count).map(|offset| super::RecordedFrameElement::Texture {
        phase: crate::render::FramePhase::Overlay,
        item_index: first_index + offset,
    }));
    items.append(software_cursor_items);
}

fn collect_protected_output_render_plan(
    state: &mut State,
    drm_id: DrmId,
    drm_crtc_id: u32,
    logical_output_id: OutputId,
    global_bounds: RectI,
) -> super::OutputRenderPlan {
    let (mut items, mut frame_elements, deferred_releases) = super::collect_protected_scene(
        state,
        drm_id,
        drm_crtc_id,
        logical_output_id,
        global_bounds,
    );
    super::translate_scene_items(&mut items, -global_bounds.x, -global_bounds.y);
    super::translate_recorded_frame_elements(
        &mut frame_elements,
        -global_bounds.x,
        -global_bounds.y,
    );
    let frame = {
        let drm = state.drm.get_mut(drm_id);
        build_frame_plan(
            drm.output_mut(drm_crtc_id)
                .expect("missing DRM output pipeline for frame plan"),
            items,
            Vec::new(),
            frame_elements,
            None,
            true,
        )
    };
    super::OutputRenderPlan {
        output_id: Some(logical_output_id),
        global_bounds,
        local_origin: (global_bounds.x, global_bounds.y),
        deferred_releases,
        cursor_animation_delay_msec: None,
        frame,
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

pub(super) fn scene_change_damage(
    output_bounds: RectI,
    items: &[super::SceneItem],
    last_scene: &[super::SceneItemState],
    has_effects: bool,
    had_effects_last_frame: bool,
    force_full_redraw: bool,
) -> DamageRegion {
    let mut base_damage = DamageRegion::new();
    let current_order: Vec<_> = items.iter().map(|item| item.state.key).collect();
    let previous_order = scene_order(last_scene);
    if super::effect_transition_requires_full_redraw(has_effects, had_effects_last_frame)
        || !crate::ENABLE_DAMAGE
        || force_full_redraw
        || current_order != previous_order
        || items.len() != last_scene.len()
    {
        base_damage.add_rect(output_bounds);
        return base_damage;
    }
    for (current, previous) in items.iter().zip(last_scene.iter()) {
        if current.state.key != previous.key {
            base_damage.add_rect(output_bounds);
            break;
        }
        if current.state.bounds != previous.bounds
            || current.state.content_key != previous.content_key
            || current.state.tex_transform != previous.tex_transform
            || current.state.alpha != previous.alpha
            || current.state.mask != previous.mask
        {
            base_damage.add_rect(previous.bounds);
            base_damage.add_rect(current.state.bounds);
        } else {
            base_damage.add_region(&current.damage);
        }
    }
    base_damage
}

pub(super) fn damage_live_workspace_transition(
    damage: &mut DamageRegion,
    live: bool,
    output_bounds: RectI,
) {
    if live {
        damage.add_rect(output_bounds);
    }
}

#[cfg(test)]
mod tests {
    use {
        super::*,
        crate::{
            clients::{ClientId, Resolved},
            protocol_generated::{WlBufferId, WlSurfaceId},
            region::RectI,
            state::{ObjectId, RootSurfaceClip},
        },
    };

    fn surface_key(id: u32) -> Resolved<WlSurfaceId> {
        Resolved::new(ClientId::from_raw(1), WlSurfaceId::from(id), ObjectId::from(id as usize))
    }

    fn buffer_key(id: u32) -> Resolved<WlBufferId> {
        Resolved::new(ClientId::from_raw(1), WlBufferId::from(id), ObjectId::from(id as usize))
    }

    fn clip(x: i32, y: i32, width: i32, height: i32, radius: i32) -> RootSurfaceClip {
        RootSurfaceClip::new(
            RectI::new(x, y, width, height).unwrap(),
            radius,
            crate::config::RoundingPower::DEFAULT,
        )
    }

    fn cursor_item(bounds: RectI, fingerprint: u64) -> super::super::SceneItem {
        let mut cursor =
            item(fingerprint as u32, bounds, u8::MAX, super::super::SceneItemMask::None, bounds);
        cursor.state.key = super::super::SceneItemKey::Cursor;
        cursor.state.surface_key = None;
        cursor.state.content_key =
            super::super::SceneItemContentKey::NamedCursor(crate::cursor::CursorImageContentKey {
                width: bounds.width,
                height: bounds.height,
                fingerprint,
            });
        cursor.state_has_alpha = true;
        cursor
    }

    fn item(
        id: u32,
        bounds: RectI,
        alpha: u8,
        mask: super::super::SceneItemMask,
        damage: RectI,
    ) -> super::super::SceneItem {
        super::super::SceneItem {
            state: super::super::SceneItemState {
                key: super::super::SceneItemKey::Surface(surface_key(id)),
                surface_key: Some(surface_key(id)),
                content_key: super::super::SceneItemContentKey::Buffer(buffer_key(id)),
                bounds,
                tex_transform: crate::transform::Mat3::identity(),
                alpha,
                mask,
            },
            mask,
            texture: crate::render::inert_texture(),
            damage: DamageRegion::from_rect(damage),
            sample_damage_padding: 0,
            state_has_alpha: false,
            alpha,
            opaque_region: DamageRegion::new(),
            opaque: false,
            sync: crate::sync::SurfaceSyncState::default(),
            direct_scanout: None,
            presentation_feedbacks: Vec::new(),
        }
    }

    fn output_bounds() -> RectI {
        RectI::new(0, 0, 200, 100).unwrap()
    }

    fn covers_output(damage: &DamageRegion) -> bool {
        damage.intersects_rect(RectI::new(0, 80, 10, 10).unwrap())
    }

    #[test]
    fn alpha_only_change_damages_previous_and_current_bounds() {
        let bounds = RectI::new(10, 10, 40, 30).unwrap();
        let previous = item(
            1,
            bounds,
            255,
            super::super::SceneItemMask::None,
            RectI::new(0, 0, 0, 0).unwrap_or(bounds),
        );
        let mut current = item(1, bounds, 128, super::super::SceneItemMask::None, bounds);
        current.damage = DamageRegion::new();
        let damage = scene_change_damage(
            output_bounds(),
            &[current],
            &[previous.state],
            false,
            false,
            false,
        );
        assert!(damage.intersects_rect(bounds));
        assert!(!covers_output(&damage));
    }

    #[test]
    fn mask_only_change_damages_previous_and_current_bounds() {
        let bounds = RectI::new(10, 10, 40, 30).unwrap();
        let previous = item(
            1,
            bounds,
            255,
            super::super::SceneItemMask::RoundedClip { clip: clip(10, 10, 40, 30, 4) },
            bounds,
        );
        let mut current = item(
            1,
            bounds,
            255,
            super::super::SceneItemMask::RoundedClip { clip: clip(10, 10, 40, 30, 12) },
            bounds,
        );
        current.damage = DamageRegion::new();
        let damage = scene_change_damage(
            output_bounds(),
            &[current],
            &[previous.state],
            false,
            false,
            false,
        );
        assert!(damage.intersects_rect(bounds));
        assert!(!covers_output(&damage));
    }

    #[test]
    fn moved_visual_state_damages_previous_and_current_bounds() {
        let previous_bounds = RectI::new(10, 10, 40, 30).unwrap();
        let current_bounds = RectI::new(80, 10, 40, 30).unwrap();
        let previous =
            item(1, previous_bounds, 255, super::super::SceneItemMask::None, previous_bounds);
        let mut current =
            item(1, current_bounds, 255, super::super::SceneItemMask::None, current_bounds);
        current.damage = DamageRegion::new();
        let damage = scene_change_damage(
            output_bounds(),
            &[current],
            &[previous.state],
            false,
            false,
            false,
        );
        assert!(damage.intersects_rect(previous_bounds));
        assert!(damage.intersects_rect(current_bounds));
        assert!(!covers_output(&damage));
    }

    #[test]
    fn unchanged_visual_state_keeps_client_damage_only() {
        let bounds = RectI::new(10, 10, 40, 30).unwrap();
        let client = RectI::new(12, 12, 4, 4).unwrap();
        let previous = item(1, bounds, 200, super::super::SceneItemMask::None, client);
        let current = item(1, bounds, 200, super::super::SceneItemMask::None, client);
        let damage = scene_change_damage(
            output_bounds(),
            &[current],
            &[previous.state.clone()],
            false,
            false,
            false,
        );
        assert!(damage.intersects_rect(client));
        assert!(!damage.intersects_rect(RectI::new(30, 30, 10, 8).unwrap()));
        assert!(!covers_output(&damage));
    }

    #[test]
    fn effects_and_direct_scanout_transitions_redraw_the_output() {
        let bounds = RectI::new(10, 10, 40, 30).unwrap();
        let previous = item(1, bounds, 255, super::super::SceneItemMask::None, bounds);
        let current = item(1, bounds, 255, super::super::SceneItemMask::None, bounds);
        let effects = scene_change_damage(
            output_bounds(),
            &[current],
            &[previous.state.clone()],
            true,
            false,
            false,
        );
        assert!(covers_output(&effects));
        let scanout = scene_change_damage(
            output_bounds(),
            std::slice::from_ref(&previous),
            &[previous.state.clone()],
            false,
            false,
            true,
        );
        assert!(covers_output(&scanout));
    }

    #[test]
    fn scene_order_change_redraws_the_output() {
        let first_bounds = RectI::new(10, 10, 20, 20).unwrap();
        let second_bounds = RectI::new(40, 10, 20, 20).unwrap();
        let first = item(1, first_bounds, 255, super::super::SceneItemMask::None, first_bounds);
        let second = item(2, second_bounds, 255, super::super::SceneItemMask::None, second_bounds);
        let damage = scene_change_damage(
            output_bounds(),
            &[second, first],
            &[
                item(1, first_bounds, 255, super::super::SceneItemMask::None, first_bounds).state,
                item(2, second_bounds, 255, super::super::SceneItemMask::None, second_bounds).state,
            ],
            false,
            false,
            false,
        );
        assert!(covers_output(&damage));
    }

    #[test]
    fn software_cursor_is_the_final_overlay_once() {
        let window_bounds = RectI::new(0, 0, 200, 100).unwrap();
        let cursor_bounds = RectI::new(12, 18, 24, 24).unwrap();
        let mut items =
            vec![item(1, window_bounds, u8::MAX, super::super::SceneItemMask::None, window_bounds)];
        let mut software_cursor_items = vec![cursor_item(cursor_bounds, 7)];
        let mut frame_elements = vec![super::super::RecordedFrameElement::Texture {
            phase: crate::render::FramePhase::Content,
            item_index: 0,
        }];

        incorporate_software_cursor(
            &mut items,
            &mut software_cursor_items,
            &mut frame_elements,
            false,
        );

        assert!(software_cursor_items.is_empty());
        assert_eq!(items.len(), 2);
        assert!(matches!(items[1].state.key, super::super::SceneItemKey::Cursor));
        assert_eq!(
            frame_elements,
            vec![
                super::super::RecordedFrameElement::Texture {
                    phase: crate::render::FramePhase::Content,
                    item_index: 0,
                },
                super::super::RecordedFrameElement::Texture {
                    phase: crate::render::FramePhase::Overlay,
                    item_index: 1,
                },
            ]
        );
        let overlay_textures = frame_elements
            .iter()
            .filter(|element| {
                matches!(
                    element,
                    super::super::RecordedFrameElement::Texture {
                        phase: crate::render::FramePhase::Overlay,
                        ..
                    }
                )
            })
            .count();
        assert_eq!(overlay_textures, 1);
        let recipe = super::super::build_output_frame_recipe(&items, &frame_elements);
        assert!(matches!(
            recipe.elements.last(),
            Some(crate::render::FrameElement::Texture {
                phase: crate::render::FramePhase::Overlay,
                element,
            }) if element.x == cursor_bounds.x
                && element.y == cursor_bounds.y
                && element.width == cursor_bounds.width
                && element.height == cursor_bounds.height
        ));
    }

    #[test]
    fn hardware_cursor_omits_software_cursor_from_the_recipe() {
        let window_bounds = RectI::new(0, 0, 200, 100).unwrap();
        let mut items =
            vec![item(1, window_bounds, u8::MAX, super::super::SceneItemMask::None, window_bounds)];
        let mut software_cursor_items = vec![cursor_item(RectI::new(12, 18, 24, 24).unwrap(), 7)];
        let mut frame_elements = vec![super::super::RecordedFrameElement::Texture {
            phase: crate::render::FramePhase::Content,
            item_index: 0,
        }];
        let original_elements = frame_elements.clone();

        incorporate_software_cursor(
            &mut items,
            &mut software_cursor_items,
            &mut frame_elements,
            true,
        );

        assert_eq!(items.len(), 1);
        assert_eq!(software_cursor_items.len(), 1);
        assert_eq!(frame_elements, original_elements);
        let recipe = super::super::build_output_frame_recipe(&items, &frame_elements);
        assert!(recipe.elements.iter().all(|element| {
            !matches!(
                element,
                crate::render::FrameElement::Texture {
                    phase: crate::render::FramePhase::Overlay,
                    ..
                }
            )
        }));
    }

    #[test]
    fn retry_fallback_records_the_held_software_cursor_once() {
        let window_bounds = RectI::new(0, 0, 200, 100).unwrap();
        let cursor_bounds = RectI::new(30, 40, 16, 16).unwrap();
        let mut items =
            vec![item(1, window_bounds, u8::MAX, super::super::SceneItemMask::None, window_bounds)];
        let mut software_cursor_items =
            vec![cursor_item(cursor_bounds, 3), cursor_item(cursor_bounds, 4)];
        let mut frame_elements = vec![super::super::RecordedFrameElement::Texture {
            phase: crate::render::FramePhase::Content,
            item_index: 0,
        }];

        incorporate_software_cursor(
            &mut items,
            &mut software_cursor_items,
            &mut frame_elements,
            true,
        );
        assert_eq!(software_cursor_items.len(), 2);
        incorporate_software_cursor(
            &mut items,
            &mut software_cursor_items,
            &mut frame_elements,
            false,
        );
        incorporate_software_cursor(
            &mut items,
            &mut software_cursor_items,
            &mut frame_elements,
            false,
        );

        assert!(software_cursor_items.is_empty());
        assert_eq!(items.len(), 3);
        assert_eq!(
            frame_elements,
            vec![
                super::super::RecordedFrameElement::Texture {
                    phase: crate::render::FramePhase::Content,
                    item_index: 0,
                },
                super::super::RecordedFrameElement::Texture {
                    phase: crate::render::FramePhase::Overlay,
                    item_index: 1,
                },
                super::super::RecordedFrameElement::Texture {
                    phase: crate::render::FramePhase::Overlay,
                    item_index: 2,
                },
            ]
        );
    }

    #[test]
    fn empty_software_cursor_list_leaves_the_recipe_unchanged() {
        let window_bounds = RectI::new(0, 0, 80, 40).unwrap();
        let mut items =
            vec![item(1, window_bounds, u8::MAX, super::super::SceneItemMask::None, window_bounds)];
        let mut software_cursor_items = Vec::new();
        let mut frame_elements = vec![super::super::RecordedFrameElement::Texture {
            phase: crate::render::FramePhase::Content,
            item_index: 0,
        }];
        let original_elements = frame_elements.clone();

        incorporate_software_cursor(
            &mut items,
            &mut software_cursor_items,
            &mut frame_elements,
            false,
        );

        assert_eq!(items.len(), 1);
        assert!(matches!(items[0].state.key, super::super::SceneItemKey::Surface(_)));
        assert_eq!(frame_elements, original_elements);
    }

    #[test]
    fn moved_software_cursor_damages_previous_and_current_bounds() {
        let previous_bounds = RectI::new(10, 10, 24, 24).unwrap();
        let current_bounds = RectI::new(40, 12, 24, 24).unwrap();
        let previous = cursor_item(previous_bounds, 7);
        let mut current = cursor_item(current_bounds, 7);
        current.damage = DamageRegion::new();
        let damage = scene_change_damage(
            output_bounds(),
            &[current],
            &[previous.state],
            false,
            false,
            false,
        );
        assert!(damage.intersects_rect(previous_bounds));
        assert!(damage.intersects_rect(current_bounds));
        assert!(!covers_output(&damage));
    }

    #[test]
    fn live_workspace_transition_covers_the_output() {
        let mut damage = DamageRegion::from_rect(RectI::new(10, 10, 4, 4).unwrap());
        damage_live_workspace_transition(&mut damage, true, output_bounds());
        assert!(covers_output(&damage));
        let mut steady = DamageRegion::from_rect(RectI::new(10, 10, 4, 4).unwrap());
        damage_live_workspace_transition(&mut steady, false, output_bounds());
        assert!(!covers_output(&steady));
    }
}
