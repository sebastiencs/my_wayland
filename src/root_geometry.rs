use {
    crate::{
        clients::Resolved,
        config::DecorationInsets,
        protocol_generated::WlSurfaceId,
        region::RectI,
        state::State,
        tiling::LayoutAllocation,
        transform::SizeI,
        wayland_objects::{
            wl_surface::WlSurface,
            xdg_surface::{xdg_toplevel::ToplevelDecorationMode, XdgSurface},
        },
    },
    std::cell::Cell,
};

/// Compositor-owned decoration layout for a root surface.
/// This exists only when the compositor is responsible for SSD chrome.
/// It separates shell frame semantics from generic client window geometry.
/// Input code should treat this as compositor chrome, not client content.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DecorationFrameLayout {
    /// Full compositor-owned frame rectangle in global coordinates.
    pub frame_rect: RectI,
    /// Client-declared window rectangle inside the compositor frame.
    pub client_window_rect: RectI,
    /// Optional titlebar area inside the compositor-owned decoration frame.
    pub titlebar_rect: Option<RectI>,
}

/// Final committed geometry snapshot for one root surface.
/// Each field has a single role: placement, client window, whole-window targeting, compositor chrome, or visuals.
/// This is the single geometry object consumed by configure, input, and rendering code.
/// Callers should read the field matching their subsystem instead of inferring semantics.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CommittedRootGeometry {
    /// Layout/compositor placement rectangle for the root surface as a whole.
    pub placement_rect: RectI,
    /// Client-visible window rectangle used for protocol/configure semantics.
    pub client_window_rect: RectI,
    /// Rectangle that should answer whole-window targeting and focus hits.
    pub window_target_rect: RectI,
    /// Rectangle that should answer compositor bindings and resize-edge inference.
    pub binding_rect: RectI,
    /// Full visual bounds used for rendering, clips, and damage calculations.
    pub visual_outer_rect: RectI,
    /// Explicit compositor-owned SSD chrome layout, if any.
    pub compositor_decoration_layout: Option<DecorationFrameLayout>,
    /// Current root `wl_surface` origin in global compositor coordinates.
    pub root_pos: (i32, i32),
    /// Tiled edge mask exported to xdg_toplevel configure state.
    pub tiled_edges: u32,
    /// Whether this snapshot represents fullscreen layout semantics.
    pub fullscreen: bool,
    /// Effective decoration mode used when this snapshot was built.
    pub decoration_mode: Option<ToplevelDecorationMode>,
    /// Effective per-edge decoration insets used by final geometry resolution.
    pub decoration_insets: DecorationInsets,
    /// Effective xdg window geometry after clamping to known surface bounds.
    pub effective_window_geometry: Option<RectI>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ConfigureGeometry {
    pub window_rect: RectI,
    pub size: (i32, i32),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WindowTargetGeometry {
    pub root_pos: (i32, i32),
    pub resize_size: (i32, i32),
    pub target_rect: RectI,
    pub binding_rect: RectI,
}

pub fn project_configure_geometry(
    geometry: CommittedRootGeometry,
    pending_decoration_mode: ToplevelDecorationMode,
    configured_decoration_insets: DecorationInsets,
) -> ConfigureGeometry {
    let window_rect =
        if geometry.fullscreen || pending_decoration_mode == ToplevelDecorationMode::ClientSide {
            geometry.placement_rect
        } else if geometry.decoration_mode == Some(ToplevelDecorationMode::ServerSide) {
            geometry.client_window_rect
        } else {
            shrink_rect(geometry.placement_rect, configured_decoration_insets)
        };
    ConfigureGeometry { window_rect, size: (window_rect.width, window_rect.height) }
}

pub fn project_window_target_geometry(geometry: CommittedRootGeometry) -> WindowTargetGeometry {
    WindowTargetGeometry {
        root_pos: geometry.root_pos,
        resize_size: (geometry.client_window_rect.width, geometry.client_window_rect.height),
        target_rect: geometry.window_target_rect,
        binding_rect: geometry.binding_rect,
    }
}

pub fn resolve_committed_root_geometry_with_overrides(
    state: &State,
    surface_object_id: Resolved<WlSurfaceId>,
    surface: &WlSurface,
    overrides: GeometryBuildOverrides,
) -> Option<CommittedRootGeometry> {
    cached_geometry_reusable_for_overrides(state, surface_object_id, overrides).or_else(|| {
        build_committed_root_geometry(
            state,
            surface_object_id,
            surface,
            crate::tiling::layout_allocation_for_surface(state, surface_object_id),
            overrides,
        )
    })
}

fn cached_geometry_reusable_for_overrides(
    state: &State,
    surface_object_id: Resolved<WlSurfaceId>,
    overrides: GeometryBuildOverrides,
) -> Option<CommittedRootGeometry> {
    state
        .root_geometry_cache
        .get(&surface_object_id)
        .map(Cell::get)
        .filter(|entry| entry.is_fresh())
        .and_then(|entry| entry.geometry)
        .filter(|geometry| {
            (overrides.decoration_mode.is_none()
                || geometry.decoration_mode == overrides.decoration_mode)
                && overrides.explicit_window_geometry.is_none()
                && overrides.decoration_insets.is_none()
                && overrides
                    .root_pos
                    .is_none_or(|root_pos| geometry.root_pos == root_pos)
        })
}

pub fn resolve_protocol_move_target_geometry(
    state: &State,
    surface_object_id: Resolved<WlSurfaceId>,
    surface: &WlSurface,
    decoration_mode: ToplevelDecorationMode,
) -> Option<WindowTargetGeometry> {
    let geometry = resolve_committed_root_geometry_with_overrides(
        state,
        surface_object_id,
        surface,
        GeometryBuildOverrides {
            decoration_mode: Some(decoration_mode),
            explicit_window_geometry: None,
            decoration_insets: None,
            root_pos: Some(surface.root_position()),
        },
    )?;
    Some(project_window_target_geometry(geometry))
}

pub fn resolve_protocol_resize_target_geometry(
    state: &State,
    surface_object_id: Resolved<WlSurfaceId>,
    surface: &WlSurface,
    configure_decoration_mode: ToplevelDecorationMode,
) -> Option<WindowTargetGeometry> {
    let geometry = resolve_committed_root_geometry_with_overrides(
        state,
        surface_object_id,
        surface,
        GeometryBuildOverrides {
            decoration_mode: Some(configure_decoration_mode),
            explicit_window_geometry: None,
            decoration_insets: None,
            root_pos: Some(surface.root_position()),
        },
    )?;
    Some(WindowTargetGeometry {
        root_pos: geometry.root_pos,
        resize_size: project_configure_geometry(
            geometry,
            configure_decoration_mode,
            state.configured_decoration_insets(),
        )
        .size,
        target_rect: geometry.window_target_rect,
        binding_rect: geometry.binding_rect,
    })
}

#[derive(Clone, Copy, Debug, Default)]
pub struct GeometryBuildOverrides {
    pub decoration_mode: Option<ToplevelDecorationMode>,
    pub explicit_window_geometry: Option<RectI>,
    pub decoration_insets: Option<DecorationInsets>,
    pub root_pos: Option<(i32, i32)>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum GeometryOwnership {
    Compositor,
    Client,
}

impl GeometryOwnership {
    fn for_decoration_mode(decoration_mode: ToplevelDecorationMode) -> Self {
        match decoration_mode {
            ToplevelDecorationMode::ServerSide => Self::Compositor,
            ToplevelDecorationMode::ClientSide => Self::Client,
        }
    }

    fn decoration_insets(
        self,
        fallback: DecorationInsets,
        surface: &WlSurface,
        effective_window_geometry: Option<RectI>,
    ) -> DecorationInsets {
        match self {
            Self::Compositor => fallback,
            Self::Client => effective_window_geometry
                .and_then(|window_geometry| {
                    decoration_insets_from_window_geometry(surface, window_geometry)
                })
                .unwrap_or(fallback),
        }
    }
}

pub fn effective_decoration_insets(
    state: &State,
    surface_object_id: Resolved<WlSurfaceId>,
) -> DecorationInsets {
    let Some(surface) = state
        .objects
        .try_ref_object::<WlSurface>(surface_object_id.object_id())
    else {
        return state.configured_decoration_insets();
    };
    effective_decoration_insets_for_surface(state, surface)
}

pub fn effective_decoration_insets_for_surface(
    state: &State,
    surface: &WlSurface,
) -> DecorationInsets {
    let fallback = state.configured_decoration_insets();
    let Some(toplevel) = surface
        .xdg_surface()
        .and_then(|xdg_surface| state.xdg_toplevel(xdg_surface))
        .and_then(|toplevel| toplevel.try_ref(state))
    else {
        return fallback;
    };
    let geometry_ownership = GeometryOwnership::for_decoration_mode(toplevel.decoration_mode());
    let effective_window_geometry = toplevel.window_geometry().and_then(|window_geometry| {
        effective_window_geometry_for_surface(state, surface, Some(window_geometry))
    });
    geometry_ownership.decoration_insets(fallback, surface, effective_window_geometry)
}

pub fn decoration_insets_from_window_geometry(
    surface: &WlSurface,
    window_geometry: RectI,
) -> Option<DecorationInsets> {
    let logical_size = surface.logical_size()?;
    decoration_insets_from_window_geometry_and_logical_size(logical_size, window_geometry)
}

pub fn decoration_insets_from_window_geometry_and_logical_size(
    logical_size: SizeI,
    window_geometry: RectI,
) -> Option<DecorationInsets> {
    let w = logical_size.width;
    let h = logical_size.height;
    let gx2 = window_geometry.x2();
    let gy2 = window_geometry.y2();
    Some(DecorationInsets {
        left: window_geometry
            .x
            .saturating_neg()
            .max(0)
            .saturating_add(window_geometry.x.max(0)),
        top: window_geometry
            .y
            .saturating_neg()
            .max(0)
            .saturating_add(window_geometry.y.max(0)),
        right: gx2
            .saturating_sub(w)
            .max(0)
            .saturating_add(w.saturating_sub(gx2).max(0)),
        bottom: gy2
            .saturating_sub(h)
            .max(0)
            .saturating_add(h.saturating_sub(gy2).max(0)),
    })
}

pub fn effective_window_geometry_for_surface(
    state: &State,
    surface: &WlSurface,
    explicit_window_geometry: Option<RectI>,
) -> Option<RectI> {
    let surface_bounds = WlSurface::local_surface_tree_bounds_from_ref(state, surface);
    match (surface_bounds, explicit_window_geometry) {
        (Some(surface_bounds), Some(raw)) => {
            Some(clamp_window_geometry_to_surface_bounds(raw, surface_bounds))
        }
        (Some(surface_bounds), None) => Some(surface_bounds),
        (None, Some(raw)) => Some(raw),
        (None, None) => None,
    }
}

pub fn build_committed_root_geometry(
    state: &State,
    surface_object_id: Resolved<WlSurfaceId>,
    surface: &WlSurface,
    layout_allocation: Option<LayoutAllocation>,
    overrides: GeometryBuildOverrides,
) -> Option<CommittedRootGeometry> {
    let xdg_surface = surface
        .xdg_surface()
        .and_then(|xdg_surface| xdg_surface.try_ref(state));
    let toplevel = surface
        .xdg_surface()
        .and_then(|xdg_surface| state.xdg_toplevel(xdg_surface));
    let decoration_mode = overrides.decoration_mode.or_else(|| {
        toplevel.and_then(|toplevel| {
            toplevel
                .try_ref(state)
                .map(|toplevel| toplevel.decoration_mode())
        })
    });
    let explicit_window_geometry = overrides
        .explicit_window_geometry
        .or_else(|| xdg_surface.and_then(XdgSurface::current_window_geometry));
    let effective_window_geometry =
        effective_window_geometry_for_surface(state, surface, explicit_window_geometry);
    let root_pos = overrides
        .root_pos
        .unwrap_or_else(|| surface.root_position());
    let fullscreen = layout_allocation.is_some_and(|allocation| allocation.fullscreen);
    let decoration_insets = overrides.decoration_insets.unwrap_or_else(|| {
        decoration_insets_for_surface(
            state,
            surface_object_id,
            surface,
            decoration_mode,
            effective_window_geometry,
            fullscreen,
        )
    });

    let local_surface_bounds = WlSurface::local_surface_tree_bounds_from_ref(state, surface);
    let (
        placement_rect,
        client_window_rect,
        window_target_rect,
        binding_rect,
        visual_outer_rect,
        computed_root_pos,
        tiled_edges,
    ) = if let Some(allocation) = layout_allocation {
        let placement_rect = allocation.frame_rect;
        let fallback_client_window_rect =
            if decoration_mode == Some(ToplevelDecorationMode::ClientSide) {
                placement_rect
            } else if allocation.fullscreen {
                placement_rect
            } else {
                shrink_rect(placement_rect, decoration_insets)
            };
        let anchor_rect = match decoration_mode {
            Some(ToplevelDecorationMode::ClientSide) => placement_rect,
            _ => fallback_client_window_rect,
        };
        // With server-side decorations the compositor owns the frame; `client_window_rect` is the
        // content region. Client `set_window_geometry` may still carry CSD-era insets (e.g. GTK);
        // subtracting them here shifts the root surface and misaligns pixels vs our SSD border.
        let computed_root_pos = match decoration_mode {
            Some(ToplevelDecorationMode::ServerSide) => {
                (fallback_client_window_rect.x, fallback_client_window_rect.y)
            }
            _ => effective_window_geometry
                .map(|window_geometry| {
                    (
                        anchor_rect.x.saturating_sub(window_geometry.x),
                        anchor_rect.y.saturating_sub(window_geometry.y),
                    )
                })
                .unwrap_or((fallback_client_window_rect.x, fallback_client_window_rect.y)),
        };
        let client_window_rect = anchor_rect;
        let compositor_decoration_layout = compositor_decoration_layout(
            placement_rect,
            client_window_rect,
            decoration_insets,
            decoration_mode,
            fullscreen,
        );
        let visual_outer_rect =
            translated_surface_tree_bounds(local_surface_bounds, computed_root_pos)
                .unwrap_or(client_window_rect);
        let window_target_rect = compositor_decoration_layout
            .map(|layout| layout.frame_rect)
            .unwrap_or(visual_outer_rect);
        let binding_rect = compositor_decoration_layout
            .map(|layout| layout.frame_rect)
            .unwrap_or(client_window_rect);
        (
            placement_rect,
            client_window_rect,
            window_target_rect,
            binding_rect,
            visual_outer_rect,
            computed_root_pos,
            allocation.tiled_edges,
        )
    } else {
        let translated_surface_bounds =
            translated_surface_tree_bounds(local_surface_bounds, root_pos);
        let client_window_rect = effective_window_geometry
            .and_then(|window_geometry| {
                project_window_geometry_from_surface_bounds(
                    window_geometry,
                    translated_surface_bounds,
                    local_surface_bounds,
                    root_pos,
                )
            })
            .or(translated_surface_bounds)?;
        let placement_rect = if compositor_decorations_enabled(decoration_mode, fullscreen) {
            inflate_rect_with_insets(client_window_rect, decoration_insets)
        } else {
            client_window_rect
        };
        let compositor_decoration_layout = compositor_decoration_layout(
            placement_rect,
            client_window_rect,
            decoration_insets,
            decoration_mode,
            fullscreen,
        );
        let visual_outer_rect = translated_surface_bounds.unwrap_or(client_window_rect);
        let window_target_rect = compositor_decoration_layout
            .map(|layout| layout.frame_rect)
            .unwrap_or(visual_outer_rect);
        let binding_rect = compositor_decoration_layout
            .map(|layout| layout.frame_rect)
            .unwrap_or_else(|| {
                non_ssd_binding_rect(surface.logical_size(), effective_window_geometry, root_pos)
                    .unwrap_or(client_window_rect)
            });
        (
            placement_rect,
            client_window_rect,
            window_target_rect,
            binding_rect,
            visual_outer_rect,
            root_pos,
            0,
        )
    };

    let compositor_decoration_layout = compositor_decoration_layout(
        placement_rect,
        client_window_rect,
        decoration_insets,
        decoration_mode,
        fullscreen,
    );

    if decoration_mode == Some(ToplevelDecorationMode::ServerSide)
        && !fullscreen
        && (decoration_insets.top > 0
            || decoration_insets.right > 0
            || decoration_insets.bottom > 0
            || decoration_insets.left > 0)
    {
        log::debug!(
            target: "state_management::root_geometry",
            "ssd_insets surface={:?} layout_allocated={} placement={:?} client_window={:?} \
             visual_outer={:?} window_target={:?} binding={:?} root_pos={:?} \
             surface_root_pos={:?} local_surface_bounds={:?} insets=({},{},{},{}) \
             effective_wg={:?} compositor_decoration={:?}",
            surface_object_id,
            layout_allocation.is_some(),
            placement_rect,
            client_window_rect,
            visual_outer_rect,
            window_target_rect,
            binding_rect,
            computed_root_pos,
            surface.root_position(),
            local_surface_bounds,
            decoration_insets.top,
            decoration_insets.right,
            decoration_insets.bottom,
            decoration_insets.left,
            effective_window_geometry,
            compositor_decoration_layout,
        );
    }

    Some(CommittedRootGeometry {
        placement_rect,
        client_window_rect,
        window_target_rect,
        binding_rect,
        visual_outer_rect,
        compositor_decoration_layout,
        root_pos: computed_root_pos,
        tiled_edges,
        fullscreen,
        decoration_mode,
        decoration_insets,
        effective_window_geometry,
    })
}

pub fn predictive_tiled_geometry(
    layout_allocation: LayoutAllocation,
    decoration_mode: ToplevelDecorationMode,
    explicit_window_geometry: Option<RectI>,
    decoration_insets: DecorationInsets,
) -> CommittedRootGeometry {
    let placement_rect = layout_allocation.frame_rect;
    let decoration_insets =
        if layout_allocation.fullscreen { DecorationInsets::default() } else { decoration_insets };
    let fallback_client_window_rect = if decoration_mode == ToplevelDecorationMode::ClientSide {
        placement_rect
    } else if layout_allocation.fullscreen {
        placement_rect
    } else {
        shrink_rect(placement_rect, decoration_insets)
    };
    let anchor_rect = if decoration_mode == ToplevelDecorationMode::ClientSide {
        placement_rect
    } else {
        fallback_client_window_rect
    };
    let root_pos = match decoration_mode {
        ToplevelDecorationMode::ServerSide => {
            (fallback_client_window_rect.x, fallback_client_window_rect.y)
        }
        _ => explicit_window_geometry
            .map(|window_geometry| {
                (
                    anchor_rect.x.saturating_sub(window_geometry.x),
                    anchor_rect.y.saturating_sub(window_geometry.y),
                )
            })
            .unwrap_or((fallback_client_window_rect.x, fallback_client_window_rect.y)),
    };
    let client_window_rect = anchor_rect;
    let compositor_decoration_layout = compositor_decoration_layout(
        placement_rect,
        client_window_rect,
        decoration_insets,
        Some(decoration_mode),
        layout_allocation.fullscreen,
    );
    CommittedRootGeometry {
        placement_rect,
        client_window_rect,
        window_target_rect: compositor_decoration_layout
            .map(|layout| layout.frame_rect)
            .unwrap_or(placement_rect),
        binding_rect: compositor_decoration_layout
            .map(|layout| layout.frame_rect)
            .unwrap_or_else(|| {
                non_ssd_binding_rect(
                    Some(SizeI { width: placement_rect.width, height: placement_rect.height }),
                    explicit_window_geometry,
                    root_pos,
                )
                .unwrap_or(client_window_rect)
            }),
        visual_outer_rect: client_window_rect,
        compositor_decoration_layout,
        root_pos,
        tiled_edges: layout_allocation.tiled_edges,
        fullscreen: layout_allocation.fullscreen,
        decoration_mode: Some(decoration_mode),
        decoration_insets,
        effective_window_geometry: explicit_window_geometry,
    }
}

fn decoration_insets_for_surface(
    state: &State,
    _surface_object_id: Resolved<WlSurfaceId>,
    surface: &WlSurface,
    decoration_mode: Option<ToplevelDecorationMode>,
    effective_window_geometry: Option<RectI>,
    fullscreen: bool,
) -> DecorationInsets {
    if fullscreen {
        return DecorationInsets::default();
    }
    let fallback = state.configured_decoration_insets();
    match decoration_mode {
        Some(ToplevelDecorationMode::ServerSide) => fallback,
        Some(ToplevelDecorationMode::ClientSide) => effective_window_geometry
            .and_then(|window_geometry| {
                decoration_insets_from_window_geometry(surface, window_geometry)
            })
            .unwrap_or(fallback),
        None => DecorationInsets::default(),
    }
}

fn non_ssd_binding_rect(
    logical_size: Option<SizeI>,
    effective_window_geometry: Option<RectI>,
    root_pos: (i32, i32),
) -> Option<RectI> {
    let Some(window_geometry) = effective_window_geometry else {
        return None;
    };
    let logical_size = logical_size?;
    let x1 = if window_geometry.x >= 0 && window_geometry.x2() <= logical_size.width {
        0
    } else {
        window_geometry.x.max(0)
    };
    let y1 = if window_geometry.y >= 0 && window_geometry.y2() <= logical_size.height {
        0
    } else {
        window_geometry.y.max(0)
    };
    let x2 = if window_geometry.x >= 0 && window_geometry.x2() <= logical_size.width {
        logical_size.width
    } else {
        window_geometry.x2().min(logical_size.width)
    };
    let y2 = if window_geometry.y >= 0 && window_geometry.y2() <= logical_size.height {
        logical_size.height
    } else {
        window_geometry.y2().min(logical_size.height)
    };
    RectI::new(
        root_pos.0.saturating_add(x1),
        root_pos.1.saturating_add(y1),
        x2.saturating_sub(x1).max(1),
        y2.saturating_sub(y1).max(1),
    )
}

fn compositor_decorations_enabled(
    decoration_mode: Option<ToplevelDecorationMode>,
    fullscreen: bool,
) -> bool {
    decoration_mode == Some(ToplevelDecorationMode::ServerSide) && !fullscreen
}

fn compositor_decoration_layout(
    frame_rect: RectI,
    client_window_rect: RectI,
    decoration_insets: DecorationInsets,
    decoration_mode: Option<ToplevelDecorationMode>,
    fullscreen: bool,
) -> Option<DecorationFrameLayout> {
    if !compositor_decorations_enabled(decoration_mode, fullscreen)
        || frame_rect == client_window_rect
    {
        return None;
    }
    Some(DecorationFrameLayout {
        frame_rect,
        client_window_rect,
        titlebar_rect: (decoration_insets.top > 0)
            .then(|| {
                RectI::new(
                    client_window_rect.x,
                    frame_rect.y,
                    client_window_rect.width,
                    decoration_insets.top,
                )
            })
            .flatten(),
    })
}

fn translated_surface_tree_bounds(
    local_surface_bounds: Option<RectI>,
    root_pos: (i32, i32),
) -> Option<RectI> {
    local_surface_bounds.and_then(|local_bounds| {
        RectI::new(
            root_pos.0.saturating_add(local_bounds.x),
            root_pos.1.saturating_add(local_bounds.y),
            local_bounds.width,
            local_bounds.height,
        )
    })
}

fn project_window_geometry_from_surface_bounds(
    window_geometry: RectI,
    translated_surface_bounds: Option<RectI>,
    local_surface_bounds: Option<RectI>,
    root_pos: (i32, i32),
) -> Option<RectI> {
    match (translated_surface_bounds, local_surface_bounds) {
        (Some(global_bounds), Some(local_bounds)) => RectI::new(
            global_bounds
                .x
                .saturating_add(window_geometry.x.saturating_sub(local_bounds.x)),
            global_bounds
                .y
                .saturating_add(window_geometry.y.saturating_sub(local_bounds.y)),
            window_geometry.width,
            window_geometry.height,
        ),
        _ => RectI::new(
            root_pos.0.saturating_add(window_geometry.x),
            root_pos.1.saturating_add(window_geometry.y),
            window_geometry.width,
            window_geometry.height,
        ),
    }
}

fn inflate_rect_with_insets(rect: RectI, insets: DecorationInsets) -> RectI {
    RectI::from_extents(
        rect.x.saturating_sub(insets.left.max(0)),
        rect.y.saturating_sub(insets.top.max(0)),
        rect.x2().saturating_add(insets.right.max(0)),
        rect.y2().saturating_add(insets.bottom.max(0)),
    )
    .unwrap_or(rect)
}

fn shrink_rect(rect: RectI, insets: DecorationInsets) -> RectI {
    if rect.width <= 0 || rect.height <= 0 {
        return rect;
    }
    let x1 = rect.x.saturating_add(insets.left.max(0));
    let y1 = rect.y.saturating_add(insets.top.max(0));
    let x2 = rect.x2().saturating_sub(insets.right.max(0));
    let y2 = rect.y2().saturating_sub(insets.bottom.max(0));
    RectI::from_extents(x1, y1, x2.max(x1.saturating_add(1)), y2.max(y1.saturating_add(1)))
        .unwrap_or(rect)
}

fn clamp_window_geometry_to_surface_bounds(window_geometry: RectI, surface_bounds: RectI) -> RectI {
    let x1 = window_geometry
        .x
        .clamp(surface_bounds.x, surface_bounds.x2());
    let y1 = window_geometry
        .y
        .clamp(surface_bounds.y, surface_bounds.y2());
    let x2 = window_geometry
        .x2()
        .clamp(surface_bounds.x, surface_bounds.x2());
    let y2 = window_geometry
        .y2()
        .clamp(surface_bounds.y, surface_bounds.y2());
    RectI::from_extents(x1, y1, x2, y2).unwrap_or(surface_bounds)
}

#[cfg(test)]
mod tests {
    use {
        super::*,
        crate::{
            clients::ClientId, protocol_generated::WlSurfaceId, test_support::state_for_tests,
            wayland_objects::wl_surface::WlSurface,
        },
    };

    #[test]
    fn effective_window_geometry_keeps_explicit_geometry_without_tree_bounds() {
        let state = state_for_tests();
        let surface = WlSurface::new(WlSurfaceId::from(1u32), ClientId::from_raw(1), 6);
        let explicit = RectI::new(10, 20, 300, 200).unwrap();
        assert_eq!(
            effective_window_geometry_for_surface(&state, &surface, Some(explicit)),
            Some(explicit)
        );
    }
}
