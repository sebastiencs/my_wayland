//! One `zwlr_foreign_toplevel_handle_v1`.
//!
//! The initial burst is title, app id, `output_enter` for each `wl_output` the client has bound,
//! the state array, `parent` on version 3, then one `done`. Later changes send the changed events
//! and one `done`. `closed` is the last event. `set_maximized` and `set_minimized`
//! are ignored. `activate` shows the window's workspace and focuses it. `close`
//! asks the toplevel to quit. `set_fullscreen` fullscreens it on its current workspace.

use crate::{
    capture::ToplevelKey,
    clients::{Client, ClientDispatcher, ClientManagerAction, EventWriter, RawRequest, Resolved},
    drm::{request_active_output, request_outputs, request_outputs_for_rect},
    object::{Object, Parser, RequestResult},
    protocol_generated::{
        zwlr_foreign_toplevel_handle_v1, WaylandId, WlOutputId, WlSurfaceId,
        ZwlrForeignToplevelHandleV1Id, ZwlrForeignToplevelManagerV1Id,
    },
    seat::{resolve_activation_target, set_keyboard_focus_from_surface},
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

    fn activate(&self, state: &mut State, dispatcher: &mut ClientDispatcher) {
        let Some(root) = crate::capture::root_surface_for_toplevel_key(state, self.key) else {
            return;
        };
        if let Some(workspace_id) = state.root_surface_workspace(root) {
            if state.activate_workspace(workspace_id) {
                crate::input::pointer_lock::release_pointer_capture(state, dispatcher);
                let outputs = state.workspace_output_ids(workspace_id);
                crate::wayland_objects::ext_workspace_manager_v1::publish_workspace_state_changed(
                    state, dispatcher, outputs,
                );
            }
        }
        let Some(focus) = resolve_activation_target(state, root) else {
            return;
        };
        if !state.raise_root_surface(focus.surface) {
            return;
        }
        set_keyboard_focus_from_surface(state, dispatcher, Some(focus));
        if let Some(bounds) = state
            .root_surface_for_surface(focus.surface)
            .and_then(|root_surface| state.root_surface_surface_bounds_global(root_surface))
        {
            request_outputs_for_rect(state, bounds, dispatcher);
        } else {
            request_active_output(state, dispatcher);
        }
    }

    fn close(&self, state: &mut State, dispatcher: &mut ClientDispatcher) {
        let Some(root) = crate::capture::root_surface_for_toplevel_key(state, self.key) else {
            return;
        };
        let _ =
            crate::wayland_objects::xdg_surface::request_toplevel_close(state, dispatcher, root);
    }

    fn fullscreen(&self, state: &mut State, dispatcher: &mut ClientDispatcher, fullscreen: bool) {
        let Some(root) = crate::capture::root_surface_for_toplevel_key(state, self.key) else {
            return;
        };
        if let Some(xdg_surface) = root
            .try_ref(state)
            .and_then(|surface| surface.xdg_surface())
        {
            if let Some(toplevel) = state.xdg_toplevel(xdg_surface) {
                if let Some(toplevel) = toplevel.try_ref_mut(state) {
                    toplevel.set_requested_fullscreen(fullscreen);
                }
            }
        }
        let affected_outputs = state
            .root_surface_workspace(root)
            .map(|workspace_id| state.workspace_output_ids(workspace_id))
            .unwrap_or_default();
        if state.set_root_surface_fullscreen(root, fullscreen) {
            if !affected_outputs.is_empty() {
                request_outputs(state, affected_outputs.into_iter(), dispatcher);
            }
            crate::seat::refresh_scene_focus(state, dispatcher);
            dispatcher.dispatch(ClientManagerAction::WorkspaceStateChanged);
        }
    }

    fn validate_rectangle(
        &self,
        client: &mut Client,
        state: &State,
        dispatcher: &mut ClientDispatcher,
        surface: WlSurfaceId,
        width: i32,
        height: i32,
    ) -> bool {
        if width < 0 || height < 0 {
            client.protocol_violation_disconnect(
                dispatcher,
                self.id.into(),
                zwlr_foreign_toplevel_handle_v1::Error::InvalidRectangle as u32,
                "set_rectangle width and height must be non-negative",
            );
            return false;
        }
        client
            .resolve_or_disconnect(surface, state, dispatcher)
            .is_ok()
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
        let request = client.parse::<Self>(req, dispatcher)?;
        if self.closed && !matches!(request, Destroy) {
            return Ok(());
        }
        match request {
            Destroy => self.delete_self(client, state, dispatcher),
            // Sway ignores foreign maximize. Minimize has no compositor state.
            // set_rectangle is checked and then discarded.
            SetMaximized | UnsetMaximized | SetMinimized | UnsetMinimized => {}
            Activate { seat } => {
                if client
                    .resolve_or_disconnect(seat, state, dispatcher)
                    .is_err()
                {
                    return Ok(());
                }
                self.activate(state, dispatcher);
            }
            Close => self.close(state, dispatcher),
            SetRectangle { surface, x: _, y: _, width, height } => {
                if !self.validate_rectangle(client, state, dispatcher, surface, width, height) {
                    return Ok(());
                }
            }
            SetFullscreen { output } => {
                let output_id: u32 = output.into();
                if output_id != 0
                    && client
                        .resolve_or_disconnect(output, state, dispatcher)
                        .is_err()
                {
                    return Ok(());
                }
                self.fullscreen(state, dispatcher, true);
            }
            UnsetFullscreen => self.fullscreen(state, dispatcher, false),
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
