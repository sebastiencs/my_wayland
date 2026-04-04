use {
    crate::{
        clients::{Client, ClientAction, ClientId, Resolved},
        protocol_generated::{WlSeatId, WlSurfaceId},
        runner::Dispatcher,
        seat::{dispatch_client_event, SeatClientEvent},
        state::{DragPhase, DragTargetState, State},
        wayland_objects::zwlr_layer_surface_v1::{
            layer_surface_keyboard_focus_with_snapshot, lookup_snapshot_surface,
            ordered_scene_root_surfaces_with_snapshot, LayerKeyboardFocus,
            LayerLayoutObjectSnapshot, LayerShellLayer,
        },
    },
    std::collections::BTreeSet,
};

use super::{scene_pick_for_pointer, surface_focus_pick_at, SurfaceFocus, SurfaceRect};

pub(crate) fn set_keyboard_focus(
    state: &mut State,
    dispatcher: &mut Dispatcher,
    new_focus: Option<SurfaceFocus>,
) {
    let new_focus = normalize_keyboard_focus(state, new_focus);
    if state.seat.keyboard_focus == new_focus {
        return;
    }

    log::debug!(
        "seat keyboard focus change old={:?} new={:?}",
        state.seat.keyboard_focus,
        new_focus
    );

    let serial = state.seat.next_serial();

    if let Some(old_focus) = state.seat.keyboard_focus {
        dispatch_client_event(
            dispatcher,
            old_focus.client_id(),
            SeatClientEvent::KeyboardLeave { serial, surface: old_focus.surface },
        );
    }

    if let Some(new_focus) = new_focus {
        let keys = state.keyboard.pressed_keys();
        dispatch_client_event(
            dispatcher,
            new_focus.client_id(),
            SeatClientEvent::KeyboardEnter { serial, surface: new_focus.surface, keys },
        );
        dispatch_modifiers(dispatcher, new_focus.client_id(), serial, state.keyboard.modifiers());
    }

    let old_focus = state.seat.keyboard_focus;
    state.seat.keyboard_focus = new_focus;
    if let Some(new_focus) = new_focus {
        let root_focus = state
            .root_surface_for_surface(new_focus.surface)
            .map(SurfaceFocus::new)
            .unwrap_or(new_focus);
        state.seat.remember_keyboard_focus(root_focus);
    }
    refresh_selection_focus_clients(dispatcher, old_focus, new_focus);
}

pub(crate) fn set_keyboard_focus_from_surface(
    state: &mut State,
    dispatcher: &mut Dispatcher,
    new_focus: Option<SurfaceFocus>,
) {
    set_keyboard_focus(state, dispatcher, new_focus);
}

pub(crate) fn resolve_activation_target(
    state: &State,
    mut surface: Resolved<WlSurfaceId>,
) -> Option<SurfaceFocus> {
    loop {
        let surface_ref = surface.try_ref(state)?;
        match surface_ref.parent_surface() {
            Some(parent) => surface = parent.clone(),
            None => {
                if !surface_ref.has_committed_buffer() {
                    return None;
                }
                return Some(SurfaceFocus::new(surface));
            }
        }
    }
}

pub(crate) fn validate_activation_serial(
    state: &State,
    client: &Client,
    seat: Resolved<WlSeatId>,
    surface: Resolved<WlSurfaceId>,
    serial: u32,
) -> bool {
    client.object_is(seat.wayland_id(), state)
        && seat.client_id() == client.client_id()
        && surface.client_id() == client.client_id()
        && state
            .seat
            .validate_implicit_grab_serial(client.client_id(), surface, serial)
}

pub(crate) fn refresh_keyboard_focus(state: &mut State, dispatcher: &mut Dispatcher) {
    refresh_keyboard_focus_with_snapshot(state, dispatcher, None);
}

pub(crate) fn refresh_keyboard_focus_with_snapshot(
    state: &mut State,
    dispatcher: &mut Dispatcher,
    snapshot: Option<LayerLayoutObjectSnapshot<'_>>,
) {
    let preferred_focus =
        popup_keyboard_focus(state).or_else(|| state.seat.pointer_focus.map(|focus| focus.target));
    let new_focus = normalize_keyboard_focus_with_snapshot(state, preferred_focus, snapshot);
    set_keyboard_focus(state, dispatcher, new_focus);
}

pub(crate) fn refresh_scene_focus(state: &mut State, dispatcher: &mut Dispatcher) {
    refresh_pointer_focus(state, dispatcher, monotonic_time_msec(), false);
    refresh_keyboard_focus(state, dispatcher);
}

pub(crate) fn focus_after_client_disconnect(state: &mut State, dispatcher: &mut Dispatcher) {
    if state.seat.keyboard_focus().is_some() {
        return;
    }
    let Some(workspace_id) = state
        .active_output_id()
        .and_then(|output_id| state.active_workspace_for_output(output_id))
    else {
        refresh_keyboard_focus(state, dispatcher);
        return;
    };
    if let Some(candidate) = mru_focus_candidate_for_workspace(state, workspace_id)
        .or_else(|| topmost_focus_candidate_for_workspace(state, workspace_id))
        .or_else(|| pointer_focus_candidate_for_workspace(state, workspace_id))
    {
        set_keyboard_focus(state, dispatcher, Some(candidate));
        return;
    }
    refresh_keyboard_focus(state, dispatcher);
}

fn mru_focus_candidate_for_workspace(
    state: &State,
    workspace_id: crate::state::WorkspaceId,
) -> Option<SurfaceFocus> {
    let mut seen = BTreeSet::new();
    for focus in state.seat.keyboard_focus_history() {
        let root_surface = state
            .root_surface_for_surface(focus.surface)
            .unwrap_or(focus.surface);
        if !seen.insert(root_surface) {
            continue;
        }
        if !focus_candidate_matches_workspace(state, root_surface, workspace_id) {
            continue;
        }
        return Some(SurfaceFocus::new(root_surface));
    }
    None
}

fn topmost_focus_candidate_for_workspace(
    state: &State,
    workspace_id: crate::state::WorkspaceId,
) -> Option<SurfaceFocus> {
    state
        .tiling_scene_root_surfaces()
        .into_iter()
        .rev()
        .find(|surface| focus_candidate_matches_workspace(state, *surface, workspace_id))
        .map(SurfaceFocus::new)
}

fn pointer_focus_candidate_for_workspace(
    state: &State,
    workspace_id: crate::state::WorkspaceId,
) -> Option<SurfaceFocus> {
    let pointer_focus = state.seat.pointer_focus.map(|focus| focus.target)?;
    let root_surface = state
        .root_surface_for_surface(pointer_focus.surface)
        .unwrap_or(pointer_focus.surface);
    focus_candidate_matches_workspace(state, root_surface, workspace_id)
        .then_some(SurfaceFocus::new(root_surface))
}

fn focus_candidate_matches_workspace(
    state: &State,
    root_surface: Resolved<WlSurfaceId>,
    workspace_id: crate::state::WorkspaceId,
) -> bool {
    root_surface.exists(state)
        && state.root_surface_workspace(root_surface) == Some(workspace_id)
        && state.root_surface_is_mapped(root_surface)
        && state.root_surface_is_visible(root_surface)
}

pub(crate) fn refresh_active_drag_focus(state: &mut State, dispatcher: &mut Dispatcher, time: u32) {
    let _ = refresh_drag_focus(state, dispatcher, time, true);
}

pub(crate) fn maybe_dismiss_popup_grab_on_button(
    state: &mut State,
    dispatcher: &mut Dispatcher,
    button_state: u32,
    time: u32,
) -> bool {
    if button_state != 1 {
        return false;
    }
    let Some(_grab_client_id) = state.seat.popup_grab_client() else {
        return false;
    };
    let inside_popup_stack = state
        .seat
        .pointer_focus
        .is_some_and(|focus| state.seat.popup_grab_contains_surface(&focus.target));
    if inside_popup_stack {
        return false;
    }
    let Some((client_id, popups)) = state.seat.dismiss_popup_grab() else {
        return false;
    };
    dispatcher.dispatch_to(client_id, ClientAction::DismissPopupGrab { popups });
    refresh_pointer_focus(state, dispatcher, time, false);
    set_keyboard_focus(state, dispatcher, state.seat.pointer_focus.map(|focus| focus.target));
    true
}

pub(crate) fn refresh_pointer_focus(
    state: &mut State,
    dispatcher: &mut Dispatcher,
    time: u32,
    send_motion: bool,
) {
    let old_focus = state.seat.pointer_focus;
    let scene_pick = scene_pick_for_pointer(state);
    let mut new_focus = scene_pick.surface_focus;
    if let Some(grab_client_id) = state.seat.popup_grab_client() {
        if new_focus.is_some_and(|focus| focus.target.client_id() != grab_client_id) {
            new_focus = None;
        }
    }

    if old_focus.map(|focus| focus.target) != new_focus.map(|focus| focus.target) {
        log::debug!(
            "seat pointer focus change pos=({:.1}, {:.1}) old={:?} new={:?}",
            state.seat.pointer_pos.0,
            state.seat.pointer_pos.1,
            old_focus.map(|focus| focus.target),
            new_focus.map(|focus| focus.target)
        );
    }

    if old_focus.map(|focus| focus.target) != new_focus.map(|focus| focus.target) {
        let serial = state.seat.next_serial();

        if let Some(old_focus) = old_focus {
            dispatch_client_event(
                dispatcher,
                old_focus.target.client_id(),
                SeatClientEvent::PointerLeave { serial, surface: old_focus.target.surface },
            );
            dispatch_client_event(
                dispatcher,
                old_focus.target.client_id(),
                SeatClientEvent::PointerFrame,
            );
        }

        if let Some(new_focus) = new_focus {
            let (x, y) = local_coords(new_focus.rect, state.seat.pointer_pos);
            state
                .seat
                .record_pointer_enter(new_focus.target.client_id(), serial);
            dispatch_client_event(
                dispatcher,
                new_focus.target.client_id(),
                SeatClientEvent::PointerEnter { serial, surface: new_focus.target.surface, x, y },
            );
            if send_motion {
                dispatch_client_event(
                    dispatcher,
                    new_focus.target.client_id(),
                    SeatClientEvent::PointerMotion { time, x, y },
                );
            }
            dispatch_client_event(
                dispatcher,
                new_focus.target.client_id(),
                SeatClientEvent::PointerFrame,
            );
        } else {
            state.seat.clear_pointer_enter();
        }
    } else if send_motion {
        if let Some(new_focus) = new_focus {
            let (x, y) = local_coords(new_focus.rect, state.seat.pointer_pos);
            dispatch_client_event(
                dispatcher,
                new_focus.target.client_id(),
                SeatClientEvent::PointerMotion { time, x, y },
            );
            dispatch_client_event(
                dispatcher,
                new_focus.target.client_id(),
                SeatClientEvent::PointerFrame,
            );
        }
    }

    state.seat.pointer_focus = new_focus;
}

pub(crate) fn refresh_drag_focus(
    state: &mut State,
    dispatcher: &mut Dispatcher,
    time: u32,
    send_motion: bool,
) -> bool {
    let Some(active_drag) = state.seat.active_drag() else {
        return false;
    };
    if active_drag.phase != DragPhase::Dragging {
        return true;
    }

    let hit = surface_focus_pick_at(state, state.seat.pointer_position());
    let mut new_target = hit.map(|focus| DragTargetState { surface: focus.target.surface });
    if active_drag.source.object_id().is_none()
        && new_target
            .is_some_and(|target| target.client_id() != active_drag.source.initiator_client_id())
    {
        new_target = None;
    }

    let old_target = active_drag.target;
    if old_target != new_target {
        if let Some(old_target) = old_target {
            dispatcher.dispatch_to(old_target.client_id(), ClientAction::DataDeviceDragLeave);
        }
        if let Some(source) = active_drag.source.resolved() {
            dispatcher.dispatch_to(
                source.client_id(),
                ClientAction::DataSourceEvent {
                    source,
                    event: crate::clients::DataSourceClientEvent::Target(None),
                },
            );
            dispatcher.dispatch_to(
                source.client_id(),
                ClientAction::DataSourceEvent {
                    source,
                    event: crate::clients::DataSourceClientEvent::Action { dnd_action: 0 },
                },
            );
        }

        if let (Some(hit), Some(new_target)) = (hit, new_target) {
            let serial = state.seat.next_serial();
            let (x, y) = local_coords(hit.rect, state.seat.pointer_pos);
            dispatcher.dispatch_to(
                new_target.client_id(),
                ClientAction::DataDeviceDragEnter {
                    serial,
                    surface: new_target.surface,
                    x,
                    y,
                    source: active_drag.source.resolved(),
                },
            );
            if send_motion {
                dispatcher.dispatch_to(
                    new_target.client_id(),
                    ClientAction::DataDeviceDragMotion { time, x, y },
                );
            }
        }

        state.seat.set_active_drag_target(new_target);
        return true;
    }

    if send_motion {
        if let (Some(hit), Some(target)) = (hit, new_target) {
            let (x, y) = local_coords(hit.rect, state.seat.pointer_pos);
            dispatcher
                .dispatch_to(target.client_id(), ClientAction::DataDeviceDragMotion { time, x, y });
        }
    }

    true
}

pub(crate) fn popup_keyboard_focus(state: &State) -> Option<SurfaceFocus> {
    Some(SurfaceFocus::new(state.seat.popup_keyboard_surface()?))
}

fn local_coords(rect: SurfaceRect, pointer_pos: (f64, f64)) -> (f32, f32) {
    ((pointer_pos.0 - rect.x as f64) as f32, (pointer_pos.1 - rect.y as f64) as f32)
}

fn refresh_selection_focus_clients(
    dispatcher: &mut Dispatcher,
    old_focus: Option<SurfaceFocus>,
    new_focus: Option<SurfaceFocus>,
) {
    let old_client = old_focus.map(|surface| surface.client_id());
    let new_client = new_focus.map(|surface| surface.client_id());
    if old_client == new_client {
        return;
    }
    if let Some(client_id) = old_client {
        dispatcher.dispatch_to(client_id, ClientAction::RefreshDataDeviceSelection);
    }
    if let Some(client_id) = new_client {
        dispatcher.dispatch_to(client_id, ClientAction::RefreshDataDeviceSelection);
    }
}

fn normalize_keyboard_focus(
    state: &State,
    requested_focus: Option<SurfaceFocus>,
) -> Option<SurfaceFocus> {
    normalize_keyboard_focus_with_snapshot(state, requested_focus, None)
}

fn normalize_keyboard_focus_with_snapshot(
    state: &State,
    requested_focus: Option<SurfaceFocus>,
    snapshot: Option<LayerLayoutObjectSnapshot<'_>>,
) -> Option<SurfaceFocus> {
    let requested_focus = requested_focus.and_then(|focus| {
        let root_surface = root_surface_for_focus(state, focus)?;
        match layer_surface_keyboard_focus_with_snapshot(state, root_surface, snapshot) {
            Some(LayerKeyboardFocus::None) => None,
            _ => Some(focus),
        }
    });

    if let Some(popup_focus) = popup_keyboard_focus(state) {
        if requested_focus == Some(popup_focus) {
            return requested_focus;
        }
    }

    topmost_exclusive_layer_focus_with_snapshot(state, snapshot).or(requested_focus)
}

fn topmost_exclusive_layer_focus_with_snapshot(
    state: &State,
    snapshot: Option<LayerLayoutObjectSnapshot<'_>>,
) -> Option<SurfaceFocus> {
    for root_surface in ordered_scene_root_surfaces_with_snapshot(state, snapshot)
        .into_iter()
        .rev()
    {
        let Some(_surface) = lookup_snapshot_surface(state, snapshot, root_surface) else {
            continue;
        };
        let Some(LayerKeyboardFocus::Exclusive { layer }) =
            layer_surface_keyboard_focus_with_snapshot(state, root_surface, snapshot)
        else {
            continue;
        };
        if !matches!(layer, LayerShellLayer::Top | LayerShellLayer::Overlay) {
            continue;
        }
        return Some(SurfaceFocus::new(root_surface));
    }

    None
}

fn root_surface_for_focus(state: &State, focus: SurfaceFocus) -> Option<Resolved<WlSurfaceId>> {
    focus.exists(state).then_some(focus.surface)
}

fn monotonic_time_msec() -> u32 {
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) } != 0 {
        return 0;
    }
    let millis = (ts.tv_sec as u64)
        .saturating_mul(1000)
        .saturating_add((ts.tv_nsec as u64) / 1_000_000);
    millis.min(u64::from(u32::MAX)) as u32
}

fn dispatch_modifiers(
    dispatcher: &mut Dispatcher,
    client_id: ClientId,
    serial: u32,
    modifiers: crate::xkb::KeyboardModifiers,
) {
    dispatch_client_event(
        dispatcher,
        client_id,
        SeatClientEvent::KeyboardModifiers {
            serial,
            mods_depressed: modifiers.mods_depressed,
            mods_latched: modifiers.mods_latched,
            mods_locked: modifiers.mods_locked,
            group: modifiers.group,
        },
    );
}
