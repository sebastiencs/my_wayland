use crate::{
    clients::{Client, ClientDispatcher, EventWriter, RawRequest},
    object::{Object, Parser},
    protocol_generated::{
        wl_display, xdg_activation_token_v1, Constants, WaylandId, WlSeatId, WlSurfaceId,
        XdgActivationTokenV1Id,
    },
    seat::validate_activation_serial,
    state::State,
    wayland_objects::wl_display::WlDisplay,
};

#[derive(Debug)]
pub struct XdgActivationTokenV1 {
    id: XdgActivationTokenV1Id,
    committed: bool,
    seat_id: Option<WlSeatId>,
    serial: Option<u32>,
    surface_id: Option<WlSurfaceId>,
    app_id: Option<String>,
}

impl XdgActivationTokenV1 {
    pub fn new(id: XdgActivationTokenV1Id) -> Self {
        Self {
            id,
            committed: false,
            seat_id: None,
            serial: None,
            surface_id: None,
            app_id: None,
        }
    }

    fn already_used(&self, dispatcher: &mut ClientDispatcher) {
        self.send_error(
            dispatcher,
            xdg_activation_token_v1::Error::AlreadyUsed as u32,
            "xdg_activation_token_v1 has already been committed",
        );
    }

    fn send_done(&self, token: String, dispatcher: &mut ClientDispatcher) {
        self.send_event(xdg_activation_token_v1::Event::Done { token }, dispatcher);
    }
}

impl Object for XdgActivationTokenV1 {
    fn id(&self) -> WaylandId {
        self.id.into()
    }

    fn on(
        &mut self,
        client: &mut Client,
        state: &mut State,
        req: RawRequest,
        dispatcher: &mut ClientDispatcher,
    ) -> crate::object::RequestResult {
        use xdg_activation_token_v1::Request::*;

        match client.parse::<Self>(req).unwrap() {
            SetSerial { serial, seat } => {
                if self.committed {
                    self.already_used(dispatcher);
                    return Ok(());
                }
                if !client.object_is(seat, state) {
                    client.protocol_violation_disconnect(
                        dispatcher,
                        self.id.into(),
                        wl_display::Error::InvalidObject as u32,
                        "xdg_activation_token_v1.set_serial requires a live wl_seat",
                    );
                    return Ok(());
                }
                self.serial = Some(serial);
                self.seat_id = Some(seat);
            }
            SetAppId { app_id } => {
                if self.committed {
                    self.already_used(dispatcher);
                    return Ok(());
                }
                self.app_id = Some(app_id);
            }
            SetSurface { surface } => {
                if self.committed {
                    self.already_used(dispatcher);
                    return Ok(());
                }
                if !client.object_is(surface, state) {
                    client.protocol_violation_disconnect(
                        dispatcher,
                        self.id.into(),
                        wl_display::Error::InvalidObject as u32,
                        "xdg_activation_token_v1.set_surface requires a live wl_surface",
                    );
                    return Ok(());
                }
                self.surface_id = Some(surface);
            }
            Commit => {
                if self.committed {
                    self.already_used(dispatcher);
                    return Ok(());
                }
                self.committed = true;

                let tracked = match (self.seat_id, self.serial, self.surface_id) {
                    (Some(seat_id), Some(serial), Some(surface_id)) => {
                        validate_activation_serial(state, client, seat_id, surface_id, serial)
                    }
                    _ => false,
                };

                let token = if tracked {
                    state.activation_tokens.issue_tracked(
                        client.client_id(),
                        self.seat_id,
                        self.serial,
                        self.surface_id,
                        self.app_id.clone(),
                    )
                } else {
                    log::debug!(
                        "rejecting activation token commit client={:?} seat={:?} serial={:?} surface={:?}",
                        client.client_id(),
                        self.seat_id,
                        self.serial,
                        self.surface_id
                    );
                    state.activation_tokens.issue_untracked()
                };
                self.send_done(token, dispatcher);
            }
            Destroy => {
                client.delete_id(&WlDisplay::default(), state, dispatcher, self.id.into());
            }
        }
        Ok(())
    }
}
