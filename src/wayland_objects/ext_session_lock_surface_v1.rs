//! `ext_session_lock_surface_v1`. The first configure is sent on creation.
//! Commit-time buffer and size checks belong to the next phase.

use crate::{
    clients::{Client, ClientDispatcher, EventWriter, RawRequest, Resolved},
    object::{Object, Parser, RequestResult},
    protocol_generated::{
        ext_session_lock_surface_v1, Constants, ExtSessionLockSurfaceV1Id, ExtSessionLockV1Id,
        WaylandId, WlSurfaceId,
    },
    state::{GrabbedObject, OutputId, State},
    wayland_objects::wl_display::WlDisplay,
};

#[derive(Debug)]
pub struct ExtSessionLockSurfaceV1 {
    id: ExtSessionLockSurfaceV1Id,
    lock: Resolved<ExtSessionLockV1Id>,
    surface: Resolved<WlSurfaceId>,
    output_id: OutputId,
    inert: bool,
    configures: Vec<(u32, u32, u32)>,
    acked_index: Option<usize>,
}

impl ExtSessionLockSurfaceV1 {
    pub fn new(
        id: ExtSessionLockSurfaceV1Id,
        lock: Resolved<ExtSessionLockV1Id>,
        surface: Resolved<WlSurfaceId>,
        output_id: OutputId,
    ) -> Self {
        Self {
            id,
            lock,
            surface,
            output_id,
            inert: false,
            configures: Vec::new(),
            acked_index: None,
        }
    }

    pub fn output_id(&self) -> OutputId {
        self.output_id
    }

    pub fn wl_surface(&self) -> Resolved<WlSurfaceId> {
        self.surface
    }

    pub fn configures(&self) -> &[(u32, u32, u32)] {
        &self.configures
    }

    pub fn make_inert(&mut self) {
        self.inert = true;
    }

    pub fn is_inert(&self) -> bool {
        self.inert
    }

    pub fn configure(
        &mut self,
        serial: u32,
        width: u32,
        height: u32,
        dispatcher: &mut ClientDispatcher,
    ) {
        self.configures.push((serial, width, height));
        self.send_event(
            ext_session_lock_surface_v1::Event::Configure { serial, width, height },
            dispatcher,
        );
    }

    fn delete_self(
        &self,
        client: &mut Client,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
    ) {
        client.delete_id(&WlDisplay::default(), state, dispatcher, self.id.into());
    }
}

impl Object for ExtSessionLockSurfaceV1 {
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
        use ext_session_lock_surface_v1::Request::*;
        match client.parse::<Self>(req, dispatcher)? {
            Destroy => {
                if !self.inert {
                    state.session_lock.clear_surface(self.output_id);
                }
                self.delete_self(client, state, dispatcher);
            }
            AckConfigure { serial } => {
                let Some(index) = self
                    .configures
                    .iter()
                    .position(|(sent, _, _)| *sent == serial)
                else {
                    client.protocol_violation_disconnect(
                        dispatcher,
                        self.id.into(),
                        ext_session_lock_surface_v1::Error::InvalidSerial as u32,
                        "ext_session_lock_surface_v1.ack_configure serial was not sent",
                    );
                    return Ok(());
                };
                if self.acked_index.is_some_and(|acked| index <= acked) {
                    client.protocol_violation_disconnect(
                        dispatcher,
                        self.id.into(),
                        ext_session_lock_surface_v1::Error::InvalidSerial as u32,
                        "ext_session_lock_surface_v1.ack_configure serial was already consumed",
                    );
                    return Ok(());
                }
                self.acked_index = Some(index);
            }
        }
        Ok(())
    }

    fn on_client_disconnect(
        &mut self,
        _client: &mut Client,
        state: &mut State,
        _dispatcher: &mut ClientDispatcher,
    ) {
        if !self.inert {
            state.session_lock.clear_surface(self.output_id);
        }
    }
}
