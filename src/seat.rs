use std::collections::BTreeMap;

use crate::{
    clients::{ClientAction, ClientId},
    drm::Drm,
    object::Object,
    protocol_generated::{Array, WlKeyboardId, WlPointerId, WlSurfaceId},
    runner::Dispatcher,
    state::{DrmId, IndexMap, ObjectId, State},
    wayland_objects::{wl_buffer::WlBuffer, wl_surface::WlSurface},
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

pub struct SeatState {
    current_serial: u32,
    pointer_pos: (f64, f64),
    pointer_focus: Option<PointerFocus>,
    keyboard_focus: Option<SurfaceFocus>,
    pointer_resources: BTreeMap<ClientId, Vec<WlPointerId>>,
    keyboard_resources: BTreeMap<ClientId, Vec<WlKeyboardId>>,
}

impl Default for SeatState {
    fn default() -> Self {
        Self {
            current_serial: 1,
            pointer_pos: (0.0, 0.0),
            pointer_focus: None,
            keyboard_focus: None,
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

    pub fn remove_client(&mut self, client_id: ClientId) {
        self.pointer_resources.remove(&client_id);
        self.keyboard_resources.remove(&client_id);
        if self
            .pointer_focus
            .is_some_and(|focus| focus.target.client_id == client_id)
        {
            self.pointer_focus = None;
        }
        if self
            .keyboard_focus
            .is_some_and(|focus| focus.client_id == client_id)
        {
            self.keyboard_focus = None;
        }
    }

    fn next_serial(&mut self) -> u32 {
        let serial = self.current_serial;
        self.current_serial = self.current_serial.wrapping_add(1).max(1);
        serial
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
    refresh_pointer_focus(state, dispatcher, time, true);
}

pub fn handle_pointer_button(
    state: &mut State,
    dispatcher: &mut Dispatcher,
    button: u32,
    button_state: u32,
    time: u32,
) {
    refresh_pointer_focus(state, dispatcher, time, false);
    let pointer_focus = state.seat.pointer_focus;
    log::debug!(
        "seat pointer button button={} state={} time={} focus={:?}",
        button,
        button_state,
        time,
        pointer_focus.map(|focus| focus.target)
    );
    if button_state == 1 {
        set_keyboard_focus(state, dispatcher, pointer_focus.map(|focus| focus.target));
    }

    let Some(pointer_focus) = state.seat.pointer_focus else {
        return;
    };

    let serial = state.seat.next_serial();
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
    refresh_pointer_focus(state, dispatcher, time, true);
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

fn hit_test_surface(
    objects: &IndexMap<ObjectId, Box<dyn Object>>,
    _drm: &IndexMap<DrmId, Drm>,
    x: f64,
    y: f64,
) -> Option<PointerFocus> {
    let mut hit = None;

    for (_, object) in objects.iter() {
        let Some(surface) = object.downcast_ref::<WlSurface>() else {
            continue;
        };
        let Some(surface_state) = surface.render_state() else {
            continue;
        };
        let Some(buffer_object) = objects.try_get(surface_state.buffer_object_id) else {
            continue;
        };
        let Some(buffer) = buffer_object.downcast_ref::<WlBuffer>() else {
            continue;
        };

        let (width, height) = buffer.dimensions();
        // Hit-testing stays in compositor logical coordinates; the render path projects this space.
        let rect = SurfaceRect { x: surface_state.x, y: surface_state.y, width, height };

        let inside = x >= rect.x as f64
            && y >= rect.y as f64
            && x < (rect.x + rect.width) as f64
            && y < (rect.y + rect.height) as f64;
        if inside {
            hit = Some(PointerFocus {
                target: SurfaceFocus {
                    client_id: surface.client_id(),
                    surface_id: surface.surface_id(),
                },
                rect,
            });
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
    let new_focus = hit_test_surface(
        &state.objects,
        &state.drm,
        state.seat.pointer_pos.0,
        state.seat.pointer_pos.1,
    );

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
        }

        state.seat.pointer_focus = new_focus;
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
    let (_, drm) = state.drm.iter().next()?;
    let mode = drm.output.in_use.connector.preferred_mode.as_ref()?;
    let (width, height) = mode.size();
    Some((width as f64, height as f64))
}

fn local_coords(rect: SurfaceRect, pointer_pos: (f64, f64)) -> (f32, f32) {
    ((pointer_pos.0 - rect.x as f64) as f32, (pointer_pos.1 - rect.y as f64) as f32)
}

fn dispatch_client_event(dispatcher: &mut Dispatcher, client_id: ClientId, event: SeatClientEvent) {
    dispatcher.dispatch_to(client_id, ClientAction::SeatEvent { event });
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
