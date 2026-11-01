//! `ext_foreign_toplevel_list_v1`. Mapped toplevels are announced by a later phase.
//! `stop` ends the stream with one `finished` event. The global stays unpublished.

use crate::{
    clients::{Client, ClientDispatcher, EventWriter, RawRequest, Resolved},
    globals::Global,
    object::{Object, Parser, RequestResult},
    protocol_generated::{
        ext_foreign_toplevel_list_v1, Constants, ExtForeignToplevelHandleV1Id,
        ExtForeignToplevelListV1Id, NewId, WaylandId,
    },
    state::{GrabbedObject, State},
    wayland_objects::wl_display::WlDisplay,
};

#[derive(Debug)]
pub struct ExtForeignToplevelListV1Global;

impl ExtForeignToplevelListV1Global {
    pub fn new() -> Box<dyn Global> {
        Box::new(Self)
    }
}

impl Global for ExtForeignToplevelListV1Global {
    fn interface(&self) -> (&'static str, u32) {
        (ExtForeignToplevelListV1::INTERFACE_NAME, ExtForeignToplevelListV1::INTERFACE_VERSION)
    }

    fn bind(
        &mut self,
        client: &mut Client,
        state: &mut State,
        id: NewId,
        _version: u32,
        _dispatcher: &mut ClientDispatcher,
    ) {
        ExtForeignToplevelListV1::attach(client, state, ExtForeignToplevelListV1Id::from(id));
    }
}

#[derive(Debug)]
pub struct ExtForeignToplevelListV1 {
    id: ExtForeignToplevelListV1Id,
    finished: bool,
}

impl ExtForeignToplevelListV1 {
    pub fn attach(
        client: &mut Client,
        state: &mut State,
        id: ExtForeignToplevelListV1Id,
    ) -> Resolved<ExtForeignToplevelListV1Id> {
        let (resolved, _) = client.insert_object(state, Self { id, finished: false });
        state.capture.track_list(resolved);
        resolved
    }

    pub(crate) fn is_finished(&self) -> bool {
        self.finished
    }

    pub(crate) fn send_toplevel(
        &self,
        toplevel: ExtForeignToplevelHandleV1Id,
        dispatcher: &mut ClientDispatcher,
    ) {
        self.send_event(ext_foreign_toplevel_list_v1::Event::Toplevel { toplevel }, dispatcher);
    }

    fn delete_self(
        &self,
        client: &mut Client,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
    ) {
        if let Some(list) = client.resolve(self.id, state) {
            state.capture.forget_list(list);
        }
        client.delete_id(&WlDisplay::default(), state, dispatcher, self.id.into());
    }
}

impl Object for ExtForeignToplevelListV1 {
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
        use ext_foreign_toplevel_list_v1::Request::*;
        match client.parse::<Self>(req, dispatcher)? {
            Stop => {
                if !self.finished {
                    self.finished = true;
                    self.send_event(ext_foreign_toplevel_list_v1::Event::Finished, dispatcher);
                }
            }
            Destroy => self.delete_self(client, state, dispatcher),
        }
        Ok(())
    }

    fn on_client_disconnect(
        &mut self,
        client: &mut Client,
        state: &mut State,
        _dispatcher: &mut ClientDispatcher,
    ) {
        if let Some(list) = client.resolve(self.id, state) {
            state.capture.forget_list(list);
        }
    }
}
