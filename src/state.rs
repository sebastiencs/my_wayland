use crate::{
    clients::{ClientId, ProcessInfo, Resolved},
    config::{
        DecorationInsets, OutputScaleSetting, RoundingPower, RuntimeConfig, XdgDecorationModePolicy,
    },
    drm::Drm,
    egl::EGL,
    globals::{Global, GlobalName, Globals},
    libinput::Input,
    object::Object,
    protocol_generated::{
        HyprlandCtmControlManagerV1Id, WaylandId, WlDataOfferId, WlDataSourceId, WlSeatId,
        WlSurfaceId, XdgSurfaceId, XdgToplevelId, ZwlrGammaControlV1Id, ZwlrLayerSurfaceV1Id,
    },
    region::RectI,
    render::{
        BlurVisualStyle, BorderGradient, BorderVisualStyle, RootSurfaceVisualState,
        ShadowVisualStyle, SurfaceEffectStyle, WindowAnimationState, WindowAnimationValues,
    },
    root_geometry::{self, CommittedRootGeometry, DecorationFrameLayout},
    seat::SeatState,
    tiling::{self, TiledResizeState, WorkspaceLayout},
    wayland_objects::{
        wl_surface::WlSurface,
        xdg_surface::{
            xdg_toplevel::{ToplevelDecorationMode, XdgToplevel},
            XdgSurface,
        },
        zwlr_layer_surface_v1::{output_usable_bounds_with_snapshot, LayerLayoutObjectSnapshot},
    },
    xkb::{KeyboardState, Xkb},
};

use {
    rand::{rngs::OsRng, RngCore},
    std::{
        cell::Cell,
        collections::{BTreeMap, BTreeSet},
        rc::Rc,
    },
};

pub use crate::indexmap::{Grab, GrabbedObject, IndexMap, IndexMapKey};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RootSurfaceClip {
    pub outer_rect: RectI,
    pub corner_radius: i32,
    pub rounding_power: RoundingPower,
}

impl RootSurfaceClip {
    pub fn new(outer_rect: RectI, corner_radius: i32, rounding_power: RoundingPower) -> Self {
        Self {
            outer_rect,
            corner_radius: clamp_corner_radius_for_rect(outer_rect, corner_radius),
            rounding_power,
        }
    }

    pub fn contains_point(self, x: f64, y: f64) -> bool {
        let rect = self.outer_rect;
        let x1 = f64::from(rect.x);
        let y1 = f64::from(rect.y);
        let x2 = f64::from(rect.x2());
        let y2 = f64::from(rect.y2());
        if x < x1 || x >= x2 || y < y1 || y >= y2 {
            return false;
        }

        let radius = f64::from(self.corner_radius.max(0));
        if radius <= 0.0 {
            return true;
        }
        let rounding_power = f64::from(self.rounding_power.get());

        let left = x1 + radius;
        let top = y1 + radius;
        let right = x2 - radius;
        let bottom = y2 - radius;

        if x < left && y < top {
            let dx = left - x;
            let dy = top - y;
            return rounded_corner_distance(dx, dy, rounding_power) <= radius;
        }
        if x >= right && y < top {
            let dx = x - right;
            let dy = top - y;
            return rounded_corner_distance(dx, dy, rounding_power) <= radius;
        }
        if x < left && y >= bottom {
            let dx = left - x;
            let dy = y - bottom;
            return rounded_corner_distance(dx, dy, rounding_power) <= radius;
        }
        if x >= right && y >= bottom {
            let dx = x - right;
            let dy = y - bottom;
            return rounded_corner_distance(dx, dy, rounding_power) <= radius;
        }

        true
    }
}

fn inflate_rect_with_insets(rect: RectI, insets: DecorationInsets) -> Option<RectI> {
    RectI::from_extents(
        rect.x.saturating_sub(insets.left.max(0)),
        rect.y.saturating_sub(insets.top.max(0)),
        rect.x2().saturating_add(insets.right.max(0)),
        rect.y2().saturating_add(insets.bottom.max(0)),
    )
}

fn clamp_corner_radius_for_rect(rect: RectI, corner_radius: i32) -> i32 {
    let max_radius = rect.width.min(rect.height).max(0) / 2;
    corner_radius.max(0).min(max_radius)
}

fn rounded_corner_distance(dx: f64, dy: f64, rounding_power: f64) -> f64 {
    (dx.abs().powf(rounding_power) + dy.abs().powf(rounding_power)).powf(1.0 / rounding_power)
}

index_map_id!(ObjectId);
index_map_id!(InputId);
index_map_id!(DrmId);
index_map_id!(OutputId);
index_map_id!(WorkspaceGroupId);
index_map_id!(WorkspaceId);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BackendOutputRef {
    pub drm_id: DrmId,
}

impl BackendOutputRef {
    pub fn new(drm_id: DrmId) -> Self {
        Self { drm_id }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutputMode {
    pub width: i32,
    pub height: i32,
    pub refresh_millihz: i32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, PartialOrd, Ord)]
pub struct FractionalScale(u32);

impl FractionalScale {
    pub const DENOMINATOR: u32 = 120;
    pub const ONE: Self = Self(Self::DENOMINATOR);
    pub const ONE_POINT_FIVE: Self = Self(180);
    pub const TWO: Self = Self(240);

    pub const fn from_v120(raw: u32) -> Self {
        Self(if raw == 0 { 1 } else { raw })
    }

    pub const fn from_integer(scale: u32) -> Self {
        Self::from_v120(scale.saturating_mul(Self::DENOMINATOR))
    }

    pub fn try_from_f64(scale: f64) -> Option<Self> {
        if !scale.is_finite() || scale <= 0.0 {
            return None;
        }
        let raw = (scale * f64::from(Self::DENOMINATOR)).round();
        (raw >= 1.0 && raw <= f64::from(u32::MAX)).then_some(Self(raw as u32))
    }

    pub const fn as_v120(self) -> u32 {
        self.0
    }

    pub fn as_f64(self) -> f64 {
        f64::from(self.0) / f64::from(Self::DENOMINATOR)
    }

    pub fn ceil(self) -> i32 {
        self.0.div_ceil(Self::DENOMINATOR) as i32
    }

    pub fn logical_length(self, pixels: i32) -> i32 {
        if pixels <= 0 {
            return 0;
        }
        let pixels = i64::from(pixels);
        let scale = i64::from(self.0);
        ((pixels * i64::from(Self::DENOMINATOR)) + (scale / 2)).div_euclid(scale) as i32
    }
}

impl Default for FractionalScale {
    fn default() -> Self {
        Self::ONE
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ColorTransformMatrix {
    pub values: [f32; 9],
}

impl ColorTransformMatrix {
    pub const fn identity() -> Self {
        Self { values: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0] }
    }
}

#[derive(Clone, Debug)]
pub struct Output {
    pub backend: BackendOutputRef,
    pub drm_crtc_id: u32,
    pub global_name: Option<GlobalName>,
    pub gamma_lut_size: u32,
    pub ctm_supported: bool,
    pub gamma_control_object_id: Option<Resolved<ZwlrGammaControlV1Id>>,
    pub x: i32,
    pub y: i32,
    pub physical_width: i32,
    pub physical_height: i32,
    pub scale: FractionalScale,
    pub transform: i32,
    pub make: String,
    pub model: String,
    pub name: String,
    pub description: String,
    pub enabled: bool,
    pub current_mode: OutputMode,
    pub preferred_mode: Option<OutputMode>,
}

impl Output {
    pub fn new(backend: BackendOutputRef, drm_crtc_id: u32, drm: &Drm) -> Self {
        let mut output = Self {
            backend,
            drm_crtc_id,
            global_name: None,
            gamma_lut_size: 0,
            ctm_supported: false,
            gamma_control_object_id: None,
            x: 0,
            y: 0,
            physical_width: 0,
            physical_height: 0,
            scale: FractionalScale::ONE,
            transform: 0,
            make: String::new(),
            model: String::new(),
            name: String::new(),
            description: String::new(),
            enabled: false,
            current_mode: OutputMode { width: 0, height: 0, refresh_millihz: 0 },
            preferred_mode: None,
        };
        output.refresh_from_backend(drm);
        output
    }

    pub fn backend(&self) -> BackendOutputRef {
        self.backend
    }

    pub fn drm_crtc_id(&self) -> u32 {
        self.drm_crtc_id
    }

    pub fn refresh_from_backend(&mut self, drm: &Drm) {
        let Some(in_use) = drm.output_connector_in_use(self.drm_crtc_id) else {
            self.enabled = false;
            self.gamma_lut_size = 0;
            self.ctm_supported = false;
            return;
        };
        let connector = &in_use.connector;

        let preferred_mode = connector
            .preferred_mode
            .as_ref()
            .map(Self::mode_from_connector);
        let current_mode = drm
            .current_output_mode(self.drm_crtc_id)
            .or_else(|| preferred_mode.clone())
            .unwrap_or(OutputMode { width: 0, height: 0, refresh_millihz: 0 });

        let make = connector.manufacturer_id.clone();
        let model = connector.model_name.clone().unwrap_or_default();
        let name = connector.interface_name.clone();
        let description = match (make.is_empty(), model.is_empty()) {
            (false, false) => format!("{make} {model}"),
            (false, true) => make.clone(),
            (true, false) => model.clone(),
            (true, true) => name.clone(),
        };

        self.physical_width = connector.mm_width as i32;
        self.physical_height = connector.mm_height as i32;
        self.make = make;
        self.model = model;
        self.name = name;
        self.description = description;
        self.enabled = true;
        self.gamma_lut_size = drm.output_gamma_size(self.drm_crtc_id);
        self.ctm_supported = drm.output_supports_ctm(self.drm_crtc_id);
        self.current_mode = current_mode;
        self.preferred_mode = preferred_mode;
    }

    pub fn configured_scale(&self, config: &RuntimeConfig) -> FractionalScale {
        match config.output_scale(self.name.as_str()) {
            Some(OutputScaleSetting::Fixed(scale)) => scale,
            Some(OutputScaleSetting::Auto) => self.auto_scale(),
            None => FractionalScale::ONE,
        }
    }

    pub fn auto_scale(&self) -> FractionalScale {
        if !self.enabled || self.physical_width <= 0 || self.physical_height <= 0 {
            return FractionalScale::ONE;
        }

        let diagonal_px =
            f64::from(self.current_mode.width).hypot(f64::from(self.current_mode.height));
        if diagonal_px <= 0.0 {
            return FractionalScale::ONE;
        }

        let mm_per_inch = 25.4;
        let diagonal_in = (f64::from(self.physical_width) / mm_per_inch)
            .hypot(f64::from(self.physical_height) / mm_per_inch);
        if diagonal_in <= 0.0 {
            return FractionalScale::ONE;
        }

        let ppi = diagonal_px / diagonal_in;
        if ppi > 200.0 {
            FractionalScale::TWO
        } else if ppi > 140.0 {
            FractionalScale::ONE_POINT_FIVE
        } else {
            FractionalScale::ONE
        }
    }

    pub fn legacy_scale(&self) -> i32 {
        self.scale.ceil()
    }

    pub fn logical_size(&self) -> (i32, i32) {
        let (width, height) = match self.transform {
            1 | 3 | 5 | 7 => (self.current_mode.height, self.current_mode.width),
            _ => (self.current_mode.width, self.current_mode.height),
        };
        (self.scale.logical_length(width), self.scale.logical_length(height))
    }

    pub fn logical_bounds(&self) -> Option<RectI> {
        let (width, height) = self.logical_size();
        RectI::new(self.x, self.y, width, height)
    }

    fn mode_from_connector(mode: &drm::control::Mode) -> OutputMode {
        let (width, height) = mode.size();
        let refresh_millihz = {
            let clock_milli_hertz = mode.clock() as u64 * 1_000_000;
            let (_, _, htotal) = mode.hsync();
            let (_, _, vtotal) = mode.vsync();
            (((clock_milli_hertz / htotal as u64) + (vtotal as u64 / 2)) / vtotal as u64) as i32
        };

        OutputMode { width: width as i32, height: height as i32, refresh_millihz }
    }
}

#[derive(Clone, Debug, Default)]
pub struct WorkspaceGroup {
    pub outputs: BTreeSet<OutputId>,
    pub workspaces: Vec<WorkspaceId>,
    pub active_workspace: Option<WorkspaceId>,
}

#[derive(Debug)]
pub struct Workspace {
    pub stable_id: String,
    pub name: String,
    pub coordinates: Vec<u32>,
    pub group: Option<WorkspaceGroupId>,
    pub root_surfaces: BTreeSet<Resolved<WlSurfaceId>>,
    pub layout: WorkspaceLayout,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ManagedWindowMode {
    #[default]
    Tiled,
    Floating,
}

#[derive(Clone, Copy, Debug)]
pub struct ManagedRootSurfaceState {
    pub workspace_id: WorkspaceId,
    pub mode: ManagedWindowMode,
    pub fullscreen: bool,
    pub mapped: bool,
    pub awaiting_initial_layout: bool,
    pub remembered_floating_state: Option<RememberedFloatingState>,
    pub restore_floating_state_on_next_configure: bool,
    pub visual_animation: WindowAnimationState,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RememberedFloatingState {
    pub root_pos: (i32, i32),
    pub configure_size: (i32, i32),
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RootGeometryCacheEntry {
    pub geometry: Option<CommittedRootGeometry>,
    pub dirty: bool,
    pub revision: u64,
    pub built_from_revision: u64,
}

impl RootGeometryCacheEntry {
    pub(crate) fn is_fresh(self) -> bool {
        !self.dirty && self.built_from_revision == self.revision
    }

    fn mark_dirty(&mut self) {
        self.revision = self.revision.saturating_add(1);
        self.dirty = true;
    }

    fn store_geometry(&mut self, geometry: Option<CommittedRootGeometry>) {
        self.geometry = geometry;
        self.built_from_revision = self.revision;
        self.dirty = false;
    }
}

#[derive(Debug, Default)]
pub struct WindowAnimationAdvanceResult {
    pub damaged_rects: Vec<RectI>,
    pub has_active_animations: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OutputLayoutEntry {
    pub output_id: OutputId,
    pub backend: BackendOutputRef,
    pub bounds: RectI,
}

impl OutputLayoutEntry {
    pub fn local_bounds(&self) -> RectI {
        self.bounds.translate(-self.bounds.x, -self.bounds.y)
    }

    pub fn global_to_local_point(&self, x: i32, y: i32) -> (i32, i32) {
        (x.saturating_sub(self.bounds.x), y.saturating_sub(self.bounds.y))
    }

    pub fn global_to_local_rect(&self, rect: RectI) -> RectI {
        rect.translate(-self.bounds.x, -self.bounds.y)
    }

    pub fn overlap_area(&self, rect: RectI) -> i64 {
        let Some(overlap) = self.bounds.intersection(rect) else {
            return 0;
        };
        overlap.width as i64 * overlap.height as i64
    }
}

#[derive(Clone, Debug, Default)]
pub struct OutputLayout {
    entries: Vec<OutputLayoutEntry>,
    extents: Option<RectI>,
}

impl OutputLayout {
    pub fn from_outputs(outputs: &IndexMap<OutputId, Output>) -> Self {
        let entries = outputs
            .iter()
            .filter_map(|(output_id, output)| {
                if !output.enabled {
                    return None;
                }
                Some(OutputLayoutEntry {
                    output_id,
                    backend: output.backend(),
                    bounds: output.logical_bounds()?,
                })
            })
            .collect::<Vec<_>>();
        let extents = entries
            .iter()
            .map(|entry| entry.bounds)
            .reduce(|acc, bounds| {
                RectI::from_extents(
                    acc.x.min(bounds.x),
                    acc.y.min(bounds.y),
                    acc.x2().max(bounds.x2()),
                    acc.y2().max(bounds.y2()),
                )
                .expect("output layout extents must stay valid")
            });
        Self { entries, extents }
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn entries(&self) -> &[OutputLayoutEntry] {
        &self.entries
    }

    pub fn extents(&self) -> Option<RectI> {
        self.extents
    }

    pub fn entry(&self, output_id: OutputId) -> Option<&OutputLayoutEntry> {
        self.entries
            .iter()
            .find(|entry| entry.output_id == output_id)
    }

    pub fn bounds(&self, output_id: OutputId) -> Option<RectI> {
        self.entry(output_id).map(|entry| entry.bounds)
    }

    pub fn output_at(&self, x: i32, y: i32) -> Option<OutputId> {
        self.entries
            .iter()
            .find(|entry| entry.bounds.contains(x, y))
            .map(|entry| entry.output_id)
    }

    pub fn nearest_point(&self, x: i32, y: i32) -> Option<(OutputId, (i32, i32))> {
        let mut best = None;
        let mut best_distance = i64::MAX;
        for entry in &self.entries {
            let clamped_x = x.clamp(entry.bounds.x, entry.bounds.x2().saturating_sub(1));
            let clamped_y = y.clamp(entry.bounds.y, entry.bounds.y2().saturating_sub(1));
            let dx = i64::from(x.saturating_sub(clamped_x));
            let dy = i64::from(y.saturating_sub(clamped_y));
            let distance = dx * dx + dy * dy;
            if distance < best_distance {
                best_distance = distance;
                best = Some((entry.output_id, (clamped_x, clamped_y)));
            }
        }
        best
    }

    pub fn intersecting(&self, rect: RectI) -> Vec<OutputId> {
        self.entries
            .iter()
            .filter_map(|entry| entry.bounds.intersects(rect).then_some(entry.output_id))
            .collect()
    }

    pub fn primary_output_for_rect(
        &self,
        rect: RectI,
        preferred_output_id: Option<OutputId>,
    ) -> Option<OutputId> {
        let mut best = None;
        let mut best_area = -1i64;
        let mut best_is_preferred = false;
        for entry in &self.entries {
            let area = entry.overlap_area(rect);
            if area <= 0 {
                continue;
            }
            let is_preferred = Some(entry.output_id) == preferred_output_id;
            if area > best_area || (area == best_area && is_preferred && !best_is_preferred) {
                best = Some(entry.output_id);
                best_area = area;
                best_is_preferred = is_preferred;
            }
        }
        best.or(preferred_output_id.filter(|output_id| self.entry(*output_id).is_some()))
    }
}

const ACTIVATION_TOKEN_TTL_MSEC: u64 = 30_000;

#[derive(Clone, Debug)]
pub struct ActivationTokenData {
    pub client_id: ClientId,
    pub seat: Option<Resolved<WlSeatId>>,
    pub serial: Option<u32>,
    pub surface: Option<Resolved<WlSurfaceId>>,
    pub app_id: Option<String>,
    pub committed_at_msec: u64,
    pub expires_at_msec: u64,
}

#[derive(Debug, Default)]
pub struct ActivationTokenStore {
    tokens: BTreeMap<String, ActivationTokenData>,
}

impl ActivationTokenStore {
    fn monotonic_time_msec() -> u64 {
        let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
        if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) } != 0 {
            return 0;
        }
        (ts.tv_sec as u64)
            .saturating_mul(1000)
            .saturating_add((ts.tv_nsec as u64) / 1_000_000)
    }

    fn format_16_bytes_hex(bytes: &[u8; 16]) -> String {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut s = String::with_capacity(32);
        for byte in bytes {
            s.push(HEX[(byte >> 4) as usize] as char);
            s.push(HEX[(byte & 0xf) as usize] as char);
        }
        s
    }

    /// 128 bits from the OS RNG, encoded as 32 lowercase hex digits (same shape as wlroots).
    fn random_hex_token() -> String {
        let mut buf = [0u8; 16];
        OsRng.fill_bytes(&mut buf);
        Self::format_16_bytes_hex(&buf)
    }

    fn random_activation_token(&mut self) -> String {
        loop {
            let s = Self::random_hex_token();
            if !self.tokens.contains_key(&s) {
                return s;
            }
        }
    }

    pub fn issue_tracked(
        &mut self,
        client_id: ClientId,
        seat: Option<Resolved<WlSeatId>>,
        serial: Option<u32>,
        surface: Option<Resolved<WlSurfaceId>>,
        app_id: Option<String>,
    ) -> String {
        self.purge_expired();
        let committed_at_msec = Self::monotonic_time_msec();
        let expires_at_msec = committed_at_msec.saturating_add(ACTIVATION_TOKEN_TTL_MSEC);
        let token = self.random_activation_token();
        self.tokens.insert(
            token.clone(),
            ActivationTokenData {
                client_id,
                seat,
                serial,
                surface,
                app_id,
                committed_at_msec,
                expires_at_msec,
            },
        );
        token
    }

    pub fn issue_untracked(&mut self) -> String {
        self.purge_expired();
        Self::random_hex_token()
    }

    pub fn consume(&mut self, token: &str) -> Option<ActivationTokenData> {
        self.purge_expired();
        self.tokens.remove(token)
    }

    pub fn purge_expired(&mut self) {
        let now = Self::monotonic_time_msec();
        self.tokens.retain(|_, data| data.expires_at_msec > now);
    }

    pub fn remove_client(&mut self, client_id: ClientId) {
        self.tokens.retain(|_, data| data.client_id != client_id);
    }

    #[cfg(test)]
    pub fn expire_now(&mut self, token: &str) -> bool {
        let now = Self::monotonic_time_msec();
        let Some(data) = self.tokens.get_mut(token) else {
            return false;
        };
        data.expires_at_msec = now;
        true
    }

    #[cfg(test)]
    pub fn first_token(&self) -> Option<String> {
        self.tokens.keys().next().cloned()
    }

    #[cfg(test)]
    pub fn tracked_count(&self) -> usize {
        self.tokens.len()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DataOfferRole {
    Selection,
    Drag,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SelectionSourceState {
    pub source: Resolved<WlDataSourceId>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DragSource {
    None { initiator_client_id: ClientId },
    DataSource { source: Resolved<WlDataSourceId> },
}

impl DragSource {
    pub fn initiator_client_id(self) -> ClientId {
        match self {
            Self::None { initiator_client_id } => initiator_client_id,
            Self::DataSource { source, .. } => source.client_id(),
        }
    }

    pub fn resolved(self) -> Option<Resolved<WlDataSourceId>> {
        match self {
            Self::None { .. } => None,
            Self::DataSource { source } => Some(source),
        }
    }

    pub fn object_id(self) -> Option<ObjectId> {
        match self {
            Self::None { .. } => None,
            Self::DataSource { source } => Some(source.object_id()),
        }
    }

    pub fn is_none(&self) -> bool {
        matches!(self, Self::None { .. })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, derive_more::Deref)]
pub struct DragTargetState {
    pub surface: Resolved<WlSurfaceId>,
}

impl DragTargetState {
    pub fn surface_id(self) -> WlSurfaceId {
        self.surface.wayland_id()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DragPhase {
    Dragging,
    DropSent,
    Finished,
    Cancelled,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct DragNegotiationState {
    pub offer_ids: Vec<Resolved<WlDataOfferId>>,
    pub accepted_mime: Option<String>,
    pub destination_actions: u32,
    pub preferred_action: u32,
    pub compositor_action: u32,
    pub chosen_action: u32,
    pub finished: bool,
    pub has_finish_capable_offer: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DragState {
    pub source: DragSource,
    pub origin_surface: Resolved<WlSurfaceId>,
    pub icon_surface: Option<Resolved<WlSurfaceId>>,
    pub target: Option<DragTargetState>,
    pub negotiation: DragNegotiationState,
    pub phase: DragPhase,
}

pub struct State {
    pub objects: Box<IndexMap<ObjectId, Box<dyn Object>>>,
    pub root_surfaces: Vec<Resolved<WlSurfaceId>>,
    pub(crate) root_surface_process_info: BTreeMap<Resolved<WlSurfaceId>, Option<Rc<ProcessInfo>>>,
    pub layer_surface_objects: BTreeSet<Resolved<ZwlrLayerSurfaceV1Id>>,
    pub inputs: IndexMap<InputId, Input>,
    pub drm: IndexMap<DrmId, Drm>,
    pub outputs: IndexMap<OutputId, Output>,
    pub active_output: Option<OutputId>,
    pub workspace_groups: IndexMap<WorkspaceGroupId, WorkspaceGroup>,
    pub workspaces: IndexMap<WorkspaceId, Workspace>,
    pub output_workspace_groups: BTreeMap<OutputId, WorkspaceGroupId>,
    pub root_surface_workspaces: BTreeMap<Resolved<WlSurfaceId>, WorkspaceId>,
    pub managed_root_surfaces: BTreeMap<Resolved<WlSurfaceId>, ManagedRootSurfaceState>,
    pub root_geometry_cache: BTreeMap<Resolved<WlSurfaceId>, Cell<RootGeometryCacheEntry>>,
    pub(crate) client_process_info: BTreeMap<ClientId, Option<Rc<ProcessInfo>>>,
    pub xdg_toplevel_roles: BTreeMap<Resolved<XdgSurfaceId>, Resolved<XdgToplevelId>>,
    /// Unblocked `hyprland_ctm_control_manager_v1` singleton; new binds are blocked while set.
    pub ctm_control_owner: Option<Resolved<HyprlandCtmControlManagerV1Id>>,
    /// Offers keyed by their bound `wl_data_source` for teardown without scanning all objects.
    pub data_offers_by_source:
        BTreeMap<Resolved<WlDataSourceId>, BTreeSet<Resolved<WlDataOfferId>>>,
    pub next_workspace_serial: u32,
    pub backend_syncobj_globals: BTreeMap<DrmId, GlobalName>,
    pub globals: Globals,
    pub activation_tokens: ActivationTokenStore,
    pub egl: EGL,
    pub config: RuntimeConfig,
    pub seat: SeatState,
    pub keyboard: KeyboardState,
    pub xkb: Xkb,
}

impl State {
    fn next_workspace_stable_id(&mut self) -> String {
        let serial = self.next_workspace_serial;
        self.next_workspace_serial = self.next_workspace_serial.saturating_add(1);
        format!("workspace-{serial}")
    }

    fn primary_output_for_workspace_group(&self, group_id: WorkspaceGroupId) -> Option<OutputId> {
        self.workspace_groups
            .try_get(group_id)
            .and_then(|group| group.outputs.iter().copied().next())
    }

    fn default_workspace_name_for_group(&self, group_id: WorkspaceGroupId) -> String {
        self.workspace_groups
            .try_get(group_id)
            .map(|group| group.workspaces.len() + 1)
            .unwrap_or(1)
            .to_string()
    }

    fn refresh_workspace_coordinates_for_group(&mut self, group_id: WorkspaceGroupId) {
        let Some(workspace_ids) = self
            .workspace_groups
            .try_get(group_id)
            .map(|group| group.workspaces.clone())
        else {
            return;
        };
        for (index, workspace_id) in workspace_ids.into_iter().enumerate() {
            let Some(workspace) = self.workspaces.try_get_mut(workspace_id) else {
                continue;
            };
            workspace.coordinates = vec![(index + 1) as u32];
        }
    }

    fn clear_workspace_coordinates(&mut self, workspace_id: WorkspaceId) {
        let Some(workspace) = self.workspaces.try_get_mut(workspace_id) else {
            return;
        };
        workspace.coordinates.clear();
    }

    pub fn workspace_group_for_output(&self, output_id: OutputId) -> Option<WorkspaceGroupId> {
        self.output_workspace_groups.get(&output_id).copied()
    }

    pub fn active_workspace_for_group(&self, group_id: WorkspaceGroupId) -> Option<WorkspaceId> {
        self.workspace_groups
            .try_get(group_id)
            .and_then(|group| group.active_workspace)
    }

    pub fn active_workspace_for_output(&self, output_id: OutputId) -> Option<WorkspaceId> {
        self.workspace_group_for_output(output_id)
            .and_then(|group_id| self.active_workspace_for_group(group_id))
    }

    pub fn root_surface_workspace(&self, surface: Resolved<WlSurfaceId>) -> Option<WorkspaceId> {
        self.root_surface_workspaces
            .get(&surface)
            .copied()
            .or_else(|| {
                let parent_surface = self.popup_parent_surface(surface)?;
                let parent_root = self.root_surface_for_surface(parent_surface)?;
                self.root_surface_workspaces.get(&parent_root).copied()
            })
    }

    pub fn managed_root_surface_state(
        &self,
        surface: Resolved<WlSurfaceId>,
    ) -> Option<&ManagedRootSurfaceState> {
        self.managed_root_surfaces.get(&surface)
    }

    pub fn take_scheduled_floating_restore_state(
        &mut self,
        surface: Resolved<WlSurfaceId>,
    ) -> Option<RememberedFloatingState> {
        let managed = self.managed_root_surfaces.get_mut(&surface)?;
        std::mem::take(&mut managed.restore_floating_state_on_next_configure)
            .then_some(managed.remembered_floating_state)
            .flatten()
    }

    pub fn root_surface_is_mapped(&self, surface: Resolved<WlSurfaceId>) -> bool {
        self.managed_root_surfaces
            .get(&surface)
            .is_some_and(|managed| managed.mapped)
    }

    pub fn prepare_root_surface_initial_layout(&mut self, surface: Resolved<WlSurfaceId>) -> bool {
        let Some(workspace_id) = self
            .managed_root_surfaces
            .get(&surface)
            .map(|managed| managed.workspace_id)
            .or_else(|| self.ensure_root_surface_workspace(surface))
        else {
            return false;
        };
        let Some(managed) = self.managed_root_surfaces.get_mut(&surface) else {
            return false;
        };
        if managed.mode != ManagedWindowMode::Tiled || managed.mapped {
            return false;
        }
        if managed.awaiting_initial_layout {
            return true;
        }
        managed.awaiting_initial_layout = true;
        tiling::mark_workspace_dirty(self, workspace_id);
        true
    }

    pub fn confirm_root_surface_mapped(&mut self, surface: Resolved<WlSurfaceId>) {
        let Some(workspace_id) = self
            .managed_root_surfaces
            .get(&surface)
            .map(|managed| managed.workspace_id)
        else {
            return;
        };
        let Some(managed) = self.managed_root_surfaces.get_mut(&surface) else {
            return;
        };
        if !managed.awaiting_initial_layout {
            return;
        }
        managed.awaiting_initial_layout = false;
        tiling::mark_workspace_dirty(self, workspace_id);
    }

    pub fn register_xdg_toplevel_role(
        &mut self,
        xdg_surface: Resolved<XdgSurfaceId>,
        toplevel: Resolved<XdgToplevelId>,
    ) {
        self.xdg_toplevel_roles.insert(xdg_surface, toplevel);
    }

    pub fn unregister_xdg_toplevel_role(
        &mut self,
        xdg_surface: Resolved<XdgSurfaceId>,
        toplevel: Resolved<XdgToplevelId>,
    ) {
        if self.xdg_toplevel_roles.get(&xdg_surface) == Some(&toplevel) {
            self.xdg_toplevel_roles.remove(&xdg_surface);
        }
    }

    pub fn xdg_toplevel(
        &self,
        xdg_surface: Resolved<XdgSurfaceId>,
    ) -> Option<Resolved<XdgToplevelId>> {
        self.xdg_toplevel_roles
            .get(&xdg_surface)
            .copied()
            .filter(|toplevel| toplevel.exists(self))
    }

    #[cfg(test)]
    pub fn xdg_toplevel_object_id(&self, xdg_surface: Resolved<XdgSurfaceId>) -> Option<ObjectId> {
        self.xdg_toplevel(xdg_surface)
            .map(|toplevel| toplevel.object_id())
    }

    pub fn workspace_ids(&self) -> Vec<WorkspaceId> {
        self.workspaces
            .iter()
            .map(|(workspace_id, _)| workspace_id)
            .collect()
    }

    pub fn workspace_group_ids(&self) -> Vec<WorkspaceGroupId> {
        self.workspace_groups
            .iter()
            .map(|(group_id, _)| group_id)
            .collect()
    }

    pub fn create_workspace_group_for_output(&mut self, output_id: OutputId) -> WorkspaceGroupId {
        if let Some(group_id) = self.workspace_group_for_output(output_id) {
            return group_id;
        }
        let group_id = self.workspace_groups.insert(WorkspaceGroup {
            outputs: BTreeSet::from([output_id]),
            workspaces: Vec::new(),
            active_workspace: None,
        });
        self.output_workspace_groups.insert(output_id, group_id);
        let workspace_id = self.create_workspace(Some(group_id), None);
        if let Some(group) = self.workspace_groups.try_get_mut(group_id) {
            group.active_workspace = Some(workspace_id);
        }
        group_id
    }

    pub fn create_workspace(
        &mut self,
        group_id: Option<WorkspaceGroupId>,
        requested_name: Option<String>,
    ) -> WorkspaceId {
        let group_id =
            group_id.filter(|group_id| self.workspace_groups.try_get(*group_id).is_some());
        let name = requested_name
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| {
                group_id
                    .map(|group_id| self.default_workspace_name_for_group(group_id))
                    .unwrap_or_else(|| "workspace".to_string())
            });
        let stable_id = self.next_workspace_stable_id();
        let workspace_id = self.workspaces.insert(Workspace {
            stable_id,
            name,
            coordinates: Vec::new(),
            group: group_id,
            root_surfaces: BTreeSet::new(),
            layout: WorkspaceLayout::default(),
        });
        if let Some(group_id) = group_id {
            if let Some(group) = self.workspace_groups.try_get_mut(group_id) {
                group.workspaces.push(workspace_id);
                if group.active_workspace.is_none() {
                    group.active_workspace = Some(workspace_id);
                }
            }
            self.refresh_workspace_coordinates_for_group(group_id);
        }
        workspace_id
    }

    fn ensure_group_has_active_workspace(&mut self, group_id: WorkspaceGroupId) {
        let Some(group_snapshot) = self.workspace_groups.try_get(group_id).cloned() else {
            return;
        };
        let still_valid = group_snapshot
            .active_workspace
            .is_some_and(|workspace_id| group_snapshot.workspaces.contains(&workspace_id));
        if still_valid {
            return;
        }
        if let Some(next_active) = group_snapshot.workspaces.first().copied() {
            if let Some(group) = self.workspace_groups.try_get_mut(group_id) {
                group.active_workspace = Some(next_active);
            }
            return;
        }
        if group_snapshot.outputs.is_empty() {
            if let Some(group) = self.workspace_groups.try_get_mut(group_id) {
                group.active_workspace = None;
            }
            return;
        }
        let workspace_id = self.create_workspace(Some(group_id), None);
        if let Some(group) = self.workspace_groups.try_get_mut(group_id) {
            group.active_workspace = Some(workspace_id);
        }
    }

    pub fn set_workspace_name(
        &mut self,
        workspace_id: WorkspaceId,
        requested_name: impl Into<String>,
    ) -> bool {
        let Some(workspace) = self.workspaces.try_get_mut(workspace_id) else {
            return false;
        };
        let name = requested_name.into();
        if name.trim().is_empty() || workspace.name == name {
            return false;
        }
        workspace.name = name;
        true
    }

    pub fn assign_workspace_to_group(
        &mut self,
        workspace_id: WorkspaceId,
        new_group_id: Option<WorkspaceGroupId>,
    ) -> bool {
        let new_group_id = match new_group_id {
            Some(group_id) if self.workspace_groups.try_get(group_id).is_some() => Some(group_id),
            Some(_) => return false,
            None => None,
        };
        let Some(old_group_id) = self.workspaces.try_get(workspace_id).and_then(|w| w.group) else {
            if let Some(workspace) = self.workspaces.try_get_mut(workspace_id) {
                workspace.group = new_group_id;
            }
            if let Some(group_id) = new_group_id {
                if let Some(group) = self.workspace_groups.try_get_mut(group_id) {
                    if !group.workspaces.contains(&workspace_id) {
                        group.workspaces.push(workspace_id);
                    }
                    if group.active_workspace.is_none() {
                        group.active_workspace = Some(workspace_id);
                    }
                }
                self.refresh_workspace_coordinates_for_group(group_id);
            } else {
                self.clear_workspace_coordinates(workspace_id);
            }
            return true;
        };
        if Some(old_group_id) == new_group_id {
            return false;
        }
        if let Some(old_group) = self.workspace_groups.try_get_mut(old_group_id) {
            old_group
                .workspaces
                .retain(|candidate| *candidate != workspace_id);
            if old_group.active_workspace == Some(workspace_id) {
                old_group.active_workspace = old_group.workspaces.first().copied();
            }
        }
        if let Some(workspace) = self.workspaces.try_get_mut(workspace_id) {
            workspace.group = new_group_id;
        }
        if let Some(group_id) = new_group_id {
            if let Some(group) = self.workspace_groups.try_get_mut(group_id) {
                if !group.workspaces.contains(&workspace_id) {
                    group.workspaces.push(workspace_id);
                }
                if group.active_workspace.is_none() {
                    group.active_workspace = Some(workspace_id);
                }
            }
        }
        self.refresh_workspace_coordinates_for_group(old_group_id);
        self.ensure_group_has_active_workspace(old_group_id);
        if let Some(group_id) = new_group_id {
            self.refresh_workspace_coordinates_for_group(group_id);
        } else {
            self.clear_workspace_coordinates(workspace_id);
        }
        true
    }

    pub fn activate_workspace(&mut self, workspace_id: WorkspaceId) -> bool {
        let previous_workspace = self
            .workspaces
            .try_get(workspace_id)
            .and_then(|workspace| workspace.group)
            .and_then(|group_id| self.active_workspace_for_group(group_id));
        let Some(group_id) = self
            .workspaces
            .try_get(workspace_id)
            .and_then(|workspace| workspace.group)
        else {
            return false;
        };
        let Some(group) = self.workspace_groups.try_get_mut(group_id) else {
            return false;
        };
        if group.active_workspace == Some(workspace_id) {
            return false;
        }
        group.active_workspace = Some(workspace_id);
        if let Some(previous_workspace) = previous_workspace {
            tiling::mark_workspace_dirty(self, previous_workspace);
        }
        tiling::mark_workspace_dirty(self, workspace_id);
        true
    }

    pub fn assign_root_surface_to_workspace(
        &mut self,
        surface: Resolved<WlSurfaceId>,
        workspace_id: WorkspaceId,
    ) -> bool {
        if self.root_surface_workspaces.get(&surface).copied() == Some(workspace_id) {
            return false;
        }
        if let Some(previous_workspace_id) =
            self.root_surface_workspaces.insert(surface, workspace_id)
        {
            if let Some(previous_workspace) = self.workspaces.try_get_mut(previous_workspace_id) {
                previous_workspace.root_surfaces.remove(&surface);
                if previous_workspace
                    .layout
                    .fullscreen
                    .as_ref()
                    .is_some_and(|fullscreen| fullscreen.surface_object_id == surface)
                {
                    previous_workspace.layout.set_fullscreen(None);
                }
            }
            tiling::mark_workspace_dirty(self, previous_workspace_id);
        }
        let Some(workspace) = self.workspaces.try_get_mut(workspace_id) else {
            self.root_surface_workspaces.remove(&surface);
            return false;
        };
        workspace.root_surfaces.insert(surface);
        workspace.layout.mark_dirty();
        let initial_animation =
            WindowAnimationState::from_values(self.window_animation_target_values(surface));
        self.managed_root_surfaces
            .entry(surface)
            .and_modify(|managed| managed.workspace_id = workspace_id)
            .or_insert(ManagedRootSurfaceState {
                workspace_id,
                mode: ManagedWindowMode::Tiled,
                fullscreen: false,
                mapped: false,
                awaiting_initial_layout: true,
                remembered_floating_state: None,
                restore_floating_state_on_next_configure: false,
                visual_animation: initial_animation,
            });
        self.mark_root_geometry_dirty(surface);
        true
    }

    pub fn detach_root_surface_from_workspace(
        &mut self,
        surface: Resolved<WlSurfaceId>,
    ) -> Option<WorkspaceId> {
        let workspace_id = self.root_surface_workspaces.remove(&surface)?;
        if let Some(workspace) = self.workspaces.try_get_mut(workspace_id) {
            workspace.root_surfaces.remove(&surface);
            workspace.layout.remove_surface(surface);
            if workspace
                .layout
                .fullscreen
                .as_ref()
                .is_some_and(|fullscreen| fullscreen.surface_object_id == surface)
            {
                workspace.layout.set_fullscreen(None);
            }
        }
        self.managed_root_surfaces.remove(&surface);
        self.root_geometry_cache.remove(&surface);
        Some(workspace_id)
    }

    pub fn clear_root_surface_workspace_assignment(
        &mut self,
        surface: Resolved<WlSurfaceId>,
    ) -> Option<WorkspaceId> {
        let workspace_id = self.root_surface_workspaces.remove(&surface)?;
        if let Some(workspace) = self.workspaces.try_get_mut(workspace_id) {
            workspace.root_surfaces.remove(&surface);
            workspace.layout.remove_surface(surface);
            if workspace
                .layout
                .fullscreen
                .as_ref()
                .is_some_and(|fullscreen| fullscreen.surface_object_id == surface)
            {
                workspace.layout.set_fullscreen(None);
            }
        }
        self.managed_root_surfaces.remove(&surface);
        Some(workspace_id)
    }

    pub fn inherit_root_surface_workspace(
        &mut self,
        surface: Resolved<WlSurfaceId>,
        parent_surface: Resolved<WlSurfaceId>,
    ) -> Option<WorkspaceId> {
        let workspace_id = self.root_surface_workspace(parent_surface)?;
        self.assign_root_surface_to_workspace(surface, workspace_id);
        Some(workspace_id)
    }

    pub fn ensure_root_surface_workspace(
        &mut self,
        surface: Resolved<WlSurfaceId>,
    ) -> Option<WorkspaceId> {
        if let Some(workspace_id) = self.root_surface_workspace(surface) {
            return Some(workspace_id);
        }
        let workspace_id = self
            .active_output_id()
            .and_then(|output_id| self.active_workspace_for_output(output_id))
            .or_else(|| {
                self.workspace_groups
                    .iter()
                    .find_map(|(_, group)| group.active_workspace)
            })?;
        self.assign_root_surface_to_workspace(surface, workspace_id);
        Some(workspace_id)
    }

    pub fn workspace_output_id(&self, workspace_id: WorkspaceId) -> Option<OutputId> {
        self.workspaces
            .try_get(workspace_id)
            .and_then(|workspace| workspace.group)
            .and_then(|group_id| self.primary_output_for_workspace_group(group_id))
    }

    pub fn workspace_output_ids(&self, workspace_id: WorkspaceId) -> Vec<OutputId> {
        self.workspaces
            .try_get(workspace_id)
            .and_then(|workspace| workspace.group)
            .and_then(|group_id| self.workspace_groups.try_get(group_id))
            .map(|group| group.outputs.iter().copied().collect())
            .unwrap_or_default()
    }

    pub fn workspace_is_active(&self, workspace_id: WorkspaceId) -> bool {
        self.workspaces
            .try_get(workspace_id)
            .and_then(|workspace| workspace.group)
            .and_then(|group_id| self.workspace_groups.try_get(group_id))
            .is_some_and(|group| group.active_workspace == Some(workspace_id))
    }

    pub fn root_surface_is_visible(&self, surface: Resolved<WlSurfaceId>) -> bool {
        self.root_surface_workspace(surface)
            .map(|workspace_id| {
                if !self.workspace_is_active(workspace_id) {
                    return false;
                }
                let Some(workspace) = self.workspaces.try_get(workspace_id) else {
                    return false;
                };
                workspace
                    .layout
                    .fullscreen
                    .as_ref()
                    .filter(|fullscreen| self.root_surface_is_mapped(fullscreen.surface_object_id))
                    .map(|fullscreen| fullscreen.surface_object_id == surface)
                    .unwrap_or(true)
            })
            .unwrap_or(true)
    }

    pub fn relayout_dirty_workspaces(&mut self) {
        tiling::relayout_dirty_workspaces(self);
    }

    pub fn mark_all_workspaces_dirty(&mut self) {
        let workspace_ids = self
            .workspaces
            .iter()
            .map(|(workspace_id, _)| workspace_id)
            .collect::<Vec<_>>();
        for workspace_id in workspace_ids {
            tiling::mark_workspace_dirty(self, workspace_id);
        }
    }

    pub fn tiling_scene_root_surfaces(&self) -> Vec<Resolved<WlSurfaceId>> {
        tiling::managed_scene_order(self)
    }

    pub fn committed_root_geometry(
        &self,
        surface: Resolved<WlSurfaceId>,
    ) -> Option<CommittedRootGeometry> {
        if !self.root_surfaces.contains(&surface) {
            debug_assert!(
                !self.root_geometry_cache.contains_key(&surface),
                "non-root surface retained root geometry cache entry"
            );
            return None;
        }
        let Some(entry_cell) = self.root_geometry_cache.get(&surface) else {
            debug_assert!(false, "registered root surface missing geometry cache entry");
            return self.resolve_committed_root_geometry(surface);
        };
        let entry = entry_cell.get();
        if entry.is_fresh() {
            return entry.geometry;
        }
        let geometry = self.resolve_committed_root_geometry(surface);
        let mut refreshed = entry;
        refreshed.store_geometry(geometry);
        entry_cell.set(refreshed);
        geometry
    }

    fn committed_root_geometry_with_surface_ref(
        &self,
        surface: Resolved<WlSurfaceId>,
        surface_ref: &WlSurface,
    ) -> Option<CommittedRootGeometry> {
        if !self.root_surfaces.contains(&surface) {
            debug_assert!(
                !self.root_geometry_cache.contains_key(&surface),
                "non-root surface retained root geometry cache entry"
            );
            return None;
        }
        let Some(entry_cell) = self.root_geometry_cache.get(&surface) else {
            debug_assert!(false, "registered root surface missing geometry cache entry");
            return self.resolve_committed_root_geometry_with_surface(surface, surface_ref);
        };
        let entry = entry_cell.get();
        if entry.is_fresh() {
            return entry.geometry;
        }
        let geometry = self.resolve_committed_root_geometry_with_surface(surface, surface_ref);
        let mut refreshed = entry;
        refreshed.store_geometry(geometry);
        entry_cell.set(refreshed);
        geometry
    }

    pub fn configure_bounds_for_surface(
        &self,
        surface: Resolved<WlSurfaceId>,
        snapshot: Option<LayerLayoutObjectSnapshot<'_>>,
    ) -> Option<(i32, i32)> {
        let surface_ref = surface.try_ref(self)?;
        self.configure_bounds_for_surface_with_ref(surface, surface_ref, snapshot)
    }

    pub fn configure_bounds_for_surface_with_ref(
        &self,
        surface: Resolved<WlSurfaceId>,
        surface_ref: &WlSurface,
        snapshot: Option<LayerLayoutObjectSnapshot<'_>>,
    ) -> Option<(i32, i32)> {
        self.output_id_for_surface_object(surface, &surface_ref)
            .and_then(|output_id| output_usable_bounds_with_snapshot(self, output_id, snapshot))
            .map(|bounds| (bounds.width, bounds.height))
    }

    pub fn mark_root_geometry_dirty(&mut self, surface: Resolved<WlSurfaceId>) {
        if !self.root_surfaces.contains(&surface) {
            return;
        }
        let Some(entry) = self.root_geometry_cache.get_mut(&surface) else {
            debug_assert!(false, "registered root surface missing geometry cache entry");
            return;
        };
        entry.get_mut().mark_dirty();
    }

    pub fn mark_root_geometry_dirty_for_surface(&mut self, surface: Resolved<WlSurfaceId>) {
        let root_surface = if self.root_surfaces.contains(&surface) {
            Some(surface)
        } else {
            self.root_surface_for_surface(surface)
        };
        if let Some(root_surface) = root_surface {
            self.mark_root_geometry_dirty(root_surface);
        }
    }

    pub fn ensure_root_geometry_fresh(
        &mut self,
        surface: Resolved<WlSurfaceId>,
    ) -> Option<CommittedRootGeometry> {
        if !self.root_surfaces.contains(&surface) {
            self.root_geometry_cache.remove(&surface);
            return None;
        }
        self.committed_root_geometry(surface)
    }

    pub fn ensure_root_geometry_fresh_for_surface(
        &mut self,
        surface: Resolved<WlSurfaceId>,
    ) -> Option<CommittedRootGeometry> {
        let root_surface = if self.root_surfaces.contains(&surface) {
            Some(surface)
        } else {
            self.root_surface_for_surface(surface)
        }?;
        self.ensure_root_geometry_fresh(root_surface)
    }

    fn resolve_committed_root_geometry(
        &self,
        surface: Resolved<WlSurfaceId>,
    ) -> Option<CommittedRootGeometry> {
        let surface_ref = surface.try_ref(self)?;
        self.resolve_committed_root_geometry_with_surface(surface, &surface_ref)
    }

    fn resolve_committed_root_geometry_with_surface(
        &self,
        surface: Resolved<WlSurfaceId>,
        surface_ref: &WlSurface,
    ) -> Option<CommittedRootGeometry> {
        self.resolve_committed_root_geometry_with_surface_and_overrides(
            surface,
            surface_ref,
            root_geometry::GeometryBuildOverrides {
                decoration_mode: None,
                explicit_window_geometry: None,
                decoration_insets: None,
                root_pos: Some(surface_ref.root_position()),
            },
        )
    }

    fn resolve_committed_root_geometry_with_surface_and_overrides(
        &self,
        surface: Resolved<WlSurfaceId>,
        surface_ref: &WlSurface,
        overrides: root_geometry::GeometryBuildOverrides,
    ) -> Option<CommittedRootGeometry> {
        let layout_allocation = tiling::layout_allocation_for_surface(self, surface);
        root_geometry::build_committed_root_geometry(
            self,
            surface,
            surface_ref,
            layout_allocation,
            overrides,
        )
    }

    pub fn tiled_root_surface_geometry(
        &self,
        surface: Resolved<WlSurfaceId>,
    ) -> Option<CommittedRootGeometry> {
        self.committed_root_geometry(surface)
            .filter(|geometry| geometry.tiled_edges != 0 || geometry.fullscreen)
    }

    fn root_surface_xdg_toplevel(&self, object_id: ObjectId) -> Option<&XdgToplevel> {
        let surface = self.objects.try_ref_object::<WlSurface>(object_id)?;
        let xdg_surface = surface.xdg_surface()?;
        self.xdg_toplevel(xdg_surface)
            .map(|toplevel| toplevel.rref(self))
    }

    fn resolve_xdg_decoration_mode_policy(
        policy: XdgDecorationModePolicy,
        requested_mode: Option<ToplevelDecorationMode>,
    ) -> ToplevelDecorationMode {
        match policy {
            XdgDecorationModePolicy::Server => ToplevelDecorationMode::ServerSide,
            XdgDecorationModePolicy::Client => ToplevelDecorationMode::ClientSide,
            XdgDecorationModePolicy::HonorClient => {
                requested_mode.unwrap_or(ToplevelDecorationMode::ClientSide)
            }
        }
    }

    pub fn xdg_decoration_mode_for_surface(
        &self,
        root_surface: Option<Resolved<WlSurfaceId>>,
        has_decoration_object: bool,
        requested_mode: Option<ToplevelDecorationMode>,
    ) -> ToplevelDecorationMode {
        let policy = &self.config.decorations.xdg;
        if !has_decoration_object {
            return Self::resolve_xdg_decoration_mode_policy(policy.no_protocol, None);
        }

        let managed_state =
            root_surface.and_then(|surface| self.managed_root_surfaces.get(&surface));
        let selected_policy = if requested_mode.is_none() {
            policy.unset_mode
        } else if managed_state.is_some_and(|managed| managed.fullscreen)
            || root_surface.is_some_and(|surface| self.root_surface_is_fullscreen(surface))
        {
            policy.fullscreen
        } else if managed_state.is_some_and(|managed| managed.mode == ManagedWindowMode::Tiled) {
            policy.tiled
        } else if managed_state.is_some_and(|managed| managed.mode == ManagedWindowMode::Floating) {
            policy.floating
        } else {
            policy.floating
        };

        Self::resolve_xdg_decoration_mode_policy(selected_policy, requested_mode)
    }

    pub fn root_surface_content_rect_global(
        &self,
        surface: Resolved<WlSurfaceId>,
    ) -> Option<RectI> {
        self.committed_root_geometry(surface)
            .map(|geometry| geometry.content_rect_global())
    }

    pub fn root_surface_binding_rect(&self, surface: Resolved<WlSurfaceId>) -> Option<RectI> {
        let geometry = self.committed_root_geometry(surface)?;
        let logical_size = surface
            .try_ref(self)
            .and_then(|surface| surface.logical_size());
        Some(geometry.binding_rect(logical_size))
    }

    pub fn root_surface_decoration_insets(
        &self,
        surface: Resolved<WlSurfaceId>,
    ) -> Option<DecorationInsets> {
        let geometry = self.committed_root_geometry(surface)?;
        (geometry.decoration_mode == Some(ToplevelDecorationMode::ServerSide)
            && !geometry.fullscreen
            && (geometry.decoration_insets.top > 0
                || geometry.decoration_insets.right > 0
                || geometry.decoration_insets.bottom > 0
                || geometry.decoration_insets.left > 0))
            .then_some(geometry.decoration_insets)
    }

    pub fn compositor_decorations_enabled(&self, surface: Resolved<WlSurfaceId>) -> bool {
        self.root_surface_xdg_toplevel(surface.object_id()).is_some_and(|toplevel| {
            toplevel.decoration_mode()
                == crate::wayland_objects::xdg_surface::xdg_toplevel::ToplevelDecorationMode::ServerSide
                && !self.root_surface_is_fullscreen(surface)
        })
    }

    pub fn configured_decoration_insets(&self) -> DecorationInsets {
        let explicit = self.config.general.decoration_insets;
        let border_size = self.config.decorations.border.thickness.max(0);
        DecorationInsets {
            top: if explicit.top > 0 { explicit.top } else { border_size },
            right: if explicit.right > 0 { explicit.right } else { border_size },
            bottom: if explicit.bottom > 0 { explicit.bottom } else { border_size },
            left: if explicit.left > 0 { explicit.left } else { border_size },
        }
    }

    pub fn root_surface_decoration_layout(
        &self,
        surface: Resolved<WlSurfaceId>,
    ) -> Option<DecorationFrameLayout> {
        self.committed_root_geometry(surface)?.decoration_layout()
    }

    pub fn root_surface_clip(&self, surface: Resolved<WlSurfaceId>) -> Option<RootSurfaceClip> {
        let outer_rect = self
            .root_surface_decoration_layout(surface)
            .map(|layout| layout.frame_rect)?;
        let rounding = self.config.decorations.border.rounding.max(0);
        (rounding > 0).then(|| {
            RootSurfaceClip::new(
                outer_rect,
                rounding,
                self.config.decorations.border.rounding_power,
            )
        })
    }

    pub fn window_animation_target_values(
        &self,
        surface: Resolved<WlSurfaceId>,
    ) -> WindowAnimationValues {
        let focused = self.focused_root_surface() == Some(surface);
        let opacity_config = self.config.effects.opacity;
        WindowAnimationValues {
            content_alpha: if focused {
                opacity_config.active_alpha
            } else {
                opacity_config.inactive_alpha
            },
            dim_alpha: if focused { 0 } else { opacity_config.inactive_dim },
            shadow_alpha: if focused {
                opacity_config.active_alpha
            } else {
                opacity_config.inactive_alpha
            },
            border_alpha: if focused {
                opacity_config.active_alpha
            } else {
                opacity_config.inactive_alpha
            },
        }
    }

    pub fn root_surface_animation_damage_rect(
        &self,
        surface: Resolved<WlSurfaceId>,
    ) -> Option<RectI> {
        self.root_surface_visual_state(surface)
            .map(|visual| visual.influence_bounds())
            .or_else(|| self.root_surface_surface_bounds_global(surface))
            .or_else(|| self.root_surface_content_rect_global(surface))
    }

    pub fn advance_window_visual_animations(
        &mut self,
        frame_time_msec: u32,
    ) -> WindowAnimationAdvanceResult {
        let animation_duration = self.config.animations.window.duration_msec;
        let surfaces = self
            .managed_root_surfaces
            .keys()
            .copied()
            .collect::<Vec<_>>();
        let mut damaged_rects = Vec::new();
        let mut has_active_animations = false;
        for surface in surfaces {
            let target_values = self.window_animation_target_values(surface);
            let damage_rect = self.root_surface_animation_damage_rect(surface);
            let Some(managed) = self.managed_root_surfaces.get_mut(&surface) else {
                continue;
            };
            let previous_values = managed.visual_animation.current_values();
            let next_values = managed.visual_animation.advance_to(
                target_values,
                animation_duration,
                frame_time_msec,
            );
            if previous_values != next_values {
                if let Some(rect) = damage_rect {
                    damaged_rects.push(rect);
                }
            }
            has_active_animations |= managed.visual_animation.is_animating();
        }
        WindowAnimationAdvanceResult { damaged_rects, has_active_animations }
    }

    pub fn root_surface_visual_state(
        &self,
        surface: Resolved<WlSurfaceId>,
    ) -> Option<RootSurfaceVisualState> {
        let geometry = self.committed_root_geometry(surface)?;
        let compositor_layout = geometry.decoration_layout();
        let outer_rect = compositor_layout
            .map(|layout| layout.frame_rect)
            .or_else(|| geometry.surface_bounds_global())
            .unwrap_or_else(|| geometry.content_rect_global());
        let content_rect = compositor_layout
            .map(|layout| layout.client_window_rect)
            .unwrap_or_else(|| geometry.content_rect_global());
        let compositor_decorations_enabled = compositor_layout.is_some();
        let has_visible_decoration_ring =
            compositor_decorations_enabled && outer_rect != content_rect;
        let clip = compositor_decorations_enabled
            .then(|| self.root_surface_clip(surface))
            .flatten();
        let focused = self.focused_root_surface() == Some(surface);
        let border_config = self.config.decorations.border;
        let shadow_config = self.config.decorations.shadow;
        let animated_values = self
            .managed_root_surfaces
            .get(&surface)
            .map(|managed| managed.visual_animation.current_values())
            .unwrap_or_else(|| self.window_animation_target_values(surface));
        let gradient = if focused {
            BorderGradient {
                start_color: border_config.active_start_color,
                end_color: border_config.active_end_color,
                angle_degrees: border_config.gradient_angle_degrees,
            }
        } else {
            BorderGradient {
                start_color: border_config.inactive_start_color,
                end_color: border_config.inactive_end_color,
                angle_degrees: border_config.gradient_angle_degrees,
            }
        };
        let border = (compositor_decorations_enabled
            && border_config.thickness > 0
            && has_visible_decoration_ring)
            .then_some(BorderVisualStyle { gradient });
        let shadow = (compositor_decorations_enabled && shadow_config.radius > 0).then_some(
            ShadowVisualStyle {
                radius: shadow_config.radius,
                offset_x: shadow_config.offset_x,
                offset_y: shadow_config.offset_y,
                color: shadow_config.color,
                power: shadow_config.power,
            },
        );
        let visual_state = RootSurfaceVisualState {
            outer_rect,
            content_rect,
            clip,
            border,
            border_alpha: animated_values.border_alpha,
            shadow,
            shadow_alpha: animated_values.shadow_alpha,
            effects: SurfaceEffectStyle {
                blur: (self.config.effects.blur.enabled && has_visible_decoration_ring).then_some(
                    BlurVisualStyle {
                        radius: self.config.effects.blur.radius,
                        passes: self.config.effects.blur.passes,
                    },
                ),
                opacity_alpha: animated_values.content_alpha,
                dim_alpha: animated_values.dim_alpha,
            },
        };
        Some(visual_state)
    }

    pub fn root_surface_surface_bounds_global(
        &self,
        surface: Resolved<WlSurfaceId>,
    ) -> Option<RectI> {
        self.committed_root_geometry(surface)
            .and_then(|geometry| geometry.surface_bounds_global())
    }

    pub fn root_surface_for_surface(
        &self,
        mut surface: Resolved<WlSurfaceId>,
    ) -> Option<Resolved<WlSurfaceId>> {
        loop {
            if let Some(parent_surface) = self.popup_parent_surface(surface) {
                surface = parent_surface;
                continue;
            }
            if self.root_surfaces.contains(&surface) {
                return Some(surface);
            }
            let wl_surface = surface.try_ref(self)?;
            if wl_surface.is_root() {
                return Some(surface);
            }
            surface = *wl_surface.parent_surface()?;
        }
    }

    pub fn popup_parent_surface(
        &self,
        surface: Resolved<WlSurfaceId>,
    ) -> Option<Resolved<WlSurfaceId>> {
        let wl_surface = surface.try_ref(self)?;
        self.popup_parent_surface_with_ref(surface, wl_surface)
    }

    pub fn popup_parent_surface_with_ref(
        &self,
        _surface: Resolved<WlSurfaceId>,
        wl_surface: &WlSurface,
    ) -> Option<Resolved<WlSurfaceId>> {
        let xdg_surface = wl_surface.xdg_surface()?.try_ref(self)?;
        let popup = xdg_surface.popup_role()?.try_ref(self)?;
        popup
            .parent_surface()
            .and_then(|parent_xdg| parent_xdg.try_ref(self))
            .map(XdgSurface::wl_surface)
            .or_else(|| {
                popup
                    .parent_layer_surface()
                    .and_then(|parent_layer| parent_layer.try_ref(self))
                    .map(|layer| layer.surface())
            })
    }

    pub fn is_popup_root_surface(&self, surface: Resolved<WlSurfaceId>) -> bool {
        self.popup_parent_surface(surface).is_some()
    }

    pub fn popup_child_surfaces(
        &self,
        parent_surface: Resolved<WlSurfaceId>,
    ) -> Vec<Resolved<WlSurfaceId>> {
        let Some(parent_surface_ref) = parent_surface.try_ref(self) else {
            return Vec::new();
        };
        let popup_ids = parent_surface_ref
            .xdg_surface()
            .and_then(|xdg_surface| xdg_surface.try_ref(self))
            .map(|xdg_surface| xdg_surface.popup_children().to_vec())
            .or_else(|| {
                parent_surface_ref
                    .layer_surface()
                    .and_then(|layer_surface| layer_surface.try_ref(self))
                    .map(|layer_surface| layer_surface.popup_children().to_vec())
            })
            .unwrap_or_default();
        popup_ids
            .into_iter()
            .filter_map(|popup_id| {
                let popup = popup_id.try_ref(self)?;
                let xdg_surface = popup.xdg_surface().try_ref(self)?;
                Some(xdg_surface.wl_surface())
            })
            .collect()
    }

    pub fn surface_scene_origin(&self, surface: Resolved<WlSurfaceId>) -> Option<(i32, i32)> {
        let wl_surface = surface.try_ref(self)?;
        self.surface_scene_origin_with_ref(surface, wl_surface)
    }

    pub fn surface_scene_origin_with_ref(
        &self,
        surface: Resolved<WlSurfaceId>,
        wl_surface: &WlSurface,
    ) -> Option<(i32, i32)> {
        if let Some(parent_surface) = self.popup_parent_surface_with_ref(surface, wl_surface) {
            let parent_origin = self.surface_scene_origin(parent_surface)?;
            let xdg_surface_id = wl_surface.xdg_surface()?;
            let xdg_surface = xdg_surface_id.try_ref(self)?;
            let popup_id = xdg_surface.popup_role()?;
            let popup = popup_id.try_ref(self)?;
            let local_window_geometry = xdg_surface.window_geometry_rect_for_surface(surface, self);
            let geometry = popup.current_geometry();
            return Some((
                parent_origin.0 + geometry.x - local_window_geometry.x,
                parent_origin.1 + geometry.y - local_window_geometry.y,
            ));
        }
        Some(wl_surface.root_position())
    }

    pub fn surface_scene_bounds(&self, surface: Resolved<WlSurfaceId>) -> Option<RectI> {
        let wl_surface = surface.try_ref(self)?;
        self.surface_scene_bounds_with_ref(surface, wl_surface)
    }

    pub fn surface_scene_bounds_with_ref(
        &self,
        surface: Resolved<WlSurfaceId>,
        wl_surface: &WlSurface,
    ) -> Option<RectI> {
        let origin = self.surface_scene_origin_with_ref(surface, wl_surface)?;
        WlSurface::local_surface_tree_bounds_from_ref(self, wl_surface)
            .map(|bounds| bounds.translate(origin.0, origin.1))
    }

    pub fn focused_root_surface(&self) -> Option<Resolved<WlSurfaceId>> {
        let focus = self.seat.keyboard_focus()?;
        focus.exists(self).then_some(())?;
        self.root_surface_for_surface(focus.surface)
    }

    pub fn root_surface_is_fullscreen(&self, surface: Resolved<WlSurfaceId>) -> bool {
        let Some(workspace_id) = self.root_surface_workspace(surface) else {
            return false;
        };
        let Some(workspace) = self.workspaces.try_get(workspace_id) else {
            return false;
        };
        workspace
            .layout
            .fullscreen
            .as_ref()
            .is_some_and(|fullscreen| fullscreen.surface_object_id == surface)
    }

    pub fn is_tiled_root_surface(&self, surface: Resolved<WlSurfaceId>) -> bool {
        tiling::is_tiled_surface(self, surface)
    }

    pub fn is_floating_root_surface(&self, surface: Resolved<WlSurfaceId>) -> bool {
        tiling::is_floating_surface(self, surface)
    }

    fn current_root_surface_remembered_floating_state(
        &self,
        surface: Resolved<WlSurfaceId>,
    ) -> Option<RememberedFloatingState> {
        let geometry = self.committed_root_geometry(surface)?;
        Some(RememberedFloatingState {
            root_pos: geometry.root_pos,
            configure_size: crate::root_geometry::project_configure_geometry(
                geometry,
                geometry
                    .decoration_mode
                    .unwrap_or(ToplevelDecorationMode::ClientSide),
                self.configured_decoration_insets(),
            )
            .size,
        })
    }

    pub fn set_root_surface_floating(
        &mut self,
        surface: Resolved<WlSurfaceId>,
        floating: bool,
    ) -> bool {
        let Some(workspace_id) = self
            .managed_root_surfaces
            .get(&surface)
            .map(|managed| managed.workspace_id)
            .or_else(|| self.root_surface_workspace(surface))
        else {
            return false;
        };
        let current_state = self.current_root_surface_remembered_floating_state(surface);
        let Some(managed) = self.managed_root_surfaces.get_mut(&surface) else {
            return false;
        };
        let was_floating = managed.mode == ManagedWindowMode::Floating;
        let new_mode =
            if floating { ManagedWindowMode::Floating } else { ManagedWindowMode::Tiled };
        let mut changed = managed.mode != new_mode;
        if !floating && was_floating {
            if let Some(remembered_state) = current_state {
                managed.remembered_floating_state = Some(remembered_state);
            }
        }
        managed.mode = new_mode;
        managed.awaiting_initial_layout = !floating && !managed.mapped;
        let restore_state = floating
            .then_some(managed.remembered_floating_state)
            .flatten();
        managed.restore_floating_state_on_next_configure = floating && restore_state.is_some();
        if floating {
            managed.fullscreen = false;
            if let Some(workspace) = self.workspaces.try_get_mut(workspace_id) {
                if workspace
                    .layout
                    .fullscreen
                    .as_ref()
                    .is_some_and(|fullscreen| fullscreen.surface_object_id == surface)
                {
                    changed = true;
                    workspace.layout.set_fullscreen(None);
                }
            }
        }
        if changed {
            tiling::mark_workspace_dirty(self, workspace_id);
            self.relayout_dirty_workspaces();
        }
        if floating {
            if let Some(remembered_state) = restore_state {
                self.restore_root_surface_floating_state(surface, remembered_state);
            }
        }
        changed
    }

    pub fn set_root_surface_fullscreen(
        &mut self,
        surface: Resolved<WlSurfaceId>,
        fullscreen: bool,
    ) -> bool {
        let Some(workspace_id) = self
            .managed_root_surfaces
            .get(&surface)
            .map(|managed| managed.workspace_id)
            .or_else(|| self.root_surface_workspace(surface))
        else {
            return false;
        };
        let object_id = surface.object_id();
        let current_state = self.current_root_surface_remembered_floating_state(surface);
        let mut restore_state = None;
        if fullscreen {
            let managed_ids = self
                .managed_root_surfaces
                .iter()
                .filter_map(|(candidate_surface, managed)| {
                    (managed.workspace_id == workspace_id && managed.fullscreen)
                        .then_some(*candidate_surface)
                })
                .collect::<Vec<_>>();
            for candidate_surface in managed_ids {
                if let Some(managed) = self.managed_root_surfaces.get_mut(&candidate_surface) {
                    if candidate_surface.object_id() != object_id {
                        if managed.mode == ManagedWindowMode::Floating {
                            managed.restore_floating_state_on_next_configure =
                                managed.remembered_floating_state.is_some();
                        }
                        managed.fullscreen = false;
                    }
                }
                if candidate_surface.object_id() != object_id {
                    self.sync_root_surface_requested_fullscreen(candidate_surface, false);
                }
            }
            let initial_animation =
                WindowAnimationState::from_values(self.window_animation_target_values(surface));
            self.managed_root_surfaces
                .entry(surface)
                .and_modify(|managed| {
                    if !managed.fullscreen && managed.mode == ManagedWindowMode::Floating {
                        if let Some(remembered_state) = current_state {
                            managed.remembered_floating_state = Some(remembered_state);
                        }
                    }
                    managed.restore_floating_state_on_next_configure = false;
                    managed.fullscreen = true;
                })
                .or_insert(ManagedRootSurfaceState {
                    workspace_id,
                    mode: ManagedWindowMode::Tiled,
                    fullscreen: true,
                    mapped: false,
                    awaiting_initial_layout: true,
                    remembered_floating_state: None,
                    restore_floating_state_on_next_configure: false,
                    visual_animation: initial_animation,
                });
        } else {
            {
                let Some(managed) = self.managed_root_surfaces.get_mut(&surface) else {
                    return false;
                };
                if !managed.fullscreen {
                    return false;
                }
                managed.fullscreen = false;
                if managed.mode == ManagedWindowMode::Floating {
                    restore_state = managed.remembered_floating_state;
                    managed.restore_floating_state_on_next_configure = restore_state.is_some();
                } else {
                    managed.restore_floating_state_on_next_configure = false;
                }
            }
            self.sync_root_surface_requested_fullscreen(surface, false);
        }
        tiling::mark_workspace_dirty(self, workspace_id);
        self.relayout_dirty_workspaces();
        if let Some(remembered_state) = restore_state {
            self.restore_root_surface_floating_state(surface, remembered_state);
        }
        true
    }

    pub fn resize_tiled_root_surface(
        &mut self,
        surface: Resolved<WlSurfaceId>,
        edges: u32,
        dx: i32,
        dy: i32,
    ) -> bool {
        let changed = tiling::resize_tiled_surface(self, surface, edges, dx, dy);
        if changed {
            self.relayout_dirty_workspaces();
        }
        changed
    }

    pub fn begin_tiled_root_surface_resize(
        &mut self,
        surface: Resolved<WlSurfaceId>,
        edges: u32,
    ) -> Option<TiledResizeState> {
        self.relayout_dirty_workspaces();
        let workspace_id = self.root_surface_workspace(surface)?;
        let workspace = self.workspaces.try_get(workspace_id)?;
        workspace.layout.begin_tiled_resize(surface, edges)
    }

    pub fn resize_tiled_root_surface_from_state(
        &mut self,
        surface: Resolved<WlSurfaceId>,
        resize_state: TiledResizeState,
        dx: i32,
        dy: i32,
    ) -> bool {
        let Some(workspace_id) = self.root_surface_workspace(surface) else {
            return false;
        };
        let Some(workspace) = self.workspaces.try_get_mut(workspace_id) else {
            return false;
        };
        let changed =
            workspace
                .layout
                .resize_tiled_surface_from_state(surface, resize_state, dx, dy);
        if changed {
            self.relayout_dirty_workspaces();
        }
        changed
    }

    pub fn move_workspace_to_group_output(
        &mut self,
        workspace_id: WorkspaceId,
        new_group_id: WorkspaceGroupId,
    ) {
        let old_output = self.workspace_output_id(workspace_id);
        if !self.assign_workspace_to_group(workspace_id, Some(new_group_id)) {
            return;
        }
        let Some(new_output) = self.workspace_output_id(workspace_id) else {
            return;
        };
        let Some(old_output) = old_output else {
            return;
        };
        if old_output == new_output {
            return;
        }
        let Some(old_bounds) = self.output_bounds(old_output) else {
            return;
        };
        let Some(new_bounds) = self.output_bounds(new_output) else {
            return;
        };
        let dx = new_bounds.x - old_bounds.x;
        let dy = new_bounds.y - old_bounds.y;
        if dx == 0 && dy == 0 {
            return;
        }
        for managed in self.managed_root_surfaces.values_mut() {
            if managed.workspace_id != workspace_id {
                continue;
            }
            managed.remembered_floating_state =
                managed
                    .remembered_floating_state
                    .map(|remembered_state| RememberedFloatingState {
                        root_pos: (
                            remembered_state.root_pos.0 + dx,
                            remembered_state.root_pos.1 + dy,
                        ),
                        ..remembered_state
                    });
        }
        let Some(surface_ids) = self.workspaces.try_get(workspace_id).map(|workspace| {
            workspace
                .root_surfaces
                .iter()
                .map(|surface| surface.object_id())
                .collect::<Vec<_>>()
        }) else {
            return;
        };
        for surface_object_id in surface_ids {
            let Some(surface) = self
                .root_surfaces
                .iter()
                .copied()
                .find(|surface| surface.object_id() == surface_object_id)
            else {
                continue;
            };
            if self.is_tiled_root_surface(surface) {
                continue;
            }
            let Some(mut wl_surface) = surface.try_grab(self) else {
                continue;
            };
            let (x, y) = wl_surface.root_position();
            wl_surface.set_root_position(x + dx, y + dy);
            drop(wl_surface);
            self.mark_root_geometry_dirty(surface);
        }
        tiling::mark_workspace_dirty(self, workspace_id);
        self.relayout_dirty_workspaces();
    }

    fn restore_root_surface_floating_state(
        &mut self,
        surface: Resolved<WlSurfaceId>,
        remembered_state: RememberedFloatingState,
    ) -> bool {
        let Some(mut wl_surface) = surface.try_grab(self) else {
            return false;
        };
        wl_surface.set_root_position(remembered_state.root_pos.0, remembered_state.root_pos.1);
        drop(wl_surface);
        self.mark_root_geometry_dirty(surface);
        true
    }

    fn sync_root_surface_requested_fullscreen(
        &mut self,
        surface: Resolved<WlSurfaceId>,
        fullscreen: bool,
    ) {
        let Some(xdg_surface) = self
            .objects
            .try_ref_object::<WlSurface>(surface.object_id())
            .and_then(|surface| surface.xdg_surface())
        else {
            return;
        };
        let Some(toplevel) = self.xdg_toplevel(xdg_surface) else {
            return;
        };
        let mut toplevel = toplevel.grab(self);
        toplevel.set_requested_fullscreen(fullscreen);
    }

    pub fn can_remove_workspace(&self, workspace_id: WorkspaceId) -> bool {
        let Some(workspace) = self.workspaces.try_get(workspace_id) else {
            return false;
        };
        if !workspace.root_surfaces.is_empty() {
            return false;
        }
        let Some(group_id) = workspace.group else {
            return true;
        };
        let Some(group) = self.workspace_groups.try_get(group_id) else {
            return true;
        };
        group.workspaces.len() > 1 && group.active_workspace != Some(workspace_id)
    }

    pub fn remove_workspace(&mut self, workspace_id: WorkspaceId) -> bool {
        if !self.can_remove_workspace(workspace_id) {
            return false;
        }
        let old_group_id = self
            .workspaces
            .try_get(workspace_id)
            .and_then(|workspace| workspace.group);
        if let Some(group_id) = old_group_id {
            if let Some(group) = self.workspace_groups.try_get_mut(group_id) {
                group
                    .workspaces
                    .retain(|candidate| *candidate != workspace_id);
                if group.active_workspace == Some(workspace_id) {
                    group.active_workspace = group.workspaces.first().copied();
                }
            }
            self.refresh_workspace_coordinates_for_group(group_id);
            self.ensure_group_has_active_workspace(group_id);
        }
        self.workspaces.remove(workspace_id).is_some()
    }

    pub fn insert_object<O: Object + 'static>(&mut self, object: O) -> (WaylandId, ObjectId) {
        let wayland_id = object.id();
        let object_id = self.objects.insert(Box::new(object));
        (wayland_id, object_id)
    }
    /// Removes the object now, or defers if the slot is currently grabbed.
    /// Returns `None` if deferred or if the slot was already empty.
    pub fn remove_object(&mut self, object_id: ObjectId) -> Option<Box<dyn Object>> {
        if let Some(surface) = self
            .root_surfaces
            .iter()
            .copied()
            .find(|surface| surface.object_id() == object_id)
        {
            self.unregister_root_surface(surface);
        }
        self.objects.remove_or_defer(object_id)
    }

    pub fn register_root_surface(&mut self, object_id: ObjectId) {
        let Some(surface) = self
            .objects
            .try_ref_object::<WlSurface>(object_id)
            .map(|surface| Resolved::new(surface.client_id(), surface.surface_id(), object_id))
        else {
            return;
        };
        self.unregister_root_surface(surface);
        self.root_surfaces.push(surface);
        self.root_surface_process_info.insert(
            surface,
            self.client_process_info
                .get(&surface.client_id())
                .cloned()
                .flatten(),
        );
        self.root_geometry_cache
            .entry(surface)
            .or_insert_with(|| Cell::new(RootGeometryCacheEntry::default()));
        if let Some(workspace_id) = self.ensure_root_surface_workspace(surface) {
            tiling::mark_workspace_dirty(self, workspace_id);
        }
        self.mark_root_geometry_dirty(surface);
    }

    pub fn sanitize_ctm_control_owner(&mut self) {
        if let Some(owner) = self.ctm_control_owner {
            if !owner.exists(self) {
                self.ctm_control_owner = None;
            }
        }
    }

    pub fn clear_ctm_control_owner_if_matches(
        &mut self,
        candidate: Resolved<HyprlandCtmControlManagerV1Id>,
    ) {
        if self.ctm_control_owner == Some(candidate) {
            self.ctm_control_owner = None;
        }
    }

    pub fn register_data_offer_for_source(
        &mut self,
        source: Resolved<WlDataSourceId>,
        offer: Resolved<WlDataOfferId>,
    ) {
        self.data_offers_by_source
            .entry(source)
            .or_default()
            .insert(offer);
    }

    pub fn unregister_data_offer_for_source(
        &mut self,
        source: Resolved<WlDataSourceId>,
        offer: Resolved<WlDataOfferId>,
    ) {
        if let Some(set) = self.data_offers_by_source.get_mut(&source) {
            set.remove(&offer);
            if set.is_empty() {
                self.data_offers_by_source.remove(&source);
            }
        }
    }

    pub fn take_data_offers_for_source(
        &mut self,
        source: Resolved<WlDataSourceId>,
    ) -> Vec<Resolved<WlDataOfferId>> {
        self.data_offers_by_source
            .remove(&source)
            .map(|offers| offers.into_iter().collect())
            .unwrap_or_default()
    }

    pub fn register_layer_surface_object(&mut self, layer_surface: Resolved<ZwlrLayerSurfaceV1Id>) {
        self.layer_surface_objects.insert(layer_surface);
    }

    pub fn unregister_layer_surface_object(
        &mut self,
        layer_surface: Resolved<ZwlrLayerSurfaceV1Id>,
    ) {
        self.layer_surface_objects.remove(&layer_surface);
    }

    pub fn layer_surface(
        &self,
        layer_surface_id: Resolved<ZwlrLayerSurfaceV1Id>,
    ) -> Option<Resolved<ZwlrLayerSurfaceV1Id>> {
        self.layer_surface_objects
            .get(&layer_surface_id)
            .copied()
            .filter(|layer_surface| layer_surface.exists(self))
    }

    pub fn unregister_root_surface(&mut self, surface: Resolved<WlSurfaceId>) {
        let Some(found) = self
            .root_surfaces
            .iter()
            .copied()
            .find(|candidate| *candidate == surface)
        else {
            return;
        };
        self.root_surfaces.retain(|candidate| *candidate != found);
        self.root_surface_process_info.remove(&found);
        if let Some(workspace_id) = self.detach_root_surface_from_workspace(found) {
            tiling::mark_workspace_dirty(self, workspace_id);
        }
        self.root_geometry_cache.remove(&found);
    }

    pub(crate) fn set_client_process_info(
        &mut self,
        client_id: ClientId,
        info: Option<Rc<ProcessInfo>>,
    ) {
        self.client_process_info.insert(client_id, info.clone());
        for surface in self
            .root_surfaces
            .iter()
            .copied()
            .filter(|surface| surface.client_id() == client_id)
        {
            self.root_surface_process_info.insert(surface, info.clone());
        }
    }

    pub fn clear_client_process_info(&mut self, client_id: ClientId) {
        self.client_process_info.remove(&client_id);
        for info in self
            .root_surface_process_info
            .iter_mut()
            .filter(|(surface, _)| surface.client_id() == client_id)
            .map(|(_, info)| info)
        {
            *info = None;
        }
    }

    pub fn root_surface_command(&self, surface: Resolved<WlSurfaceId>) -> Option<&str> {
        self.root_surface_process_info
            .get(&surface)
            .and_then(|info| info.as_ref())
            .map(|info| info.command())
    }

    pub fn raise_root_surface(&mut self, surface: Resolved<WlSurfaceId>) -> bool {
        if !surface.exists(self) {
            return false;
        }
        if tiling::raise_surface(self, surface) {
            return true;
        }
        self.register_root_surface(surface.object_id());
        true
    }

    pub fn root_surfaces(&self) -> Vec<Resolved<WlSurfaceId>> {
        self.root_surfaces
            .iter()
            .filter(|surface| surface.exists(self))
            .copied()
            .collect()
    }

    pub fn insert_global(&mut self, global: Box<dyn Global>) -> GlobalName {
        self.globals.insert(global)
    }

    pub fn remove_global(&mut self, name: GlobalName) -> Option<Box<dyn Global>> {
        self.globals.remove(name)
    }

    pub fn insert_output(&mut self, output: Output) -> OutputId {
        let mut output = output;
        output.scale = output.configured_scale(&self.config);
        let output_id = self.outputs.insert(output);
        self.active_output.get_or_insert(output_id);
        self.create_workspace_group_for_output(output_id);
        output_id
    }

    pub fn remove_output(&mut self, output_id: OutputId) -> Option<Output> {
        let removed = self.outputs.remove(output_id)?;
        if let Some(group_id) = self.output_workspace_groups.remove(&output_id) {
            let fallback_group_id = self
                .output_workspace_groups
                .values()
                .copied()
                .find(|candidate| *candidate != group_id);
            let workspace_ids = self
                .workspace_groups
                .try_get(group_id)
                .map(|group| group.workspaces.clone())
                .unwrap_or_default();
            for workspace_id in workspace_ids {
                match fallback_group_id {
                    Some(fallback_group_id) => {
                        self.move_workspace_to_group_output(workspace_id, fallback_group_id)
                    }
                    None => {
                        let _ = self.assign_workspace_to_group(workspace_id, None);
                    }
                }
            }
            let _ = self.workspace_groups.remove(group_id);
        }
        if self.active_output == Some(output_id) {
            self.active_output = self.outputs.iter().next().map(|(other_id, _)| other_id);
        }
        Some(removed)
    }

    pub fn active_output_id(&self) -> Option<OutputId> {
        let (pointer_x, pointer_y) = self.seat.pointer_position();
        self.output_at_point(pointer_x.round() as i32, pointer_y.round() as i32)
            .or_else(|| {
                self.clamp_point_to_outputs(pointer_x.round() as i32, pointer_y.round() as i32)
                    .and_then(|(x, y)| self.output_at_point(x, y))
            })
            .or_else(|| {
                self.seat.keyboard_focus().and_then(|focus| {
                    focus.try_ref(self).and_then(|surface| {
                        self.layout_output_id_for_surface(focus.surface, surface)
                    })
                })
            })
            .or_else(|| self.fallback_output_id())
    }

    pub fn output_layout(&self) -> OutputLayout {
        OutputLayout::from_outputs(&self.outputs)
    }

    pub fn output_layout_bounds(&self) -> Option<RectI> {
        self.output_layout().extents()
    }

    pub fn output_bounds(&self, output_id: OutputId) -> Option<RectI> {
        self.output_layout().bounds(output_id)
    }

    pub fn output_at_point(&self, x: i32, y: i32) -> Option<OutputId> {
        self.output_layout().output_at(x, y)
    }

    pub fn clamp_point_to_outputs(&self, x: i32, y: i32) -> Option<(i32, i32)> {
        self.output_layout()
            .nearest_point(x, y)
            .map(|(_, point)| point)
    }

    pub fn outputs_intersecting_rect(&self, rect: RectI) -> Vec<OutputId> {
        self.output_layout().intersecting(rect)
    }

    pub fn primary_output_for_rect(
        &self,
        rect: RectI,
        preferred_output_id: Option<OutputId>,
    ) -> Option<OutputId> {
        let preferred_output_id = preferred_output_id.or(self.active_output_id());
        self.output_layout()
            .primary_output_for_rect(rect, preferred_output_id)
    }

    pub fn output_id_for_surface_object(
        &self,
        surface_id: Resolved<WlSurfaceId>,
        surface: &WlSurface,
    ) -> Option<OutputId> {
        self.layout_output_id_for_surface(surface_id, surface)
            .or_else(|| {
                let root_surface = if self.root_surfaces.contains(&surface_id) {
                    Some(surface_id)
                } else if let Some(parent_surface) = surface.parent_surface() {
                    self.root_surface_for_surface(*parent_surface)
                } else {
                    None
                };
                root_surface
                    .and_then(|surface| self.root_surface_workspace(surface))
                    .and_then(|workspace_id| self.workspace_output_id(workspace_id))
            })
            .or_else(|| self.active_output_id())
    }

    pub fn backend_for_output(&self, output_id: OutputId) -> Option<BackendOutputRef> {
        self.outputs
            .try_get(output_id)
            .map(|output| output.backend())
    }

    pub fn output_crtc_id(&self, output_id: OutputId) -> Option<u32> {
        self.outputs.try_get(output_id).map(Output::drm_crtc_id)
    }

    pub fn output_gamma_lut_size(&self, output_id: OutputId) -> Option<u32> {
        self.outputs
            .try_get(output_id)
            .map(|output| output.gamma_lut_size)
    }

    pub fn output_supports_gamma_control(&self, output_id: OutputId) -> bool {
        self.output_gamma_lut_size(output_id)
            .is_some_and(|size| size > 0)
    }

    pub fn output_supports_ctm_control(&self, output_id: OutputId) -> bool {
        self.outputs
            .try_get(output_id)
            .is_some_and(|output| output.ctm_supported)
    }

    pub fn any_output_supports_gamma_control(&self) -> bool {
        self.outputs
            .iter()
            .any(|(_, output)| output.enabled && output.gamma_lut_size > 0)
    }

    pub fn any_output_supports_ctm_control(&self) -> bool {
        self.outputs
            .iter()
            .any(|(_, output)| output.enabled && output.ctm_supported)
    }

    pub fn client_allows_ctm_control(&self, _client: &crate::clients::Client) -> bool {
        true
    }

    pub fn gamma_control_owner(
        &self,
        output_id: OutputId,
    ) -> Option<Resolved<ZwlrGammaControlV1Id>> {
        self.outputs
            .try_get(output_id)
            .and_then(|output| output.gamma_control_object_id)
    }

    pub fn set_gamma_control_owner(
        &mut self,
        output_id: OutputId,
        gamma_object: Resolved<ZwlrGammaControlV1Id>,
    ) -> Result<(), std::io::Error> {
        let Some(output) = self.outputs.try_get_mut(output_id) else {
            return Err(std::io::Error::new(std::io::ErrorKind::NotFound, "output disappeared"));
        };
        output.gamma_control_object_id = Some(gamma_object);
        Ok(())
    }

    pub fn clear_gamma_control_owner(
        &mut self,
        output_id: OutputId,
        gamma_object: Resolved<ZwlrGammaControlV1Id>,
    ) {
        let Some(output) = self.outputs.try_get_mut(output_id) else {
            return;
        };
        if output.gamma_control_object_id == Some(gamma_object) {
            output.gamma_control_object_id = None;
        }
    }

    pub fn set_output_gamma_lut(
        &mut self,
        output_id: OutputId,
        lut: Option<Vec<u16>>,
    ) -> Result<(), std::io::Error> {
        let Some(output) = self.outputs.try_get(output_id) else {
            return Err(std::io::Error::new(std::io::ErrorKind::NotFound, "output disappeared"));
        };
        let backend = output.backend;
        let drm_crtc_id = output.drm_crtc_id;
        let Some(drm) = self.drm.try_get_mut(backend.drm_id) else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "backend disappeared for output",
            ));
        };
        drm.set_output_gamma_lut(drm_crtc_id, lut)
    }

    pub fn set_output_ctm(
        &mut self,
        output_id: OutputId,
        ctm: Option<ColorTransformMatrix>,
    ) -> Result<(), std::io::Error> {
        let Some(output) = self.outputs.try_get(output_id) else {
            return Err(std::io::Error::new(std::io::ErrorKind::NotFound, "output disappeared"));
        };
        let backend = output.backend;
        let drm_crtc_id = output.drm_crtc_id;
        let Some(drm) = self.drm.try_get_mut(backend.drm_id) else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "backend disappeared for output",
            ));
        };
        drm.set_output_ctm(drm_crtc_id, ctm)
    }

    pub fn output_ids_for_drm(&self, drm_id: DrmId) -> Vec<OutputId> {
        self.outputs
            .iter()
            .filter_map(|(output_id, output)| {
                (output.backend.drm_id == drm_id).then_some(output_id)
            })
            .collect()
    }

    pub fn output_id_for_drm_crtc(&self, drm_id: DrmId, drm_crtc_id: u32) -> Option<OutputId> {
        self.outputs.iter().find_map(|(output_id, output)| {
            (output.backend.drm_id == drm_id && output.drm_crtc_id == drm_crtc_id)
                .then_some(output_id)
        })
    }

    pub fn active_output_id_for_drm(&self, drm_id: DrmId) -> Option<OutputId> {
        self.active_output_id()
            .filter(|output_id| {
                self.backend_for_output(*output_id)
                    .is_some_and(|backend| backend.drm_id == drm_id)
            })
            .or_else(|| self.output_ids_for_drm(drm_id).into_iter().next())
    }

    fn fallback_output_id(&self) -> Option<OutputId> {
        self.active_output
            .filter(|output_id| {
                self.outputs
                    .try_get(*output_id)
                    .is_some_and(|output| output.enabled)
            })
            .or_else(|| {
                self.output_layout()
                    .entries()
                    .first()
                    .map(|entry| entry.output_id)
            })
    }

    fn layout_output_id_for_surface(
        &self,
        resolved_surface: Resolved<WlSurfaceId>,
        surface: &WlSurface,
    ) -> Option<OutputId> {
        let preferred_output_id = surface.active_output_id().filter(|output_id| {
            self.outputs
                .try_get(*output_id)
                .is_some_and(|output| output.enabled)
        });
        let root_surface = (self.root_surfaces.contains(&resolved_surface) || surface.is_root())
            .then_some(resolved_surface)
            .or_else(|| {
                surface
                    .parent_surface()
                    .and_then(|parent| self.root_surface_for_surface(*parent))
            });
        let geometry = root_surface.and_then(|root_surface| {
            if root_surface == resolved_surface {
                self.committed_root_geometry_with_surface_ref(root_surface, surface)
            } else {
                self.committed_root_geometry(root_surface)
            }
        });
        geometry
            .and_then(|geometry| {
                let surface_bounds_global = geometry.surface_bounds_global()?;
                self.output_layout()
                    .primary_output_for_rect(surface_bounds_global, preferred_output_id)
            })
            .or(preferred_output_id)
    }

    // TODO: Remove this
    pub(crate) fn surface_object_id_for_client(
        &self,
        client_id: ClientId,
        surface_id: WlSurfaceId,
    ) -> Option<ObjectId> {
        self.objects.iter().find_map(|(object_id, object)| {
            object
                .downcast_ref::<WlSurface>()
                .is_some_and(|surface| {
                    surface.client_id() == client_id && surface.surface_id() == surface_id
                })
                .then_some(object_id)
        })
    }

    pub fn refresh_output_from_backend(&mut self, output_id: OutputId) -> bool {
        let Some(backend) = self.backend_for_output(output_id) else {
            return false;
        };
        let Some(drm) = self.drm.try_get(backend.drm_id) else {
            return false;
        };
        let Some(output) = self.outputs.try_get_mut(output_id) else {
            return false;
        };
        output.refresh_from_backend(drm);
        output.scale = output.configured_scale(&self.config);
        true
    }

    pub fn apply_output_scales_from_config(&mut self) -> Vec<OutputId> {
        let output_ids = self
            .outputs
            .iter()
            .map(|(output_id, _)| output_id)
            .collect::<Vec<_>>();
        let mut changed = Vec::new();
        for output_id in output_ids {
            let desired_scale = self.outputs[output_id].configured_scale(&self.config);
            let Some(output) = self.outputs.try_get_mut(output_id) else {
                continue;
            };
            if output.scale != desired_scale {
                output.scale = desired_scale;
                changed.push(output_id);
            }
        }
        changed
    }

    pub fn refresh_outputs_for_backend(&mut self, backend: BackendOutputRef) {
        let Some(drm) = self.drm.try_get(backend.drm_id) else {
            return;
        };
        let output_ids = self
            .outputs
            .iter()
            .filter_map(|(output_id, output)| (output.backend == backend).then_some(output_id))
            .collect::<Vec<_>>();

        for output_id in output_ids {
            if let Some(output) = self.outputs.try_get_mut(output_id) {
                output.refresh_from_backend(drm);
                output.scale = output.configured_scale(&self.config);
            }
        }
    }

    pub fn output_ids_for_backend(&self, backend: BackendOutputRef) -> Vec<OutputId> {
        self.outputs
            .iter()
            .filter_map(|(output_id, output)| (output.backend == backend).then_some(output_id))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use {
        super::*,
        crate::{
            clients::{Client, ClientDispatcher, RawRequest},
            protocol_generated::{WaylandId, WlSurfaceId},
            test_support::{
                resolved_wl_surface as root, state_for_tests, test_output_with_crtc_name,
            },
        },
    };

    #[test]
    fn root_surface_clip_clamps_radius_and_excludes_outer_corner_points() {
        let clip =
            RootSurfaceClip::new(RectI::new(10, 20, 40, 30).unwrap(), 99, RoundingPower::DEFAULT);

        assert_eq!(clip.corner_radius, 15);
        assert_eq!(clip.rounding_power, RoundingPower::DEFAULT);
        assert!(clip.contains_point(30.0, 35.0));
        assert!(!clip.contains_point(10.0, 20.0));
        assert!(!clip.contains_point(49.0, 20.0));
    }

    #[test]
    fn root_surface_clip_rounding_power_changes_corner_shape() {
        let circular = RootSurfaceClip::new(
            RectI::new(10, 20, 40, 40).unwrap(),
            12,
            RoundingPower::sanitize(2.0),
        );
        let squircle = RootSurfaceClip::new(
            RectI::new(10, 20, 40, 40).unwrap(),
            12,
            RoundingPower::sanitize(4.0),
        );

        assert!(!circular.contains_point(13.0, 23.0));
        assert!(squircle.contains_point(13.0, 23.0));
    }

    #[test]
    fn root_surface_visual_state_is_read_only_after_animation_tick() {
        let mut state = state_for_tests();
        let backend = BackendOutputRef::new(DrmId::from(0usize));
        let output_id =
            state.insert_output(test_output_with_crtc_name(backend, 1, 0, 0, 1280, 720));
        let workspace_id = state.active_workspace_for_output(output_id).unwrap();
        let (_, object_id) =
            state.insert_object(WlSurface::new(WlSurfaceId::from(90u32), ClientId::from_raw(5), 6));
        {
            let mut surface = state.objects.grab_object::<WlSurface>(object_id);
            surface.test_map(ObjectId::from(0usize), 30, 40, 160, 120);
        }
        state.config.effects.opacity.active_alpha = 255;
        state.config.effects.opacity.inactive_alpha = 120;
        state.config.effects.opacity.inactive_dim = 44;
        state.config.animations.window.duration_msec = 120;
        state.register_root_surface(object_id);
        state.assign_root_surface_to_workspace(root(&state, object_id), workspace_id);
        let surface = root(&state, object_id);
        state.seat.keyboard_focus = Some(crate::input::SurfaceFocus::new(surface));

        let advance = state.advance_window_visual_animations(10);
        assert!(advance.has_active_animations);

        let first = state.root_surface_visual_state(surface).unwrap();
        let second = state.root_surface_visual_state(surface).unwrap();

        assert_eq!(first.effects.opacity_alpha, second.effects.opacity_alpha);
        assert_eq!(first.effects.dim_alpha, second.effects.dim_alpha);
        assert_eq!(first.border_alpha, second.border_alpha);
        assert_eq!(first.shadow_alpha, second.shadow_alpha);
    }

    #[test]
    fn root_surface_visual_state_skips_blur_without_visible_decoration_ring() {
        let mut state = state_for_tests();
        let backend = BackendOutputRef::new(DrmId::from(0usize));
        let output_id =
            state.insert_output(test_output_with_crtc_name(backend, 1, 0, 0, 1280, 720));
        let workspace_id = state.active_workspace_for_output(output_id).unwrap();
        let (_, object_id) =
            state.insert_object(WlSurface::new(WlSurfaceId::from(91u32), ClientId::from_raw(6), 6));
        {
            let mut surface = state.objects.grab_object::<WlSurface>(object_id);
            surface.test_map(ObjectId::from(0usize), 30, 40, 160, 120);
        }
        state.config.decorations.border.thickness = 0;
        state.config.effects.blur.enabled = true;
        state.register_root_surface(object_id);
        state.assign_root_surface_to_workspace(root(&state, object_id), workspace_id);

        let visual = state
            .root_surface_visual_state(root(&state, object_id))
            .unwrap();

        assert_eq!(visual.outer_rect, visual.content_rect);
        assert!(visual.effects.blur.is_none());
    }

    #[test]
    fn protocol_geometry_override_rebuilds_when_cached_decoration_mode_differs() {
        let mut state = state_for_tests();
        let (_, object_id) =
            state.insert_object(WlSurface::new(WlSurfaceId::from(92u32), ClientId::from_raw(7), 6));
        {
            let mut surface = state.objects.grab_object::<WlSurface>(object_id);
            surface.test_map(ObjectId::from(0usize), 0, 0, 160, 120);
        }
        state.register_root_surface(object_id);
        let surface = root(&state, object_id);
        state.root_geometry_cache.insert(
            surface,
            Cell::new(RootGeometryCacheEntry {
                geometry: Some(crate::root_geometry::CommittedRootGeometry {
                    layout_rect_global: RectI::new(10, 20, 200, 140).unwrap(),
                    root_pos: (10, 20),
                    surface_bounds_local: Some(RectI::new(0, 0, 200, 140).unwrap()),
                    logical_window_local: Some(RectI::new(5, 5, 190, 130).unwrap()),
                    decoration_mode: Some(ToplevelDecorationMode::ServerSide),
                    decoration_insets: DecorationInsets { top: 5, right: 5, bottom: 5, left: 5 },
                    tiled_edges: 0,
                    fullscreen: false,
                }),
                dirty: false,
                revision: 0,
                built_from_revision: 0,
            }),
        );

        let surface_ref = surface.try_ref(&state).unwrap();
        let geometry = crate::root_geometry::resolve_committed_root_geometry_with_overrides(
            &state,
            surface,
            &surface_ref,
            crate::root_geometry::GeometryBuildOverrides {
                decoration_mode: Some(ToplevelDecorationMode::ClientSide),
                explicit_window_geometry: None,
                decoration_insets: None,
                root_pos: Some(surface_ref.root_position()),
            },
        )
        .unwrap();

        assert_eq!(geometry.decoration_mode, Some(ToplevelDecorationMode::ClientSide));
        assert_eq!(geometry.content_rect_global(), RectI::new(0, 0, 160, 120).unwrap());
        assert_eq!(geometry.window_target_rect(), RectI::new(0, 0, 160, 120).unwrap());
    }

    #[test]
    fn committed_root_geometry_rebuilds_dirty_entries_for_readers() {
        let mut state = state_for_tests();
        let (_, object_id) =
            state.insert_object(WlSurface::new(WlSurfaceId::from(93u32), ClientId::from_raw(8), 6));
        {
            let mut surface = state.objects.grab_object::<WlSurface>(object_id);
            surface.test_map(ObjectId::from(0usize), 0, 0, 160, 120);
        }
        state.register_root_surface(object_id);
        let surface = root(&state, object_id);

        {
            let mut surface_ref = state.objects.grab_object::<WlSurface>(object_id);
            surface_ref.set_root_position(45, 55);
        }
        state.mark_root_geometry_dirty(surface);

        let geometry = state
            .committed_root_geometry(surface)
            .expect("geometry should rebuild for dirty reader");

        assert_eq!(geometry.root_pos, (45, 55));
        assert_eq!(geometry.layout_rect_global, RectI::new(45, 55, 160, 120).unwrap());
        assert!(state.root_geometry_cache[&surface].get().is_fresh());
    }

    #[test]
    fn register_root_surface_prepopulates_geometry_cache_entry() {
        let mut state = state_for_tests();
        let (_, object_id) =
            state.insert_object(WlSurface::new(WlSurfaceId::from(94u32), ClientId::from_raw(9), 6));

        state.register_root_surface(object_id);

        let surface = root(&state, object_id);
        assert!(state.root_geometry_cache.contains_key(&surface));
    }

    #[test]
    fn unregister_root_surface_without_workspace_clears_geometry_cache_entry() {
        let mut state = state_for_tests();
        let (_, object_id) = state.insert_object(WlSurface::new(
            WlSurfaceId::from(95u32),
            ClientId::from_raw(10),
            6,
        ));

        state.register_root_surface(object_id);
        let surface = root(&state, object_id);
        state.root_surface_workspaces.remove(&surface);

        state.unregister_root_surface(surface);

        assert!(!state.root_geometry_cache.contains_key(&surface));
    }

    #[derive(Debug)]
    struct TestObject {
        id: WaylandId,
    }

    impl Object for TestObject {
        fn id(&self) -> WaylandId {
            self.id
        }

        fn on(
            self: crate::state::GrabbedObject<Self>,
            _client: &mut Client,
            _state: &mut State,
            _request: RawRequest,
            _dispatcher: &mut ClientDispatcher,
        ) -> crate::object::RequestResult {
            Ok(())
        }
    }

    #[derive(Debug)]
    struct OtherTestObject {
        id: WaylandId,
    }

    impl Object for OtherTestObject {
        fn id(&self) -> WaylandId {
            self.id
        }

        fn on(
            self: crate::state::GrabbedObject<Self>,
            _client: &mut Client,
            _state: &mut State,
            _request: RawRequest,
            _dispatcher: &mut ClientDispatcher,
        ) -> crate::object::RequestResult {
            Ok(())
        }
    }

    #[test]
    fn indexmap_try_grab_empty_slot() {
        let mut map = Box::new(IndexMap::<ObjectId, u32>::default());
        assert!(map.try_grab(ObjectId::from(0usize)).is_none());
    }

    #[test]
    fn indexmap_try_grab_roundtrip() {
        let mut map = Box::new(IndexMap::<ObjectId, u32>::default());
        let k = map.insert(99);
        {
            let g = map.try_grab(k).unwrap();
            assert_eq!(*g, 99);
        }
        assert_eq!(*map.get(k), 99);
    }

    #[test]
    fn indexmap_stale_key_after_reuse_is_invalid() {
        let mut map = Box::new(IndexMap::<ObjectId, u32>::default());
        let old_key = map.insert(10);

        assert_eq!(map.remove(old_key), Some(10));

        let new_key = map.insert(20);
        assert_eq!(new_key.index(), old_key.index());
        assert_ne!(new_key.generation(), old_key.generation());
        assert_eq!(map.try_get(old_key), None);
        assert!(!map.contains(old_key));
        assert_eq!(map.remove(old_key), None);
        assert_eq!(map.remove_or_defer(old_key), None);
        assert_eq!(*map.get(new_key), 20);
    }

    #[test]
    fn indexmap_deferred_remove_reuse_invalidates_previous_generation() {
        let mut map = Box::new(IndexMap::<ObjectId, u32>::default());
        let old_key = map.insert(42);

        {
            let grabbed = map.grab(old_key);
            assert!(map.remove_or_defer(old_key).is_none());
            drop(grabbed);
        }

        assert_eq!(map.try_get(old_key), None);
        let new_key = map.insert(77);
        assert_eq!(new_key.index(), old_key.index());
        assert_ne!(new_key.generation(), old_key.generation());
        assert_eq!(map.try_get(old_key), None);
        assert_eq!(*map.get(new_key), 77);
    }

    #[test]
    fn indexmap_try_grab_object_roundtrip() {
        let mut map = Box::new(IndexMap::<ObjectId, Box<dyn Object>>::default());
        let key = map.insert(Box::new(TestObject { id: WaylandId::from(1u32) }));
        {
            let object = map.try_grab_object::<TestObject>(key).unwrap();
            assert_eq!(object.id(), WaylandId::from(1u32));
        }
        assert!(map.try_ref_object::<TestObject>(key).is_some());
    }

    #[test]
    fn indexmap_try_grab_object_wrong_type_restores_slot() {
        let mut map = Box::new(IndexMap::<ObjectId, Box<dyn Object>>::default());
        let key = map.insert(Box::new(TestObject { id: WaylandId::from(1u32) }));
        assert!(map.try_grab_object::<OtherTestObject>(key).is_none());
        assert!(map.try_ref_object::<TestObject>(key).is_some());
    }

    #[test]
    fn indexmap_remove_or_defer_while_grabbed() {
        let mut map = Box::new(IndexMap::<ObjectId, u32>::default());
        let k = map.insert(42);
        assert!(map.contains(k));
        let g = map.grab(k);
        assert!(map.contains(k));
        assert!(map.remove_or_defer(k).is_none());
        assert!(!map.contains(k));
        drop(g);
        assert!(!map.contains(k));
    }

    #[test]
    fn output_layout_tracks_extents_and_lookup() {
        let mut outputs = IndexMap::<OutputId, Output>::default();
        let left_id = outputs.insert(Output {
            backend: BackendOutputRef::new(DrmId::from(1usize)),
            drm_crtc_id: 1,
            global_name: None,
            gamma_lut_size: 0,
            ctm_supported: false,
            gamma_control_object_id: None,
            x: -100,
            y: 20,
            physical_width: 0,
            physical_height: 0,
            scale: FractionalScale::ONE,
            transform: 0,
            make: String::new(),
            model: String::new(),
            name: "left".to_string(),
            description: String::new(),
            enabled: true,
            current_mode: OutputMode { width: 100, height: 50, refresh_millihz: 60_000 },
            preferred_mode: None,
        });
        let right_id = outputs.insert(Output {
            backend: BackendOutputRef::new(DrmId::from(2usize)),
            drm_crtc_id: 2,
            global_name: None,
            gamma_lut_size: 0,
            ctm_supported: false,
            gamma_control_object_id: None,
            x: 0,
            y: 0,
            physical_width: 0,
            physical_height: 0,
            scale: FractionalScale::ONE,
            transform: 0,
            make: String::new(),
            model: String::new(),
            name: "right".to_string(),
            description: String::new(),
            enabled: true,
            current_mode: OutputMode { width: 120, height: 80, refresh_millihz: 60_000 },
            preferred_mode: None,
        });
        let layout = OutputLayout::from_outputs(&outputs);
        assert_eq!(layout.extents(), RectI::from_extents(-100, 0, 120, 80));
        assert_eq!(layout.output_at(-50, 40), Some(left_id));
        assert_eq!(layout.output_at(10, 10), Some(right_id));
        assert_eq!(layout.output_at(1000, 1000), None);
    }

    #[test]
    fn output_layout_primary_output_prefers_overlap_then_hint() {
        let mut outputs = IndexMap::<OutputId, Output>::default();
        let first_id = outputs.insert(Output {
            backend: BackendOutputRef::new(DrmId::from(1usize)),
            drm_crtc_id: 1,
            global_name: None,
            gamma_lut_size: 0,
            ctm_supported: false,
            gamma_control_object_id: None,
            x: 0,
            y: 0,
            physical_width: 0,
            physical_height: 0,
            scale: FractionalScale::ONE,
            transform: 0,
            make: String::new(),
            model: String::new(),
            name: "first".to_string(),
            description: String::new(),
            enabled: true,
            current_mode: OutputMode { width: 100, height: 100, refresh_millihz: 60_000 },
            preferred_mode: None,
        });
        let second_id = outputs.insert(Output {
            backend: BackendOutputRef::new(DrmId::from(2usize)),
            drm_crtc_id: 2,
            global_name: None,
            gamma_lut_size: 0,
            ctm_supported: false,
            gamma_control_object_id: None,
            x: 100,
            y: 0,
            physical_width: 0,
            physical_height: 0,
            scale: FractionalScale::ONE,
            transform: 0,
            make: String::new(),
            model: String::new(),
            name: "second".to_string(),
            description: String::new(),
            enabled: true,
            current_mode: OutputMode { width: 100, height: 100, refresh_millihz: 60_000 },
            preferred_mode: None,
        });
        let layout = OutputLayout::from_outputs(&outputs);
        let even_split = RectI::new(50, 0, 100, 100).unwrap();
        assert_eq!(layout.primary_output_for_rect(even_split, Some(second_id)), Some(second_id));
        let mostly_first = RectI::new(10, 0, 120, 100).unwrap();
        assert_eq!(layout.primary_output_for_rect(mostly_first, Some(second_id)), Some(first_id));
    }

    #[test]
    fn output_layout_clamps_points_to_nearest_output_region() {
        let mut outputs = IndexMap::<OutputId, Output>::default();
        outputs.insert(Output {
            backend: BackendOutputRef::new(DrmId::from(1usize)),
            drm_crtc_id: 1,
            global_name: None,
            gamma_lut_size: 0,
            ctm_supported: false,
            gamma_control_object_id: None,
            x: 0,
            y: 0,
            physical_width: 0,
            physical_height: 0,
            scale: FractionalScale::ONE,
            transform: 0,
            make: String::new(),
            model: String::new(),
            name: "left".to_string(),
            description: String::new(),
            enabled: true,
            current_mode: OutputMode { width: 100, height: 100, refresh_millihz: 60_000 },
            preferred_mode: None,
        });
        let right_id = outputs.insert(Output {
            backend: BackendOutputRef::new(DrmId::from(2usize)),
            drm_crtc_id: 2,
            global_name: None,
            gamma_lut_size: 0,
            ctm_supported: false,
            gamma_control_object_id: None,
            x: 200,
            y: 0,
            physical_width: 0,
            physical_height: 0,
            scale: FractionalScale::ONE,
            transform: 0,
            make: String::new(),
            model: String::new(),
            name: "right".to_string(),
            description: String::new(),
            enabled: true,
            current_mode: OutputMode { width: 100, height: 100, refresh_millihz: 60_000 },
            preferred_mode: None,
        });
        let layout = OutputLayout::from_outputs(&outputs);
        assert_eq!(layout.nearest_point(150, 10), Some((right_id, (200, 10))));
        assert_eq!(layout.nearest_point(250, 50), Some((right_id, (250, 50))));
    }

    #[test]
    fn active_output_prefers_pointer_location_over_stored_fallback() {
        let mut state = state_for_tests();
        let left = state.insert_output(test_output_with_crtc_name(
            BackendOutputRef::new(DrmId::from(1usize)),
            1,
            0,
            0,
            100,
            100,
        ));
        let right = state.insert_output(test_output_with_crtc_name(
            BackendOutputRef::new(DrmId::from(1usize)),
            2,
            100,
            0,
            100,
            100,
        ));

        assert_eq!(state.active_output, Some(left));
        assert_eq!(state.active_output_id(), Some(left));

        state.outputs.try_get_mut(left).unwrap().x = -200;
        state.outputs.try_get_mut(right).unwrap().x = 0;

        assert_eq!(state.active_output_id(), Some(right));
    }

    #[test]
    fn output_for_surface_falls_back_to_current_output_without_bounds() {
        let mut state = state_for_tests();
        let output_id = state.insert_output(test_output_with_crtc_name(
            BackendOutputRef::new(DrmId::from(1usize)),
            1,
            0,
            0,
            100,
            100,
        ));
        let (_, surface_object_id) =
            state.insert_object(WlSurface::new(WlSurfaceId::from(1), ClientId::from_raw(2), 6));
        let surface_resolved = crate::clients::Resolved::new(
            ClientId::from_raw(2),
            WlSurfaceId::from(1),
            surface_object_id,
        );
        let surface = state
            .objects
            .try_ref_object::<WlSurface>(surface_object_id)
            .unwrap();

        assert_eq!(state.output_id_for_surface_object(surface_resolved, surface), Some(output_id));
    }

    #[test]
    fn insert_output_creates_workspace_group_and_default_workspace() {
        let mut state = state_for_tests();
        let backend = BackendOutputRef::new(DrmId::from(0usize));
        let output_id =
            state.insert_output(test_output_with_crtc_name(backend, 1, 0, 0, 1280, 720));

        let group_id = state.workspace_group_for_output(output_id).unwrap();
        let workspace_id = state.active_workspace_for_output(output_id).unwrap();
        let group = state.workspace_groups.try_get(group_id).unwrap();
        let workspace = state.workspaces.try_get(workspace_id).unwrap();

        assert_eq!(group.active_workspace, Some(workspace_id));
        assert_eq!(group.workspaces, vec![workspace_id]);
        assert_eq!(workspace.group, Some(group_id));
        assert_eq!(workspace.coordinates, vec![1]);
    }

    #[test]
    fn root_surface_visibility_tracks_active_workspace() {
        let mut state = state_for_tests();
        let backend = BackendOutputRef::new(DrmId::from(0usize));
        let output_id =
            state.insert_output(test_output_with_crtc_name(backend, 1, 0, 0, 1280, 720));
        let group_id = state.workspace_group_for_output(output_id).unwrap();
        let first_workspace = state.active_workspace_for_output(output_id).unwrap();
        let second_workspace = state.create_workspace(Some(group_id), Some("2".to_string()));

        let (_, object_id) =
            state.insert_object(WlSurface::new(WlSurfaceId::from(10u32), ClientId::from_raw(1), 6));
        state.register_root_surface(object_id);
        state.assign_root_surface_to_workspace(root(&state, object_id), second_workspace);

        assert!(!state.root_surface_is_visible(root(&state, object_id)));
        assert!(state.activate_workspace(second_workspace));
        assert!(state.root_surface_is_visible(root(&state, object_id)));
        assert!(!state.workspace_is_active(first_workspace));
    }

    #[test]
    fn assigning_root_surface_creates_persistent_managed_state() {
        let mut state = state_for_tests();
        let backend = BackendOutputRef::new(DrmId::from(0usize));
        let output_id =
            state.insert_output(test_output_with_crtc_name(backend, 1, 0, 0, 1280, 720));
        let workspace_id = state.active_workspace_for_output(output_id).unwrap();

        let (_, object_id) =
            state.insert_object(WlSurface::new(WlSurfaceId::from(12u32), ClientId::from_raw(3), 6));
        state.register_root_surface(object_id);
        assert_eq!(state.root_surface_workspace(root(&state, object_id)), Some(workspace_id));

        let managed = state
            .managed_root_surface_state(root(&state, object_id))
            .unwrap();
        assert_eq!(managed.workspace_id, workspace_id);
        assert_eq!(managed.mode, ManagedWindowMode::Tiled);
        assert!(!managed.fullscreen);
        assert!(!managed.restore_floating_state_on_next_configure);

        assert!(state.set_root_surface_floating(root(&state, object_id), true));
        assert_eq!(
            state
                .managed_root_surface_state(root(&state, object_id))
                .unwrap()
                .mode,
            ManagedWindowMode::Floating
        );
    }

    #[test]
    fn xdg_decoration_policy_defaults_to_client_without_protocol() {
        let state = state_for_tests();

        assert_eq!(
            state.xdg_decoration_mode_for_surface(None, false, None),
            ToplevelDecorationMode::ClientSide
        );
    }

    #[test]
    fn xdg_decoration_policy_forces_server_side_for_tiled_windows() {
        let mut state = state_for_tests();
        let backend = BackendOutputRef::new(DrmId::from(0usize));
        let output_id =
            state.insert_output(test_output_with_crtc_name(backend, 1, 0, 0, 1280, 720));
        let _workspace_id = state.active_workspace_for_output(output_id).unwrap();
        let (_, object_id) = state.insert_object(WlSurface::new(
            WlSurfaceId::from(50u32),
            ClientId::from_raw(50),
            6,
        ));
        state.register_root_surface(object_id);
        let surface = state
            .root_surfaces
            .iter()
            .copied()
            .find(|surface| surface.object_id() == object_id);

        assert_eq!(
            state.xdg_decoration_mode_for_surface(
                surface,
                true,
                Some(ToplevelDecorationMode::ClientSide)
            ),
            ToplevelDecorationMode::ServerSide
        );
    }

    #[test]
    fn xdg_decoration_policy_honors_client_for_floating_windows() {
        let mut state = state_for_tests();
        let backend = BackendOutputRef::new(DrmId::from(0usize));
        let output_id =
            state.insert_output(test_output_with_crtc_name(backend, 1, 0, 0, 1280, 720));
        let _workspace_id = state.active_workspace_for_output(output_id).unwrap();
        let (_, object_id) = state.insert_object(WlSurface::new(
            WlSurfaceId::from(51u32),
            ClientId::from_raw(51),
            6,
        ));
        state.register_root_surface(object_id);
        assert!(state.set_root_surface_floating(root(&state, object_id), true));
        let surface = state
            .root_surfaces
            .iter()
            .copied()
            .find(|surface| surface.object_id() == object_id);

        assert_eq!(
            state.xdg_decoration_mode_for_surface(
                surface,
                true,
                Some(ToplevelDecorationMode::ClientSide)
            ),
            ToplevelDecorationMode::ClientSide
        );
        assert_eq!(
            state.xdg_decoration_mode_for_surface(
                surface,
                true,
                Some(ToplevelDecorationMode::ServerSide)
            ),
            ToplevelDecorationMode::ServerSide
        );
    }

    #[test]
    fn xdg_decoration_policy_uses_unset_mode_for_unset_requests() {
        let mut state = state_for_tests();
        let backend = BackendOutputRef::new(DrmId::from(0usize));
        let output_id =
            state.insert_output(test_output_with_crtc_name(backend, 1, 0, 0, 1280, 720));
        let _workspace_id = state.active_workspace_for_output(output_id).unwrap();
        let (_, object_id) = state.insert_object(WlSurface::new(
            WlSurfaceId::from(52u32),
            ClientId::from_raw(52),
            6,
        ));
        state.register_root_surface(object_id);
        assert!(state.set_root_surface_floating(root(&state, object_id), true));
        let surface = state
            .root_surfaces
            .iter()
            .copied()
            .find(|surface| surface.object_id() == object_id);

        assert_eq!(
            state.xdg_decoration_mode_for_surface(surface, true, None),
            ToplevelDecorationMode::ServerSide
        );
    }

    #[test]
    fn floating_mode_restores_remembered_floating_state_when_reenabled() {
        let mut state = state_for_tests();
        let backend = BackendOutputRef::new(DrmId::from(0usize));
        let output_id =
            state.insert_output(test_output_with_crtc_name(backend, 1, 0, 0, 1280, 720));
        let _workspace_id = state.active_workspace_for_output(output_id).unwrap();

        let (_, object_id) =
            state.insert_object(WlSurface::new(WlSurfaceId::from(15u32), ClientId::from_raw(6), 6));
        {
            let mut surface = state.objects.grab_object::<WlSurface>(object_id);
            surface.test_map(ObjectId::from(0usize), 20, 30, 120, 90);
        }
        state.register_root_surface(object_id);

        assert!(state.set_root_surface_floating(root(&state, object_id), true));
        {
            let mut surface = state.objects.grab_object::<WlSurface>(object_id);
            surface.set_root_position(250, 180);
        }
        state.mark_root_geometry_dirty(root(&state, object_id));
        let _ = state.committed_root_geometry(root(&state, object_id));

        assert!(state.set_root_surface_floating(root(&state, object_id), false));
        assert_eq!(
            state
                .managed_root_surface_state(root(&state, object_id))
                .unwrap()
                .remembered_floating_state,
            Some(RememberedFloatingState { root_pos: (250, 180), configure_size: (120, 90) })
        );

        assert!(state.set_root_surface_floating(root(&state, object_id), true));
        let surface = state
            .objects
            .try_ref_object::<WlSurface>(object_id)
            .unwrap();
        assert_eq!(surface.root_position(), (250, 180));
        assert_eq!(
            state
                .managed_root_surface_state(root(&state, object_id))
                .unwrap()
                .restore_floating_state_on_next_configure,
            true
        );
    }

    #[test]
    fn fullscreen_state_is_workspace_scoped_and_replaced_cleanly() {
        let mut state = state_for_tests();
        let backend = BackendOutputRef::new(DrmId::from(0usize));
        let output_id =
            state.insert_output(test_output_with_crtc_name(backend, 1, 0, 0, 1280, 720));
        let workspace_id = state.active_workspace_for_output(output_id).unwrap();

        let (_, first_object_id) =
            state.insert_object(WlSurface::new(WlSurfaceId::from(13u32), ClientId::from_raw(4), 6));
        let (_, second_object_id) =
            state.insert_object(WlSurface::new(WlSurfaceId::from(14u32), ClientId::from_raw(5), 6));
        state.register_root_surface(first_object_id);
        state.register_root_surface(second_object_id);
        assert_eq!(state.root_surface_workspace(root(&state, first_object_id)), Some(workspace_id));
        assert_eq!(
            state.root_surface_workspace(root(&state, second_object_id)),
            Some(workspace_id)
        );

        assert!(state.set_root_surface_fullscreen(root(&state, first_object_id), true));
        assert!(
            state
                .managed_root_surface_state(root(&state, first_object_id))
                .unwrap()
                .fullscreen
        );
        assert!(
            !state
                .managed_root_surface_state(root(&state, second_object_id))
                .unwrap()
                .fullscreen
        );

        assert!(state.set_root_surface_fullscreen(root(&state, second_object_id), true));
        assert!(
            !state
                .managed_root_surface_state(root(&state, first_object_id))
                .unwrap()
                .fullscreen
        );
        assert!(
            state
                .managed_root_surface_state(root(&state, second_object_id))
                .unwrap()
                .fullscreen
        );
    }

    #[test]
    fn fullscreen_round_trip_restores_floating_rect() {
        let mut state = state_for_tests();
        let backend = BackendOutputRef::new(DrmId::from(0usize));
        let output_id =
            state.insert_output(test_output_with_crtc_name(backend, 1, 0, 0, 1280, 720));
        let _workspace_id = state.active_workspace_for_output(output_id).unwrap();

        let (_, object_id) =
            state.insert_object(WlSurface::new(WlSurfaceId::from(18u32), ClientId::from_raw(9), 6));
        {
            let mut surface = state.objects.grab_object::<WlSurface>(object_id);
            surface.test_map(ObjectId::from(0usize), 30, 40, 160, 120);
        }
        state.register_root_surface(object_id);
        assert!(state.set_root_surface_floating(root(&state, object_id), true));
        {
            let mut surface = state.objects.grab_object::<WlSurface>(object_id);
            surface.set_root_position(320, 240);
        }
        state.mark_root_geometry_dirty(root(&state, object_id));
        let _ = state.committed_root_geometry(root(&state, object_id));

        assert!(state.set_root_surface_fullscreen(root(&state, object_id), true));
        assert!(state.set_root_surface_fullscreen(root(&state, object_id), false));

        let surface = state
            .objects
            .try_ref_object::<WlSurface>(object_id)
            .unwrap();
        assert_eq!(surface.root_position(), (320, 240));
        assert_eq!(
            state
                .managed_root_surface_state(root(&state, object_id))
                .unwrap()
                .restore_floating_state_on_next_configure,
            true
        );
    }

    #[test]
    fn remembered_floating_state_uses_root_pos_and_configure_size() {
        let mut state = state_for_tests();
        let backend = BackendOutputRef::new(DrmId::from(0usize));
        let output_id =
            state.insert_output(test_output_with_crtc_name(backend, 1, 0, 0, 1280, 720));
        let _workspace_id = state.active_workspace_for_output(output_id).unwrap();

        let (_, object_id) = state.insert_object(WlSurface::new(
            WlSurfaceId::from(19u32),
            ClientId::from_raw(10),
            6,
        ));
        {
            let mut surface = state.objects.grab_object::<WlSurface>(object_id);
            surface.test_map(ObjectId::from(1usize), 20, 30, 120, 90);
        }
        state.register_root_surface(object_id);
        let root_surface = root(&state, object_id);
        state.root_geometry_cache.insert(
            root_surface,
            Cell::new(RootGeometryCacheEntry {
                geometry: Some(crate::root_geometry::CommittedRootGeometry {
                    layout_rect_global: RectI { x: 40, y: 50, width: 120, height: 90 },
                    root_pos: (40, 50),
                    surface_bounds_local: Some(RectI { x: 0, y: 0, width: 120, height: 90 }),
                    logical_window_local: Some(RectI { x: 10, y: 10, width: 100, height: 70 }),
                    decoration_mode: None,
                    decoration_insets: DecorationInsets::default(),
                    tiled_edges: 0,
                    fullscreen: false,
                }),
                dirty: false,
                revision: 0,
                built_from_revision: 0,
            }),
        );
        state
            .managed_root_surfaces
            .get_mut(&root_surface)
            .unwrap()
            .mode = ManagedWindowMode::Floating;

        assert!(state.set_root_surface_floating(root_surface, false));
        assert_eq!(
            state
                .managed_root_surface_state(root_surface)
                .unwrap()
                .remembered_floating_state,
            Some(RememberedFloatingState { root_pos: (40, 50), configure_size: (120, 90) })
        );
    }

    #[test]
    fn moving_workspace_between_outputs_translates_assigned_root_surfaces() {
        let mut state = state_for_tests();
        let backend = BackendOutputRef::new(DrmId::from(0usize));
        let left_output =
            state.insert_output(test_output_with_crtc_name(backend, 1, 0, 0, 100, 100));
        let right_output =
            state.insert_output(test_output_with_crtc_name(backend, 2, 200, 0, 100, 100));
        let left_workspace = state.active_workspace_for_output(left_output).unwrap();
        let right_group = state.workspace_group_for_output(right_output).unwrap();

        let (_, object_id) =
            state.insert_object(WlSurface::new(WlSurfaceId::from(11u32), ClientId::from_raw(2), 6));
        {
            let mut surface = state.objects.grab_object::<WlSurface>(object_id);
            surface.test_map(ObjectId::from(0usize), 10, 20, 50, 50);
        }
        state.register_root_surface(object_id);
        state.assign_root_surface_to_workspace(root(&state, object_id), left_workspace);

        state.move_workspace_to_group_output(left_workspace, right_group);

        let surface = state
            .objects
            .try_ref_object::<WlSurface>(object_id)
            .unwrap();
        assert_eq!(surface.root_position(), (210, 20));
        assert_eq!(state.workspace_output_id(left_workspace), Some(right_output));
    }

    #[test]
    fn moving_workspace_between_outputs_translates_stored_floating_rects() {
        let mut state = state_for_tests();
        let backend = BackendOutputRef::new(DrmId::from(0usize));
        let left_output =
            state.insert_output(test_output_with_crtc_name(backend, 1, 0, 0, 100, 100));
        let right_output =
            state.insert_output(test_output_with_crtc_name(backend, 2, 200, 0, 100, 100));
        let left_workspace = state.active_workspace_for_output(left_output).unwrap();
        let right_group = state.workspace_group_for_output(right_output).unwrap();

        let (_, object_id) =
            state.insert_object(WlSurface::new(WlSurfaceId::from(16u32), ClientId::from_raw(7), 6));
        state.register_root_surface(object_id);
        state.assign_root_surface_to_workspace(root(&state, object_id), left_workspace);
        let surface = root(&state, object_id);
        state
            .managed_root_surfaces
            .get_mut(&surface)
            .unwrap()
            .remembered_floating_state =
            Some(RememberedFloatingState { root_pos: (10, 20), configure_size: (50, 50) });

        state.move_workspace_to_group_output(left_workspace, right_group);

        assert_eq!(
            state
                .managed_root_surface_state(root(&state, object_id))
                .unwrap()
                .remembered_floating_state,
            Some(RememberedFloatingState { root_pos: (210, 20), configure_size: (50, 50) })
        );
    }

    #[test]
    fn moving_workspace_between_outputs_keeps_scheduled_floating_restore() {
        let mut state = state_for_tests();
        let backend = BackendOutputRef::new(DrmId::from(0usize));
        let left_output =
            state.insert_output(test_output_with_crtc_name(backend, 1, 0, 0, 100, 100));
        let right_output =
            state.insert_output(test_output_with_crtc_name(backend, 2, 200, 0, 100, 100));
        let left_workspace = state.active_workspace_for_output(left_output).unwrap();
        let right_group = state.workspace_group_for_output(right_output).unwrap();

        let (_, object_id) =
            state.insert_object(WlSurface::new(WlSurfaceId::from(17u32), ClientId::from_raw(8), 6));
        state.register_root_surface(object_id);
        state.assign_root_surface_to_workspace(root(&state, object_id), left_workspace);
        let surface = root(&state, object_id);
        state
            .managed_root_surfaces
            .get_mut(&surface)
            .unwrap()
            .remembered_floating_state =
            Some(RememberedFloatingState { root_pos: (10, 20), configure_size: (50, 50) });
        state
            .managed_root_surfaces
            .get_mut(&surface)
            .unwrap()
            .restore_floating_state_on_next_configure = true;

        state.move_workspace_to_group_output(left_workspace, right_group);

        assert_eq!(
            state
                .managed_root_surface_state(root(&state, object_id))
                .unwrap()
                .remembered_floating_state,
            Some(RememberedFloatingState { root_pos: (210, 20), configure_size: (50, 50) })
        );
        assert!(
            state
                .managed_root_surface_state(root(&state, object_id))
                .unwrap()
                .restore_floating_state_on_next_configure
        );
    }

    #[test]
    fn create_workspace_with_missing_group_stays_unassigned() {
        let mut state = state_for_tests();

        let workspace_id = state.create_workspace(Some(WorkspaceGroupId::from(999usize)), None);
        let workspace = state.workspaces.try_get(workspace_id).unwrap();

        assert_eq!(workspace.group, None);
        assert!(workspace.coordinates.is_empty());
    }

    #[test]
    fn assign_workspace_to_missing_group_is_rejected() {
        let mut state = state_for_tests();
        let backend = BackendOutputRef::new(DrmId::from(0usize));
        let output_id =
            state.insert_output(test_output_with_crtc_name(backend, 1, 0, 0, 1280, 720));
        let workspace_id = state.active_workspace_for_output(output_id).unwrap();
        let original_group = state.workspaces.try_get(workspace_id).unwrap().group;

        assert!(
            !state.assign_workspace_to_group(workspace_id, Some(WorkspaceGroupId::from(999usize)))
        );
        assert_eq!(state.workspaces.try_get(workspace_id).unwrap().group, original_group);
    }

    #[test]
    fn unassigning_workspace_clears_coordinates() {
        let mut state = state_for_tests();
        let backend = BackendOutputRef::new(DrmId::from(0usize));
        let output_id =
            state.insert_output(test_output_with_crtc_name(backend, 1, 0, 0, 1280, 720));
        let workspace_id = state.active_workspace_for_output(output_id).unwrap();

        assert_eq!(state.workspaces.try_get(workspace_id).unwrap().coordinates, vec![1]);
        assert!(state.assign_workspace_to_group(workspace_id, None));

        let workspace = state.workspaces.try_get(workspace_id).unwrap();
        assert_eq!(workspace.group, None);
        assert!(workspace.coordinates.is_empty());
    }
}
