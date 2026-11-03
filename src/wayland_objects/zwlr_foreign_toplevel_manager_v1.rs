//! `zwlr_foreign_toplevel_manager_v1`, advertised at version 3.
//!
//! Binding creates one handle per mapped toplevel, then sends that handle's initial details.
//! Creating every handle first matches wlroots: a child's `parent` event can point at a handle
//! that belongs to this same manager. `stop` sends `finished` and destroys the manager.
//! Handles that already exist keep receiving updates until they are closed.

use crate::{
    clients::{Client, ClientDispatcher, EventWriter, RawRequest, Resolved},
    globals::Global,
    object::{Object, Parser, RequestResult},
    protocol_generated::{
        zwlr_foreign_toplevel_manager_v1, Constants, NewId, WaylandId,
        ZwlrForeignToplevelHandleV1Id, ZwlrForeignToplevelManagerV1Id,
    },
    state::{GrabbedObject, State},
    wayland_objects::wl_display::WlDisplay,
};

#[derive(Debug)]
pub struct ZwlrForeignToplevelManagerV1Global;

impl ZwlrForeignToplevelManagerV1Global {
    pub fn new() -> Box<dyn Global> {
        Box::new(Self)
    }
}

impl Global for ZwlrForeignToplevelManagerV1Global {
    fn interface(&self) -> (&'static str, u32) {
        (
            ZwlrForeignToplevelManagerV1::INTERFACE_NAME,
            ZwlrForeignToplevelManagerV1::INTERFACE_VERSION,
        )
    }

    fn bind(
        &mut self,
        client: &mut Client,
        state: &mut State,
        id: NewId,
        version: u32,
        dispatcher: &mut ClientDispatcher,
    ) {
        ZwlrForeignToplevelManagerV1::attach(
            client,
            state,
            dispatcher,
            ZwlrForeignToplevelManagerV1Id::from(id),
            version,
        );
    }
}

#[derive(Debug)]
pub struct ZwlrForeignToplevelManagerV1 {
    id: ZwlrForeignToplevelManagerV1Id,
    version: u32,
    finished: bool,
}

impl ZwlrForeignToplevelManagerV1 {
    pub fn attach(
        client: &mut Client,
        state: &mut State,
        dispatcher: &mut crate::runner::Dispatcher,
        id: ZwlrForeignToplevelManagerV1Id,
        version: u32,
    ) -> Resolved<ZwlrForeignToplevelManagerV1Id> {
        let (resolved, _) = client.insert_object(state, Self { id, version, finished: false });
        state.capture.track_wlr_manager(resolved);
        crate::capture::sync_wlr_manager(state, dispatcher, resolved);
        resolved
    }

    pub(crate) fn version(&self) -> u32 {
        self.version
    }

    pub(crate) fn is_finished(&self) -> bool {
        self.finished
    }

    pub(crate) fn send_toplevel(
        &self,
        toplevel: ZwlrForeignToplevelHandleV1Id,
        dispatcher: &mut ClientDispatcher,
    ) {
        self.send_event(zwlr_foreign_toplevel_manager_v1::Event::Toplevel { toplevel }, dispatcher);
    }

    fn delete_self(
        &self,
        client: &mut Client,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
    ) {
        if let Some(manager) = client.resolve(self.id, state) {
            state.capture.forget_wlr_manager(manager);
        }
        client.delete_id(&WlDisplay::default(), state, dispatcher, self.id.into());
    }
}

impl Object for ZwlrForeignToplevelManagerV1 {
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
        use zwlr_foreign_toplevel_manager_v1::Request::*;
        match client.parse::<Self>(req, dispatcher)? {
            Stop => {
                if !self.finished {
                    self.finished = true;
                    self.send_event(zwlr_foreign_toplevel_manager_v1::Event::Finished, dispatcher);
                    self.delete_self(client, state, dispatcher);
                }
            }
        }
        Ok(())
    }

    fn on_client_disconnect(
        &mut self,
        client: &mut Client,
        state: &mut State,
        _dispatcher: &mut ClientDispatcher,
    ) {
        if let Some(manager) = client.resolve(self.id, state) {
            state.capture.forget_wlr_manager(manager);
        }
    }
}
