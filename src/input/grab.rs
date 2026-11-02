use std::collections::BTreeSet;

use crate::{
    clients::{ClientAction, ClientId, ClientManagerAction, Resolved},
    cursor::CursorSourceRef,
    drm::{request_active_output, request_outputs_for_points, request_outputs_for_rects},
    protocol_generated::{xdg_toplevel::ResizeEdge, WlSurfaceId, XdgToplevelId},
    region::RectI,
    root_geometry::{CommittedRootGeometry, WindowTargetGeometry},
    runner::Dispatcher,
    seat::{dispatch_client_action, dispatch_client_event, refresh_pointer_focus, SeatClientEvent},
    state::State,
    tiling::ExtractedTiledLeaf,
    wayland_objects::xdg_surface::xdg_toplevel::InteractiveSizeConstraints,
};

use super::{
    set_keyboard_focus, ActiveGrabState, InteractionIntent, InteractionStartOptions, SurfaceFocus,
    WindowRoleSnapshot, WindowTarget,
};

const POINTER_MOVE_CURSOR_ICON_NAMES: &[&str] = &["grab", "grabbing", "move"];
const POINTER_RESIZE_NW_CURSOR_ICON_NAMES: &[&str] = &["nw-resize", "size_fdiag"];
const POINTER_RESIZE_NE_CURSOR_ICON_NAMES: &[&str] = &["ne-resize", "size_bdiag"];
const POINTER_RESIZE_SW_CURSOR_ICON_NAMES: &[&str] = &["sw-resize", "size_bdiag"];
const POINTER_RESIZE_SE_CURSOR_ICON_NAMES: &[&str] = &["se-resize", "size_fdiag"];
const POINTER_RESIZE_N_CURSOR_ICON_NAMES: &[&str] = &["n-resize", "size_ver"];
const POINTER_RESIZE_S_CURSOR_ICON_NAMES: &[&str] = &["s-resize", "size_ver"];
const POINTER_RESIZE_W_CURSOR_ICON_NAMES: &[&str] = &["w-resize", "size_hor"];
const POINTER_RESIZE_E_CURSOR_ICON_NAMES: &[&str] = &["e-resize", "size_hor"];

pub(crate) fn apply_interaction_intent(
    state: &mut State,
    dispatcher: &mut Dispatcher,
    intent: InteractionIntent,
    options: InteractionStartOptions,
) -> bool {
    match intent {
        InteractionIntent::FocusSurface(focus) => {
            set_keyboard_focus(state, dispatcher, Some(focus));
            true
        }
        InteractionIntent::StartMove(target) | InteractionIntent::StartResize(target, _) => {
            let target = if options.prepare_window {
                prepare_window_for_pointer_grab(state, dispatcher, &target)
            } else {
                target
            };
            if options.prepare_window
                && matches!(intent, InteractionIntent::StartMove(_))
                && state.is_tiled_root_surface(target.root_surface)
            {
                if let WindowRoleSnapshot::XdgToplevel {
                    xdg_toplevel,
                    size_constraints,
                    decoration_mode,
                    ..
                } = target.role
                {
                    return start_tiled_drag(
                        state,
                        dispatcher,
                        target.root_surface,
                        xdg_toplevel,
                        size_constraints,
                        decoration_mode,
                        target.geometry,
                    );
                }
            }
            let target = anchored_grab_target(state, target);
            start_window_grab(state, dispatcher, &target, intent)
        }
        InteractionIntent::DismissPopupGrab | InteractionIntent::None => false,
    }
}

pub(crate) fn apply_pointer_interactive_grab(
    state: &mut State,
    dispatcher: &mut Dispatcher,
) -> bool {
    let Some(grab) = state.seat.pointer_interactive_grab else {
        return false;
    };
    match grab {
        ActiveGrabState::Move { root_surface, initial_pointer_pos, initial_geometry, .. } => {
            move_grabbed_root(
                state,
                dispatcher,
                root_surface,
                initial_pointer_pos,
                initial_geometry.root_pos,
            );
        }
        ActiveGrabState::MoveTiled {
            root_surface,
            xdg_toplevel,
            size_constraints,
            decoration_mode,
            origin_pointer,
            threshold,
            picked_up: false,
            ..
        } => {
            if !tiled_drag_threshold_reached(origin_pointer, state.seat.pointer_pos, threshold) {
                return true;
            }
            pickup_tiled_drag(
                state,
                dispatcher,
                root_surface,
                xdg_toplevel,
                size_constraints,
                decoration_mode,
            );
        }
        ActiveGrabState::MoveTiled {
            root_surface,
            picked_up: true,
            preview_size,
            frame_from_root,
            ..
        } => {
            // The pickup animation plays only while the pointer stays put. Motion samples the
            // pointer-centered preview and warps to it, matching Hyprland's warpPositionSize.
            let root_pos =
                preview_root_for_pointer(state.seat.pointer_pos, preview_size, frame_from_root);
            place_grabbed_root(state, dispatcher, root_surface, root_pos);
        }
        ActiveGrabState::Resize {
            root_surface,
            xdg_toplevel,
            size_constraints,
            edges,
            initial_pointer_pos,
            initial_geometry,
            mut last_size,
        } => {
            let dx = (state.seat.pointer_pos.0 - initial_pointer_pos.0).round() as i32;
            let dy = (state.seat.pointer_pos.1 - initial_pointer_pos.1).round() as i32;
            let (mut width, mut height) = initial_geometry.resize_size;
            let (mut x, mut y) = initial_geometry.root_pos;

            if edges & 4 != 0 {
                width = initial_geometry.resize_size.0 - dx;
            }
            if edges & 8 != 0 {
                width = initial_geometry.resize_size.0 + dx;
            }
            if edges & 1 != 0 {
                height = initial_geometry.resize_size.1 - dy;
            }
            if edges & 2 != 0 {
                height = initial_geometry.resize_size.1 + dy;
            }
            (width, height) = size_constraints.clamp(width, height);
            if edges & 4 != 0 {
                x = initial_geometry.root_pos.0 + (initial_geometry.resize_size.0 - width);
            }
            if edges & 1 != 0 {
                y = initial_geometry.root_pos.1 + (initial_geometry.resize_size.1 - height);
            }

            let old_bounds = interactive_damage_rect(state, root_surface);
            let Some(mut surface) = root_surface.try_grab(state) else {
                abort_pointer_interactive_grab(state, dispatcher, 0);
                return true;
            };
            surface.set_root_position(x, y);
            drop(surface);
            state.mark_root_geometry_dirty(root_surface);
            state.sync_window_geometry_goal(
                root_surface,
                crate::visual_animation::WindowGeometryMotion::Warp,
            );
            let new_bounds = interactive_damage_rect(state, root_surface);
            dispatch_client_action(
                dispatcher,
                root_surface.client_id(),
                ClientAction::RefreshSurfaceOutputs,
            );
            if let Some(new_bounds) = new_bounds {
                let rects = old_bounds.into_iter().chain(std::iter::once(new_bounds));
                request_outputs_for_rects(state, rects, dispatcher);
            } else {
                request_active_output(state, dispatcher);
            }

            if last_size != (width, height) {
                dispatch_client_action(
                    dispatcher,
                    root_surface.client_id(),
                    ClientAction::InteractiveResizeConfigure {
                        xdg_toplevel,
                        width,
                        height,
                        resizing: true,
                    },
                );
                last_size = (width, height);
            }

            state.seat.pointer_interactive_grab = Some(ActiveGrabState::Resize {
                root_surface,
                xdg_toplevel,
                size_constraints,
                edges,
                initial_pointer_pos,
                initial_geometry,
                last_size,
            });
        }
        ActiveGrabState::ResizeTiled { root_surface, edges, initial_pointer_pos, resize_state } => {
            let dx = (state.seat.pointer_pos.0 - initial_pointer_pos.0).round() as i32;
            let dy = (state.seat.pointer_pos.1 - initial_pointer_pos.1).round() as i32;
            if state.resize_tiled_root_surface_from_state(root_surface, resize_state, dx, dy) {
                dispatcher.dispatch(ClientManagerAction::WorkspaceStateChanged);
                request_active_output(state, dispatcher);
            }
            state.seat.pointer_interactive_grab = Some(ActiveGrabState::ResizeTiled {
                root_surface,
                edges,
                initial_pointer_pos,
                resize_state,
            });
        }
    }
    true
}

pub(crate) fn finish_pointer_interactive_grab(
    state: &mut State,
    dispatcher: &mut Dispatcher,
    time: u32,
) {
    let _ = settle_pointer_interactive_grab(state, dispatcher, time, false);
}

pub(crate) fn abort_pointer_interactive_grab(
    state: &mut State,
    dispatcher: &mut Dispatcher,
    time: u32,
) -> bool {
    settle_pointer_interactive_grab(state, dispatcher, time, true)
}

fn settle_pointer_interactive_grab(
    state: &mut State,
    dispatcher: &mut Dispatcher,
    time: u32,
    cancel: bool,
) -> bool {
    let Some(grab) = state.seat.pointer_interactive_grab else {
        return false;
    };
    if !cancel {
        if let ActiveGrabState::Resize { root_surface, xdg_toplevel, last_size, .. } = grab {
            dispatch_client_action(
                dispatcher,
                root_surface.client_id(),
                ClientAction::InteractiveResizeConfigure {
                    xdg_toplevel,
                    width: last_size.0,
                    height: last_size.1,
                    resizing: false,
                },
            );
        }
    }
    let origin = state.seat.tiled_drag_origin.take();
    let was_tiled_resize = matches!(grab, ActiveGrabState::ResizeTiled { .. });
    state.seat.clear_pointer_interactive_grab();
    if let ActiveGrabState::MoveTiled { root_surface, picked_up, .. } = grab {
        settle_tiled_drag(state, dispatcher, root_surface, picked_up, origin, cancel);
    }
    if was_tiled_resize {
        dispatcher.dispatch(ClientManagerAction::WorkspaceStateChanged);
    }
    end_compositor_pointer_grab(state, dispatcher, time);
    true
}

fn settle_tiled_drag(
    state: &mut State,
    dispatcher: &mut Dispatcher,
    root_surface: Resolved<WlSurfaceId>,
    picked_up: bool,
    origin: Option<ExtractedTiledLeaf>,
    cancel: bool,
) {
    if !picked_up {
        return;
    }
    let mapped = root_surface
        .try_ref(state)
        .is_some_and(|surface| surface.has_committed_buffer());
    if mapped && !cancel {
        let placed = place_dragged_window(state, root_surface);
        if !placed {
            restore_tiled_drag_origin(state, root_surface, origin);
        }
    } else if mapped {
        restore_tiled_drag_origin(state, root_surface, origin);
    }
    let retiled = state.retile_root_surface_after_drag(root_surface);
    if retiled {
        dispatcher.dispatch(ClientManagerAction::WorkspaceStateChanged);
    }
    if cancel && mapped && retiled {
        // Cancel, session lock, and session pause put the window back on its tile immediately.
        // A normal drop leaves the windows animation that retile just recorded.
        state.sync_window_geometry_goal(
            root_surface,
            crate::visual_animation::WindowGeometryMotion::Warp,
        );
    }
}

fn place_dragged_window(state: &mut State, surface: Resolved<WlSurfaceId>) -> bool {
    let (pointer_x, pointer_y) = state.seat.pointer_pos;
    let precise_mouse_move = state.config.tiling.drag.precise_mouse_move;
    let split_ratio = state.config.tiling.drag.split_ratio.get();
    let Some(workspace_id) = state.root_surface_workspace(surface) else {
        return false;
    };
    let Some(workspace) = state.workspaces.try_get_mut(workspace_id) else {
        return false;
    };
    workspace.layout.place_dragged_tiled_leaf(
        surface,
        pointer_x,
        pointer_y,
        precise_mouse_move,
        split_ratio,
    )
}

fn restore_tiled_drag_origin(
    state: &mut State,
    surface: Resolved<WlSurfaceId>,
    origin: Option<ExtractedTiledLeaf>,
) {
    let Some(origin) = origin else {
        return;
    };
    let Some(workspace_id) = state.root_surface_workspace(surface) else {
        return;
    };
    if let Some(workspace) = state.workspaces.try_get_mut(workspace_id) {
        workspace.layout.restore_extracted_tiled_leaf(origin);
    }
}

pub(crate) fn tiled_drag_tracks_surface(state: &State, surface: Resolved<WlSurfaceId>) -> bool {
    matches!(
        state.seat.pointer_interactive_grab,
        Some(ActiveGrabState::MoveTiled { root_surface, .. }) if root_surface == surface
    )
}

pub(crate) fn tiled_resize_active_root_surfaces(state: &State) -> BTreeSet<Resolved<WlSurfaceId>> {
    let Some(ActiveGrabState::ResizeTiled { root_surface, resize_state, .. }) =
        state.seat.pointer_interactive_grab
    else {
        return BTreeSet::new();
    };
    let Some(workspace_id) = state.root_surface_workspace(root_surface) else {
        return BTreeSet::new();
    };
    let Some(workspace) = state.workspaces.try_get(workspace_id) else {
        return BTreeSet::new();
    };
    workspace
        .layout
        .resize_affected_surfaces(root_surface, resize_state)
        .unwrap_or_default()
}

pub(crate) fn recover_pointer_after_interactive_grab_loss(
    state: &mut State,
    dispatcher: &mut Dispatcher,
    time: u32,
) {
    end_compositor_pointer_grab(state, dispatcher, time);
}

pub(crate) fn resize_cursor_icon_names(edges: u32) -> &'static [&'static str] {
    match edges {
        e if e == ResizeEdge::Top as u32 => POINTER_RESIZE_N_CURSOR_ICON_NAMES,
        e if e == ResizeEdge::Bottom as u32 => POINTER_RESIZE_S_CURSOR_ICON_NAMES,
        e if e == ResizeEdge::Left as u32 => POINTER_RESIZE_W_CURSOR_ICON_NAMES,
        e if e == ResizeEdge::Right as u32 => POINTER_RESIZE_E_CURSOR_ICON_NAMES,
        e if e == ResizeEdge::TopLeft as u32 => POINTER_RESIZE_NW_CURSOR_ICON_NAMES,
        e if e == ResizeEdge::TopRight as u32 => POINTER_RESIZE_NE_CURSOR_ICON_NAMES,
        e if e == ResizeEdge::BottomLeft as u32 => POINTER_RESIZE_SW_CURSOR_ICON_NAMES,
        e if e == ResizeEdge::BottomRight as u32 => POINTER_RESIZE_SE_CURSOR_ICON_NAMES,
        _ => POINTER_RESIZE_SE_CURSOR_ICON_NAMES,
    }
}

/// Place the logical window so the point under the cursor stays put, then snap the animation.
fn anchored_grab_target(state: &mut State, mut target: WindowTarget) -> WindowTarget {
    if state.is_tiled_root_surface(target.root_surface) {
        return target;
    }
    let pointer = state.seat.pointer_position();
    let logical = state
        .root_geometry_cache
        .get(&target.root_surface)
        .and_then(|entry| entry.get().geometry)
        .map(|geometry| geometry.layout_rect_global)
        .unwrap_or(target.geometry.target_rect);
    let Some(visual) = state
        .managed_root_surfaces
        .get(&target.root_surface)
        .and_then(|managed| managed.geometry_animation.visual_rect())
    else {
        return target;
    };
    if visual == logical {
        return target;
    }
    let logical_point = crate::render::unmap_layout_point(logical, visual, pointer);
    let dx = (pointer.0 - logical_point.0).round() as i32;
    let dy = (pointer.1 - logical_point.1).round() as i32;
    if dx != 0 || dy != 0 {
        let Some(mut surface) = target.root_surface.try_grab(state) else {
            return target;
        };
        let position = surface.root_position();
        surface.set_root_position(position.0 + dx, position.1 + dy);
        drop(surface);
        state.mark_root_geometry_dirty(target.root_surface);
        target.geometry.root_pos.0 += dx;
        target.geometry.root_pos.1 += dy;
        target.geometry.target_rect.x += dx;
        target.geometry.target_rect.y += dy;
        target.geometry.binding_rect.x += dx;
        target.geometry.binding_rect.y += dy;
    }
    let Some(goal) = RectI::new(logical.x + dx, logical.y + dy, logical.width, logical.height)
    else {
        return target;
    };
    if let Some(managed) = state.managed_root_surfaces.get_mut(&target.root_surface) {
        managed
            .geometry_animation
            .set_goal(goal, crate::visual_animation::WindowGeometryMotion::Warp);
    }
    target
}

fn prepare_window_for_pointer_grab(
    state: &mut State,
    dispatcher: &mut Dispatcher,
    target: &WindowTarget,
) -> WindowTarget {
    let target = *target;
    state.raise_root_surface(target.root_surface);
    request_outputs_for_rects(
        state,
        [interactive_damage_rect(state, target.root_surface)
            .unwrap_or(target.geometry.target_rect)],
        dispatcher,
    );
    set_keyboard_focus(state, dispatcher, Some(SurfaceFocus::new(target.root_surface)));
    target
}

fn start_window_grab(
    state: &mut State,
    dispatcher: &mut Dispatcher,
    target: &WindowTarget,
    intent: InteractionIntent,
) -> bool {
    match (intent, target.role) {
        (InteractionIntent::StartMove(_), _) => start_interactive_move(
            state,
            dispatcher,
            target.client_id(),
            target.root_surface,
            target.geometry,
        ),
        (
            InteractionIntent::StartResize(_, edges),
            WindowRoleSnapshot::XdgToplevel { xdg_toplevel, size_constraints, .. },
        ) => {
            if state.is_tiled_root_surface(target.root_surface) {
                start_interactive_tiled_resize(
                    state,
                    dispatcher,
                    target.client_id(),
                    target.root_surface,
                    edges,
                )
            } else {
                start_interactive_resize(
                    state,
                    dispatcher,
                    target.client_id(),
                    target.root_surface,
                    xdg_toplevel,
                    size_constraints,
                    edges,
                    target.geometry,
                )
            }
        }
        _ => false,
    }
}

const TILED_DRAG_PREVIEW_SCALE: f64 = 0.8489;

fn start_tiled_drag(
    state: &mut State,
    dispatcher: &mut Dispatcher,
    root_surface: Resolved<WlSurfaceId>,
    xdg_toplevel: Resolved<XdgToplevelId>,
    size_constraints: InteractiveSizeConstraints,
    decoration_mode: crate::wayland_objects::xdg_surface::xdg_toplevel::ToplevelDecorationMode,
    initial_geometry: WindowTargetGeometry,
) -> bool {
    begin_compositor_pointer_grab(state, dispatcher, POINTER_MOVE_CURSOR_ICON_NAMES);
    let pointer = state.seat.pointer_pos;
    let threshold = state.config.tiling.drag.threshold;
    state.seat.pointer_interactive_grab = Some(ActiveGrabState::MoveTiled {
        root_surface,
        xdg_toplevel,
        size_constraints,
        decoration_mode,
        origin_pointer: pointer,
        threshold,
        picked_up: false,
        grab_pointer: pointer,
        grab_geometry: initial_geometry,
        preview_size: (0, 0),
        frame_from_root: (0, 0),
    });
    if threshold <= 0 {
        pickup_tiled_drag(
            state,
            dispatcher,
            root_surface,
            xdg_toplevel,
            size_constraints,
            decoration_mode,
        );
    }
    true
}

fn tiled_drag_threshold_reached(origin: (f64, f64), pointer: (f64, f64), threshold: i32) -> bool {
    let threshold = threshold.max(0) as f64;
    let dx = pointer.0 - origin.0;
    let dy = pointer.1 - origin.1;
    dx * dx + dy * dy > threshold * threshold
}

fn pickup_tiled_drag(
    state: &mut State,
    dispatcher: &mut Dispatcher,
    root_surface: Resolved<WlSurfaceId>,
    xdg_toplevel: Resolved<XdgToplevelId>,
    size_constraints: InteractiveSizeConstraints,
    decoration_mode: crate::wayland_objects::xdg_surface::xdg_toplevel::ToplevelDecorationMode,
) {
    let Some(ActiveGrabState::MoveTiled { picked_up: false, grab_geometry, .. }) =
        state.seat.pointer_interactive_grab
    else {
        return;
    };
    let Some(geometry) = state.committed_root_geometry(root_surface) else {
        abort_pointer_interactive_grab(state, dispatcher, 0);
        return;
    };
    let pointer = state.seat.pointer_pos;
    let mut frame = scaled_tiled_preview_frame(geometry.layout_rect_global, pointer);
    let Some(mut built) = root_surface
        .try_ref(state)
        .and_then(|surface| preview_geometry(state, root_surface, &surface, frame))
    else {
        abort_pointer_interactive_grab(state, dispatcher, 0);
        return;
    };
    let configure = crate::root_geometry::project_configure_geometry(
        built,
        decoration_mode,
        state.configured_decoration_insets(),
    )
    .size;
    let clamped = size_constraints.clamp(configure.0, configure.1);
    if clamped != configure {
        frame.width = frame.width.saturating_add(clamped.0 - configure.0).max(1);
        frame.height = frame.height.saturating_add(clamped.1 - configure.1).max(1);
        frame.x = (pointer.0 - frame.width as f64 / 2.0).round() as i32;
        frame.y = (pointer.1 - frame.height as f64 / 2.0).round() as i32;
        let Some(rebuilt) = root_surface
            .try_ref(state)
            .and_then(|surface| preview_geometry(state, root_surface, &surface, frame))
        else {
            abort_pointer_interactive_grab(state, dispatcher, 0);
            return;
        };
        built = rebuilt;
    }
    let frame_from_root = (frame.x - built.root_pos.0, frame.y - built.root_pos.1);
    let preview_size = (frame.width, frame.height);
    let root_pos = preview_root_for_pointer(pointer, preview_size, frame_from_root);
    state.seat.tiled_drag_origin = state
        .root_surface_workspace(root_surface)
        .and_then(|workspace_id| state.workspaces.try_get(workspace_id))
        .and_then(|workspace| workspace.layout.capture_tiled_drag_origin(root_surface));
    if !state.float_tiled_root_surface_for_drag(root_surface) {
        abort_pointer_interactive_grab(state, dispatcher, 0);
        return;
    }
    let mut grab_geometry = grab_geometry;
    grab_geometry.root_pos = root_pos;
    state.seat.pointer_interactive_grab = Some(ActiveGrabState::MoveTiled {
        root_surface,
        xdg_toplevel,
        size_constraints,
        decoration_mode,
        origin_pointer: pointer,
        threshold: state.config.tiling.drag.threshold,
        picked_up: true,
        grab_pointer: pointer,
        grab_geometry,
        preview_size,
        frame_from_root,
    });
    let old_bounds = interactive_damage_rect(state, root_surface);
    let Some(mut surface) = root_surface.try_grab(state) else {
        abort_pointer_interactive_grab(state, dispatcher, 0);
        return;
    };
    surface.set_root_position(root_pos.0, root_pos.1);
    drop(surface);
    state.mark_root_geometry_dirty(root_surface);
    // Tile to preview uses the windows animation. Hyprland does the same by assigning
    // m_realPosition and m_realSize when the tiled window becomes floating.
    state.sync_window_geometry_goal(
        root_surface,
        crate::visual_animation::WindowGeometryMotion::Layout,
    );
    dispatch_client_action(
        dispatcher,
        root_surface.client_id(),
        ClientAction::InteractiveResizeConfigure {
            xdg_toplevel,
            width: clamped.0,
            height: clamped.1,
            resizing: false,
        },
    );
    dispatcher.dispatch(ClientManagerAction::WorkspaceStateChanged);
    let new_bounds = interactive_damage_rect(state, root_surface);
    if let Some(new_bounds) = new_bounds {
        request_outputs_for_rects(
            state,
            old_bounds.into_iter().chain(std::iter::once(new_bounds)),
            dispatcher,
        );
    } else {
        request_active_output(state, dispatcher);
    }
}

/// Places the preview so its center is the pointer. The frame may extend past the
/// source output. Pointer motion is already limited to the output layout.
fn preview_root_for_pointer(
    pointer: (f64, f64),
    preview_size: (i32, i32),
    frame_from_root: (i32, i32),
) -> (i32, i32) {
    let frame_x = (pointer.0 - preview_size.0 as f64 / 2.0).round() as i32;
    let frame_y = (pointer.1 - preview_size.1 as f64 / 2.0).round() as i32;
    (frame_x.saturating_sub(frame_from_root.0), frame_y.saturating_sub(frame_from_root.1))
}

fn scaled_tiled_preview_frame(
    tile: crate::region::RectI,
    pointer: (f64, f64),
) -> crate::region::RectI {
    let width = ((tile.width.max(0) as f64) * TILED_DRAG_PREVIEW_SCALE)
        .floor()
        .max(1.0) as i32;
    let height = ((tile.height.max(0) as f64) * TILED_DRAG_PREVIEW_SCALE)
        .floor()
        .max(1.0) as i32;
    crate::region::RectI {
        x: (pointer.0 - width as f64 / 2.0).round() as i32,
        y: (pointer.1 - height as f64 / 2.0).round() as i32,
        width,
        height,
    }
}

fn preview_geometry(
    state: &State,
    root_surface: Resolved<WlSurfaceId>,
    surface: &crate::wayland_objects::wl_surface::WlSurface,
    frame: crate::region::RectI,
) -> Option<CommittedRootGeometry> {
    crate::root_geometry::build_committed_root_geometry(
        state,
        root_surface,
        surface,
        Some(crate::tiling::LayoutAllocation {
            frame_rect: frame,
            tiled_edges: 0,
            fullscreen: false,
        }),
        crate::root_geometry::GeometryBuildOverrides::default(),
    )
}

fn move_grabbed_root(
    state: &mut State,
    dispatcher: &mut Dispatcher,
    root_surface: Resolved<WlSurfaceId>,
    initial_pointer_pos: (f64, f64),
    initial_root_pos: (i32, i32),
) {
    let dx = (state.seat.pointer_pos.0 - initial_pointer_pos.0).round() as i32;
    let dy = (state.seat.pointer_pos.1 - initial_pointer_pos.1).round() as i32;
    place_grabbed_root(
        state,
        dispatcher,
        root_surface,
        (initial_root_pos.0 + dx, initial_root_pos.1 + dy),
    );
}

fn place_grabbed_root(
    state: &mut State,
    dispatcher: &mut Dispatcher,
    root_surface: Resolved<WlSurfaceId>,
    root_pos: (i32, i32),
) {
    let old_bounds = interactive_damage_rect(state, root_surface);
    let Some(mut surface) = root_surface.try_grab(state) else {
        abort_pointer_interactive_grab(state, dispatcher, 0);
        return;
    };
    surface.set_root_position(root_pos.0, root_pos.1);
    drop(surface);
    state.mark_root_geometry_dirty(root_surface);
    state.sync_window_geometry_goal(
        root_surface,
        crate::visual_animation::WindowGeometryMotion::Warp,
    );
    let new_bounds = interactive_damage_rect(state, root_surface);
    dispatch_client_action(
        dispatcher,
        root_surface.client_id(),
        ClientAction::RefreshSurfaceOutputs,
    );
    if let Some(new_bounds) = new_bounds {
        let rects = old_bounds.into_iter().chain(std::iter::once(new_bounds));
        request_outputs_for_rects(state, rects, dispatcher);
    } else {
        request_active_output(state, dispatcher);
    }
}

fn start_interactive_move(
    state: &mut State,
    dispatcher: &mut Dispatcher,
    client_id: ClientId,
    root_surface: Resolved<WlSurfaceId>,
    initial_geometry: WindowTargetGeometry,
) -> bool {
    begin_compositor_pointer_grab(state, dispatcher, POINTER_MOVE_CURSOR_ICON_NAMES);
    state
        .seat
        .start_toplevel_move(client_id, root_surface, initial_geometry);
    true
}

fn start_interactive_resize(
    state: &mut State,
    dispatcher: &mut Dispatcher,
    client_id: ClientId,
    root_surface: Resolved<WlSurfaceId>,
    xdg_toplevel: Resolved<XdgToplevelId>,
    size_constraints: InteractiveSizeConstraints,
    edges: u32,
    initial_geometry: WindowTargetGeometry,
) -> bool {
    begin_compositor_pointer_grab(state, dispatcher, resize_cursor_icon_names(edges));
    state.seat.start_toplevel_resize(
        client_id,
        root_surface,
        xdg_toplevel,
        size_constraints,
        edges,
        initial_geometry,
    );
    true
}

fn start_interactive_tiled_resize(
    state: &mut State,
    dispatcher: &mut Dispatcher,
    client_id: ClientId,
    root_surface: Resolved<WlSurfaceId>,
    edges: u32,
) -> bool {
    let Some(resize_state) = state.begin_tiled_root_surface_resize(root_surface, edges) else {
        return false;
    };
    begin_compositor_pointer_grab(state, dispatcher, resize_cursor_icon_names(edges));
    state
        .seat
        .start_toplevel_tiled_resize(client_id, root_surface, edges, resize_state);
    true
}

fn clear_pointer_focus_for_compositor_grab(state: &mut State, dispatcher: &mut Dispatcher) {
    crate::input::pointer_lock::release_pointer_capture(state, dispatcher);
    let Some(old_focus) = state.seat.pointer_focus.take() else {
        state.seat.clear_pointer_enter();
        return;
    };
    let serial = state.seat.next_serial();
    dispatch_client_event(
        dispatcher,
        old_focus.target.client_id(),
        SeatClientEvent::PointerLeave { serial, surface: old_focus.target.surface },
    );
    dispatch_client_event(dispatcher, old_focus.target.client_id(), SeatClientEvent::PointerFrame);
    state.seat.clear_pointer_enter();
}

fn update_visible_cursor_for_grab_change(
    state: &mut State,
    dispatcher: &mut Dispatcher,
    update: impl FnOnce(&mut crate::seat::SeatState) -> bool,
) {
    let previous_cursor_owner = visible_cursor_owner_client(state);
    if !update(&mut state.seat) {
        return;
    }
    if let Some(client_id) = previous_cursor_owner {
        dispatch_client_action(dispatcher, client_id, ClientAction::RefreshSurfaceOutputs);
    }
    refresh_visible_cursor_surface_outputs(state, dispatcher);
    request_outputs_for_points(state, [state.seat.pointer_position()], dispatcher);
}

fn begin_compositor_pointer_grab(
    state: &mut State,
    dispatcher: &mut Dispatcher,
    cursor_icons: &[&'static str],
) {
    clear_pointer_focus_for_compositor_grab(state, dispatcher);
    update_visible_cursor_for_grab_change(state, dispatcher, |seat| {
        seat.set_compositor_named_cursor(cursor_icons)
    });
}

fn end_compositor_pointer_grab(state: &mut State, dispatcher: &mut Dispatcher, time: u32) {
    update_visible_cursor_for_grab_change(state, dispatcher, crate::seat::SeatState::clear_cursor);
    refresh_pointer_focus(state, dispatcher, time, false);
    update_visible_cursor_for_grab_change(
        state,
        dispatcher,
        crate::seat::SeatState::set_default_cursor,
    );
}

fn visible_cursor_owner_client(state: &State) -> Option<ClientId> {
    match state.seat.visible_cursor_source()? {
        CursorSourceRef::Named { owner_client_id, .. } => owner_client_id,
        CursorSourceRef::Surface { cursor } => Some(cursor.surface.client_id()),
    }
}

fn refresh_visible_cursor_surface_outputs(state: &State, dispatcher: &mut Dispatcher) {
    if let Some(CursorSourceRef::Surface { cursor }) = state.seat.visible_cursor_source() {
        dispatch_client_action(
            dispatcher,
            cursor.surface.client_id(),
            ClientAction::RefreshSurfaceOutputs,
        );
    }
}

fn interactive_damage_rect(
    state: &State,
    root_surface: Resolved<WlSurfaceId>,
) -> Option<crate::region::RectI> {
    state
        .root_surface_surface_bounds_global(root_surface)
        .or_else(|| state.root_surface_content_rect_global(root_surface))
}

#[cfg(test)]
mod tests {
    use crate::{
        clients::{ClientId, Resolved},
        config::DecorationInsets,
        protocol_generated::WlSurfaceId,
        region::RectI,
        root_geometry::CommittedRootGeometry,
        test_support::state_for_tests,
        wayland_objects::wl_surface::WlSurface,
    };

    use super::interactive_damage_rect;

    #[test]
    fn interactive_damage_rect_prefers_visual_outer_rect() {
        let mut state = state_for_tests();
        let root_surface_id = WlSurfaceId::from(71u32);
        let root_object_id = state
            .insert_object(WlSurface::new(root_surface_id, ClientId::from_raw(7), 6))
            .1;
        let root_surface = Resolved::new(ClientId::from_raw(7), root_surface_id, root_object_id);
        state.register_root_surface(root_surface);
        state.root_geometry_cache.insert(
            root_surface,
            std::cell::Cell::new(crate::state::RootGeometryCacheEntry {
                geometry: Some(CommittedRootGeometry {
                    layout_rect_global: RectI { x: 20, y: 30, width: 120, height: 90 },
                    root_pos: (20, 30),
                    surface_bounds_local: Some(RectI { x: -5, y: -5, width: 130, height: 100 }),
                    logical_window_local: Some(RectI { x: 10, y: 10, width: 100, height: 70 }),
                    decoration_mode: None,
                    decoration_insets: DecorationInsets::default(),
                    tiled_edges: 0,
                    fullscreen: false,
                }),
                dirty: false,
                revision: 0,
                built_from_revision: 0,
            }),
        );

        assert_eq!(
            interactive_damage_rect(&state, root_surface),
            Some(RectI { x: 15, y: 25, width: 130, height: 100 })
        );
    }

    #[test]
    fn tiled_drag_preview_is_hyprland_scale_centered_on_the_pointer() {
        let frame = super::scaled_tiled_preview_frame(
            RectI { x: 10, y: 20, width: 200, height: 100 },
            (80.0, 70.0),
        );
        assert_eq!(frame.width, 169);
        assert_eq!(frame.height, 84);
        assert_eq!(frame.x, (80.0_f64 - 169.0 / 2.0).round() as i32);
        assert_eq!(frame.y, (70.0_f64 - 84.0 / 2.0).round() as i32);
    }

    #[test]
    fn tiled_drag_threshold_uses_euclidean_distance() {
        assert!(!super::tiled_drag_threshold_reached((0.0, 0.0), (3.0, 4.0), 5));
        assert!(super::tiled_drag_threshold_reached((0.0, 0.0), (3.0, 4.0), 4));
        assert!(super::tiled_drag_threshold_reached((10.0, 10.0), (10.0, 21.0), 10));
    }

    #[test]
    fn tiled_drag_preview_follows_the_pointer_past_the_output() {
        let size = (40, 20);
        assert_eq!(super::preview_root_for_pointer((1000.0, 1000.0), size, (0, 0)), (980, 990));
        assert_eq!(super::preview_root_for_pointer((0.0, 0.0), size, (0, 0)), (-20, -10));
        assert_eq!(super::preview_root_for_pointer((1000.0, 1000.0), size, (2, 3)), (978, 987));
        assert_eq!(super::preview_root_for_pointer((50.0, 50.0), (200, 200), (0, 0)), (-50, -50));
    }
}
