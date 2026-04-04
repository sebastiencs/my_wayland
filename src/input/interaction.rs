use crate::{
    clients::Resolved,
    config,
    protocol_generated::{xdg_toplevel::ResizeEdge, WlSurfaceId, XdgToplevelId},
    runner::Dispatcher,
    state::State,
    wayland_objects::xdg_surface::xdg_toplevel::{
        InteractiveSizeConstraints, ToplevelDecorationMode,
    },
};

use super::{
    apply_interaction_intent, build_protocol_window_target, rect_contains_point,
    resize_cursor_icon_names, resize_edges_for_pointer, scene_pick_at, HitSemantics,
    InteractionIntent, WindowHit, WindowRoleSnapshot, WindowTarget,
    POINTER_INTERACTION_START_OPTIONS, PROTOCOL_INTERACTION_START_OPTIONS,
};

const POINTER_MOVE_CURSOR_ICON_NAMES: &[&str] = &["grab", "grabbing", "move"];

pub(crate) fn window_allows_compositor_interaction(target: &WindowTarget) -> bool {
    !matches!(
        target.role,
        WindowRoleSnapshot::XdgToplevel { is_maximized: true, .. }
            | WindowRoleSnapshot::XdgToplevel { is_fullscreen: true, .. }
            | WindowRoleSnapshot::Unsupported
    )
}

pub(crate) fn begin_pointer_binding(
    state: &mut State,
    dispatcher: &mut Dispatcher,
    action: config::MouseBindingAction,
) -> bool {
    let Some(hit) = window_hit_at(state, state.seat.pointer_position()) else {
        return false;
    };
    let Some(intent) = interaction_intent_for_pointer_binding(action, &hit) else {
        return false;
    };
    apply_interaction_intent(state, dispatcher, intent, POINTER_INTERACTION_START_OPTIONS)
}

pub(crate) fn begin_ssd_pointer_action(state: &mut State, dispatcher: &mut Dispatcher) -> bool {
    let pointer_pos = state.seat.pointer_position();
    let Some(intent) = window_interaction_intent_at(state, pointer_pos) else {
        return false;
    };
    apply_interaction_intent(state, dispatcher, intent, POINTER_INTERACTION_START_OPTIONS)
}

pub(crate) fn window_hit_at(state: &State, pointer_pos: (f64, f64)) -> Option<WindowHit> {
    let target = scene_pick_at(state, pointer_pos).window_target?;
    let semantics = classify_window_semantics(state, &target, pointer_pos);
    let binding_resize_edges = resize_edges_for_pointer(target.interactive_rect, pointer_pos);
    Some(WindowHit { target, semantics, binding_resize_edges })
}

pub(crate) fn ssd_hover_cursor_icon_names(state: &State) -> Option<&'static [&'static str]> {
    let pointer_pos = state.seat.pointer_position();
    let hit = window_hit_at(state, pointer_pos)?;
    match hit.semantics {
        HitSemantics::CompositorMove => Some(POINTER_MOVE_CURSOR_ICON_NAMES),
        HitSemantics::CompositorResize(edges) => Some(resize_cursor_icon_names(edges)),
        HitSemantics::SurfaceContent | HitSemantics::NoCompositorAction => None,
    }
}

pub(crate) fn request_toplevel_move(
    state: &mut State,
    dispatcher: &mut Dispatcher,
    client_id: crate::clients::ClientId,
    surface: Resolved<WlSurfaceId>,
    serial: u32,
    root_surface: Resolved<WlSurfaceId>,
    is_maximized: bool,
    is_fullscreen: bool,
) -> bool {
    if !state
        .seat
        .validate_implicit_grab_serial(client_id, surface, serial)
    {
        return false;
    }
    let Some(xdg_toplevel) = root_surface
        .try_ref(state)
        .and_then(|root| root.xdg_surface())
        .and_then(|xdg_surface| state.xdg_toplevel(xdg_surface))
    else {
        return false;
    };
    let Some(target) = build_protocol_window_target(
        state,
        root_surface,
        WindowRoleSnapshot::XdgToplevel {
            xdg_toplevel,
            size_constraints: InteractiveSizeConstraints::default(),
            is_maximized,
            is_fullscreen,
            decoration_mode: ToplevelDecorationMode::ClientSide,
        },
    ) else {
        return false;
    };
    if target.client_id() != client_id || target.surface_id() != surface.wayland_id() {
        return false;
    }
    apply_interaction_intent(
        state,
        dispatcher,
        InteractionIntent::StartMove(target),
        PROTOCOL_INTERACTION_START_OPTIONS,
    )
}

pub(crate) fn request_toplevel_resize(
    state: &mut State,
    dispatcher: &mut Dispatcher,
    client_id: crate::clients::ClientId,
    surface: Resolved<WlSurfaceId>,
    serial: u32,
    root_surface: Resolved<WlSurfaceId>,
    xdg_toplevel: Resolved<XdgToplevelId>,
    size_constraints: InteractiveSizeConstraints,
    is_maximized: bool,
    is_fullscreen: bool,
    edges: u32,
) -> bool {
    if !state
        .seat
        .validate_implicit_grab_serial(client_id, surface, serial)
    {
        return false;
    }
    let Some(target) = build_protocol_window_target(
        state,
        root_surface,
        WindowRoleSnapshot::XdgToplevel {
            xdg_toplevel,
            size_constraints,
            is_maximized,
            is_fullscreen,
            decoration_mode: ToplevelDecorationMode::ClientSide,
        },
    ) else {
        return false;
    };
    if target.client_id() != client_id || target.surface_id() != surface.wayland_id() {
        return false;
    }
    apply_interaction_intent(
        state,
        dispatcher,
        InteractionIntent::StartResize(target, edges),
        PROTOCOL_INTERACTION_START_OPTIONS,
    )
}

fn interaction_intent_for_pointer_binding(
    action: config::MouseBindingAction,
    hit: &WindowHit,
) -> Option<InteractionIntent> {
    if !window_allows_compositor_interaction(&hit.target) {
        return None;
    }
    match action {
        config::MouseBindingAction::MoveWindow => Some(InteractionIntent::StartMove(hit.target)),
        config::MouseBindingAction::ResizeWindow => {
            Some(InteractionIntent::StartResize(hit.target, hit.binding_resize_edges))
        }
    }
}

fn classify_window_semantics(
    state: &State,
    target: &WindowTarget,
    pointer_pos: (f64, f64),
) -> HitSemantics {
    if !window_allows_compositor_interaction(target) {
        return HitSemantics::NoCompositorAction;
    }
    let Some(layout) = state.root_surface_ssd_layout(target.root_surface) else {
        return HitSemantics::SurfaceContent;
    };
    if !rect_contains_point(layout.frame_rect, pointer_pos)
        || rect_contains_point(layout.content_rect, pointer_pos)
    {
        return HitSemantics::SurfaceContent;
    }

    let x = pointer_pos.0.round() as i32;
    let y = pointer_pos.1.round() as i32;
    let border_resize_height = state
        .root_surface_ssd_insets(target.root_surface)
        .map(|insets| insets.left.max(insets.right).max(insets.bottom).max(6))
        .unwrap_or(6);
    let top_resize_height =
        (layout.content_rect.y - layout.frame_rect.y).clamp(1, border_resize_height);
    let top_resize_limit = layout.frame_rect.y.saturating_add(top_resize_height);

    if x < layout.content_rect.x {
        return HitSemantics::CompositorResize(if y < layout.content_rect.y {
            ResizeEdge::TopLeft as u32
        } else if y >= layout.content_rect.y2() {
            ResizeEdge::BottomLeft as u32
        } else {
            ResizeEdge::Left as u32
        });
    }
    if x >= layout.content_rect.x2() {
        return HitSemantics::CompositorResize(if y < layout.content_rect.y {
            ResizeEdge::TopRight as u32
        } else if y >= layout.content_rect.y2() {
            ResizeEdge::BottomRight as u32
        } else {
            ResizeEdge::Right as u32
        });
    }
    if y >= layout.content_rect.y2() {
        return HitSemantics::CompositorResize(ResizeEdge::Bottom as u32);
    }
    if y < layout.content_rect.y {
        if y < top_resize_limit {
            return HitSemantics::CompositorResize(ResizeEdge::Top as u32);
        }
        return HitSemantics::CompositorMove;
    }

    HitSemantics::SurfaceContent
}

fn window_interaction_intent_at(
    state: &State,
    pointer_pos: (f64, f64),
) -> Option<InteractionIntent> {
    let hit = window_hit_at(state, pointer_pos)?;
    match hit.semantics {
        HitSemantics::CompositorMove => Some(InteractionIntent::StartMove(hit.target)),
        HitSemantics::CompositorResize(edges) => {
            Some(InteractionIntent::StartResize(hit.target, edges))
        }
        HitSemantics::SurfaceContent | HitSemantics::NoCompositorAction => None,
    }
}
