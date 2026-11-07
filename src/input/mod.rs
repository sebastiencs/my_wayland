pub(crate) mod focus;
pub(crate) mod grab;
pub(crate) mod ime;
pub(crate) mod interaction;
pub(crate) mod pick;
pub(crate) mod pointer_confine;
pub(crate) mod pointer_lock;
pub(crate) mod types;

pub use types::SurfaceFocus;
pub(crate) use {
    focus::{
        focus_after_client_disconnect, maybe_dismiss_popup_grab_on_button, popup_keyboard_focus,
        refresh_active_drag_focus, refresh_drag_focus, refresh_keyboard_focus,
        refresh_keyboard_focus_with_snapshot, refresh_pointer_focus, resolve_activation_target,
        set_keyboard_focus, set_keyboard_focus_from_surface, validate_activation_serial,
    },
    grab::{
        abort_pointer_interactive_grab, apply_interaction_intent, apply_pointer_interactive_grab,
        finish_pointer_interactive_grab, recover_pointer_after_interactive_grab_loss,
        resize_cursor_icon_names, tiled_drag_tracks_surface, tiled_resize_active_root_surfaces,
    },
    ime::ImeState,
    interaction::{
        apply_pointer_press_keyboard_focus, begin_decoration_pointer_action, begin_pointer_binding,
        decoration_hover_cursor_icon_names, request_toplevel_move, request_toplevel_resize,
        window_hit_at,
    },
    pick::{
        build_protocol_move_target, build_protocol_resize_target, rect_contains_point,
        resize_edges_for_pointer, scene_pick_at, scene_pick_for_pointer, surface_focus_pick_at,
    },
    types::{
        ActiveGrabState, HitSemantics, InteractionIntent, InteractionStartOptions,
        PointerInteractiveGrab, ScenePick, SurfaceFocusPick, SurfaceRect, WindowHit,
        WindowRoleSnapshot, WindowSnapshot, WindowTarget, POINTER_INTERACTION_START_OPTIONS,
        PROTOCOL_INTERACTION_START_OPTIONS,
    },
};
