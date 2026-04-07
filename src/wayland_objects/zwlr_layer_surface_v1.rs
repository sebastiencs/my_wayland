use std::collections::{BTreeMap, VecDeque};

use crate::{
    clients::{Client, ClientDispatcher, ClientId, EventWriter, RawRequest, Resolved},
    object::{Object, Parser, ParserError, RequestResult},
    protocol_generated::{
        wl_display, zwlr_layer_shell_v1, zwlr_layer_surface_v1, Constants, WaylandId, WlSurfaceId,
        XdgPopupId, ZwlrLayerShellV1Id, ZwlrLayerSurfaceV1Id,
    },
    region::RectI,
    seat::{refresh_keyboard_focus, refresh_keyboard_focus_with_snapshot},
    state::{GrabbedObject, ObjectId, OutputId, State},
    wayland_objects::{
        wl_display::WlDisplay,
        wl_surface::WlSurface,
        xdg_popup::{AssignLayerParentError, XdgPopup},
        zwlr_layer_shell_v1::ZwlrLayerShellV1,
    },
};

const ANCHOR_TOP: u32 = 1;
const ANCHOR_BOTTOM: u32 = 2;
const ANCHOR_LEFT: u32 = 4;
const ANCHOR_RIGHT: u32 = 8;
const ANCHOR_MASK: u32 = ANCHOR_TOP | ANCHOR_BOTTOM | ANCHOR_LEFT | ANCHOR_RIGHT;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct LayerMargins {
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
    pub left: i32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum LayerShellLayer {
    Background,
    Bottom,
    Top,
    Overlay,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LayerKeyboardFocus {
    None,
    OnDemand { layer: LayerShellLayer },
    Exclusive { layer: LayerShellLayer },
}

fn max_keyboard_interactivity_for_version(version: u32) -> u32 {
    if version >= 4 {
        zwlr_layer_surface_v1::KeyboardInteractivity::OnDemand as u32
    } else {
        zwlr_layer_surface_v1::KeyboardInteractivity::Exclusive as u32
    }
}

impl LayerShellLayer {
    pub fn from_raw(raw: u32) -> Option<Self> {
        Some(match raw {
            value if value == zwlr_layer_shell_v1::Layer::Background as u32 => Self::Background,
            value if value == zwlr_layer_shell_v1::Layer::Bottom as u32 => Self::Bottom,
            value if value == zwlr_layer_shell_v1::Layer::Top as u32 => Self::Top,
            value if value == zwlr_layer_shell_v1::Layer::Overlay as u32 => Self::Overlay,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct LayerSurfaceState {
    layer: LayerShellLayer,
    desired_width: i32,
    desired_height: i32,
    anchor: u32,
    exclusive_zone: i32,
    margin: LayerMargins,
    keyboard_interactivity: u32,
    exclusive_edge: Option<u32>,
}

impl LayerSurfaceState {
    fn new(layer: LayerShellLayer) -> Self {
        Self {
            layer,
            desired_width: 0,
            desired_height: 0,
            anchor: 0,
            exclusive_zone: 0,
            margin: LayerMargins::default(),
            keyboard_interactivity: zwlr_layer_surface_v1::KeyboardInteractivity::None as u32,
            exclusive_edge: None,
        }
    }
}

#[derive(Clone, Debug, Default)]
struct PendingLayerSurfaceState {
    desired_width: Option<i32>,
    desired_height: Option<i32>,
    anchor: Option<u32>,
    exclusive_zone: Option<i32>,
    margin: Option<LayerMargins>,
    keyboard_interactivity: Option<u32>,
    layer: Option<LayerShellLayer>,
    exclusive_edge: Option<Option<u32>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct LayerConfigure {
    serial: u32,
    width: u32,
    height: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LayerPlacement {
    pub output_id: OutputId,
    pub rect: RectI,
    pub configure_size: (u32, u32),
    pub layer: LayerShellLayer,
}

#[derive(Clone, Copy, Debug)]
struct LayerSnapshot {
    root_surface: Resolved<WlSurfaceId>,
    root_index: usize,
    output_id: OutputId,
    mapped: bool,
    layer: LayerShellLayer,
    desired_width: i32,
    desired_height: i32,
    anchor: u32,
    exclusive_zone: i32,
    margin: LayerMargins,
    exclusive_edge: Option<u32>,
    actual_width: i32,
    actual_height: i32,
}

#[derive(Debug, Default)]
struct ComputedLayerLayout {
    placements: BTreeMap<Resolved<WlSurfaceId>, LayerPlacement>,
    usable_by_output: BTreeMap<OutputId, RectI>,
    background_roots: Vec<Resolved<WlSurfaceId>>,
    bottom_roots: Vec<Resolved<WlSurfaceId>>,
    top_roots: Vec<Resolved<WlSurfaceId>>,
    overlay_roots: Vec<Resolved<WlSurfaceId>>,
}

#[derive(Clone, Copy, Debug)]
pub struct LayerLayoutObjectSnapshot<'a> {
    pub root_surface: Resolved<WlSurfaceId>,
    pub surface: &'a WlSurface,
    pub resolved_layer_surface: Option<Resolved<ZwlrLayerSurfaceV1Id>>,
    pub layer_surface: Option<&'a ZwlrLayerSurfaceV1>,
}

impl<'a> LayerLayoutObjectSnapshot<'a> {
    pub fn for_surface(root_surface: Resolved<WlSurfaceId>, surface: &'a WlSurface) -> Self {
        Self {
            root_surface,
            surface,
            resolved_layer_surface: None,
            layer_surface: None,
        }
    }

    pub fn with_layer_surface(
        mut self,
        resolved_layer_surface: Resolved<ZwlrLayerSurfaceV1Id>,
        layer_surface: &'a ZwlrLayerSurfaceV1,
    ) -> Self {
        self.resolved_layer_surface = Some(resolved_layer_surface);
        self.layer_surface = Some(layer_surface);
        self
    }
}

#[derive(Debug)]
pub struct ZwlrLayerSurfaceV1 {
    id: ZwlrLayerSurfaceV1Id,
    manager: Resolved<ZwlrLayerShellV1Id>,
    version: u32,
    surface: Resolved<WlSurfaceId>,
    requested_output_id: Option<OutputId>,
    assigned_output_id: Option<OutputId>,
    namespace: String,
    initial_layer: LayerShellLayer,
    current: LayerSurfaceState,
    pending: PendingLayerSurfaceState,
    initial_commit_complete: bool,
    mapped: bool,
    closed: bool,
    needs_layout_refresh: bool,
    next_configure_serial: u32,
    pending_configures: VecDeque<LayerConfigure>,
    last_acked: Option<LayerConfigure>,
    last_sent_configure_size: Option<(u32, u32)>,
    popup_children: Vec<Resolved<XdgPopupId>>,
}

impl ZwlrLayerSurfaceV1 {
    pub fn new(
        id: ZwlrLayerSurfaceV1Id,
        manager: Resolved<ZwlrLayerShellV1Id>,
        version: u32,
        surface: Resolved<WlSurfaceId>,
        requested_output_id: Option<OutputId>,
        layer: LayerShellLayer,
        namespace: String,
    ) -> Self {
        Self {
            id,
            manager,
            version,
            surface,
            requested_output_id,
            assigned_output_id: requested_output_id,
            namespace,
            initial_layer: layer,
            current: LayerSurfaceState::new(layer),
            pending: PendingLayerSurfaceState::default(),
            initial_commit_complete: false,
            mapped: false,
            closed: false,
            needs_layout_refresh: false,
            next_configure_serial: 1,
            pending_configures: VecDeque::new(),
            last_acked: None,
            last_sent_configure_size: None,
            popup_children: Vec::new(),
        }
    }

    pub fn client_id(&self) -> ClientId {
        self.surface.client_id()
    }

    pub fn surface_id(&self) -> WlSurfaceId {
        self.surface.wayland_id()
    }

    pub fn surface_object_id(&self) -> ObjectId {
        self.surface.object_id()
    }

    pub fn surface(&self) -> Resolved<WlSurfaceId> {
        self.surface
    }

    pub fn manager(&self) -> Resolved<ZwlrLayerShellV1Id> {
        self.manager
    }

    pub fn requested_output_id(&self) -> Option<OutputId> {
        self.requested_output_id
    }

    fn ensure_output_assignment(&mut self, state: &State) {
        if self.requested_output_id.is_some() {
            return;
        }
        if self
            .assigned_output_id
            .is_some_and(|output_id| state.output_bounds(output_id).is_some())
        {
            return;
        }
        self.assigned_output_id = state.active_output_id();
    }

    pub fn resolved_output_id(&self, state: &State) -> Option<OutputId> {
        match self.requested_output_id {
            Some(output_id) => state.output_bounds(output_id).map(|_| output_id),
            None => self
                .assigned_output_id
                .filter(|output_id| state.output_bounds(*output_id).is_some()),
        }
    }

    pub fn layer(&self) -> LayerShellLayer {
        self.current.layer
    }

    pub fn keyboard_interactivity(&self) -> u32 {
        self.current.keyboard_interactivity
    }

    pub fn is_mapped(&self) -> bool {
        self.mapped
    }

    pub fn namespace(&self) -> &str {
        &self.namespace
    }

    pub fn add_popup_child(&mut self, popup: Resolved<XdgPopupId>) {
        self.popup_children.push(popup);
    }

    pub fn remove_popup_child(&mut self, popup: Resolved<XdgPopupId>) {
        self.popup_children.retain(|candidate| *candidate != popup);
    }

    pub fn has_popup_child(&self, popup: Resolved<XdgPopupId>) -> bool {
        self.popup_children.contains(&popup)
    }

    pub fn is_topmost_popup(&self, popup: Resolved<XdgPopupId>) -> bool {
        self.popup_children.last().copied() == Some(popup)
    }

    fn dismiss_popup_children(
        &mut self,
        client: &mut Client,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
    ) {
        let popups = std::mem::take(&mut self.popup_children);
        if popups.is_empty() {
            return;
        }
        client.dismiss_popup_ids(
            state,
            dispatcher,
            popups
                .into_iter()
                .filter(|popup| popup.exists(state))
                .collect(),
            Some(self.id),
        );
    }

    pub fn popup_anchor_geometry(&self, state: &State) -> RectI {
        let Some(surface) = self.surface.try_ref(state) else {
            return RectI { x: 0, y: 0, width: 0, height: 0 };
        };
        surface
            .logical_size()
            .map(|size| RectI { x: 0, y: 0, width: size.width, height: size.height })
            .unwrap_or(RectI { x: 0, y: 0, width: 0, height: 0 })
    }

    fn reset_after_unmap(&mut self) {
        // Keep the configure queue so in-flight configures stay distinguishable
        // after an unmap/remap cycle. Smithay carries this for clients like waybar.
        let pending_configures = std::mem::take(&mut self.pending_configures);
        self.current = LayerSurfaceState::new(self.initial_layer);
        self.pending = PendingLayerSurfaceState::default();
        self.initial_commit_complete = false;
        self.mapped = false;
        self.pending_configures = pending_configures;
        self.last_acked = None;
        self.last_sent_configure_size = None;
        self.needs_layout_refresh = true;
    }

    fn make_inert_after_close(&mut self) {
        self.current = LayerSurfaceState::new(self.initial_layer);
        self.pending = PendingLayerSurfaceState::default();
        self.initial_commit_complete = false;
        self.mapped = false;
        self.pending_configures.clear();
        self.last_acked = None;
        self.last_sent_configure_size = None;
        self.needs_layout_refresh = false;
    }

    fn next_serial(&mut self) -> u32 {
        let serial = self.next_configure_serial;
        self.next_configure_serial = self.next_configure_serial.saturating_add(1);
        serial
    }

    fn send_error(
        &self,
        client: &Client,
        dispatcher: &mut ClientDispatcher,
        code: zwlr_layer_surface_v1::Error,
        message: impl AsRef<str>,
    ) {
        client.send_protocol_error(dispatcher, self.id.into(), code as u32, message);
    }

    fn parse_request(
        &self,
        client: &mut Client,
        req: RawRequest,
        dispatcher: &mut ClientDispatcher,
    ) -> Option<<Self as Parser>::Request> {
        match client.parse::<Self>(req) {
            Ok(request) => Some(request),
            Err(err) => {
                self.send_parse_error(client, dispatcher, err);
                None
            }
        }
    }

    fn send_parse_error(
        &self,
        client: &mut Client,
        dispatcher: &mut ClientDispatcher,
        err: ParserError,
    ) {
        client.protocol_violation_disconnect(
            dispatcher,
            self.id.into(),
            wl_display::Error::InvalidMethod as u32,
            format!("malformed zwlr_layer_surface_v1 request: {err:?}"),
        );
    }

    fn unregister_from_manager(&self, state: &mut State) {
        let Some(mut manager) = self.manager.try_grab(state) else {
            return;
        };
        manager.unregister_surface(self.id);
    }

    fn apply_pending_state(
        &mut self,
        client: &Client,
        dispatcher: &mut ClientDispatcher,
    ) -> Result<(), ()> {
        let mut changed = false;
        if let Some(width) = self.pending.desired_width.take() {
            self.current.desired_width = width;
            changed = true;
        }
        if let Some(height) = self.pending.desired_height.take() {
            self.current.desired_height = height;
            changed = true;
        }
        if let Some(anchor) = self.pending.anchor.take() {
            self.current.anchor = anchor;
            changed = true;
        }
        if let Some(exclusive_zone) = self.pending.exclusive_zone.take() {
            self.current.exclusive_zone = exclusive_zone;
            changed = true;
        }
        if let Some(margin) = self.pending.margin.take() {
            self.current.margin = margin;
            changed = true;
        }
        if let Some(keyboard_interactivity) = self.pending.keyboard_interactivity.take() {
            self.current.keyboard_interactivity = keyboard_interactivity;
            changed = true;
        }
        if let Some(layer) = self.pending.layer.take() {
            self.current.layer = layer;
            changed = true;
        }
        if let Some(exclusive_edge) = self.pending.exclusive_edge.take() {
            self.current.exclusive_edge = exclusive_edge;
            changed = true;
        }

        if self.current.anchor & !ANCHOR_MASK != 0 {
            self.send_error(
                client,
                dispatcher,
                zwlr_layer_surface_v1::Error::InvalidAnchor,
                "zwlr_layer_surface_v1.set_anchor contained unknown bits",
            );
            return Err(());
        }

        let max_keyboard = max_keyboard_interactivity_for_version(self.version);
        if self.current.keyboard_interactivity > max_keyboard {
            self.send_error(
                client,
                dispatcher,
                zwlr_layer_surface_v1::Error::InvalidKeyboardInteractivity,
                "zwlr_layer_surface_v1.set_keyboard_interactivity used an invalid value",
            );
            return Err(());
        }

        if self.current.desired_width == 0
            && (self.current.anchor & (ANCHOR_LEFT | ANCHOR_RIGHT)) != (ANCHOR_LEFT | ANCHOR_RIGHT)
        {
            self.send_error(
                client,
                dispatcher,
                zwlr_layer_surface_v1::Error::InvalidSize,
                "zwlr_layer_surface_v1 width 0 requires left and right anchors",
            );
            return Err(());
        }

        if self.current.desired_height == 0
            && (self.current.anchor & (ANCHOR_TOP | ANCHOR_BOTTOM)) != (ANCHOR_TOP | ANCHOR_BOTTOM)
        {
            self.send_error(
                client,
                dispatcher,
                zwlr_layer_surface_v1::Error::InvalidSize,
                "zwlr_layer_surface_v1 height 0 requires top and bottom anchors",
            );
            return Err(());
        }

        if let Some(edge) = self.current.exclusive_edge {
            if self.version < zwlr_layer_surface_v1::SET_EXCLUSIVE_EDGE_SINCE {
                self.send_error(
                    client,
                    dispatcher,
                    zwlr_layer_surface_v1::Error::InvalidExclusiveEdge,
                    "zwlr_layer_surface_v1.set_exclusive_edge requires version 5",
                );
                return Err(());
            }
            if edge & !ANCHOR_MASK != 0
                || edge.count_ones() != 1
                || self.current.anchor & edge != edge
            {
                self.send_error(
                    client,
                    dispatcher,
                    zwlr_layer_surface_v1::Error::InvalidExclusiveEdge,
                    "zwlr_layer_surface_v1 exclusive edge must be one of the anchored edges",
                );
                return Err(());
            }
        }

        if changed {
            self.needs_layout_refresh = true;
        }

        Ok(())
    }

    pub fn on_commit(
        &mut self,
        client: &mut Client,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
        surface: &WlSurface,
        has_pending_buffer_attach: bool,
        is_null_buffer_commit: bool,
    ) -> bool {
        if self.closed {
            return true;
        }

        let was_mapped = self.mapped;
        let was_keyboard_interactivity = self.current.keyboard_interactivity;
        let was_layer = self.current.layer;

        if self.apply_pending_state(client, dispatcher).is_err() {
            return false;
        }

        if !self.initial_commit_complete {
            if has_pending_buffer_attach {
                self.send_error(
                    client,
                    dispatcher,
                    zwlr_layer_surface_v1::Error::InvalidSurfaceState,
                    "initial layer-surface commit must not attach a buffer",
                );
                return false;
            }
            self.initial_commit_complete = true;
            self.needs_layout_refresh = true;
            if should_refresh_focus_after_commit(
                was_mapped,
                was_keyboard_interactivity,
                was_layer,
                self,
            ) {
                let layout_snapshot = LayerLayoutObjectSnapshot::for_surface(self.surface, surface)
                    .with_layer_surface(
                        Resolved::new(self.client_id(), self.id, client.object_id(self.id)),
                        self,
                    );
                refresh_keyboard_focus_with_snapshot(state, dispatcher, Some(layout_snapshot));
            }
            return true;
        }

        if has_pending_buffer_attach && self.last_acked.is_none() {
            self.send_error(
                client,
                dispatcher,
                zwlr_layer_surface_v1::Error::InvalidSurfaceState,
                "layer-surface must ack the latest configure before attaching a buffer",
            );
            return false;
        }

        if has_pending_buffer_attach {
            self.mapped = true;
        }

        if is_null_buffer_commit {
            self.dismiss_popup_children(client, state, dispatcher);
            self.reset_after_unmap();
        }

        if should_refresh_focus_after_commit(
            was_mapped,
            was_keyboard_interactivity,
            was_layer,
            self,
        ) {
            let layout_snapshot = LayerLayoutObjectSnapshot::for_surface(self.surface, surface)
                .with_layer_surface(
                    Resolved::new(self.client_id(), self.id, client.object_id(self.id)),
                    self,
                );
            refresh_keyboard_focus_with_snapshot(state, dispatcher, Some(layout_snapshot));
        }

        true
    }

    fn send_configure(&mut self, size: (u32, u32), dispatcher: &mut ClientDispatcher) {
        let serial = self.next_serial();
        self.pending_configures
            .push_back(LayerConfigure { serial, width: size.0, height: size.1 });
        self.last_sent_configure_size = Some(size);
        self.send_event(
            zwlr_layer_surface_v1::Event::Configure { serial, width: size.0, height: size.1 },
            dispatcher,
        );
    }

    pub fn send_closed(&mut self, dispatcher: &mut ClientDispatcher) {
        if self.closed {
            return;
        }
        self.make_inert_after_close();
        self.closed = true;
        self.send_event(zwlr_layer_surface_v1::Event::Closed, dispatcher);
    }

    fn destroy_impl(
        &mut self,
        client: &mut Client,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
    ) {
        let this = client.resolve(self.id, state).unwrap();
        self.dismiss_popup_children(client, state, dispatcher);
        if let Some(mut surface) = self.surface.try_grab(state) {
            surface.force_unmap_for_role_dismissal(client, state, dispatcher);
            surface.clear_layer_surface_role(this);
        }
        state.unregister_layer_surface_object(this);
        self.unregister_from_manager(state);
        client.delete_id(&WlDisplay::default(), state, dispatcher, self.id.into());
        dispatcher.dispatch(crate::clients::ClientManagerAction::LayerShellLayoutChanged);
        refresh_keyboard_focus(state, dispatcher);
    }

    fn apply_layout(
        &mut self,
        client: &mut Client,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
        placement: Option<LayerPlacement>,
    ) {
        let requested_output_missing = self.requested_output_id.is_some()
            && self
                .requested_output_id
                .is_some_and(|output_id| state.output_bounds(output_id).is_none());
        if requested_output_missing {
            self.dismiss_popup_children(client, state, dispatcher);
            if let Some(mut surface) = self.surface.try_grab(state) {
                surface.force_unmap_for_role_dismissal(client, state, dispatcher);
            }
            self.send_closed(dispatcher);
            dispatcher.dispatch(crate::clients::ClientManagerAction::LayerShellLayoutChanged);
            refresh_keyboard_focus(state, dispatcher);
            return;
        }

        let Some(placement) = placement else {
            return;
        };

        if let Some(mut surface) = self.surface.try_grab(state) {
            surface.set_root_position(placement.rect.x, placement.rect.y);
        }

        let should_send_configure = self.initial_commit_complete
            && !self.closed
            && (self.needs_layout_refresh
                || self.last_sent_configure_size != Some(placement.configure_size)
                || self.last_acked.is_none());
        if should_send_configure {
            self.send_configure(placement.configure_size, dispatcher);
            self.needs_layout_refresh = false;
        }
    }

    fn snapshot(
        &self,
        state: &State,
        root_surface: Resolved<WlSurfaceId>,
        root_index: usize,
    ) -> Option<LayerSnapshot> {
        if self.closed {
            return None;
        }
        let output_id = self.resolved_output_id(state)?;
        let actual_size = self
            .surface
            .try_ref(state)
            .and_then(WlSurface::logical_size);
        let (actual_width, actual_height) = actual_size
            .map(|size| (size.width, size.height))
            .unwrap_or((self.current.desired_width.max(0), self.current.desired_height.max(0)));
        Some(LayerSnapshot {
            root_surface,
            root_index,
            output_id,
            mapped: self.mapped,
            layer: self.current.layer,
            desired_width: self.current.desired_width,
            desired_height: self.current.desired_height,
            anchor: self.current.anchor,
            exclusive_zone: self.current.exclusive_zone,
            margin: self.current.margin,
            exclusive_edge: self.current.exclusive_edge,
            actual_width,
            actual_height,
        })
    }
}

fn anchor_has(anchor: u32, edge: u32) -> bool {
    anchor & edge != 0
}

fn resolve_configure_size(snapshot: LayerSnapshot, placement_bounds: RectI) -> (u32, u32) {
    let available_width = placement_bounds
        .width
        .saturating_sub(snapshot.margin.left)
        .saturating_sub(snapshot.margin.right);
    let available_height = placement_bounds
        .height
        .saturating_sub(snapshot.margin.top)
        .saturating_sub(snapshot.margin.bottom);
    let width = if snapshot.desired_width == 0 {
        available_width.max(0)
    } else {
        snapshot.desired_width.max(0)
    } as u32;
    let height = if snapshot.desired_height == 0 {
        available_height.max(0)
    } else {
        snapshot.desired_height.max(0)
    } as u32;
    (width, height)
}

fn anchored_rect(
    placement_bounds: RectI,
    snapshot: LayerSnapshot,
    actual_width: i32,
    actual_height: i32,
) -> RectI {
    let x =
        match (anchor_has(snapshot.anchor, ANCHOR_LEFT), anchor_has(snapshot.anchor, ANCHOR_RIGHT))
        {
            (true, true) => {
                let available_width = placement_bounds
                    .width
                    .saturating_sub(snapshot.margin.left)
                    .saturating_sub(snapshot.margin.right);
                placement_bounds.x
                    + snapshot.margin.left
                    + available_width.saturating_sub(actual_width).div_euclid(2)
            }
            (true, false) => placement_bounds.x + snapshot.margin.left,
            (false, true) => placement_bounds.x2() - snapshot.margin.right - actual_width,
            (false, false) => {
                placement_bounds.x + placement_bounds.width.saturating_sub(actual_width) / 2
            }
        };
    let y =
        match (anchor_has(snapshot.anchor, ANCHOR_TOP), anchor_has(snapshot.anchor, ANCHOR_BOTTOM))
        {
            (true, true) => {
                let available_height = placement_bounds
                    .height
                    .saturating_sub(snapshot.margin.top)
                    .saturating_sub(snapshot.margin.bottom);
                placement_bounds.y
                    + snapshot.margin.top
                    + available_height.saturating_sub(actual_height).div_euclid(2)
            }
            (true, false) => placement_bounds.y + snapshot.margin.top,
            (false, true) => placement_bounds.y2() - snapshot.margin.bottom - actual_height,
            (false, false) => {
                placement_bounds.y + placement_bounds.height.saturating_sub(actual_height) / 2
            }
        };
    RectI {
        x,
        y,
        width: actual_width.max(0),
        height: actual_height.max(0),
    }
}

fn deduce_exclusive_edge(snapshot: LayerSnapshot) -> Option<u32> {
    if let Some(edge) = snapshot.exclusive_edge {
        return Some(edge);
    }

    let top = anchor_has(snapshot.anchor, ANCHOR_TOP);
    let bottom = anchor_has(snapshot.anchor, ANCHOR_BOTTOM);
    let left = anchor_has(snapshot.anchor, ANCHOR_LEFT);
    let right = anchor_has(snapshot.anchor, ANCHOR_RIGHT);

    match (top, bottom, left, right) {
        (true, false, false, false) | (true, false, true, true) => Some(ANCHOR_TOP),
        (false, true, false, false) | (false, true, true, true) => Some(ANCHOR_BOTTOM),
        (false, false, true, false) | (true, true, true, false) => Some(ANCHOR_LEFT),
        (false, false, false, true) | (true, true, false, true) => Some(ANCHOR_RIGHT),
        _ => None,
    }
}

fn effective_exclusive_edge(snapshot: LayerSnapshot) -> Option<u32> {
    (snapshot.exclusive_zone > 0)
        .then(|| deduce_exclusive_edge(snapshot))
        .flatten()
}

fn snapshot_reserves_usable_bounds(snapshot: LayerSnapshot) -> bool {
    snapshot.exclusive_zone > 0 && effective_exclusive_edge(snapshot).is_some()
}

fn apply_exclusive_zone(
    bounds: RectI,
    exclusive_edge: u32,
    zone: i32,
    margin: LayerMargins,
) -> RectI {
    if zone <= 0 {
        return bounds;
    }
    let reserved = match exclusive_edge {
        ANCHOR_TOP => zone.saturating_add(margin.top),
        ANCHOR_BOTTOM => zone.saturating_add(margin.bottom),
        ANCHOR_LEFT => zone.saturating_add(margin.left),
        ANCHOR_RIGHT => zone.saturating_add(margin.right),
        _ => zone,
    };
    match exclusive_edge {
        ANCHOR_TOP => {
            RectI::new(bounds.x, bounds.y + reserved, bounds.width, bounds.height - reserved)
                .unwrap_or(RectI { x: bounds.x, y: bounds.y, width: bounds.width, height: 0 })
        }
        ANCHOR_BOTTOM => RectI::new(bounds.x, bounds.y, bounds.width, bounds.height - reserved)
            .unwrap_or(RectI { x: bounds.x, y: bounds.y, width: bounds.width, height: 0 }),
        ANCHOR_LEFT => {
            RectI::new(bounds.x + reserved, bounds.y, bounds.width - reserved, bounds.height)
                .unwrap_or(RectI { x: bounds.x, y: bounds.y, width: 0, height: bounds.height })
        }
        ANCHOR_RIGHT => RectI::new(bounds.x, bounds.y, bounds.width - reserved, bounds.height)
            .unwrap_or(RectI { x: bounds.x, y: bounds.y, width: 0, height: bounds.height }),
        _ => bounds,
    }
}

fn reserve_usable_bounds_for_snapshot(usable: RectI, snapshot: LayerSnapshot) -> RectI {
    if !snapshot.mapped {
        return usable;
    }
    let Some(exclusive_edge) = effective_exclusive_edge(snapshot) else {
        return usable;
    };
    apply_exclusive_zone(usable, exclusive_edge, snapshot.exclusive_zone, snapshot.margin)
}

fn should_refresh_focus_after_commit(
    was_mapped: bool,
    was_keyboard_interactivity: u32,
    was_layer: LayerShellLayer,
    layer_surface: &ZwlrLayerSurfaceV1,
) -> bool {
    if was_mapped != layer_surface.mapped {
        return true;
    }
    (was_mapped || layer_surface.mapped)
        && (was_keyboard_interactivity != layer_surface.current.keyboard_interactivity
            || was_layer != layer_surface.current.layer)
}

fn keyboard_focus_from_layer_surface(layer_surface: &ZwlrLayerSurfaceV1) -> LayerKeyboardFocus {
    match layer_surface.keyboard_interactivity() {
        value if value == zwlr_layer_surface_v1::KeyboardInteractivity::Exclusive as u32 => {
            LayerKeyboardFocus::Exclusive { layer: layer_surface.layer() }
        }
        value if value == zwlr_layer_surface_v1::KeyboardInteractivity::OnDemand as u32 => {
            LayerKeyboardFocus::OnDemand { layer: layer_surface.layer() }
        }
        _ => LayerKeyboardFocus::None,
    }
}

fn popup_parent_layer_surface_keyboard_focus(
    state: &State,
    surface: &WlSurface,
) -> Option<LayerKeyboardFocus> {
    let parent_layer_surface_id = surface.popup_parent_layer_surface()?;
    let parent_layer_surface = state.layer_surface(parent_layer_surface_id)?;
    let parent_layer_surface = state
        .objects
        .try_ref_object::<ZwlrLayerSurfaceV1>(parent_layer_surface.object_id())?;
    Some(keyboard_focus_from_layer_surface(parent_layer_surface))
}

#[derive(Debug)]
struct OutputLayerLayout {
    placements: BTreeMap<Resolved<WlSurfaceId>, LayerPlacement>,
    usable: RectI,
    background_roots: Vec<Resolved<WlSurfaceId>>,
    bottom_roots: Vec<Resolved<WlSurfaceId>>,
    top_roots: Vec<Resolved<WlSurfaceId>>,
    overlay_roots: Vec<Resolved<WlSurfaceId>>,
}

fn compute_output_layout(
    full_bounds: RectI,
    mut snapshots: Vec<LayerSnapshot>,
) -> OutputLayerLayout {
    snapshots.sort_by_key(|snapshot| {
        (snapshot.layer, !snapshot_reserves_usable_bounds(*snapshot), snapshot.root_index)
    });
    let mut usable = full_bounds;
    let mut output_layout = OutputLayerLayout {
        placements: BTreeMap::new(),
        usable,
        background_roots: Vec::new(),
        bottom_roots: Vec::new(),
        top_roots: Vec::new(),
        overlay_roots: Vec::new(),
    };

    for snapshot in snapshots {
        let placement_bounds = if snapshot.exclusive_zone == -1 { full_bounds } else { usable };
        let configure_size = resolve_configure_size(snapshot, placement_bounds);
        let actual_width =
            if snapshot.actual_width > 0 { snapshot.actual_width } else { configure_size.0 as i32 };
        let actual_height = if snapshot.actual_height > 0 {
            snapshot.actual_height
        } else {
            configure_size.1 as i32
        };
        let rect = anchored_rect(placement_bounds, snapshot, actual_width, actual_height);
        output_layout.placements.insert(
            snapshot.root_surface,
            LayerPlacement {
                output_id: snapshot.output_id,
                rect,
                configure_size,
                layer: snapshot.layer,
            },
        );
        if snapshot.mapped {
            match snapshot.layer {
                LayerShellLayer::Background => {
                    output_layout.background_roots.push(snapshot.root_surface)
                }
                LayerShellLayer::Bottom => output_layout.bottom_roots.push(snapshot.root_surface),
                LayerShellLayer::Top => output_layout.top_roots.push(snapshot.root_surface),
                LayerShellLayer::Overlay => output_layout.overlay_roots.push(snapshot.root_surface),
            }
        }
        usable = reserve_usable_bounds_for_snapshot(usable, snapshot);
    }

    output_layout.usable = usable;
    output_layout
}

pub(crate) fn lookup_snapshot_surface<'a>(
    state: &'a State,
    snapshot: Option<LayerLayoutObjectSnapshot<'a>>,
    root_surface: Resolved<WlSurfaceId>,
) -> Option<&'a WlSurface> {
    snapshot
        .filter(|snapshot| snapshot.root_surface == root_surface)
        .map(|snapshot| snapshot.surface)
        .or_else(|| root_surface.try_ref(state))
}

fn snapshot_layer_surface<'a>(
    state: &'a State,
    snapshot: Option<LayerLayoutObjectSnapshot<'a>>,
    layer_surface: Resolved<ZwlrLayerSurfaceV1Id>,
) -> Option<&'a ZwlrLayerSurfaceV1> {
    snapshot
        .filter(|snapshot| {
            snapshot
                .resolved_layer_surface
                .is_some_and(|resolved| resolved == layer_surface)
        })
        .and_then(|snapshot| snapshot.layer_surface)
        .or_else(|| layer_surface.try_ref(state))
}

fn compute_layer_layout(
    state: &State,
    snapshot: Option<LayerLayoutObjectSnapshot<'_>>,
) -> ComputedLayerLayout {
    let live_root_ids = state.root_surfaces();
    let root_order = live_root_ids
        .iter()
        .enumerate()
        .map(|(index, surface)| (*surface, index))
        .collect::<BTreeMap<_, _>>();
    let mut snapshots_by_output = BTreeMap::<OutputId, Vec<LayerSnapshot>>::new();

    for root_surface in live_root_ids.iter().copied() {
        let Some(surface) = lookup_snapshot_surface(state, snapshot, root_surface) else {
            continue;
        };
        let Some(layer_surface_id) = surface.layer_surface() else {
            continue;
        };
        let Some(layer_surface) = state.layer_surface(layer_surface_id) else {
            continue;
        };
        let Some(layer_surface_ref) = snapshot_layer_surface(state, snapshot, layer_surface) else {
            continue;
        };
        let Some(&root_index) = root_order.get(&root_surface) else {
            continue;
        };
        let Some(snapshot) = layer_surface_ref.snapshot(state, root_surface, root_index) else {
            continue;
        };
        snapshots_by_output
            .entry(snapshot.output_id)
            .or_default()
            .push(snapshot);
    }

    let mut computed = ComputedLayerLayout::default();
    for (output_id, snapshots) in snapshots_by_output {
        let Some(full_bounds) = state.output_bounds(output_id) else {
            continue;
        };
        let output_layout = compute_output_layout(full_bounds, snapshots);
        computed.placements.extend(output_layout.placements);
        computed
            .background_roots
            .extend(output_layout.background_roots);
        computed.bottom_roots.extend(output_layout.bottom_roots);
        computed.top_roots.extend(output_layout.top_roots);
        computed.overlay_roots.extend(output_layout.overlay_roots);
        computed
            .usable_by_output
            .insert(output_id, output_layout.usable);
    }

    computed
}

pub fn output_usable_bounds(state: &State, output_id: OutputId) -> Option<RectI> {
    output_usable_bounds_with_snapshot(state, output_id, None)
}

pub fn output_usable_bounds_with_snapshot(
    state: &State,
    output_id: OutputId,
    snapshot: Option<LayerLayoutObjectSnapshot<'_>>,
) -> Option<RectI> {
    let layout = compute_layer_layout(state, snapshot);
    layout
        .usable_by_output
        .get(&output_id)
        .copied()
        .or_else(|| state.output_bounds(output_id))
}

pub fn ordered_scene_root_surfaces(state: &State) -> Vec<Resolved<WlSurfaceId>> {
    ordered_scene_root_surfaces_with_snapshot(state, None)
}

pub fn ordered_scene_root_surfaces_with_snapshot(
    state: &State,
    snapshot: Option<LayerLayoutObjectSnapshot<'_>>,
) -> Vec<Resolved<WlSurfaceId>> {
    let layout = compute_layer_layout(state, snapshot);
    let mut ordered = Vec::new();
    ordered.extend(layout.background_roots);
    ordered.extend(layout.bottom_roots);
    let managed_roots = state
        .tiling_scene_root_surfaces()
        .into_iter()
        .collect::<Vec<_>>();
    let managed_set = managed_roots
        .iter()
        .copied()
        .collect::<std::collections::BTreeSet<_>>();
    for root_surface in managed_roots.into_iter().chain(
        state
            .root_surfaces()
            .into_iter()
            .filter(|surface| !managed_set.contains(surface)),
    ) {
        let Some(surface) = lookup_snapshot_surface(state, snapshot, root_surface) else {
            continue;
        };
        if surface.role() == crate::wayland_objects::wl_surface::Role::LayerSurface {
            continue;
        }
        if !state.root_surface_is_visible(root_surface) {
            continue;
        }
        ordered.push(root_surface);
    }
    ordered.extend(layout.top_roots);
    ordered.extend(layout.overlay_roots);
    ordered
}

pub fn layer_surface_keyboard_focus(
    state: &State,
    root_surface: Resolved<WlSurfaceId>,
) -> Option<LayerKeyboardFocus> {
    layer_surface_keyboard_focus_with_snapshot(state, root_surface, None)
}

pub fn layer_surface_keyboard_focus_with_snapshot(
    state: &State,
    root_surface: Resolved<WlSurfaceId>,
    snapshot: Option<LayerLayoutObjectSnapshot<'_>>,
) -> Option<LayerKeyboardFocus> {
    let surface = lookup_snapshot_surface(state, snapshot, root_surface)?;
    let Some(layer_surface_id) = surface.layer_surface() else {
        return popup_parent_layer_surface_keyboard_focus(state, surface);
    };
    let Some(layer_surface) = state.layer_surface(layer_surface_id) else {
        return Some(LayerKeyboardFocus::None);
    };
    let Some(layer_surface) = snapshot_layer_surface(state, snapshot, layer_surface) else {
        return Some(LayerKeyboardFocus::None);
    };
    Some(keyboard_focus_from_layer_surface(layer_surface))
}

pub fn relayout_client_layer_surfaces(
    client: &mut Client,
    state: &mut State,
    dispatcher: &mut ClientDispatcher,
) {
    let layer_surfaces = state
        .layer_surface_objects
        .iter()
        .copied()
        .filter_map(|layer_id| {
            let layer = layer_id.try_ref(state)?;
            (layer.client_id() == client.client_id()).then_some(layer_id)
        })
        .collect::<Vec<_>>();

    for layer_surface in layer_surfaces.iter().copied() {
        let mut layer_surface = layer_surface.grab(state);
        layer_surface.ensure_output_assignment(state);
    }

    let layout = compute_layer_layout(state, None);

    for layer_surface in layer_surfaces {
        let mut layer_surface = layer_surface.grab(state);
        let root_surface = layer_surface.surface();
        let placement = layout.placements.get(&root_surface).copied();
        layer_surface.apply_layout(client, state, dispatcher, placement);
    }
}

impl Object for ZwlrLayerSurfaceV1 {
    fn id(&self) -> WaylandId {
        self.id.into()
    }

    fn on(
        mut self: GrabbedObject<Self>,
        client: &mut Client,
        state: &mut State,
        req: RawRequest,
        dispatcher: &mut ClientDispatcher,
    ) -> RequestResult {
        use zwlr_layer_surface_v1::Request::*;

        let Some(request) = self.parse_request(client, req, dispatcher) else {
            return Ok(());
        };

        if self.closed {
            if matches!(request, Destroy) {
                self.destroy_impl(client, state, dispatcher);
            }
            return Ok(());
        }

        match request {
            SetSize { width, height } => {
                self.pending.desired_width = Some(width.min(i32::MAX as u32) as i32);
                self.pending.desired_height = Some(height.min(i32::MAX as u32) as i32);
            }
            SetAnchor { anchor } => {
                if anchor & !ANCHOR_MASK != 0 {
                    self.send_error(
                        client,
                        dispatcher,
                        zwlr_layer_surface_v1::Error::InvalidAnchor,
                        "zwlr_layer_surface_v1.set_anchor contained unknown bits",
                    );
                    return Ok(());
                }
                self.pending.anchor = Some(anchor);
            }
            SetExclusiveZone { zone } => {
                self.pending.exclusive_zone = Some(zone);
            }
            SetMargin { top, right, bottom, left } => {
                self.pending.margin = Some(LayerMargins { top, right, bottom, left });
            }
            SetKeyboardInteractivity { keyboard_interactivity } => {
                if keyboard_interactivity > max_keyboard_interactivity_for_version(self.version) {
                    self.send_error(
                        client,
                        dispatcher,
                        zwlr_layer_surface_v1::Error::InvalidKeyboardInteractivity,
                        "zwlr_layer_surface_v1.set_keyboard_interactivity used an invalid value",
                    );
                    return Ok(());
                }
                self.pending.keyboard_interactivity = Some(keyboard_interactivity);
            }
            GetPopup { popup } => {
                let popup = client.resolve_or_disconnect(popup, state, dispatcher)?;
                let mut popup_object = popup.grab(state);
                let this = client.resolve(self.id, state).unwrap();
                if let Err(err) = popup_object.assign_layer_parent(this) {
                    let message = match err {
                        AssignLayerParentError::AlreadyHasParent => {
                            "zwlr_layer_surface_v1.get_popup requires an unparented xdg_popup"
                        }
                        AssignLayerParentError::AlreadyMapped => {
                            "zwlr_layer_surface_v1.get_popup must happen before the popup is mapped"
                        }
                    };
                    self.send_error(
                        client,
                        dispatcher,
                        zwlr_layer_surface_v1::Error::InvalidSurfaceState,
                        message,
                    );
                    return Ok(());
                }
                let popup_surface_id = {
                    let xdg_surface = popup_object.xdg_surface().rref(state);
                    xdg_surface.wl_surface()
                };
                let mut popup_surface = popup_surface_id.ref_mut(state);
                popup_surface.set_popup_parent_layer_surface(Some(this));
                self.add_popup_child(popup);
            }
            AckConfigure { serial } => {
                let Some(position) = self
                    .pending_configures
                    .iter()
                    .position(|configure| configure.serial == serial)
                else {
                    self.send_error(
                        client,
                        dispatcher,
                        zwlr_layer_surface_v1::Error::InvalidSurfaceState,
                        "zwlr_layer_surface_v1.ack_configure referenced an unknown serial",
                    );
                    return Ok(());
                };
                let acked = self.pending_configures[position];
                self.pending_configures.drain(..=position);
                self.last_acked = Some(acked);
            }
            SetLayer { layer } => {
                if self.version < zwlr_layer_surface_v1::SET_LAYER_SINCE {
                    client.protocol_violation_disconnect(
                        dispatcher,
                        self.id.into(),
                        wl_display::Error::InvalidMethod as u32,
                        "zwlr_layer_surface_v1.set_layer requires version 2",
                    );
                    return Ok(());
                }
                let Some(layer) = LayerShellLayer::from_raw(layer) else {
                    client.send_protocol_error(
                        dispatcher,
                        self.id.into(),
                        zwlr_layer_shell_v1::Error::InvalidLayer as u32,
                        "zwlr_layer_surface_v1.set_layer used an invalid layer value",
                    );
                    return Ok(());
                };
                self.pending.layer = Some(layer);
            }
            SetExclusiveEdge { edge } => {
                if self.version < zwlr_layer_surface_v1::SET_EXCLUSIVE_EDGE_SINCE {
                    client.protocol_violation_disconnect(
                        dispatcher,
                        self.id.into(),
                        wl_display::Error::InvalidMethod as u32,
                        "zwlr_layer_surface_v1.set_exclusive_edge requires version 5",
                    );
                    return Ok(());
                }
                if edge == 0 {
                    self.pending.exclusive_edge = Some(None);
                    return Ok(());
                }
                if edge & !ANCHOR_MASK != 0 || edge.count_ones() != 1 {
                    self.send_error(
                        client,
                        dispatcher,
                        zwlr_layer_surface_v1::Error::InvalidExclusiveEdge,
                        "zwlr_layer_surface_v1.set_exclusive_edge requires exactly one edge or 0",
                    );
                    return Ok(());
                }
                self.pending.exclusive_edge = Some(Some(edge));
            }
            Destroy => {
                self.destroy_impl(client, state, dispatcher);
            }
        }
        Ok(())
    }

    fn on_client_disconnect(
        &mut self,
        client: &mut Client,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
    ) {
        let this = client.resolve(self.id, state).unwrap();
        self.dismiss_popup_children(client, state, dispatcher);
        if let Some(mut surface) = self.surface.try_ref_mut(state) {
            surface.clear_layer_surface_role(this);
        }
        state.unregister_layer_surface_object(this);
        self.unregister_from_manager(state);
        dispatcher.dispatch(crate::clients::ClientManagerAction::LayerShellLayoutChanged);
        refresh_keyboard_focus(state, dispatcher);
    }
}

#[cfg(test)]
mod tests {
    use {
        super::*,
        crate::{
            clients::{Client, ClientId, ClientManagerAction, RawEvent},
            io_uring::model::IoUringAction,
            runner::Dispatcher,
            state::{GrabbedObject, State},
            test_support::{
                state_for_tests, test_output_drm1_crtc, test_resolved_wl_surface_client1,
            },
            wayland_objects::wl_display::WlDisplay,
        },
    };

    fn set_layer_request(
        layer_surface_id: ZwlrLayerSurfaceV1Id,
        layer: zwlr_layer_shell_v1::Layer,
    ) -> crate::clients::RawRequest<'static> {
        let mut event = RawEvent::new(layer_surface_id.into(), 8, 1);
        event.uint(layer as u32);
        (&*event.into_bytes()).try_into().unwrap()
    }

    fn set_anchor_request(
        layer_surface_id: ZwlrLayerSurfaceV1Id,
        anchor: u32,
    ) -> crate::clients::RawRequest<'static> {
        let mut event = RawEvent::new(layer_surface_id.into(), 1, 1);
        event.uint(anchor);
        (&*event.into_bytes()).try_into().unwrap()
    }

    fn set_keyboard_interactivity_request(
        layer_surface_id: ZwlrLayerSurfaceV1Id,
        keyboard_interactivity: u32,
    ) -> crate::clients::RawRequest<'static> {
        let mut event = RawEvent::new(layer_surface_id.into(), 4, 1);
        event.uint(keyboard_interactivity);
        (&*event.into_bytes()).try_into().unwrap()
    }

    fn fake_layer_manager(raw: usize) -> Resolved<ZwlrLayerShellV1Id> {
        Resolved::new(
            ClientId::from_raw(3),
            ZwlrLayerShellV1Id::from(raw as u32),
            ObjectId::from(raw),
        )
    }

    fn fake_layer_surface_handle(surface_id: u32, object_raw: usize) -> Resolved<WlSurfaceId> {
        Resolved::new(
            ClientId::from_raw(3),
            WlSurfaceId::from(surface_id),
            ObjectId::from(object_raw),
        )
    }

    fn make_test_layer_surface(
        id: ZwlrLayerSurfaceV1Id,
        version: u32,
        requested_output_id: Option<OutputId>,
        layer: LayerShellLayer,
        namespace: &str,
    ) -> ZwlrLayerSurfaceV1 {
        ZwlrLayerSurfaceV1::new(
            id,
            fake_layer_manager(2),
            version,
            fake_layer_surface_handle(4, 5),
            requested_output_id,
            layer,
            namespace.to_string(),
        )
    }

    fn snapshot_with_zone(zone: i32, margin: LayerMargins) -> LayerSnapshot {
        LayerSnapshot {
            root_surface: test_resolved_wl_surface_client1(1),
            root_index: 0,
            output_id: OutputId::from(0usize),
            mapped: true,
            layer: LayerShellLayer::Top,
            desired_width: 0,
            desired_height: 0,
            anchor: ANCHOR_TOP,
            exclusive_zone: zone,
            margin,
            exclusive_edge: None,
            actual_width: 10,
            actual_height: 10,
        }
    }

    fn top_panel_snapshot(
        surface_object_id: usize,
        root_index: usize,
        exclusive_zone: i32,
    ) -> LayerSnapshot {
        LayerSnapshot {
            root_surface: test_resolved_wl_surface_client1(surface_object_id),
            root_index,
            output_id: OutputId::from(0usize),
            mapped: true,
            layer: LayerShellLayer::Top,
            desired_width: 0,
            desired_height: 10,
            anchor: ANCHOR_TOP | ANCHOR_LEFT | ANCHOR_RIGHT,
            exclusive_zone,
            margin: LayerMargins::default(),
            exclusive_edge: None,
            actual_width: 100,
            actual_height: 10,
        }
    }

    #[test]
    fn exclusive_zone_shrinks_usable_bounds_by_zone_and_margin() {
        let bounds = RectI { x: 0, y: 0, width: 200, height: 100 };
        let updated = apply_exclusive_zone(
            bounds,
            ANCHOR_TOP,
            20,
            LayerMargins { top: 5, right: 0, bottom: 0, left: 0 },
        );

        assert_eq!(updated, RectI { x: 0, y: 25, width: 200, height: 75 });
    }

    #[test]
    fn unmapped_positive_exclusive_zone_does_not_shrink_usable_bounds() {
        let bounds = RectI { x: 0, y: 0, width: 200, height: 100 };
        let snapshot = LayerSnapshot {
            mapped: false,
            ..snapshot_with_zone(20, LayerMargins::default())
        };

        let updated = reserve_usable_bounds_for_snapshot(bounds, snapshot);

        assert_eq!(updated, bounds);
    }

    #[test]
    fn mapped_positive_exclusive_zone_shrinks_usable_bounds() {
        let bounds = RectI { x: 0, y: 0, width: 200, height: 100 };
        let snapshot =
            snapshot_with_zone(20, LayerMargins { top: 5, right: 0, bottom: 0, left: 0 });

        let updated = reserve_usable_bounds_for_snapshot(bounds, snapshot);

        assert_eq!(updated, RectI { x: 0, y: 25, width: 200, height: 75 });
    }

    #[test]
    fn positive_exclusive_surfaces_stack_inside_remaining_usable_bounds() {
        let output_layout = compute_output_layout(
            RectI { x: 0, y: 0, width: 100, height: 100 },
            vec![top_panel_snapshot(1, 0, 10), top_panel_snapshot(2, 1, 10)],
        );

        assert_eq!(
            output_layout.placements[&test_resolved_wl_surface_client1(1)]
                .rect
                .y,
            0
        );
        assert_eq!(
            output_layout.placements[&test_resolved_wl_surface_client1(2)]
                .rect
                .y,
            10
        );
        assert_eq!(output_layout.usable, RectI { x: 0, y: 20, width: 100, height: 80 });
    }

    #[test]
    fn non_exclusive_surfaces_are_arranged_after_exclusive_ones() {
        let output_layout = compute_output_layout(
            RectI { x: 0, y: 0, width: 100, height: 100 },
            vec![top_panel_snapshot(1, 0, 0), top_panel_snapshot(2, 1, 10)],
        );

        assert_eq!(
            output_layout.placements[&test_resolved_wl_surface_client1(2)]
                .rect
                .y,
            0
        );
        assert_eq!(
            output_layout.placements[&test_resolved_wl_surface_client1(1)]
                .rect
                .y,
            10
        );
        assert_eq!(output_layout.usable, RectI { x: 0, y: 10, width: 100, height: 90 });
    }

    #[test]
    fn non_positive_exclusive_zones_do_not_reserve_an_edge() {
        assert_eq!(effective_exclusive_edge(snapshot_with_zone(0, LayerMargins::default())), None);
        assert_eq!(effective_exclusive_edge(snapshot_with_zone(-1, LayerMargins::default())), None);
    }

    #[test]
    fn reset_after_unmap_keeps_pending_configures() {
        let mut layer_surface = make_test_layer_surface(
            ZwlrLayerSurfaceV1Id::from(1u32),
            5,
            None,
            LayerShellLayer::Top,
            "panel",
        );
        layer_surface.pending_configures.push_back(LayerConfigure {
            serial: 7,
            width: 800,
            height: 30,
        });
        layer_surface.last_acked = Some(LayerConfigure { serial: 7, width: 800, height: 30 });
        layer_surface.initial_commit_complete = true;
        layer_surface.mapped = true;

        layer_surface.reset_after_unmap();

        assert_eq!(layer_surface.pending_configures.len(), 1);
        assert_eq!(layer_surface.pending_configures[0].serial, 7);
        assert!(!layer_surface.mapped);
        assert!(layer_surface.last_acked.is_none());
        assert!(!layer_surface.initial_commit_complete);
    }

    #[test]
    fn send_closed_makes_surface_inert() {
        let mut layer_surface = make_test_layer_surface(
            ZwlrLayerSurfaceV1Id::from(1u32),
            5,
            Some(OutputId::from(6usize)),
            LayerShellLayer::Overlay,
            "lock",
        );
        layer_surface.pending_configures.push_back(LayerConfigure {
            serial: 11,
            width: 1920,
            height: 1080,
        });
        layer_surface.initial_commit_complete = true;
        layer_surface.mapped = true;
        layer_surface.current.keyboard_interactivity =
            zwlr_layer_surface_v1::KeyboardInteractivity::Exclusive as u32;
        let mut dispatcher = crate::runner::Dispatcher::default();
        let mut client = crate::clients::Client::new_for_tests(ClientId::from_raw(9));
        let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);

        layer_surface.send_closed(&mut client_dispatcher);

        assert!(layer_surface.closed);
        assert!(!layer_surface.mapped);
        assert!(layer_surface.pending_configures.is_empty());
        assert_eq!(
            layer_surface.keyboard_interactivity(),
            zwlr_layer_surface_v1::KeyboardInteractivity::None as u32
        );
    }

    #[test]
    fn requested_output_does_not_fallback_to_active_output_when_missing() {
        let mut state = state_for_tests();
        let _active_output = state.insert_output(test_output_drm1_crtc(1, 0, 0, 1920, 1080));
        let missing_output = OutputId::from(99usize);
        let layer_surface = make_test_layer_surface(
            ZwlrLayerSurfaceV1Id::from(1u32),
            5,
            Some(missing_output),
            LayerShellLayer::Top,
            "panel",
        );

        assert_eq!(layer_surface.resolved_output_id(&state), None);
    }

    #[test]
    fn focus_refresh_is_not_needed_for_unmapped_pending_state_changes() {
        let layer_surface = make_test_layer_surface(
            ZwlrLayerSurfaceV1Id::from(1u32),
            5,
            None,
            LayerShellLayer::Bottom,
            "panel",
        );

        let needs_refresh = should_refresh_focus_after_commit(
            false,
            zwlr_layer_surface_v1::KeyboardInteractivity::None as u32,
            LayerShellLayer::Bottom,
            &layer_surface,
        );

        assert!(!needs_refresh);
    }

    #[test]
    fn focus_refresh_is_needed_when_layer_surface_maps() {
        let mut layer_surface = make_test_layer_surface(
            ZwlrLayerSurfaceV1Id::from(1u32),
            5,
            None,
            LayerShellLayer::Bottom,
            "panel",
        );
        layer_surface.mapped = true;

        let needs_refresh = should_refresh_focus_after_commit(
            false,
            zwlr_layer_surface_v1::KeyboardInteractivity::None as u32,
            LayerShellLayer::Bottom,
            &layer_surface,
        );

        assert!(needs_refresh);
    }

    #[test]
    fn focus_refresh_is_needed_for_mapped_interactivity_changes() {
        let mut layer_surface = make_test_layer_surface(
            ZwlrLayerSurfaceV1Id::from(1u32),
            5,
            None,
            LayerShellLayer::Bottom,
            "panel",
        );
        layer_surface.mapped = true;
        layer_surface.current.keyboard_interactivity =
            zwlr_layer_surface_v1::KeyboardInteractivity::Exclusive as u32;

        let needs_refresh = should_refresh_focus_after_commit(
            true,
            zwlr_layer_surface_v1::KeyboardInteractivity::None as u32,
            LayerShellLayer::Bottom,
            &layer_surface,
        );

        assert!(needs_refresh);
    }

    #[test]
    fn focus_refresh_is_needed_for_mapped_layer_changes() {
        let mut layer_surface = make_test_layer_surface(
            ZwlrLayerSurfaceV1Id::from(1u32),
            5,
            None,
            LayerShellLayer::Bottom,
            "panel",
        );
        layer_surface.mapped = true;
        layer_surface.current.layer = LayerShellLayer::Overlay;

        let needs_refresh = should_refresh_focus_after_commit(
            true,
            zwlr_layer_surface_v1::KeyboardInteractivity::Exclusive as u32,
            LayerShellLayer::Bottom,
            &layer_surface,
        );

        assert!(needs_refresh);
    }

    #[test]
    fn set_layer_reports_error_for_version_one_objects() {
        let mut state = state_for_tests();
        let layer_surface_id = ZwlrLayerSurfaceV1Id::from(1u32);
        let mut client = Client::new_for_tests(ClientId::from_raw(3));
        let mut dispatcher = Dispatcher::default();
        let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);

        client.insert_object(
            &mut state,
            make_test_layer_surface(layer_surface_id, 1, None, LayerShellLayer::Bottom, "panel"),
        );
        client.grab_object(layer_surface_id, &mut state).on(
            &mut client,
            &mut state,
            set_layer_request(layer_surface_id, zwlr_layer_shell_v1::Layer::Overlay),
            &mut client_dispatcher,
        );
        drop(client_dispatcher);

        let layer_surface = state
            .objects
            .get(client.object_id(layer_surface_id))
            .downcast_ref::<ZwlrLayerSurfaceV1>()
            .unwrap();
        assert_eq!(layer_surface.pending.layer, None);
        assert_eq!(layer_surface.layer(), LayerShellLayer::Bottom);
        assert!(dispatcher.test_has_queued_action::<crate::clients::ClientManagerAction>(|action| {
            matches!(action, crate::clients::ClientManagerAction::Close { id } if *id == client.client_id())
        }));
        assert!(dispatcher.test_has_queued_action::<IoUringAction>(|action| matches!(
            action,
            IoUringAction::Sendmsg { .. }
        )));
    }

    #[test]
    fn set_anchor_rejects_invalid_bits_immediately() {
        let mut state = state_for_tests();
        let layer_surface_id = ZwlrLayerSurfaceV1Id::from(1u32);
        let mut client = Client::new_for_tests(ClientId::from_raw(3));
        let mut dispatcher = Dispatcher::default();
        let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);

        client.insert_object(
            &mut state,
            make_test_layer_surface(layer_surface_id, 5, None, LayerShellLayer::Bottom, "panel"),
        );
        client.grab_object(layer_surface_id, &mut state).on(
            &mut client,
            &mut state,
            set_anchor_request(layer_surface_id, ANCHOR_MASK | 0x10),
            &mut client_dispatcher,
        );
        drop(client_dispatcher);

        let layer_surface = state
            .objects
            .get(client.object_id(layer_surface_id))
            .downcast_ref::<ZwlrLayerSurfaceV1>()
            .unwrap();
        assert_eq!(layer_surface.pending.anchor, None);
        assert!(!dispatcher.test_has_queued_action::<crate::clients::ClientManagerAction>(|action| {
            matches!(action, crate::clients::ClientManagerAction::Close { id } if *id == client.client_id())
        }));
        assert!(dispatcher.test_has_queued_action::<IoUringAction>(|action| matches!(
            action,
            IoUringAction::Sendmsg { .. }
        )));
    }

    #[test]
    fn set_keyboard_interactivity_rejects_invalid_values_immediately() {
        let mut state = state_for_tests();
        let layer_surface_id = ZwlrLayerSurfaceV1Id::from(1u32);
        let mut client = Client::new_for_tests(ClientId::from_raw(3));
        let mut dispatcher = Dispatcher::default();
        let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);

        client.insert_object(
            &mut state,
            make_test_layer_surface(layer_surface_id, 5, None, LayerShellLayer::Bottom, "panel"),
        );
        client.grab_object(layer_surface_id, &mut state).on(
            &mut client,
            &mut state,
            set_keyboard_interactivity_request(layer_surface_id, 99),
            &mut client_dispatcher,
        );
        drop(client_dispatcher);

        let layer_surface = state
            .objects
            .get(client.object_id(layer_surface_id))
            .downcast_ref::<ZwlrLayerSurfaceV1>()
            .unwrap();
        assert_eq!(layer_surface.pending.keyboard_interactivity, None);
        assert!(!dispatcher.test_has_queued_action::<crate::clients::ClientManagerAction>(|action| {
            matches!(action, crate::clients::ClientManagerAction::Close { id } if *id == client.client_id())
        }));
        assert!(dispatcher.test_has_queued_action::<IoUringAction>(|action| matches!(
            action,
            IoUringAction::Sendmsg { .. }
        )));
    }

    fn set_exclusive_edge_request(
        layer_surface_id: ZwlrLayerSurfaceV1Id,
        edge: u32,
    ) -> crate::clients::RawRequest<'static> {
        let mut event = RawEvent::new(layer_surface_id.into(), 9, 1);
        event.uint(edge);
        (&*event.into_bytes()).try_into().unwrap()
    }

    fn malformed_request(
        layer_surface_id: ZwlrLayerSurfaceV1Id,
    ) -> crate::clients::RawRequest<'static> {
        let event = RawEvent::new(layer_surface_id.into(), 99, 0);
        (&*event.into_bytes()).try_into().unwrap()
    }

    #[test]
    fn set_exclusive_edge_reports_invalid_method_before_version_five() {
        let mut state = state_for_tests();
        let layer_surface_id = ZwlrLayerSurfaceV1Id::from(1u32);
        let mut client = Client::new_for_tests(ClientId::from_raw(3));
        let mut dispatcher = Dispatcher::default();
        let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);

        client.insert_object(
            &mut state,
            make_test_layer_surface(layer_surface_id, 4, None, LayerShellLayer::Bottom, "panel"),
        );
        client.grab_object(layer_surface_id, &mut state).on(
            &mut client,
            &mut state,
            set_exclusive_edge_request(layer_surface_id, ANCHOR_TOP),
            &mut client_dispatcher,
        );
        drop(client_dispatcher);

        let layer_surface = state
            .objects
            .get(client.object_id(layer_surface_id))
            .downcast_ref::<ZwlrLayerSurfaceV1>()
            .unwrap();
        assert_eq!(layer_surface.pending.exclusive_edge, None);
        assert!(dispatcher.test_has_queued_action::<crate::clients::ClientManagerAction>(|action| {
            matches!(action, crate::clients::ClientManagerAction::Close { id } if *id == client.client_id())
        }));
        assert!(dispatcher.test_has_queued_action::<IoUringAction>(|action| matches!(
            action,
            IoUringAction::Sendmsg { .. }
        )));
    }

    #[test]
    fn destroy_ignores_missing_manager_binding() {
        let mut state = state_for_tests();
        let mut client = Client::new_for_tests(ClientId::from_raw(3));
        let layer_surface_id = ZwlrLayerSurfaceV1Id::from(1u32);
        client.insert_object(
            &mut state,
            make_test_layer_surface(layer_surface_id, 5, None, LayerShellLayer::Top, "panel"),
        );
        let mut dispatcher = Dispatcher::default();
        let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);

        {
            let mut layer_surface = client.grab_object(layer_surface_id, &mut state);
            layer_surface.destroy_impl(&mut client, &mut state, &mut client_dispatcher);
        }
        drop(client_dispatcher);

        assert!(!client.contains_object(layer_surface_id, &state));
        assert!(dispatcher.test_has_queued_action::<ClientManagerAction>(|action| {
            matches!(action, ClientManagerAction::LayerShellLayoutChanged)
        }));
    }

    #[test]
    fn malformed_requests_disconnect_the_client() {
        let mut state = state_for_tests();
        let mut client = Client::new_for_tests(ClientId::from_raw(3));
        let layer_surface_id = ZwlrLayerSurfaceV1Id::from(1u32);
        client.insert_object(
            &mut state,
            make_test_layer_surface(layer_surface_id, 5, None, LayerShellLayer::Top, "panel"),
        );
        let mut dispatcher = Dispatcher::default();
        let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);

        {
            let mut layer_surface = client.grab_object(layer_surface_id, &mut state);
            layer_surface.on(
                &mut client,
                &mut state,
                malformed_request(layer_surface_id),
                &mut client_dispatcher,
            );
        }
        drop(client_dispatcher);

        assert!(dispatcher.test_has_queued_action::<ClientManagerAction>(|action| {
            matches!(action, ClientManagerAction::Close { id } if *id == client.client_id())
        }));
        assert!(dispatcher.test_has_queued_action::<IoUringAction>(|action| matches!(
            action,
            IoUringAction::Sendmsg { .. }
        )));
    }
}
