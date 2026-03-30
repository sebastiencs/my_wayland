use crate::{
    clients::ClientId,
    protocol_generated::{WlSurfaceId, XdgToplevelId},
    region::RectI,
    state::ObjectId,
    tiling::TiledResizeState,
    wayland_objects::xdg_surface::xdg_toplevel::{
        InteractiveSizeConstraints, ToplevelDecorationMode,
    },
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SurfaceFocus {
    pub client_id: ClientId,
    pub surface_id: WlSurfaceId,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SurfaceRect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SurfaceFocusPick {
    pub target: SurfaceFocus,
    pub rect: SurfaceRect,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct ScenePick {
    pub surface_focus: Option<SurfaceFocusPick>,
    pub window_target: Option<WindowSnapshot>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum WindowRoleSnapshot {
    XdgToplevel {
        xdg_toplevel_id: XdgToplevelId,
        size_constraints: InteractiveSizeConstraints,
        is_maximized: bool,
        is_fullscreen: bool,
        decoration_mode: ToplevelDecorationMode,
    },
    Unsupported,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct WindowSnapshot {
    pub client_id: ClientId,
    pub surface_id: WlSurfaceId,
    pub root_surface_object_id: ObjectId,
    pub role: WindowRoleSnapshot,
    pub initial_root_pos: (i32, i32),
    pub initial_size: (i32, i32),
    pub content_rect: RectI,
    pub interactive_rect: RectI,
    pub outer_rect: RectI,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum HitSemantics {
    SurfaceContent,
    CompositorMove,
    CompositorResize(u32),
    NoCompositorAction,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum InteractionIntent {
    FocusSurface(SurfaceFocus),
    StartMove(WindowSnapshot),
    StartResize(WindowSnapshot, u32),
    DismissPopupGrab,
    None,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct WindowHit {
    pub target: WindowSnapshot,
    pub semantics: HitSemantics,
    pub binding_resize_edges: u32,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct InteractionStartOptions {
    pub prepare_window: bool,
    pub promote_tiled_on_move: bool,
}

pub(crate) const POINTER_INTERACTION_START_OPTIONS: InteractionStartOptions =
    InteractionStartOptions { prepare_window: true, promote_tiled_on_move: true };

pub(crate) const PROTOCOL_INTERACTION_START_OPTIONS: InteractionStartOptions =
    InteractionStartOptions { prepare_window: false, promote_tiled_on_move: false };

#[derive(Clone, Copy, Debug)]
pub(crate) enum ActiveGrab {
    Move {
        client_id: ClientId,
        root_surface_object_id: ObjectId,
        initial_pointer_pos: (f64, f64),
        initial_root_pos: (i32, i32),
    },
    Resize {
        client_id: ClientId,
        root_surface_object_id: ObjectId,
        xdg_toplevel_id: XdgToplevelId,
        size_constraints: InteractiveSizeConstraints,
        edges: u32,
        initial_pointer_pos: (f64, f64),
        initial_root_pos: (i32, i32),
        initial_size: (i32, i32),
        last_size: (i32, i32),
    },
    ResizeTiled {
        client_id: ClientId,
        root_surface_object_id: ObjectId,
        edges: u32,
        initial_pointer_pos: (f64, f64),
        resize_state: TiledResizeState,
    },
}

pub(crate) type ActiveGrabState = ActiveGrab;
pub(crate) type PointerFocus = SurfaceFocusPick;
pub(crate) type PointerInteractiveGrab = ActiveGrab;
pub(crate) type WindowTarget = WindowSnapshot;
