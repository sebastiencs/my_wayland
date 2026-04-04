use crate::{
    clients::{ClientId, Resolved},
    protocol_generated::{WlSurfaceId, XdgToplevelId},
    region::RectI,
    tiling::TiledResizeState,
    wayland_objects::xdg_surface::xdg_toplevel::{
        InteractiveSizeConstraints, ToplevelDecorationMode,
    },
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, derive_more::Deref)]
pub struct SurfaceFocus {
    pub surface: Resolved<WlSurfaceId>,
}

impl SurfaceFocus {
    pub fn new(surface: Resolved<WlSurfaceId>) -> Self {
        Self { surface }
    }

    pub fn surface_id(self) -> WlSurfaceId {
        self.surface.wayland_id()
    }
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
        xdg_toplevel: Resolved<XdgToplevelId>,
        size_constraints: InteractiveSizeConstraints,
        is_maximized: bool,
        is_fullscreen: bool,
        decoration_mode: ToplevelDecorationMode,
    },
    Unsupported,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct WindowSnapshot {
    pub root_surface: Resolved<WlSurfaceId>,
    pub role: WindowRoleSnapshot,
    pub initial_root_pos: (i32, i32),
    pub initial_size: (i32, i32),
    pub content_rect: RectI,
    pub interactive_rect: RectI,
    pub outer_rect: RectI,
}

impl WindowSnapshot {
    pub fn client_id(self) -> ClientId {
        self.root_surface.client_id()
    }

    pub fn surface_id(self) -> WlSurfaceId {
        self.root_surface.wayland_id()
    }

    pub fn object_id(self) -> crate::state::ObjectId {
        self.root_surface.object_id()
    }
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
        root_surface: Resolved<WlSurfaceId>,
        initial_pointer_pos: (f64, f64),
        initial_root_pos: (i32, i32),
    },
    Resize {
        root_surface: Resolved<WlSurfaceId>,
        xdg_toplevel: Resolved<XdgToplevelId>,
        size_constraints: InteractiveSizeConstraints,
        edges: u32,
        initial_pointer_pos: (f64, f64),
        initial_root_pos: (i32, i32),
        initial_size: (i32, i32),
        last_size: (i32, i32),
    },
    ResizeTiled {
        root_surface: Resolved<WlSurfaceId>,
        edges: u32,
        initial_pointer_pos: (f64, f64),
        resize_state: TiledResizeState,
    },
}

pub(crate) type ActiveGrabState = ActiveGrab;
pub(crate) type PointerFocus = SurfaceFocusPick;
pub(crate) type PointerInteractiveGrab = ActiveGrab;
pub(crate) type WindowTarget = WindowSnapshot;
