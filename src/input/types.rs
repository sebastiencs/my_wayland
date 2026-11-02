use crate::{
    clients::{ClientId, Resolved},
    protocol_generated::{WlSurfaceId, XdgToplevelId},
    region::RectI,
    root_geometry::WindowTargetGeometry,
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
    pub geometry: WindowTargetGeometry,
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
}

pub(crate) const POINTER_INTERACTION_START_OPTIONS: InteractionStartOptions =
    InteractionStartOptions { prepare_window: true };

pub(crate) const PROTOCOL_INTERACTION_START_OPTIONS: InteractionStartOptions =
    InteractionStartOptions { prepare_window: false };

#[derive(Clone, Copy, Debug)]
pub(crate) enum ActiveGrab {
    Move {
        root_surface: Resolved<WlSurfaceId>,
        initial_pointer_pos: (f64, f64),
        initial_geometry: WindowTargetGeometry,
    },
    Resize {
        root_surface: Resolved<WlSurfaceId>,
        xdg_toplevel: Resolved<XdgToplevelId>,
        size_constraints: InteractiveSizeConstraints,
        edges: u32,
        initial_pointer_pos: (f64, f64),
        initial_geometry: WindowTargetGeometry,
        last_size: (i32, i32),
    },
    ResizeTiled {
        root_surface: Resolved<WlSurfaceId>,
        edges: u32,
        initial_pointer_pos: (f64, f64),
        resize_state: TiledResizeState,
    },
    /// Compositor move of a tiled window. The window stays tiled until the pointer
    /// travels past `threshold`, then it becomes a temporary floating preview.
    /// The preview frame stays inside `source_output_bounds`.
    MoveTiled {
        root_surface: Resolved<WlSurfaceId>,
        xdg_toplevel: Resolved<XdgToplevelId>,
        size_constraints: InteractiveSizeConstraints,
        decoration_mode: ToplevelDecorationMode,
        origin_pointer: (f64, f64),
        threshold: i32,
        picked_up: bool,
        grab_pointer: (f64, f64),
        grab_geometry: WindowTargetGeometry,
        /// Width and height of the preview frame, including server-side decoration.
        preview_size: (i32, i32),
        /// Preview-frame origin minus the window root. Zero when they coincide.
        frame_from_root: (i32, i32),
        /// Output rectangle frozen at pickup. The preview frame stays inside it.
        source_output_bounds: RectI,
    },
}

pub(crate) type ActiveGrabState = ActiveGrab;
pub(crate) type PointerFocus = SurfaceFocusPick;
pub(crate) type PointerInteractiveGrab = ActiveGrab;
pub(crate) type WindowTarget = WindowSnapshot;
