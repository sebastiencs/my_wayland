use crate::{
    clients::{Client, ClientDispatcher, EventWriter, RawRequest},
    object::{Object, Parser},
    protocol_generated::{xdg_popup, xdg_wm_base, WaylandId, WlSeatId, XdgPopupId, XdgSurfaceId},
    region::RectI,
    state::State,
    wayland_objects::{
        wl_display::WlDisplay, xdg_positioner::PositionerState, xdg_surface::XdgSurface,
    },
};

#[derive(Debug)]
pub struct XdgPopup {
    id: XdgPopupId,
    xdg_surface_id: XdgSurfaceId,
    /// `xdg_wm_base` version for this shell (from the factory that created the popup).
    wm_base_version: u32,
    parent_surface_id: Option<XdgSurfaceId>,
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
            parent_surface_id,
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
        self.parent_surface_id
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
        if let Some(parent_surface_id) = self.parent_surface_id {
            let Some(parent_object_id) = client.object_id_for_wayland(parent_surface_id.into())
            else {
                return self.positioner.geometry().unwrap_or(self.current_geometry);
            };
            let Some(parent_surface) = state.objects.try_ref_object::<XdgSurface>(parent_object_id)
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
            return geometry;
        }
        self.positioner.geometry().unwrap_or(self.current_geometry)
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

    fn dismiss(
        &mut self,
        xdg_surface: &mut XdgSurface,
        client: &mut Client,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
    ) {
        self.mapped = false;
        xdg_surface.destroy_popup_role(self.id);
        self.send_event(xdg_popup::Event::PopupDone, dispatcher);
        self.delete_self(client, state, dispatcher);
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
                if let Some(parent_surface_id) = self.parent_surface_id {
                    let mut parent_surface = client.grab_object(parent_surface_id, state);
                    if !parent_surface.is_topmost_popup(self.id) {
                        self.send_wm_base_error(
                            &xdg_surface,
                            client,
                            dispatcher,
                            xdg_wm_base::Error::NotTheTopmostPopup,
                            "xdg_popup.destroy requires the topmost popup",
                        );
                        return;
                    }
                    fallback_focus = Some(crate::seat::SurfaceFocus {
                        client_id: client.client_id(),
                        surface_id: parent_surface.wl_surface_id(),
                    });
                    parent_surface.remove_popup_child(self.id);
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
                if let Some(parent_surface_id) = self.parent_surface_id {
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
                let Some(parent_surface_id) = self.parent_surface_id else {
                    self.send_error(
                        client,
                        dispatcher,
                        xdg_popup::Error::InvalidGrab,
                        "xdg_popup.grab requires a parent surface",
                    );
                    return;
                };
                let parent_wl_surface_id = {
                    let parent_surface = client.ref_object(parent_surface_id, state);
                    parent_surface.wl_surface_id()
                };
                self.explicit_grab = true;
                let popup_surface_id = {
                    let xdg_surface = client.ref_object(self.xdg_surface_id, state);
                    xdg_surface.wl_surface_id()
                };
                if !crate::seat::request_popup_grab(
                    state,
                    dispatcher,
                    client.client_id(),
                    parent_wl_surface_id,
                    serial,
                    self.id,
                    popup_surface_id,
                ) {
                    self.explicit_grab = false;
                    if let Some(parent_surface_id) = self.parent_surface_id {
                        if client.contains_object(parent_surface_id, state) {
                            let mut parent_surface = client.grab_object(parent_surface_id, state);
                            parent_surface.remove_popup_child(self.id);
                        }
                    }
                    let mut xdg_surface = client.grab_object(self.xdg_surface_id, state);
                    self.dismiss(&mut xdg_surface, client, state, dispatcher);
                }
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
