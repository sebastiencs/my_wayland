use crate::{
    clients::Resolved,
    protocol_generated::WlSurfaceId,
    region::RectI,
    state::{OutputId, State},
    wayland_objects::zwlr_layer_surface_v1::{
        LayerKeyboardFocus, LayerLayoutObjectSnapshot, SceneLayerPlacement,
    },
};

use super::{
    compose_output,
    model::{SceneEntry, SceneRole},
};

pub struct CompositionEvalContext<'a> {
    pub state: &'a State,
    pub output_id: OutputId,
    pub snapshot: Option<LayerLayoutObjectSnapshot<'a>>,
}

#[derive(Clone, Copy)]
pub enum CompositionSource<'a> {
    SceneEntry(&'a SceneEntry),
    LayerRoot { root_surface: Resolved<WlSurfaceId>, placement: &'a SceneLayerPlacement },
}

pub fn render_participates(
    ctx: &CompositionEvalContext<'_>,
    source: CompositionSource<'_>,
) -> bool {
    match source {
        CompositionSource::SceneEntry(entry) => {
            if !root_is_globally_visible(ctx.state, entry.root_surface) {
                return false;
            }
            if !scene_entry_belongs_to_output(ctx.state, entry, ctx.output_id) {
                return false;
            }
            root_bounds_for_output_membership(ctx.state, entry.root_surface)
                .map(|bounds| bounds.intersects(output_bounds(ctx.state, ctx.output_id)))
                .unwrap_or(true)
        }
        CompositionSource::LayerRoot { placement, .. } => placement.output_id == ctx.output_id,
    }
}

pub fn protocol_visible(ctx: &CompositionEvalContext<'_>, source: CompositionSource<'_>) -> bool {
    match source {
        CompositionSource::SceneEntry(entry) => {
            root_is_globally_visible(ctx.state, entry.root_surface)
                && scene_entry_belongs_to_output(ctx.state, entry, ctx.output_id)
        }
        CompositionSource::LayerRoot { placement, .. } => placement.output_id == ctx.output_id,
    }
}

pub fn input_participates(ctx: &CompositionEvalContext<'_>, source: CompositionSource<'_>) -> bool {
    match source {
        CompositionSource::SceneEntry(entry) => {
            if !root_is_globally_visible(ctx.state, entry.root_surface) {
                return false;
            }
            if !scene_entry_belongs_to_output(ctx.state, entry, ctx.output_id) {
                return false;
            }
            match entry.role {
                SceneRole::WorkspaceRoot => ctx.state.root_surface_is_mapped(entry.root_surface),
                SceneRole::UnmanagedRoot => true,
            }
        }
        CompositionSource::LayerRoot { root_surface, placement } => {
            if placement.output_id != ctx.output_id {
                return false;
            }
            !matches!(
                crate::wayland_objects::zwlr_layer_surface_v1::layer_surface_keyboard_focus_with_snapshot(
                    ctx.state,
                    root_surface,
                    ctx.snapshot,
                ),
                Some(LayerKeyboardFocus::None)
            )
        }
    }
}

pub fn composition_allows_direct_scanout(
    ctx: &CompositionEvalContext<'_>,
    source: CompositionSource<'_>,
) -> bool {
    match source {
        CompositionSource::SceneEntry(_) => render_participates(ctx, source),
        CompositionSource::LayerRoot { .. } => false,
    }
}

pub fn root_is_globally_visible(state: &State, root_surface: Resolved<WlSurfaceId>) -> bool {
    state
        .root_surface_workspace(root_surface)
        .map(|workspace_id| {
            if !state.workspace_is_active(workspace_id) {
                return false;
            }
            let Some(workspace) = state.workspaces.try_get(workspace_id) else {
                return false;
            };
            workspace
                .layout
                .fullscreen
                .as_ref()
                .filter(|fullscreen| state.root_surface_is_mapped(fullscreen.surface_object_id))
                .map(|fullscreen| fullscreen.surface_object_id == root_surface)
                .unwrap_or(true)
        })
        .unwrap_or(true)
}

pub fn root_has_protocol_visibility(state: &State, root_surface: Resolved<WlSurfaceId>) -> bool {
    let scene = super::build_scene_model(state);
    state
        .outputs
        .iter()
        .map(|(output_id, _)| output_id)
        .filter(|output_id| state.output_bounds(*output_id).is_some())
        .map(|output_id| compose_output(state, &scene, output_id, None))
        .any(|view| {
            view.entries.iter().any(|entry| {
                entry.root_surface == root_surface && entry.participates_in_protocol_visibility
            })
        })
}

pub fn root_bounds_for_output_membership(
    state: &State,
    root_surface: Resolved<WlSurfaceId>,
) -> Option<RectI> {
    state.root_surface_surface_bounds_global(root_surface)
}

pub fn scene_entry_belongs_to_output(
    state: &State,
    entry: &SceneEntry,
    output_id: OutputId,
) -> bool {
    match entry.role {
        SceneRole::WorkspaceRoot => {
            state.active_workspace_for_output(output_id) == entry.workspace_id
        }
        SceneRole::UnmanagedRoot => root_bounds_for_output_membership(state, entry.root_surface)
            .map(|bounds| bounds.intersects(output_bounds(state, output_id)))
            .unwrap_or(true),
    }
}

fn output_bounds(state: &State, output_id: OutputId) -> RectI {
    state
        .output_bounds(output_id)
        .expect("scene visibility requires an existing output")
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

    use super::{
        root_has_protocol_visibility, root_is_globally_visible, scene_entry_belongs_to_output,
    };

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
    fn root_without_workspace_stays_globally_visible() {
        let mut state = state_for_tests();
        let root = insert_root(&mut state, 71);
        assert!(root_is_globally_visible(&state, root));
    }

    #[test]
    fn workspace_root_visibility_tracks_active_workspace() {
        let mut state = state_for_tests();
        let backend = BackendOutputRef::new(DrmId::from(0usize));
        let output_id = state.insert_output(test_output_with_crtc_name(backend, 1, 0, 0, 100, 100));
        let group_id = state.workspace_group_for_output(output_id).unwrap();
        let inactive_workspace = state.create_workspace(Some(group_id), Some("2".to_string()));
        let root = insert_root(&mut state, 72);
        assert!(state.assign_root_surface_to_workspace(root, inactive_workspace));

        assert!(!root_is_globally_visible(&state, root));
        assert!(state.activate_workspace(inactive_workspace));
        assert!(root_is_globally_visible(&state, root));
    }

    #[test]
    fn geometryless_unmanaged_root_participates_without_single_output_ownership() {
        let mut state = state_for_tests();
        let backend = BackendOutputRef::new(DrmId::from(0usize));
        let left = state.insert_output(test_output_with_crtc_name(backend, 1, 0, 0, 100, 100));
        let right = state.insert_output(test_output_with_crtc_name(backend, 2, 100, 0, 100, 100));
        let root = insert_root(&mut state, 73);
        let entry = crate::scene::SceneEntry {
            id: 1,
            root_surface: root,
            role: crate::scene::SceneRole::UnmanagedRoot,
            workspace_id: None,
        };

        assert!(scene_entry_belongs_to_output(&state, &entry, left));
        assert!(scene_entry_belongs_to_output(&state, &entry, right));
    }

    #[test]
    fn protocol_visibility_helper_tracks_workspace_activation() {
        let mut state = state_for_tests();
        let backend = BackendOutputRef::new(DrmId::from(0usize));
        let output_id = state.insert_output(test_output_with_crtc_name(backend, 1, 0, 0, 100, 100));
        let group_id = state.workspace_group_for_output(output_id).unwrap();
        let inactive_workspace = state.create_workspace(Some(group_id), Some("2".to_string()));
        let root = insert_root(&mut state, 74);
        assert!(state.assign_root_surface_to_workspace(root, inactive_workspace));

        assert!(!root_has_protocol_visibility(&state, root));
        assert!(state.activate_workspace(inactive_workspace));
        assert!(root_has_protocol_visibility(&state, root));
    }
}
