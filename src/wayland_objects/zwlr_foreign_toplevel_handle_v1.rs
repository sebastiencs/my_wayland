//! One `zwlr_foreign_toplevel_handle_v1`.
//!
//! The initial burst is title, app id, `output_enter` for each `wl_output` the client has bound,
//! the state array, `parent` on version 3, then one `done`. Later changes send the changed events
//! and one `done`. `closed` is the last event. Requests other than `destroy` are accepted and
//! ignored until the actions are implemented.

use crate::{
    capture::ToplevelKey,
    clients::{Client, ClientDispatcher, EventWriter, RawRequest, Resolved},
    object::{Object, Parser, RequestResult},
    protocol_generated::{
        zwlr_foreign_toplevel_handle_v1, WaylandId, WlOutputId, ZwlrForeignToplevelHandleV1Id,
        ZwlrForeignToplevelManagerV1Id,
    },
    state::{GrabbedObject, State},
    wayland_objects::wl_display::WlDisplay,
};

/// `fullscreen` was added to the state enum in version 2, with `set_fullscreen`.
const FULLSCREEN_STATE_SINCE: u32 = zwlr_foreign_toplevel_handle_v1::SET_FULLSCREEN_SINCE;

#[derive(Debug)]
pub struct ZwlrForeignToplevelHandleV1 {
    id: ZwlrForeignToplevelHandleV1Id,
    key: ToplevelKey,
    version: u32,
    manager: Resolved<ZwlrForeignToplevelManagerV1Id>,
    closed: bool,
}

impl ZwlrForeignToplevelHandleV1 {
    pub(crate) fn new(
        id: ZwlrForeignToplevelHandleV1Id,
        key: ToplevelKey,
        version: u32,
        manager: Resolved<ZwlrForeignToplevelManagerV1Id>,
    ) -> Self {
        Self { id, key, version, manager, closed: false }
    }

    pub(crate) fn key(&self) -> ToplevelKey {
        self.key
    }

    pub(crate) fn version(&self) -> u32 {
        self.version
    }

    pub(crate) fn manager(&self) -> Resolved<ZwlrForeignToplevelManagerV1Id> {
        self.manager
    }

    pub(crate) fn is_closed(&self) -> bool {
        self.closed
    }

    pub(crate) fn state_values(
        version: u32,
        activated: bool,
        maximized: bool,
        fullscreen: bool,
    ) -> Vec<u32> {
        let mut states = Vec::new();
        if maximized {
            states.push(zwlr_foreign_toplevel_handle_v1::State::Maximized as u32);
        }
        if activated {
            states.push(zwlr_foreign_toplevel_handle_v1::State::Activated as u32);
        }
        if version >= FULLSCREEN_STATE_SINCE && fullscreen {
            states.push(zwlr_foreign_toplevel_handle_v1::State::Fullscreen as u32);
        }
        states
    }

    pub(crate) fn send_title(&self, title: &str, dispatcher: &mut ClientDispatcher) {
        self.send_event(
            zwlr_foreign_toplevel_handle_v1::Event::Title { title: title.to_string() },
            dispatcher,
        );
    }

    pub(crate) fn send_app_id(&self, app_id: &str, dispatcher: &mut ClientDispatcher) {
        self.send_event(
            zwlr_foreign_toplevel_handle_v1::Event::AppId { app_id: app_id.to_string() },
            dispatcher,
        );
    }

    pub(crate) fn send_output_enter(&self, output: WlOutputId, dispatcher: &mut ClientDispatcher) {
        self.send_event(zwlr_foreign_toplevel_handle_v1::Event::OutputEnter { output }, dispatcher);
    }

    pub(crate) fn send_output_leave(&self, output: WlOutputId, dispatcher: &mut ClientDispatcher) {
        self.send_event(zwlr_foreign_toplevel_handle_v1::Event::OutputLeave { output }, dispatcher);
    }

    pub(crate) fn send_state(&self, state: Vec<u32>, dispatcher: &mut ClientDispatcher) {
        self.send_event(
            zwlr_foreign_toplevel_handle_v1::Event::State {
                state: crate::protocol_generated::Array(state),
            },
            dispatcher,
        );
    }

    pub(crate) fn send_parent(
        &self,
        parent: ZwlrForeignToplevelHandleV1Id,
        dispatcher: &mut ClientDispatcher,
    ) {
        self.send_event(zwlr_foreign_toplevel_handle_v1::Event::Parent { parent }, dispatcher);
    }

    pub(crate) fn send_done(&self, dispatcher: &mut ClientDispatcher) {
        self.send_event(zwlr_foreign_toplevel_handle_v1::Event::Done, dispatcher);
    }

    pub(crate) fn send_closed(&mut self, dispatcher: &mut ClientDispatcher) {
        if self.closed {
            return;
        }
        self.closed = true;
        self.send_event(zwlr_foreign_toplevel_handle_v1::Event::Closed, dispatcher);
    }

    fn delete_self(
        &self,
        client: &mut Client,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
    ) {
        if let Some(handle) = client.resolve(self.id, state) {
            state.capture.forget_wlr_handle(handle);
        }
        client.delete_id(&WlDisplay::default(), state, dispatcher, self.id.into());
    }
}

impl Object for ZwlrForeignToplevelHandleV1 {
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
        use zwlr_foreign_toplevel_handle_v1::Request::*;
        match client.parse::<Self>(req, dispatcher)? {
            Destroy => self.delete_self(client, state, dispatcher),
            // Activate, close, fullscreen, and maximize are applied in a later phase.
            // Minimize stays a no-op. set_rectangle will be validated then and stored nowhere.
            SetMaximized
            | UnsetMaximized
            | SetMinimized
            | UnsetMinimized
            | Activate { .. }
            | Close
            | SetRectangle { .. }
            | SetFullscreen { .. }
            | UnsetFullscreen => {}
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
            state.capture.forget_wlr_handle(handle);
        }
    }
}
