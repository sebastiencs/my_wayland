use std::collections::BTreeMap;

use crate::{
    clients::{ClientAction, ClientId},
    drm::Drm,
    object::Object,
    protocol_generated::{Array, WlKeyboardId, WlPointerId, WlSurfaceId},
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

fn hit_test_subtree(
    objects: &IndexMap<ObjectId, Box<dyn Object>>,
    surface_object_id: ObjectId,
    base_x: i32,
    base_y: i32,
    x: f64,
    y: f64,
) -> Option<PointerFocus> {
    let object = objects.try_get(surface_object_id)?;
    let surface = object.downcast_ref::<WlSurface>()?;
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
                let Some(child_object) = objects.try_get(child_surface_object_id) else {
                    continue;
                };
                let Some(child_surface) = child_object.downcast_ref::<WlSurface>() else {
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
        let Some(object) = state.objects.try_get(object_id) else {
            continue;
        };
        let Some(surface) = object.downcast_ref::<WlSurface>() else {
            continue;
        };
        if !surface.is_root() {
            continue;
        }
        if let Some(root_hit) = hit_test_subtree(&state.objects, object_id, 0, 0, x, y) {
            hit = Some(root_hit);
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
            let mut root_object = state.objects.grab(root);
            let root_surface = root_object.downcast_mut::<WlSurface>().unwrap();
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
            let mut root_object = state.objects.grab(root);
            let root_surface = root_object.downcast_mut::<WlSurface>().unwrap();
            assert!(root_surface.apply_pending_tree_state(&mut state));
        }
        {
            let mut child_object = state.objects.grab(child);
            let child_surface = child_object.downcast_mut::<WlSurface>().unwrap();
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
