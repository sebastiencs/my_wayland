//! Workspace participation for scene composition.
//!
//! `ComposedEntry` keeps three consumer-facing axes: input, protocol visibility, and render
//! participation. `WorkspaceParticipationPolicy::Steady` preserves today's active-workspace
//! behavior. `WorkspaceParticipationPolicy::Transition` is dormant until a later phase supplies a
//! real workspace transition: incoming roots participate on every axis, while outgoing roots stay
//! protocol-visible and renderable but are not interactive.
//!
//! # Child surfaces and layer-shell
//!
//! Subsurfaces and xdg popups are not independent scene participants. Hit testing, output
//! membership refresh, and render snapshot collection walk their parent tree, so they inherit all
//! three axes from their top-level root. Layer-shell surfaces stay output-scoped and do not join
//! workspace transitions.
//!
//! # Roots without a workspace
//!
//! A root whose workspace is `None` stays eligible under its role and output rules. That covers
//! unmanaged roots, roots seen during teardown, and roots that have not been assigned yet. Narrow
//! that fallback only with a separate, tested policy change.
//!
//! # Frame callbacks and presentation
//!
//! `wl_surface.frame` and `wp_presentation` follow surfaces that were actually submitted in the
//! render item list. They do not follow interactive participation or logical workspace activity.
//! A later live transition must keep an outgoing participant in that submitted set through its
//! final frame before protocol or render participation ends. This module does not add another
//! callback, timer, or presentation path.

use crate::{
    clients::Resolved,
    protocol_generated::WlSurfaceId,
    region::RectI,
    state::{OutputId, State, WorkspaceGroupId, WorkspaceId},
    wayland_objects::zwlr_layer_surface_v1::{LayerLayoutObjectSnapshot, SceneLayerPlacement},
};

use super::{
    compose_output,
    model::{SceneEntry, SceneRole},
};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum WorkspaceParticipationPolicy {
    #[default]
    Steady,
    Transition {
        group_id: WorkspaceGroupId,
        outgoing: WorkspaceId,
        incoming: WorkspaceId,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ParticipationAxis {
    Input,
    Protocol,
    Render,
}

pub struct CompositionEvalContext<'a> {
    pub state: &'a State,
    pub output_id: OutputId,
    pub snapshot: Option<LayerLayoutObjectSnapshot<'a>>,
    pub policy: WorkspaceParticipationPolicy,
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
            if !workspace_axis_allows(ctx, entry.root_surface, ParticipationAxis::Render) {
                return false;
            }
            if !scene_entry_belongs_to_output_with_policy(
                ctx.state,
                entry,
                ctx.output_id,
                ctx.policy,
            ) {
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
            workspace_axis_allows(ctx, entry.root_surface, ParticipationAxis::Protocol)
                && scene_entry_belongs_to_output_with_policy(
                    ctx.state,
                    entry,
                    ctx.output_id,
                    ctx.policy,
                )
        }
        CompositionSource::LayerRoot { placement, .. } => placement.output_id == ctx.output_id,
    }
}

pub fn input_participates(ctx: &CompositionEvalContext<'_>, source: CompositionSource<'_>) -> bool {
    match source {
        CompositionSource::SceneEntry(entry) => {
            if !workspace_axis_allows(ctx, entry.root_surface, ParticipationAxis::Input) {
                return false;
            }
            if !scene_entry_belongs_to_output_with_policy(
                ctx.state,
                entry,
                ctx.output_id,
                ctx.policy,
            ) {
                return false;
            }
            match entry.role {
                SceneRole::WorkspaceRoot => ctx.state.root_surface_is_mapped(entry.root_surface),
                SceneRole::UnmanagedRoot => true,
            }
        }
        CompositionSource::LayerRoot { placement, .. } => placement.output_id == ctx.output_id,
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
    workspace_axis_allows(&steady_context(state), root_surface, ParticipationAxis::Protocol)
}

pub fn root_has_protocol_visibility(state: &State, root_surface: Resolved<WlSurfaceId>) -> bool {
    root_has_participation(state, root_surface, |entry| entry.participates_in_protocol_visibility)
}

pub fn root_participates_in_input(state: &State, root_surface: Resolved<WlSurfaceId>) -> bool {
    root_has_participation(state, root_surface, |entry| entry.participates_in_input)
}

pub fn output_workspace_ids(
    state: &State,
    output_id: OutputId,
    policy: WorkspaceParticipationPolicy,
) -> Vec<WorkspaceId> {
    let Some(active_workspace_id) = state.active_workspace_for_output(output_id) else {
        return Vec::new();
    };
    let WorkspaceParticipationPolicy::Transition { group_id, outgoing, incoming } =
        normalize_participation_policy(state, policy)
    else {
        return vec![active_workspace_id];
    };
    if state.workspace_group_for_output(output_id) != Some(group_id) {
        return vec![active_workspace_id];
    }
    let mut workspace_ids = Vec::new();
    if outgoing != incoming {
        workspace_ids.push(outgoing);
    }
    workspace_ids.push(incoming);
    workspace_ids
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
    scene_entry_belongs_to_output_with_policy(
        state,
        entry,
        output_id,
        WorkspaceParticipationPolicy::Steady,
    )
}

pub fn scene_entry_belongs_to_output_with_policy(
    state: &State,
    entry: &SceneEntry,
    output_id: OutputId,
    policy: WorkspaceParticipationPolicy,
) -> bool {
    match entry.role {
        SceneRole::WorkspaceRoot => entry.workspace_id.is_some_and(|workspace_id| {
            output_workspace_ids(state, output_id, policy).contains(&workspace_id)
        }),
        SceneRole::UnmanagedRoot => root_bounds_for_output_membership(state, entry.root_surface)
            .map(|bounds| bounds.intersects(output_bounds(state, output_id)))
            .unwrap_or(true),
    }
}

fn root_has_participation(
    state: &State,
    root_surface: Resolved<WlSurfaceId>,
    mut matches_entry: impl FnMut(&super::ComposedEntry) -> bool,
) -> bool {
    let scene = super::build_scene_model(state);
    state
        .outputs
        .iter()
        .map(|(output_id, _)| output_id)
        .filter(|output_id| state.output_bounds(*output_id).is_some())
        .map(|output_id| compose_output(state, &scene, output_id, None))
        .any(|view| {
            view.entries
                .iter()
                .any(|entry| entry.root_surface == root_surface && matches_entry(entry))
        })
}

fn steady_context(state: &State) -> CompositionEvalContext<'_> {
    CompositionEvalContext {
        state,
        output_id: OutputId::from(0usize),
        snapshot: None,
        policy: WorkspaceParticipationPolicy::Steady,
    }
}

fn workspace_axis_allows(
    ctx: &CompositionEvalContext<'_>,
    root_surface: Resolved<WlSurfaceId>,
    axis: ParticipationAxis,
) -> bool {
    let Some(workspace_id) = ctx.state.root_surface_workspace(root_surface) else {
        return true;
    };
    if !workspace_eligible_for_axis(ctx.state, workspace_id, ctx.policy, axis) {
        return false;
    }
    fullscreen_allows_root(ctx.state, workspace_id, root_surface)
}

fn workspace_eligible_for_axis(
    state: &State,
    workspace_id: WorkspaceId,
    policy: WorkspaceParticipationPolicy,
    axis: ParticipationAxis,
) -> bool {
    let WorkspaceParticipationPolicy::Transition { outgoing, incoming, .. } =
        normalize_participation_policy(state, policy)
    else {
        return state.workspace_is_active(workspace_id);
    };
    if workspace_id == incoming {
        return true;
    }
    if workspace_id == outgoing {
        return axis != ParticipationAxis::Input;
    }
    state.workspace_is_active(workspace_id)
}

fn fullscreen_allows_root(
    state: &State,
    workspace_id: WorkspaceId,
    root_surface: Resolved<WlSurfaceId>,
) -> bool {
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
}

fn normalize_participation_policy(
    state: &State,
    policy: WorkspaceParticipationPolicy,
) -> WorkspaceParticipationPolicy {
    let WorkspaceParticipationPolicy::Transition { group_id, outgoing, incoming } = policy else {
        return WorkspaceParticipationPolicy::Steady;
    };
    let valid = state
        .workspace_groups
        .try_get(group_id)
        .is_some_and(|group| {
            group.workspaces.contains(&outgoing) && group.workspaces.contains(&incoming)
        });
    if valid {
        policy
    } else {
        WorkspaceParticipationPolicy::Steady
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
        input_participates, protocol_visible, render_participates, root_has_protocol_visibility,
        root_is_globally_visible, root_participates_in_input, scene_entry_belongs_to_output,
        scene_entry_belongs_to_output_with_policy, CompositionEvalContext, CompositionSource,
        WorkspaceParticipationPolicy,
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

    fn workspace_entry(
        root_surface: crate::clients::Resolved<WlSurfaceId>,
        workspace_id: crate::state::WorkspaceId,
    ) -> crate::scene::SceneEntry {
        crate::scene::SceneEntry {
            id: 1,
            root_surface,
            role: crate::scene::SceneRole::WorkspaceRoot,
            workspace_id: Some(workspace_id),
        }
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

    #[test]
    fn transition_policy_splits_outgoing_participation_by_axis() {
        let mut state = state_for_tests();
        let backend = BackendOutputRef::new(DrmId::from(0usize));
        let output_id = state.insert_output(test_output_with_crtc_name(backend, 1, 0, 0, 100, 100));
        let group_id = state.workspace_group_for_output(output_id).unwrap();
        let incoming = state.active_workspace_for_output(output_id).unwrap();
        let outgoing = state.create_workspace(Some(group_id), Some("2".to_string()));
        let outgoing_root = insert_root(&mut state, 81);
        let incoming_root = insert_root(&mut state, 82);
        assert!(state.assign_root_surface_to_workspace(outgoing_root, outgoing));
        let _ = state.assign_root_surface_to_workspace(incoming_root, incoming);
        state
            .managed_root_surfaces
            .get_mut(&outgoing_root)
            .unwrap()
            .mapped = true;
        state
            .managed_root_surfaces
            .get_mut(&incoming_root)
            .unwrap()
            .mapped = true;
        let policy = WorkspaceParticipationPolicy::Transition { group_id, outgoing, incoming };
        let ctx = CompositionEvalContext { state: &state, output_id, snapshot: None, policy };
        let outgoing_entry = workspace_entry(outgoing_root, outgoing);
        let incoming_entry = workspace_entry(incoming_root, incoming);
        let outgoing_source = CompositionSource::SceneEntry(&outgoing_entry);
        let incoming_source = CompositionSource::SceneEntry(&incoming_entry);

        assert!(!input_participates(&ctx, outgoing_source));
        assert!(protocol_visible(&ctx, outgoing_source));
        assert!(render_participates(&ctx, outgoing_source));
        assert!(input_participates(&ctx, incoming_source));
        assert!(protocol_visible(&ctx, incoming_source));
        assert!(render_participates(&ctx, incoming_source));
    }

    #[test]
    fn steady_policy_keeps_unmapped_workspace_roots_out_of_input() {
        let mut state = state_for_tests();
        let backend = BackendOutputRef::new(DrmId::from(0usize));
        let output_id = state.insert_output(test_output_with_crtc_name(backend, 1, 0, 0, 100, 100));
        let workspace_id = state.active_workspace_for_output(output_id).unwrap();
        let root = insert_root(&mut state, 83);
        let _ = state.assign_root_surface_to_workspace(root, workspace_id);
        state
            .workspaces
            .try_get_mut(workspace_id)
            .unwrap()
            .layout
            .floating
            .stack = vec![root];

        assert!(root_has_protocol_visibility(&state, root));
        assert!(!root_participates_in_input(&state, root));
        state.managed_root_surfaces.get_mut(&root).unwrap().mapped = true;
        assert!(root_participates_in_input(&state, root));
    }

    #[test]
    fn transition_fullscreen_suppression_stays_inside_each_workspace() {
        let mut state = state_for_tests();
        let backend = BackendOutputRef::new(DrmId::from(0usize));
        let output_id = state.insert_output(test_output_with_crtc_name(backend, 1, 0, 0, 100, 100));
        let group_id = state.workspace_group_for_output(output_id).unwrap();
        let incoming = state.active_workspace_for_output(output_id).unwrap();
        let outgoing = state.create_workspace(Some(group_id), Some("2".to_string()));
        let fullscreen = insert_root(&mut state, 84);
        let covered = insert_root(&mut state, 85);
        assert!(state.assign_root_surface_to_workspace(fullscreen, outgoing));
        assert!(state.assign_root_surface_to_workspace(covered, outgoing));
        state
            .managed_root_surfaces
            .get_mut(&fullscreen)
            .unwrap()
            .mapped = true;
        state
            .managed_root_surfaces
            .get_mut(&covered)
            .unwrap()
            .mapped = true;
        state
            .workspaces
            .try_get_mut(outgoing)
            .unwrap()
            .layout
            .fullscreen = Some(crate::tiling::FullscreenState { surface_object_id: fullscreen });
        let policy = WorkspaceParticipationPolicy::Transition { group_id, outgoing, incoming };
        let ctx = CompositionEvalContext { state: &state, output_id, snapshot: None, policy };
        let fullscreen_entry = workspace_entry(fullscreen, outgoing);
        let covered_entry = workspace_entry(covered, outgoing);

        assert!(protocol_visible(&ctx, CompositionSource::SceneEntry(&fullscreen_entry)));
        assert!(render_participates(&ctx, CompositionSource::SceneEntry(&fullscreen_entry)));
        assert!(!input_participates(&ctx, CompositionSource::SceneEntry(&fullscreen_entry)));
        assert!(!protocol_visible(&ctx, CompositionSource::SceneEntry(&covered_entry)));
        assert!(!render_participates(&ctx, CompositionSource::SceneEntry(&covered_entry)));
    }

    #[test]
    fn unrelated_workspace_stays_on_steady_rules_during_transition() {
        let mut state = state_for_tests();
        let backend = BackendOutputRef::new(DrmId::from(0usize));
        let transitioning =
            state.insert_output(test_output_with_crtc_name(backend, 1, 0, 0, 100, 100));
        let other = state.insert_output(test_output_with_crtc_name(backend, 2, 100, 0, 100, 100));
        let group_id = state.workspace_group_for_output(transitioning).unwrap();
        let incoming = state.active_workspace_for_output(transitioning).unwrap();
        let outgoing = state.create_workspace(Some(group_id), Some("2".to_string()));
        let other_workspace = state.active_workspace_for_output(other).unwrap();
        let unrelated = insert_root(&mut state, 86);
        assert!(state.assign_root_surface_to_workspace(unrelated, other_workspace));
        state
            .managed_root_surfaces
            .get_mut(&unrelated)
            .unwrap()
            .mapped = true;
        let policy = WorkspaceParticipationPolicy::Transition { group_id, outgoing, incoming };
        let ctx =
            CompositionEvalContext { state: &state, output_id: other, snapshot: None, policy };
        let entry = workspace_entry(unrelated, other_workspace);
        let source = CompositionSource::SceneEntry(&entry);

        assert!(input_participates(&ctx, source));
        assert!(protocol_visible(&ctx, source));
        assert!(render_participates(&ctx, source));
        assert!(!scene_entry_belongs_to_output_with_policy(&state, &entry, transitioning, policy));
    }
}
