use std::collections::{BTreeSet, VecDeque};

use crate::{
    clients::{Client, ClientDispatcher, EventWriter, RawRequest},
    object::{Object, Parser},
    protocol_generated::{
        xdg_surface, xdg_wm_base, WaylandId, WlSurfaceId, XdgPopupId, XdgSurfaceId, XdgToplevelId,
        XdgWmBaseId,
    },
    region::RectI,
    state::State,
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
    Toplevel(XdgToplevelId),
    Popup(XdgPopupId),
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

#[derive(Debug)]
pub struct XdgSurface {
    id: XdgSurfaceId,
    wm_base_id: XdgWmBaseId,
    wl_surface: WlSurfaceId,
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
    popup_children: Vec<XdgPopupId>,
}

impl XdgSurface {
    pub fn new(
        id: XdgSurfaceId,
        wm_base_id: XdgWmBaseId,
        wl_surface: WlSurfaceId,
        wm_base_version: u32,
    ) -> Self {
        Self {
            id,
            wm_base_id,
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

    pub(crate) fn xdg_wm_base_id(&self) -> XdgWmBaseId {
        self.wm_base_id
    }

    pub(crate) fn wl_surface_id(&self) -> WlSurfaceId {
        self.wl_surface
    }

    pub(crate) fn popup_role_id(&self) -> Option<XdgPopupId> {
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
        let Some(popup_id) = self.popup_role_id() else {
            return false;
        };
        let Some(popup_object_id) = client.object_id_for_wayland(popup_id.into()) else {
            return false;
        };
        state
            .objects
            .try_get_ref_of::<XdgPopup>(popup_object_id)
            .is_some_and(XdgPopup::has_explicit_grab)
    }

    pub(crate) fn is_toplevel_parent(&self) -> bool {
        matches!(self.role_object, RoleObject::Toplevel(_))
    }

    pub(crate) fn parent_geometry(&self, client: &Client, state: &State) -> RectI {
        if let Some(geometry) = self.current_window_geometry {
            return geometry;
        }
        let Some(surface_object_id) = client.object_id_for_wayland(self.wl_surface.into()) else {
            return RectI { x: 0, y: 0, width: 0, height: 0 };
        };
        let Some(surface) = state.objects.try_get_ref_of::<WlSurface>(surface_object_id) else {
            return RectI { x: 0, y: 0, width: 0, height: 0 };
        };
        let Some(size) = surface.logical_size() else {
            return RectI { x: 0, y: 0, width: 0, height: 0 };
        };
        RectI { x: 0, y: 0, width: size.width, height: size.height }
    }

    fn send_wm_base_error(
        &self,
        client: &Client,
        dispatcher: &mut ClientDispatcher,
        code: xdg_wm_base::Error,
        message: impl AsRef<str>,
    ) {
        client.send_protocol_error(dispatcher, self.wm_base_id.into(), code as u32, message);
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
        if let Some(mut wl_surface_object) =
            state.objects.try_grab(client.object_id(self.wl_surface))
        {
            if let Some(wl_surface) = wl_surface_object.downcast_mut::<WlSurface>() {
                wl_surface.clear_xdg_surface_role(self.id);
            }
        }
        if let Some(mut wm_base_object) = state.objects.try_grab(client.object_id(self.wm_base_id))
        {
            if let Some(wm_base) = wm_base_object.downcast_mut::<XdgWmBase>() {
                wm_base.unregister_surface(self.id);
            }
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
        dispatcher: &mut ClientDispatcher,
    ) -> u32 {
        let serial = self.next_serial();
        self.pending_configures.push_back(PendingConfigure {
            serial,
            state: ConfigureState::Toplevel(ToplevelConfigureState { width, height, states }),
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

    pub fn on_commit(
        &mut self,
        client: &mut Client,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
        has_pending_buffer_attach: bool,
        is_null_buffer_commit: bool,
    ) -> bool {
        if self.pending_window_geometry.is_some() {
            self.current_window_geometry = self.pending_window_geometry.take();
        }

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
                    return false;
                }
                RoleObject::Toplevel(id) => {
                    if has_pending_buffer_attach {
                        self.send_surface_error(
                            client,
                            dispatcher,
                            xdg_surface::Error::UnconfiguredBuffer,
                            "initial xdg_toplevel commit must not attach a buffer",
                        );
                        return false;
                    }
                    let mut xdg_toplevel = client.grab_object(id, state);
                    xdg_toplevel.send_initial_configure(client, state, self, dispatcher);
                    self.initial_commit_complete = true;
                    return true;
                }
                RoleObject::Popup(id) => {
                    if has_pending_buffer_attach {
                        self.send_surface_error(
                            client,
                            dispatcher,
                            xdg_surface::Error::UnconfiguredBuffer,
                            "initial xdg_popup commit must not attach a buffer",
                        );
                        return false;
                    }
                    let mut popup = client.grab_object(id, state);
                    popup.send_initial_configure(self, client, state, dispatcher);
                    self.initial_commit_complete = true;
                    return true;
                }
            }
        }

        if !self.validate_committed_state(client, dispatcher, has_pending_buffer_attach) {
            return false;
        }

        if has_pending_buffer_attach {
            if let RoleObject::Popup(id) = self.role_object {
                let mut popup = client.grab_object(id, state);
                popup.mark_mapped();
            }
        }
        if is_null_buffer_commit {
            if let RoleObject::Toplevel(id) = self.role_object {
                let mut toplevel = client.grab_object(id, state);
                toplevel.reset_on_unmap();
            }
            if let RoleObject::Popup(id) = self.role_object {
                let mut popup = client.grab_object(id, state);
                popup.mark_unmapped();
            }
            self.reset_configure_state();
        }

        true
    }

    pub(crate) fn add_popup_child(&mut self, popup_id: XdgPopupId) {
        self.popup_children.push(popup_id);
    }

    pub(crate) fn remove_popup_child(&mut self, popup_id: XdgPopupId) {
        self.popup_children
            .retain(|candidate| *candidate != popup_id);
    }

    pub(crate) fn is_topmost_popup(&self, popup_id: XdgPopupId) -> bool {
        self.popup_children.last().copied() == Some(popup_id)
    }

    pub(crate) fn destroy_toplevel_role(&mut self, toplevel_id: XdgToplevelId) {
        if self.role_object == RoleObject::Toplevel(toplevel_id) {
            self.role_object = RoleObject::None;
            self.reset_configure_state();
        }
    }

    pub(crate) fn destroy_popup_role(&mut self, popup_id: XdgPopupId) {
        if self.role_object == RoleObject::Popup(popup_id) {
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
        &mut self,
        client: &mut Client,
        state: &mut State,
        req: RawRequest,
        dispatcher: &mut ClientDispatcher,
    ) {
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
                    return;
                }
                self.delete_self(client, state, dispatcher);
            }
            GetToplevel { id } => {
                if self
                    .set_role_kind(RoleKind::Toplevel, client, dispatcher)
                    .is_err()
                {
                    return;
                }
                self.role_object = RoleObject::Toplevel(id);

                let xdg_toplevel =
                    xdg_toplevel::XdgToplevel::new(id, self.id, self.wm_base_version);
                xdg_toplevel.send_capabilities(dispatcher);
                client.insert_object(state, xdg_toplevel);
            }
            GetPopup { id, parent, positioner } => {
                if !client.object_is(positioner, state) {
                    client.protocol_violation_disconnect(
                        dispatcher,
                        self.id.into(),
                        crate::protocol_generated::wl_display::Error::InvalidObject as u32,
                        "xdg_surface.get_popup requires a live xdg_positioner",
                    );
                    return;
                }

                let parent = match u32::from(parent) {
                    0 => {
                        self.send_wm_base_error(
                            client,
                            dispatcher,
                            xdg_wm_base::Error::InvalidPopupParent,
                            "popup parent must be provided for xdg_surface.get_popup",
                        );
                        return;
                    }
                    _ => {
                        if !client.object_is(parent, state) {
                            client.protocol_violation_disconnect(
                                dispatcher,
                                self.id.into(),
                                crate::protocol_generated::wl_display::Error::InvalidObject as u32,
                                "xdg_surface.get_popup requires a live parent xdg_surface",
                            );
                            return;
                        }
                        Some(parent)
                    }
                };

                let positioner_state = state
                    .objects
                    .try_get_ref_of::<XdgPositioner>(client.object_id(positioner))
                    .unwrap()
                    .snapshot();
                if !positioner_state.is_complete() {
                    self.send_wm_base_error(
                        client,
                        dispatcher,
                        xdg_wm_base::Error::InvalidPositioner,
                        "xdg_surface.get_popup requires a complete xdg_positioner",
                    );
                    return;
                }

                if let Some(parent_surface_id) = parent {
                    let valid_parent = {
                        let parent_surface_object_id = client.object_id(parent_surface_id);
                        let parent_surface = state
                            .objects
                            .try_get_ref_of::<Self>(parent_surface_object_id)
                            .unwrap();
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
                        return;
                    }
                    let mut parent_surface = client.grab_object(parent_surface_id, state);
                    parent_surface.add_popup_child(id);
                }

                if self
                    .set_role_kind(RoleKind::Popup, client, dispatcher)
                    .is_err()
                {
                    if let Some(parent_surface_id) = parent {
                        let mut parent_surface = client.grab_object(parent_surface_id, state);
                        parent_surface.remove_popup_child(id);
                    }
                    return;
                }
                self.role_object = RoleObject::Popup(id);
                let popup = XdgPopup::new(id, self.id, parent, positioner_state);
                client.insert_object(state, popup);
            }
            SetWindowGeometry { x, y, width, height } => {
                let Some(window_geometry) = RectI::new(x, y, width, height) else {
                    self.send_surface_error(
                        client,
                        dispatcher,
                        xdg_surface::Error::InvalidSize,
                        "xdg_surface.set_window_geometry requires positive dimensions",
                    );
                    return;
                };
                if window_geometry.is_empty() {
                    self.send_surface_error(
                        client,
                        dispatcher,
                        xdg_surface::Error::InvalidSize,
                        "xdg_surface.set_window_geometry requires positive dimensions",
                    );
                    return;
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
                    return;
                };

                let acked = self.pending_configures[position].clone();
                self.pending_configures.drain(..=position);
                self.last_acked = Some(acked.clone());

                match acked.state {
                    ConfigureState::Toplevel(ToplevelConfigureState { width, height, states }) => {
                        if let RoleObject::Toplevel(id) = self.role_object {
                            let mut toplevel = client.grab_object(id, state);
                            toplevel.apply_acked_configure(width, height, states);
                        }
                    }
                    ConfigureState::Popup(PopupConfigureState { geometry }) => {
                        if let RoleObject::Popup(id) = self.role_object {
                            let mut popup = client.grab_object(id, state);
                            popup.apply_acked_configure(geometry);
                        }
                    }
                }
            }
        }
    }
}

pub mod xdg_toplevel {
    use std::collections::BTreeSet;

    use crate::{
        clients::{Client, ClientDispatcher, EventWriter, RawRequest},
        object::{Object, Parser},
        protocol_generated::{
            xdg_toplevel as proto_xdg_toplevel, Array, WaylandId, WlOutputId, WlSeatId,
            XdgToplevelId,
        },
        state::State,
        wayland_objects::{wl_display::WlDisplay, wl_surface::WlSurface},
    };

    use super::XdgSurface;

    #[derive(Debug)]
    pub struct XdgToplevel {
        id: XdgToplevelId,
        xdg_surface_id: super::XdgSurfaceId,
        wm_base_version: u32,
        title: String,
        app_id: String,
        parent: Option<XdgToplevelId>,
        min_size: Option<(i32, i32)>,
        max_size: Option<(i32, i32)>,
        requested_fullscreen: bool,
        requested_maximized: bool,
        current_width: i32,
        current_height: i32,
        current_states: BTreeSet<proto_xdg_toplevel::State>,
    }

    impl XdgToplevel {
        pub fn new(
            id: XdgToplevelId,
            xdg_surface_id: super::XdgSurfaceId,
            wm_base_version: u32,
        ) -> Self {
            Self {
                id,
                xdg_surface_id,
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
            states.extend(self.current_states.iter().cloned());
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

        fn configure_bounds(&self, client: &Client, state: &State) -> Option<(i32, i32)> {
            let xdg_surface_object_id = client.object_id_for_wayland(self.xdg_surface_id.into())?;
            let xdg_surface = state
                .objects
                .try_get_ref_of::<XdgSurface>(xdg_surface_object_id)?;
            let wl_surface_object_id =
                client.object_id_for_wayland(xdg_surface.wl_surface_id().into())?;
            let active_output_id = state
                .objects
                .try_get_ref_of::<WlSurface>(wl_surface_object_id)
                .and_then(WlSurface::active_output_id)
                .or_else(|| state.active_output_id())?;
            let output = state.outputs.try_get(active_output_id)?;
            Some(output.logical_size())
        }

        fn send_configure_bounds(
            &self,
            client: &Client,
            state: &State,
            dispatcher: &mut ClientDispatcher,
        ) {
            if self.wm_base_version < 4 {
                return;
            }
            let Some((width, height)) = self.configure_bounds(client, state) else {
                return;
            };
            self.send_event(
                proto_xdg_toplevel::Event::ConfigureBounds { width, height },
                dispatcher,
            );
        }

        pub(crate) fn schedule_configure(
            &mut self,
            client: &Client,
            state: &State,
            xdg_surface: &mut XdgSurface,
            dispatcher: &mut ClientDispatcher,
        ) {
            self.send_configure_bounds(client, state, dispatcher);
            self.send_configure(dispatcher);
            xdg_surface.queue_toplevel_configure(
                self.current_width,
                self.current_height,
                self.effective_states(),
                dispatcher,
            );
        }

        pub fn send_initial_configure(
            &mut self,
            client: &Client,
            state: &State,
            xdg_surface: &mut XdgSurface,
            dispatcher: &mut ClientDispatcher,
        ) {
            self.schedule_configure(client, state, xdg_surface, dispatcher);
        }

        pub fn apply_acked_configure(&mut self, width: i32, height: i32, states: Vec<u32>) {
            self.current_width = width;
            self.current_height = height;
            self.current_states = states.into_iter().filter_map(decode_state).collect();
        }

        pub fn xdg_surface_id(&self) -> super::XdgSurfaceId {
            self.xdg_surface_id
        }

        pub fn toplevel_id(&self) -> XdgToplevelId {
            self.id
        }

        pub fn send_interactive_resize_configure(
            &mut self,
            client: &Client,
            state: &State,
            xdg_surface: &mut XdgSurface,
            width: i32,
            height: i32,
            resizing: bool,
            dispatcher: &mut ClientDispatcher,
        ) {
            let mut states = self.effective_states();
            if resizing {
                let resizing_flag = proto_xdg_toplevel::State::Resizing as u32;
                if !states.contains(&resizing_flag) {
                    states.push(resizing_flag);
                }
            } else {
                states.retain(|state| *state != proto_xdg_toplevel::State::Resizing as u32);
            }
            self.send_configure_bounds(client, state, dispatcher);
            self.send_event(
                proto_xdg_toplevel::Event::Configure {
                    width,
                    height,
                    states: Array(states.clone()),
                },
                dispatcher,
            );
            xdg_surface.queue_toplevel_configure(width, height, states, dispatcher);
        }

        pub fn reset_on_unmap(&mut self) {
            self.parent = None;
            self.requested_fullscreen = false;
            self.requested_maximized = false;
            self.current_width = 0;
            self.current_height = 0;
            self.current_states.clear();
        }

        /// `wm_capabilities` exists only since xdg_wm_base version 5; older clients (e.g. wev) must
        /// not receive wire opcode 3 or libwayland aborts with a NULL listener.
        pub fn send_capabilities(&self, dispatcher: &mut ClientDispatcher) {
            if self.wm_base_version < 5 {
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

        fn validate_parent(
            &self,
            parent: Option<XdgToplevelId>,
            client: &Client,
            state: &State,
        ) -> bool {
            let Some(mut current) = parent else {
                return true;
            };
            if current == self.id {
                return false;
            }
            loop {
                if current == self.id {
                    return false;
                }
                let Some(object_id) = client.object_id_for_wayland(current.into()) else {
                    return true;
                };
                let Some(toplevel) = state.objects.try_get_ref_of::<Self>(object_id) else {
                    return true;
                };
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
            &mut self,
            client: &mut Client,
            state: &mut State,
            req: RawRequest,
            dispatcher: &mut ClientDispatcher,
        ) {
            use proto_xdg_toplevel::Request::*;

            match client.parse::<Self>(req).unwrap() {
                Destroy => {
                    let mut xdg_surface = client.grab_object(self.xdg_surface_id, state);
                    xdg_surface.destroy_toplevel_role(self.id);
                    self.delete_self(client, state, dispatcher);
                }
                SetParent { parent } => {
                    let parent = match u32::from(parent) {
                        0 => None,
                        _ => {
                            if !client.object_is(parent, state) {
                                client.protocol_violation_disconnect(
                                    dispatcher,
                                    self.id.into(),
                                    crate::protocol_generated::wl_display::Error::InvalidObject
                                        as u32,
                                    "xdg_toplevel.set_parent requires a live xdg_toplevel",
                                );
                                return;
                            }
                            Some(parent)
                        }
                    };
                    if !self.validate_parent(parent, client, state) {
                        self.send_error(
                            client,
                            dispatcher,
                            proto_xdg_toplevel::Error::InvalidParent,
                            "xdg_toplevel parent would create a cycle",
                        );
                        return;
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
                    if !client.object_is(seat, state) {
                        client.protocol_violation_disconnect(
                            dispatcher,
                            self.id.into(),
                            crate::protocol_generated::wl_display::Error::InvalidObject as u32,
                            "xdg_toplevel.move requires a live wl_seat",
                        );
                        return;
                    }
                    let xdg_surface = client.get_object(self.xdg_surface_id, state);
                    let surface_id = xdg_surface.wl_surface_id();
                    let surface_object_id = client.object_id(surface_id);
                    let Some(surface) =
                        state.objects.try_get_ref_of::<WlSurface>(surface_object_id)
                    else {
                        return;
                    };
                    if !crate::seat::request_toplevel_move(
                        state,
                        client.client_id(),
                        surface_id,
                        serial,
                        surface_object_id,
                        surface.root_position(),
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
                        return;
                    }
                    if decode_resize_edge(edges).is_none() {
                        self.send_error(
                            client,
                            dispatcher,
                            proto_xdg_toplevel::Error::InvalidResizeEdge,
                            "xdg_toplevel.resize received an unknown resize edge",
                        );
                        return;
                    }
                    if !client.object_is(seat, state) {
                        client.protocol_violation_disconnect(
                            dispatcher,
                            self.id.into(),
                            crate::protocol_generated::wl_display::Error::InvalidObject as u32,
                            "xdg_toplevel.resize requires a live wl_seat",
                        );
                        return;
                    }
                    let xdg_surface = client.get_object(self.xdg_surface_id, state);
                    let surface_id = xdg_surface.wl_surface_id();
                    let surface_object_id = client.object_id(surface_id);
                    let Some(surface) =
                        state.objects.try_get_ref_of::<WlSurface>(surface_object_id)
                    else {
                        return;
                    };
                    let initial_size = surface.logical_size().map(|size| (size.width, size.height));
                    let Some(initial_size) = initial_size else {
                        log::debug!("xdg_toplevel.resize ignored for unmapped surface");
                        return;
                    };
                    if !crate::seat::request_toplevel_resize(
                        state,
                        client.client_id(),
                        surface_id,
                        serial,
                        surface_object_id,
                        self.id,
                        edges,
                        surface.root_position(),
                        initial_size,
                    ) {
                        log::debug!(
                            "xdg_toplevel.resize ignored due to invalid pointer grab serial {}",
                            serial
                        );
                    }
                }
                SetMaxSize { width, height } => {
                    if !self.store_size_pair(client, dispatcher, width, height, false) {
                        return;
                    }
                }
                SetMinSize { width, height } => {
                    if !self.store_size_pair(client, dispatcher, width, height, true) {
                        return;
                    }
                }
                SetMaximized => {
                    self.requested_maximized = true;
                    let mut xdg_surface = client.grab_object(self.xdg_surface_id, state);
                    self.schedule_configure(client, state, &mut xdg_surface, dispatcher);
                }
                UnsetMaximized => {
                    self.requested_maximized = false;
                    let mut xdg_surface = client.grab_object(self.xdg_surface_id, state);
                    self.schedule_configure(client, state, &mut xdg_surface, dispatcher);
                }
                SetFullscreen { output: _ } => {
                    self.requested_fullscreen = true;
                    let mut xdg_surface = client.grab_object(self.xdg_surface_id, state);
                    self.schedule_configure(client, state, &mut xdg_surface, dispatcher);
                }
                UnsetFullscreen => {
                    self.requested_fullscreen = false;
                    let mut xdg_surface = client.grab_object(self.xdg_surface_id, state);
                    self.schedule_configure(client, state, &mut xdg_surface, dispatcher);
                }
                SetMinimized => {
                    log::debug!("xdg_toplevel.set_minimized has no compositor integration yet");
                }
            }
        }
    }
}
