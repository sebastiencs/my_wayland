use std::collections::BTreeMap;

use crate::{
    clients::{ClientAction, ClientId},
    cursor::CursorState,
    drm::{schedule_active_output, Drm},
    object::Object,
    protocol_generated::{
        Array, WlKeyboardId, WlPointerId, WlSurfaceId, XdgPopupId, XdgToplevelId,
    },
    runner::Dispatcher,
    state::{DrmId, IndexMap, ObjectId, State},
    wayland_objects::wl_surface::{StackEntry, WlSurface},
    xkb::KeyboardModifiers,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SurfaceFocus {
    pub client_id: ClientId,
    pub surface_id: WlSurfaceId,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SurfaceRect {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PointerFocus {
    target: SurfaceFocus,
    rect: SurfaceRect,
}

#[derive(Clone, Debug)]
pub enum SeatClientEvent {
    PointerEnter {
        serial: u32,
        surface: WlSurfaceId,
        x: f32,
        y: f32,
    },
    PointerLeave {
        serial: u32,
        surface: WlSurfaceId,
    },
    PointerMotion {
        time: u32,
        x: f32,
        y: f32,
    },
    PointerButton {
        serial: u32,
        time: u32,
        button: u32,
        state: u32,
    },
    PointerFrame,
    KeyboardEnter {
        serial: u32,
        surface: WlSurfaceId,
        keys: Array,
    },
    KeyboardLeave {
        serial: u32,
        surface: WlSurfaceId,
    },
    KeyboardKey {
        serial: u32,
        time: u32,
        key: u32,
        state: u32,
    },
    KeyboardModifiers {
        serial: u32,
        mods_depressed: u32,
        mods_latched: u32,
        mods_locked: u32,
        group: u32,
    },
}

#[derive(Clone, Debug)]
struct PopupGrabState {
    client_id: ClientId,
    popup_stack: Vec<(XdgPopupId, WlSurfaceId)>,
}

#[derive(Clone, Copy, Debug)]
struct PointerPressState {
    serial: u32,
    focus: SurfaceFocus,
}

#[derive(Clone, Copy, Debug)]
struct PointerEnterState {
    serial: u32,
    client_id: ClientId,
}

#[derive(Clone, Copy, Debug)]
struct KeyboardTriggerState {
    serial: u32,
    focus: SurfaceFocus,
}

#[derive(Clone, Copy, Debug)]
enum PointerInteractiveGrab {
    Move {
        client_id: ClientId,
        root_surface_object_id: ObjectId,
        initial_pointer_pos: (f64, f64),
        initial_root_pos: (i32, i32),
    },
    Resize {
        client_id: ClientId,
        root_surface_object_id: ObjectId,
        xdg_toplevel_id: XdgToplevelId,
        edges: u32,
        initial_pointer_pos: (f64, f64),
        initial_root_pos: (i32, i32),
        initial_size: (i32, i32),
        last_size: (i32, i32),
    },
}

pub struct SeatState {
    current_serial: u32,
    pointer_pos: (f64, f64),
    pointer_focus: Option<PointerFocus>,
    last_pointer_enter: Option<PointerEnterState>,
    keyboard_focus: Option<SurfaceFocus>,
    pointer_button_count: u32,
    last_pointer_press: Option<PointerPressState>,
    last_keyboard_trigger: Option<KeyboardTriggerState>,
    cursor: CursorState,
    popup_grab: Option<PopupGrabState>,
    pointer_interactive_grab: Option<PointerInteractiveGrab>,
    pointer_resources: BTreeMap<ClientId, Vec<WlPointerId>>,
    keyboard_resources: BTreeMap<ClientId, Vec<WlKeyboardId>>,
}

impl Default for SeatState {
    fn default() -> Self {
        Self {
            current_serial: 1,
            pointer_pos: (0.0, 0.0),
            pointer_focus: None,
            last_pointer_enter: None,
            keyboard_focus: None,
            pointer_button_count: 0,
            last_pointer_press: None,
            last_keyboard_trigger: None,
            cursor: CursorState::default(),
            popup_grab: None,
            pointer_interactive_grab: None,
            pointer_resources: BTreeMap::default(),
            keyboard_resources: BTreeMap::default(),
        }
    }
}

impl SeatState {
    pub fn register_pointer(&mut self, client_id: ClientId, pointer_id: WlPointerId) {
        let resources = self.pointer_resources.entry(client_id).or_default();
        if !resources.contains(&pointer_id) {
            resources.push(pointer_id);
        }
    }

    pub fn unregister_pointer(&mut self, client_id: ClientId, pointer_id: WlPointerId) {
        remove_resource(&mut self.pointer_resources, client_id, pointer_id);
    }

    pub fn register_keyboard(&mut self, client_id: ClientId, keyboard_id: WlKeyboardId) {
        let resources = self.keyboard_resources.entry(client_id).or_default();
        if !resources.contains(&keyboard_id) {
            resources.push(keyboard_id);
        }
    }

    pub fn unregister_keyboard(&mut self, client_id: ClientId, keyboard_id: WlKeyboardId) {
        remove_resource(&mut self.keyboard_resources, client_id, keyboard_id);
    }

    pub fn pointer_resources(&self, client_id: ClientId) -> Vec<WlPointerId> {
        self.pointer_resources
            .get(&client_id)
            .cloned()
            .unwrap_or_default()
    }

    pub fn keyboard_resources(&self, client_id: ClientId) -> Vec<WlKeyboardId> {
        self.keyboard_resources
            .get(&client_id)
            .cloned()
            .unwrap_or_default()
    }

    pub fn remove_client(&mut self, client_id: ClientId) -> bool {
        self.pointer_resources.remove(&client_id);
        self.keyboard_resources.remove(&client_id);
        if self
            .pointer_focus
            .is_some_and(|focus| focus.target.client_id == client_id)
        {
            self.pointer_focus = None;
        }
        if self
            .last_pointer_enter
            .is_some_and(|enter| enter.client_id == client_id)
        {
            self.last_pointer_enter = None;
        }
        if self
            .keyboard_focus
            .is_some_and(|focus| focus.client_id == client_id)
        {
            self.keyboard_focus = None;
        }
        if self
            .popup_grab
            .as_ref()
            .is_some_and(|grab| grab.client_id == client_id)
        {
            self.popup_grab = None;
        }
        if self
            .pointer_interactive_grab
            .is_some_and(|grab| match grab {
                PointerInteractiveGrab::Move { client_id: grab_client_id, .. }
                | PointerInteractiveGrab::Resize { client_id: grab_client_id, .. } => {
                    grab_client_id == client_id
                }
            })
        {
            self.pointer_interactive_grab = None;
        }
        self.cursor.clear_for_client(client_id)
    }

    fn next_serial(&mut self) -> u32 {
        let serial = self.current_serial;
        self.current_serial = self.current_serial.wrapping_add(1).max(1);
        serial
    }

    fn record_pointer_button(&mut self, state: u32, serial: u32, focus: Option<SurfaceFocus>) {
        if state == 1 {
            self.pointer_button_count = self.pointer_button_count.saturating_add(1);
            self.last_pointer_press = focus.map(|focus| PointerPressState { serial, focus });
        } else {
            self.pointer_button_count = self.pointer_button_count.saturating_sub(1);
            if self.pointer_button_count == 0 {
                self.last_pointer_press = None;
            }
        }
    }

    fn record_keyboard_trigger(&mut self, serial: u32, focus: SurfaceFocus) {
        self.last_keyboard_trigger = Some(KeyboardTriggerState { serial, focus });
    }

    fn record_pointer_enter(&mut self, client_id: ClientId, serial: u32) {
        self.last_pointer_enter = Some(PointerEnterState { client_id, serial });
    }

    fn clear_pointer_enter(&mut self) {
        self.last_pointer_enter = None;
    }

    pub fn pointer_cursor_request_allowed(&self, client_id: ClientId, serial: u32) -> bool {
        self.pointer_focus
            .is_some_and(|focus| focus.target.client_id == client_id)
            && self
                .last_pointer_enter
                .is_some_and(|enter| enter.client_id == client_id && enter.serial == serial)
    }

    pub fn set_named_cursor(&mut self, client_id: ClientId, icon_names: &[&'static str]) -> bool {
        self.cursor.set_named_cursor(client_id, icon_names)
    }

    pub fn visible_cursor_image(&self) -> Option<&crate::cursor::CursorImage> {
        self.cursor.image()
    }

    pub fn cursor_is_visible(&self) -> bool {
        self.cursor.is_visible()
    }

    pub fn pointer_position(&self) -> (f64, f64) {
        self.pointer_pos
    }

    fn clear_cursor_for_focus(&mut self, focus_client_id: Option<ClientId>) -> bool {
        self.cursor.clear_for_focus(focus_client_id)
    }

    fn validate_implicit_grab_serial(
        &self,
        client_id: ClientId,
        surface_id: WlSurfaceId,
        serial: u32,
    ) -> bool {
        (self.pointer_button_count > 0
            && self.last_pointer_press.is_some_and(|press| {
                press.serial == serial
                    && press.focus.client_id == client_id
                    && press.focus.surface_id == surface_id
            }))
            || self.last_keyboard_trigger.is_some_and(|trigger| {
                trigger.serial == serial
                    && trigger.focus.client_id == client_id
                    && trigger.focus.surface_id == surface_id
            })
    }

    fn validate_popup_grab_serial(
        &self,
        client_id: ClientId,
        surface_id: WlSurfaceId,
        serial: u32,
    ) -> bool {
        self.validate_implicit_grab_serial(client_id, surface_id, serial)
    }

    fn popup_grab_client(&self) -> Option<ClientId> {
        self.popup_grab.as_ref().map(|grab| grab.client_id)
    }

    fn popup_keyboard_surface(&self) -> Option<WlSurfaceId> {
        self.popup_grab
            .as_ref()
            .and_then(|grab| grab.popup_stack.last())
            .map(|(_, surface_id)| *surface_id)
    }

    fn popup_grab_contains_surface(&self, surface_id: WlSurfaceId) -> bool {
        self.popup_grab.as_ref().is_some_and(|grab| {
            grab.popup_stack
                .iter()
                .any(|(_, candidate)| *candidate == surface_id)
        })
    }

    fn start_popup_grab(
        &mut self,
        client_id: ClientId,
        popup_id: XdgPopupId,
        popup_surface_id: WlSurfaceId,
    ) {
        let grab = self
            .popup_grab
            .get_or_insert_with(|| PopupGrabState { client_id, popup_stack: Vec::new() });
        if grab.client_id != client_id {
            *grab = PopupGrabState { client_id, popup_stack: Vec::new() };
        }
        grab.popup_stack
            .retain(|(candidate, _)| *candidate != popup_id);
        grab.popup_stack.push((popup_id, popup_surface_id));
    }

    fn remove_popup_grab(&mut self, popup_id: XdgPopupId) {
        let Some(grab) = self.popup_grab.as_mut() else {
            return;
        };
        grab.popup_stack
            .retain(|(candidate, _)| *candidate != popup_id);
        if grab.popup_stack.is_empty() {
            self.popup_grab = None;
        }
    }

    fn dismiss_popup_grab(&mut self) -> Option<(ClientId, Vec<XdgPopupId>)> {
        let grab = self.popup_grab.take()?;
        Some((
            grab.client_id,
            grab.popup_stack
                .into_iter()
                .map(|(popup_id, _)| popup_id)
                .collect(),
        ))
    }

    fn start_toplevel_move(
        &mut self,
        client_id: ClientId,
        root_surface_object_id: ObjectId,
        initial_root_pos: (i32, i32),
    ) {
        self.pointer_interactive_grab = Some(PointerInteractiveGrab::Move {
            client_id,
            root_surface_object_id,
            initial_pointer_pos: self.pointer_pos,
            initial_root_pos,
        });
    }

    fn start_toplevel_resize(
        &mut self,
        client_id: ClientId,
        root_surface_object_id: ObjectId,
        xdg_toplevel_id: XdgToplevelId,
        edges: u32,
        initial_root_pos: (i32, i32),
        initial_size: (i32, i32),
    ) {
        self.pointer_interactive_grab = Some(PointerInteractiveGrab::Resize {
            client_id,
            root_surface_object_id,
            xdg_toplevel_id,
            edges,
            initial_pointer_pos: self.pointer_pos,
            initial_root_pos,
            initial_size,
            last_size: initial_size,
        });
    }

    fn clear_pointer_interactive_grab(&mut self) {
        self.pointer_interactive_grab = None;
    }
}

pub fn handle_pointer_motion(
    state: &mut State,
    dispatcher: &mut Dispatcher,
    dx: f64,
    dy: f64,
    time: u32,
) {
    let seat = &mut state.seat;
    seat.pointer_pos.0 += dx;
    seat.pointer_pos.1 += dy;
    clamp_pointer_to_outputs(state);
    log::debug!(
        "seat pointer motion pos=({:.1}, {:.1}) time={}",
        state.seat.pointer_pos.0,
        state.seat.pointer_pos.1,
        time
    );
    if apply_pointer_interactive_grab(state, dispatcher) {
        return;
    }
    refresh_pointer_focus(state, dispatcher, time, true);
    if state.seat.cursor_is_visible() {
        schedule_active_output(state, dispatcher);
    }
}

pub fn handle_pointer_button(
    state: &mut State,
    dispatcher: &mut Dispatcher,
    button: u32,
    button_state: u32,
    time: u32,
) {
    if let Some(focus) = popup_keyboard_focus(state) {
        set_keyboard_focus(state, dispatcher, Some(focus));
    }
    refresh_pointer_focus(state, dispatcher, time, false);
    let pointer_focus = state.seat.pointer_focus;
    log::debug!(
        "seat pointer button button={} state={} time={} focus={:?}",
        button,
        button_state,
        time,
        pointer_focus.map(|focus| focus.target)
    );

    let serial = state.seat.next_serial();
    state
        .seat
        .record_pointer_button(button_state, serial, pointer_focus.map(|focus| focus.target));

    if maybe_dismiss_popup_grab_on_button(state, dispatcher, button_state, time) {
        return;
    }

    if button_state == 1 {
        set_keyboard_focus(state, dispatcher, pointer_focus.map(|focus| focus.target));
    }

    let Some(pointer_focus) = state.seat.pointer_focus else {
        if button_state != 1 {
            finish_pointer_interactive_grab(state, dispatcher);
        }
        return;
    };

    dispatch_client_event(
        dispatcher,
        pointer_focus.target.client_id,
        SeatClientEvent::PointerButton { serial, time, button, state: button_state },
    );
    dispatch_client_event(
        dispatcher,
        pointer_focus.target.client_id,
        SeatClientEvent::PointerFrame,
    );

    if button_state != 1 {
        finish_pointer_interactive_grab(state, dispatcher);
    }
}

pub fn handle_pointer_motion_absolute(
    state: &mut State,
    dispatcher: &mut Dispatcher,
    x: f64,
    y: f64,
    time: u32,
) {
    state.seat.pointer_pos = (x, y);
    clamp_pointer_to_outputs(state);
    log::debug!(
        "seat pointer motion absolute pos=({:.1}, {:.1}) time={}",
        state.seat.pointer_pos.0,
        state.seat.pointer_pos.1,
        time
    );
    if apply_pointer_interactive_grab(state, dispatcher) {
        return;
    }
    refresh_pointer_focus(state, dispatcher, time, true);
    if state.seat.cursor_is_visible() {
        schedule_active_output(state, dispatcher);
    }
}

pub fn handle_keyboard_key(
    state: &mut State,
    dispatcher: &mut Dispatcher,
    key: u32,
    key_state: u32,
    time: u32,
) {
    let changed_modifiers = state.keyboard.update_key(key, key_state == 1);
    log::debug!(
        "seat keyboard key key={} state={} time={} focus_before={:?}",
        key,
        key_state,
        time,
        state.seat.keyboard_focus
    );

    if state.seat.keyboard_focus.is_none() {
        set_keyboard_focus(state, dispatcher, state.seat.pointer_focus.map(|focus| focus.target));
    }

    let Some(focus) = state.seat.keyboard_focus else {
        return;
    };

    let serial = state.seat.next_serial();
    state.seat.record_keyboard_trigger(serial, focus);
    log::debug!(
        "seat dispatch keyboard key client={:?} serial={} key={} state={}",
        focus.client_id,
        serial,
        key,
        key_state
    );
    dispatch_client_event(
        dispatcher,
        focus.client_id,
        SeatClientEvent::KeyboardKey { serial, time, key, state: key_state },
    );

    if let Some(modifiers) = changed_modifiers {
        log::debug!(
            "seat modifiers changed client={:?} serial={} depressed={} latched={} locked={} group={}",
            focus.client_id,
            state.seat.current_serial,
            modifiers.mods_depressed,
            modifiers.mods_latched,
            modifiers.mods_locked,
            modifiers.group
        );
        dispatch_modifiers(dispatcher, focus.client_id, state.seat.next_serial(), modifiers);
    }
}

fn set_keyboard_focus(
    state: &mut State,
    dispatcher: &mut Dispatcher,
    new_focus: Option<SurfaceFocus>,
) {
    if state.seat.keyboard_focus == new_focus {
        return;
    }

    log::debug!(
        "seat keyboard focus change old={:?} new={:?}",
        state.seat.keyboard_focus,
        new_focus
    );

    let serial = state.seat.next_serial();

    if let Some(old_focus) = state.seat.keyboard_focus {
        dispatch_client_event(
            dispatcher,
            old_focus.client_id,
            SeatClientEvent::KeyboardLeave { serial, surface: old_focus.surface_id },
        );
    }

    if let Some(new_focus) = new_focus {
        let keys = state.keyboard.pressed_keys();
        dispatch_client_event(
            dispatcher,
            new_focus.client_id,
            SeatClientEvent::KeyboardEnter { serial, surface: new_focus.surface_id, keys },
        );
        dispatch_modifiers(dispatcher, new_focus.client_id, serial, state.keyboard.modifiers());
    }

    state.seat.keyboard_focus = new_focus;
}

fn surface_focus_for_wayland_id(state: &State, surface_id: WlSurfaceId) -> Option<SurfaceFocus> {
    for (_, object) in state.objects.iter() {
        let Some(surface) = object.downcast_ref::<WlSurface>() else {
            continue;
        };
        if surface.surface_id() == surface_id {
            return Some(SurfaceFocus { client_id: surface.client_id(), surface_id });
        }
    }
    None
}

fn popup_keyboard_focus(state: &State) -> Option<SurfaceFocus> {
    let surface_id = state.seat.popup_keyboard_surface()?;
    surface_focus_for_wayland_id(state, surface_id)
}

fn maybe_dismiss_popup_grab_on_button(
    state: &mut State,
    dispatcher: &mut Dispatcher,
    button_state: u32,
    time: u32,
) -> bool {
    if button_state != 1 {
        return false;
    }
    let Some(_grab_client_id) = state.seat.popup_grab_client() else {
        return false;
    };
    let inside_popup_stack = state.seat.pointer_focus.is_some_and(|focus| {
        state
            .seat
            .popup_grab_contains_surface(focus.target.surface_id)
    });
    if inside_popup_stack {
        return false;
    }
    let Some((client_id, popup_ids)) = state.seat.dismiss_popup_grab() else {
        return false;
    };
    dispatch_client_action(dispatcher, client_id, ClientAction::DismissPopupGrab { popup_ids });
    refresh_pointer_focus(state, dispatcher, time, false);
    set_keyboard_focus(state, dispatcher, state.seat.pointer_focus.map(|focus| focus.target));
    true
}

fn apply_pointer_interactive_grab(state: &mut State, dispatcher: &mut Dispatcher) -> bool {
    let Some(grab) = state.seat.pointer_interactive_grab else {
        return false;
    };
    match grab {
        PointerInteractiveGrab::Move {
            client_id,
            root_surface_object_id,
            initial_pointer_pos,
            initial_root_pos,
            ..
        } => {
            let dx = (state.seat.pointer_pos.0 - initial_pointer_pos.0).round() as i32;
            let dy = (state.seat.pointer_pos.1 - initial_pointer_pos.1).round() as i32;
            let Some(mut surface) = state
                .objects
                .try_grab_object::<WlSurface>(root_surface_object_id)
            else {
                state.seat.clear_pointer_interactive_grab();
                return true;
            };
            surface.set_root_position(initial_root_pos.0 + dx, initial_root_pos.1 + dy);
            dispatch_client_action(dispatcher, client_id, ClientAction::RefreshSurfaceOutputs);
            schedule_active_output(state, dispatcher);
        }
        PointerInteractiveGrab::Resize {
            client_id,
            root_surface_object_id,
            xdg_toplevel_id,
            edges,
            initial_pointer_pos,
            initial_root_pos,
            initial_size,
            mut last_size,
        } => {
            let dx = (state.seat.pointer_pos.0 - initial_pointer_pos.0).round() as i32;
            let dy = (state.seat.pointer_pos.1 - initial_pointer_pos.1).round() as i32;
            let (mut width, mut height) = initial_size;
            let (mut x, mut y) = initial_root_pos;

            if edges & 4 != 0 {
                width = (initial_size.0 - dx).max(1);
                x = initial_root_pos.0 + (initial_size.0 - width);
            }
            if edges & 8 != 0 {
                width = (initial_size.0 + dx).max(1);
            }
            if edges & 1 != 0 {
                height = (initial_size.1 - dy).max(1);
                y = initial_root_pos.1 + (initial_size.1 - height);
            }
            if edges & 2 != 0 {
                height = (initial_size.1 + dy).max(1);
            }

            let Some(mut surface) = state
                .objects
                .try_grab_object::<WlSurface>(root_surface_object_id)
            else {
                state.seat.clear_pointer_interactive_grab();
                return true;
            };
            surface.set_root_position(x, y);
            dispatch_client_action(dispatcher, client_id, ClientAction::RefreshSurfaceOutputs);
            schedule_active_output(state, dispatcher);

            if last_size != (width, height) {
                dispatch_client_action(
                    dispatcher,
                    client_id,
                    ClientAction::InteractiveResizeConfigure {
                        xdg_toplevel_id,
                        width,
                        height,
                        resizing: true,
                    },
                );
                last_size = (width, height);
            }

            state.seat.pointer_interactive_grab = Some(PointerInteractiveGrab::Resize {
                client_id,
                root_surface_object_id,
                xdg_toplevel_id,
                edges,
                initial_pointer_pos,
                initial_root_pos,
                initial_size,
                last_size,
            });
        }
    }
    true
}

fn finish_pointer_interactive_grab(state: &mut State, dispatcher: &mut Dispatcher) {
    let Some(grab) = state.seat.pointer_interactive_grab else {
        return;
    };
    if let PointerInteractiveGrab::Resize { client_id, xdg_toplevel_id, last_size, .. } = grab {
        dispatch_client_action(
            dispatcher,
            client_id,
            ClientAction::InteractiveResizeConfigure {
                xdg_toplevel_id,
                width: last_size.0,
                height: last_size.1,
                resizing: false,
            },
        );
    }
    state.seat.clear_pointer_interactive_grab();
}

fn hit_test_subtree(
    objects: &IndexMap<ObjectId, Box<dyn Object>>,
    surface_object_id: ObjectId,
    base_x: i32,
    base_y: i32,
    x: f64,
    y: f64,
) -> Option<PointerFocus> {
    let surface = objects.try_ref_object::<WlSurface>(surface_object_id)?;
    let surface_state = surface.render_state_at(base_x, base_y)?;
    let stack = surface.current_stack().to_vec();

    for entry in stack.iter().rev() {
        match *entry {
            StackEntry::Parent => {
                let rect = SurfaceRect {
                    x: surface_state.x,
                    y: surface_state.y,
                    width: surface_state.width,
                    height: surface_state.height,
                };
                if surface.accepts_input_at_with_origin(x, y, base_x, base_y) {
                    return Some(PointerFocus {
                        target: SurfaceFocus {
                            client_id: surface.client_id(),
                            surface_id: surface.surface_id(),
                        },
                        rect,
                    });
                }
            }
            StackEntry::Child(child_surface_object_id) => {
                let Some(child_surface) =
                    objects.try_ref_object::<WlSurface>(child_surface_object_id)
                else {
                    continue;
                };
                let (child_x, child_y) = child_surface.current_subsurface_position();
                if let Some(hit) = hit_test_subtree(
                    objects,
                    child_surface_object_id,
                    surface_state.x + child_x,
                    surface_state.y + child_y,
                    x,
                    y,
                ) {
                    return Some(hit);
                }
            }
        }
    }

    None
}

fn hit_test_surface(
    state: &State,
    _drm: &IndexMap<DrmId, Drm>,
    x: f64,
    y: f64,
) -> Option<PointerFocus> {
    let mut hit = None;

    for object_id in state.root_surface_ids() {
        let Some(surface) = state.objects.try_ref_object::<WlSurface>(object_id) else {
            continue;
        };
        if !surface.is_root() {
            continue;
        }
        if let Some(root_hit) = hit_test_subtree(&state.objects, object_id, 0, 0, x, y) {
            hit = Some(root_hit);
        }
    }

    if let Some(grab_client_id) = state.seat.popup_grab_client() {
        if hit.is_some_and(|focus| focus.target.client_id != grab_client_id) {
            hit = None;
        }
    }

    hit
}

fn refresh_pointer_focus(
    state: &mut State,
    dispatcher: &mut Dispatcher,
    time: u32,
    send_motion: bool,
) {
    let old_focus = state.seat.pointer_focus;
    let new_focus =
        hit_test_surface(state, &state.drm, state.seat.pointer_pos.0, state.seat.pointer_pos.1);

    if old_focus.map(|focus| focus.target) != new_focus.map(|focus| focus.target) {
        log::debug!(
            "seat pointer focus change pos=({:.1}, {:.1}) old={:?} new={:?}",
            state.seat.pointer_pos.0,
            state.seat.pointer_pos.1,
            old_focus.map(|focus| focus.target),
            new_focus.map(|focus| focus.target)
        );
    }

    if old_focus.map(|focus| focus.target) != new_focus.map(|focus| focus.target) {
        let serial = state.seat.next_serial();

        if let Some(old_focus) = old_focus {
            dispatch_client_event(
                dispatcher,
                old_focus.target.client_id,
                SeatClientEvent::PointerLeave { serial, surface: old_focus.target.surface_id },
            );
            dispatch_client_event(
                dispatcher,
                old_focus.target.client_id,
                SeatClientEvent::PointerFrame,
            );
        }

        if let Some(new_focus) = new_focus {
            let (x, y) = local_coords(new_focus.rect, state.seat.pointer_pos);
            state
                .seat
                .record_pointer_enter(new_focus.target.client_id, serial);
            dispatch_client_event(
                dispatcher,
                new_focus.target.client_id,
                SeatClientEvent::PointerEnter {
                    serial,
                    surface: new_focus.target.surface_id,
                    x,
                    y,
                },
            );
            if send_motion {
                dispatch_client_event(
                    dispatcher,
                    new_focus.target.client_id,
                    SeatClientEvent::PointerMotion { time, x, y },
                );
            }
            dispatch_client_event(
                dispatcher,
                new_focus.target.client_id,
                SeatClientEvent::PointerFrame,
            );
        } else {
            state.seat.clear_pointer_enter();
        }

        state.seat.pointer_focus = new_focus;
        if state
            .seat
            .clear_cursor_for_focus(new_focus.map(|focus| focus.target.client_id))
        {
            schedule_active_output(state, dispatcher);
        }
        return;
    }

    state.seat.pointer_focus = new_focus;

    if send_motion {
        if let Some(focus) = new_focus {
            let (x, y) = local_coords(focus.rect, state.seat.pointer_pos);
            dispatch_client_event(
                dispatcher,
                focus.target.client_id,
                SeatClientEvent::PointerMotion { time, x, y },
            );
            dispatch_client_event(
                dispatcher,
                focus.target.client_id,
                SeatClientEvent::PointerFrame,
            );
        }
    }
}

fn clamp_pointer_to_outputs(state: &mut State) {
    let Some((width, height)) = output_size(state) else {
        state.seat.pointer_pos.0 = state.seat.pointer_pos.0.max(0.0);
        state.seat.pointer_pos.1 = state.seat.pointer_pos.1.max(0.0);
        return;
    };

    let max_x = (width - 1.0).max(0.0);
    let max_y = (height - 1.0).max(0.0);
    state.seat.pointer_pos.0 = state.seat.pointer_pos.0.clamp(0.0, max_x);
    state.seat.pointer_pos.1 = state.seat.pointer_pos.1.clamp(0.0, max_y);
}

fn output_size(state: &State) -> Option<(f64, f64)> {
    let mut max_width = 0;
    let mut max_height = 0;

    for (_, output) in state.outputs.iter() {
        if !output.enabled {
            continue;
        }
        let (width, height) = output.logical_size();
        max_width = max_width.max(output.x + width);
        max_height = max_height.max(output.y + height);
    }

    if max_width <= 0 || max_height <= 0 {
        return None;
    }

    Some((max_width as f64, max_height as f64))
}

fn local_coords(rect: SurfaceRect, pointer_pos: (f64, f64)) -> (f32, f32) {
    ((pointer_pos.0 - rect.x as f64) as f32, (pointer_pos.1 - rect.y as f64) as f32)
}

fn dispatch_client_event(dispatcher: &mut Dispatcher, client_id: ClientId, event: SeatClientEvent) {
    dispatcher.dispatch_to(client_id, ClientAction::SeatEvent { event });
}

fn dispatch_client_action(dispatcher: &mut Dispatcher, client_id: ClientId, action: ClientAction) {
    dispatcher.dispatch_to(client_id, action);
}

pub fn begin_popup_grab(
    state: &mut State,
    dispatcher: &mut Dispatcher,
    client_id: ClientId,
    popup_id: XdgPopupId,
    popup_surface_id: WlSurfaceId,
) {
    state
        .seat
        .start_popup_grab(client_id, popup_id, popup_surface_id);
    if let Some(focus) = popup_keyboard_focus(state) {
        set_keyboard_focus(state, dispatcher, Some(focus));
    }
}

pub fn request_popup_grab(
    state: &mut State,
    dispatcher: &mut Dispatcher,
    client_id: ClientId,
    parent_surface_id: WlSurfaceId,
    serial: u32,
    popup_id: XdgPopupId,
    popup_surface_id: WlSurfaceId,
) -> bool {
    if !state
        .seat
        .validate_popup_grab_serial(client_id, parent_surface_id, serial)
    {
        return false;
    }
    begin_popup_grab(state, dispatcher, client_id, popup_id, popup_surface_id);
    true
}

pub fn remove_popup_grab(
    state: &mut State,
    dispatcher: &mut Dispatcher,
    popup_id: XdgPopupId,
    fallback_focus: Option<SurfaceFocus>,
) {
    state.seat.remove_popup_grab(popup_id);
    if let Some(focus) = popup_keyboard_focus(state) {
        set_keyboard_focus(state, dispatcher, Some(focus));
    } else if state.seat.popup_grab.is_none() {
        set_keyboard_focus(
            state,
            dispatcher,
            fallback_focus.or_else(|| state.seat.pointer_focus.map(|focus| focus.target)),
        );
    }
}

pub fn request_toplevel_move(
    state: &mut State,
    client_id: ClientId,
    surface_id: WlSurfaceId,
    serial: u32,
    root_surface_object_id: ObjectId,
    initial_root_pos: (i32, i32),
) -> bool {
    if !state
        .seat
        .validate_implicit_grab_serial(client_id, surface_id, serial)
    {
        return false;
    }
    state
        .seat
        .start_toplevel_move(client_id, root_surface_object_id, initial_root_pos);
    true
}

pub fn request_toplevel_resize(
    state: &mut State,
    client_id: ClientId,
    surface_id: WlSurfaceId,
    serial: u32,
    root_surface_object_id: ObjectId,
    xdg_toplevel_id: XdgToplevelId,
    edges: u32,
    initial_root_pos: (i32, i32),
    initial_size: (i32, i32),
) -> bool {
    if !state
        .seat
        .validate_implicit_grab_serial(client_id, surface_id, serial)
    {
        return false;
    }
    state.seat.start_toplevel_resize(
        client_id,
        root_surface_object_id,
        xdg_toplevel_id,
        edges,
        initial_root_pos,
        initial_size,
    );
    true
}

fn remove_resource<K, V>(map: &mut BTreeMap<K, Vec<V>>, key: K, value: V)
where
    K: Ord + Copy,
    V: PartialEq,
{
    let Some(resources) = map.get_mut(&key) else {
        return;
    };
    resources.retain(|resource| *resource != value);
    if resources.is_empty() {
        map.remove(&key);
    }
}

pub fn empty_keys() -> Array {
    Array(Vec::new())
}

fn dispatch_modifiers(
    dispatcher: &mut Dispatcher,
    client_id: ClientId,
    serial: u32,
    modifiers: KeyboardModifiers,
) {
    dispatch_client_event(
        dispatcher,
        client_id,
        SeatClientEvent::KeyboardModifiers {
            serial,
            mods_depressed: modifiers.mods_depressed,
            mods_latched: modifiers.mods_latched,
            mods_locked: modifiers.mods_locked,
            group: modifiers.group,
        },
    );
}

#[cfg(test)]
mod surface_tree_hit_test_tests {
    use {
        super::hit_test_surface,
        crate::{
            clients::ClientId,
            egl::EGL,
            globals::Globals,
            protocol_generated::{WlSubsurfaceId, WlSurfaceId},
            state::{ObjectId, State},
            wayland_objects::wl_surface::WlSurface,
            xkb::Xkb,
        },
    };

    fn make_state() -> State {
        let xkb = Xkb::new();
        let keyboard = xkb.create_keyboard_state();
        let mut globals = Globals::new();
        globals.initialize();
        State {
            objects: Default::default(),
            root_surfaces: Default::default(),
            inputs: Default::default(),
            drm: Default::default(),
            outputs: Default::default(),
            active_output: None,
            backend_syncobj_globals: Default::default(),
            globals,
            egl: EGL::new(),
            seat: Default::default(),
            keyboard,
            xkb,
        }
    }

    fn insert_surface(state: &mut State, id: u32) -> ObjectId {
        let object_id = state
            .insert_object(WlSurface::new(WlSurfaceId::from(id), ClientId::from_raw(1), 6))
            .1;
        state.register_root_surface(object_id);
        object_id
    }

    #[test]
    fn hit_test_prefers_above_then_parent_then_below() {
        let mut state = make_state();
        let root = insert_surface(&mut state, 1);
        let below = insert_surface(&mut state, 2);
        let above = insert_surface(&mut state, 3);

        {
            let root_surface = state
                .objects
                .get_mut(root)
                .downcast_mut::<WlSurface>()
                .unwrap();
            root_surface.test_map(ObjectId::from(200usize), 0, 0, 20, 20);
            root_surface.add_pending_child(below);
            root_surface.add_pending_child(above);
            assert!(root_surface.restack_child_relative(below, None, false));
            assert!(root_surface.restack_child_relative(above, None, true));
        }
        {
            let below_surface = state
                .objects
                .get_mut(below)
                .downcast_mut::<WlSurface>()
                .unwrap();
            below_surface.assign_subsurface(WlSubsurfaceId::from(31), root);
            below_surface.set_subsurface_position(0, 0);
            below_surface.test_map(ObjectId::from(201usize), 0, 0, 20, 20);
        }
        {
            let above_surface = state
                .objects
                .get_mut(above)
                .downcast_mut::<WlSurface>()
                .unwrap();
            above_surface.assign_subsurface(WlSubsurfaceId::from(32), root);
            above_surface.set_subsurface_position(0, 0);
            above_surface.test_map(ObjectId::from(202usize), 0, 0, 20, 20);
        }
        {
            let mut root_surface = state.objects.grab_object::<WlSurface>(root);
            assert!(root_surface.apply_pending_tree_state(&mut state));
        }

        let hit = hit_test_surface(&state, &state.drm, 5.0, 5.0).unwrap();
        assert_eq!(hit.target.surface_id, WlSurfaceId::from(3));
    }

    #[test]
    fn hit_test_uses_nested_offsets_and_falls_back_when_child_rejects_input() {
        let mut state = make_state();
        let root = insert_surface(&mut state, 1);
        let child = insert_surface(&mut state, 2);
        let grandchild = insert_surface(&mut state, 3);

        {
            let root_surface = state
                .objects
                .get_mut(root)
                .downcast_mut::<WlSurface>()
                .unwrap();
            root_surface.test_map(ObjectId::from(210usize), 50, 60, 100, 100);
            root_surface.add_pending_child(child);
            assert!(root_surface.restack_child_relative(child, None, true));
        }
        {
            let child_surface = state
                .objects
                .get_mut(child)
                .downcast_mut::<WlSurface>()
                .unwrap();
            child_surface.assign_subsurface(WlSubsurfaceId::from(41), root);
            child_surface.set_subsurface_position(10, 10);
            child_surface.test_map(ObjectId::from(211usize), 0, 0, 40, 40);
            child_surface.add_pending_child(grandchild);
        }
        {
            let grandchild_surface = state
                .objects
                .get_mut(grandchild)
                .downcast_mut::<WlSurface>()
                .unwrap();
            grandchild_surface.assign_subsurface(WlSubsurfaceId::from(42), child);
            grandchild_surface.set_subsurface_position(5, 6);
            grandchild_surface.test_map(ObjectId::from(212usize), 0, 0, 20, 20);
            grandchild_surface.test_set_input_region(crate::region::RegionValue::empty());
        }
        {
            let mut root_surface = state.objects.grab_object::<WlSurface>(root);
            assert!(root_surface.apply_pending_tree_state(&mut state));
        }
        {
            let mut child_surface = state.objects.grab_object::<WlSurface>(child);
            assert!(child_surface.apply_pending_tree_state(&mut state));
        }

        let fallback_hit = hit_test_surface(&state, &state.drm, 66.0, 77.0).unwrap();
        assert_eq!(fallback_hit.target.surface_id, WlSurfaceId::from(2));

        let parent_hit = hit_test_surface(&state, &state.drm, 145.0, 145.0).unwrap();
        assert_eq!(parent_hit.target.surface_id, WlSurfaceId::from(1));
    }

    #[test]
    fn hit_test_uses_explicit_root_surface_order() {
        let mut state = make_state();
        let root_a = insert_surface(&mut state, 1);
        let root_b = insert_surface(&mut state, 2);

        {
            let root_a_surface = state
                .objects
                .get_mut(root_a)
                .downcast_mut::<WlSurface>()
                .unwrap();
            root_a_surface.test_map(ObjectId::from(220usize), 0, 0, 20, 20);
        }
        {
            let root_b_surface = state
                .objects
                .get_mut(root_b)
                .downcast_mut::<WlSurface>()
                .unwrap();
            root_b_surface.test_map(ObjectId::from(221usize), 0, 0, 20, 20);
        }

        state.register_root_surface(root_a);

        let hit = hit_test_surface(&state, &state.drm, 5.0, 5.0).unwrap();
        assert_eq!(hit.target.surface_id, WlSurfaceId::from(1));
    }
}

#[cfg(test)]
mod xdg_seat_tests {
    use {
        super::{begin_popup_grab, remove_popup_grab, SeatState, SurfaceFocus},
        crate::{
            clients::{Client, ClientId, RawEvent},
            egl::EGL,
            globals::Globals,
            object::Object,
            protocol_generated::{
                xdg_toplevel, Constants, WlSeatId, WlSurfaceId, XdgPopupId, XdgPositionerId,
                XdgSurfaceId, XdgToplevelId, XdgWmBaseId,
            },
            runner::Dispatcher,
            state::{ObjectId, State},
            wayland_objects::{
                wl_seat::WlSeat,
                wl_surface::{Role, WlSurface},
                xdg_popup::XdgPopup,
                xdg_positioner::{PositionerState, XdgPositioner},
                xdg_surface::{xdg_toplevel::XdgToplevel, XdgSurface},
                xdg_wm_base::XdgWmBase,
            },
            xkb::Xkb,
        },
    };

    fn make_state() -> State {
        let xkb = Xkb::new();
        let keyboard = xkb.create_keyboard_state();
        let mut globals = Globals::new();
        globals.initialize();
        State {
            objects: Default::default(),
            root_surfaces: Default::default(),
            inputs: Default::default(),
            drm: Default::default(),
            outputs: Default::default(),
            active_output: None,
            backend_syncobj_globals: Default::default(),
            globals,
            egl: EGL::new(),
            seat: SeatState::default(),
            keyboard,
            xkb,
        }
    }

    fn make_client() -> (Client, Dispatcher) {
        (Client::new_for_tests(ClientId::from_raw(7)), Dispatcher::default())
    }

    fn insert_surface(state: &mut State, client: &mut Client, surface_id: WlSurfaceId) -> ObjectId {
        let ((_, object_id), _) =
            client.insert_object(state, WlSurface::new(surface_id, client.client_id(), 6));
        state.register_root_surface(object_id);
        object_id
    }

    fn insert_xdg_surface(
        state: &mut State,
        client: &mut Client,
        dispatcher: &mut crate::clients::ClientDispatcher<'_>,
        wl_surface_id: WlSurfaceId,
        xdg_surface_id: XdgSurfaceId,
    ) -> (ObjectId, ObjectId) {
        let wl_surface_object_id = insert_surface(state, client, wl_surface_id);
        {
            let wl_surface = state
                .objects
                .get_mut(wl_surface_object_id)
                .downcast_mut::<WlSurface>()
                .unwrap();
            assert!(wl_surface.set_role(Role::XdgSurface, dispatcher).is_ok());
            wl_surface.set_impl(xdg_surface_id);
        }
        let ((_, xdg_surface_object_id), _) = client.insert_object(
            state,
            XdgSurface::new(xdg_surface_id, XdgWmBaseId::from(1), wl_surface_id, 7),
        );
        (wl_surface_object_id, xdg_surface_object_id)
    }

    fn get_toplevel_request(
        xdg_surface_id: XdgSurfaceId,
        toplevel_id: XdgToplevelId,
    ) -> crate::clients::RawRequest<'static> {
        let mut event = RawEvent::new(xdg_surface_id.into(), 1, 1);
        event.id(toplevel_id);
        (&*event.into_bytes()).try_into().unwrap()
    }

    fn popup_grab_request(
        popup_id: XdgPopupId,
        seat_id: WlSeatId,
        serial: u32,
    ) -> crate::clients::RawRequest<'static> {
        let mut event = RawEvent::new(popup_id.into(), 1, 2);
        event.id(seat_id);
        event.uint(serial);
        (&*event.into_bytes()).try_into().unwrap()
    }

    fn toplevel_resize_request(
        toplevel_id: XdgToplevelId,
        seat_id: WlSeatId,
        serial: u32,
        edges: u32,
    ) -> crate::clients::RawRequest<'static> {
        let mut event = RawEvent::new(toplevel_id.into(), 6, 3);
        event.id(seat_id);
        event.uint(serial);
        event.uint(edges);
        (&*event.into_bytes()).try_into().unwrap()
    }

    fn get_popup_request(
        xdg_surface_id: XdgSurfaceId,
        popup_id: XdgPopupId,
        parent_id: XdgSurfaceId,
        positioner_id: XdgPositionerId,
    ) -> crate::clients::RawRequest<'static> {
        let mut event = RawEvent::new(xdg_surface_id.into(), 2, 3);
        event.id(popup_id);
        event.id(parent_id);
        event.id(positioner_id);
        (&*event.into_bytes()).try_into().unwrap()
    }

    fn positioner_set_size_request(
        positioner_id: XdgPositionerId,
        width: i32,
        height: i32,
    ) -> crate::clients::RawRequest<'static> {
        let mut event = RawEvent::new(positioner_id.into(), 1, 2);
        event.int(width);
        event.int(height);
        (&*event.into_bytes()).try_into().unwrap()
    }

    fn positioner_set_anchor_rect_request(
        positioner_id: XdgPositionerId,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
    ) -> crate::clients::RawRequest<'static> {
        let mut event = RawEvent::new(positioner_id.into(), 2, 4);
        event.int(x);
        event.int(y);
        event.int(width);
        event.int(height);
        (&*event.into_bytes()).try_into().unwrap()
    }

    fn assign_toplevel_role(
        state: &mut State,
        client: &mut Client,
        dispatcher: &mut crate::clients::ClientDispatcher<'_>,
        xdg_surface_object_id: ObjectId,
        xdg_surface_id: XdgSurfaceId,
        toplevel_id: XdgToplevelId,
    ) -> ObjectId {
        let mut xdg_surface = state
            .objects
            .grab_object::<XdgSurface>(xdg_surface_object_id);
        xdg_surface.on(
            client,
            state,
            get_toplevel_request(xdg_surface_id, toplevel_id),
            dispatcher,
        );
        drop(xdg_surface);
        client.object_id(toplevel_id)
    }

    #[test]
    fn popup_grab_dismisses_popup_when_serial_is_invalid() {
        let mut state = make_state();
        let (mut client, mut dispatcher) = make_client();
        let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);

        client.insert_object(&mut state, WlSeat::new(WlSeatId::from(2)));

        let (_parent_wl_surface_object_id, parent_xdg_surface_object_id) = insert_xdg_surface(
            &mut state,
            &mut client,
            &mut client_dispatcher,
            WlSurfaceId::from(10),
            XdgSurfaceId::from(20),
        );
        let _parent_toplevel_object_id = assign_toplevel_role(
            &mut state,
            &mut client,
            &mut client_dispatcher,
            parent_xdg_surface_object_id,
            XdgSurfaceId::from(20),
            XdgToplevelId::from(30),
        );
        let (_popup_wl_surface_object_id, _popup_xdg_surface_object_id) = insert_xdg_surface(
            &mut state,
            &mut client,
            &mut client_dispatcher,
            WlSurfaceId::from(11),
            XdgSurfaceId::from(21),
        );

        let ((_, popup_object_id), _) = client.insert_object(
            &mut state,
            XdgPopup::new(
                XdgPopupId::from(40),
                XdgSurfaceId::from(21),
                Some(XdgSurfaceId::from(20)),
                PositionerState::default(),
                XdgWmBase::INTERFACE_VERSION,
            ),
        );

        {
            let mut popup = state.objects.grab_object::<XdgPopup>(popup_object_id);
            popup.on(
                &mut client,
                &mut state,
                popup_grab_request(XdgPopupId::from(40), WlSeatId::from(2), 999),
                &mut client_dispatcher,
            );
        }

        assert!(!state.objects.contains(popup_object_id));
        assert!(!client.contains_object(XdgPopupId::from(40), &state));
        assert!(state.seat.popup_grab.is_none());
        assert_eq!(state.seat.keyboard_focus, None);
    }

    #[test]
    fn popup_grab_requires_matching_implicit_serial() {
        let mut state = make_state();
        let (mut client, mut dispatcher) = make_client();
        let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);

        client.insert_object(&mut state, WlSeat::new(WlSeatId::from(2)));

        let (parent_wl_surface_object_id, parent_xdg_surface_object_id) = insert_xdg_surface(
            &mut state,
            &mut client,
            &mut client_dispatcher,
            WlSurfaceId::from(10),
            XdgSurfaceId::from(20),
        );
        let _parent_toplevel_object_id = assign_toplevel_role(
            &mut state,
            &mut client,
            &mut client_dispatcher,
            parent_xdg_surface_object_id,
            XdgSurfaceId::from(20),
            XdgToplevelId::from(30),
        );
        let (_popup_wl_surface_object_id, popup_xdg_surface_object_id) = insert_xdg_surface(
            &mut state,
            &mut client,
            &mut client_dispatcher,
            WlSurfaceId::from(11),
            XdgSurfaceId::from(21),
        );

        let ((_, popup_object_id), _) = client.insert_object(
            &mut state,
            XdgPopup::new(
                XdgPopupId::from(40),
                XdgSurfaceId::from(21),
                Some(XdgSurfaceId::from(20)),
                PositionerState::default(),
                XdgWmBase::INTERFACE_VERSION,
            ),
        );

        let parent_focus = SurfaceFocus {
            client_id: client.client_id(),
            surface_id: WlSurfaceId::from(10),
        };
        let serial = state.seat.next_serial();
        state.seat.record_keyboard_trigger(serial, parent_focus);

        {
            let mut popup = state.objects.grab_object::<XdgPopup>(popup_object_id);
            popup.on(
                &mut client,
                &mut state,
                popup_grab_request(XdgPopupId::from(40), WlSeatId::from(2), serial),
                &mut client_dispatcher,
            );
        }

        let popup = state
            .objects
            .get(popup_object_id)
            .downcast_ref::<XdgPopup>()
            .unwrap();
        assert!(popup.has_explicit_grab());
        assert_eq!(
            state.seat.popup_grab.as_ref().unwrap().popup_stack,
            vec![(XdgPopupId::from(40), WlSurfaceId::from(11))]
        );
        assert_eq!(
            state.seat.keyboard_focus,
            Some(SurfaceFocus {
                client_id: client.client_id(),
                surface_id: WlSurfaceId::from(11)
            })
        );

        let parent_surface = state
            .objects
            .get(parent_wl_surface_object_id)
            .downcast_ref::<WlSurface>()
            .unwrap();
        assert_eq!(parent_surface.surface_id(), WlSurfaceId::from(10));
        let popup_xdg_surface = state
            .objects
            .get(popup_xdg_surface_object_id)
            .downcast_ref::<XdgSurface>()
            .unwrap();
        assert_eq!(popup_xdg_surface.wl_surface_id(), WlSurfaceId::from(11));
    }

    #[test]
    fn popup_parent_click_dismisses_grab() {
        let mut state = make_state();
        let (_client, mut dispatcher) = make_client();

        let parent_focus = SurfaceFocus {
            client_id: ClientId::from_raw(7),
            surface_id: WlSurfaceId::from(50),
        };
        let popup_focus = SurfaceFocus {
            client_id: ClientId::from_raw(7),
            surface_id: WlSurfaceId::from(51),
        };

        begin_popup_grab(
            &mut state,
            &mut dispatcher,
            parent_focus.client_id,
            XdgPopupId::from(60),
            popup_focus.surface_id,
        );
        state.seat.pointer_focus = Some(super::PointerFocus {
            target: parent_focus,
            rect: super::SurfaceRect { x: 0, y: 0, width: 100, height: 100 },
        });

        assert!(super::maybe_dismiss_popup_grab_on_button(&mut state, &mut dispatcher, 1, 1000));
        assert!(state.seat.popup_grab.is_none());
    }

    #[test]
    fn removing_last_popup_grab_restores_parent_focus() {
        let mut state = make_state();
        let (_client, mut dispatcher) = make_client();

        let parent_focus = SurfaceFocus {
            client_id: ClientId::from_raw(7),
            surface_id: WlSurfaceId::from(50),
        };
        let popup_focus = SurfaceFocus {
            client_id: ClientId::from_raw(7),
            surface_id: WlSurfaceId::from(51),
        };

        state.seat.keyboard_focus = Some(parent_focus);
        begin_popup_grab(
            &mut state,
            &mut dispatcher,
            parent_focus.client_id,
            XdgPopupId::from(60),
            popup_focus.surface_id,
        );
        state.seat.keyboard_focus = Some(popup_focus);

        remove_popup_grab(&mut state, &mut dispatcher, XdgPopupId::from(60), Some(parent_focus));

        assert!(state.seat.popup_grab.is_none());
        assert_eq!(state.seat.keyboard_focus, Some(parent_focus));
    }

    #[test]
    fn toplevel_resize_rejects_none_edge_without_starting_grab() {
        let mut state = make_state();
        let (mut client, mut dispatcher) = make_client();
        let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);

        client.insert_object(&mut state, WlSeat::new(WlSeatId::from(3)));

        let (wl_surface_object_id, xdg_surface_object_id) = insert_xdg_surface(
            &mut state,
            &mut client,
            &mut client_dispatcher,
            WlSurfaceId::from(70),
            XdgSurfaceId::from(80),
        );
        let toplevel_object_id = assign_toplevel_role(
            &mut state,
            &mut client,
            &mut client_dispatcher,
            xdg_surface_object_id,
            XdgSurfaceId::from(80),
            XdgToplevelId::from(90),
        );

        {
            let surface = state
                .objects
                .get_mut(wl_surface_object_id)
                .downcast_mut::<WlSurface>()
                .unwrap();
            surface.test_map(ObjectId::from(500usize), 40, 50, 120, 80);
        }

        let focus = SurfaceFocus {
            client_id: client.client_id(),
            surface_id: WlSurfaceId::from(70),
        };
        let serial = state.seat.next_serial();
        state.seat.record_pointer_button(1, serial, Some(focus));

        {
            let mut toplevel = state.objects.grab_object::<XdgToplevel>(toplevel_object_id);
            toplevel.on(
                &mut client,
                &mut state,
                toplevel_resize_request(
                    XdgToplevelId::from(90),
                    WlSeatId::from(3),
                    serial,
                    xdg_toplevel::ResizeEdge::None as u32,
                ),
                &mut client_dispatcher,
            );
        }

        assert!(state.seat.pointer_interactive_grab.is_none());

        {
            let mut toplevel = state.objects.grab_object::<XdgToplevel>(toplevel_object_id);
            toplevel.on(
                &mut client,
                &mut state,
                toplevel_resize_request(
                    XdgToplevelId::from(90),
                    WlSeatId::from(3),
                    serial,
                    xdg_toplevel::ResizeEdge::Right as u32,
                ),
                &mut client_dispatcher,
            );
        }

        assert!(matches!(
            state.seat.pointer_interactive_grab,
            Some(super::PointerInteractiveGrab::Resize { edges, .. })
                if edges == xdg_toplevel::ResizeEdge::Right as u32
        ));
    }

    #[test]
    fn toplevel_resize_accepts_keyboard_trigger_serial() {
        let mut state = make_state();
        let (mut client, mut dispatcher) = make_client();
        let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);

        client.insert_object(&mut state, WlSeat::new(WlSeatId::from(4)));

        let (wl_surface_object_id, xdg_surface_object_id) = insert_xdg_surface(
            &mut state,
            &mut client,
            &mut client_dispatcher,
            WlSurfaceId::from(100),
            XdgSurfaceId::from(110),
        );
        let toplevel_object_id = assign_toplevel_role(
            &mut state,
            &mut client,
            &mut client_dispatcher,
            xdg_surface_object_id,
            XdgSurfaceId::from(110),
            XdgToplevelId::from(120),
        );

        {
            let surface = state
                .objects
                .get_mut(wl_surface_object_id)
                .downcast_mut::<WlSurface>()
                .unwrap();
            surface.test_map(ObjectId::from(700usize), 10, 20, 80, 60);
        }

        let focus = SurfaceFocus {
            client_id: client.client_id(),
            surface_id: WlSurfaceId::from(100),
        };
        let serial = state.seat.next_serial();
        state.seat.record_keyboard_trigger(serial, focus);

        {
            let mut toplevel = state.objects.grab_object::<XdgToplevel>(toplevel_object_id);
            toplevel.on(
                &mut client,
                &mut state,
                toplevel_resize_request(
                    XdgToplevelId::from(120),
                    WlSeatId::from(4),
                    serial,
                    xdg_toplevel::ResizeEdge::BottomRight as u32,
                ),
                &mut client_dispatcher,
            );
        }

        assert!(matches!(
            state.seat.pointer_interactive_grab,
            Some(super::PointerInteractiveGrab::Resize { edges, .. })
                if edges == xdg_toplevel::ResizeEdge::BottomRight as u32
        ));
    }

    #[test]
    fn get_popup_rejects_null_parent_without_poisoning_surface_role() {
        let mut state = make_state();
        let (mut client, mut dispatcher) = make_client();
        let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);

        let (_wl_surface_object_id, xdg_surface_object_id) = insert_xdg_surface(
            &mut state,
            &mut client,
            &mut client_dispatcher,
            WlSurfaceId::from(130),
            XdgSurfaceId::from(140),
        );
        let ((_, positioner_object_id), _) =
            client.insert_object(&mut state, XdgPositioner::new(XdgPositionerId::from(150)));

        {
            let mut positioner = state
                .objects
                .grab_object::<XdgPositioner>(positioner_object_id);
            positioner.on(
                &mut client,
                &mut state,
                positioner_set_size_request(XdgPositionerId::from(150), 40, 30),
                &mut client_dispatcher,
            );
            positioner.on(
                &mut client,
                &mut state,
                positioner_set_anchor_rect_request(XdgPositionerId::from(150), 0, 0, 10, 10),
                &mut client_dispatcher,
            );
        }

        {
            let mut xdg_surface = state
                .objects
                .grab_object::<XdgSurface>(xdg_surface_object_id);
            xdg_surface.on(
                &mut client,
                &mut state,
                get_popup_request(
                    XdgSurfaceId::from(140),
                    XdgPopupId::from(160),
                    XdgSurfaceId::from(0),
                    XdgPositionerId::from(150),
                ),
                &mut client_dispatcher,
            );
        }

        assert!(!client.contains_object(XdgPopupId::from(160), &state));

        {
            let mut xdg_surface = state
                .objects
                .grab_object::<XdgSurface>(xdg_surface_object_id);
            xdg_surface.on(
                &mut client,
                &mut state,
                get_toplevel_request(XdgSurfaceId::from(140), XdgToplevelId::from(170)),
                &mut client_dispatcher,
            );
        }

        assert!(client.contains_object(XdgToplevelId::from(170), &state));
    }
}
