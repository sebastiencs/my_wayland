use crate::{
    clients::Resolved,
    protocol_generated::{xdg_toplevel::ResizeEdge, WlSurfaceId, XdgSurfaceId},
    region::RectI,
    scene::{compose_global_root_order, compose_output},
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
    if state.session_lock.input_gated() {
        return pick_mapped_lock_surface(state, pointer_pos);
    }
    let scene = state.build_scene_model();
    // Output-less state has no composition view. Keep the global root walk so geometry tests and
    // startup before the first output still resolve hits. Once an output exists, pointer picks
    // come only from that output's interactive participants.
    if state.outputs.iter().next().is_none() {
        let ordered_roots = compose_global_root_order(state, &scene, None);
        return pick_roots(state, pointer_pos, ordered_roots.into_iter().rev());
    }
    let Some(output_id) = pointer_output(state, pointer_pos) else {
        return ScenePick { surface_focus: None, window_target: None };
    };
    let view = compose_output(state, &scene, output_id, None);
    pick_roots(
        state,
        pointer_pos,
        view.entries
            .iter()
            .rev()
            .filter(|entry| {
                entry.participates_in_input
                    || (entry.participates_in_render
                        && state.root_surface_is_mapped(entry.root_surface))
            })
            .map(|entry| entry.root_surface),
    )
}

/// Global top-left of `surface`.
///
/// A subsurface adds its current offset to its parent. An xdg popup uses the
/// same origin as scene rendering (`surface_scene_origin`). Anything else uses
/// the surface's layout position.
pub(crate) fn surface_global_origin(
    state: &State,
    surface: Resolved<WlSurfaceId>,
) -> Option<(i32, i32)> {
    surface_global_origin_depth(state, surface, 0)
}

fn surface_global_origin_depth(
    state: &State,
    surface: Resolved<WlSurfaceId>,
    depth: u32,
) -> Option<(i32, i32)> {
    if depth > 32 {
        return None;
    }
    let (parent, dx, dy) = {
        let wl_surface = surface.try_ref(state)?;
        match wl_surface.parent_surface().copied() {
            Some(parent) => {
                let (dx, dy) = wl_surface.current_subsurface_position();
                (Some(parent), dx, dy)
            }
            None => (None, 0, 0),
        }
    };
    if let Some(parent) = parent {
        let (parent_x, parent_y) = surface_global_origin_depth(state, parent, depth + 1)?;
        return Some((parent_x.saturating_add(dx), parent_y.saturating_add(dy)));
    }
    state.surface_scene_origin(surface)
}

fn pick_roots(
    state: &State,
    pointer_pos: (f64, f64),
    roots: impl Iterator<Item = Resolved<WlSurfaceId>>,
) -> ScenePick {
    for root_surface in roots {
        let Some(surface) = root_surface.try_ref(state) else {
            continue;
        };
        if !surface.is_root()
            || surface.has_cursor_role()
            || surface.has_dnd_icon_role()
            || surface.has_lock_surface_role()
            || surface.has_input_popup_role()
        {
            continue;
        }
        let hit_pos = state.logical_pointer_for_surface(root_surface, pointer_pos);
        let root_hit = hit_test_subtree(state, root_surface, 0, 0, hit_pos.0, hit_pos.1);
        let has_mapped_bounds =
            WlSurface::local_surface_tree_bounds_from_ref(state, &surface).is_some();
        let window_target_for_root = has_mapped_bounds
            .then(|| state.committed_root_geometry(root_surface))
            .flatten()
            .filter(|geometry| rect_contains_point(geometry.window_target_rect(), hit_pos))
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

fn pointer_output(state: &State, pointer_pos: (f64, f64)) -> Option<crate::state::OutputId> {
    let x = pointer_pos.0.round() as i32;
    let y = pointer_pos.1.round() as i32;
    state
        .output_at_point(x, y)
        .filter(|output_id| state.output_bounds(*output_id).is_some())
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

fn pick_mapped_lock_surface(state: &State, pointer_pos: (f64, f64)) -> ScenePick {
    let Some(output_id) = pointer_output(state, pointer_pos) else {
        return ScenePick { surface_focus: None, window_target: None };
    };
    if !state.session_lock.shows_lock_surface(output_id) {
        return ScenePick { surface_focus: None, window_target: None };
    }
    let Some(lock_surface) = state.session_lock.surface_for_output(output_id) else {
        return ScenePick { surface_focus: None, window_target: None };
    };
    let Some(surface) = lock_surface
        .try_ref(state)
        .map(|lock_surface| lock_surface.wl_surface())
    else {
        return ScenePick { surface_focus: None, window_target: None };
    };
    let Some(bounds) = state.output_bounds(output_id) else {
        return ScenePick { surface_focus: None, window_target: None };
    };
    ScenePick {
        surface_focus: hit_test_lock_tree(
            state,
            surface,
            bounds.x,
            bounds.y,
            pointer_pos.0,
            pointer_pos.1,
        ),
        window_target: None,
    }
}

/// Lock surfaces are not scene roots. Walk the surface and its subsurfaces, and skip popups.
fn hit_test_lock_tree(
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
                if let Some(hit) = hit_test_lock_tree(
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
        protocol_generated::{
            xdg_toplevel::ResizeEdge, WlSubsurfaceId, WlSurfaceId, XdgPopupId, XdgSurfaceId,
            XdgToplevelId, XdgWmBaseId,
        },
        region::RectI,
        root_geometry::CommittedRootGeometry,
        state::ObjectId,
        test_support::state_for_tests,
        wayland_objects::{
            wl_subsurface::WlSubsurface,
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

    fn insert_pick_root(state: &mut crate::state::State, raw: u32) -> Resolved<WlSurfaceId> {
        let (_, object_id) =
            state.insert_object(WlSurface::new(WlSurfaceId::from(raw), ClientId::from_raw(1), 6));
        let surface = Resolved::new(ClientId::from_raw(1), WlSurfaceId::from(raw), object_id);
        state.register_root_surface(surface);
        surface
    }

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

        state.managed_root_surfaces.get_mut(&lower).unwrap().mapped = true;
        state.managed_root_surfaces.get_mut(&upper).unwrap().mapped = true;

        let pick = scene_pick_at(&state, (10.0, 10.0));
        assert_eq!(pick.surface_focus.map(|focus| focus.target.surface), Some(upper));
    }

    #[test]
    fn scene_pick_ignores_unmapped_workspace_roots_and_other_outputs() {
        let mut state = state_for_tests();
        let backend = crate::state::BackendOutputRef::new(crate::state::DrmId::from(0usize));
        let left = state.insert_output(crate::test_support::test_output_with_crtc_name(
            backend, 1, 0, 0, 100, 100,
        ));
        let right = state.insert_output(crate::test_support::test_output_with_crtc_name(
            backend, 2, 100, 0, 100, 100,
        ));
        let left_workspace = state.active_workspace_for_output(left).unwrap();
        let right_workspace = state.active_workspace_for_output(right).unwrap();
        let unmapped = insert_pick_root(&mut state, 81);
        let other_output = insert_pick_root(&mut state, 82);
        let _ = state.assign_root_surface_to_workspace(unmapped, left_workspace);
        let _ = state.assign_root_surface_to_workspace(other_output, right_workspace);
        state
            .managed_root_surfaces
            .get_mut(&other_output)
            .unwrap()
            .mapped = true;
        state
            .workspaces
            .try_get_mut(left_workspace)
            .unwrap()
            .layout
            .floating
            .stack = vec![unmapped];
        state
            .workspaces
            .try_get_mut(right_workspace)
            .unwrap()
            .layout
            .floating
            .stack = vec![other_output];
        state
            .objects
            .get_mut(unmapped.object_id())
            .downcast_mut::<WlSurface>()
            .unwrap()
            .test_map(ObjectId::from(810usize), 0, 0, 40, 40);
        state
            .objects
            .get_mut(other_output.object_id())
            .downcast_mut::<WlSurface>()
            .unwrap()
            .test_map(ObjectId::from(820usize), 100, 0, 40, 40);

        assert!(scene_pick_at(&state, (10.0, 10.0)).surface_focus.is_none());
        assert!(scene_pick_at(&state, (10.0, 10.0)).window_target.is_none());
        assert_eq!(
            scene_pick_at(&state, (110.0, 10.0))
                .surface_focus
                .map(|focus| focus.target.surface),
            Some(other_output)
        );
    }

    #[test]
    fn scene_pick_inherits_subsurface_without_making_it_a_scene_root() {
        let mut state = state_for_tests();
        let parent = insert_pick_root(&mut state, 91);
        let (_, child_object_id) =
            state.insert_object(WlSurface::new(WlSurfaceId::from(92), ClientId::from_raw(1), 6));
        let child = Resolved::new(ClientId::from_raw(1), WlSurfaceId::from(92), child_object_id);
        state
            .objects
            .get_mut(parent.object_id())
            .downcast_mut::<WlSurface>()
            .unwrap()
            .test_map(ObjectId::from(910usize), 0, 0, 20, 20);
        state
            .objects
            .get_mut(parent.object_id())
            .downcast_mut::<WlSurface>()
            .unwrap()
            .add_pending_child(child);
        let (_, subsurface_object_id) =
            state.insert_object(WlSubsurface::new(WlSubsurfaceId::from(93), child, parent));
        let subsurface =
            Resolved::new(ClientId::from_raw(1), WlSubsurfaceId::from(93), subsurface_object_id);
        {
            let child_surface = state
                .objects
                .get_mut(child.object_id())
                .downcast_mut::<WlSurface>()
                .unwrap();
            child_surface.assign_subsurface(subsurface, parent);
            child_surface.set_subsurface_position(30, 0);
            child_surface.test_map(ObjectId::from(920usize), 0, 0, 10, 10);
        }
        {
            let mut parent_surface = state.objects.grab_object::<WlSurface>(parent.object_id());
            assert!(parent_surface.apply_pending_tree_state(&mut state));
        }

        let scene = state.build_scene_model();
        assert!(scene
            .entries
            .iter()
            .all(|entry| entry.root_surface != child));
        assert_eq!(
            scene_pick_at(&state, (5.0, 5.0))
                .surface_focus
                .map(|focus| focus.target.surface),
            Some(parent)
        );
        assert_eq!(
            scene_pick_at(&state, (35.0, 5.0))
                .surface_focus
                .map(|focus| focus.target.surface),
            Some(child)
        );
    }

    fn floating_visual_root(
        state: &mut crate::state::State,
        output_bounds: RectI,
        logical: RectI,
        visual: RectI,
    ) -> (crate::state::OutputId, Resolved<WlSurfaceId>) {
        let backend = crate::state::BackendOutputRef::new(crate::state::DrmId::from(0usize));
        let output = state.insert_output(crate::test_support::test_output_with_crtc_name(
            backend,
            1,
            output_bounds.x,
            output_bounds.y,
            output_bounds.width,
            output_bounds.height,
        ));
        let workspace = state.active_workspace_for_output(output).unwrap();
        let surface = insert_pick_root(state, 91);
        state.assign_root_surface_to_workspace(surface, workspace);
        state
            .workspaces
            .try_get_mut(workspace)
            .unwrap()
            .layout
            .floating
            .stack = vec![surface];
        state
            .managed_root_surfaces
            .get_mut(&surface)
            .unwrap()
            .mapped = true;
        state
            .objects
            .get_mut(surface.object_id())
            .downcast_mut::<WlSurface>()
            .unwrap()
            .test_map(ObjectId::from(910usize), 0, 0, logical.width, logical.height);
        state
            .objects
            .get_mut(surface.object_id())
            .downcast_mut::<WlSurface>()
            .unwrap()
            .set_root_position(logical.x, logical.y);
        state.mark_root_geometry_dirty(surface);
        state
            .managed_root_surfaces
            .get_mut(&surface)
            .unwrap()
            .geometry_animation
            .set_goal(visual, crate::visual_animation::WindowGeometryMotion::Layout);
        state.advance_visual_animations(std::time::Instant::now());
        (output, surface)
    }

    #[test]
    fn visual_box_receives_pointer_hits_and_resize_edges() {
        let mut state = state_for_tests();
        let logical = RectI::new(0, 0, 90, 90).unwrap();
        let visual = RectI::new(200, 0, 90, 90).unwrap();
        let (_, surface) =
            floating_visual_root(&mut state, RectI::new(0, 0, 400, 100).unwrap(), logical, visual);

        assert_eq!(
            scene_pick_at(&state, (230.0, 45.0))
                .surface_focus
                .map(|focus| focus.target.surface),
            Some(surface)
        );
        assert!(scene_pick_at(&state, (20.0, 45.0)).surface_focus.is_none());

        let hit = crate::input::window_hit_at(&state, (270.0, 45.0)).unwrap();
        assert_eq!(hit.binding_resize_edges, ResizeEdge::Right as u32);
    }

    #[test]
    fn visual_box_is_clickable_on_a_neighbor_output() {
        let mut state = state_for_tests();
        let logical = RectI::new(10, 10, 40, 40).unwrap();
        let visual = RectI::new(120, 10, 40, 40).unwrap();
        let (left, surface) =
            floating_visual_root(&mut state, RectI::new(0, 0, 100, 100).unwrap(), logical, visual);
        let backend = crate::state::BackendOutputRef::new(crate::state::DrmId::from(0usize));
        let right = state.insert_output(crate::test_support::test_output_with_crtc_name(
            backend, 2, 100, 0, 100, 100,
        ));

        assert_eq!(
            scene_pick_at(&state, (130.0, 20.0))
                .surface_focus
                .map(|focus| focus.target.surface),
            Some(surface)
        );
        let scene = crate::scene::build_scene_model(&state);
        let right_entry = crate::scene::compose_output(&state, &scene, right, None)
            .entries
            .into_iter()
            .find(|entry| entry.root_surface == surface)
            .expect("neighbor output draws the visual box");
        assert!(!right_entry.participates_in_protocol_visibility);
        assert!(right_entry.participates_in_render);
        let left_entry = crate::scene::compose_output(&state, &scene, left, None)
            .entries
            .into_iter()
            .find(|entry| entry.root_surface == surface)
            .expect("owning output keeps the window");
        assert!(left_entry.participates_in_protocol_visibility);
    }

    #[test]
    fn direct_move_keeps_the_grabbed_point_under_the_cursor() {
        let mut state = state_for_tests();
        let logical = RectI::new(0, 0, 100, 80).unwrap();
        let visual = RectI::new(100, 0, 100, 80).unwrap();
        let (_, surface) =
            floating_visual_root(&mut state, RectI::new(0, 0, 300, 100).unwrap(), logical, visual);
        state.seat.pointer_pos = (150.0, 40.0);
        let target = super::build_window_target(&state, surface).unwrap();
        let mut dispatcher = crate::runner::Dispatcher::default();
        crate::input::apply_interaction_intent(
            &mut state,
            &mut dispatcher,
            crate::input::InteractionIntent::StartMove(target),
            crate::input::PROTOCOL_INTERACTION_START_OPTIONS,
        );

        let position = state
            .objects
            .get(surface.object_id())
            .downcast_ref::<WlSurface>()
            .unwrap()
            .root_position();
        assert_eq!(position, (100, 0));
        state.advance_visual_animations(std::time::Instant::now());
        let visual_now = state
            .managed_root_surfaces
            .get(&surface)
            .unwrap()
            .geometry_animation
            .visual_rect()
            .unwrap();
        assert_eq!(visual_now.x, 100);
        assert_eq!(visual_now.y, 0);
    }

    #[test]
    fn visual_hit_follows_a_move_until_it_settles() {
        use std::time::{Duration, Instant};

        let mut state = state_for_tests();
        let logical = RectI::new(0, 0, 100, 100).unwrap();
        let (_, surface) =
            floating_visual_root(&mut state, RectI::new(0, 0, 400, 120).unwrap(), logical, logical);
        state
            .config
            .animations
            .set_duration_msec(crate::animation_config::AnimationNode::Windows, 200);
        state
            .config
            .animations
            .set_linear(crate::animation_config::AnimationNode::Windows);
        state
            .config
            .animations
            .set_duration_msec(crate::animation_config::AnimationNode::WindowsMove, 200);
        state
            .config
            .animations
            .set_linear(crate::animation_config::AnimationNode::WindowsMove);
        let goal = RectI::new(200, 0, 100, 100).unwrap();
        state
            .managed_root_surfaces
            .get_mut(&surface)
            .unwrap()
            .geometry_animation
            .set_goal(goal, crate::visual_animation::WindowGeometryMotion::Layout);
        let now = Instant::now();
        state.advance_visual_animations(now);
        state.advance_visual_animations(now + Duration::from_millis(100));
        let midway = state
            .managed_root_surfaces
            .get(&surface)
            .unwrap()
            .geometry_animation
            .visual_rect()
            .unwrap();
        assert!(midway.x > logical.x && midway.x < goal.x);
        assert_eq!(
            scene_pick_at(&state, (midway.x as f64 + 10.0, 10.0))
                .surface_focus
                .map(|focus| focus.target.surface),
            Some(surface)
        );
        assert!(scene_pick_at(&state, (10.0, 10.0)).surface_focus.is_none());

        state.advance_visual_animations(now + Duration::from_millis(200));
        assert_eq!(
            state
                .managed_root_surfaces
                .get(&surface)
                .unwrap()
                .geometry_animation
                .visual_rect(),
            Some(goal)
        );
        assert_eq!(
            scene_pick_at(&state, (210.0, 10.0))
                .surface_focus
                .map(|focus| focus.target.surface),
            Some(surface)
        );
        assert!(scene_pick_at(&state, (10.0, 10.0)).surface_focus.is_none());
    }

    #[test]
    fn popup_hit_follows_the_parent_visual_box() {
        let mut state = state_for_tests();
        let client_id = ClientId::from_raw(7);
        let (parent_wl, parent_xdg) = insert_test_xdg_surface(
            &mut state,
            client_id,
            WlSurfaceId::from(161),
            XdgSurfaceId::from(162),
        );
        let (popup_wl, popup_xdg) = insert_test_xdg_surface(
            &mut state,
            client_id,
            WlSurfaceId::from(163),
            XdgSurfaceId::from(164),
        );
        state
            .objects
            .get_mut(parent_wl.object_id())
            .downcast_mut::<WlSurface>()
            .unwrap()
            .test_map(ObjectId::from(1401usize), 640, 0, 640, 800);
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
            .test_map(ObjectId::from(1402usize), 0, 0, 299, 457);

        let (_, popup_object_id) = state.insert_object(XdgPopup::new(
            XdgPopupId::from(165),
            popup_xdg,
            Some(parent_xdg),
            PositionerState::default(),
            7,
        ));
        let popup = Resolved::new(client_id, XdgPopupId::from(165), popup_object_id);
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

        let backend = crate::state::BackendOutputRef::new(crate::state::DrmId::from(0usize));
        let output = state.insert_output(crate::test_support::test_output_with_crtc_name(
            backend, 1, 0, 0, 2000, 1200,
        ));
        let workspace = state.active_workspace_for_output(output).unwrap();
        state.assign_root_surface_to_workspace(parent_wl, workspace);
        state
            .workspaces
            .try_get_mut(workspace)
            .unwrap()
            .layout
            .floating
            .stack = vec![parent_wl];
        state
            .managed_root_surfaces
            .get_mut(&parent_wl)
            .unwrap()
            .mapped = true;

        let layout = state
            .committed_root_geometry(parent_wl)
            .unwrap()
            .layout_rect_global;
        let shifted = RectI { x: layout.x + 80, ..layout };
        state
            .managed_root_surfaces
            .get_mut(&parent_wl)
            .unwrap()
            .geometry_animation
            .set_goal(shifted, crate::visual_animation::WindowGeometryMotion::Layout);
        state.advance_visual_animations(std::time::Instant::now());

        assert_eq!(
            scene_pick_at(&state, (1090.0, 25.0))
                .surface_focus
                .map(|focus| focus.target.surface),
            Some(popup_wl)
        );
        assert_ne!(
            scene_pick_at(&state, (1010.0, 25.0))
                .surface_focus
                .map(|focus| focus.target.surface),
            Some(popup_wl)
        );
    }
}
