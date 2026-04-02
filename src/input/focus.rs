use crate::{
    clients::{Client, ClientAction, ClientId, Resolved},
    protocol_generated::{WlSeatId, WlSurfaceId},
    runner::Dispatcher,
    seat::{dispatch_client_event, SeatClientEvent},
    state::{DragPhase, DragTargetState, ObjectId, State},
    wayland_objects::{
        wl_surface::WlSurface,
        zwlr_layer_surface_v1::{
            layer_surface_keyboard_focus_with_snapshot, lookup_snapshot_surface,
            ordered_scene_root_surface_ids_with_snapshot, LayerKeyboardFocus,
            LayerLayoutObjectSnapshot, LayerShellLayer,
        },
    },
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
            old_focus.client_id,
            SeatClientEvent::KeyboardLeave { serial, surface: old_focus.surface_id },
        );
    }

    if let Some(new_focus) = new_focus {
        let keys = state.keyboard.pressed_keys();
        dispatch_client_event(
            dispatcher,
            new_focus.client_id,
            SeatClientEvent::KeyboardEnter { serial, surface: new_focus.surface_id, keys },
        );
        dispatch_modifiers(dispatcher, new_focus.client_id, serial, state.keyboard.modifiers());
    }

    let old_focus = state.seat.keyboard_focus;
    state.seat.keyboard_focus = new_focus;
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
    surface: Resolved<WlSurfaceId>,
) -> Option<(ObjectId, SurfaceFocus)> {
    let mut current_object_id = surface.object_id();
    loop {
        let surface = state
            .objects
            .try_ref_object::<WlSurface>(current_object_id)?;
        match surface.parent_surface_object_id() {
            Some(parent_object_id) => current_object_id = parent_object_id,
            None => {
                if !surface.has_committed_buffer() {
                    return None;
                }
                return Some((
                    current_object_id,
                    SurfaceFocus {
                        client_id: surface.client_id(),
                        surface_id: surface.surface_id(),
                    },
                ));
            }
        }
    }
}

pub(crate) fn validate_activation_serial(
    state: &State,
    client: &Client,
    seat_id: WlSeatId,
    surface_id: WlSurfaceId,
    serial: u32,
) -> bool {
    client.object_is(seat_id, state)
        && state
            .seat
            .validate_implicit_grab_serial(client.client_id(), surface_id, serial)
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
    let inside_popup_stack = state.seat.pointer_focus.is_some_and(|focus| {
        state
            .seat
            .popup_grab_contains_surface(focus.target.surface_id)
    });
    if inside_popup_stack {
        return false;
    }
    let Some((client_id, popup_ids)) = state.seat.dismiss_popup_grab() else {
        return false;
    };
    dispatcher.dispatch_to(client_id, ClientAction::DismissPopupGrab { popup_ids });
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
        if new_focus.is_some_and(|focus| focus.target.client_id != grab_client_id) {
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
                old_focus.target.client_id,
                SeatClientEvent::PointerLeave { serial, surface: old_focus.target.surface_id },
            );
            dispatch_client_event(
                dispatcher,
                old_focus.target.client_id,
                SeatClientEvent::PointerFrame,
            );
        }

        if let Some(new_focus) = new_focus {
            let (x, y) = local_coords(new_focus.rect, state.seat.pointer_pos);
            state
                .seat
                .record_pointer_enter(new_focus.target.client_id, serial);
            dispatch_client_event(
                dispatcher,
                new_focus.target.client_id,
                SeatClientEvent::PointerEnter {
                    serial,
                    surface: new_focus.target.surface_id,
                    x,
                    y,
                },
            );
            if send_motion {
                dispatch_client_event(
                    dispatcher,
                    new_focus.target.client_id,
                    SeatClientEvent::PointerMotion { time, x, y },
                );
            }
            dispatch_client_event(
                dispatcher,
                new_focus.target.client_id,
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
                new_focus.target.client_id,
                SeatClientEvent::PointerMotion { time, x, y },
            );
            dispatch_client_event(
                dispatcher,
                new_focus.target.client_id,
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
    let mut new_target = hit.map(|focus| DragTargetState {
        client_id: focus.target.client_id,
        surface_id: focus.target.surface_id,
    });
    if active_drag.source.object_id().is_none()
        && new_target
            .is_some_and(|target| target.client_id != active_drag.source.initiator_client_id())
    {
        new_target = None;
    }

    let old_target = active_drag.target;
    if old_target != new_target {
        if let Some(old_target) = old_target {
            dispatcher.dispatch_to(old_target.client_id, ClientAction::DataDeviceDragLeave);
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
                new_target.client_id,
                ClientAction::DataDeviceDragEnter {
                    serial,
                    surface: new_target.surface_id,
                    x,
                    y,
                    source: active_drag.source.resolved(),
                },
            );
            if send_motion {
                dispatcher.dispatch_to(
                    new_target.client_id,
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
                .dispatch_to(target.client_id, ClientAction::DataDeviceDragMotion { time, x, y });
        }
    }

    true
}

pub(crate) fn popup_keyboard_focus(state: &State) -> Option<SurfaceFocus> {
    let surface_id = state.seat.popup_keyboard_surface()?;
    let client_id = state.seat.popup_grab_client()?;
    Some(SurfaceFocus { client_id, surface_id })
}

fn local_coords(rect: SurfaceRect, pointer_pos: (f64, f64)) -> (f32, f32) {
    ((pointer_pos.0 - rect.x as f64) as f32, (pointer_pos.1 - rect.y as f64) as f32)
}

fn refresh_selection_focus_clients(
    dispatcher: &mut Dispatcher,
    old_focus: Option<SurfaceFocus>,
    new_focus: Option<SurfaceFocus>,
) {
    let old_client = old_focus.map(|focus| focus.client_id);
    let new_client = new_focus.map(|focus| focus.client_id);
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
        let surface_object_id = surface_object_id_for_focus(state, focus)?;
        match layer_surface_keyboard_focus_with_snapshot(state, surface_object_id, snapshot) {
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
    for object_id in ordered_scene_root_surface_ids_with_snapshot(state, snapshot)
        .into_iter()
        .rev()
    {
        let Some(surface) = lookup_snapshot_surface(state, snapshot, object_id) else {
            continue;
        };
        let Some(LayerKeyboardFocus::Exclusive { layer }) =
            layer_surface_keyboard_focus_with_snapshot(state, object_id, snapshot)
        else {
            continue;
        };
        if !matches!(layer, LayerShellLayer::Top | LayerShellLayer::Overlay) {
            continue;
        }
        return Some(SurfaceFocus {
            client_id: surface.client_id(),
            surface_id: surface.surface_id(),
        });
    }

    None
}

fn surface_object_id_for_focus(state: &State, focus: SurfaceFocus) -> Option<ObjectId> {
    for (object_id, object) in state.objects.iter() {
        let Some(surface) = object.downcast_ref::<WlSurface>() else {
            continue;
        };
        if surface.client_id() == focus.client_id && surface.surface_id() == focus.surface_id {
            return Some(object_id);
        }
    }
    None
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
