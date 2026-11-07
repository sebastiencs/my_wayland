//! Seat-side state for text input and input methods.
//!
//! One text input can be active. One input method receives its state and
//! sends back preedit, commit, and delete requests. Popup surfaces are placed
//! next to the cursor rectangle and drawn above window content.

use std::collections::{BTreeMap, BTreeSet};

use crate::{
    clients::{Client, ClientDispatcher, ClientId, Resolved},
    object::PostAction,
    protocol_generated::{
        WlSurfaceId, ZwpInputMethodKeyboardGrabV2Id, ZwpInputMethodV2Id, ZwpInputPopupSurfaceV2Id,
        ZwpTextInputV3Id,
    },
    region::RectI,
    runner::Dispatcher,
    state::State,
    wayland_objects::zwp_input_method_v2::ZwpInputMethodV2,
};

use super::SurfaceFocus;

/// How one `zwp_text_input_v3.commit` changed the enabled flag.
///
/// `Enable` and `Disable` are the edges. `Reset` is a commit that disables and
/// enables again while the object was already enabled. Anything else is `Update`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TextInputCommitKind {
    Enable,
    Disable,
    Reset,
    Update,
}

pub(crate) fn commit_kind(
    was_enabled: bool,
    enabled: bool,
    enable_requested: bool,
    disable_requested: bool,
) -> TextInputCommitKind {
    if !was_enabled && enabled {
        TextInputCommitKind::Enable
    } else if was_enabled && !enabled {
        TextInputCommitKind::Disable
    } else if enabled && enable_requested && disable_requested {
        TextInputCommitKind::Reset
    } else {
        TextInputCommitKind::Update
    }
}

/// Current text-input fields the input method is allowed to see.
#[derive(Clone, Debug)]
pub(crate) struct TextInputRelayState {
    pub surrounding: Option<(String, u32, u32)>,
    pub change_cause: u32,
    pub content_type: Option<(u32, u32)>,
}

/// Which of those fields this commit changed. `Enable` and `Reset` send the
/// whole state instead.
#[derive(Clone, Copy, Debug)]
pub(crate) struct TextInputRelayChanges {
    pub surrounding: bool,
    pub change_cause: bool,
    pub content_type: bool,
}

/// `zwp_text_input_v3` events produced by focus changes and by the input method.
#[derive(Clone, Debug)]
pub(crate) enum TextInputClientEvent {
    Enter {
        surface: Resolved<WlSurfaceId>,
    },
    Leave {
        surface: Resolved<WlSurfaceId>,
    },
    /// `preedit_string(null)`, `commit_string(null)`, `done`.
    ClearPreedit,
    ImeCommit {
        preedit: Option<(String, i32, i32)>,
        commit: Option<String>,
        delete: Option<(u32, u32)>,
    },
}

/// One `zwp_input_method_v2` event. A batch is applied together, and `Done` is
/// the serial the input method must echo.
#[derive(Clone, Debug)]
pub(crate) enum ImEvent {
    Activate,
    Deactivate,
    SurroundingText { text: String, cursor: u32, anchor: u32 },
    TextChangeCause { cause: u32 },
    ContentType { hint: u32, purpose: u32 },
    Done,
}

#[derive(Debug, Default)]
pub(crate) struct ImeState {
    text_inputs: BTreeMap<ClientId, Vec<Resolved<ZwpTextInputV3Id>>>,
    active_text_input: Option<Resolved<ZwpTextInputV3Id>>,
    input_method: Option<Resolved<ZwpInputMethodV2Id>>,
    keyboard_grab: Option<Resolved<ZwpInputMethodKeyboardGrabV2Id>>,
    /// Presses forwarded to the grab. Releases of these keys stay with the grab,
    /// or are swallowed after the grab is gone. Presses from before the grab are
    /// absent, so their releases still reach the focused client.
    grabbed_keys: BTreeSet<u32>,
    popups: Vec<InputPopupRecord>,
}

/// One `zwp_input_popup_surface_v2` owned by the live input method.
#[derive(Clone, Copy, Debug)]
pub(crate) struct InputPopupRecord {
    pub popup: Resolved<ZwpInputPopupSurfaceV2Id>,
    pub surface: Resolved<WlSurfaceId>,
    /// Last global rectangle we asked outputs to draw. Absent when the popup is hidden.
    pub global: Option<RectI>,
    /// Last `text_input_rectangle` sent, in popup-local coordinates.
    pub cursor_in_popup: Option<RectI>,
}

impl ImeState {
    pub(crate) fn register_text_input(
        &mut self,
        client_id: ClientId,
        text_input: Resolved<ZwpTextInputV3Id>,
    ) {
        self.text_inputs
            .entry(client_id)
            .or_default()
            .push(text_input);
    }

    /// Returns whether this object held the active slot.
    pub(crate) fn unregister_text_input(&mut self, text_input: Resolved<ZwpTextInputV3Id>) -> bool {
        let client_id = text_input.client_id();
        if let Some(text_inputs) = self.text_inputs.get_mut(&client_id) {
            text_inputs.retain(|candidate| *candidate != text_input);
            if text_inputs.is_empty() {
                self.text_inputs.remove(&client_id);
            }
        }
        let was_active = self.active_text_input == Some(text_input);
        if was_active {
            self.active_text_input = None;
        }
        was_active
    }

    pub(crate) fn active_text_input(&self) -> Option<Resolved<ZwpTextInputV3Id>> {
        self.active_text_input
    }

    pub(crate) fn input_method(&self) -> Option<Resolved<ZwpInputMethodV2Id>> {
        self.input_method
    }

    pub(crate) fn set_input_method(&mut self, input_method: Resolved<ZwpInputMethodV2Id>) {
        self.input_method = Some(input_method);
    }

    pub(crate) fn clear_input_method(&mut self) {
        self.input_method = None;
    }

    pub(crate) fn keyboard_grab(&self) -> Option<Resolved<ZwpInputMethodKeyboardGrabV2Id>> {
        self.keyboard_grab
    }

    pub(crate) fn set_keyboard_grab(&mut self, grab: Resolved<ZwpInputMethodKeyboardGrabV2Id>) {
        self.keyboard_grab = Some(grab);
    }

    pub(crate) fn note_grabbed_key(&mut self, key: u32) {
        self.grabbed_keys.insert(key);
    }

    /// Returns whether this release belongs to a press the grab consumed.
    pub(crate) fn take_grabbed_key(&mut self, key: u32) -> bool {
        self.grabbed_keys.remove(&key)
    }

    /// `Enable` and `Reset` take the active slot when this object is entered on
    /// the keyboard focus. A later one replaces the current one. `Disable`
    /// clears the slot only when this object holds it.
    pub(crate) fn note_commit(
        &mut self,
        text_input: Resolved<ZwpTextInputV3Id>,
        kind: TextInputCommitKind,
        entered: Option<Resolved<WlSurfaceId>>,
        focused_surface: Option<Resolved<WlSurfaceId>>,
    ) {
        let on_focus = entered.is_some() && entered == focused_surface;
        match kind {
            TextInputCommitKind::Enable | TextInputCommitKind::Reset if on_focus => {
                self.active_text_input = Some(text_input);
            }
            TextInputCommitKind::Disable if self.active_text_input == Some(text_input) => {
                self.active_text_input = None;
            }
            TextInputCommitKind::Enable
            | TextInputCommitKind::Disable
            | TextInputCommitKind::Reset
            | TextInputCommitKind::Update => {}
        }
    }

    pub(crate) fn add_popup(
        &mut self,
        popup: Resolved<ZwpInputPopupSurfaceV2Id>,
        surface: Resolved<WlSurfaceId>,
    ) {
        self.popups
            .push(InputPopupRecord { popup, surface, global: None, cursor_in_popup: None });
    }

    pub(crate) fn take_popup(
        &mut self,
        popup: Resolved<ZwpInputPopupSurfaceV2Id>,
    ) -> Option<InputPopupRecord> {
        let index = self
            .popups
            .iter()
            .position(|record| record.popup == popup)?;
        Some(self.popups.remove(index))
    }

    pub(crate) fn input_popups(&self) -> &[InputPopupRecord] {
        &self.popups
    }

    pub(crate) fn take_input_popups(&mut self) -> Vec<InputPopupRecord> {
        std::mem::take(&mut self.popups)
    }

    pub(crate) fn set_popup_geometry(
        &mut self,
        popup: Resolved<ZwpInputPopupSurfaceV2Id>,
        global: Option<RectI>,
        cursor_in_popup: Option<RectI>,
    ) {
        if let Some(record) = self.popups.iter_mut().find(|record| record.popup == popup) {
            record.global = global;
            record.cursor_in_popup = cursor_in_popup;
        }
    }

    pub(crate) fn popup_global(&self, surface: Resolved<WlSurfaceId>) -> Option<RectI> {
        self.popups
            .iter()
            .find(|record| record.surface == surface)
            .and_then(|record| record.global)
    }
}

/// Forward one text-input commit to the seat's input method, when there is one.
pub(crate) fn relay_text_input_commit(
    state: &State,
    dispatcher: &mut Dispatcher,
    text_input: Resolved<ZwpTextInputV3Id>,
    kind: TextInputCommitKind,
    was_active: bool,
    relay: &TextInputRelayState,
    changes: TextInputRelayChanges,
) {
    let Some(input_method) = state.seat.ime.input_method else {
        return;
    };
    let is_active = state.seat.ime.active_text_input == Some(text_input);
    let mut events = Vec::new();
    match kind {
        TextInputCommitKind::Enable if is_active => {
            events.push(ImEvent::Activate);
            push_state(&mut events, relay, FULL_CHANGES);
            events.push(ImEvent::Done);
        }
        TextInputCommitKind::Reset if is_active => {
            events.push(ImEvent::Deactivate);
            events.push(ImEvent::Activate);
            push_state(&mut events, relay, FULL_CHANGES);
            events.push(ImEvent::Done);
        }
        TextInputCommitKind::Disable if was_active => {
            events.push(ImEvent::Deactivate);
            events.push(ImEvent::Done);
        }
        TextInputCommitKind::Update if is_active => {
            push_state(&mut events, relay, changes);
            events.push(ImEvent::Done);
        }
        TextInputCommitKind::Enable
        | TextInputCommitKind::Disable
        | TextInputCommitKind::Reset
        | TextInputCommitKind::Update => {}
    }
    dispatch_input_method(dispatcher, input_method, events);
}

const FULL_CHANGES: TextInputRelayChanges =
    TextInputRelayChanges { surrounding: true, change_cause: true, content_type: true };

fn push_state(
    events: &mut Vec<ImEvent>,
    relay: &TextInputRelayState,
    changes: TextInputRelayChanges,
) {
    if changes.surrounding {
        if let Some((text, cursor, anchor)) = relay.surrounding.clone() {
            events.push(ImEvent::SurroundingText { text, cursor, anchor });
        }
    }
    if changes.change_cause {
        events.push(ImEvent::TextChangeCause { cause: relay.change_cause });
    }
    if changes.content_type {
        if let Some((hint, purpose)) = relay.content_type {
            events.push(ImEvent::ContentType { hint, purpose });
        }
    }
}

/// The input method bound while a text input was already active.
pub(crate) fn relay_existing_activation(
    state: &State,
    dispatcher: &mut Dispatcher,
    input_method: Resolved<ZwpInputMethodV2Id>,
    text_input: Resolved<ZwpTextInputV3Id>,
) {
    let Some(relay) = text_input
        .try_ref(state)
        .map(|text_input| text_input.relay_state())
    else {
        return;
    };
    let mut events = vec![ImEvent::Activate];
    push_state(&mut events, &relay, FULL_CHANGES);
    events.push(ImEvent::Done);
    dispatch_input_method(dispatcher, input_method, events);
}

/// The active text input went away. Tell the input method, unless that client
/// is the one currently disconnecting.
pub(crate) fn deactivate_input_method(
    state: &State,
    dispatcher: &mut Dispatcher,
    skip_client: Option<ClientId>,
) {
    let Some(input_method) = state.seat.ime.input_method else {
        return;
    };
    if skip_client == Some(input_method.client_id()) {
        return;
    }
    dispatch_input_method(dispatcher, input_method, vec![ImEvent::Deactivate, ImEvent::Done]);
}

/// The input method went away. Drop any preedit it left in the text input.
/// Drop the grab but keep `grabbed_keys`, then give the focused client the
/// current hardware modifiers. `skip` is a client that is disconnecting.
pub(crate) fn release_keyboard_grab(
    state: &mut State,
    dispatcher: &mut Dispatcher,
    skip_client: Option<ClientId>,
) {
    if state.seat.ime.keyboard_grab.take().is_none() {
        return;
    }
    let Some(focus) = state.seat.keyboard_focus else {
        return;
    };
    if skip_client == Some(focus.client_id()) {
        return;
    }
    crate::seat::resend_keyboard_modifiers(state, dispatcher);
}

/// Route one hardware key to the grab. Returns whether the focused client
/// must not see it. A locked session always returns false.
pub(crate) fn forward_hardware_key(
    state: &mut State,
    dispatcher: &mut Dispatcher,
    key: u32,
    key_state: u32,
    time: u32,
    changed_modifiers: Option<crate::xkb::KeyboardModifiers>,
) -> bool {
    if state.session_lock.input_gated() {
        return false;
    }
    let pressed = key_state == 1;
    if pressed {
        let Some(grab) = state.seat.ime.keyboard_grab else {
            return false;
        };
        state.seat.ime.note_grabbed_key(key);
        send_grab_key(state, dispatcher, grab, time, key, key_state);
        if let Some(modifiers) = changed_modifiers {
            send_grab_modifiers(state, dispatcher, grab, modifiers);
        }
        return true;
    }
    if !state.seat.ime.take_grabbed_key(key) {
        return false;
    }
    if let Some(grab) = state.seat.ime.keyboard_grab {
        send_grab_key(state, dispatcher, grab, time, key, key_state);
        if let Some(modifiers) = changed_modifiers {
            send_grab_modifiers(state, dispatcher, grab, modifiers);
        }
    } else if changed_modifiers.is_some() {
        crate::seat::resend_keyboard_modifiers(state, dispatcher);
    }
    true
}

fn send_grab_key(
    state: &mut State,
    dispatcher: &mut Dispatcher,
    grab: Resolved<ZwpInputMethodKeyboardGrabV2Id>,
    time: u32,
    key: u32,
    key_state: u32,
) {
    let serial = state.seat.next_serial();
    dispatch_grab(
        dispatcher,
        grab,
        KeyboardGrabClientEvent::Key { serial, time, key, state: key_state },
    );
}

fn send_grab_modifiers(
    state: &mut State,
    dispatcher: &mut Dispatcher,
    grab: Resolved<ZwpInputMethodKeyboardGrabV2Id>,
    modifiers: crate::xkb::KeyboardModifiers,
) {
    let serial = state.seat.next_serial();
    dispatch_grab(dispatcher, grab, KeyboardGrabClientEvent::Modifiers { serial, modifiers });
}

pub(crate) fn announce_keyboard_grab(
    state: &State,
    dispatcher: &mut Dispatcher,
    grab: Resolved<ZwpInputMethodKeyboardGrabV2Id>,
) {
    let modifiers = state.keyboard.modifiers();
    dispatch_grab(dispatcher, grab, KeyboardGrabClientEvent::Initial { modifiers });
}

fn dispatch_grab(
    dispatcher: &mut Dispatcher,
    grab: Resolved<ZwpInputMethodKeyboardGrabV2Id>,
    event: KeyboardGrabClientEvent,
) {
    dispatcher.dispatch_to(
        grab.client_id(),
        crate::clients::ClientAction::KeyboardGrabEvent { grab, event },
    );
}

/// Events for `zwp_input_method_keyboard_grab_v2`. `Initial` is the hardware
/// keymap, the modifiers at grab time, and the repeat settings.
#[derive(Clone, Debug)]
pub(crate) enum KeyboardGrabClientEvent {
    Initial { modifiers: crate::xkb::KeyboardModifiers },
    Key { serial: u32, time: u32, key: u32, state: u32 },
    Modifiers { serial: u32, modifiers: crate::xkb::KeyboardModifiers },
}

pub(crate) fn clear_active_preedit(
    state: &State,
    dispatcher: &mut Dispatcher,
    skip_client: Option<ClientId>,
) {
    let Some(text_input) = state.seat.ime.active_text_input else {
        return;
    };
    if skip_client == Some(text_input.client_id()) || !text_input.exists(state) {
        return;
    }
    dispatch_text_input(
        dispatcher,
        text_input.client_id(),
        text_input,
        TextInputClientEvent::ClearPreedit,
    );
}

/// Keyboard focus moved. Clear the active text input, `leave` objects entered
/// on the old surface, then `enter` every text input of the newly focused client.
///
/// `enabled` is left alone. The client sends `disable` or `enable` itself.
/// Requests after `leave` are still applied: the client's `disable` has to land,
/// or the next `enable` would not be an edge.
pub(crate) fn keyboard_focus_changed(
    state: &mut State,
    dispatcher: &mut Dispatcher,
    old: Option<SurfaceFocus>,
    new: Option<SurfaceFocus>,
) {
    if let Some(active) = state.seat.ime.active_text_input.take() {
        if active.exists(state) {
            dispatch_text_input(
                dispatcher,
                active.client_id(),
                active,
                TextInputClientEvent::ClearPreedit,
            );
        }
        deactivate_input_method(state, dispatcher, None);
    }

    if let Some(old) = old {
        leave_text_inputs(state, dispatcher, old);
    }
    if let Some(new) = new {
        enter_text_inputs(state, dispatcher, new);
    }
    reposition_popups(state, dispatcher);
}

fn leave_text_inputs(state: &mut State, dispatcher: &mut Dispatcher, focus: SurfaceFocus) {
    let text_inputs = state
        .seat
        .ime
        .text_inputs
        .get(&focus.client_id())
        .cloned()
        .unwrap_or_default();
    let surface_live = focus.surface.exists(state);
    for text_input in text_inputs {
        let was_entered = {
            let Some(object) = text_input.try_ref_mut(state) else {
                continue;
            };
            if object.entered() != Some(focus.surface) {
                false
            } else {
                object.set_entered(None);
                true
            }
        };
        if was_entered && surface_live {
            dispatch_text_input(
                dispatcher,
                focus.client_id(),
                text_input,
                TextInputClientEvent::Leave { surface: focus.surface },
            );
        }
    }
}

fn enter_text_inputs(state: &mut State, dispatcher: &mut Dispatcher, focus: SurfaceFocus) {
    if !focus.surface.exists(state) {
        return;
    }
    let text_inputs = state
        .seat
        .ime
        .text_inputs
        .get(&focus.client_id())
        .cloned()
        .unwrap_or_default();
    for text_input in text_inputs {
        if text_input.try_ref_mut(state).is_none() {
            continue;
        }
        text_input.ref_mut(state).set_entered(Some(focus.surface));
        dispatch_text_input(
            dispatcher,
            focus.client_id(),
            text_input,
            TextInputClientEvent::Enter { surface: focus.surface },
        );
    }
}

fn dispatch_text_input(
    dispatcher: &mut Dispatcher,
    client_id: ClientId,
    text_input: Resolved<ZwpTextInputV3Id>,
    event: TextInputClientEvent,
) {
    dispatcher
        .dispatch_to(client_id, crate::clients::ClientAction::TextInputEvent { text_input, event });
}

fn dispatch_input_method(
    dispatcher: &mut Dispatcher,
    input_method: Resolved<ZwpInputMethodV2Id>,
    events: Vec<ImEvent>,
) {
    if events.is_empty() {
        return;
    }
    dispatcher.dispatch_to(
        input_method.client_id(),
        crate::clients::ClientAction::InputMethodEvent { input_method, events },
    );
}

pub(crate) fn deliver_text_input_event(
    state: &State,
    dispatcher: &mut ClientDispatcher,
    text_input: Resolved<ZwpTextInputV3Id>,
    event: TextInputClientEvent,
) {
    match event {
        TextInputClientEvent::Enter { surface } => {
            if !surface.exists(state) {
                return;
            }
            let Some(object) = text_input.try_ref(state) else {
                return;
            };
            log::debug!(
                "text-input enter id={} surface={}",
                u32::from(text_input.wayland_id()),
                u32::from(surface.wayland_id())
            );
            object.send_enter(surface.wayland_id(), dispatcher);
            object.send_done(dispatcher);
        }
        TextInputClientEvent::Leave { surface } => {
            if !surface.exists(state) {
                return;
            }
            let Some(object) = text_input.try_ref(state) else {
                return;
            };
            log::debug!(
                "text-input leave id={} surface={}",
                u32::from(text_input.wayland_id()),
                u32::from(surface.wayland_id())
            );
            object.send_leave(surface.wayland_id(), dispatcher);
            object.send_done(dispatcher);
        }
        TextInputClientEvent::ClearPreedit => {
            let Some(object) = text_input.try_ref(state) else {
                return;
            };
            log::debug!("text-input clear-preedit id={}", u32::from(text_input.wayland_id()));
            object.send_preedit(None, 0, 0, dispatcher);
            object.send_commit_string(None, dispatcher);
            object.send_done(dispatcher);
        }
        TextInputClientEvent::ImeCommit { preedit, commit, delete } => {
            if state.seat.ime.active_text_input != Some(text_input) {
                log::debug!(
                    "text-input dropped stale input-method commit id={}",
                    u32::from(text_input.wayland_id())
                );
                return;
            }
            let Some(object) = text_input.try_ref(state) else {
                return;
            };
            if let Some((before, after)) = delete {
                object.send_delete_surrounding(before, after, dispatcher);
            }
            if let Some((text, begin, end)) = preedit {
                object.send_preedit(Some(text), begin, end, dispatcher);
            }
            if let Some(text) = commit {
                object.send_commit_string(Some(text), dispatcher);
            }
            object.send_done(dispatcher);
        }
    }
}

pub(crate) fn deliver_input_method_event(
    state: &mut State,
    dispatcher: &mut ClientDispatcher,
    input_method: Resolved<ZwpInputMethodV2Id>,
    events: Vec<ImEvent>,
) {
    let Some(object) = input_method.try_ref_mut(state) else {
        return;
    };
    for event in events {
        match event {
            ImEvent::Activate => {
                object.set_active(true);
                object.send_activate(dispatcher);
            }
            ImEvent::Deactivate => {
                object.set_active(false);
                object.send_deactivate(dispatcher);
            }
            ImEvent::SurroundingText { text, cursor, anchor } => {
                object.send_surrounding_text(text, cursor, anchor, dispatcher);
            }
            ImEvent::TextChangeCause { cause } => object.send_text_change_cause(cause, dispatcher),
            ImEvent::ContentType { hint, purpose } => {
                object.send_content_type(hint, purpose, dispatcher);
            }
            ImEvent::Done => {
                object.increment_done_count();
                object.send_done(dispatcher);
            }
        }
    }
}

impl ZwpInputMethodV2 {
    pub(crate) fn send_activate(&self, dispatcher: &mut ClientDispatcher) {
        use crate::{clients::EventWriter, protocol_generated::zwp_input_method_v2::Event};
        self.send_event(Event::Activate, dispatcher);
    }

    pub(crate) fn send_deactivate(&self, dispatcher: &mut ClientDispatcher) {
        use crate::{clients::EventWriter, protocol_generated::zwp_input_method_v2::Event};
        self.send_event(Event::Deactivate, dispatcher);
    }

    pub(crate) fn send_surrounding_text(
        &self,
        text: String,
        cursor: u32,
        anchor: u32,
        dispatcher: &mut ClientDispatcher,
    ) {
        use crate::{clients::EventWriter, protocol_generated::zwp_input_method_v2::Event};
        self.send_event(Event::SurroundingText { text, cursor, anchor }, dispatcher);
    }

    pub(crate) fn send_text_change_cause(&self, cause: u32, dispatcher: &mut ClientDispatcher) {
        use crate::{clients::EventWriter, protocol_generated::zwp_input_method_v2::Event};
        self.send_event(Event::TextChangeCause { cause }, dispatcher);
    }

    pub(crate) fn send_content_type(
        &self,
        hint: u32,
        purpose: u32,
        dispatcher: &mut ClientDispatcher,
    ) {
        use crate::{clients::EventWriter, protocol_generated::zwp_input_method_v2::Event};
        self.send_event(Event::ContentType { hint, purpose }, dispatcher);
    }

    pub(crate) fn send_done(&self, dispatcher: &mut ClientDispatcher) {
        use crate::{clients::EventWriter, protocol_generated::zwp_input_method_v2::Event};
        self.send_event(Event::Done, dispatcher);
    }

    pub(crate) fn send_unavailable(&self, dispatcher: &mut ClientDispatcher) {
        use crate::{clients::EventWriter, protocol_generated::zwp_input_method_v2::Event};
        self.send_event(Event::Unavailable, dispatcher);
    }
}

/// Runs after the request that changed cursor geometry or the popup buffer.
pub(crate) struct RepositionInputPopups;

impl PostAction for RepositionInputPopups {
    fn execute(
        self: Box<Self>,
        _client: &mut Client,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
    ) {
        reposition_popups(state, dispatcher);
    }
}

/// A popup is drawn only with a buffer, a live input method, an active text
/// input, and an unlocked session.
pub(crate) fn input_popup_visible(state: &State, surface: Resolved<WlSurfaceId>) -> bool {
    if state.session_lock.input_gated() {
        return false;
    }
    if state.seat.ime.input_method().is_none() || state.seat.ime.active_text_input().is_none() {
        return false;
    }
    surface
        .try_ref(state)
        .is_some_and(|surface| surface.has_committed_buffer())
}

/// Where to draw `popup`, and the cursor rectangle in that popup's local coordinates.
///
/// The anchor is the focused surface's global origin plus the text input's cursor
/// rectangle. Without a cursor rectangle the anchor is a zero-size point at the
/// surface origin. The popup sits below the anchor, or above it when that would
/// leave the output, then both axes are clamped into the output that contains
/// the anchor.
pub(crate) fn popup_placement(
    state: &State,
    popup: Resolved<ZwpInputPopupSurfaceV2Id>,
) -> Option<(RectI, RectI)> {
    let popup_surface = popup.try_ref(state)?.surface();
    let size = popup_surface.try_ref(state)?.logical_size()?;
    let focus = state.seat.keyboard_focus()?;
    let (origin_x, origin_y) = super::pick::surface_global_origin(state, focus.surface)?;
    let cursor = state
        .seat
        .ime
        .active_text_input()
        .and_then(|text_input| text_input.try_ref(state))
        .and_then(|text_input| text_input.cursor_rect())
        .unwrap_or(RectI { x: 0, y: 0, width: 0, height: 0 });
    let anchor = RectI {
        x: origin_x.saturating_add(cursor.x),
        y: origin_y.saturating_add(cursor.y),
        width: cursor.width,
        height: cursor.height,
    };
    let output_id = state
        .output_at_point(anchor.x, anchor.y)
        .or_else(|| state.primary_output_for_rect(anchor, None))?;
    let output = state.output_bounds(output_id)?;
    Some(place_popup(anchor, size.width, size.height, output))
}

fn place_popup(anchor: RectI, width: i32, height: i32, output: RectI) -> (RectI, RectI) {
    let mut x = anchor.x;
    let mut y = anchor.y.saturating_add(anchor.height);
    if y.saturating_add(height) > output.y2() {
        y = anchor.y.saturating_sub(height);
    }
    x = clamp_axis(x, output.x, output.x2().saturating_sub(width));
    y = clamp_axis(y, output.y, output.y2().saturating_sub(height));
    let global = RectI { x, y, width, height };
    let cursor_in_popup = RectI {
        x: anchor.x.saturating_sub(x),
        y: anchor.y.saturating_sub(y),
        width: anchor.width,
        height: anchor.height,
    };
    (global, cursor_in_popup)
}

fn clamp_axis(value: i32, min: i32, max: i32) -> i32 {
    if max < min {
        min
    } else {
        value.clamp(min, max)
    }
}

/// Recompute every live popup. Sends `text_input_rectangle` only when the
/// cursor rectangle relative to the popup changed, and damages both the old
/// and new global rectangles.
pub(crate) fn reposition_popups(state: &mut State, dispatcher: &mut Dispatcher) {
    let popups = state.seat.ime.input_popups().to_vec();
    let mut damage = Vec::new();
    let mut clients = BTreeSet::new();
    for popup in popups {
        clients.insert(popup.surface.client_id());
        if let Some(old) = popup.global {
            damage.push(old);
        }
        let placement = input_popup_visible(state, popup.surface)
            .then(|| popup_placement(state, popup.popup))
            .flatten();
        let cursor_changed = match placement {
            Some((global, cursor)) => {
                damage.push(global);
                let changed = popup.cursor_in_popup != Some(cursor);
                state
                    .seat
                    .ime
                    .set_popup_geometry(popup.popup, Some(global), Some(cursor));
                changed
            }
            None => {
                state.seat.ime.set_popup_geometry(popup.popup, None, None);
                false
            }
        };
        if cursor_changed {
            if let Some((_, cursor)) = placement {
                dispatcher.dispatch_to(
                    popup.popup.client_id(),
                    crate::clients::ClientAction::InputPopupEvent { popup: popup.popup, cursor },
                );
            }
        }
    }
    crate::drm::request_outputs_for_rects(state, damage, dispatcher);
    for client_id in clients {
        dispatcher.dispatch_to(client_id, crate::clients::ClientAction::RefreshSurfaceOutputs);
    }
}

/// The input method went away. Its popups stay as protocol objects but are no
/// longer placed or drawn.
pub(crate) fn retire_input_method_popups(
    state: &mut State,
    dispatcher: &mut Dispatcher,
    skip_client: Option<ClientId>,
) {
    let popups = state.seat.ime.take_input_popups();
    let mut damage = Vec::new();
    let mut clients = BTreeSet::new();
    for popup in popups {
        if let Some(global) = popup.global {
            damage.push(global);
        }
        if let Some(object) = popup.popup.try_ref_mut(state) {
            object.set_inert();
        }
        if Some(popup.surface.client_id()) != skip_client {
            clients.insert(popup.surface.client_id());
        }
    }
    crate::drm::request_outputs_for_rects(state, damage, dispatcher);
    for client_id in clients {
        dispatcher.dispatch_to(client_id, crate::clients::ClientAction::RefreshSurfaceOutputs);
    }
}

/// Base passed to output refresh so the popup's buffer lands on its global rectangle.
pub(crate) fn popup_output_base(
    state: &State,
    surface: Resolved<WlSurfaceId>,
) -> Option<(i32, i32)> {
    let global = state.seat.ime.popup_global(surface)?;
    let wl_surface = surface.try_ref(state)?;
    let (root_x, root_y) = wl_surface.root_position();
    Some((global.x.saturating_sub(root_x), global.y.saturating_sub(root_y)))
}
