use std::collections::BTreeSet;

use crate::{
    clients::Resolved,
    protocol_generated::WlSurfaceId,
    region::RectI,
    state::{OutputId, State},
    wayland_objects::zwlr_layer_surface_v1::{
        scene_layer_layout, LayerLayoutObjectSnapshot, SceneLayerLayout,
    },
};

use super::{
    model::{SceneEntryId, SceneModel},
    visibility::{
        composition_allows_direct_scanout, input_participates, protocol_visible,
        render_participates, root_bounds_for_output_membership, root_is_globally_visible,
        scene_entry_belongs_to_output, CompositionEvalContext, CompositionSource,
    },
};

const LAYER_ENTRY_ID_BASE: u64 = 1 << 63;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CompositionTier {
    Background,
    Bottom,
    Content,
    Top,
    Overlay,
}

#[derive(Clone, Debug)]
pub struct ComposedEntry {
    pub scene_entry_id: SceneEntryId,
    pub root_surface: Resolved<WlSurfaceId>,
    pub tier: CompositionTier,
    pub output_id: OutputId,
    pub global_bounds: Option<RectI>,
    pub participates_in_input: bool,
    pub participates_in_protocol_visibility: bool,
    pub participates_in_render: bool,
    pub allow_direct_scanout: bool,
}

#[derive(Clone, Debug)]
pub struct OutputCompositionView {
    pub output_id: OutputId,
    pub usable_bounds: RectI,
    pub entries: Vec<ComposedEntry>,
}

pub fn compose_output(
    state: &State,
    scene: &SceneModel,
    output_id: OutputId,
    snapshot: Option<LayerLayoutObjectSnapshot<'_>>,
) -> OutputCompositionView {
    let layer_layout = scene_layer_layout(state, snapshot);
    let usable_bounds = layer_layout
        .usable_by_output
        .get(&output_id)
        .copied()
        .or_else(|| state.output_bounds(output_id))
        .expect("compose_output requires an existing output");
    let ctx = CompositionEvalContext { state, output_id, snapshot };
    let mut background_entries = Vec::new();
    let mut bottom_entries = Vec::new();
    let mut content_entries = Vec::new();
    let mut top_entries = Vec::new();
    let mut overlay_entries = Vec::new();

    extend_layer_bucket(
        &mut background_entries,
        &ctx,
        &layer_layout,
        &layer_layout.background_roots,
        CompositionTier::Background,
    );
    extend_layer_bucket(
        &mut bottom_entries,
        &ctx,
        &layer_layout,
        &layer_layout.bottom_roots,
        CompositionTier::Bottom,
    );
    extend_layer_bucket(
        &mut top_entries,
        &ctx,
        &layer_layout,
        &layer_layout.top_roots,
        CompositionTier::Top,
    );
    extend_layer_bucket(
        &mut overlay_entries,
        &ctx,
        &layer_layout,
        &layer_layout.overlay_roots,
        CompositionTier::Overlay,
    );

    for entry in &scene.entries {
        if !scene_entry_belongs_to_output(state, entry, output_id) {
            continue;
        }
        let source = CompositionSource::SceneEntry(entry);
        content_entries.push(ComposedEntry {
            scene_entry_id: entry.id,
            root_surface: entry.root_surface,
            tier: CompositionTier::Content,
            output_id,
            global_bounds: root_bounds_for_output_membership(state, entry.root_surface),
            participates_in_input: input_participates(&ctx, source),
            participates_in_protocol_visibility: protocol_visible(&ctx, source),
            participates_in_render: render_participates(&ctx, source),
            allow_direct_scanout: composition_allows_direct_scanout(&ctx, source),
        });
    }

    let mut entries = Vec::new();
    entries.extend(background_entries);
    entries.extend(bottom_entries);
    entries.extend(content_entries);
    entries.extend(top_entries);
    entries.extend(overlay_entries);

    OutputCompositionView { output_id, usable_bounds, entries }
}

pub fn compose_global_root_order(
    state: &State,
    scene: &SceneModel,
    snapshot: Option<LayerLayoutObjectSnapshot<'_>>,
) -> Vec<Resolved<WlSurfaceId>> {
    let layout = scene_layer_layout(state, snapshot);
    let mut ordered = Vec::new();
    let mut seen = BTreeSet::new();

    for root_surface in layout.background_roots {
        if seen.insert(root_surface) {
            ordered.push(root_surface);
        }
    }
    for root_surface in layout.bottom_roots {
        if seen.insert(root_surface) {
            ordered.push(root_surface);
        }
    }
    for entry in &scene.entries {
        if !root_is_globally_visible(state, entry.root_surface) {
            continue;
        }
        if seen.insert(entry.root_surface) {
            ordered.push(entry.root_surface);
        }
    }
    for root_surface in layout.top_roots {
        if seen.insert(root_surface) {
            ordered.push(root_surface);
        }
    }
    for root_surface in layout.overlay_roots {
        if seen.insert(root_surface) {
            ordered.push(root_surface);
        }
    }

    ordered
}

pub fn composed_roots_front_to_back(view: &OutputCompositionView) -> &[ComposedEntry] {
    &view.entries
}

pub fn composed_roots_back_to_front(
    view: &OutputCompositionView,
) -> impl DoubleEndedIterator<Item = &ComposedEntry> {
    view.entries.iter().rev()
}

fn extend_layer_bucket(
    entries: &mut Vec<ComposedEntry>,
    ctx: &CompositionEvalContext<'_>,
    layout: &SceneLayerLayout,
    roots: &[Resolved<WlSurfaceId>],
    tier: CompositionTier,
) {
    for &root_surface in roots {
        let Some(placement) = layout.placements_by_root.get(&root_surface) else {
            continue;
        };
        if placement.output_id != ctx.output_id {
            continue;
        }
        let source = CompositionSource::LayerRoot { root_surface, placement };
        entries.push(ComposedEntry {
            scene_entry_id: layer_scene_entry_id(root_surface),
            root_surface,
            tier,
            output_id: ctx.output_id,
            global_bounds: Some(placement.rect),
            participates_in_input: input_participates(ctx, source),
            participates_in_protocol_visibility: protocol_visible(ctx, source),
            participates_in_render: render_participates(ctx, source),
            allow_direct_scanout: composition_allows_direct_scanout(ctx, source),
        });
    }
}

fn layer_scene_entry_id(root_surface: Resolved<WlSurfaceId>) -> SceneEntryId {
    LAYER_ENTRY_ID_BASE | root_surface.object_id().as_u64()
}

#[cfg(test)]
mod tests {
    use crate::{
        clients::ClientId,
        protocol_generated::WlSurfaceId,
        state::{BackendOutputRef, DrmId},
        test_support::{state_for_tests, test_output_with_crtc_name},
        wayland_objects::wl_surface::WlSurface,
    };

    use super::{compose_global_root_order, compose_output};

    fn insert_root(
        state: &mut crate::state::State,
        raw: u32,
    ) -> crate::clients::Resolved<WlSurfaceId> {
        let (_, oid) =
            state.insert_object(WlSurface::new(WlSurfaceId::from(raw), ClientId::from_raw(1), 6));
        let surface =
            crate::clients::Resolved::new(ClientId::from_raw(1), WlSurfaceId::from(raw), oid);
        state.register_root_surface(surface);
        surface
    }

    #[test]
    fn compose_global_root_order_uses_scene_model_content_order() {
        let mut state = state_for_tests();
        let backend = BackendOutputRef::new(DrmId::from(0usize));
        let output_id = state.insert_output(test_output_with_crtc_name(backend, 1, 0, 0, 100, 100));
        let workspace_id = state.active_workspace_for_output(output_id).unwrap();
        let first = insert_root(&mut state, 41);
        let second = insert_root(&mut state, 42);
        let unmanaged = insert_root(&mut state, 43);
        let _ = state.assign_root_surface_to_workspace(first, workspace_id);
        let _ = state.assign_root_surface_to_workspace(second, workspace_id);
        state
            .workspaces
            .try_get_mut(workspace_id)
            .unwrap()
            .layout
            .floating
            .stack = vec![first, second];

        let scene = crate::scene::build_scene_model(&state);
        let ordered = compose_global_root_order(&state, &scene, None);
        assert_eq!(ordered, vec![first, second, unmanaged]);
    }

    #[test]
    fn unmanaged_root_participates_on_each_intersecting_output() {
        let mut state = state_for_tests();
        let backend = BackendOutputRef::new(DrmId::from(0usize));
        let left = state.insert_output(test_output_with_crtc_name(backend, 1, 0, 0, 100, 100));
        let right = state.insert_output(test_output_with_crtc_name(backend, 2, 100, 0, 100, 100));
        let unmanaged = insert_root(&mut state, 51);
        state
            .objects
            .get_mut(unmanaged.object_id())
            .downcast_mut::<WlSurface>()
            .unwrap()
            .test_map(crate::state::ObjectId::from(500usize), 50, 10, 100, 20);

        let scene = crate::scene::build_scene_model(&state);
        let left_view = compose_output(&state, &scene, left, None);
        let right_view = compose_output(&state, &scene, right, None);

        assert!(left_view
            .entries
            .iter()
            .any(|entry| entry.root_surface == unmanaged && entry.participates_in_render));
        assert!(right_view
            .entries
            .iter()
            .any(|entry| entry.root_surface == unmanaged && entry.participates_in_render));
    }

    #[test]
    fn geometryless_unmanaged_root_stays_in_output_views_with_no_global_bounds() {
        let mut state = state_for_tests();
        let backend = BackendOutputRef::new(DrmId::from(0usize));
        let left = state.insert_output(test_output_with_crtc_name(backend, 1, 0, 0, 100, 100));
        let right = state.insert_output(test_output_with_crtc_name(backend, 2, 100, 0, 100, 100));
        let unmanaged = insert_root(&mut state, 53);

        let scene = crate::scene::build_scene_model(&state);
        let left_view = compose_output(&state, &scene, left, None);
        let right_view = compose_output(&state, &scene, right, None);

        let left_entry = left_view
            .entries
            .iter()
            .find(|entry| entry.root_surface == unmanaged)
            .expect("left output should keep geometry-less unmanaged root");
        let right_entry = right_view
            .entries
            .iter()
            .find(|entry| entry.root_surface == unmanaged)
            .expect("right output should keep geometry-less unmanaged root");

        assert_eq!(left_entry.global_bounds, None);
        assert_eq!(right_entry.global_bounds, None);
        assert!(left_entry.participates_in_protocol_visibility);
        assert!(right_entry.participates_in_protocol_visibility);
    }

    #[test]
    fn fullscreen_suppresses_non_fullscreen_workspace_roots() {
        let mut state = state_for_tests();
        let backend = BackendOutputRef::new(DrmId::from(0usize));
        let output_id = state.insert_output(test_output_with_crtc_name(backend, 1, 0, 0, 100, 100));
        let workspace_id = state.active_workspace_for_output(output_id).unwrap();
        let fullscreen = insert_root(&mut state, 61);
        let other = insert_root(&mut state, 62);
        let _ = state.assign_root_surface_to_workspace(fullscreen, workspace_id);
        let _ = state.assign_root_surface_to_workspace(other, workspace_id);
        state
            .workspaces
            .try_get_mut(workspace_id)
            .unwrap()
            .layout
            .floating
            .stack = vec![fullscreen, other];
        state
            .workspaces
            .try_get_mut(workspace_id)
            .unwrap()
            .layout
            .fullscreen = Some(crate::tiling::FullscreenState { surface_object_id: fullscreen });
        state
            .managed_root_surfaces
            .get_mut(&fullscreen)
            .unwrap()
            .mapped = true;
        state.managed_root_surfaces.get_mut(&other).unwrap().mapped = true;
        state
            .objects
            .get_mut(fullscreen.object_id())
            .downcast_mut::<WlSurface>()
            .unwrap()
            .test_map(crate::state::ObjectId::from(601usize), 0, 0, 100, 100);
        state
            .objects
            .get_mut(other.object_id())
            .downcast_mut::<WlSurface>()
            .unwrap()
            .test_map(crate::state::ObjectId::from(602usize), 0, 0, 100, 100);

        let scene = crate::scene::build_scene_model(&state);
        let view = compose_output(&state, &scene, output_id, None);

        assert!(view
            .entries
            .iter()
            .any(|entry| entry.root_surface == fullscreen && entry.participates_in_render));
        assert!(view
            .entries
            .iter()
            .any(|entry| entry.root_surface == other && !entry.participates_in_render));
    }
}
