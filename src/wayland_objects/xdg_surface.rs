use std::collections::{BTreeSet, VecDeque};

use crate::{
    clients::{Client, ClientDispatcher, EventWriter, RawRequest, Resolved},
    object::{Object, Parser, RequestResult},
    protocol_generated::{
        xdg_surface, xdg_wm_base, WaylandId, WlSurfaceId, XdgPopupId, XdgSurfaceId, XdgToplevelId,
        XdgWmBaseId,
    },
    region::RectI,
    state::{GrabbedObject, ObjectId, State},
    transform::SizeI,
    wayland_objects::{
        wl_display::WlDisplay, wl_surface::WlSurface, xdg_popup::XdgPopup,
        xdg_positioner::XdgPositioner, xdg_wm_base::XdgWmBase,
    },
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RoleKind {
    Toplevel,
    Popup,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RoleObject {
    None,
    Toplevel(Resolved<XdgToplevelId>),
    Popup(Resolved<XdgPopupId>),
}

#[derive(Clone, Debug)]
enum ConfigureState {
    Toplevel(ToplevelConfigureState),
    Popup(PopupConfigureState),
}

#[derive(Clone, Debug)]
struct ToplevelConfigureState {
    width: i32,
    height: i32,
    states: Vec<u32>,
    decoration_mode: xdg_toplevel::ToplevelDecorationMode,
}

#[derive(Clone, Debug)]
struct PopupConfigureState {
    geometry: RectI,
}

#[derive(Clone, Debug)]
struct PendingConfigure {
    serial: u32,
    state: ConfigureState,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct CommitRoleChanges {
    pub apply_window_geometry: bool,
    pub initial_commit_complete: bool,
    pub map_popup: bool,
    pub reset_on_unmap: bool,
}

#[derive(Debug)]
pub struct XdgSurface {
    id: XdgSurfaceId,
    wm_base: Resolved<XdgWmBaseId>,
    wl_surface: Resolved<WlSurfaceId>,
    /// Negotiated `xdg_wm_base` version for this shell (from the factory that created us).
    wm_base_version: u32,
    role_kind: Option<RoleKind>,
    role_object: RoleObject,
    initial_commit_complete: bool,
    next_configure_serial: u32,
    pending_configures: VecDeque<PendingConfigure>,
    last_acked: Option<PendingConfigure>,
    pending_window_geometry: Option<RectI>,
    current_window_geometry: Option<RectI>,
    popup_children: Vec<Resolved<XdgPopupId>>,
}

impl XdgSurface {
    pub fn new(
        id: XdgSurfaceId,
        wm_base: Resolved<XdgWmBaseId>,
        wl_surface: Resolved<WlSurfaceId>,
        wm_base_version: u32,
    ) -> Self {
        Self {
            id,
            wm_base,
            wl_surface,
            wm_base_version,
            role_kind: None,
            role_object: RoleObject::None,
            initial_commit_complete: false,
            next_configure_serial: 1,
            pending_configures: VecDeque::new(),
            last_acked: None,
            pending_window_geometry: None,
            current_window_geometry: None,
            popup_children: Vec::new(),
        }
    }

    pub(crate) fn xdg_wm_base(&self) -> Resolved<XdgWmBaseId> {
        self.wm_base
    }

    pub(crate) fn wl_surface(&self) -> Resolved<WlSurfaceId> {
        self.wl_surface
    }

    pub(crate) fn current_window_geometry(&self) -> Option<RectI> {
        self.current_window_geometry
    }

    pub(crate) fn initial_commit_complete(&self) -> bool {
        self.initial_commit_complete
    }

    pub(crate) fn popup_role(&self) -> Option<Resolved<XdgPopupId>> {
        match self.role_object {
            RoleObject::Popup(id) => Some(id),
            _ => None,
        }
    }

    pub(crate) fn has_live_role_object(&self) -> bool {
        !matches!(self.role_object, RoleObject::None)
    }

    pub(crate) fn is_popup_parent(&self) -> bool {
        matches!(self.role_object, RoleObject::Popup(_))
    }

    pub(crate) fn popup_has_explicit_grab(&self, client: &Client, state: &State) -> bool {
        let Some(popup) = self.popup_role() else {
            return false;
        };
        popup
            .try_ref(state)
            .is_some_and(XdgPopup::has_explicit_grab)
    }

    pub(crate) fn is_toplevel_parent(&self) -> bool {
        matches!(self.role_object, RoleObject::Toplevel(_))
    }

    pub(crate) fn parent_geometry(&self, _client: &Client, state: &State) -> RectI {
        self.effective_window_geometry(self.wl_surface, state)
    }

    /// Surface-local geometry for the whole interactive window (see `set_window_geometry`),
    /// falling back to the committed `wl_surface` extents — same basis as [`Self::parent_geometry`]
    /// but without needing a [`Client`] handle.
    pub(crate) fn window_geometry_rect_for_surface(
        &self,
        surface: Resolved<WlSurfaceId>,
        state: &State,
    ) -> RectI {
        self.effective_window_geometry(surface, state)
    }

    #[cfg(test)]
    pub(crate) fn test_set_current_window_geometry(&mut self, geometry: Option<RectI>) {
        self.current_window_geometry = geometry;
    }

    #[cfg(test)]
    pub(crate) fn test_set_pending_window_geometry(&mut self, geometry: Option<RectI>) {
        self.pending_window_geometry = geometry;
    }

    #[cfg(test)]
    pub(crate) fn test_pending_configure_count(&self) -> usize {
        self.pending_configures.len()
    }

    #[cfg(test)]
    pub(crate) fn test_set_initial_commit_complete(&mut self, complete: bool) {
        self.initial_commit_complete = complete;
    }

    #[cfg(test)]
    pub(crate) fn test_set_popup_role(&mut self, popup: Resolved<XdgPopupId>) {
        self.role_kind = Some(RoleKind::Popup);
        self.role_object = RoleObject::Popup(popup);
    }

    fn send_wm_base_error(
        &self,
        client: &Client,
        dispatcher: &mut ClientDispatcher,
        code: xdg_wm_base::Error,
        message: impl AsRef<str>,
    ) {
        client.send_protocol_error(
            dispatcher,
            self.wm_base.wayland_id().into(),
            code as u32,
            message,
        );
    }

    fn send_surface_error(
        &self,
        client: &Client,
        dispatcher: &mut ClientDispatcher,
        code: xdg_surface::Error,
        message: impl AsRef<str>,
    ) {
        client.send_protocol_error(dispatcher, self.id.into(), code as u32, message);
    }

    fn delete_self(
        &self,
        client: &mut Client,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
    ) {
        let this = client.resolve(self.id, state).unwrap();
        if let Some(mut wl_surface) = self.wl_surface.try_ref_mut(state) {
            wl_surface.clear_xdg_surface_role(this);
        }
        if let Some(mut wm_base) = self.wm_base.try_ref_mut(state) {
            wm_base.unregister_surface(this);
        }
        client.delete_id(&WlDisplay::default(), state, dispatcher, self.id.into());
    }

    fn set_role_kind(
        &mut self,
        new_role: RoleKind,
        client: &Client,
        dispatcher: &mut ClientDispatcher,
    ) -> Result<(), ()> {
        if self.role_kind.is_some() {
            self.send_surface_error(
                client,
                dispatcher,
                xdg_surface::Error::AlreadyConstructed,
                "xdg_surface already has a role object",
            );
            return Err(());
        }
        self.role_kind = Some(new_role);
        Ok(())
    }

    fn next_serial(&mut self) -> u32 {
        let serial = self.next_configure_serial;
        self.next_configure_serial = self.next_configure_serial.saturating_add(1);
        serial
    }

    pub(crate) fn queue_toplevel_configure(
        &mut self,
        width: i32,
        height: i32,
        states: Vec<u32>,
        decoration_mode: xdg_toplevel::ToplevelDecorationMode,
        dispatcher: &mut ClientDispatcher,
    ) -> u32 {
        let serial = self.next_serial();
        self.pending_configures.push_back(PendingConfigure {
            serial,
            state: ConfigureState::Toplevel(ToplevelConfigureState {
                width,
                height,
                states,
                decoration_mode,
            }),
        });
        self.send_event(xdg_surface::Event::Configure { serial }, dispatcher);
        serial
    }

    pub(crate) fn queue_popup_configure(
        &mut self,
        geometry: RectI,
        dispatcher: &mut ClientDispatcher,
    ) -> u32 {
        let serial = self.next_serial();
        self.pending_configures.push_back(PendingConfigure {
            serial,
            state: ConfigureState::Popup(PopupConfigureState { geometry }),
        });
        self.send_event(xdg_surface::Event::Configure { serial }, dispatcher);
        serial
    }

    fn reset_configure_state(&mut self) {
        self.initial_commit_complete = false;
        self.pending_configures.clear();
        self.last_acked = None;
        self.pending_window_geometry = None;
        self.current_window_geometry = None;
    }

    fn validate_committed_state(
        &self,
        client: &Client,
        dispatcher: &mut ClientDispatcher,
        has_pending_buffer_attach: bool,
    ) -> bool {
        if has_pending_buffer_attach && self.last_acked.is_none() {
            self.send_surface_error(
                client,
                dispatcher,
                xdg_surface::Error::UnconfiguredBuffer,
                "xdg_surface must ack the initial configure before attaching a buffer",
            );
            return false;
        }
        true
    }

    fn geometry_after_this_commit(&self) -> Option<RectI> {
        self.pending_window_geometry
            .or(self.current_window_geometry)
    }

    pub(crate) fn effective_window_geometry(
        &self,
        surface: Resolved<WlSurfaceId>,
        state: &State,
    ) -> RectI {
        let Some(surface_ref) = surface.try_ref(state) else {
            return self.current_window_geometry.unwrap_or(RectI {
                x: 0,
                y: 0,
                width: 0,
                height: 0,
            });
        };
        crate::root_geometry::effective_window_geometry_for_surface(
            state,
            &surface_ref,
            self.current_window_geometry,
        )
        .unwrap_or(RectI { x: 0, y: 0, width: 0, height: 0 })
    }

    pub(crate) fn apply_commit_role_changes(
        &mut self,
        client: &mut Client,
        state: &mut State,
        _dispatcher: &mut ClientDispatcher,
        role_changes: CommitRoleChanges,
    ) {
        if role_changes.apply_window_geometry {
            let pending_window_geometry = self.pending_window_geometry;
            if self.pending_window_geometry.is_some() {
                self.current_window_geometry = self.pending_window_geometry.take();
            }
            let surface = self.wl_surface();
            if let RoleObject::Toplevel(id) = self.role_object {
                if let Some(mut toplevel) = id.try_grab(state) {
                    toplevel.set_window_geometry(self.current_window_geometry);
                }
            }
        }
        if role_changes.initial_commit_complete {
            self.initial_commit_complete = true;
        }
        if role_changes.map_popup {
            if let RoleObject::Popup(id) = self.role_object {
                let popup_wl = self.wl_surface();
                let (rx, ry) = popup_wl
                    .try_ref(state)
                    .map(|s| s.root_position())
                    .unwrap_or((0, 0));
                let parent_summary = id.try_ref(state).and_then(|popup| {
                    popup.parent_surface().map(|parent_xdg| {
                        let pwl = parent_xdg.rref(state).wl_surface();
                        let pr = pwl
                            .try_ref(state)
                            .map(|s| s.root_position())
                            .unwrap_or((0, 0));
                        let committed = state
                            .root_surface_for_surface(pwl)
                            .and_then(|root| state.committed_root_geometry(root));
                        (parent_xdg, pr, committed.map(|g| g.content_rect_global()))
                    })
                });
                log::debug!(
                    "AAA xdg_surface apply_commit_role_changes map_popup popup_xdg={:?} popup_wl={:?} popup_wl_root_pos=({},{}) parent_summary=(parent_xdg={:?} parent_wl_root_pos={:?} parent_content_rect_global={:?})",
                    self.id,
                    popup_wl,
                    rx,
                    ry,
                    parent_summary.map(|(px, _, _)| px),
                    parent_summary.map(|(_, pr, _)| pr),
                    parent_summary.map(|(_, _, c)| c),
                );
                let mut popup = id.grab(state);
                popup.mark_mapped();
            }
        }
        if role_changes.reset_on_unmap {
            if let RoleObject::Toplevel(id) = self.role_object {
                let mut toplevel = id.grab(state);
                toplevel.reset_on_unmap();
            }
            if let RoleObject::Popup(id) = self.role_object {
                let popup_wl = self.wl_surface();
                let (rx, ry) = popup_wl
                    .try_ref(state)
                    .map(|s| s.root_position())
                    .unwrap_or((0, 0));
                log::debug!(
                    "AAA xdg_surface apply_commit_role_changes reset_on_unmap popup_xdg={:?} popup_wl={:?} popup_wl_root_pos=({},{})",
                    self.id,
                    popup_wl,
                    rx,
                    ry
                );
                let mut popup = id.grab(state);
                popup.mark_unmapped();
            }
            self.reset_configure_state();
        }
    }

    pub(crate) fn on_commit(
        &mut self,
        client: &mut Client,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
        current_logical_size: Option<SizeI>,
        has_pending_buffer_attach: bool,
        is_null_buffer_commit: bool,
        configure_bounds: Option<(i32, i32)>,
    ) -> Option<CommitRoleChanges> {
        let surface_id = self.wl_surface;
        let mut role_changes = CommitRoleChanges {
            apply_window_geometry: self.pending_window_geometry.is_some(),
            ..Default::default()
        };

        if !self.initial_commit_complete {
            match self.role_object {
                RoleObject::None => {
                    log::debug!(
                        "xdg_surface {:?}: commit before role object; sending xdg_surface.not_constructed",
                        self.id
                    );
                    client.protocol_violation_disconnect(
                        dispatcher,
                        self.id.into(),
                        xdg_surface::Error::NotConstructed as u32,
                        "xdg_surface committed before get_toplevel or get_popup",
                    );
                    return None;
                }
                RoleObject::Toplevel(id) => {
                    if has_pending_buffer_attach {
                        self.send_surface_error(
                            client,
                            dispatcher,
                            xdg_surface::Error::UnconfiguredBuffer,
                            "initial xdg_toplevel commit must not attach a buffer",
                        );
                        return None;
                    }
                    let desired_decoration_mode = id.try_ref(state).map(|toplevel| {
                        state.xdg_decoration_mode_for_surface(
                            Some(surface_id),
                            toplevel.has_decoration_object(),
                            toplevel.requested_decoration_mode(state),
                        )
                    });
                    if let Some(mode) = desired_decoration_mode {
                        if let Some(mut toplevel) = id.try_ref_mut(state) {
                            toplevel.schedule_decoration_mode(mode);
                        }
                    }
                    let fallback_decoration_insets = state.configured_decoration_insets();
                    let committed_window_geometry = self.geometry_after_this_commit();
                    let (target_min_size, surface_decoration_insets) = id
                        .try_ref(state)
                        .map(|toplevel| {
                            let insets = if toplevel.decoration_mode_for_configure()
                                == xdg_toplevel::ToplevelDecorationMode::ServerSide
                            {
                                fallback_decoration_insets
                            } else {
                                committed_window_geometry
                                    .and_then(|window_geometry| {
                                        current_logical_size.and_then(|logical_size| {
                                            crate::root_geometry::decoration_insets_from_window_geometry_and_logical_size(
                                                logical_size,
                                                window_geometry,
                                            )
                                        })
                                    })
                                    .unwrap_or(fallback_decoration_insets)
                            };
                            (toplevel.minimum_size(), insets)
                        })
                        .unwrap_or((None, fallback_decoration_insets));
                    let initial_tiled_geometry = crate::tiling::initial_root_geometry_for_surface(
                        state,
                        surface_id,
                        surface_decoration_insets,
                        id.try_ref(state)
                            .map(|toplevel| toplevel.decoration_mode_for_configure())
                            .unwrap_or(xdg_toplevel::ToplevelDecorationMode::ClientSide),
                        committed_window_geometry,
                        target_min_size,
                        configure_bounds,
                    );
                    let mut xdg_toplevel = id.grab(state);
                    xdg_toplevel.send_initial_configure(
                        client,
                        state,
                        configure_bounds,
                        initial_tiled_geometry,
                        self,
                        dispatcher,
                    );
                    role_changes.initial_commit_complete = true;
                    return Some(role_changes);
                }
                RoleObject::Popup(id) => {
                    if has_pending_buffer_attach {
                        self.send_surface_error(
                            client,
                            dispatcher,
                            xdg_surface::Error::UnconfiguredBuffer,
                            "initial xdg_popup commit must not attach a buffer",
                        );
                        return None;
                    }
                    let mut popup = id.grab(state);
                    popup.send_initial_configure(self, client, state, dispatcher);
                    role_changes.initial_commit_complete = true;
                    return Some(role_changes);
                }
            }
        }

        if !self.validate_committed_state(client, dispatcher, has_pending_buffer_attach) {
            return None;
        }

        if is_null_buffer_commit {
            role_changes.reset_on_unmap = true;
        } else if has_pending_buffer_attach && matches!(self.role_object, RoleObject::Popup(_)) {
            role_changes.map_popup = true;
        }

        Some(role_changes)
    }

    pub(crate) fn add_popup_child(&mut self, popup: Resolved<XdgPopupId>) {
        self.popup_children.push(popup);
    }

    pub(crate) fn remove_popup_child(&mut self, popup: Resolved<XdgPopupId>) {
        self.popup_children.retain(|candidate| *candidate != popup);
    }

    pub(crate) fn has_popup_child(&self, popup: Resolved<XdgPopupId>) -> bool {
        self.popup_children.contains(&popup)
    }

    pub(crate) fn is_topmost_popup(&self, popup: Resolved<XdgPopupId>) -> bool {
        self.popup_children.last().copied() == Some(popup)
    }

    pub(crate) fn popup_children(&self) -> &[Resolved<XdgPopupId>] {
        &self.popup_children
    }

    pub(crate) fn destroy_toplevel_role(&mut self, toplevel: Resolved<XdgToplevelId>) {
        if self.role_object == RoleObject::Toplevel(toplevel) {
            self.role_object = RoleObject::None;
            self.reset_configure_state();
        }
    }

    pub(crate) fn destroy_popup_role(&mut self, popup: Resolved<XdgPopupId>) {
        if self.role_object == RoleObject::Popup(popup) {
            self.role_object = RoleObject::None;
            self.reset_configure_state();
        }
    }
}

impl Object for XdgSurface {
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
        use xdg_surface::Request::*;

        match client.parse::<Self>(req).unwrap() {
            Destroy => {
                if self.has_live_role_object() {
                    self.send_surface_error(
                        client,
                        dispatcher,
                        xdg_surface::Error::DefunctRoleObject,
                        "xdg_surface destroyed before its role object",
                    );
                    return Ok(());
                }
                self.delete_self(client, state, dispatcher);
            }
            GetToplevel { id } => {
                if self
                    .set_role_kind(RoleKind::Toplevel, client, dispatcher)
                    .is_err()
                {
                    return Ok(());
                }
                let xdg_surface = client.resolve(self.id, state).unwrap();
                let xdg_toplevel =
                    xdg_toplevel::XdgToplevel::new(id, xdg_surface, self.wm_base_version);
                xdg_toplevel.send_capabilities(dispatcher);
                client.insert_object(state, xdg_toplevel);
                let toplevel = client.resolve(id, state).unwrap();
                self.role_object = RoleObject::Toplevel(toplevel);
                state.register_xdg_toplevel_role(xdg_surface, toplevel);
            }
            GetPopup { id, parent, positioner } => {
                let positioner = client.resolve_or_disconnect(positioner, state, dispatcher)?;

                let parent = match parent.to_opt() {
                    None => None,
                    Some(parent) => Some(client.resolve_or_disconnect(parent, state, dispatcher)?),
                };

                let positioner_state = positioner.rref(state).snapshot();
                if !positioner_state.is_complete() {
                    self.send_wm_base_error(
                        client,
                        dispatcher,
                        xdg_wm_base::Error::InvalidPositioner,
                        "xdg_surface.get_popup requires a complete xdg_positioner",
                    );
                    return Ok(());
                }

                if let Some(parent_surface_id) = parent {
                    let valid_parent = {
                        let parent_surface = parent_surface_id.try_ref(state).unwrap();
                        parent_surface.is_toplevel_parent()
                            || (parent_surface.is_popup_parent()
                                && parent_surface.popup_has_explicit_grab(client, state))
                    };
                    if !valid_parent {
                        self.send_wm_base_error(
                            client,
                            dispatcher,
                            xdg_wm_base::Error::InvalidPopupParent,
                            "popup parent must be a toplevel or a grabbing popup",
                        );
                        return Ok(());
                    }
                }

                if self
                    .set_role_kind(RoleKind::Popup, client, dispatcher)
                    .is_err()
                {
                    return Ok(());
                }
                let this = client.resolve(self.id, state).unwrap();
                log::debug!(
                    "AAA xdg_surface get_popup popup_xdg={:?} popup_wl={:?} parent_xdg={:?} positioner_state={:?}",
                    self.id,
                    self.wl_surface,
                    parent,
                    positioner_state
                );
                let popup_obj =
                    XdgPopup::new(id, this, parent, positioner_state, self.wm_base_version);
                let (popup, _) = client.insert_object(state, popup_obj);
                self.role_object = RoleObject::Popup(popup);
                let _ = state.clear_root_surface_workspace_assignment(self.wl_surface);
                if let Some(parent_surface_id) = parent {
                    let popup_surface = self.wl_surface;
                    let parent_surface = parent_surface_id.ref_mut(state);
                    parent_surface.add_popup_child(popup);
                }
            }
            SetWindowGeometry { x, y, width, height } => {
                let Some(window_geometry) = RectI::new(x, y, width, height) else {
                    self.send_surface_error(
                        client,
                        dispatcher,
                        xdg_surface::Error::InvalidSize,
                        "xdg_surface.set_window_geometry requires positive dimensions",
                    );
                    return Ok(());
                };
                if window_geometry.is_empty() {
                    self.send_surface_error(
                        client,
                        dispatcher,
                        xdg_surface::Error::InvalidSize,
                        "xdg_surface.set_window_geometry requires positive dimensions",
                    );
                    return Ok(());
                }
                self.pending_window_geometry = Some(window_geometry);
            }
            AckConfigure { serial } => {
                let Some(position) = self
                    .pending_configures
                    .iter()
                    .position(|configure| configure.serial == serial)
                else {
                    self.send_surface_error(
                        client,
                        dispatcher,
                        xdg_surface::Error::InvalidSerial,
                        "xdg_surface.ack_configure referenced an unknown serial",
                    );
                    return Ok(());
                };

                let acked = self.pending_configures[position].clone();
                self.pending_configures.drain(..=position);
                self.last_acked = Some(acked.clone());

                match acked.state {
                    ConfigureState::Toplevel(ToplevelConfigureState {
                        width,
                        height,
                        states,
                        decoration_mode,
                    }) => {
                        if let RoleObject::Toplevel(id) = self.role_object {
                            let mut toplevel = id.ref_mut(state);
                            toplevel.apply_acked_configure(width, height, states, decoration_mode);
                        }
                    }
                    ConfigureState::Popup(PopupConfigureState { geometry }) => {
                        if let RoleObject::Popup(popup_resolved) = self.role_object {
                            let popup_wl = self.wl_surface;
                            let (before_rx, before_ry) = popup_wl
                                .try_ref(state)
                                .map(|s| s.root_position())
                                .unwrap_or((0, 0));
                            let parent_placement =
                                popup_resolved.try_ref(state).and_then(|popup_obj| {
                                    popup_obj.parent_surface().map(|parent_xdg| {
                                        let pwl = parent_xdg.rref(state).wl_surface();
                                        let pr = pwl
                                            .try_ref(state)
                                            .map(|s| s.root_position())
                                            .unwrap_or((0, 0));
                                        let root = state.root_surface_for_surface(pwl);
                                        let geo =
                                            root.and_then(|r| state.committed_root_geometry(r));
                                        (parent_xdg, pwl, pr, root, geo)
                                    })
                                });
                            let mut popup = popup_resolved.ref_mut(state);
                            popup.apply_acked_configure(geometry);
                            let (after_rx, after_ry) = popup_wl
                                .try_ref(state)
                                .map(|s| s.root_position())
                                .unwrap_or((0, 0));
                            log::debug!(
                                "AAA xdg_surface ack_configure popup serial={} acked_geometry={:?} popup_xdg={:?} popup_wl={:?} popup_wl_root_pos_before=({},{}) after=({},{}) parent=(xdg={:?} wl={:?} wl_root=({},{}) root_surface={:?} committed_root_pos={:?} content_rect_global={:?} layout_rect_global={:?})",
                                acked.serial,
                                geometry,
                                self.id,
                                popup_wl,
                                before_rx,
                                before_ry,
                                after_rx,
                                after_ry,
                                parent_placement.map(|(pxd, _, _, _, _)| pxd),
                                parent_placement.map(|(_, pwl, _, _, _)| pwl),
                                parent_placement.map(|(_, _, pr, _, _)| pr.0).unwrap_or(0),
                                parent_placement.map(|(_, _, pr, _, _)| pr.1).unwrap_or(0),
                                parent_placement.map(|(_, _, _, root, _)| root),
                                parent_placement
                                    .and_then(|(_, _, _, _, g)| g.map(|geo| geo.root_pos)),
                                parent_placement
                                    .and_then(|(_, _, _, _, g)| g.map(|geo| geo.content_rect_global())),
                                parent_placement
                                    .and_then(|(_, _, _, _, g)| g.map(|geo| geo.layout_rect_global)),
                            );
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

pub mod xdg_toplevel {
    use std::collections::BTreeSet;

    use crate::{
        clients::{
            Client, ClientDispatcher, ClientId, ClientManagerAction, EventWriter, RawRequest,
            Resolved,
        },
        drm::schedule_outputs,
        object::{Object, Parser, PostAction, RequestResult},
        protocol_generated::{
            xdg_toplevel as proto_xdg_toplevel, zxdg_toplevel_decoration_v1, Array, WaylandId,
            WlOutputId, WlSeatId, WlSurfaceId, XdgToplevelId, ZxdgToplevelDecorationV1Id,
        },
        region::RectI,
        state::{GrabbedObject, ObjectId, State},
        wayland_objects::{
            wl_display::WlDisplay, wl_surface::WlSurface,
            zxdg_toplevel_decoration_v1::ZxdgToplevelDecorationV1,
        },
    };

    use super::XdgSurface;

    #[derive(Debug)]
    pub struct XdgToplevel {
        id: XdgToplevelId,
        xdg_surface: Resolved<super::XdgSurfaceId>,
        wm_base_version: u32,
        title: String,
        app_id: String,
        parent: Option<Resolved<XdgToplevelId>>,
        min_size: Option<(i32, i32)>,
        max_size: Option<(i32, i32)>,
        requested_fullscreen: bool,
        requested_maximized: bool,
        current_width: i32,
        current_height: i32,
        current_states: BTreeSet<proto_xdg_toplevel::State>,
        decoration_mode: ToplevelDecorationMode,
        pending_decoration_mode: Option<ToplevelDecorationMode>,
        window_geometry: Option<RectI>,
        decoration_object: Option<Resolved<ZxdgToplevelDecorationV1Id>>,
    }

    #[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
    pub struct InteractiveSizeConstraints {
        min_size: Option<(i32, i32)>,
        max_size: Option<(i32, i32)>,
    }

    #[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
    pub enum ToplevelDecorationMode {
        ServerSide,
        #[default]
        ClientSide,
    }

    impl InteractiveSizeConstraints {
        pub fn clamp(self, width: i32, height: i32) -> (i32, i32) {
            let (mut width, mut height) = (width.max(1), height.max(1));
            if let Some((min_width, min_height)) = self.min_size {
                if min_width > 0 {
                    width = width.max(min_width);
                }
                if min_height > 0 {
                    height = height.max(min_height);
                }
            }
            if let Some((max_width, max_height)) = self.max_size {
                if max_width > 0 {
                    width = width.min(max_width);
                }
                if max_height > 0 {
                    height = height.min(max_height);
                }
            }
            (width, height)
        }
    }

    impl XdgToplevel {
        pub fn new(
            id: XdgToplevelId,
            xdg_surface: Resolved<super::XdgSurfaceId>,
            wm_base_version: u32,
        ) -> Self {
            Self {
                id,
                xdg_surface,
                wm_base_version,
                title: String::new(),
                app_id: String::new(),
                parent: None,
                min_size: None,
                max_size: None,
                requested_fullscreen: false,
                requested_maximized: false,
                current_width: 0,
                current_height: 0,
                current_states: BTreeSet::default(),
                decoration_mode: ToplevelDecorationMode::ClientSide,
                pending_decoration_mode: None,
                window_geometry: None,
                decoration_object: None,
            }
        }

        fn delete_self(
            &self,
            client: &mut Client,
            state: &mut State,
            dispatcher: &mut ClientDispatcher,
        ) {
            client.delete_id(&WlDisplay::default(), state, dispatcher, self.id.into());
        }

        fn send_error(
            &self,
            client: &Client,
            dispatcher: &mut ClientDispatcher,
            code: proto_xdg_toplevel::Error,
            message: impl AsRef<str>,
        ) {
            client.send_protocol_error(dispatcher, self.id.into(), code as u32, message);
        }

        fn effective_states(&self) -> Vec<u32> {
            let mut states = BTreeSet::default();
            if self.requested_maximized {
                states.insert(proto_xdg_toplevel::State::Maximized);
            }
            if self.requested_fullscreen {
                states.insert(proto_xdg_toplevel::State::Fullscreen);
            }
            for state in self.current_states.iter().cloned() {
                if matches!(
                    state,
                    proto_xdg_toplevel::State::Activated | proto_xdg_toplevel::State::Suspended
                ) {
                    states.insert(state);
                }
            }
            states.into_iter().map(|state| state as u32).collect()
        }

        fn layout_states(&self, tiled_edges: u32, fullscreen: bool, resizing: bool) -> Vec<u32> {
            let mut states = BTreeSet::default();
            if fullscreen {
                states.insert(proto_xdg_toplevel::State::Fullscreen);
            } else if tiled_edges != 0 {
                // Match Hyprland's tiled startup hint so GTK clients settle into the compositor's
                // managed frame earlier instead of briefly presenting a smaller intermediate size.
                states.insert(proto_xdg_toplevel::State::Maximized);
            }
            for state in self.current_states.iter().cloned() {
                if matches!(
                    state,
                    proto_xdg_toplevel::State::Activated | proto_xdg_toplevel::State::Suspended
                ) {
                    states.insert(state);
                }
            }
            if resizing {
                states.insert(proto_xdg_toplevel::State::Resizing);
            }
            if tiled_edges & crate::tiling::EDGE_LEFT != 0 {
                states.insert(proto_xdg_toplevel::State::TiledLeft);
            }
            if tiled_edges & crate::tiling::EDGE_RIGHT != 0 {
                states.insert(proto_xdg_toplevel::State::TiledRight);
            }
            if tiled_edges & crate::tiling::EDGE_TOP != 0 {
                states.insert(proto_xdg_toplevel::State::TiledTop);
            }
            if tiled_edges & crate::tiling::EDGE_BOTTOM != 0 {
                states.insert(proto_xdg_toplevel::State::TiledBottom);
            }
            states.into_iter().map(|state| state as u32).collect()
        }

        fn send_configure(&self, dispatcher: &mut ClientDispatcher) {
            self.send_event(
                proto_xdg_toplevel::Event::Configure {
                    width: self.current_width,
                    height: self.current_height,
                    states: Array(self.effective_states()),
                },
                dispatcher,
            );
        }

        pub(crate) fn configure_bounds_for_xdg_surface(
            _client: &Client,
            state: &State,
            xdg_surface: &XdgSurface,
        ) -> Option<(i32, i32)> {
            state.configure_bounds_for_surface(xdg_surface.wl_surface(), None)
        }

        fn send_configure_bounds(
            &self,
            configure_bounds: Option<(i32, i32)>,
            dispatcher: &mut ClientDispatcher,
        ) {
            if self.wm_base_version < proto_xdg_toplevel::CONFIGURE_BOUNDS_SINCE {
                return;
            }
            let Some((width, height)) = configure_bounds else {
                return;
            };
            self.send_event(
                proto_xdg_toplevel::Event::ConfigureBounds { width, height },
                dispatcher,
            );
        }

        pub(crate) fn schedule_configure(
            &mut self,
            _client: &Client,
            state: &State,
            configure_bounds: Option<(i32, i32)>,
            xdg_surface: &mut XdgSurface,
            dispatcher: &mut ClientDispatcher,
        ) {
            let states = self.effective_states();
            self.send_configure_bounds(configure_bounds, dispatcher);
            self.send_configure(dispatcher);
            self.send_decoration_configure_if_needed(state, dispatcher);
            xdg_surface.queue_toplevel_configure(
                self.current_width,
                self.current_height,
                states,
                self.decoration_mode_for_configure(),
                dispatcher,
            );
        }

        pub(crate) fn schedule_configure_with_known_decoration(
            &mut self,
            decoration: &ZxdgToplevelDecorationV1,
            state: &State,
            configure_bounds: Option<(i32, i32)>,
            xdg_surface: &mut XdgSurface,
            dispatcher: &mut ClientDispatcher,
        ) {
            let states = self.effective_states();
            self.send_configure_bounds(configure_bounds, dispatcher);
            self.send_configure(dispatcher);
            decoration.send_configure_mode(self.decoration_mode_for_configure(), dispatcher);
            xdg_surface.queue_toplevel_configure(
                self.current_width,
                self.current_height,
                states,
                self.decoration_mode_for_configure(),
                dispatcher,
            );
        }

        pub fn send_initial_configure(
            &mut self,
            client: &Client,
            state: &State,
            configure_bounds: Option<(i32, i32)>,
            initial_tiled_geometry: Option<crate::root_geometry::CommittedRootGeometry>,
            xdg_surface: &mut XdgSurface,
            dispatcher: &mut ClientDispatcher,
        ) {
            if let Some(geometry) = initial_tiled_geometry {
                let configure_geometry = crate::root_geometry::project_configure_geometry(
                    geometry,
                    self.decoration_mode_for_configure(),
                    state.configured_decoration_insets(),
                );
                self.send_tiled_layout_configure(
                    client,
                    state,
                    xdg_surface,
                    geometry,
                    configure_geometry,
                    false,
                    dispatcher,
                );
            } else {
                self.schedule_configure(client, state, configure_bounds, xdg_surface, dispatcher);
            }
        }

        pub fn send_tiled_layout_configure(
            &mut self,
            client: &Client,
            state: &State,
            xdg_surface: &mut XdgSurface,
            geometry: crate::root_geometry::CommittedRootGeometry,
            configure_geometry: crate::root_geometry::ConfigureGeometry,
            resizing: bool,
            dispatcher: &mut ClientDispatcher,
        ) {
            let (width, height) = configure_geometry.size;
            self.send_layout_configure(
                client,
                state,
                Some(configure_geometry.size),
                xdg_surface,
                width,
                height,
                geometry.tiled_edges,
                geometry.fullscreen,
                resizing,
                dispatcher,
            );
        }

        pub fn send_layout_configure(
            &mut self,
            client: &Client,
            state: &State,
            configure_bounds: Option<(i32, i32)>,
            xdg_surface: &mut XdgSurface,
            width: i32,
            height: i32,
            tiled_edges: u32,
            fullscreen: bool,
            resizing: bool,
            dispatcher: &mut ClientDispatcher,
        ) {
            let states = self.layout_states(tiled_edges, fullscreen, resizing);
            self.send_configure_bounds(configure_bounds, dispatcher);
            self.send_event(
                proto_xdg_toplevel::Event::Configure {
                    width,
                    height,
                    states: Array(states.clone()),
                },
                dispatcher,
            );
            self.send_decoration_configure_if_needed(state, dispatcher);
            xdg_surface.queue_toplevel_configure(
                width,
                height,
                states,
                self.decoration_mode_for_configure(),
                dispatcher,
            );
        }

        pub fn apply_acked_configure(
            &mut self,
            width: i32,
            height: i32,
            states: Vec<u32>,
            decoration_mode: ToplevelDecorationMode,
        ) {
            self.current_width = width;
            self.current_height = height;
            self.current_states = states.into_iter().filter_map(decode_state).collect();
            self.decoration_mode = decoration_mode;
            self.pending_decoration_mode = None;
        }

        pub fn xdg_surface(&self) -> Resolved<super::XdgSurfaceId> {
            self.xdg_surface
        }

        pub fn toplevel_id(&self) -> XdgToplevelId {
            self.id
        }

        pub fn interactive_size_constraints(&self) -> InteractiveSizeConstraints {
            InteractiveSizeConstraints { min_size: self.min_size, max_size: self.max_size }
        }

        pub fn clamp_interactive_size(&self, width: i32, height: i32) -> (i32, i32) {
            self.interactive_size_constraints().clamp(width, height)
        }

        pub fn minimum_size(&self) -> Option<(i32, i32)> {
            self.min_size.and_then(|(width, height)| {
                let width = if width > 0 { width } else { 1 };
                let height = if height > 0 { height } else { 1 };
                ((width > 1) || (height > 1) || self.min_size == Some((1, 1)))
                    .then_some((width, height))
            })
        }

        pub fn decoration_mode(&self) -> ToplevelDecorationMode {
            self.decoration_mode
        }

        pub fn decoration_mode_for_configure(&self) -> ToplevelDecorationMode {
            self.pending_decoration_mode.unwrap_or(self.decoration_mode)
        }

        pub fn requested_decoration_mode(&self, state: &State) -> Option<ToplevelDecorationMode> {
            self.decoration_object.and_then(|decoration| {
                decoration
                    .try_ref(state)
                    .and_then(ZxdgToplevelDecorationV1::requested_mode)
            })
        }

        pub fn window_geometry(&self) -> Option<RectI> {
            self.window_geometry
        }

        pub fn title(&self) -> &str {
            &self.title
        }

        pub fn app_id(&self) -> &str {
            &self.app_id
        }

        pub fn set_decoration_mode(&mut self, mode: ToplevelDecorationMode) -> bool {
            if self.decoration_mode == mode {
                return false;
            }
            self.decoration_mode = mode;
            true
        }

        pub fn schedule_decoration_mode(&mut self, mode: ToplevelDecorationMode) -> bool {
            if self.decoration_mode_for_configure() == mode {
                return false;
            }
            self.pending_decoration_mode = Some(mode);
            true
        }

        pub fn set_window_geometry(&mut self, window_geometry: Option<RectI>) {
            self.window_geometry = window_geometry;
        }

        pub fn has_decoration_object(&self) -> bool {
            self.decoration_object.is_some()
        }

        pub fn decoration_object(&self) -> Option<Resolved<ZxdgToplevelDecorationV1Id>> {
            self.decoration_object
        }

        pub fn set_decoration_object(
            &mut self,
            decoration: Resolved<ZxdgToplevelDecorationV1Id>,
        ) -> bool {
            if self.decoration_object.is_some() {
                return false;
            }
            self.decoration_object = Some(decoration);
            true
        }

        pub fn clear_decoration_object(
            &mut self,
            decoration: Resolved<ZxdgToplevelDecorationV1Id>,
        ) {
            if self.decoration_object == Some(decoration) {
                self.decoration_object = None;
            }
        }

        fn send_decoration_configure_if_needed(
            &self,
            state: &State,
            dispatcher: &mut ClientDispatcher,
        ) {
            let Some(decoration) = self
                .decoration_object
                .and_then(|decoration| decoration.try_ref(state))
            else {
                return;
            };
            decoration.send_configure_mode(self.decoration_mode_for_configure(), dispatcher);
        }

        pub fn set_requested_fullscreen(&mut self, fullscreen: bool) {
            self.requested_fullscreen = fullscreen;
        }

        pub fn is_maximized(&self) -> bool {
            self.requested_maximized
                || self
                    .current_states
                    .contains(&proto_xdg_toplevel::State::Maximized)
        }

        pub fn is_fullscreen(&self) -> bool {
            self.requested_fullscreen
                || self
                    .current_states
                    .contains(&proto_xdg_toplevel::State::Fullscreen)
        }

        fn has_known_configure_size(&self) -> bool {
            self.current_width > 0 && self.current_height > 0
        }

        fn relayout_after_constraint_change(
            &self,
            client: &Client,
            state: &mut State,
            dispatcher: &mut ClientDispatcher,
        ) {
            let xdg_surface = self.xdg_surface.rref(state);
            let surface = xdg_surface.wl_surface();
            let Some(root_surface) = state.root_surface_for_surface(surface) else {
                return;
            };
            if !state.is_tiled_root_surface(root_surface)
                && !state.root_surface_is_fullscreen(root_surface)
            {
                return;
            }
            let Some(workspace_id) = state.root_surface_workspace(root_surface) else {
                return;
            };
            let affected_outputs = state.workspace_output_ids(workspace_id);
            crate::tiling::mark_workspace_dirty(state, workspace_id);
            dispatcher
                .dispatch(ClientManagerAction::PublishWorkspaceStateChanged { affected_outputs });
        }

        pub fn send_interactive_resize_configure(
            &mut self,
            client: &Client,
            state: &State,
            configure_bounds: Option<(i32, i32)>,
            xdg_surface: &mut XdgSurface,
            width: i32,
            height: i32,
            resizing: bool,
            dispatcher: &mut ClientDispatcher,
        ) {
            let (width, height) = self.clamp_interactive_size(width, height);
            let mut states = self.effective_states();
            if resizing {
                let resizing_flag = proto_xdg_toplevel::State::Resizing as u32;
                if !states.contains(&resizing_flag) {
                    states.push(resizing_flag);
                }
            } else {
                states.retain(|state| *state != proto_xdg_toplevel::State::Resizing as u32);
            }
            self.send_configure_bounds(configure_bounds, dispatcher);
            self.send_event(
                proto_xdg_toplevel::Event::Configure {
                    width,
                    height,
                    states: Array(states.clone()),
                },
                dispatcher,
            );
            self.send_decoration_configure_if_needed(state, dispatcher);
            xdg_surface.queue_toplevel_configure(
                width,
                height,
                states,
                self.decoration_mode_for_configure(),
                dispatcher,
            );
        }

        #[cfg(test)]
        pub(crate) fn test_set_maximized(&mut self, maximized: bool) {
            self.requested_maximized = maximized;
        }

        #[cfg(test)]
        pub(crate) fn test_set_fullscreen(&mut self, fullscreen: bool) {
            self.requested_fullscreen = fullscreen;
        }

        #[cfg(test)]
        pub(crate) fn test_current_size(&self) -> (i32, i32) {
            (self.current_width, self.current_height)
        }

        #[cfg(test)]
        pub(crate) fn test_current_states(&self) -> Vec<u32> {
            self.current_states
                .iter()
                .cloned()
                .map(|state| state as u32)
                .collect()
        }

        #[cfg(test)]
        pub(crate) fn test_set_interactive_size_constraints(
            &mut self,
            min_size: Option<(i32, i32)>,
            max_size: Option<(i32, i32)>,
        ) {
            self.min_size = min_size;
            self.max_size = max_size;
        }

        #[cfg(test)]
        pub(crate) fn test_set_decoration_mode(&mut self, mode: ToplevelDecorationMode) {
            self.decoration_mode = mode;
        }

        pub fn reset_on_unmap(&mut self) {
            self.parent = None;
            self.requested_fullscreen = false;
            self.requested_maximized = false;
            self.current_width = 0;
            self.current_height = 0;
            self.current_states.clear();
            self.decoration_mode = ToplevelDecorationMode::ClientSide;
            self.pending_decoration_mode = None;
            self.window_geometry = None;
        }

        /// `wm_capabilities` exists only since xdg_wm_base version 5; older clients (e.g. wev) must
        /// not receive wire opcode 3 or libwayland aborts with a NULL listener.
        pub fn send_capabilities(&self, dispatcher: &mut ClientDispatcher) {
            if self.wm_base_version < proto_xdg_toplevel::WM_CAPABILITIES_SINCE {
                return;
            }
            use proto_xdg_toplevel::WmCapabilities;
            let capabilities_list =
                [WmCapabilities::Fullscreen as u32, WmCapabilities::Maximize as u32];
            self.send_event(
                proto_xdg_toplevel::Event::WmCapabilities {
                    capabilities: Array(capabilities_list.to_vec()),
                },
                dispatcher,
            );
        }

        fn validate_parent(&self, parent: Option<Resolved<XdgToplevelId>>, state: &State) -> bool {
            let Some(mut current) = parent else {
                return true;
            };
            if current.wayland_id() == self.id {
                return false;
            }
            loop {
                if current.wayland_id() == self.id {
                    return false;
                }
                if !current.exists(state) {
                    return true;
                }
                let toplevel = current.rref(state);
                let Some(next) = toplevel.parent else {
                    return true;
                };
                current = next;
            }
        }

        fn store_size_pair(
            &mut self,
            client: &Client,
            dispatcher: &mut ClientDispatcher,
            width: i32,
            height: i32,
            is_min: bool,
        ) -> bool {
            if width < 0 || height < 0 {
                self.send_error(
                    client,
                    dispatcher,
                    proto_xdg_toplevel::Error::InvalidSize,
                    "xdg_toplevel size constraints must be non-negative",
                );
                return false;
            }
            let candidate = if width == 0 && height == 0 { None } else { Some((width, height)) };

            if is_min {
                if let (Some((min_w, min_h)), Some((max_w, max_h))) = (candidate, self.max_size) {
                    if (max_w != 0 && min_w > max_w) || (max_h != 0 && min_h > max_h) {
                        self.send_error(
                            client,
                            dispatcher,
                            proto_xdg_toplevel::Error::InvalidSize,
                            "xdg_toplevel minimum size must not exceed maximum size",
                        );
                        return false;
                    }
                }
                self.min_size = candidate;
            } else {
                if let (Some((max_w, max_h)), Some((min_w, min_h))) = (candidate, self.min_size) {
                    if (max_w != 0 && max_w < min_w) || (max_h != 0 && max_h < min_h) {
                        self.send_error(
                            client,
                            dispatcher,
                            proto_xdg_toplevel::Error::InvalidSize,
                            "xdg_toplevel maximum size must not be smaller than minimum size",
                        );
                        return false;
                    }
                }
                self.max_size = candidate;
            }
            true
        }
    }

    fn decode_state(raw: u32) -> Option<proto_xdg_toplevel::State> {
        Some(match raw {
            1 => proto_xdg_toplevel::State::Maximized,
            2 => proto_xdg_toplevel::State::Fullscreen,
            3 => proto_xdg_toplevel::State::Resizing,
            4 => proto_xdg_toplevel::State::Activated,
            5 => proto_xdg_toplevel::State::TiledLeft,
            6 => proto_xdg_toplevel::State::TiledRight,
            7 => proto_xdg_toplevel::State::TiledTop,
            8 => proto_xdg_toplevel::State::TiledBottom,
            9 => proto_xdg_toplevel::State::Suspended,
            10 => proto_xdg_toplevel::State::ConstrainedLeft,
            11 => proto_xdg_toplevel::State::ConstrainedRight,
            12 => proto_xdg_toplevel::State::ConstrainedTop,
            13 => proto_xdg_toplevel::State::ConstrainedBottom,
            _ => return None,
        })
    }

    fn decode_resize_edge(raw: u32) -> Option<proto_xdg_toplevel::ResizeEdge> {
        Some(match raw {
            0 => proto_xdg_toplevel::ResizeEdge::None,
            1 => proto_xdg_toplevel::ResizeEdge::Top,
            2 => proto_xdg_toplevel::ResizeEdge::Bottom,
            4 => proto_xdg_toplevel::ResizeEdge::Left,
            5 => proto_xdg_toplevel::ResizeEdge::TopLeft,
            6 => proto_xdg_toplevel::ResizeEdge::BottomLeft,
            8 => proto_xdg_toplevel::ResizeEdge::Right,
            9 => proto_xdg_toplevel::ResizeEdge::TopRight,
            10 => proto_xdg_toplevel::ResizeEdge::BottomRight,
            _ => return None,
        })
    }

    impl Object for XdgToplevel {
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
            use proto_xdg_toplevel::Request::*;

            match client.parse::<Self>(req).unwrap() {
                Destroy => {
                    if let Some(decoration) = self.decoration_object() {
                        let decoration_id =
                            decoration.try_ref(state).map(|decoration| decoration.id());
                        if let Some(decoration_id) = decoration_id {
                            client.protocol_violation_disconnect(
                                dispatcher,
                                decoration_id,
                                zxdg_toplevel_decoration_v1::Error::Orphaned as u32,
                                "xdg_toplevel destroyed before xdg_toplevel_decoration",
                            );
                        }
                        self.decoration_object = None;
                    }
                    if let Some(toplevel) = client.resolve(self.id, state) {
                        state.unregister_xdg_toplevel_role(self.xdg_surface, toplevel);
                    }
                    let mut xdg_surface = self.xdg_surface.grab(state);
                    let this = client.resolve(self.id, state).unwrap();
                    xdg_surface.destroy_toplevel_role(this);
                    self.delete_self(client, state, dispatcher);
                }
                SetParent { parent } => {
                    let parent = match parent.to_opt() {
                        None => None,
                        Some(parent) => {
                            Some(client.resolve_or_disconnect(parent, state, dispatcher)?)
                        }
                    };
                    if !self.validate_parent(parent, state) {
                        self.send_error(
                            client,
                            dispatcher,
                            proto_xdg_toplevel::Error::InvalidParent,
                            "xdg_toplevel parent would create a cycle",
                        );
                        return Ok(());
                    }
                    self.parent = parent;
                }
                SetTitle { title } => {
                    self.title = title;
                }
                SetAppId { app_id } => {
                    self.app_id = app_id;
                }
                ShowWindowMenu { seat: _, serial: _, x: _, y: _ } => {
                    log::debug!("xdg_toplevel.show_window_menu is not integrated yet");
                }
                Move { seat, serial } => {
                    let _seat = client.resolve_or_disconnect(seat, state, dispatcher)?;
                    let xdg_surface = self.xdg_surface.rref(state);
                    let surface = xdg_surface.wl_surface();
                    let Some(surface_ref) = surface.try_ref(state) else {
                        return Ok(());
                    };
                    let root_surface = state.root_surface_for_surface(surface).unwrap_or(surface);
                    let Some(xdg_toplevel) = client.resolve(self.id, state) else {
                        return Ok(());
                    };
                    if !crate::input::request_toplevel_move(
                        state,
                        dispatcher,
                        client.client_id(),
                        surface,
                        serial,
                        root_surface,
                        xdg_toplevel,
                        self.decoration_mode_for_configure(),
                        self.is_maximized(),
                        state.root_surface_is_fullscreen(root_surface),
                    ) {
                        log::debug!(
                            "xdg_toplevel.move ignored due to invalid pointer grab serial {}",
                            serial
                        );
                    }
                }
                Resize { seat, serial, edges } => {
                    if edges == proto_xdg_toplevel::ResizeEdge::None as u32 {
                        self.send_error(
                            client,
                            dispatcher,
                            proto_xdg_toplevel::Error::InvalidResizeEdge,
                            "xdg_toplevel.resize requires a non-zero resize edge",
                        );
                        return Ok(());
                    }
                    if decode_resize_edge(edges).is_none() {
                        self.send_error(
                            client,
                            dispatcher,
                            proto_xdg_toplevel::Error::InvalidResizeEdge,
                            "xdg_toplevel.resize received an unknown resize edge",
                        );
                        return Ok(());
                    }
                    let _seat = client.resolve_or_disconnect(seat, state, dispatcher)?;
                    let xdg_surface = self.xdg_surface.rref(state);
                    let surface = xdg_surface.wl_surface();
                    let Some(surface_ref) = surface.try_ref(state) else {
                        return Ok(());
                    };
                    if surface_ref.logical_size().is_none() {
                        log::debug!("xdg_toplevel.resize ignored for unmapped surface");
                        return Ok(());
                    }
                    let root_surface = state.root_surface_for_surface(surface).unwrap_or(surface);
                    let Some(xdg_toplevel) = client.resolve(self.id, state) else {
                        return Ok(());
                    };

                    dispatcher.dispatch_post_action(PostXdgToplevel::RequestResize {
                        client_id: client.client_id(),
                        surface,
                        serial,
                        root_surface,
                        xdg_toplevel,
                        configure_decoration_mode: self.decoration_mode_for_configure(),
                        size_constraints: self.interactive_size_constraints(),
                        is_maximized: self.is_maximized(),
                        is_fullscreen: state.root_surface_is_fullscreen(root_surface),
                        edges,
                    });
                }
                SetMaxSize { width, height } => {
                    if !self.store_size_pair(client, dispatcher, width, height, false) {
                        return Ok(());
                    }
                    self.relayout_after_constraint_change(client, state, dispatcher);
                }
                SetMinSize { width, height } => {
                    if !self.store_size_pair(client, dispatcher, width, height, true) {
                        return Ok(());
                    }
                    self.relayout_after_constraint_change(client, state, dispatcher);
                }
                SetMaximized => {
                    self.requested_maximized = true;
                    if !self.has_known_configure_size() {
                        return Ok(());
                    }
                    let xdg_surface = self.xdg_surface;
                    let (mut this, bounds) = client.with_ungrabbed(self, state, |state| {
                        let mut xdg_surface = xdg_surface.rref(state);
                        Self::configure_bounds_for_xdg_surface(client, state, &xdg_surface)
                    });
                    let mut xdg_surface = this.xdg_surface.grab(state);
                    this.schedule_configure(client, state, bounds, &mut xdg_surface, dispatcher);
                }
                UnsetMaximized => {
                    self.requested_maximized = false;
                    if !self.has_known_configure_size() {
                        return Ok(());
                    }
                    let xdg_surface = self.xdg_surface;
                    let (mut this, bounds) = client.with_ungrabbed(self, state, |state| {
                        let xdg_surface = xdg_surface.rref(state);
                        Self::configure_bounds_for_xdg_surface(client, state, &xdg_surface)
                    });
                    let mut xdg_surface = this.xdg_surface.grab(state);
                    this.schedule_configure(client, state, bounds, &mut xdg_surface, dispatcher);
                }
                SetFullscreen { output: _ } => {
                    self.requested_fullscreen = true;
                    let xdg_surface = self.xdg_surface.rref(state);
                    let surface = xdg_surface.wl_surface();
                    let Some(root_surface) = state.root_surface_for_surface(surface) else {
                        return Ok(());
                    };
                    let affected_outputs = state
                        .root_surface_workspace(root_surface)
                        .map(|workspace_id| state.workspace_output_ids(workspace_id))
                        .unwrap_or_default();
                    if state.set_root_surface_fullscreen(root_surface, true) {
                        if !affected_outputs.is_empty() {
                            schedule_outputs(state, affected_outputs.into_iter(), dispatcher);
                        }
                        crate::seat::refresh_scene_focus(state, dispatcher);
                        dispatcher.dispatch(ClientManagerAction::WorkspaceStateChanged);
                    }
                }
                UnsetFullscreen => {
                    self.requested_fullscreen = false;
                    let xdg_surface = self.xdg_surface.rref(state);
                    let surface = xdg_surface.wl_surface();
                    let Some(root_surface) = state.root_surface_for_surface(surface) else {
                        return Ok(());
                    };
                    let affected_outputs = state
                        .root_surface_workspace(root_surface)
                        .map(|workspace_id| state.workspace_output_ids(workspace_id))
                        .unwrap_or_default();
                    if state.set_root_surface_fullscreen(root_surface, false) {
                        if !affected_outputs.is_empty() {
                            schedule_outputs(state, affected_outputs.into_iter(), dispatcher);
                        }
                        crate::seat::refresh_scene_focus(state, dispatcher);
                        dispatcher.dispatch(ClientManagerAction::WorkspaceStateChanged);
                    }
                }
                SetMinimized => {
                    log::debug!("xdg_toplevel.set_minimized has no compositor integration yet");
                }
            }
            Ok(())
        }
    }

    enum PostXdgToplevel {
        RequestResize {
            client_id: ClientId,
            surface: Resolved<WlSurfaceId>,
            serial: u32,
            root_surface: Resolved<WlSurfaceId>,
            xdg_toplevel: Resolved<XdgToplevelId>,
            configure_decoration_mode: ToplevelDecorationMode,
            size_constraints: InteractiveSizeConstraints,
            is_maximized: bool,
            is_fullscreen: bool,
            edges: u32,
        },
    }

    impl PostAction for PostXdgToplevel {
        fn execute(
            self: Box<Self>,
            _client: &mut Client,
            state: &mut State,
            dispatcher: &mut ClientDispatcher,
        ) {
            match *self {
                PostXdgToplevel::RequestResize {
                    client_id,
                    surface,
                    serial,
                    root_surface,
                    xdg_toplevel,
                    configure_decoration_mode,
                    size_constraints,
                    is_maximized,
                    is_fullscreen,
                    edges,
                } => {
                    if !crate::input::request_toplevel_resize(
                        state,
                        dispatcher,
                        client_id,
                        surface,
                        serial,
                        root_surface,
                        xdg_toplevel,
                        configure_decoration_mode,
                        size_constraints,
                        is_maximized,
                        is_fullscreen,
                        edges,
                    ) {
                        log::debug!(
                            "xdg_toplevel.resize ignored due to invalid pointer grab serial {}",
                            serial
                        );
                    }
                }
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn layout_states_strip_maximized_from_tiled_configures() {
            let mut toplevel = XdgToplevel::new(
                XdgToplevelId::from(1),
                Resolved::new(
                    crate::clients::ClientId::from_raw(1),
                    super::super::XdgSurfaceId::from(2),
                    ObjectId::from(2usize),
                ),
                7,
            );
            toplevel.test_set_maximized(true);

            let states = toplevel.layout_states(
                crate::tiling::EDGE_LEFT | crate::tiling::EDGE_RIGHT,
                false,
                false,
            );

            assert!(states.contains(&(proto_xdg_toplevel::State::Maximized as u32)));
            assert!(states.contains(&(proto_xdg_toplevel::State::TiledLeft as u32)));
            assert!(states.contains(&(proto_xdg_toplevel::State::TiledRight as u32)));
        }

        #[test]
        fn layout_states_strip_maximized_from_fullscreen_layout_configures() {
            let mut toplevel = XdgToplevel::new(
                XdgToplevelId::from(3),
                Resolved::new(
                    crate::clients::ClientId::from_raw(1),
                    super::super::XdgSurfaceId::from(4),
                    ObjectId::from(4usize),
                ),
                7,
            );
            toplevel.test_set_maximized(true);

            let states = toplevel.layout_states(0, true, false);

            assert!(!states.contains(&(proto_xdg_toplevel::State::Maximized as u32)));
            assert!(states.contains(&(proto_xdg_toplevel::State::Fullscreen as u32)));
        }

        #[test]
        fn layout_states_include_resizing_for_interactive_tiled_resize() {
            let toplevel = XdgToplevel::new(
                XdgToplevelId::from(5),
                Resolved::new(
                    crate::clients::ClientId::from_raw(1),
                    super::super::XdgSurfaceId::from(6),
                    ObjectId::from(6usize),
                ),
                7,
            );

            let states = toplevel.layout_states(crate::tiling::EDGE_RIGHT, false, true);

            assert!(states.contains(&(proto_xdg_toplevel::State::Resizing as u32)));
            assert!(states.contains(&(proto_xdg_toplevel::State::TiledRight as u32)));
        }

        #[test]
        fn effective_states_drop_stale_fullscreen_from_acked_state() {
            let mut toplevel = XdgToplevel::new(
                XdgToplevelId::from(7),
                Resolved::new(
                    crate::clients::ClientId::from_raw(1),
                    super::super::XdgSurfaceId::from(8),
                    ObjectId::from(8usize),
                ),
                7,
            );
            toplevel.apply_acked_configure(
                800,
                600,
                vec![
                    proto_xdg_toplevel::State::Activated as u32,
                    proto_xdg_toplevel::State::Fullscreen as u32,
                ],
                ToplevelDecorationMode::ClientSide,
            );

            let states = toplevel.effective_states();

            assert!(states.contains(&(proto_xdg_toplevel::State::Activated as u32)));
            assert!(!states.contains(&(proto_xdg_toplevel::State::Fullscreen as u32)));
        }

        #[test]
        fn tiled_layout_configure_uses_content_size_for_server_side_mode() {
            let state = crate::test_support::state_for_tests();
            let mut client =
                crate::clients::Client::new_for_tests(crate::clients::ClientId::from_raw(1));
            let mut dispatcher = crate::runner::Dispatcher::default();
            let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);
            let mut xdg_surface = XdgSurface::new(
                crate::protocol_generated::XdgSurfaceId::from(9),
                Resolved::new(
                    crate::clients::ClientId::from_raw(1),
                    crate::protocol_generated::XdgWmBaseId::from(10),
                    ObjectId::from(10usize),
                ),
                Resolved::new(
                    crate::clients::ClientId::from_raw(1),
                    crate::protocol_generated::WlSurfaceId::from(11),
                    ObjectId::from(11usize),
                ),
                7,
            );
            let mut toplevel = XdgToplevel::new(
                XdgToplevelId::from(12),
                Resolved::new(
                    crate::clients::ClientId::from_raw(1),
                    crate::protocol_generated::XdgSurfaceId::from(9),
                    ObjectId::from(9usize),
                ),
                7,
            );
            let geometry = crate::root_geometry::CommittedRootGeometry {
                layout_rect_global: RectI { x: 20, y: 20, width: 1240, height: 760 },
                root_pos: (25, 25),
                surface_bounds_local: Some(RectI { x: -5, y: -5, width: 1240, height: 760 }),
                logical_window_local: Some(RectI { x: 0, y: 0, width: 1230, height: 750 }),
                decoration_mode: Some(ToplevelDecorationMode::ServerSide),
                decoration_insets: crate::config::DecorationInsets {
                    top: 5,
                    right: 5,
                    bottom: 5,
                    left: 5,
                },
                tiled_edges: crate::tiling::EDGE_TOP | crate::tiling::EDGE_LEFT,
                fullscreen: false,
            };

            toplevel.schedule_decoration_mode(ToplevelDecorationMode::ServerSide);
            toplevel.send_tiled_layout_configure(
                &client,
                &state,
                &mut xdg_surface,
                geometry,
                crate::root_geometry::project_configure_geometry(
                    geometry,
                    ToplevelDecorationMode::ServerSide,
                    state.configured_decoration_insets(),
                ),
                false,
                &mut client_dispatcher,
            );

            let pending = xdg_surface
                .pending_configures
                .back()
                .expect("pending configure");
            let super::super::ConfigureState::Toplevel(configure) = &pending.state else {
                panic!("expected toplevel configure");
            };
            assert_eq!((configure.width, configure.height), (1230, 750));
            assert_eq!(configure.decoration_mode, ToplevelDecorationMode::ServerSide);
        }

        #[test]
        fn tiled_layout_configure_uses_frame_size_when_pending_mode_switches_to_client_side() {
            let state = crate::test_support::state_for_tests();
            let mut client =
                crate::clients::Client::new_for_tests(crate::clients::ClientId::from_raw(1));
            let mut dispatcher = crate::runner::Dispatcher::default();
            let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);
            let mut xdg_surface = XdgSurface::new(
                crate::protocol_generated::XdgSurfaceId::from(9),
                Resolved::new(
                    crate::clients::ClientId::from_raw(1),
                    crate::protocol_generated::XdgWmBaseId::from(10),
                    ObjectId::from(10usize),
                ),
                Resolved::new(
                    crate::clients::ClientId::from_raw(1),
                    crate::protocol_generated::WlSurfaceId::from(11),
                    ObjectId::from(11usize),
                ),
                7,
            );
            let mut toplevel = XdgToplevel::new(
                XdgToplevelId::from(12),
                Resolved::new(
                    crate::clients::ClientId::from_raw(1),
                    crate::protocol_generated::XdgSurfaceId::from(9),
                    ObjectId::from(9usize),
                ),
                7,
            );
            let geometry = crate::root_geometry::CommittedRootGeometry {
                layout_rect_global: RectI { x: 20, y: 20, width: 1240, height: 760 },
                root_pos: (25, 25),
                surface_bounds_local: Some(RectI { x: -5, y: -5, width: 1240, height: 760 }),
                logical_window_local: Some(RectI { x: 0, y: 0, width: 1230, height: 750 }),
                decoration_mode: Some(ToplevelDecorationMode::ServerSide),
                decoration_insets: crate::config::DecorationInsets {
                    top: 5,
                    right: 5,
                    bottom: 5,
                    left: 5,
                },
                tiled_edges: crate::tiling::EDGE_TOP | crate::tiling::EDGE_LEFT,
                fullscreen: false,
            };

            toplevel.schedule_decoration_mode(ToplevelDecorationMode::ClientSide);
            toplevel.send_tiled_layout_configure(
                &client,
                &state,
                &mut xdg_surface,
                geometry,
                crate::root_geometry::project_configure_geometry(
                    geometry,
                    ToplevelDecorationMode::ClientSide,
                    state.configured_decoration_insets(),
                ),
                false,
                &mut client_dispatcher,
            );

            let pending = xdg_surface
                .pending_configures
                .back()
                .expect("pending configure");
            let super::super::ConfigureState::Toplevel(configure) = &pending.state else {
                panic!("expected toplevel configure");
            };
            assert_eq!((configure.width, configure.height), (1240, 760));
            assert_eq!(configure.decoration_mode, ToplevelDecorationMode::ClientSide);
        }

        #[test]
        fn reset_on_unmap_restores_default_decoration_mode() {
            let mut toplevel = XdgToplevel::new(
                XdgToplevelId::from(9),
                Resolved::new(
                    crate::clients::ClientId::from_raw(1),
                    super::super::XdgSurfaceId::from(10),
                    ObjectId::from(10usize),
                ),
                7,
            );
            toplevel.set_decoration_mode(ToplevelDecorationMode::ClientSide);
            toplevel.set_window_geometry(Some(RectI { x: 1, y: 2, width: 3, height: 4 }));

            toplevel.reset_on_unmap();

            assert_eq!(toplevel.decoration_mode(), ToplevelDecorationMode::ClientSide);
            assert_eq!(toplevel.window_geometry(), None);
        }
    }
}

#[cfg(test)]
mod tests {
    use {
        super::*,
        crate::{
            clients::{Client, ClientId, ClientManagerAction, RawEvent},
            protocol_generated::{
                WaylandId, WlSurfaceId, XdgPopupId, XdgToplevelId, ZwlrLayerShellV1Id,
                ZwlrLayerSurfaceV1Id,
            },
            runner::Dispatcher,
            state::{ObjectId, State},
            test_support::{resolved_wl_surface as root, state_for_tests, test_output_drm0_wh},
            wayland_objects::{
                wl_surface::Role,
                xdg_popup::XdgPopup,
                xdg_positioner::PositionerState,
                xdg_surface::xdg_toplevel::XdgToplevel,
                zwlr_layer_surface_v1::{LayerShellLayer, ZwlrLayerSurfaceV1},
                zxdg_toplevel_decoration_v1::ZxdgToplevelDecorationV1,
            },
        },
    };

    fn insert_test_xdg_surface(
        state: &mut State,
        client_id: ClientId,
        wl_surface_id: WlSurfaceId,
        xdg_surface_id: XdgSurfaceId,
    ) -> (Resolved<WlSurfaceId>, Resolved<XdgSurfaceId>) {
        let (_, surface_object_id) =
            state.insert_object(WlSurface::new(wl_surface_id, client_id, 6));
        let wl_surface = Resolved::new(client_id, wl_surface_id, surface_object_id);
        state.register_root_surface(wl_surface);
        let (_, xdg_surface_object_id) = state.insert_object(XdgSurface::new(
            xdg_surface_id,
            Resolved::new(client_id, XdgWmBaseId::from(1), ObjectId::from(1usize)),
            wl_surface,
            7,
        ));
        let xdg_surface = Resolved::new(client_id, xdg_surface_id, xdg_surface_object_id);
        state
            .objects
            .get_mut(surface_object_id)
            .downcast_mut::<WlSurface>()
            .unwrap()
            .set_xdg_surface_impl(xdg_surface);
        (wl_surface, xdg_surface)
    }

    fn insert_test_popup(
        state: &mut State,
        client_id: ClientId,
        popup_id: XdgPopupId,
        popup_xdg_surface: Resolved<XdgSurfaceId>,
        parent_xdg_surface: Option<Resolved<XdgSurfaceId>>,
        geometry: RectI,
        window_geometry: RectI,
    ) -> Resolved<XdgPopupId> {
        let (_, popup_object_id) = state.insert_object(XdgPopup::new(
            popup_id,
            popup_xdg_surface,
            parent_xdg_surface,
            PositionerState::default(),
            7,
        ));
        let popup = Resolved::new(client_id, popup_id, popup_object_id);
        state
            .objects
            .get_mut(popup_object_id)
            .downcast_mut::<XdgPopup>()
            .unwrap()
            .apply_acked_configure(geometry);
        {
            let popup_xdg = state
                .objects
                .get_mut(popup_xdg_surface.object_id())
                .downcast_mut::<XdgSurface>()
                .unwrap();
            popup_xdg.test_set_current_window_geometry(Some(window_geometry));
            popup_xdg.test_set_popup_role(popup);
        }
        if let Some(parent_xdg_surface) = parent_xdg_surface {
            state
                .objects
                .get_mut(parent_xdg_surface.object_id())
                .downcast_mut::<XdgSurface>()
                .unwrap()
                .add_popup_child(popup);
        }
        popup
    }

    #[test]
    fn geometry_commit_updates_toplevel_window_geometry() {
        let mut state = state_for_tests();
        let output_id = state.insert_output(test_output_drm0_wh(120, 80));
        let workspace_id = state.active_workspace_for_output(output_id).unwrap();
        let mut client = Client::new_for_tests(ClientId::from_raw(77));
        let mut dispatcher = Dispatcher::default();
        let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);

        let wl_surface_id = WlSurfaceId::from(10u32);
        let xdg_surface_id = XdgSurfaceId::from(11u32);
        let toplevel_id = XdgToplevelId::from(12u32);

        let (surface_resolved, _) =
            client.insert_object(&mut state, WlSurface::new(wl_surface_id, client.client_id(), 6));
        let surface_object_id = surface_resolved.object_id();
        {
            let mut surface = state.objects.grab_object::<WlSurface>(surface_object_id);
            assert!(surface
                .set_role(Role::XdgSurface, &mut client_dispatcher)
                .is_ok());
        }
        state.register_root_surface(surface_resolved);

        let xdg_surface = XdgSurface {
            id: xdg_surface_id,
            wm_base: Resolved::new(
                client.client_id(),
                XdgWmBaseId::from(1),
                ObjectId::from(1usize),
            ),
            wl_surface: Resolved::new(client.client_id(), wl_surface_id, surface_object_id),
            wm_base_version: 7,
            role_kind: Some(RoleKind::Toplevel),
            role_object: RoleObject::None,
            initial_commit_complete: true,
            next_configure_serial: 1,
            pending_configures: VecDeque::new(),
            last_acked: None,
            pending_window_geometry: None,
            current_window_geometry: None,
            popup_children: Vec::new(),
        };
        let (xdg_surface_resolved, _) = client.insert_object(&mut state, xdg_surface);
        let xdg_surface_object_id = xdg_surface_resolved.object_id();
        {
            let mut surface = state.objects.grab_object::<WlSurface>(surface_object_id);
            surface.set_xdg_surface_impl(Resolved::new(
                client.client_id(),
                xdg_surface_id,
                xdg_surface_object_id,
            ));
            surface.test_map(ObjectId::from(950usize), 0, 0, 80, 60);
        }
        let (toplevel_resolved, _) = client.insert_object(
            &mut state,
            XdgToplevel::new(
                toplevel_id,
                Resolved::new(client.client_id(), xdg_surface_id, xdg_surface_object_id),
                7,
            ),
        );
        state.register_xdg_toplevel_role(xdg_surface_resolved, toplevel_resolved);
        state
            .objects
            .get_mut(xdg_surface_object_id)
            .downcast_mut::<XdgSurface>()
            .unwrap()
            .role_object = RoleObject::Toplevel(toplevel_resolved);

        assert_eq!(
            state.root_surface_workspace(root(&state, surface_object_id)),
            Some(workspace_id)
        );
        state.relayout_dirty_workspaces();
        let surface = state
            .objects
            .try_ref_object::<WlSurface>(surface_object_id)
            .unwrap();
        assert_eq!(surface.root_position(), (0, 0));

        {
            let xdg_surface = client.ref_object_mut(xdg_surface_id, &mut state);
            xdg_surface.pending_window_geometry =
                Some(RectI { x: 0, y: -12, width: 80, height: 72 });
        }

        {
            let mut xdg_surface = client.grab_object(xdg_surface_id, &mut state);
            let role_changes = xdg_surface
                .on_commit(
                    &mut client,
                    &mut state,
                    &mut client_dispatcher,
                    None,
                    false,
                    false,
                    None,
                )
                .expect("geometry-only commit should be accepted");
            xdg_surface.apply_commit_role_changes(
                &mut client,
                &mut state,
                &mut client_dispatcher,
                role_changes,
            );
        }

        let toplevel = state
            .objects
            .try_ref_object::<XdgToplevel>(toplevel_resolved.object_id())
            .unwrap();
        assert_eq!(toplevel.window_geometry(), Some(RectI { x: 0, y: -12, width: 80, height: 72 }));
    }

    #[test]
    fn reset_configure_state_clears_window_geometry() {
        let mut xdg_surface = XdgSurface::new(
            XdgSurfaceId::from(20),
            Resolved::new(ClientId::from_raw(1), XdgWmBaseId::from(21), ObjectId::from(21usize)),
            Resolved::new(ClientId::from_raw(1), WlSurfaceId::from(22), ObjectId::from(22usize)),
            7,
        );
        xdg_surface.pending_window_geometry = Some(RectI { x: 1, y: 2, width: 3, height: 4 });
        xdg_surface.current_window_geometry = Some(RectI { x: 5, y: 6, width: 7, height: 8 });

        xdg_surface.reset_configure_state();

        assert_eq!(xdg_surface.pending_window_geometry, None);
        assert_eq!(xdg_surface.current_window_geometry, None);
    }

    #[test]
    fn destroying_toplevel_before_decoration_disconnects_client_with_orphaned_error() {
        let mut state = state_for_tests();
        let mut client = Client::new_for_tests(ClientId::from_raw(88));
        let mut dispatcher = Dispatcher::default();
        let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);

        let wl_surface_id = WlSurfaceId::from(30u32);
        let xdg_surface_id = XdgSurfaceId::from(31u32);
        let toplevel_id = XdgToplevelId::from(32u32);
        let decoration_id = crate::protocol_generated::ZxdgToplevelDecorationV1Id::from(33u32);

        let (surface_resolved, _) =
            client.insert_object(&mut state, WlSurface::new(wl_surface_id, client.client_id(), 6));
        let surface_object_id = surface_resolved.object_id();
        {
            let mut surface = state.objects.grab_object::<WlSurface>(surface_object_id);
            assert!(surface
                .set_role(Role::XdgSurface, &mut client_dispatcher)
                .is_ok());
        }
        let (xdg_surface_resolved, _) = client.insert_object(
            &mut state,
            XdgSurface::new(
                xdg_surface_id,
                Resolved::new(client.client_id(), XdgWmBaseId::from(1), ObjectId::from(1usize)),
                Resolved::new(client.client_id(), wl_surface_id, surface_object_id),
                7,
            ),
        );
        let xdg_surface_object_id = xdg_surface_resolved.object_id();
        state
            .objects
            .get_mut(surface_object_id)
            .downcast_mut::<WlSurface>()
            .unwrap()
            .set_xdg_surface_impl(Resolved::new(
                client.client_id(),
                xdg_surface_id,
                xdg_surface_object_id,
            ));
        let (toplevel_resolved, _) = client.insert_object(
            &mut state,
            XdgToplevel::new(
                toplevel_id,
                Resolved::new(client.client_id(), xdg_surface_id, xdg_surface_object_id),
                7,
            ),
        );
        let toplevel_object_id = toplevel_resolved.object_id();
        state.register_xdg_toplevel_role(xdg_surface_resolved, toplevel_resolved);
        let (decoration_resolved, _) = client.insert_object(
            &mut state,
            ZxdgToplevelDecorationV1::new(decoration_id, toplevel_resolved),
        );
        let decoration_object_id = decoration_resolved.object_id();
        {
            let toplevel = state
                .objects
                .get_mut(toplevel_object_id)
                .downcast_mut::<XdgToplevel>()
                .unwrap();
            assert!(toplevel.set_decoration_object(Resolved::new(
                client.client_id(),
                decoration_id,
                decoration_object_id,
            )));
        }

        let destroy_request: RawRequest<'static> =
            (&*crate::clients::RawEvent::new(toplevel_id.into(), 0, 0).into_bytes())
                .try_into()
                .unwrap();
        {
            let mut toplevel = client.grab_object(toplevel_id, &mut state);
            toplevel.on(&mut client, &mut state, destroy_request, &mut client_dispatcher);
        }
        drop(client_dispatcher);

        assert!(!client.contains_object(toplevel_id, &state));
        assert!(client.contains_object(decoration_id, &state));
        assert!(dispatcher.test_has_queued_action::<crate::clients::ClientManagerAction>(
            |action| matches!(action, crate::clients::ClientManagerAction::Close { id } if *id == client.client_id())
        ));
        assert!(dispatcher.test_has_queued_action::<crate::io_uring::model::IoUringAction>(
            |action| matches!(action, crate::io_uring::model::IoUringAction::Sendmsg { .. })
        ));
    }

    #[test]
    fn set_min_size_defers_workspace_relayout_to_client_manager() {
        let mut state = state_for_tests();
        let output_id = state.insert_output(test_output_drm0_wh(120, 80));
        let workspace_id = state.active_workspace_for_output(output_id).unwrap();
        let mut client = Client::new_for_tests(ClientId::from_raw(89));
        let mut dispatcher = Dispatcher::default();
        let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);

        let wl_surface_id = WlSurfaceId::from(40u32);
        let xdg_surface_id = XdgSurfaceId::from(41u32);
        let toplevel_id = XdgToplevelId::from(42u32);

        let (surface_resolved, _) =
            client.insert_object(&mut state, WlSurface::new(wl_surface_id, client.client_id(), 6));
        let surface_object_id = surface_resolved.object_id();
        {
            let mut surface = state.objects.grab_object::<WlSurface>(surface_object_id);
            assert!(surface
                .set_role(Role::XdgSurface, &mut client_dispatcher)
                .is_ok());
        }
        state.register_root_surface(surface_resolved);

        let (xdg_surface_resolved, _) = client.insert_object(
            &mut state,
            XdgSurface {
                id: xdg_surface_id,
                wm_base: Resolved::new(
                    client.client_id(),
                    XdgWmBaseId::from(1),
                    ObjectId::from(1usize),
                ),
                wl_surface: Resolved::new(client.client_id(), wl_surface_id, surface_object_id),
                wm_base_version: 7,
                role_kind: Some(RoleKind::Toplevel),
                role_object: RoleObject::None,
                initial_commit_complete: true,
                next_configure_serial: 1,
                pending_configures: VecDeque::new(),
                last_acked: None,
                pending_window_geometry: None,
                current_window_geometry: None,
                popup_children: Vec::new(),
            },
        );
        let xdg_surface_object_id = xdg_surface_resolved.object_id();
        {
            let mut surface = state.objects.grab_object::<WlSurface>(surface_object_id);
            surface.set_xdg_surface_impl(Resolved::new(
                client.client_id(),
                xdg_surface_id,
                xdg_surface_object_id,
            ));
            surface.test_map(ObjectId::from(951usize), 0, 0, 80, 60);
        }
        let (toplevel_resolved, _) = client.insert_object(
            &mut state,
            XdgToplevel::new(
                toplevel_id,
                Resolved::new(client.client_id(), xdg_surface_id, xdg_surface_object_id),
                7,
            ),
        );
        state.register_xdg_toplevel_role(xdg_surface_resolved, toplevel_resolved);
        state
            .objects
            .get_mut(xdg_surface_object_id)
            .downcast_mut::<XdgSurface>()
            .unwrap()
            .role_object = RoleObject::Toplevel(toplevel_resolved);

        assert_eq!(
            state.root_surface_workspace(root(&state, surface_object_id)),
            Some(workspace_id)
        );
        state.relayout_dirty_workspaces();
        assert!(state.is_tiled_root_surface(root(&state, surface_object_id)));

        let set_min_size_request: RawRequest<'static> = (&*{
            let mut event = RawEvent::new(toplevel_id.into(), 8, 2);
            event.int(120);
            event.int(90);
            event.into_bytes()
        })
            .try_into()
            .unwrap();

        {
            let mut toplevel = client.grab_object(toplevel_id, &mut state);
            toplevel.on(&mut client, &mut state, set_min_size_request, &mut client_dispatcher);
        }

        assert!(dispatcher.test_has_queued_action::<ClientManagerAction>(|action| matches!(
            action,
            ClientManagerAction::PublishWorkspaceStateChanged { affected_outputs }
                if affected_outputs == &vec![output_id]
        )));
    }

    #[test]
    fn popup_scene_origin_tracks_parent_move_and_nested_popup_offsets() {
        let mut state = state_for_tests();
        let client_id = ClientId::from_raw(7);
        let (parent_wl, parent_xdg) = insert_test_xdg_surface(
            &mut state,
            client_id,
            WlSurfaceId::from(60),
            XdgSurfaceId::from(61),
        );
        let (popup_wl, popup_xdg) = insert_test_xdg_surface(
            &mut state,
            client_id,
            WlSurfaceId::from(62),
            XdgSurfaceId::from(63),
        );
        let (nested_wl, nested_xdg) = insert_test_xdg_surface(
            &mut state,
            client_id,
            WlSurfaceId::from(64),
            XdgSurfaceId::from(65),
        );
        state
            .objects
            .get_mut(parent_wl.object_id())
            .downcast_mut::<WlSurface>()
            .unwrap()
            .test_map(ObjectId::from(960usize), 640, 0, 640, 800);
        state
            .objects
            .get_mut(parent_wl.object_id())
            .downcast_mut::<WlSurface>()
            .unwrap()
            .set_root_position(640, 0);
        state
            .objects
            .get_mut(popup_wl.object_id())
            .downcast_mut::<WlSurface>()
            .unwrap()
            .test_map(ObjectId::from(961usize), 0, 0, 299, 457);
        state
            .objects
            .get_mut(nested_wl.object_id())
            .downcast_mut::<WlSurface>()
            .unwrap()
            .test_map(ObjectId::from(962usize), 0, 0, 110, 140);

        let popup = insert_test_popup(
            &mut state,
            client_id,
            XdgPopupId::from(66),
            popup_xdg,
            Some(parent_xdg),
            RectI { x: 387, y: 40, width: 253, height: 411 },
            RectI { x: 23, y: 21, width: 253, height: 411 },
        );
        insert_test_popup(
            &mut state,
            client_id,
            XdgPopupId::from(67),
            nested_xdg,
            Some(popup_xdg),
            RectI { x: 50, y: 60, width: 100, height: 120 },
            RectI { x: 5, y: 6, width: 100, height: 120 },
        );

        assert_eq!(state.root_surface_for_surface(popup_wl), Some(parent_wl));
        assert_eq!(state.root_surface_for_surface(nested_wl), Some(parent_wl));
        assert_eq!(state.surface_scene_origin(popup_wl), Some((1004, 19)));
        assert_eq!(state.surface_scene_origin(nested_wl), Some((1049, 73)));

        state
            .objects
            .get_mut(popup.object_id())
            .downcast_mut::<XdgPopup>()
            .unwrap()
            .apply_acked_configure(RectI { x: 420, y: 50, width: 253, height: 411 });
        state
            .objects
            .get_mut(parent_wl.object_id())
            .downcast_mut::<WlSurface>()
            .unwrap()
            .set_root_position(700, 10);

        assert_eq!(state.surface_scene_origin(popup_wl), Some((1097, 39)));
        assert_eq!(state.surface_scene_origin(nested_wl), Some((1142, 93)));
    }

    #[test]
    fn layer_parent_popup_scene_origin_uses_layer_root_transform() {
        let mut state = state_for_tests();
        let client_id = ClientId::from_raw(9);
        let (_, layer_root_object_id) =
            state.insert_object(WlSurface::new(WlSurfaceId::from(70), client_id, 6));
        let layer_root = Resolved::new(client_id, WlSurfaceId::from(70), layer_root_object_id);
        state.register_root_surface(layer_root);
        state
            .objects
            .get_mut(layer_root_object_id)
            .downcast_mut::<WlSurface>()
            .unwrap()
            .test_map(ObjectId::from(970usize), 50, 60, 200, 40);
        state
            .objects
            .get_mut(layer_root_object_id)
            .downcast_mut::<WlSurface>()
            .unwrap()
            .set_root_position(50, 60);

        let (_, layer_object_id) = state.insert_object(ZwlrLayerSurfaceV1::new(
            ZwlrLayerSurfaceV1Id::from(71),
            Resolved::new(client_id, ZwlrLayerShellV1Id::from(1), ObjectId::from(1usize)),
            5,
            layer_root,
            None,
            LayerShellLayer::Top,
            "panel".to_string(),
        ));
        let layer_surface =
            Resolved::new(client_id, ZwlrLayerSurfaceV1Id::from(71), layer_object_id);
        state
            .objects
            .get_mut(layer_root_object_id)
            .downcast_mut::<WlSurface>()
            .unwrap()
            .set_layer_surface_impl(layer_surface);

        let (popup_wl, popup_xdg) = insert_test_xdg_surface(
            &mut state,
            client_id,
            WlSurfaceId::from(72),
            XdgSurfaceId::from(73),
        );
        state
            .objects
            .get_mut(popup_wl.object_id())
            .downcast_mut::<WlSurface>()
            .unwrap()
            .test_map(ObjectId::from(971usize), 0, 0, 40, 50);
        state
            .objects
            .get_mut(popup_wl.object_id())
            .downcast_mut::<WlSurface>()
            .unwrap()
            .set_popup_parent_layer_surface(Some(layer_surface));

        let (_, popup_object_id) = state.insert_object(XdgPopup::new(
            XdgPopupId::from(74),
            popup_xdg,
            None,
            PositionerState::default(),
            7,
        ));
        let popup = Resolved::new(client_id, XdgPopupId::from(74), popup_object_id);
        state
            .objects
            .get_mut(popup_object_id)
            .downcast_mut::<XdgPopup>()
            .unwrap()
            .assign_layer_parent(layer_surface)
            .expect("layer parent assignment should succeed");
        state
            .objects
            .get_mut(popup_object_id)
            .downcast_mut::<XdgPopup>()
            .unwrap()
            .apply_acked_configure(RectI { x: 12, y: 18, width: 30, height: 40 });
        state
            .objects
            .get_mut(popup_xdg.object_id())
            .downcast_mut::<XdgSurface>()
            .unwrap()
            .test_set_popup_role(popup);
        state
            .objects
            .get_mut(popup_xdg.object_id())
            .downcast_mut::<XdgSurface>()
            .unwrap()
            .test_set_current_window_geometry(Some(RectI { x: 2, y: 3, width: 30, height: 40 }));
        state
            .objects
            .get_mut(layer_object_id)
            .downcast_mut::<ZwlrLayerSurfaceV1>()
            .unwrap()
            .add_popup_child(popup);

        assert_eq!(state.root_surface_for_surface(popup_wl), Some(layer_root));
        assert_eq!(state.surface_scene_origin(popup_wl), Some((60, 75)));
    }
}
