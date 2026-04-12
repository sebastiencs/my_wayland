use crate::{
    clients::{Client, ClientDispatcher, EventWriter, RawRequest, Resolved},
    input::SurfaceFocus,
    object::{Object, Parser, PostAction, RequestResult},
    protocol_generated::{
        xdg_popup, xdg_wm_base, WaylandId, WlSeatId, WlSurfaceId, XdgPopupId, XdgSurfaceId,
        ZwlrLayerSurfaceV1Id,
    },
    region::RectI,
    state::{GrabbedObject, State},
    wayland_objects::{
        wl_display::WlDisplay, xdg_positioner::PositionerState, xdg_surface::XdgSurface,
        zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
    },
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PopupParent {
    None,
    XdgSurface(Resolved<XdgSurfaceId>),
    LayerSurface(Resolved<ZwlrLayerSurfaceV1Id>),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AssignLayerParentError {
    AlreadyHasParent,
    AlreadyMapped,
}

#[derive(Debug)]
pub struct XdgPopup {
    id: XdgPopupId,
    xdg_surface: Resolved<XdgSurfaceId>,
    /// `xdg_wm_base` version for this shell (from the factory that created the popup).
    wm_base_version: u32,
    parent: PopupParent,
    positioner: PositionerState,
    current_geometry: RectI,
    pending_reposition_token: Option<u32>,
    explicit_grab: bool,
    mapped: bool,
}

impl XdgPopup {
    pub fn new(
        id: XdgPopupId,
        xdg_surface: Resolved<XdgSurfaceId>,
        parent_surface: Option<Resolved<XdgSurfaceId>>,
        positioner: PositionerState,
        wm_base_version: u32,
    ) -> Self {
        let current_geometry =
            positioner
                .geometry()
                .unwrap_or(RectI { x: 0, y: 0, width: 0, height: 0 });
        Self {
            id,
            xdg_surface,
            wm_base_version,
            parent: parent_surface
                .map(PopupParent::XdgSurface)
                .unwrap_or(PopupParent::None),
            positioner,
            current_geometry,
            pending_reposition_token: None,
            explicit_grab: false,
            mapped: false,
        }
    }

    pub fn has_explicit_grab(&self) -> bool {
        self.explicit_grab
    }

    pub fn xdg_surface(&self) -> Resolved<XdgSurfaceId> {
        self.xdg_surface
    }

    pub fn parent_surface(&self) -> Option<Resolved<XdgSurfaceId>> {
        match self.parent {
            PopupParent::XdgSurface(id) => Some(id),
            _ => None,
        }
    }

    pub fn parent_layer_surface(&self) -> Option<Resolved<ZwlrLayerSurfaceV1Id>> {
        match self.parent {
            PopupParent::LayerSurface(layer) => Some(layer),
            _ => None,
        }
    }

    pub fn mark_mapped(&mut self) {
        self.mapped = true;
    }

    pub fn mark_unmapped(&mut self) {
        self.mapped = false;
    }

    pub fn apply_acked_configure(&mut self, geometry: RectI) {
        self.current_geometry = geometry;
    }

    pub fn send_popup_done(&self, dispatcher: &mut ClientDispatcher) {
        self.send_event(xdg_popup::Event::PopupDone, dispatcher);
    }

    pub fn assign_layer_parent(
        &mut self,
        layer_surface: Resolved<ZwlrLayerSurfaceV1Id>,
    ) -> Result<(), AssignLayerParentError> {
        if !matches!(self.parent, PopupParent::None) {
            return Err(AssignLayerParentError::AlreadyHasParent);
        }
        if self.mapped {
            return Err(AssignLayerParentError::AlreadyMapped);
        }
        self.parent = PopupParent::LayerSurface(layer_surface);
        Ok(())
    }

    fn send_error(
        &self,
        client: &Client,
        dispatcher: &mut ClientDispatcher,
        code: xdg_popup::Error,
        message: impl AsRef<str>,
    ) {
        client.send_protocol_error(dispatcher, self.id.into(), code as u32, message);
    }

    fn send_wm_base_error(
        &self,
        xdg_surface: &XdgSurface,
        client: &Client,
        dispatcher: &mut ClientDispatcher,
        code: xdg_wm_base::Error,
        message: impl AsRef<str>,
    ) {
        client.send_protocol_error(
            dispatcher,
            xdg_surface.xdg_wm_base().wayland_id().into(),
            code as u32,
            message,
        );
    }

    fn computed_geometry(&self, client: &Client, state: &State) -> RectI {
        let popup_id = self.id;
        let positioner_dbg = format!("{:?}", self.positioner);
        match self.parent {
            PopupParent::XdgSurface(parent_surface_id) => {
                if !parent_surface_id.exists(state) {
                    let r = self.positioner.geometry().unwrap_or(self.current_geometry);
                    log::debug!(
                        "AAA xdg_popup computed_geometry popup={:?} parent=xdg_surface(missing) positioner={} -> {:?}",
                        popup_id,
                        positioner_dbg,
                        r
                    );
                    return r;
                }
                let parent_surface = parent_surface_id.rref(state);
                let parent_geometry = parent_surface.parent_geometry(client, state);
                let parent_wl = parent_surface.wl_surface();
                let parent_wl_root = parent_wl
                    .try_ref(state)
                    .map(|s| s.root_position())
                    .unwrap_or((0, 0));
                let bounds = RectI {
                    x: 0,
                    y: 0,
                    width: parent_geometry.width,
                    height: parent_geometry.height,
                };
                let Some(mut geometry) = self.positioner.geometry_in_bounds(bounds) else {
                    log::debug!(
                        "AAA xdg_popup computed_geometry popup={:?} parent_xdg={:?} parent_geometry={:?} parent_wl_root_pos=({},{}) bounds_for_positioner={:?} positioner={} geometry_in_bounds=None -> fallback current_geometry={:?}",
                        popup_id,
                        parent_surface_id,
                        parent_geometry,
                        parent_wl_root.0,
                        parent_wl_root.1,
                        bounds,
                        positioner_dbg,
                        self.current_geometry
                    );
                    return self.current_geometry;
                };
                geometry.x += parent_geometry.x;
                geometry.y += parent_geometry.y;
                log::debug!(
                    "AAA xdg_popup computed_geometry popup={:?} parent_xdg={:?} parent_geometry={:?} parent_wl_root_pos=({},{}) bounds_for_positioner={:?} positioner={} -> {:?}",
                    popup_id,
                    parent_surface_id,
                    parent_geometry,
                    parent_wl_root.0,
                    parent_wl_root.1,
                    bounds,
                    positioner_dbg,
                    geometry
                );
                geometry
            }
            PopupParent::LayerSurface(parent_layer_surface_id) => {
                let Some(parent_layer_surface) = parent_layer_surface_id.try_ref(state) else {
                    let r = self.positioner.geometry().unwrap_or(self.current_geometry);
                    log::debug!(
                        "AAA xdg_popup computed_geometry popup={:?} parent=layer_surface(missing) positioner={} -> {:?}",
                        popup_id,
                        positioner_dbg,
                        r
                    );
                    return r;
                };
                let parent_geometry = parent_layer_surface.popup_anchor_geometry(state);
                let bounds = RectI {
                    x: 0,
                    y: 0,
                    width: parent_geometry.width,
                    height: parent_geometry.height,
                };
                let Some(mut geometry) = self.positioner.geometry_in_bounds(bounds) else {
                    log::debug!(
                        "AAA xdg_popup computed_geometry popup={:?} parent_layer={:?} parent_anchor_geometry={:?} bounds_for_positioner={:?} positioner={} geometry_in_bounds=None -> fallback current_geometry={:?}",
                        popup_id,
                        parent_layer_surface_id,
                        parent_geometry,
                        bounds,
                        positioner_dbg,
                        self.current_geometry
                    );
                    return self.current_geometry;
                };
                geometry.x += parent_geometry.x;
                geometry.y += parent_geometry.y;
                log::debug!(
                    "AAA xdg_popup computed_geometry popup={:?} parent_layer={:?} parent_anchor_geometry={:?} bounds_for_positioner={:?} positioner={} -> {:?}",
                    popup_id,
                    parent_layer_surface_id,
                    parent_geometry,
                    bounds,
                    positioner_dbg,
                    geometry
                );
                geometry
            }
            PopupParent::None => {
                let r = self.positioner.geometry().unwrap_or(self.current_geometry);
                log::debug!(
                    "AAA xdg_popup computed_geometry popup={:?} parent=None positioner={} -> {:?}",
                    popup_id,
                    positioner_dbg,
                    r
                );
                r
            }
        }
    }

    fn send_configure_sequence(
        &mut self,
        xdg_surface: &mut XdgSurface,
        client: &Client,
        state: &State,
        dispatcher: &mut ClientDispatcher,
    ) {
        let geometry = self.computed_geometry(client, state);
        let popup_wl = xdg_surface.wl_surface();
        let popup_wl_root = popup_wl
            .try_ref(state)
            .map(|s| s.root_position())
            .unwrap_or((0, 0));
        let parent_wl_root = self
            .parent_surface()
            .and_then(|p| p.try_ref(state))
            .map(|px| {
                let pw = px.wl_surface();
                pw.try_ref(state)
                    .map(|s| s.root_position())
                    .unwrap_or((0, 0))
            });
        let parent_committed = self.parent_surface().and_then(|p| {
            let pw = p.rref(state).wl_surface();
            let root = state.root_surface_for_surface(pw)?;
            state
                .committed_root_geometry(root)
                .map(|g| (root, g.root_pos, g.content_rect_global(), g.layout_rect_global))
        });
        log::debug!(
            "AAA xdg_popup send_configure_sequence popup={:?} xdg_surface={:?} popup_wl={:?} popup_wl_root_pos=({},{}) parent_wl_root_pos={:?} parent_committed_root=(root={:?} root_pos={:?} content_rect_global={:?} layout_rect_global={:?}) mapped={} explicit_grab={} pending_reposition_token_pre_take={:?} -> send xdg_popup.configure x,y,w,h={},{},{},{}",
            self.id,
            xdg_surface.wl_surface(),
            popup_wl,
            popup_wl_root.0,
            popup_wl_root.1,
            parent_wl_root,
            parent_committed.map(|(r, rp, c, l)| r),
            parent_committed.map(|(_, rp, _, _)| rp),
            parent_committed.map(|(_, _, c, _)| c),
            parent_committed.map(|(_, _, _, l)| l),
            self.mapped,
            self.explicit_grab,
            self.pending_reposition_token,
            geometry.x,
            geometry.y,
            geometry.width,
            geometry.height
        );
        if let Some(token) = self.pending_reposition_token.take() {
            if self.wm_base_version >= xdg_popup::REPOSITIONED_SINCE {
                self.send_event(xdg_popup::Event::Repositioned { token }, dispatcher);
            }
        }
        self.send_event(
            xdg_popup::Event::Configure {
                x: geometry.x,
                y: geometry.y,
                width: geometry.width,
                height: geometry.height,
            },
            dispatcher,
        );
        let serial = xdg_surface.queue_popup_configure(geometry, dispatcher);
        log::debug!(
            "AAA xdg_popup send_configure_sequence queued xdg_surface.configure serial={} geometry={:?}",
            serial,
            geometry
        );
    }

    pub fn send_initial_configure(
        &mut self,
        xdg_surface: &mut XdgSurface,
        client: &Client,
        state: &State,
        dispatcher: &mut ClientDispatcher,
    ) {
        self.send_configure_sequence(xdg_surface, client, state, dispatcher);
    }

    pub fn dismiss(&mut self, dispatcher: &mut ClientDispatcher) {
        self.mapped = false;
        self.explicit_grab = false;
        self.send_event(xdg_popup::Event::PopupDone, dispatcher);
    }
}

impl Object for XdgPopup {
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
        use xdg_popup::Request::*;

        match client.parse::<Self>(req).unwrap() {
            Destroy => {
                let mut xdg_surface = self.xdg_surface.grab(state);
                let mut fallback_focus = None;
                let popup = client.resolve(self.id, state).unwrap();
                match self.parent {
                    PopupParent::XdgSurface(parent_surface_id) => {
                        let mut parent_surface = parent_surface_id.grab(state);
                        if parent_surface.has_popup_child(popup)
                            && !parent_surface.is_topmost_popup(popup)
                        {
                            self.send_wm_base_error(
                                &xdg_surface,
                                client,
                                dispatcher,
                                xdg_wm_base::Error::NotTheTopmostPopup,
                                "xdg_popup.destroy requires the topmost popup",
                            );
                            return Ok(());
                        }
                        fallback_focus = Some(SurfaceFocus::new(parent_surface.wl_surface()));
                        parent_surface.remove_popup_child(popup);
                    }
                    PopupParent::LayerSurface(parent_surface_id) => {
                        let mut parent_surface = parent_surface_id.grab(state);
                        if parent_surface.has_popup_child(popup)
                            && !parent_surface.is_topmost_popup(popup)
                        {
                            self.send_error(
                                client,
                                dispatcher,
                                xdg_popup::Error::InvalidGrab,
                                "xdg_popup.destroy requires the topmost popup",
                            );
                            return Ok(());
                        }
                        fallback_focus = Some(SurfaceFocus::new(parent_surface.surface()));
                        parent_surface.remove_popup_child(popup);
                    }
                    PopupParent::None => {}
                }
                drop(xdg_surface);
                dispatcher.dispatch_post_action(PostXdgPopup::DestroyPopup {
                    popup,
                    xdg_surface: self.xdg_surface,
                    fallback_focus,
                });
            }
            Grab { seat, serial } => {
                if self.mapped {
                    self.send_error(
                        client,
                        dispatcher,
                        xdg_popup::Error::InvalidGrab,
                        "xdg_popup.grab must happen before the popup is mapped",
                    );
                    return Ok(());
                }
                if let PopupParent::XdgSurface(parent_surface_id) = self.parent {
                    let invalid_parent = {
                        let parent_surface = parent_surface_id.try_ref(state).unwrap();
                        parent_surface.is_popup_parent()
                            && !parent_surface.popup_has_explicit_grab(client, state)
                    };
                    if invalid_parent {
                        let parent_surface = parent_surface_id.rref(state);
                        self.send_wm_base_error(
                            parent_surface,
                            client,
                            dispatcher,
                            xdg_wm_base::Error::InvalidPopupParent,
                            "nested grabbing popups require a grabbing popup parent",
                        );
                        return Ok(());
                    }
                }
                let _seat = client.resolve_or_disconnect(seat, state, dispatcher)?;
                let parent_wl_surface = match self.parent {
                    PopupParent::XdgSurface(parent_surface_id) => {
                        parent_surface_id.rref(state).wl_surface()
                    }
                    PopupParent::LayerSurface(parent_surface_id) => {
                        parent_surface_id.rref(state).surface()
                    }
                    PopupParent::None => {
                        self.send_error(
                            client,
                            dispatcher,
                            xdg_popup::Error::InvalidGrab,
                            "xdg_popup.grab requires a parent surface",
                        );
                        return Ok(());
                    }
                };
                self.explicit_grab = true;
                let popup = client.resolve(self.id, state).unwrap();
                let popup_surface = self.xdg_surface.rref(state).wl_surface();
                dispatcher.dispatch_post_action(PostXdgPopup::RequestPopupGrab {
                    popup,
                    client_id: client.client_id(),
                    parent_wl_surface,
                    serial,
                    popup_surface,
                });
            }
            Reposition { positioner, token } => {
                let positioner = client.resolve_or_disconnect(positioner, state, dispatcher)?;
                let positioner = positioner.rref(state).snapshot();
                if !positioner.is_complete() {
                    let xdg_surface = self.xdg_surface.rref(state);
                    self.send_wm_base_error(
                        xdg_surface,
                        client,
                        dispatcher,
                        xdg_wm_base::Error::InvalidPositioner,
                        "xdg_popup.reposition requires a complete xdg_positioner",
                    );
                    return Ok(());
                }
                self.positioner = positioner;
                self.pending_reposition_token = Some(token);
                let mut xdg_surface = self.xdg_surface.grab(state);
                self.send_configure_sequence(&mut xdg_surface, client, state, dispatcher);
            }
        }
        Ok(())
    }
}

enum PostXdgPopup {
    DestroyPopup {
        popup: Resolved<XdgPopupId>,
        xdg_surface: Resolved<XdgSurfaceId>,
        fallback_focus: Option<crate::input::SurfaceFocus>,
    },
    RequestPopupGrab {
        popup: Resolved<XdgPopupId>,
        client_id: crate::clients::ClientId,
        parent_wl_surface: Resolved<WlSurfaceId>,
        serial: u32,
        popup_surface: Resolved<WlSurfaceId>,
    },
}

impl PostAction for PostXdgPopup {
    fn execute(
        self: Box<Self>,
        client: &mut Client,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
    ) {
        match *self {
            PostXdgPopup::DestroyPopup { popup, xdg_surface, fallback_focus } => {
                if popup.exists(state) {
                    crate::seat::remove_popup_grab(state, dispatcher, popup, fallback_focus);
                }
                if xdg_surface.exists(state) {
                    let mut xdg_surface = xdg_surface.grab(state);
                    xdg_surface.destroy_popup_role(popup);
                }
                client.delete_id(
                    &WlDisplay::default(),
                    state,
                    dispatcher,
                    popup.wayland_id().into(),
                );
            }
            PostXdgPopup::RequestPopupGrab {
                popup,
                client_id,
                parent_wl_surface,
                serial,
                popup_surface,
            } => {
                if !popup.exists(state) {
                    return;
                }
                let Some(popup_parent) = popup.try_ref(state).map(|p| p.parent) else {
                    return;
                };

                if crate::seat::request_popup_grab(
                    state,
                    dispatcher,
                    client_id,
                    parent_wl_surface,
                    serial,
                    popup,
                    popup_surface,
                ) {
                    return;
                }

                match popup_parent {
                    PopupParent::XdgSurface(parent_surface_id) => {
                        if parent_surface_id.exists(state) {
                            let mut parent_surface = parent_surface_id.ref_mut(state);
                            parent_surface.remove_popup_child(popup);
                        }
                    }
                    PopupParent::LayerSurface(parent_surface_id) => {
                        if parent_surface_id.exists(state) {
                            let mut parent_surface = parent_surface_id.ref_mut(state);
                            parent_surface.remove_popup_child(popup);
                        }
                    }
                    PopupParent::None => {}
                }

                if popup.exists(state) {
                    let mut popup = popup.ref_mut(state);
                    popup.explicit_grab = false;
                    popup.dismiss(dispatcher);
                }
            }
        }
    }
}
