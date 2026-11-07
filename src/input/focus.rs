use {
    crate::{
        clients::{Client, ClientAction, ClientDispatcher, ClientId, Resolved},
        drm::{kick_visual_animation_pump, request_outputs_for_rects},
        object::PostAction,
        protocol_generated::{WlSeatId, WlSurfaceId},
        runner::Dispatcher,
        scene::{compose_output, CompositionTier},
        seat::{dispatch_client_event, SeatClientEvent},
        state::{DragPhase, DragTargetState, State},
        wayland_objects::zwlr_layer_surface_v1::{
            layer_surface_keyboard_focus_with_snapshot, LayerKeyboardFocus,
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
    set_keyboard_focus_with_snapshot(state, dispatcher, new_focus, None);
}

fn set_keyboard_focus_with_snapshot(
    state: &mut State,
    dispatcher: &mut Dispatcher,
    new_focus: Option<SurfaceFocus>,
    snapshot: Option<LayerLayoutObjectSnapshot<'_>>,
) {
    let new_focus = normalize_keyboard_focus_with_snapshot(state, new_focus, snapshot);
    if state.seat.keyboard_focus == new_focus {
        // A remap can publish a new toplevel record while keyboard focus stays put.
        crate::capture::sync_activated_from_keyboard_focus(state, dispatcher);
        return;
    }
    crate::input::pointer_lock::release_pointer_lock_unless_surface(
        state,
        dispatcher,
        new_focus.map(|focus| focus.surface),
    );
    crate::input::pointer_confine::release_pointer_confine_unless_surface(
        state,
        dispatcher,
        new_focus.map(|focus| focus.surface),
    );

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
        let keymap = state.xkb.keymap_blob();
        let modifiers = state.keyboard.modifiers();
        crate::seat::ensure_client_keymap(
            state,
            dispatcher,
            new_focus,
            keymap,
            crate::seat::empty_keys(),
            modifiers,
        );
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
    super::ime::keyboard_focus_changed(state, dispatcher, old_focus, new_focus);
    if let Some(new_focus) = new_focus.filter(|_| !state.session_lock.input_gated()) {
        let root_focus = state
            .root_surface_for_surface(new_focus.surface)
            .map(SurfaceFocus::new)
            .unwrap_or(new_focus);
        state.seat.remember_keyboard_focus(root_focus);
    }
    let affected_rects = [old_focus, new_focus].into_iter().filter_map(|focus| {
        let focus = focus?;
        let root_surface = state
            .root_surface_for_surface(focus.surface)
            .unwrap_or(focus.surface);
        state.root_surface_visual_damage_rect(root_surface)
    });
    request_outputs_for_rects(state, affected_rects, dispatcher);
    kick_visual_animation_pump(dispatcher);
    refresh_selection_focus_clients(dispatcher, old_focus, new_focus);
    crate::capture::sync_activated_from_keyboard_focus(state, dispatcher);
    crate::input::pointer_lock::maybe_activate_pointer_lock(state, dispatcher);
    crate::input::pointer_confine::maybe_activate_pointer_confine(state, dispatcher);
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
    set_keyboard_focus_with_snapshot(state, dispatcher, new_focus, snapshot);
}

pub(crate) fn refresh_scene_focus(state: &mut State, dispatcher: &mut Dispatcher) {
    refresh_pointer_focus(state, dispatcher, monotonic_time_msec(), false);
    refresh_keyboard_focus(state, dispatcher);
}

/// Runs after the committing surface is released. Picking it while it is grabbed panics.
pub(crate) struct RefreshLockFocus;

impl PostAction for RefreshLockFocus {
    fn execute(
        self: Box<Self>,
        _client: &mut Client,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
    ) {
        refresh_scene_focus(state, dispatcher);
    }
}

pub(crate) fn restore_keyboard_focus_after_unlock(
    state: &mut State,
    dispatcher: &mut Dispatcher,
    saved: Option<SurfaceFocus>,
) {
    refresh_pointer_focus(state, dispatcher, monotonic_time_msec(), false);
    if let Some(focus) = saved.filter(|focus| keyboard_focus_is_restorable(state, *focus)) {
        set_keyboard_focus(state, dispatcher, Some(focus));
        return;
    }
    focus_after_client_disconnect(state, dispatcher);
}

fn keyboard_focus_is_restorable(state: &State, focus: SurfaceFocus) -> bool {
    let Some(surface) = focus.surface.try_ref(state) else {
        return false;
    };
    if surface.has_lock_surface_role()
        || surface.has_cursor_role()
        || surface.has_dnd_icon_role()
        || surface.has_input_popup_role()
    {
        return false;
    }
    surface.logical_size().is_some()
}

pub(crate) fn focus_after_client_disconnect(state: &mut State, dispatcher: &mut Dispatcher) {
    if state.session_lock.input_gated() {
        refresh_keyboard_focus(state, dispatcher);
        return;
    }
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
    if let Some(candidate) = mru_focus_candidate_for_workspace(state, workspace_id, None)
        .or_else(|| topmost_focus_candidate_for_workspace(state, workspace_id, None))
        .or_else(|| pointer_focus_candidate_for_workspace(state, workspace_id, None))
    {
        set_keyboard_focus(state, dispatcher, Some(candidate));
        return;
    }
    refresh_keyboard_focus(state, dispatcher);
}

/// The workspace window that currently owns the keyboard, captured before it is unmapped.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ClosedWindowFocus {
    pub closed: Resolved<WlSurfaceId>,
    pub workspace_id: crate::state::WorkspaceId,
}

/// `Some` when `surface` is the focused workspace window. Safe while that surface is grabbed:
/// a managed toplevel is its own root and is not `try_ref`'d.
pub(crate) fn focused_window_close(
    state: &State,
    surface: Resolved<WlSurfaceId>,
) -> Option<ClosedWindowFocus> {
    if state.session_lock.input_gated() {
        return None;
    }
    if !state.managed_root_surfaces.contains_key(&surface) {
        return None;
    }
    let focus = state.seat.keyboard_focus()?;
    let root = state.root_surface_for_surface(focus.surface)?;
    if root != surface {
        return None;
    }
    let workspace_id = state.root_surface_workspace(surface)?;
    Some(ClosedWindowFocus { closed: surface, workspace_id })
}

/// Pointer refresh, then a same-workspace successor when the closed surface owned the keyboard.
/// A destroy that leaves the keyboard on some other live surface does not move it.
pub(crate) fn refocus_after_surface_destroy(
    state: &mut State,
    dispatcher: &mut Dispatcher,
    closed_window: Option<ClosedWindowFocus>,
) {
    refresh_pointer_focus(state, dispatcher, monotonic_time_msec(), false);
    if let Some(close) = closed_window {
        focus_after_focused_window_close(state, dispatcher, close);
        return;
    }
    let keyboard_surface_gone = state
        .seat
        .keyboard_focus()
        .is_some_and(|focus| !focus.surface.exists(state));
    if keyboard_surface_gone {
        refresh_keyboard_focus(state, dispatcher);
    }
}

pub(crate) fn refocus_after_focused_window_unmap(
    state: &mut State,
    dispatcher: &mut Dispatcher,
    close: ClosedWindowFocus,
) {
    refresh_pointer_focus(state, dispatcher, monotonic_time_msec(), false);
    focus_after_focused_window_close(state, dispatcher, close);
}

/// Window under the pointer on the closed window's workspace, then focus history, then the
/// topmost remaining window. The closed window is skipped. Hyprland's `input:focus_on_close`
/// tries the cursor first; the history step is the same list sway keeps per workspace.
fn focus_after_focused_window_close(
    state: &mut State,
    dispatcher: &mut Dispatcher,
    close: ClosedWindowFocus,
) {
    if state.session_lock.input_gated() {
        refresh_keyboard_focus(state, dispatcher);
        return;
    }
    let excluding = Some(close.closed);
    if let Some(candidate) =
        pointer_focus_candidate_for_workspace(state, close.workspace_id, excluding)
            .or_else(|| mru_focus_candidate_for_workspace(state, close.workspace_id, excluding))
            .or_else(|| topmost_focus_candidate_for_workspace(state, close.workspace_id, excluding))
    {
        set_keyboard_focus(state, dispatcher, Some(candidate));
        return;
    }
    set_keyboard_focus(state, dispatcher, None);
}

fn mru_focus_candidate_for_workspace(
    state: &State,
    workspace_id: crate::state::WorkspaceId,
    excluding: Option<Resolved<WlSurfaceId>>,
) -> Option<SurfaceFocus> {
    let mut seen = BTreeSet::new();
    for focus in state.seat.keyboard_focus_history() {
        let root_surface = state
            .root_surface_for_surface(focus.surface)
            .unwrap_or(focus.surface);
        if !seen.insert(root_surface) {
            continue;
        }
        if Some(root_surface) == excluding {
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
    excluding: Option<Resolved<WlSurfaceId>>,
) -> Option<SurfaceFocus> {
    let scene = state.build_scene_model();
    let order = state.build_workspace_focus_order(&scene, workspace_id, None);
    order
        .roots_front_to_back
        .into_iter()
        .rev()
        .find(|surface| {
            Some(*surface) != excluding
                && focus_candidate_matches_workspace(state, *surface, workspace_id)
        })
        .map(SurfaceFocus::new)
}

fn pointer_focus_candidate_for_workspace(
    state: &State,
    workspace_id: crate::state::WorkspaceId,
    excluding: Option<Resolved<WlSurfaceId>>,
) -> Option<SurfaceFocus> {
    let pointer_focus = state.seat.pointer_focus.map(|focus| focus.target)?;
    let root_surface = state
        .root_surface_for_surface(pointer_focus.surface)
        .unwrap_or(pointer_focus.surface);
    if Some(root_surface) == excluding {
        return None;
    }
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
        && workspace_root_participates_in_input(state, root_surface, workspace_id)
}

fn workspace_root_participates_in_input(
    state: &State,
    root_surface: Resolved<WlSurfaceId>,
    workspace_id: crate::state::WorkspaceId,
) -> bool {
    let scene = state.build_scene_model();
    for output_id in state.workspace_output_ids(workspace_id) {
        if state.output_bounds(output_id).is_none() {
            continue;
        }
        let view = compose_output(state, &scene, output_id, None);
        if view
            .entries
            .iter()
            .any(|entry| entry.root_surface == root_surface && entry.participates_in_input)
        {
            return true;
        }
    }
    false
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

    crate::input::pointer_lock::release_pointer_lock_unless_surface(
        state,
        dispatcher,
        new_focus.map(|focus| focus.target.surface),
    );
    crate::input::pointer_confine::release_pointer_confine_unless_surface(
        state,
        dispatcher,
        new_focus.map(|focus| focus.target.surface),
    );
    let send_motion = send_motion && state.seat.active_pointer_lock.is_none();

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
            let (x, y) = local_coords(
                state,
                new_focus.target.surface,
                new_focus.rect,
                state.seat.pointer_pos,
            );
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
            let (x, y) = local_coords(
                state,
                new_focus.target.surface,
                new_focus.rect,
                state.seat.pointer_pos,
            );
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
    } else if let (Some(old_focus), Some(new_focus)) = (old_focus, new_focus) {
        let old_local =
            local_coords(state, old_focus.target.surface, old_focus.rect, state.seat.pointer_pos);
        let new_local =
            local_coords(state, new_focus.target.surface, new_focus.rect, state.seat.pointer_pos);
        if old_local != new_local {
            dispatch_client_event(
                dispatcher,
                new_focus.target.client_id(),
                SeatClientEvent::PointerWarp { time, x: new_local.0, y: new_local.1 },
            );
            dispatch_client_event(
                dispatcher,
                new_focus.target.client_id(),
                SeatClientEvent::PointerFrame,
            );
        }
    }

    state.seat.pointer_focus = new_focus;
    crate::input::pointer_lock::maybe_activate_pointer_lock(state, dispatcher);
    crate::input::pointer_confine::maybe_activate_pointer_confine(state, dispatcher);
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
            let (x, y) = local_coords(state, hit.target.surface, hit.rect, state.seat.pointer_pos);
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
            let (x, y) = local_coords(state, hit.target.surface, hit.rect, state.seat.pointer_pos);
            dispatcher
                .dispatch_to(target.client_id(), ClientAction::DataDeviceDragMotion { time, x, y });
        }
    }

    true
}

pub(crate) fn popup_keyboard_focus(state: &State) -> Option<SurfaceFocus> {
    Some(SurfaceFocus::new(state.seat.popup_keyboard_surface()?))
}

fn local_coords(
    state: &State,
    surface: crate::clients::Resolved<crate::protocol_generated::WlSurfaceId>,
    rect: SurfaceRect,
    pointer_pos: (f64, f64),
) -> (f32, f32) {
    let pointer_pos = state.logical_pointer_for_surface(surface, pointer_pos);
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
        dispatcher.dispatch_to(client_id, ClientAction::RefreshPrimarySelection);
    }
    if let Some(client_id) = new_client {
        dispatcher.dispatch_to(client_id, ClientAction::RefreshDataDeviceSelection);
        dispatcher.dispatch_to(client_id, ClientAction::RefreshPrimarySelection);
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
    if state.session_lock.input_gated() {
        return lock_keyboard_focus(state, requested_focus);
    }
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
    let scene = state.build_scene_model();
    let mut top = Vec::new();
    let mut overlay = Vec::new();
    for output_id in state.outputs.iter().map(|(output_id, _)| output_id) {
        if state.output_bounds(output_id).is_none() {
            continue;
        }
        let view = compose_output(state, &scene, output_id, snapshot);
        for entry in view.entries {
            let expected_layer = match entry.tier {
                CompositionTier::Top => LayerShellLayer::Top,
                CompositionTier::Overlay => LayerShellLayer::Overlay,
                CompositionTier::Background
                | CompositionTier::Bottom
                | CompositionTier::Content => {
                    continue;
                }
            };
            let Some(LayerKeyboardFocus::Exclusive { layer }) =
                layer_surface_keyboard_focus_with_snapshot(state, entry.root_surface, snapshot)
            else {
                continue;
            };
            if layer != expected_layer {
                continue;
            }
            match layer {
                LayerShellLayer::Top => top.push(entry.root_surface),
                LayerShellLayer::Overlay => overlay.push(entry.root_surface),
                LayerShellLayer::Background | LayerShellLayer::Bottom => {}
            }
        }
    }

    top.into_iter()
        .chain(overlay)
        .rev()
        .next()
        .map(SurfaceFocus::new)
}

fn lock_keyboard_focus(
    state: &State,
    requested_focus: Option<SurfaceFocus>,
) -> Option<SurfaceFocus> {
    if let Some(focus) = requested_focus.filter(|focus| mapped_lock_content(state, focus.surface)) {
        return Some(focus);
    }
    state
        .seat
        .keyboard_focus()
        .filter(|focus| mapped_lock_content(state, focus.surface))
        .or_else(|| first_mapped_lock_focus(state))
}

fn mapped_lock_content(state: &State, mut surface: Resolved<WlSurfaceId>) -> bool {
    loop {
        let parent = {
            let Some(wl_surface) = surface.try_ref(state) else {
                return false;
            };
            if wl_surface.has_lock_surface_role() {
                let Some(lock_surface) = wl_surface.lock_surface() else {
                    return false;
                };
                let Some(output_id) = lock_surface
                    .try_ref(state)
                    .map(|lock_surface| lock_surface.output_id())
                else {
                    return false;
                };
                return state.session_lock.shows_lock_surface(output_id);
            }
            wl_surface.parent_surface().copied()
        };
        match parent {
            Some(parent) => surface = parent,
            None => return false,
        }
    }
}

fn first_mapped_lock_focus(state: &State) -> Option<SurfaceFocus> {
    state
        .session_lock
        .covered_outputs()
        .into_iter()
        .find_map(|output_id| {
            if !state.session_lock.shows_lock_surface(output_id) {
                return None;
            }
            let lock_surface = state.session_lock.surface_for_output(output_id)?;
            Some(SurfaceFocus::new(lock_surface.try_ref(state)?.wl_surface()))
        })
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

#[cfg(test)]
mod tests {
    use crate::{
        clients::ClientId,
        protocol_generated::WlSurfaceId,
        state::{BackendOutputRef, DrmId},
        test_support::{state_for_tests, test_output_with_crtc_name},
        wayland_objects::wl_surface::WlSurface,
    };

    use {
        super::{
            refocus_after_surface_destroy, set_keyboard_focus,
            topmost_focus_candidate_for_workspace, ClosedWindowFocus,
        },
        crate::{input::SurfaceFocus, runner::Dispatcher},
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

    #[test]
    fn topmost_focus_candidate_uses_managed_workspace_order() {
        let mut state = state_for_tests();
        let backend = BackendOutputRef::new(DrmId::from(0usize));
        let output_id = state.insert_output(test_output_with_crtc_name(backend, 1, 0, 0, 100, 100));
        let workspace_id = state.active_workspace_for_output(output_id).unwrap();
        let lower = insert_root(&mut state, 81);
        let upper = insert_root(&mut state, 82);
        let _ = state.assign_root_surface_to_workspace(lower, workspace_id);
        let _ = state.assign_root_surface_to_workspace(upper, workspace_id);
        state
            .workspaces
            .try_get_mut(workspace_id)
            .unwrap()
            .layout
            .floating
            .stack = vec![lower, upper];
        state.managed_root_surfaces.get_mut(&lower).unwrap().mapped = true;
        state.managed_root_surfaces.get_mut(&upper).unwrap().mapped = true;

        let candidate = topmost_focus_candidate_for_workspace(&state, workspace_id, None)
            .expect("focus candidate");
        assert_eq!(candidate.surface, upper);
    }

    fn place_window(
        state: &mut crate::state::State,
        raw: u32,
        workspace_id: crate::state::WorkspaceId,
        x: i32,
        y: i32,
    ) -> crate::clients::Resolved<WlSurfaceId> {
        let surface = insert_root(state, raw);
        state
            .objects
            .get_mut(surface.object_id())
            .downcast_mut::<WlSurface>()
            .unwrap()
            .test_map(crate::state::ObjectId::from(raw as usize), x, y, 30, 30);
        let _ = state.assign_root_surface_to_workspace(surface, workspace_id);
        state
            .managed_root_surfaces
            .get_mut(&surface)
            .unwrap()
            .mapped = true;
        surface
    }

    struct ThreeWindows {
        workspace_id: crate::state::WorkspaceId,
        older: crate::clients::Resolved<WlSurfaceId>,
        under_pointer: crate::clients::Resolved<WlSurfaceId>,
        closing: crate::clients::Resolved<WlSurfaceId>,
    }

    fn three_windows(state: &mut crate::state::State) -> ThreeWindows {
        let backend = BackendOutputRef::new(DrmId::from(0usize));
        let output_id = state.insert_output(test_output_with_crtc_name(backend, 1, 0, 0, 100, 100));
        let workspace_id = state.active_workspace_for_output(output_id).unwrap();
        let older = place_window(state, 81, workspace_id, 0, 0);
        let under_pointer = place_window(state, 82, workspace_id, 40, 0);
        let closing = place_window(state, 83, workspace_id, 0, 50);
        state
            .workspaces
            .try_get_mut(workspace_id)
            .unwrap()
            .layout
            .floating
            .stack = vec![older, under_pointer, closing];
        ThreeWindows { workspace_id, older, under_pointer, closing }
    }

    fn close_of(windows: &ThreeWindows) -> ClosedWindowFocus {
        ClosedWindowFocus { closed: windows.closing, workspace_id: windows.workspace_id }
    }

    #[test]
    fn closing_focused_window_prefers_the_window_under_the_pointer() {
        let mut state = state_for_tests();
        let mut dispatcher = Dispatcher::default();
        let windows = three_windows(&mut state);
        set_keyboard_focus(&mut state, &mut dispatcher, Some(SurfaceFocus::new(windows.older)));
        set_keyboard_focus(&mut state, &mut dispatcher, Some(SurfaceFocus::new(windows.closing)));
        state.seat.pointer_pos = (50.0, 10.0);

        refocus_after_surface_destroy(&mut state, &mut dispatcher, Some(close_of(&windows)));

        assert_eq!(
            state.seat.keyboard_focus().map(|focus| focus.surface),
            Some(windows.under_pointer)
        );
    }

    #[test]
    fn closing_focused_window_uses_history_when_the_pointer_misses_every_window() {
        let mut state = state_for_tests();
        let mut dispatcher = Dispatcher::default();
        let windows = three_windows(&mut state);
        set_keyboard_focus(&mut state, &mut dispatcher, Some(SurfaceFocus::new(windows.older)));
        set_keyboard_focus(&mut state, &mut dispatcher, Some(SurfaceFocus::new(windows.closing)));
        state.seat.pointer_pos = (90.0, 90.0);

        refocus_after_surface_destroy(&mut state, &mut dispatcher, Some(close_of(&windows)));

        assert_eq!(state.seat.keyboard_focus().map(|focus| focus.surface), Some(windows.older));
    }

    #[test]
    fn closing_focused_window_uses_the_topmost_window_when_history_has_no_other() {
        let mut state = state_for_tests();
        let mut dispatcher = Dispatcher::default();
        let windows = three_windows(&mut state);
        set_keyboard_focus(&mut state, &mut dispatcher, Some(SurfaceFocus::new(windows.closing)));
        state.seat.pointer_pos = (90.0, 90.0);

        refocus_after_surface_destroy(&mut state, &mut dispatcher, Some(close_of(&windows)));

        assert_eq!(
            state.seat.keyboard_focus().map(|focus| focus.surface),
            Some(windows.under_pointer)
        );
    }
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
