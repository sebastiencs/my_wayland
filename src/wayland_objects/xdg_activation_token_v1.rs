use crate::{
    clients::{Client, ClientDispatcher, EventWriter, RawRequest, Resolved},
    object::{Object, Parser, RequestResult},
    protocol_generated::{
        wl_display, xdg_activation_token_v1, Constants, WaylandId, WlSeatId, WlSurfaceId,
        XdgActivationTokenV1Id,
    },
    seat::validate_activation_serial,
    state::{GrabbedObject, State},
    wayland_objects::wl_display::WlDisplay,
};

#[derive(Debug)]
pub struct XdgActivationTokenV1 {
    id: XdgActivationTokenV1Id,
    committed: bool,
    seat: Option<Resolved<WlSeatId>>,
    serial: Option<u32>,
    surface: Option<Resolved<WlSurfaceId>>,
    app_id: Option<String>,
}

impl XdgActivationTokenV1 {
    pub fn new(id: XdgActivationTokenV1Id) -> Self {
        Self {
            id,
            committed: false,
            seat: None,
            serial: None,
            surface: None,
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
        mut self: GrabbedObject<Self>,
        client: &mut Client,
        state: &mut State,
        req: RawRequest,
        dispatcher: &mut ClientDispatcher,
    ) -> RequestResult {
        use xdg_activation_token_v1::Request::*;

        match client.parse::<Self>(req, dispatcher)? {
            SetSerial { serial, seat } => {
                if self.committed {
                    self.already_used(dispatcher);
                    return Ok(());
                }
                let seat = client.resolve_or_disconnect(seat, state, dispatcher)?;
                self.serial = Some(serial);
                self.seat = Some(seat);
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
                let surface = client.resolve_or_disconnect(surface, state, dispatcher)?;
                self.surface = Some(surface);
            }
            Commit => {
                if self.committed {
                    self.already_used(dispatcher);
                    return Ok(());
                }
                self.committed = true;

                let tracked = match (&self.seat, &self.serial, &self.surface) {
                    (Some(seat), Some(serial), Some(surface)) => {
                        validate_activation_serial(state, client, *seat, *surface, *serial)
                    }
                    _ => false,
                };

                let token = if tracked {
                    state.activation_tokens.issue_tracked(
                        client.client_id(),
                        self.seat,
                        self.serial,
                        self.surface,
                        self.app_id.clone(),
                    )
                } else {
                    log::debug!(
                        "rejecting activation token commit client={:?} seat={:?} serial={:?} surface={:?}",
                        client.client_id(),
                        self.seat,
                        self.serial,
                        self.surface
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
