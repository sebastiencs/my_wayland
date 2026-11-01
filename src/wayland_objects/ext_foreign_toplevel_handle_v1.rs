//! One `ext_foreign_toplevel_handle_v1`. After `closed`, only `destroy` is meaningful.
//! The identifier is the toplevel generation and is not reused.

use crate::{
    capture::ToplevelKey,
    clients::{Client, ClientDispatcher, EventWriter, RawRequest},
    object::{Object, Parser, RequestResult},
    protocol_generated::{ext_foreign_toplevel_handle_v1, ExtForeignToplevelHandleV1Id, WaylandId},
    state::{GrabbedObject, State},
    wayland_objects::wl_display::WlDisplay,
};

#[derive(Debug)]
pub struct ExtForeignToplevelHandleV1 {
    id: ExtForeignToplevelHandleV1Id,
    key: ToplevelKey,
    title: String,
    app_id: String,
    closed: bool,
}

impl ExtForeignToplevelHandleV1 {
    pub(crate) fn new(
        id: ExtForeignToplevelHandleV1Id,
        key: ToplevelKey,
        title: String,
        app_id: String,
    ) -> Self {
        Self { id, key, title, app_id, closed: false }
    }

    pub(crate) fn key(&self) -> ToplevelKey {
        self.key
    }

    pub(crate) fn is_closed(&self) -> bool {
        self.closed
    }

    pub(crate) fn send_initial(&self, identifier: &str, dispatcher: &mut ClientDispatcher) {
        self.send_event(
            ext_foreign_toplevel_handle_v1::Event::Identifier {
                identifier: identifier.to_string(),
            },
            dispatcher,
        );
        self.send_event(
            ext_foreign_toplevel_handle_v1::Event::Title { title: self.title.clone() },
            dispatcher,
        );
        self.send_event(
            ext_foreign_toplevel_handle_v1::Event::AppId { app_id: self.app_id.clone() },
            dispatcher,
        );
        self.send_event(ext_foreign_toplevel_handle_v1::Event::Done, dispatcher);
    }

    pub(crate) fn send_closed(&mut self, dispatcher: &mut ClientDispatcher) {
        if self.closed {
            return;
        }
        self.closed = true;
        self.send_event(ext_foreign_toplevel_handle_v1::Event::Closed, dispatcher);
    }

    fn delete_self(
        &self,
        client: &mut Client,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
    ) {
        if let Some(handle) = client.resolve(self.id, state) {
            state.capture.forget_handle(handle);
        }
        client.delete_id(&WlDisplay::default(), state, dispatcher, self.id.into());
    }
}

impl Object for ExtForeignToplevelHandleV1 {
    fn id(&self) -> WaylandId {
        self.id.into()
    }

    fn on(
        self: GrabbedObject<Self>,
        client: &mut Client,
        state: &mut State,
        req: RawRequest,
        dispatcher: &mut ClientDispatcher,
    ) -> RequestResult {
        use ext_foreign_toplevel_handle_v1::Request::*;
        match client.parse::<Self>(req, dispatcher)? {
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
        if let Some(handle) = client.resolve(self.id, state) {
            state.capture.forget_handle(handle);
        }
    }
}
