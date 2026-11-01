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
    identifier: String,
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
        Self {
            id,
            key,
            identifier: key.identifier(),
            title,
            app_id,
            closed: false,
        }
    }

    pub(crate) fn key(&self) -> ToplevelKey {
        self.key
    }

    pub(crate) fn identifier(&self) -> &str {
        &self.identifier
    }

    pub(crate) fn title(&self) -> &str {
        &self.title
    }

    pub(crate) fn is_closed(&self) -> bool {
        self.closed
    }

    pub(crate) fn send_initial(&self, dispatcher: &mut ClientDispatcher) {
        self.send_event(
            ext_foreign_toplevel_handle_v1::Event::Identifier {
                identifier: self.identifier.clone(),
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

    pub(crate) fn send_title(&mut self, title: &str, dispatcher: &mut ClientDispatcher) {
        if self.closed || self.title == title {
            return;
        }
        self.title = title.to_string();
        self.send_event(
            ext_foreign_toplevel_handle_v1::Event::Title { title: self.title.clone() },
            dispatcher,
        );
        self.send_event(ext_foreign_toplevel_handle_v1::Event::Done, dispatcher);
    }

    pub(crate) fn send_app_id(&mut self, app_id: &str, dispatcher: &mut ClientDispatcher) {
        if self.closed || self.app_id == app_id {
            return;
        }
        self.app_id = app_id.to_string();
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
