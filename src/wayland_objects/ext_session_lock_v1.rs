//! Accepted `ext_session_lock_v1`. `locked` is sent by [`confirm_lock`] after presentation.

use crate::{
    clients::{Client, ClientDispatcher, ClientId, EventWriter, RawRequest, Resolved},
    object::{Object, Parser, RequestResult},
    protocol_generated::{
        ext_session_lock_v1, Constants, ExtSessionLockSurfaceV1Id, ExtSessionLockV1Id, WaylandId,
        WlOutputId, WlSurfaceId,
    },
    session_lock::{PreLockFocus, SessionLockPhase},
    state::{GrabbedObject, State},
    wayland_objects::{
        ext_session_lock_surface_v1::ExtSessionLockSurfaceV1, wl_display::WlDisplay,
        wl_output::WlOutput, wl_surface::WlSurface,
    },
};

#[derive(Debug)]
pub struct ExtSessionLockV1 {
    id: ExtSessionLockV1Id,
    client_id: ClientId,
    finished: bool,
    locked_notified: bool,
    locked_event_sent: bool,
}

impl ExtSessionLockV1 {
    pub fn new(id: ExtSessionLockV1Id, client_id: ClientId) -> Self {
        Self {
            id,
            client_id,
            finished: false,
            locked_notified: false,
            locked_event_sent: false,
        }
    }

    pub fn finished_sent(&self) -> bool {
        self.finished
    }

    pub fn locked_notified(&self) -> bool {
        self.locked_notified
    }

    pub fn mark_locked_sent(&mut self) {
        self.locked_notified = true;
    }

    pub fn deny(&mut self, dispatcher: &mut ClientDispatcher) {
        if self.finished {
            return;
        }
        self.finished = true;
        self.send_event(ext_session_lock_v1::Event::Finished, dispatcher);
    }

    pub fn locked_event_sent(&self) -> bool {
        self.locked_event_sent
    }

    pub fn send_locked(&mut self, dispatcher: &mut ClientDispatcher) {
        if self.locked_event_sent {
            return;
        }
        self.locked_event_sent = true;
        self.locked_notified = true;
        self.send_event(ext_session_lock_v1::Event::Locked, dispatcher);
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

impl Object for ExtSessionLockV1 {
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
        use ext_session_lock_v1::Request::*;
        match client.parse::<Self>(req, dispatcher)? {
            Destroy => {
                if self.locked_notified {
                    client.protocol_violation_disconnect(
                        dispatcher,
                        self.id.into(),
                        ext_session_lock_v1::Error::InvalidDestroy as u32,
                        "ext_session_lock_v1.destroy after locked",
                    );
                    return Ok(());
                }
                release_lock(client, state, dispatcher, self.id, false);
                self.delete_self(client, state, dispatcher);
            }
            GetLockSurface { id, surface, output } => {
                create_lock_surface(&mut self, client, state, dispatcher, id, surface, output);
            }
            UnlockAndDestroy => {
                if !self.locked_notified {
                    client.protocol_violation_disconnect(
                        dispatcher,
                        self.id.into(),
                        ext_session_lock_v1::Error::InvalidUnlock as u32,
                        "ext_session_lock_v1.unlock_and_destroy before locked",
                    );
                    return Ok(());
                }
                release_lock(client, state, dispatcher, self.id, true);
                self.delete_self(client, state, dispatcher);
            }
        }
        Ok(())
    }

    fn on_client_disconnect(
        &mut self,
        _client: &mut Client,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
    ) {
        if self.finished {
            return;
        }
        let surfaces = state.session_lock.surfaces();
        let owns_session = state
            .session_lock
            .lock()
            .is_some_and(|lock| lock.wayland_id() == self.id);
        if !owns_session && !self.locked_notified {
            return;
        }
        let outputs = state.session_lock.covered_outputs();
        inert_surfaces(state, surfaces);
        if self.locked_notified {
            state.session_lock.abandon();
        } else {
            state.session_lock.cancel_pending();
        }
        publish_locked_hint(state, dispatcher);
        crate::drm::request_outputs(state, outputs, dispatcher);
    }
}

/// Send `locked` once every output that was active at acceptance has presented.
pub fn confirm_lock(state: &mut State, dispatcher: &mut ClientDispatcher) -> bool {
    let Some(lock) = state.session_lock.confirm_if_presented() else {
        return false;
    };
    let Some(mut lock) = lock.try_ref_mut(state) else {
        return false;
    };
    lock.mark_locked_sent();
    lock.send_locked(dispatcher);
    true
}

/// A protected frame finished its page flip. Queuing the frame does not reach this.
pub fn on_protected_frame_presented(
    state: &mut State,
    output_id: crate::state::OutputId,
    dispatcher: &mut crate::runner::Dispatcher,
) {
    state.session_lock.mark_output_presented(output_id);
    let Some(lock) = state.session_lock.confirm_if_presented() else {
        return;
    };
    let client_id = lock.client_id();
    if let Some(mut lock) = lock.try_ref_mut(state) {
        lock.mark_locked_sent();
    }
    dispatcher.dispatch_to(client_id, crate::clients::ClientAction::SendSessionLockLocked);
}

fn release_lock(
    client: &mut Client,
    state: &mut State,
    dispatcher: &mut ClientDispatcher,
    lock_id: ExtSessionLockV1Id,
    unlock: bool,
) {
    let owns_session = state
        .session_lock
        .lock()
        .is_some_and(|lock| lock.wayland_id() == lock_id && lock.client_id() == client.client_id());
    if !owns_session {
        return;
    }
    let saved_focus = state.session_lock.pre_lock_focus();
    let outputs = state.session_lock.covered_outputs();
    let surfaces = state.session_lock.surfaces();
    inert_surfaces(state, surfaces);
    if unlock {
        state.session_lock.unlock();
        crate::input::focus::restore_keyboard_focus_after_unlock(
            state,
            dispatcher,
            saved_focus.keyboard,
        );
    } else {
        state.session_lock.cancel_pending();
    }
    publish_locked_hint(state, dispatcher);
    crate::drm::request_outputs(state, outputs, dispatcher);
}

fn inert_surfaces(state: &mut State, surfaces: Vec<Resolved<ExtSessionLockSurfaceV1Id>>) {
    for surface in surfaces {
        let Some(mut surface) = surface.try_ref_mut(state) else {
            continue;
        };
        surface.make_inert();
    }
}

fn create_lock_surface(
    lock: &mut ExtSessionLockV1,
    client: &mut Client,
    state: &mut State,
    dispatcher: &mut ClientDispatcher,
    id: ExtSessionLockSurfaceV1Id,
    surface: WlSurfaceId,
    output: WlOutputId,
) {
    if lock.finished {
        client.protocol_violation_disconnect(
            dispatcher,
            lock.id.into(),
            ext_session_lock_v1::Error::Role as u32,
            "ext_session_lock_v1.get_lock_surface on a finished lock",
        );
        return;
    }
    let Some(surface) = client.resolve(surface, state) else {
        client.protocol_violation_disconnect(
            dispatcher,
            lock.id.into(),
            crate::protocol_generated::wl_display::Error::InvalidObject as u32,
            "ext_session_lock_v1.get_lock_surface requires a live wl_surface",
        );
        return;
    };
    let Some(output) = client.resolve(output, state) else {
        client.protocol_violation_disconnect(
            dispatcher,
            lock.id.into(),
            crate::protocol_generated::wl_display::Error::InvalidObject as u32,
            "ext_session_lock_v1.get_lock_surface requires a live wl_output",
        );
        return;
    };
    let Some(output_id) = output.rref(state).output_id() else {
        client.protocol_violation_disconnect(
            dispatcher,
            lock.id.into(),
            crate::protocol_generated::wl_display::Error::InvalidObject as u32,
            "ext_session_lock_v1.get_lock_surface output is gone",
        );
        return;
    };
    if state.session_lock.surface_for_output(output_id).is_some() {
        client.protocol_violation_disconnect(
            dispatcher,
            lock.id.into(),
            ext_session_lock_v1::Error::DuplicateOutput as u32,
            "ext_session_lock_v1.get_lock_surface output already has a lock surface",
        );
        return;
    }
    let (has_role, has_buffer) = {
        let wl_surface = surface.rref(state);
        (
            wl_surface.role() != crate::wayland_objects::wl_surface::Role::None,
            wl_surface.has_attached_or_committed_buffer(),
        )
    };
    if has_role {
        client.protocol_violation_disconnect(
            dispatcher,
            lock.id.into(),
            ext_session_lock_v1::Error::Role as u32,
            "ext_session_lock_v1.get_lock_surface surface already has a role",
        );
        return;
    }
    if has_buffer {
        client.protocol_violation_disconnect(
            dispatcher,
            lock.id.into(),
            ext_session_lock_v1::Error::AlreadyConstructed as u32,
            "ext_session_lock_v1.get_lock_surface surface already has a buffer",
        );
        return;
    }
    let (width, height) = state
        .outputs
        .try_get(output_id)
        .map(|output| output.logical_size())
        .unwrap_or((0, 0));
    let width = u32::try_from(width).unwrap_or(0);
    let height = u32::try_from(height).unwrap_or(0);
    let serial = state.seat.next_serial();
    if !surface.ref_mut(state).assign_lock_surface_role() {
        client.protocol_violation_disconnect(
            dispatcher,
            lock.id.into(),
            ext_session_lock_v1::Error::Role as u32,
            "ext_session_lock_v1.get_lock_surface surface already has a role",
        );
        return;
    }
    state.unregister_root_surface(surface);
    let lock_resolved = client
        .resolve(lock.id, state)
        .expect("ext_session_lock_v1 is live");
    let (surface_resolved, _) = client
        .insert_object(state, ExtSessionLockSurfaceV1::new(id, lock_resolved, surface, output_id));
    surface.ref_mut(state).set_lock_surface(surface_resolved);
    if !state
        .session_lock
        .assign_surface(output_id, surface_resolved)
    {
        client.protocol_violation_disconnect(
            dispatcher,
            lock.id.into(),
            ext_session_lock_v1::Error::DuplicateOutput as u32,
            "ext_session_lock_v1.get_lock_surface output already has a lock surface",
        );
        return;
    }
    surface_resolved
        .ref_mut(state)
        .configure(serial, width, height, dispatcher);
}

pub(crate) fn capture_focus(state: &State) -> PreLockFocus {
    PreLockFocus {
        keyboard: state.seat.keyboard_focus(),
        pointer: state.seat.pointer_focus.map(|focus| focus.target),
    }
}

pub(crate) fn enabled_outputs(state: &State) -> Vec<crate::state::OutputId> {
    state
        .outputs
        .iter()
        .filter(|(_, output)| output.enabled)
        .map(|(output_id, _)| output_id)
        .collect()
}

pub(crate) fn publish_locked_hint(state: &State, dispatcher: &mut crate::runner::Dispatcher) {
    dispatcher.dispatch(crate::dbus::DbusAction::SetLockedHint {
        locked: state.session_lock.input_gated(),
    });
}

/// Mode, scale, and enable changes. A disabled output leaves the barrier without
/// counting as a flip. An enabled output is covered and its lock surface is resized.
pub(crate) fn on_output_state_changed(
    state: &mut State,
    dispatcher: &mut crate::runner::Dispatcher,
    output_id: crate::state::OutputId,
) {
    if !state.session_lock.input_gated() {
        return;
    }
    let Some(output) = state.outputs.try_get(output_id) else {
        return;
    };
    if !output.enabled {
        if let Some(surface) = state.session_lock.detach_output(output_id) {
            if let Some(mut surface) = surface.try_ref_mut(state) {
                surface.make_inert();
            }
        }
        let outputs = state.session_lock.covered_outputs();
        crate::drm::request_outputs(state, outputs, dispatcher);
        return;
    }
    state.session_lock.cover_output(output_id);
    if let Some(lock) = state.session_lock.lock() {
        dispatcher.dispatch_to(
            lock.client_id(),
            crate::clients::ClientAction::ReconfigureLockOutput { output_id },
        );
    }
    crate::drm::request_output(state, output_id, dispatcher);
}

pub(crate) fn accept_lock(
    state: &mut State,
    client_id: ClientId,
    lock: Resolved<ExtSessionLockV1Id>,
) -> bool {
    if !state.session_lock.accepts_new_lock() {
        return false;
    }
    let keep_focus = state.session_lock.phase() == SessionLockPhase::Abandoned;
    let focus = capture_focus(state);
    let outputs = enabled_outputs(state);
    state
        .session_lock
        .install(client_id, lock, keep_focus, focus, outputs);
    true
}
