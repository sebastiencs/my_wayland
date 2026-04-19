use crate::{
    clients::Resolved,
    protocol_generated::WlSurfaceId,
    state::{State, WorkspaceId},
    wayland_objects::zwlr_layer_surface_v1::LayerLayoutObjectSnapshot,
};

use std::collections::BTreeSet;

use super::{compose_output, model::SceneRole, SceneModel};

pub struct WorkspaceFocusOrder {
    pub roots_front_to_back: Vec<Resolved<WlSurfaceId>>,
}

pub fn build_workspace_focus_order(
    state: &State,
    scene: &SceneModel,
    workspace_id: WorkspaceId,
    snapshot: Option<LayerLayoutObjectSnapshot<'_>>,
) -> WorkspaceFocusOrder {
    let output_set = state
        .workspace_output_ids(workspace_id)
        .into_iter()
        .collect::<BTreeSet<_>>();
    let mut seen = BTreeSet::new();
    let mut roots_front_to_back = Vec::new();

    for output_id in state.outputs.iter().map(|(output_id, _)| output_id) {
        if !output_set.contains(&output_id) {
            continue;
        }
        let view = compose_output(state, scene, output_id, snapshot);
        for entry in view.entries {
            if entry.tier != super::CompositionTier::Content {
                continue;
            }
            let Some(scene_entry) = scene
                .entries
                .iter()
                .find(|candidate| candidate.id == entry.scene_entry_id)
            else {
                continue;
            };
            if scene_entry.role != SceneRole::WorkspaceRoot
                || scene_entry.workspace_id != Some(workspace_id)
            {
                continue;
            }
            if seen.insert(entry.root_surface) {
                roots_front_to_back.push(entry.root_surface);
            }
        }
    }

    WorkspaceFocusOrder { roots_front_to_back }
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

    use super::build_workspace_focus_order;

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
    fn workspace_focus_order_merges_relevant_outputs_in_state_order() {
        let mut state = state_for_tests();
        let backend = BackendOutputRef::new(DrmId::from(0usize));
        let left = state.insert_output(test_output_with_crtc_name(backend, 1, 0, 0, 100, 100));
        let right = state.insert_output(test_output_with_crtc_name(backend, 2, 100, 0, 100, 100));
        let left_group = state.workspace_group_for_output(left).unwrap();
        let right_group = state.workspace_group_for_output(right).unwrap();
        let workspace_id = state.active_workspace_for_output(left).unwrap();

        state.output_workspace_groups.insert(right, left_group);
        state
            .workspace_groups
            .try_get_mut(left_group)
            .unwrap()
            .outputs
            .insert(right);
        state
            .workspace_groups
            .try_get_mut(right_group)
            .unwrap()
            .outputs
            .remove(&right);

        let first = insert_root(&mut state, 31);
        let second = insert_root(&mut state, 32);
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
        let order = build_workspace_focus_order(&state, &scene, workspace_id, None);
        assert_eq!(order.roots_front_to_back, vec![first, second]);
    }
}
