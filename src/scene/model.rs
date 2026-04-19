use std::collections::BTreeSet;

use crate::{
    clients::Resolved,
    protocol_generated::WlSurfaceId,
    state::{State, WorkspaceId},
};

pub type SceneEntryId = u64;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum SceneRole {
    WorkspaceRoot,
    UnmanagedRoot,
}

#[derive(Clone, Debug)]
pub struct SceneEntry {
    pub id: SceneEntryId,
    pub root_surface: Resolved<WlSurfaceId>,
    pub role: SceneRole,
    pub workspace_id: Option<WorkspaceId>,
}

#[derive(Clone, Debug, Default)]
pub struct SceneModel {
    pub entries: Vec<SceneEntry>,
}

pub fn build_scene_model(state: &State) -> SceneModel {
    let mut entries = Vec::new();
    let mut next_id = 1;
    collect_workspace_scene_entries(state, &mut entries, &mut next_id);
    let managed_roots = entries
        .iter()
        .filter(|entry| entry.role == SceneRole::WorkspaceRoot)
        .map(|entry| entry.root_surface)
        .collect::<BTreeSet<_>>();
    collect_unmanaged_scene_entries(state, &managed_roots, &mut entries, &mut next_id);
    SceneModel { entries }
}

fn collect_workspace_scene_entries(
    state: &State,
    entries: &mut Vec<SceneEntry>,
    next_id: &mut SceneEntryId,
) {
    let mut seen = BTreeSet::new();
    for output_id in state.outputs.iter().map(|(output_id, _)| output_id) {
        let Some(workspace_id) = state.active_workspace_for_output(output_id) else {
            continue;
        };
        let Some(workspace) = state.workspaces.try_get(workspace_id) else {
            continue;
        };
        if let Some(fullscreen) = workspace
            .layout
            .fullscreen
            .as_ref()
            .filter(|fullscreen| state.root_surface_is_mapped(fullscreen.surface_object_id))
        {
            if seen.insert(fullscreen.surface_object_id) {
                entries.push(SceneEntry {
                    id: *next_id,
                    root_surface: fullscreen.surface_object_id,
                    role: SceneRole::WorkspaceRoot,
                    workspace_id: Some(workspace_id),
                });
                *next_id += 1;
            }
            continue;
        }
        for root_surface in workspace.layout.tiled_scene_order() {
            if seen.insert(root_surface) {
                entries.push(SceneEntry {
                    id: *next_id,
                    root_surface,
                    role: SceneRole::WorkspaceRoot,
                    workspace_id: Some(workspace_id),
                });
                *next_id += 1;
            }
        }
        for root_surface in workspace.layout.floating_scene_order() {
            if seen.insert(root_surface) {
                entries.push(SceneEntry {
                    id: *next_id,
                    root_surface,
                    role: SceneRole::WorkspaceRoot,
                    workspace_id: Some(workspace_id),
                });
                *next_id += 1;
            }
        }
    }
}

fn collect_unmanaged_scene_entries(
    state: &State,
    managed_roots: &BTreeSet<Resolved<WlSurfaceId>>,
    entries: &mut Vec<SceneEntry>,
    next_id: &mut SceneEntryId,
) {
    for root_surface in state.root_surfaces() {
        if managed_roots.contains(&root_surface) || state.is_popup_root_surface(root_surface) {
            continue;
        }
        let Some(surface) = root_surface.try_ref(state) else {
            continue;
        };
        if surface.has_cursor_role() || surface.has_dnd_icon_role() {
            continue;
        }
        if surface.layer_surface().is_some() {
            continue;
        }
        entries.push(SceneEntry {
            id: *next_id,
            root_surface,
            role: SceneRole::UnmanagedRoot,
            workspace_id: state.root_surface_workspace(root_surface),
        });
        *next_id += 1;
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        clients::ClientId,
        protocol_generated::{WlSurfaceId, XdgPopupId, XdgSurfaceId, XdgWmBaseId},
        state::{BackendOutputRef, DrmId},
        test_support::{state_for_tests, test_output_with_crtc_name},
        wayland_objects::{
            wl_surface::WlSurface,
            xdg_popup::XdgPopup,
            xdg_positioner::PositionerState,
            xdg_surface::XdgSurface,
            zwlr_layer_surface_v1::{LayerShellLayer, ZwlrLayerSurfaceV1},
        },
    };

    use super::{build_scene_model, SceneRole};

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

    fn insert_test_xdg_surface(
        state: &mut crate::state::State,
        raw_surface: u32,
        raw_xdg: u32,
    ) -> (crate::clients::Resolved<WlSurfaceId>, crate::clients::Resolved<XdgSurfaceId>) {
        let wl_surface = insert_root(state, raw_surface);
        let (_, xdg_oid) = state.insert_object(XdgSurface::new(
            XdgSurfaceId::from(raw_xdg),
            crate::clients::Resolved::new(
                ClientId::from_raw(1),
                XdgWmBaseId::from(1u32),
                crate::state::ObjectId::from(1usize),
            ),
            wl_surface,
            7,
        ));
        let xdg_surface = crate::clients::Resolved::new(
            ClientId::from_raw(1),
            XdgSurfaceId::from(raw_xdg),
            xdg_oid,
        );
        state
            .objects
            .get_mut(wl_surface.object_id())
            .downcast_mut::<WlSurface>()
            .unwrap()
            .set_xdg_surface_impl(xdg_surface);
        (wl_surface, xdg_surface)
    }

    #[test]
    fn excludes_dnd_roots_from_scene_model() {
        let mut state = state_for_tests();
        let (_, dnd_oid) =
            state.insert_object(WlSurface::new(WlSurfaceId::from(2), ClientId::from_raw(1), 6));
        let dnd =
            crate::clients::Resolved::new(ClientId::from_raw(1), WlSurfaceId::from(2), dnd_oid);
        state.register_root_surface(dnd);
        state
            .objects
            .get_mut(dnd_oid)
            .downcast_mut::<WlSurface>()
            .unwrap()
            .set_dnd_icon_role()
            .unwrap();

        let scene = build_scene_model(&state);
        assert!(scene.entries.is_empty());
        assert!(!scene
            .entries
            .iter()
            .any(|entry| entry.role == SceneRole::UnmanagedRoot));
    }

    #[test]
    fn managed_roots_are_deduplicated_when_one_workspace_group_spans_outputs() {
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

        let root_surface = insert_root(&mut state, 10);
        let _ = state.assign_root_surface_to_workspace(root_surface, workspace_id);
        state
            .workspaces
            .try_get_mut(workspace_id)
            .unwrap()
            .layout
            .floating
            .stack
            .push(root_surface);

        let scene = build_scene_model(&state);
        let managed = scene
            .entries
            .iter()
            .filter(|entry| entry.role == SceneRole::WorkspaceRoot)
            .map(|entry| entry.root_surface)
            .collect::<Vec<_>>();
        assert_eq!(managed, vec![root_surface]);
    }

    #[test]
    fn unmanaged_non_layer_roots_follow_managed_prefix() {
        let mut state = state_for_tests();
        let backend = BackendOutputRef::new(DrmId::from(0usize));
        let output_id = state.insert_output(test_output_with_crtc_name(backend, 1, 0, 0, 100, 100));
        let workspace_id = state.active_workspace_for_output(output_id).unwrap();
        let managed = insert_root(&mut state, 11);
        let unmanaged = insert_root(&mut state, 12);
        let _ = state.assign_root_surface_to_workspace(managed, workspace_id);
        state
            .workspaces
            .try_get_mut(workspace_id)
            .unwrap()
            .layout
            .floating
            .stack
            .push(managed);

        let scene = build_scene_model(&state);
        assert_eq!(
            scene
                .entries
                .iter()
                .map(|entry| entry.role)
                .collect::<Vec<_>>(),
            vec![SceneRole::WorkspaceRoot, SceneRole::UnmanagedRoot]
        );
        assert_eq!(scene.entries[0].root_surface, managed);
        assert_eq!(scene.entries[1].root_surface, unmanaged);
    }

    #[test]
    fn layer_roots_do_not_appear_in_scene_model() {
        let mut state = state_for_tests();
        let backend = BackendOutputRef::new(DrmId::from(0usize));
        let output_id = state.insert_output(test_output_with_crtc_name(backend, 1, 0, 0, 100, 100));
        let root_surface = insert_root(&mut state, 20);
        let layer = ZwlrLayerSurfaceV1::new(
            crate::protocol_generated::ZwlrLayerSurfaceV1Id::from(21u32),
            crate::clients::Resolved::new(
                ClientId::from_raw(1),
                crate::protocol_generated::ZwlrLayerShellV1Id::from(22u32),
                crate::state::ObjectId::from(22usize),
            ),
            5,
            root_surface,
            Some(output_id),
            LayerShellLayer::Top,
            "test-layer".to_string(),
        );
        let (_, layer_oid) = state.insert_object(layer);
        let layer_surface = crate::clients::Resolved::new(
            ClientId::from_raw(1),
            crate::protocol_generated::ZwlrLayerSurfaceV1Id::from(21u32),
            layer_oid,
        );
        state
            .objects
            .get_mut(root_surface.object_id())
            .downcast_mut::<WlSurface>()
            .unwrap()
            .set_layer_surface_impl(layer_surface);
        state.register_layer_surface_object(layer_surface);

        let scene = build_scene_model(&state);
        assert!(scene.entries.is_empty());
    }

    #[test]
    fn popup_roots_are_excluded_as_top_level_scene_entries() {
        let mut state = state_for_tests();
        let (_parent_wl, parent_xdg) = insert_test_xdg_surface(&mut state, 30, 31);
        let (popup_wl, popup_xdg) = insert_test_xdg_surface(&mut state, 32, 33);
        let (_, popup_oid) = state.insert_object(XdgPopup::new(
            XdgPopupId::from(34u32),
            popup_xdg,
            Some(parent_xdg),
            PositionerState::default(),
            7,
        ));
        let popup = crate::clients::Resolved::new(
            ClientId::from_raw(1),
            XdgPopupId::from(34u32),
            popup_oid,
        );
        state
            .objects
            .get_mut(popup_xdg.object_id())
            .downcast_mut::<XdgSurface>()
            .unwrap()
            .test_set_popup_role(popup);

        let scene = build_scene_model(&state);
        assert!(!scene
            .entries
            .iter()
            .any(|entry| entry.root_surface == popup_wl));
    }
}
