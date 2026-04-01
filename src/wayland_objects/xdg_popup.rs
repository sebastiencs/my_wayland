use crate::{
    clients::{Client, ClientDispatcher, EventWriter, RawRequest},
    object::{Object, Parser, PostAction},
    protocol_generated::{
        xdg_popup, xdg_wm_base, WaylandId, WlSeatId, XdgPopupId, XdgSurfaceId, ZwlrLayerSurfaceV1Id,
    },
    region::RectI,
    state::State,
    wayland_objects::{
        wl_display::WlDisplay, xdg_positioner::PositionerState, xdg_surface::XdgSurface,
        zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
    },
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PopupParent {
    None,
    XdgSurface(XdgSurfaceId),
    LayerSurface(ZwlrLayerSurfaceV1Id),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AssignLayerParentError {
    AlreadyHasParent,
    AlreadyMapped,
}

#[derive(Debug)]
pub struct XdgPopup {
    id: XdgPopupId,
    xdg_surface_id: XdgSurfaceId,
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
        xdg_surface_id: XdgSurfaceId,
        parent_surface_id: Option<XdgSurfaceId>,
        positioner: PositionerState,
        wm_base_version: u32,
    ) -> Self {
        let current_geometry =
            positioner
                .geometry()
                .unwrap_or(RectI { x: 0, y: 0, width: 0, height: 0 });
        Self {
            id,
            xdg_surface_id,
            wm_base_version,
            parent: parent_surface_id
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

    pub fn xdg_surface_id(&self) -> XdgSurfaceId {
        self.xdg_surface_id
    }

    pub fn parent_surface_id(&self) -> Option<XdgSurfaceId> {
        match self.parent {
            PopupParent::XdgSurface(id) => Some(id),
            _ => None,
        }
    }

    pub fn parent_layer_surface_id(&self) -> Option<ZwlrLayerSurfaceV1Id> {
        match self.parent {
            PopupParent::LayerSurface(id) => Some(id),
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
        layer_surface_id: ZwlrLayerSurfaceV1Id,
    ) -> Result<(), AssignLayerParentError> {
        if !matches!(self.parent, PopupParent::None) {
            return Err(AssignLayerParentError::AlreadyHasParent);
        }
        if self.mapped {
            return Err(AssignLayerParentError::AlreadyMapped);
        }
        self.parent = PopupParent::LayerSurface(layer_surface_id);
        Ok(())
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
            xdg_surface.xdg_wm_base_id().into(),
            code as u32,
            message,
        );
    }

    fn computed_geometry(&self, client: &Client, state: &State) -> RectI {
        match self.parent {
            PopupParent::XdgSurface(parent_surface_id) => {
                let Some(parent_object_id) = client.object_id_for_wayland(parent_surface_id) else {
                    return self.positioner.geometry().unwrap_or(self.current_geometry);
                };
                let Some(parent_surface) =
                    state.objects.try_ref_object::<XdgSurface>(parent_object_id)
                else {
                    return self.positioner.geometry().unwrap_or(self.current_geometry);
                };
                let parent_geometry = parent_surface.parent_geometry(client, state);
                let Some(mut geometry) = self.positioner.geometry_in_bounds(RectI {
                    x: 0,
                    y: 0,
                    width: parent_geometry.width,
                    height: parent_geometry.height,
                }) else {
                    return self.current_geometry;
                };
                geometry.x += parent_geometry.x;
                geometry.y += parent_geometry.y;
                geometry
            }
            PopupParent::LayerSurface(parent_layer_surface_id) => {
                let Some(parent_layer_surface) = state.objects.iter().find_map(|(_, object)| {
                    let layer_surface = object.downcast_ref::<ZwlrLayerSurfaceV1>()?;
                    (layer_surface.id() == parent_layer_surface_id.into()).then_some(layer_surface)
                }) else {
                    return self.positioner.geometry().unwrap_or(self.current_geometry);
                };
                let parent_geometry = parent_layer_surface.popup_anchor_geometry(state);
                let Some(mut geometry) = self.positioner.geometry_in_bounds(RectI {
                    x: 0,
                    y: 0,
                    width: parent_geometry.width,
                    height: parent_geometry.height,
                }) else {
                    return self.current_geometry;
                };
                geometry.x += parent_geometry.x;
                geometry.y += parent_geometry.y;
                geometry
            }
            PopupParent::None => self.positioner.geometry().unwrap_or(self.current_geometry),
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
        xdg_surface.queue_popup_configure(geometry, dispatcher);
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
        &mut self,
        client: &mut Client,
        state: &mut State,
        req: RawRequest,
        dispatcher: &mut ClientDispatcher,
    ) {
        use xdg_popup::Request::*;

        match client.parse::<Self>(req).unwrap() {
            Destroy => {
                let mut xdg_surface = client.grab_object(self.xdg_surface_id, state);
                let mut fallback_focus = None;
                match self.parent {
                    PopupParent::XdgSurface(parent_surface_id) => {
                        let mut parent_surface = client.grab_object(parent_surface_id, state);
                        if parent_surface.has_popup_child(self.id)
                            && !parent_surface.is_topmost_popup(self.id)
                        {
                            self.send_wm_base_error(
                                &xdg_surface,
                                client,
                                dispatcher,
                                xdg_wm_base::Error::NotTheTopmostPopup,
                                "xdg_popup.destroy requires the topmost popup",
                            );
                            return;
                        }
                        fallback_focus = Some(crate::input::SurfaceFocus {
                            client_id: client.client_id(),
                            surface_id: parent_surface.wl_surface_id(),
                        });
                        parent_surface.remove_popup_child(self.id);
                    }
                    PopupParent::LayerSurface(parent_surface_id) => {
                        let mut parent_surface = client.grab_object(parent_surface_id, state);
                        if parent_surface.has_popup_child(self.id)
                            && !parent_surface.is_topmost_popup(self.id)
                        {
                            self.send_error(
                                client,
                                dispatcher,
                                xdg_popup::Error::InvalidGrab,
                                "xdg_popup.destroy requires the topmost popup",
                            );
                            return;
                        }
                        fallback_focus = Some(crate::input::SurfaceFocus {
                            client_id: client.client_id(),
                            surface_id: parent_surface.surface_id(),
                        });
                        parent_surface.remove_popup_child(self.id);
                    }
                    PopupParent::None => {}
                }
                crate::seat::remove_popup_grab(state, dispatcher, self.id, fallback_focus);
                xdg_surface.destroy_popup_role(self.id);
                self.delete_self(client, state, dispatcher);
            }
            Grab { seat, serial } => {
                if self.mapped {
                    self.send_error(
                        client,
                        dispatcher,
                        xdg_popup::Error::InvalidGrab,
                        "xdg_popup.grab must happen before the popup is mapped",
                    );
                    return;
                }
                if let PopupParent::XdgSurface(parent_surface_id) = self.parent {
                    let invalid_parent = {
                        let parent_surface_object_id = client.object_id(parent_surface_id);
                        let parent_surface = state
                            .objects
                            .try_ref_object::<XdgSurface>(parent_surface_object_id)
                            .unwrap();
                        parent_surface.is_popup_parent()
                            && !parent_surface.popup_has_explicit_grab(client, state)
                    };
                    if invalid_parent {
                        let parent_surface = client.ref_object(parent_surface_id, state);
                        self.send_wm_base_error(
                            parent_surface,
                            client,
                            dispatcher,
                            xdg_wm_base::Error::InvalidPopupParent,
                            "nested grabbing popups require a grabbing popup parent",
                        );
                        return;
                    }
                }
                if !client.object_is(seat, state) {
                    client.protocol_violation_disconnect(
                        dispatcher,
                        self.id.into(),
                        crate::protocol_generated::wl_display::Error::InvalidObject as u32,
                        "xdg_popup.grab requires a live wl_seat",
                    );
                    return;
                }
                let parent_wl_surface_id = {
                    match self.parent {
                        PopupParent::XdgSurface(parent_surface_id) => {
                            let parent_surface: &XdgSurface =
                                client.ref_object(parent_surface_id, state);
                            parent_surface.wl_surface_id()
                        }
                        PopupParent::LayerSurface(parent_surface_id) => {
                            let parent_surface: &ZwlrLayerSurfaceV1 =
                                client.ref_object(parent_surface_id, state);
                            parent_surface.surface_id()
                        }
                        PopupParent::None => {
                            self.send_error(
                                client,
                                dispatcher,
                                xdg_popup::Error::InvalidGrab,
                                "xdg_popup.grab requires a parent surface",
                            );
                            return;
                        }
                    }
                };
                self.explicit_grab = true;
                let popup_surface_id = {
                    let xdg_surface = client.ref_object(self.xdg_surface_id, state);
                    xdg_surface.wl_surface_id()
                };
                dispatcher.dispatch_post_action(PostXdgPopup::RequestPopupGrab {
                    popup_id: self.id,
                    client_id: client.client_id(),
                    parent_wl_surface_id,
                    serial,
                    popup_surface_id,
                });
            }
            Reposition { positioner, token } => {
                if !client.object_is(positioner, state) {
                    client.protocol_violation_disconnect(
                        dispatcher,
                        self.id.into(),
                        crate::protocol_generated::wl_display::Error::InvalidObject as u32,
                        "xdg_popup.reposition requires a live xdg_positioner",
                    );
                    return;
                }
                let positioner = state
                    .objects
                    .try_ref_object::<crate::wayland_objects::xdg_positioner::XdgPositioner>(
                        client.object_id(positioner),
                    )
                    .unwrap()
                    .snapshot();
                if !positioner.is_complete() {
                    let xdg_surface = client.ref_object(self.xdg_surface_id, state);
                    self.send_wm_base_error(
                        xdg_surface,
                        client,
                        dispatcher,
                        xdg_wm_base::Error::InvalidPositioner,
                        "xdg_popup.reposition requires a complete xdg_positioner",
                    );
                    return;
                }
                self.positioner = positioner;
                self.pending_reposition_token = Some(token);
                let mut xdg_surface = client.grab_object(self.xdg_surface_id, state);
                self.send_configure_sequence(&mut xdg_surface, client, state, dispatcher);
            }
        }
    }
}

enum PostXdgPopup {
    RequestPopupGrab {
        popup_id: XdgPopupId,
        client_id: crate::clients::ClientId,
        parent_wl_surface_id: crate::protocol_generated::WlSurfaceId,
        serial: u32,
        popup_surface_id: crate::protocol_generated::WlSurfaceId,
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
            PostXdgPopup::RequestPopupGrab {
                popup_id,
                client_id,
                parent_wl_surface_id,
                serial,
                popup_surface_id,
            } => {
                let Some(popup_object_id) = client.try_object_id(popup_id, state) else {
                    return;
                };
                let Some(popup_parent) = state
                    .objects
                    .try_ref_object::<XdgPopup>(popup_object_id)
                    .map(|popup| popup.parent)
                else {
                    return;
                };

                if crate::seat::request_popup_grab(
                    state,
                    dispatcher,
                    client_id,
                    parent_wl_surface_id,
                    serial,
                    popup_id,
                    popup_surface_id,
                ) {
                    return;
                }

                match popup_parent {
                    PopupParent::XdgSurface(parent_surface_id) => {
                        if client.contains_object(parent_surface_id, state) {
                            let mut parent_surface: crate::state::GrabbedObject<XdgSurface> =
                                client.grab_object(parent_surface_id, state);
                            parent_surface.remove_popup_child(popup_id);
                        }
                    }
                    PopupParent::LayerSurface(parent_surface_id) => {
                        if client.contains_object(parent_surface_id, state) {
                            let mut parent_surface: crate::state::GrabbedObject<
                                ZwlrLayerSurfaceV1,
                            > = client.grab_object(parent_surface_id, state);
                            parent_surface.remove_popup_child(popup_id);
                        }
                    }
                    PopupParent::None => {}
                }

                if client.contains_object(popup_id, state) {
                    let mut popup: crate::state::GrabbedObject<XdgPopup> =
                        client.grab_object(popup_id, state);
                    popup.explicit_grab = false;
                    popup.dismiss(dispatcher);
                }
            }
        }
    }
}
