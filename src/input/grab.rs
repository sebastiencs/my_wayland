use std::collections::BTreeSet;

use crate::{
    clients::{ClientAction, ClientId, ClientManagerAction, Resolved},
    cursor::CursorSourceRef,
    drm::{schedule_active_output, schedule_outputs_for_points, schedule_outputs_for_rects},
    protocol_generated::{xdg_toplevel::ResizeEdge, WlSurfaceId, XdgToplevelId},
    root_geometry::WindowTargetGeometry,
    runner::Dispatcher,
    seat::{dispatch_client_action, dispatch_client_event, refresh_pointer_focus, SeatClientEvent},
    state::State,
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
                prepare_window_for_pointer_grab(
                    state,
                    dispatcher,
                    &target,
                    intent,
                    options.promote_tiled_on_move,
                )
            } else {
                target
            };
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
            let dx = (state.seat.pointer_pos.0 - initial_pointer_pos.0).round() as i32;
            let dy = (state.seat.pointer_pos.1 - initial_pointer_pos.1).round() as i32;
            let old_bounds = interactive_damage_rect(state, root_surface);
            let Some(mut surface) = root_surface.try_grab(state) else {
                abort_pointer_interactive_grab(state, dispatcher, 0);
                return true;
            };
            surface.set_root_position(
                initial_geometry.root_pos.0 + dx,
                initial_geometry.root_pos.1 + dy,
            );
            drop(surface);
            state.mark_root_geometry_dirty(root_surface);
            let new_bounds = interactive_damage_rect(state, root_surface);
            dispatch_client_action(
                dispatcher,
                root_surface.client_id(),
                ClientAction::RefreshSurfaceOutputs,
            );
            if let Some(new_bounds) = new_bounds {
                let rects = old_bounds.into_iter().chain(std::iter::once(new_bounds));
                schedule_outputs_for_rects(state, rects, dispatcher);
            } else {
                schedule_active_output(state, dispatcher);
            }
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
            let new_bounds = interactive_damage_rect(state, root_surface);
            dispatch_client_action(
                dispatcher,
                root_surface.client_id(),
                ClientAction::RefreshSurfaceOutputs,
            );
            if let Some(new_bounds) = new_bounds {
                let rects = old_bounds.into_iter().chain(std::iter::once(new_bounds));
                schedule_outputs_for_rects(state, rects, dispatcher);
            } else {
                schedule_active_output(state, dispatcher);
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
                schedule_active_output(state, dispatcher);
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
    let Some(grab) = state.seat.pointer_interactive_grab else {
        return;
    };
    let was_tiled_resize = matches!(grab, ActiveGrabState::ResizeTiled { .. });
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
    state.seat.clear_pointer_interactive_grab();
    if was_tiled_resize {
        dispatcher.dispatch(ClientManagerAction::WorkspaceStateChanged);
    }
    end_compositor_pointer_grab(state, dispatcher, time);
}

pub(crate) fn abort_pointer_interactive_grab(
    state: &mut State,
    dispatcher: &mut Dispatcher,
    time: u32,
) -> bool {
    let was_tiled_resize =
        matches!(state.seat.pointer_interactive_grab, Some(ActiveGrabState::ResizeTiled { .. }));
    if state.seat.pointer_interactive_grab.is_none() {
        return false;
    }
    state.seat.clear_pointer_interactive_grab();
    if was_tiled_resize {
        dispatcher.dispatch(ClientManagerAction::WorkspaceStateChanged);
    }
    end_compositor_pointer_grab(state, dispatcher, time);
    true
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

fn prepare_window_for_pointer_grab(
    state: &mut State,
    dispatcher: &mut Dispatcher,
    target: &WindowTarget,
    intent: InteractionIntent,
    promote_tiled_on_move: bool,
) -> WindowTarget {
    let mut target = *target;
    if promote_tiled_on_move
        && matches!(intent, InteractionIntent::StartMove(_))
        && state.is_tiled_root_surface(target.root_surface)
        && state.set_root_surface_floating(target.root_surface, true)
    {
        dispatcher.dispatch(ClientManagerAction::WorkspaceStateChanged);
        schedule_active_output(state, dispatcher);
        target =
            crate::input::pick::build_window_target(state, target.root_surface).unwrap_or(target);
    }

    state.raise_root_surface(target.root_surface);
    schedule_outputs_for_rects(
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
    schedule_outputs_for_points(state, [state.seat.pointer_position()], dispatcher);
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
        state.register_root_surface(root_object_id);
        let root_surface = Resolved::new(ClientId::from_raw(7), root_surface_id, root_object_id);
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
}
