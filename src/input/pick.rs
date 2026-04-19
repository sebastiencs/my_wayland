use crate::{
    clients::Resolved,
    protocol_generated::{xdg_toplevel::ResizeEdge, WlSurfaceId, XdgSurfaceId},
    region::RectI,
    scene::compose_global_root_order,
    state::State,
    wayland_objects::{
        wl_surface::{Role, StackEntry, WlSurface},
        xdg_surface::xdg_toplevel::ToplevelDecorationMode,
    },
};

use super::{
    ScenePick, SurfaceFocus, SurfaceFocusPick, SurfaceRect, WindowRoleSnapshot, WindowSnapshot,
    WindowTarget,
};

pub(crate) fn scene_pick_at(state: &State, pointer_pos: (f64, f64)) -> ScenePick {
    let scene = state.build_scene_model();
    let ordered_roots = compose_global_root_order(state, &scene, None);

    for root_surface in ordered_roots.into_iter().rev() {
        let Some(surface) = root_surface.try_ref(state) else {
            continue;
        };
        if !surface.is_root() || surface.has_cursor_role() || surface.has_dnd_icon_role() {
            continue;
        }
        let root_hit = hit_test_subtree(state, root_surface, 0, 0, pointer_pos.0, pointer_pos.1);
        let has_mapped_bounds =
            WlSurface::local_surface_tree_bounds_from_ref(state, &surface).is_some();
        let window_target_for_root = has_mapped_bounds
            .then(|| state.committed_root_geometry(root_surface))
            .flatten()
            .filter(|geometry| rect_contains_point(geometry.window_target_rect(), pointer_pos))
            .and_then(|_| build_window_target(state, root_surface));
        if root_hit.is_some() || window_target_for_root.is_some() {
            return ScenePick {
                surface_focus: root_hit,
                window_target: window_target_for_root,
            };
        }
    }

    ScenePick { surface_focus: None, window_target: None }
}

pub(crate) fn scene_pick_for_pointer(state: &State) -> ScenePick {
    scene_pick_at(state, state.seat.pointer_position())
}

pub(crate) fn surface_focus_pick_at(
    state: &State,
    pointer_pos: (f64, f64),
) -> Option<SurfaceFocusPick> {
    let mut focus = scene_pick_at(state, pointer_pos).surface_focus;
    if let Some(grab_client_id) = state.seat.popup_grab_client() {
        if focus.is_some_and(|pick| pick.target.client_id() != grab_client_id) {
            focus = None;
        }
    }
    focus
}

pub(crate) fn resolve_window_target(
    state: &State,
    root_surface: Resolved<WlSurfaceId>,
) -> Option<WindowTarget> {
    build_window_target(state, root_surface)
}

pub(crate) fn build_protocol_move_target(
    state: &State,
    root_surface: Resolved<WlSurfaceId>,
    role: WindowRoleSnapshot,
) -> Option<WindowTarget> {
    let surface = root_surface.try_ref(state)?;
    let decoration_mode = protocol_decoration_mode(role)?;
    let geometry = crate::root_geometry::resolve_protocol_move_target_geometry(
        state,
        root_surface,
        &surface,
        decoration_mode,
    )?;
    Some(WindowSnapshot { root_surface, role, geometry })
}

pub(crate) fn build_protocol_resize_target(
    state: &State,
    root_surface: Resolved<WlSurfaceId>,
    role: WindowRoleSnapshot,
) -> Option<WindowTarget> {
    let surface = root_surface.try_ref(state)?;
    let geometry = crate::root_geometry::resolve_protocol_resize_target_geometry(
        state,
        root_surface,
        &surface,
        protocol_decoration_mode(role)?,
    )?;
    Some(WindowSnapshot { root_surface, role, geometry })
}

pub(crate) fn build_window_target(
    state: &State,
    root_surface: Resolved<WlSurfaceId>,
) -> Option<WindowTarget> {
    let surface = root_surface.try_ref(state)?;
    let geometry = state.committed_root_geometry(root_surface)?;
    let role = match surface.role() {
        Role::XdgSurface => surface
            .xdg_surface()
            .map(|xdg_surface_id| {
                resolve_xdg_window_role_snapshot(state, root_surface, xdg_surface_id.wayland_id())
            })
            .unwrap_or(WindowRoleSnapshot::Unsupported),
        _ => WindowRoleSnapshot::Unsupported,
    };

    Some(WindowSnapshot {
        root_surface,
        role,
        geometry: crate::root_geometry::project_window_target_geometry(
            geometry,
            surface.logical_size(),
        ),
    })
}

fn protocol_decoration_mode(role: WindowRoleSnapshot) -> Option<ToplevelDecorationMode> {
    match role {
        WindowRoleSnapshot::XdgToplevel { decoration_mode, .. } => Some(decoration_mode),
        WindowRoleSnapshot::Unsupported => None,
    }
}

pub(crate) fn resolve_xdg_window_role_snapshot(
    state: &State,
    root_surface: Resolved<WlSurfaceId>,
    xdg_surface_id: XdgSurfaceId,
) -> WindowRoleSnapshot {
    let Some(xdg_surface) = root_surface
        .try_ref(state)
        .and_then(|s| s.xdg_surface())
        .filter(|xs| xs.wayland_id() == xdg_surface_id)
    else {
        return WindowRoleSnapshot::Unsupported;
    };
    state
        .xdg_toplevel(xdg_surface)
        .map(|xdg_toplevel| {
            let toplevel = xdg_toplevel.rref(state);
            WindowRoleSnapshot::XdgToplevel {
                xdg_toplevel,
                size_constraints: toplevel.interactive_size_constraints(),
                is_maximized: toplevel.is_maximized(),
                is_fullscreen: state.root_surface_is_fullscreen(root_surface),
                decoration_mode: toplevel.decoration_mode(),
            }
        })
        .unwrap_or(WindowRoleSnapshot::Unsupported)
}

pub(crate) fn rect_contains_point(bounds: RectI, pointer_pos: (f64, f64)) -> bool {
    pointer_pos.0 >= bounds.x as f64
        && pointer_pos.0 < (bounds.x + bounds.width) as f64
        && pointer_pos.1 >= bounds.y as f64
        && pointer_pos.1 < (bounds.y + bounds.height) as f64
}

pub(crate) fn resize_edges_for_pointer(bounds: RectI, pointer_pos: (f64, f64)) -> u32 {
    let left_boundary = bounds.x as f64 + (bounds.width as f64 / 3.0);
    let right_boundary = bounds.x as f64 + ((bounds.width as f64 * 2.0) / 3.0);
    let top_boundary = bounds.y as f64 + (bounds.height as f64 / 3.0);
    let bottom_boundary = bounds.y as f64 + ((bounds.height as f64 * 2.0) / 3.0);

    let horizontal = if pointer_pos.0 < left_boundary {
        Some(ResizeEdge::Left as u32)
    } else if pointer_pos.0 >= right_boundary {
        Some(ResizeEdge::Right as u32)
    } else {
        None
    };
    let vertical = if pointer_pos.1 < top_boundary {
        Some(ResizeEdge::Top as u32)
    } else if pointer_pos.1 >= bottom_boundary {
        Some(ResizeEdge::Bottom as u32)
    } else {
        None
    };

    match (vertical, horizontal) {
        (Some(vertical), Some(horizontal)) => vertical | horizontal,
        (Some(vertical), None) => vertical,
        (None, Some(horizontal)) => horizontal,
        (None, None) => {
            let left_distance = pointer_pos.0 - bounds.x as f64;
            let right_distance = (bounds.x + bounds.width) as f64 - pointer_pos.0;
            let top_distance = pointer_pos.1 - bounds.y as f64;
            let bottom_distance = (bounds.y + bounds.height) as f64 - pointer_pos.1;

            let mut best_edge = ResizeEdge::Left as u32;
            let mut best_distance = left_distance;
            for (distance, edge) in [
                (right_distance, ResizeEdge::Right as u32),
                (top_distance, ResizeEdge::Top as u32),
                (bottom_distance, ResizeEdge::Bottom as u32),
            ] {
                if distance < best_distance {
                    best_distance = distance;
                    best_edge = edge;
                }
            }
            best_edge
        }
    }
}

fn hit_test_subtree(
    state: &State,
    surface_id: Resolved<WlSurfaceId>,
    base_x: i32,
    base_y: i32,
    x: f64,
    y: f64,
) -> Option<SurfaceFocusPick> {
    let surface = surface_id.try_ref(state)?;
    let surface_state = surface.render_state_at(base_x, base_y)?;
    let stack = surface.current_stack().to_vec();
    let popup_children = state.popup_child_surfaces(surface_id);

    for popup_surface in popup_children.into_iter().rev() {
        let Some((popup_x, popup_y)) = state.surface_scene_origin(popup_surface) else {
            continue;
        };
        if let Some(hit) = hit_test_subtree(state, popup_surface, popup_x, popup_y, x, y) {
            return Some(hit);
        }
    }

    for entry in stack.iter().rev() {
        match *entry {
            StackEntry::Parent => {
                let rect = SurfaceRect {
                    x: surface_state.x,
                    y: surface_state.y,
                    width: surface_state.width,
                    height: surface_state.height,
                };
                if surface.accepts_input_at_with_origin(x, y, base_x, base_y) {
                    return Some(SurfaceFocusPick { target: SurfaceFocus::new(surface_id), rect });
                }
            }
            StackEntry::Child(child_surface_id) => {
                let Some(child_surface) = child_surface_id.try_ref(state) else {
                    continue;
                };
                let (child_x, child_y) = child_surface.current_subsurface_position();
                if let Some(hit) = hit_test_subtree(
                    state,
                    child_surface_id,
                    surface_state.x + child_x,
                    surface_state.y + child_y,
                    x,
                    y,
                ) {
                    return Some(hit);
                }
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use crate::{
        clients::{ClientId, Resolved},
        config::DecorationInsets,
        protocol_generated::{WlSurfaceId, XdgPopupId, XdgSurfaceId, XdgToplevelId, XdgWmBaseId},
        region::RectI,
        root_geometry::CommittedRootGeometry,
        state::ObjectId,
        test_support::state_for_tests,
        wayland_objects::{
            wl_surface::WlSurface,
            xdg_popup::XdgPopup,
            xdg_positioner::PositionerState,
            xdg_surface::{
                xdg_toplevel::{InteractiveSizeConstraints, ToplevelDecorationMode},
                XdgSurface,
            },
        },
    };

    use super::{
        build_protocol_move_target, build_protocol_resize_target, scene_pick_at, WindowRoleSnapshot,
    };

    fn insert_test_xdg_surface(
        state: &mut crate::state::State,
        client_id: ClientId,
        wl_surface_id: WlSurfaceId,
        xdg_surface_id: XdgSurfaceId,
    ) -> (Resolved<WlSurfaceId>, Resolved<XdgSurfaceId>) {
        let (_, surface_object_id) =
            state.insert_object(WlSurface::new(wl_surface_id, client_id, 6));
        let wl_surface = Resolved::new(client_id, wl_surface_id, surface_object_id);
        state.register_root_surface(wl_surface);
        let (_, xdg_surface_object_id) = state.insert_object(XdgSurface::new(
            xdg_surface_id,
            Resolved::new(client_id, XdgWmBaseId::from(1), ObjectId::from(1usize)),
            wl_surface,
            7,
        ));
        let xdg_surface = Resolved::new(client_id, xdg_surface_id, xdg_surface_object_id);
        state
            .objects
            .get_mut(surface_object_id)
            .downcast_mut::<WlSurface>()
            .unwrap()
            .set_xdg_surface_impl(xdg_surface);
        (wl_surface, xdg_surface)
    }

    #[test]
    fn protocol_window_target_uses_projected_geometry_root_position() {
        let mut state = state_for_tests();
        let root_surface_id = WlSurfaceId::from(41u32);
        let root_object_id = state
            .insert_object(WlSurface::new(root_surface_id, ClientId::from_raw(7), 6))
            .1;
        let root_surface = Resolved::new(ClientId::from_raw(7), root_surface_id, root_object_id);
        state.register_root_surface(root_surface);
        {
            let surface = state
                .objects
                .get_mut(root_object_id)
                .downcast_mut::<WlSurface>()
                .unwrap();
            surface.set_root_position(20, 30);
        }
        state.root_geometry_cache.insert(
            root_surface,
            std::cell::Cell::new(crate::state::RootGeometryCacheEntry {
                geometry: Some(CommittedRootGeometry {
                    layout_rect_global: RectI { x: 20, y: 30, width: 120, height: 80 },
                    root_pos: (20, 30),
                    surface_bounds_local: Some(RectI { x: 0, y: 0, width: 120, height: 80 }),
                    logical_window_local: Some(RectI { x: 0, y: 0, width: 120, height: 80 }),
                    decoration_mode: Some(ToplevelDecorationMode::ClientSide),
                    decoration_insets: DecorationInsets::default(),
                    tiled_edges: 0,
                    fullscreen: false,
                }),
                dirty: false,
                revision: 0,
                built_from_revision: 0,
            }),
        );

        let target = build_protocol_move_target(
            &state,
            root_surface,
            WindowRoleSnapshot::XdgToplevel {
                xdg_toplevel: Resolved::new(
                    ClientId::from_raw(7),
                    XdgToplevelId::from(42u32),
                    ObjectId::from(42usize),
                ),
                size_constraints: InteractiveSizeConstraints::default(),
                is_maximized: false,
                is_fullscreen: false,
                decoration_mode: ToplevelDecorationMode::ClientSide,
            },
        )
        .expect("protocol target");

        assert_eq!(target.geometry.root_pos, (20, 30));
        assert_eq!(target.geometry.resize_size, (120, 80));
    }

    #[test]
    fn protocol_resize_target_keeps_live_ssd_anchor_but_uses_configure_size() {
        let mut state = state_for_tests();
        let root_surface_id = WlSurfaceId::from(51u32);
        let root_object_id = state
            .insert_object(WlSurface::new(root_surface_id, ClientId::from_raw(8), 6))
            .1;
        let root_surface = Resolved::new(ClientId::from_raw(8), root_surface_id, root_object_id);
        state.register_root_surface(root_surface);
        {
            let surface = state
                .objects
                .get_mut(root_object_id)
                .downcast_mut::<WlSurface>()
                .unwrap();
            surface.set_root_position(25, 25);
        }
        state.root_geometry_cache.insert(
            root_surface,
            std::cell::Cell::new(crate::state::RootGeometryCacheEntry {
                geometry: Some(CommittedRootGeometry {
                    layout_rect_global: RectI { x: 20, y: 20, width: 1240, height: 760 },
                    root_pos: (25, 25),
                    surface_bounds_local: Some(RectI { x: -5, y: -5, width: 1240, height: 760 }),
                    logical_window_local: Some(RectI { x: 0, y: 0, width: 1230, height: 750 }),
                    decoration_mode: Some(ToplevelDecorationMode::ServerSide),
                    decoration_insets: DecorationInsets { top: 5, right: 5, bottom: 5, left: 5 },
                    tiled_edges: 0,
                    fullscreen: false,
                }),
                dirty: false,
                revision: 0,
                built_from_revision: 0,
            }),
        );

        let target = build_protocol_resize_target(
            &state,
            root_surface,
            WindowRoleSnapshot::XdgToplevel {
                xdg_toplevel: Resolved::new(
                    ClientId::from_raw(8),
                    XdgToplevelId::from(52u32),
                    ObjectId::from(52usize),
                ),
                size_constraints: InteractiveSizeConstraints::default(),
                is_maximized: false,
                is_fullscreen: false,
                decoration_mode: ToplevelDecorationMode::ServerSide,
            },
        )
        .expect("protocol resize target");

        assert_eq!(target.geometry.root_pos, (25, 25));
        assert_eq!(target.geometry.resize_size, (1230, 750));
    }

    #[test]
    fn scene_pick_prefers_popup_child_and_tracks_parent_motion() {
        let mut state = state_for_tests();
        let client_id = ClientId::from_raw(7);
        let (parent_wl, parent_xdg) = insert_test_xdg_surface(
            &mut state,
            client_id,
            WlSurfaceId::from(61),
            XdgSurfaceId::from(62),
        );
        let (popup_wl, popup_xdg) = insert_test_xdg_surface(
            &mut state,
            client_id,
            WlSurfaceId::from(63),
            XdgSurfaceId::from(64),
        );
        state
            .objects
            .get_mut(parent_wl.object_id())
            .downcast_mut::<WlSurface>()
            .unwrap()
            .test_map(ObjectId::from(401usize), 640, 0, 640, 800);
        state
            .objects
            .get_mut(parent_wl.object_id())
            .downcast_mut::<WlSurface>()
            .unwrap()
            .set_root_position(640, 0);
        state
            .objects
            .get_mut(popup_wl.object_id())
            .downcast_mut::<WlSurface>()
            .unwrap()
            .test_map(ObjectId::from(402usize), 0, 0, 299, 457);

        let (_, popup_object_id) = state.insert_object(XdgPopup::new(
            XdgPopupId::from(65),
            popup_xdg,
            Some(parent_xdg),
            PositionerState::default(),
            7,
        ));
        let popup = Resolved::new(client_id, XdgPopupId::from(65), popup_object_id);
        state
            .objects
            .get_mut(popup_object_id)
            .downcast_mut::<XdgPopup>()
            .unwrap()
            .apply_acked_configure(RectI { x: 387, y: 40, width: 253, height: 411 });
        state
            .objects
            .get_mut(popup_xdg.object_id())
            .downcast_mut::<XdgSurface>()
            .unwrap()
            .test_set_current_window_geometry(Some(RectI {
                x: 23,
                y: 21,
                width: 253,
                height: 411,
            }));
        state
            .objects
            .get_mut(popup_xdg.object_id())
            .downcast_mut::<XdgSurface>()
            .unwrap()
            .test_set_popup_role(popup);
        state
            .objects
            .get_mut(parent_xdg.object_id())
            .downcast_mut::<XdgSurface>()
            .unwrap()
            .add_popup_child(popup);

        let initial_pick = scene_pick_at(&state, (1010.0, 25.0));
        assert_eq!(initial_pick.surface_focus.map(|focus| focus.target.surface), Some(popup_wl));

        state
            .objects
            .get_mut(parent_wl.object_id())
            .downcast_mut::<WlSurface>()
            .unwrap()
            .set_root_position(700, 10);

        let moved_pick = scene_pick_at(&state, (1070.0, 35.0));
        assert_eq!(moved_pick.surface_focus.map(|focus| focus.target.surface), Some(popup_wl));
        assert_eq!(
            scene_pick_at(&state, (1010.0, 25.0))
                .surface_focus
                .map(|focus| focus.target.surface),
            Some(parent_wl)
        );
    }

    #[test]
    fn scene_pick_prefers_topmost_managed_root_from_scene_order() {
        let mut state = state_for_tests();
        let backend = crate::state::BackendOutputRef::new(crate::state::DrmId::from(0usize));
        let output_id = state.insert_output(crate::test_support::test_output_with_crtc_name(
            backend, 1, 0, 0, 200, 100,
        ));
        let workspace_id = state.active_workspace_for_output(output_id).unwrap();
        let lower_object_id = state
            .insert_object(WlSurface::new(WlSurfaceId::from(71), ClientId::from_raw(1), 6))
            .1;
        let upper_object_id = state
            .insert_object(WlSurface::new(WlSurfaceId::from(72), ClientId::from_raw(1), 6))
            .1;
        let lower = Resolved::new(ClientId::from_raw(1), WlSurfaceId::from(71), lower_object_id);
        let upper = Resolved::new(ClientId::from_raw(1), WlSurfaceId::from(72), upper_object_id);
        state.register_root_surface(lower);
        state.register_root_surface(upper);
        let _ = state.assign_root_surface_to_workspace(lower, workspace_id);
        let _ = state.assign_root_surface_to_workspace(upper, workspace_id);
        state
            .workspaces
            .try_get_mut(workspace_id)
            .unwrap()
            .layout
            .floating
            .stack = vec![lower, upper];
        state
            .objects
            .get_mut(lower.object_id())
            .downcast_mut::<WlSurface>()
            .unwrap()
            .test_map(ObjectId::from(501usize), 0, 0, 100, 100);
        state
            .objects
            .get_mut(upper.object_id())
            .downcast_mut::<WlSurface>()
            .unwrap()
            .test_map(ObjectId::from(502usize), 0, 0, 100, 100);

        let pick = scene_pick_at(&state, (10.0, 10.0));
        assert_eq!(pick.surface_focus.map(|focus| focus.target.surface), Some(upper));
    }
}
