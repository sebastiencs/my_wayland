use std::collections::BTreeSet;

use crate::{
    clients::{ClientAction, ClientId, ClientManagerAction},
    cursor::CursorSourceRef,
    drm::{schedule_active_output, schedule_outputs_for_points, schedule_outputs_for_rects},
    protocol_generated::{xdg_toplevel::ResizeEdge, XdgToplevelId},
    runner::Dispatcher,
    seat::{dispatch_client_action, dispatch_client_event, refresh_pointer_focus, SeatClientEvent},
    state::{ObjectId, State},
    wayland_objects::{
        wl_surface::WlSurface, xdg_surface::xdg_toplevel::InteractiveSizeConstraints,
    },
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
            if options.prepare_window {
                prepare_window_for_pointer_grab(
                    state,
                    dispatcher,
                    &target,
                    intent,
                    options.promote_tiled_on_move,
                );
            }
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
        ActiveGrabState::Move {
            client_id,
            root_surface_object_id,
            initial_pointer_pos,
            initial_root_pos,
            ..
        } => {
            let dx = (state.seat.pointer_pos.0 - initial_pointer_pos.0).round() as i32;
            let dy = (state.seat.pointer_pos.1 - initial_pointer_pos.1).round() as i32;
            let Some(mut surface) = state
                .objects
                .try_grab_object::<WlSurface>(root_surface_object_id)
            else {
                abort_pointer_interactive_grab(state, dispatcher, 0);
                return true;
            };
            let old_bounds = surface.global_logical_bounds();
            surface.set_root_position(initial_root_pos.0 + dx, initial_root_pos.1 + dy);
            let new_bounds = surface.global_logical_bounds();
            dispatch_client_action(dispatcher, client_id, ClientAction::RefreshSurfaceOutputs);
            if let Some(new_bounds) = new_bounds {
                let rects = old_bounds.into_iter().chain(std::iter::once(new_bounds));
                schedule_outputs_for_rects(state, rects, dispatcher);
            } else {
                schedule_active_output(state, dispatcher);
            }
        }
        ActiveGrabState::Resize {
            client_id,
            root_surface_object_id,
            xdg_toplevel_id,
            size_constraints,
            edges,
            initial_pointer_pos,
            initial_root_pos,
            initial_size,
            mut last_size,
        } => {
            let dx = (state.seat.pointer_pos.0 - initial_pointer_pos.0).round() as i32;
            let dy = (state.seat.pointer_pos.1 - initial_pointer_pos.1).round() as i32;
            let (mut width, mut height) = initial_size;
            let (mut x, mut y) = initial_root_pos;

            if edges & 4 != 0 {
                width = initial_size.0 - dx;
            }
            if edges & 8 != 0 {
                width = initial_size.0 + dx;
            }
            if edges & 1 != 0 {
                height = initial_size.1 - dy;
            }
            if edges & 2 != 0 {
                height = initial_size.1 + dy;
            }
            (width, height) = size_constraints.clamp(width, height);
            if edges & 4 != 0 {
                x = initial_root_pos.0 + (initial_size.0 - width);
            }
            if edges & 1 != 0 {
                y = initial_root_pos.1 + (initial_size.1 - height);
            }

            let Some(mut surface) = state
                .objects
                .try_grab_object::<WlSurface>(root_surface_object_id)
            else {
                abort_pointer_interactive_grab(state, dispatcher, 0);
                return true;
            };
            let old_bounds = surface.global_logical_bounds();
            surface.set_root_position(x, y);
            let new_bounds = surface.global_logical_bounds();
            dispatch_client_action(dispatcher, client_id, ClientAction::RefreshSurfaceOutputs);
            if let Some(new_bounds) = new_bounds {
                let rects = old_bounds.into_iter().chain(std::iter::once(new_bounds));
                schedule_outputs_for_rects(state, rects, dispatcher);
            } else {
                schedule_active_output(state, dispatcher);
            }

            if last_size != (width, height) {
                dispatch_client_action(
                    dispatcher,
                    client_id,
                    ClientAction::InteractiveResizeConfigure {
                        xdg_toplevel_id,
                        width,
                        height,
                        resizing: true,
                    },
                );
                last_size = (width, height);
            }

            state.seat.pointer_interactive_grab = Some(ActiveGrabState::Resize {
                client_id,
                root_surface_object_id,
                xdg_toplevel_id,
                size_constraints,
                edges,
                initial_pointer_pos,
                initial_root_pos,
                initial_size,
                last_size,
            });
        }
        ActiveGrabState::ResizeTiled {
            client_id,
            root_surface_object_id,
            edges,
            initial_pointer_pos,
            resize_state,
        } => {
            let dx = (state.seat.pointer_pos.0 - initial_pointer_pos.0).round() as i32;
            let dy = (state.seat.pointer_pos.1 - initial_pointer_pos.1).round() as i32;
            if state.resize_tiled_root_surface_from_state(
                root_surface_object_id,
                resize_state,
                dx,
                dy,
            ) {
                dispatcher.dispatch(ClientManagerAction::WorkspaceStateChanged);
                schedule_active_output(state, dispatcher);
            }
            state.seat.pointer_interactive_grab = Some(ActiveGrabState::ResizeTiled {
                client_id,
                root_surface_object_id,
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
    if let ActiveGrabState::Resize { client_id, xdg_toplevel_id, last_size, .. } = grab {
        dispatch_client_action(
            dispatcher,
            client_id,
            ClientAction::InteractiveResizeConfigure {
                xdg_toplevel_id,
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

pub(crate) fn tiled_resize_active_root_surfaces(state: &State) -> BTreeSet<ObjectId> {
    let Some(ActiveGrabState::ResizeTiled { root_surface_object_id, resize_state, .. }) =
        state.seat.pointer_interactive_grab
    else {
        return BTreeSet::new();
    };
    let Some(workspace_id) = state.root_surface_workspace(root_surface_object_id) else {
        return BTreeSet::new();
    };
    let Some(workspace) = state.workspaces.try_get(workspace_id) else {
        return BTreeSet::new();
    };
    workspace
        .layout
        .resize_affected_surfaces(root_surface_object_id, resize_state)
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
) {
    if promote_tiled_on_move
        && matches!(intent, InteractionIntent::StartMove(_))
        && state.is_tiled_root_surface(target.root_surface_object_id)
        && state.set_root_surface_floating(target.root_surface_object_id, true)
    {
        dispatcher.dispatch(ClientManagerAction::WorkspaceStateChanged);
        schedule_active_output(state, dispatcher);
    }

    state.raise_root_surface(target.root_surface_object_id);
    schedule_outputs_for_rects(state, [target.interactive_rect], dispatcher);
    set_keyboard_focus(
        state,
        dispatcher,
        Some(SurfaceFocus { client_id: target.client_id, surface_id: target.surface_id }),
    );
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
            target.client_id,
            target.root_surface_object_id,
            target.initial_root_pos,
        ),
        (
            InteractionIntent::StartResize(_, edges),
            WindowRoleSnapshot::XdgToplevel { xdg_toplevel_id, size_constraints, .. },
        ) => {
            if state.is_tiled_root_surface(target.root_surface_object_id) {
                start_interactive_tiled_resize(
                    state,
                    dispatcher,
                    target.client_id,
                    target.root_surface_object_id,
                    edges,
                )
            } else {
                start_interactive_resize(
                    state,
                    dispatcher,
                    target.client_id,
                    target.root_surface_object_id,
                    xdg_toplevel_id,
                    size_constraints,
                    edges,
                    target.initial_root_pos,
                    target.initial_size,
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
    root_surface_object_id: ObjectId,
    initial_root_pos: (i32, i32),
) -> bool {
    begin_compositor_pointer_grab(state, dispatcher, POINTER_MOVE_CURSOR_ICON_NAMES);
    state
        .seat
        .start_toplevel_move(client_id, root_surface_object_id, initial_root_pos);
    true
}

fn start_interactive_resize(
    state: &mut State,
    dispatcher: &mut Dispatcher,
    client_id: ClientId,
    root_surface_object_id: ObjectId,
    xdg_toplevel_id: XdgToplevelId,
    size_constraints: InteractiveSizeConstraints,
    edges: u32,
    initial_root_pos: (i32, i32),
    initial_size: (i32, i32),
) -> bool {
    begin_compositor_pointer_grab(state, dispatcher, resize_cursor_icon_names(edges));
    state.seat.start_toplevel_resize(
        client_id,
        root_surface_object_id,
        xdg_toplevel_id,
        size_constraints,
        edges,
        initial_root_pos,
        initial_size,
    );
    true
}

fn start_interactive_tiled_resize(
    state: &mut State,
    dispatcher: &mut Dispatcher,
    client_id: ClientId,
    root_surface_object_id: ObjectId,
    edges: u32,
) -> bool {
    let Some(resize_state) = state.begin_tiled_root_surface_resize(root_surface_object_id, edges)
    else {
        return false;
    };
    begin_compositor_pointer_grab(state, dispatcher, resize_cursor_icon_names(edges));
    state
        .seat
        .start_toplevel_tiled_resize(client_id, root_surface_object_id, edges, resize_state);
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
        old_focus.target.client_id,
        SeatClientEvent::PointerLeave { serial, surface: old_focus.target.surface_id },
    );
    dispatch_client_event(dispatcher, old_focus.target.client_id, SeatClientEvent::PointerFrame);
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
        CursorSourceRef::Surface { owner_client_id, .. } => Some(owner_client_id),
    }
}

fn refresh_visible_cursor_surface_outputs(state: &State, dispatcher: &mut Dispatcher) {
    if let Some(CursorSourceRef::Surface { owner_client_id, .. }) =
        state.seat.visible_cursor_source()
    {
        dispatch_client_action(dispatcher, owner_client_id, ClientAction::RefreshSurfaceOutputs);
    }
}
