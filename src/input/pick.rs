use crate::{
    clients::Resolved,
    protocol_generated::{xdg_toplevel::ResizeEdge, WlSurfaceId, XdgSurfaceId},
    region::RectI,
    state::State,
    wayland_objects::{
        wl_surface::{Role, StackEntry},
        zwlr_layer_surface_v1::ordered_scene_root_surfaces,
    },
};

use super::{
    ScenePick, SurfaceFocus, SurfaceFocusPick, SurfaceRect, WindowRoleSnapshot, WindowSnapshot,
    WindowTarget,
};

pub(crate) fn scene_pick_at(state: &State, pointer_pos: (f64, f64)) -> ScenePick {
    let mut surface_focus = None;
    let mut window_target = None;

    for root_surface in ordered_scene_root_surfaces(state) {
        let Some(surface) = root_surface.try_ref(state) else {
            continue;
        };
        if !surface.is_root() || surface.has_cursor_role() || surface.has_dnd_icon_role() {
            continue;
        }
        if let Some(root_hit) =
            hit_test_subtree(state, root_surface, 0, 0, pointer_pos.0, pointer_pos.1)
        {
            surface_focus = Some(root_hit);
        }
        let Some(interactive_rect) = state.root_surface_interactive_rect(root_surface) else {
            continue;
        };
        if rect_contains_point(interactive_rect, pointer_pos) {
            window_target = build_window_target(state, root_surface, interactive_rect);
        }
    }

    ScenePick { surface_focus, window_target }
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
    let interactive_rect = state.root_surface_interactive_rect(root_surface)?;
    build_window_target(state, root_surface, interactive_rect)
}

pub(crate) fn build_protocol_window_target(
    state: &State,
    root_surface: Resolved<WlSurfaceId>,
    role: WindowRoleSnapshot,
) -> Option<WindowTarget> {
    let surface = root_surface.try_ref(state)?;
    let content_rect = surface
        .global_logical_bounds()
        .or_else(|| state.root_surface_content_rect(root_surface))?;
    let interactive_rect = content_rect;
    let outer_rect = content_rect;
    Some(WindowSnapshot {
        root_surface,
        role,
        initial_root_pos: surface.root_position(),
        initial_size: surface
            .logical_size()
            .map(|size| (size.width, size.height))?,
        content_rect,
        interactive_rect,
        outer_rect,
    })
}

pub(crate) fn build_window_target(
    state: &State,
    root_surface: Resolved<WlSurfaceId>,
    interactive_rect: RectI,
) -> Option<WindowTarget> {
    let surface = root_surface.try_ref(state)?;
    let content_rect = state.root_surface_content_rect(root_surface)?;
    let outer_rect = state
        .root_surface_outer_rect(root_surface)
        .unwrap_or(interactive_rect);
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
        initial_root_pos: surface.root_position(),
        initial_size: surface
            .logical_size()
            .map(|size| (size.width, size.height))?,
        content_rect,
        interactive_rect,
        outer_rect,
    })
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
