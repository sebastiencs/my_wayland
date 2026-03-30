use std::collections::{BTreeMap, BTreeSet, VecDeque};

use crate::{
    clients::{ClientAction, ClientId},
    config,
    cursor::{CursorSourceRef, CursorState},
    drm::schedule_outputs_for_points,
    input::{
        maybe_dismiss_popup_grab_on_button, popup_keyboard_focus, refresh_drag_focus,
        set_keyboard_focus, surface_focus_pick_at, ActiveGrabState, HitSemantics, PointerFocus,
        PointerInteractiveGrab, ScenePick, SurfaceFocusPick, WindowHit,
    },
    protocol_generated::{
        Array, WlDataDeviceId, WlDataOfferId, WlDataSourceId, WlKeyboardId, WlPointerId, WlSeatId,
        WlSurfaceId, XdgPopupId, XdgToplevelId,
    },
    runner::Dispatcher,
    state::{
        DragNegotiationState, DragPhase, DragState, DragTargetState, ObjectId,
        SelectionSourceState, State,
    },
    tiling::TiledResizeState,
    wayland_objects::{
        xdg_surface::xdg_toplevel::InteractiveSizeConstraints,
        zwlr_layer_surface_v1::LayerLayoutObjectSnapshot,
    },
    xkb::KeyboardModifiers,
};

pub use crate::input::SurfaceFocus;
pub(crate) use crate::input::SurfaceRect;

const POINTER_MOVE_CURSOR_ICON_NAMES: &[&str] = &["grab", "grabbing", "move"];
const POINTER_RESIZE_NW_CURSOR_ICON_NAMES: &[&str] = &["nw-resize", "size_fdiag"];
const POINTER_RESIZE_NE_CURSOR_ICON_NAMES: &[&str] = &["ne-resize", "size_bdiag"];
const POINTER_RESIZE_SW_CURSOR_ICON_NAMES: &[&str] = &["sw-resize", "size_bdiag"];
const POINTER_RESIZE_SE_CURSOR_ICON_NAMES: &[&str] = &["se-resize", "size_fdiag"];
const POINTER_RESIZE_N_CURSOR_ICON_NAMES: &[&str] = &["n-resize", "size_ver"];
const POINTER_RESIZE_S_CURSOR_ICON_NAMES: &[&str] = &["s-resize", "size_ver"];
const POINTER_RESIZE_W_CURSOR_ICON_NAMES: &[&str] = &["w-resize", "size_hor"];
const POINTER_RESIZE_E_CURSOR_ICON_NAMES: &[&str] = &["e-resize", "size_hor"];

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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct SeatClientRemovalResult {
    pub cursor_changed: bool,
    pub interactive_grab_cleared: bool,
}

#[derive(Clone, Copy, Debug)]
struct KeyboardTriggerState {
    serial: u32,
    focus: SurfaceFocus,
}

#[derive(Clone, Copy, Debug)]
struct SerialRange {
    lo: u32,
    hi: u32,
}

const MAX_TRACKED_SERIAL_RANGES: usize = 32;

pub struct SeatState {
    current_serial: u32,
    pub(crate) pointer_pos: (f64, f64),
    pub(crate) pointer_focus: Option<SurfaceFocusPick>,
    last_pointer_enter: Option<PointerEnterState>,
    pub(crate) keyboard_focus: Option<SurfaceFocus>,
    pub(crate) pointer_button_count: u32,
    last_pointer_press: Option<PointerPressState>,
    last_keyboard_trigger: Option<KeyboardTriggerState>,
    cursor: CursorState,
    popup_grab: Option<PopupGrabState>,
    pub(crate) pointer_interactive_grab: Option<ActiveGrabState>,
    client_serials: BTreeMap<ClientId, VecDeque<SerialRange>>,
    data_device_resources: BTreeMap<ClientId, Vec<WlDataDeviceId>>,
    selection_source: Option<SelectionSourceState>,
    selection_serial: u32,
    active_drag: Option<DragState>,
    pointer_resources: BTreeMap<ClientId, Vec<WlPointerId>>,
    keyboard_resources: BTreeMap<ClientId, Vec<WlKeyboardId>>,
    intercepted_keyboard_keys: BTreeSet<u32>,
    intercepted_pointer_buttons: BTreeSet<u32>,
}

impl Default for SeatState {
    fn default() -> Self {
        let mut cursor = CursorState::default();
        let _ = cursor.ensure_default_named_cursor();
        Self {
            current_serial: 1,
            pointer_pos: (0.0, 0.0),
            pointer_focus: None,
            last_pointer_enter: None,
            keyboard_focus: None,
            pointer_button_count: 0,
            last_pointer_press: None,
            last_keyboard_trigger: None,
            cursor,
            popup_grab: None,
            pointer_interactive_grab: None,
            client_serials: BTreeMap::default(),
            data_device_resources: BTreeMap::default(),
            selection_source: None,
            selection_serial: 0,
            active_drag: None,
            pointer_resources: BTreeMap::default(),
            keyboard_resources: BTreeMap::default(),
            intercepted_keyboard_keys: BTreeSet::default(),
            intercepted_pointer_buttons: BTreeSet::default(),
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

    pub fn register_data_device(&mut self, client_id: ClientId, data_device_id: WlDataDeviceId) {
        let resources = self.data_device_resources.entry(client_id).or_default();
        if !resources.contains(&data_device_id) {
            resources.push(data_device_id);
        }
    }

    pub fn unregister_data_device(&mut self, client_id: ClientId, data_device_id: WlDataDeviceId) {
        remove_resource(&mut self.data_device_resources, client_id, data_device_id);
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

    pub fn data_device_resources(&self, client_id: ClientId) -> Vec<WlDataDeviceId> {
        self.data_device_resources
            .get(&client_id)
            .cloned()
            .unwrap_or_default()
    }

    pub fn selection_source(&self) -> Option<SelectionSourceState> {
        self.selection_source
    }

    pub fn selection_serial(&self) -> u32 {
        self.selection_serial
    }

    pub fn replace_selection_source(
        &mut self,
        source: Option<SelectionSourceState>,
        serial: u32,
    ) -> Option<SelectionSourceState> {
        self.selection_serial = serial;
        std::mem::replace(&mut self.selection_source, source)
    }

    pub fn validate_selection_serial(&self, serial: u32) -> bool {
        let current = self.selection_serial;
        current == 0 || current.wrapping_sub(serial) >= (u32::MAX / 2)
    }

    pub fn validate_data_transfer_serial(
        &self,
        client_id: ClientId,
        surface_id: WlSurfaceId,
        serial: u32,
    ) -> bool {
        self.pointer_button_count > 0
            && self.last_pointer_press.is_some_and(|press| {
                press.serial == serial
                    && press.focus.client_id == client_id
                    && press.focus.surface_id == surface_id
            })
    }

    pub fn validate_client_serial(&self, client_id: ClientId, serial: u32) -> bool {
        let Some(serials) = self.client_serials.get(&client_id) else {
            return false;
        };
        for range in serials.iter().rev() {
            if serial.wrapping_sub(range.hi) as i32 > 0 {
                return false;
            }
            if serial.wrapping_sub(range.lo) as i32 >= 0 {
                return true;
            }
        }
        false
    }

    pub fn active_drag(&self) -> Option<DragState> {
        self.active_drag.clone()
    }

    pub fn active_drag_mut(&mut self) -> Option<&mut DragState> {
        self.active_drag.as_mut()
    }

    pub fn set_active_drag(&mut self, drag: DragState) -> Option<DragState> {
        std::mem::replace(&mut self.active_drag, Some(drag))
    }

    pub fn replace_active_drag(&mut self, drag: Option<DragState>) -> Option<DragState> {
        std::mem::replace(&mut self.active_drag, drag)
    }

    pub fn clear_active_drag(&mut self) -> Option<DragState> {
        self.replace_active_drag(None)
    }

    pub fn set_active_drag_target(&mut self, target: Option<DragTargetState>) -> bool {
        let Some(drag) = self.active_drag.as_mut() else {
            return false;
        };
        if drag.target == target {
            return false;
        }
        drag.target = target;
        drag.negotiation = DragNegotiationState::default();
        drag.phase = DragPhase::Dragging;
        true
    }

    pub fn register_active_drag_offer(
        &mut self,
        target_client_id: ClientId,
        offer_id: WlDataOfferId,
        has_finish_capable_offer: bool,
    ) -> bool {
        let Some(drag) = self.active_drag.as_mut() else {
            return false;
        };
        if drag
            .target
            .is_none_or(|target| target.client_id != target_client_id)
        {
            return false;
        }
        if drag.negotiation.offer_ids.contains(&offer_id) {
            return false;
        }
        drag.negotiation.offer_ids.push(offer_id);
        drag.negotiation.has_finish_capable_offer |= has_finish_capable_offer;
        true
    }

    pub fn unregister_active_drag_offer(
        &mut self,
        target_client_id: ClientId,
        offer_id: WlDataOfferId,
    ) -> bool {
        let Some(drag) = self.active_drag.as_mut() else {
            return false;
        };
        if drag
            .target
            .is_none_or(|target| target.client_id != target_client_id)
        {
            return false;
        }
        let len_before = drag.negotiation.offer_ids.len();
        drag.negotiation
            .offer_ids
            .retain(|candidate| *candidate != offer_id);
        len_before != drag.negotiation.offer_ids.len()
    }

    pub fn active_drag_contains_offer(
        &self,
        target_client_id: ClientId,
        offer_id: WlDataOfferId,
    ) -> bool {
        self.active_drag.as_ref().is_some_and(|drag| {
            drag.target
                .is_some_and(|target| target.client_id == target_client_id)
                && drag.negotiation.offer_ids.contains(&offer_id)
        })
    }

    pub fn active_drag_offer_ids(&self, target_client_id: ClientId) -> Vec<WlDataOfferId> {
        self.active_drag
            .as_ref()
            .and_then(|drag| {
                drag.target
                    .is_some_and(|target| target.client_id == target_client_id)
                    .then(|| drag.negotiation.offer_ids.clone())
            })
            .unwrap_or_default()
    }

    pub fn set_active_drag_accepted_mime(
        &mut self,
        target_client_id: ClientId,
        offer_id: WlDataOfferId,
        accepted_mime: Option<String>,
    ) -> bool {
        let Some(drag) = self.active_drag.as_mut() else {
            return false;
        };
        if drag
            .target
            .is_none_or(|target| target.client_id != target_client_id)
            || !drag.negotiation.offer_ids.contains(&offer_id)
        {
            return false;
        }
        if drag.negotiation.accepted_mime == accepted_mime {
            return false;
        }
        drag.negotiation.accepted_mime = accepted_mime;
        true
    }

    pub fn set_active_drag_negotiation_actions(
        &mut self,
        target_client_id: ClientId,
        offer_id: WlDataOfferId,
        destination_actions: u32,
        preferred_action: u32,
        chosen_action: u32,
    ) -> bool {
        let Some(drag) = self.active_drag.as_mut() else {
            return false;
        };
        if drag
            .target
            .is_none_or(|target| target.client_id != target_client_id)
            || !drag.negotiation.offer_ids.contains(&offer_id)
        {
            return false;
        }
        if drag.negotiation.destination_actions == destination_actions
            && drag.negotiation.preferred_action == preferred_action
            && drag.negotiation.chosen_action == chosen_action
        {
            return false;
        }
        drag.negotiation.destination_actions = destination_actions;
        drag.negotiation.preferred_action = preferred_action;
        drag.negotiation.chosen_action = chosen_action;
        true
    }

    pub fn set_active_drag_chosen_action(
        &mut self,
        target_client_id: ClientId,
        chosen_action: u32,
    ) -> bool {
        let Some(drag) = self.active_drag.as_mut() else {
            return false;
        };
        if drag
            .target
            .is_none_or(|target| target.client_id != target_client_id)
        {
            return false;
        }
        if drag.negotiation.chosen_action == chosen_action {
            return false;
        }
        drag.negotiation.chosen_action = chosen_action;
        true
    }

    pub fn set_active_drag_finished(
        &mut self,
        target_client_id: ClientId,
        offer_id: WlDataOfferId,
        finished: bool,
    ) -> bool {
        let Some(drag) = self.active_drag.as_mut() else {
            return false;
        };
        if drag
            .target
            .is_none_or(|target| target.client_id != target_client_id)
            || !drag.negotiation.offer_ids.contains(&offer_id)
        {
            return false;
        }
        if drag.negotiation.finished == finished {
            return false;
        }
        drag.negotiation.finished = finished;
        true
    }

    pub fn set_active_drag_phase(&mut self, phase: DragPhase) -> bool {
        let Some(drag) = self.active_drag.as_mut() else {
            return false;
        };
        if drag.phase == phase {
            return false;
        }
        drag.phase = phase;
        true
    }

    pub fn active_drag_can_drop(&self) -> bool {
        self.active_drag.as_ref().is_some_and(|drag| {
            if drag.phase != DragPhase::Dragging || drag.target.is_none() {
                return false;
            }
            if drag.source_id.is_none() {
                return true;
            }
            drag.negotiation.accepted_mime.is_some() && drag.negotiation.chosen_action != 0
        })
    }

    pub(crate) fn remove_client(&mut self, client_id: ClientId) -> SeatClientRemovalResult {
        let mut result = SeatClientRemovalResult::default();
        self.client_serials.remove(&client_id);
        self.data_device_resources.remove(&client_id);
        self.pointer_resources.remove(&client_id);
        self.keyboard_resources.remove(&client_id);
        if self
            .selection_source
            .is_some_and(|selection| selection.owner_client_id == client_id)
        {
            self.selection_source = None;
        }
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
                PointerInteractiveGrab::ResizeTiled { client_id: grab_client_id, .. } => {
                    grab_client_id == client_id
                }
            })
        {
            self.clear_pointer_interactive_grab();
            result.interactive_grab_cleared = true;
        }
        if self
            .active_drag
            .as_ref()
            .is_some_and(|drag| drag.source_client_id == client_id)
        {
            self.active_drag = None;
        } else if let Some(drag) = self.active_drag.as_mut() {
            if drag
                .target
                .is_some_and(|target| target.client_id == client_id)
            {
                drag.target = None;
                drag.negotiation = DragNegotiationState::default();
                drag.phase = DragPhase::Cancelled;
            }
        }
        result.cursor_changed = self.cursor.clear_for_client(client_id);
        result
    }

    pub(crate) fn next_serial(&mut self) -> u32 {
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

    pub(crate) fn record_client_serial(&mut self, client_id: ClientId, serial: u32) {
        let serials = self.client_serials.entry(client_id).or_default();
        if let Some(back) = serials.back_mut() {
            if serial == back.hi {
                return;
            }
            if back.hi.wrapping_add(1) == serial {
                back.hi = serial;
                return;
            }
        }
        if serials.len() == MAX_TRACKED_SERIAL_RANGES {
            serials.pop_front();
        }
        serials.push_back(SerialRange { lo: serial, hi: serial });
    }

    pub(crate) fn record_pointer_enter(&mut self, client_id: ClientId, serial: u32) {
        self.last_pointer_enter = Some(PointerEnterState { client_id, serial });
    }

    pub(crate) fn clear_pointer_enter(&mut self) {
        self.last_pointer_enter = None;
    }

    pub fn pointer_cursor_request_allowed(&self, client_id: ClientId, serial: u32) -> bool {
        if !self
            .pointer_focus
            .is_some_and(|focus| focus.target.client_id == client_id)
        {
            return false;
        }
        self.last_pointer_enter
            .is_some_and(|enter| enter.client_id == client_id && enter.serial == serial)
            || (self.pointer_button_count > 0
                && self.last_pointer_press.is_some_and(|press| {
                    press.focus.client_id == client_id && press.serial == serial
                }))
    }

    pub fn set_named_cursor(&mut self, client_id: ClientId, icon_names: &[&'static str]) -> bool {
        self.cursor.set_named_cursor(client_id, icon_names)
    }

    pub fn set_compositor_named_cursor(&mut self, icon_names: &[&'static str]) -> bool {
        self.cursor.set_compositor_named_cursor(icon_names)
    }

    pub fn set_default_cursor(&mut self) -> bool {
        self.cursor.ensure_default_named_cursor()
    }

    pub fn set_cursor_surface(
        &mut self,
        client_id: ClientId,
        surface_id: WlSurfaceId,
        object_id: ObjectId,
        hotspot_x: i32,
        hotspot_y: i32,
    ) -> bool {
        self.cursor
            .set_surface_cursor(client_id, surface_id, object_id, hotspot_x, hotspot_y)
    }

    pub fn clear_cursor_for_client(&mut self, client_id: ClientId) -> bool {
        self.cursor.clear_for_client(client_id)
    }

    pub fn clear_cursor_surface(&mut self, object_id: ObjectId) -> bool {
        self.cursor.clear_surface(object_id)
    }

    pub fn apply_cursor_surface_offset(&mut self, object_id: ObjectId, dx: i32, dy: i32) -> bool {
        self.cursor.apply_surface_offset(object_id, dx, dy)
    }

    pub fn visible_cursor_source_at(&self, now_msec: u32) -> Option<CursorSourceRef<'_>> {
        self.cursor.source_at(now_msec)
    }

    pub fn visible_cursor_source(&self) -> Option<CursorSourceRef<'_>> {
        self.cursor.source()
    }

    pub fn cursor_is_visible(&self) -> bool {
        self.cursor.is_visible()
    }

    pub fn clear_cursor(&mut self) -> bool {
        self.cursor.clear()
    }

    pub fn set_cursor_theme(&mut self, theme_name: String, theme_size: u32) -> bool {
        self.cursor.set_theme(theme_name, theme_size)
    }

    pub fn pointer_position(&self) -> (f64, f64) {
        self.pointer_pos
    }

    pub fn keyboard_focus(&self) -> Option<SurfaceFocus> {
        self.keyboard_focus
    }

    #[cfg(test)]
    pub fn last_keyboard_trigger_serial(&self) -> Option<u32> {
        self.last_keyboard_trigger.map(|trigger| trigger.serial)
    }

    #[cfg(test)]
    fn test_install_default_cursor(&mut self) -> bool {
        self.cursor.test_install_default_cursor()
    }

    fn intercept_keyboard_key(&mut self, key: u32) {
        self.intercepted_keyboard_keys.insert(key);
    }

    fn take_intercepted_keyboard_key(&mut self, key: u32) -> bool {
        self.intercepted_keyboard_keys.remove(&key)
    }

    fn intercept_pointer_button(&mut self, button: u32) {
        self.intercepted_pointer_buttons.insert(button);
    }

    fn take_intercepted_pointer_button(&mut self, button: u32) -> bool {
        self.intercepted_pointer_buttons.remove(&button)
    }

    pub(crate) fn validate_implicit_grab_serial(
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

    pub(crate) fn validate_popup_grab_serial(
        &self,
        client_id: ClientId,
        surface_id: WlSurfaceId,
        serial: u32,
    ) -> bool {
        self.validate_implicit_grab_serial(client_id, surface_id, serial)
    }

    pub(crate) fn popup_grab_client(&self) -> Option<ClientId> {
        self.popup_grab.as_ref().map(|grab| grab.client_id)
    }

    pub(crate) fn popup_keyboard_surface(&self) -> Option<WlSurfaceId> {
        self.popup_grab
            .as_ref()
            .and_then(|grab| grab.popup_stack.last())
            .map(|(_, surface_id)| *surface_id)
    }

    pub(crate) fn popup_grab_contains_surface(&self, surface_id: WlSurfaceId) -> bool {
        self.popup_grab.as_ref().is_some_and(|grab| {
            grab.popup_stack
                .iter()
                .any(|(_, candidate)| *candidate == surface_id)
        })
    }

    pub(crate) fn start_popup_grab(
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

    pub(crate) fn remove_popup_grab(&mut self, popup_id: XdgPopupId) {
        let Some(grab) = self.popup_grab.as_mut() else {
            return;
        };
        grab.popup_stack
            .retain(|(candidate, _)| *candidate != popup_id);
        if grab.popup_stack.is_empty() {
            self.popup_grab = None;
        }
    }

    pub(crate) fn dismiss_popup_grab(&mut self) -> Option<(ClientId, Vec<XdgPopupId>)> {
        let grab = self.popup_grab.take()?;
        Some((
            grab.client_id,
            grab.popup_stack
                .into_iter()
                .map(|(popup_id, _)| popup_id)
                .collect(),
        ))
    }

    pub(crate) fn start_toplevel_move(
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

    pub(crate) fn start_toplevel_resize(
        &mut self,
        client_id: ClientId,
        root_surface_object_id: ObjectId,
        xdg_toplevel_id: XdgToplevelId,
        size_constraints: InteractiveSizeConstraints,
        edges: u32,
        initial_root_pos: (i32, i32),
        initial_size: (i32, i32),
    ) {
        self.pointer_interactive_grab = Some(PointerInteractiveGrab::Resize {
            client_id,
            root_surface_object_id,
            xdg_toplevel_id,
            size_constraints,
            edges,
            initial_pointer_pos: self.pointer_pos,
            initial_root_pos,
            initial_size,
            last_size: initial_size,
        });
    }

    pub(crate) fn start_toplevel_tiled_resize(
        &mut self,
        client_id: ClientId,
        root_surface_object_id: ObjectId,
        edges: u32,
        resize_state: TiledResizeState,
    ) {
        self.pointer_interactive_grab = Some(PointerInteractiveGrab::ResizeTiled {
            client_id,
            root_surface_object_id,
            edges,
            initial_pointer_pos: self.pointer_pos,
            resize_state,
        });
    }

    pub(crate) fn clear_pointer_interactive_grab(&mut self) {
        self.pointer_interactive_grab = None;
        self.intercepted_pointer_buttons.clear();
    }
}

pub fn handle_pointer_motion(
    state: &mut State,
    dispatcher: &mut Dispatcher,
    dx: f64,
    dy: f64,
    time: u32,
) {
    let old_pointer_pos = state.seat.pointer_position();
    let seat = &mut state.seat;
    seat.pointer_pos.0 += dx;
    seat.pointer_pos.1 += dy;
    clamp_pointer_to_outputs(state);
    let new_pointer_pos = state.seat.pointer_position();
    log::debug!(
        "seat pointer motion pos=({:.1}, {:.1}) time={}",
        state.seat.pointer_pos.0,
        state.seat.pointer_pos.1,
        time
    );
    if refresh_drag_focus(state, dispatcher, time, true) {
        refresh_visible_cursor_surface_outputs(state, dispatcher);
        refresh_visible_dnd_icon_surface_outputs(state, dispatcher);
        if state.seat.cursor_is_visible() || visible_dnd_icon_owner_client(state).is_some() {
            schedule_outputs_for_points(state, [old_pointer_pos, new_pointer_pos], dispatcher);
        }
        return;
    }
    if apply_pointer_interactive_grab(state, dispatcher) {
        return;
    }
    refresh_pointer_focus(state, dispatcher, time, true);
    refresh_visible_cursor_surface_outputs(state, dispatcher);
    refresh_visible_dnd_icon_surface_outputs(state, dispatcher);
    if state.seat.cursor_is_visible() || visible_dnd_icon_owner_client(state).is_some() {
        schedule_outputs_for_points(state, [old_pointer_pos, new_pointer_pos], dispatcher);
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
    let interactive_grab_active = state.seat.pointer_interactive_grab.is_some();
    if !interactive_grab_active {
        refresh_pointer_focus(state, dispatcher, time, false);
    }
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

    if button_state != 1 && state.seat.take_intercepted_pointer_button(button) {
        finish_pointer_interactive_grab(state, dispatcher, time);
        return;
    }

    if let Some(drag) = state.seat.active_drag() {
        if drag.phase == DragPhase::Dragging
            && button_state != 1
            && state.seat.pointer_button_count == 0
        {
            complete_drag_drop(state, dispatcher);
        }
        return;
    }

    if interactive_grab_active {
        if button_state != 1 && state.seat.pointer_button_count == 0 {
            finish_pointer_interactive_grab(state, dispatcher, time);
        }
        return;
    }

    if maybe_dismiss_popup_grab_on_button(state, dispatcher, button_state, time) {
        return;
    }

    if button_state == 1 && button == 0x110 && begin_ssd_pointer_action(state, dispatcher) {
        state.seat.intercept_pointer_button(button);
        return;
    }

    if button_state == 1 {
        set_keyboard_focus(state, dispatcher, pointer_focus.map(|focus| focus.target));
    }

    if button_state == 1 {
        if let Some(action) = state
            .config
            .binding_for_pointer_button(&state.keyboard, button)
        {
            if begin_pointer_binding(state, dispatcher, action) {
                state.seat.intercept_pointer_button(button);
                return;
            }
        }
    }

    let Some(pointer_focus) = state.seat.pointer_focus else {
        if button_state != 1 {
            finish_pointer_interactive_grab(state, dispatcher, time);
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
        finish_pointer_interactive_grab(state, dispatcher, time);
    }
}

pub fn handle_pointer_motion_absolute(
    state: &mut State,
    dispatcher: &mut Dispatcher,
    x: f64,
    y: f64,
    time: u32,
) {
    let old_pointer_pos = state.seat.pointer_position();
    state.seat.pointer_pos = (x, y);
    clamp_pointer_to_outputs(state);
    let new_pointer_pos = state.seat.pointer_position();
    // log::debug!(
    //     "seat pointer motion absolute pos=({:.1}, {:.1}) time={}",
    //     state.seat.pointer_pos.0,
    //     state.seat.pointer_pos.1,
    //     time
    // );
    if refresh_drag_focus(state, dispatcher, time, true) {
        refresh_visible_cursor_surface_outputs(state, dispatcher);
        refresh_visible_dnd_icon_surface_outputs(state, dispatcher);
        if state.seat.cursor_is_visible() || visible_dnd_icon_owner_client(state).is_some() {
            schedule_outputs_for_points(state, [old_pointer_pos, new_pointer_pos], dispatcher);
        }
        return;
    }
    if apply_pointer_interactive_grab(state, dispatcher) {
        return;
    }
    refresh_pointer_focus(state, dispatcher, time, true);
    refresh_visible_cursor_surface_outputs(state, dispatcher);
    refresh_visible_dnd_icon_surface_outputs(state, dispatcher);
    if state.seat.cursor_is_visible() || visible_dnd_icon_owner_client(state).is_some() {
        schedule_outputs_for_points(state, [old_pointer_pos, new_pointer_pos], dispatcher);
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

    if key_state == 0 && state.seat.take_intercepted_keyboard_key(key) {
        log::debug!("seat swallowed intercepted key release key={} time={}", key, time);
        return;
    }

    if key_state == 1 {
        if let Some(action) = state.config.binding_for_key(&state.keyboard, key) {
            log::info!("seat executing compositor binding key={} action={:?}", key, action);
            state.seat.intercept_keyboard_key(key);
            config::execute_action(state, dispatcher, action);
            return;
        }
    }

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

pub fn surface_focus_for_wayland_id(
    state: &State,
    surface_id: WlSurfaceId,
) -> Option<SurfaceFocus> {
    crate::input::surface_focus_for_wayland_id(state, surface_id)
}

pub fn resolve_activation_target(
    state: &State,
    surface_id: WlSurfaceId,
) -> Option<(ObjectId, SurfaceFocus)> {
    crate::input::resolve_activation_target(state, surface_id)
}

pub fn validate_activation_serial(
    state: &State,
    client_id: ClientId,
    seat_id: WlSeatId,
    surface_id: WlSurfaceId,
    serial: u32,
) -> bool {
    crate::input::validate_activation_serial(state, client_id, seat_id, surface_id, serial)
}

pub fn set_keyboard_focus_from_surface(
    state: &mut State,
    dispatcher: &mut Dispatcher,
    new_focus: Option<SurfaceFocus>,
) {
    crate::input::set_keyboard_focus_from_surface(state, dispatcher, new_focus);
}

pub fn refresh_keyboard_focus(state: &mut State, dispatcher: &mut Dispatcher) {
    crate::input::refresh_keyboard_focus(state, dispatcher);
}

pub fn refresh_keyboard_focus_with_snapshot(
    state: &mut State,
    dispatcher: &mut Dispatcher,
    snapshot: Option<LayerLayoutObjectSnapshot<'_>>,
) {
    crate::input::refresh_keyboard_focus_with_snapshot(state, dispatcher, snapshot);
}

pub fn refresh_scene_focus(state: &mut State, dispatcher: &mut Dispatcher) {
    refresh_pointer_focus(state, dispatcher, monotonic_time_msec(), false);
    refresh_keyboard_focus(state, dispatcher);
}

pub fn refresh_active_drag_focus(state: &mut State, dispatcher: &mut Dispatcher, time: u32) {
    crate::input::refresh_active_drag_focus(state, dispatcher, time);
}

fn monotonic_time_msec() -> u32 {
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) } != 0 {
        return 0;
    }
    let millis = (ts.tv_sec as u64)
        .saturating_mul(1000)
        .saturating_add((ts.tv_nsec as u64) / 1_000_000);
    millis.min(u64::from(u32::MAX)) as u32
}

pub fn finish_active_drag(state: &mut State, dispatcher: &mut Dispatcher) {
    let Some(drag) = state.seat.clear_active_drag() else {
        return;
    };
    if drag.icon_surface_object_id.is_some() {
        dispatch_client_action(
            dispatcher,
            drag.source_client_id,
            ClientAction::RefreshSurfaceOutputs,
        );
        schedule_outputs_for_points(state, [state.seat.pointer_position()], dispatcher);
    }
    if let Some(target) = drag.target {
        dispatcher.dispatch_to(target.client_id, ClientAction::DataDeviceDragLeave);
    }
}

pub fn cancel_active_drag(
    state: &mut State,
    dispatcher: &mut Dispatcher,
    source: Option<(ClientId, WlDataSourceId)>,
) {
    let Some(drag) = state.seat.clear_active_drag() else {
        return;
    };
    if drag.icon_surface_object_id.is_some() {
        dispatch_client_action(
            dispatcher,
            drag.source_client_id,
            ClientAction::RefreshSurfaceOutputs,
        );
        schedule_outputs_for_points(state, [state.seat.pointer_position()], dispatcher);
    }
    if let Some(target) = drag.target {
        dispatcher.dispatch_to(target.client_id, ClientAction::DataDeviceDragLeave);
    }
    let source = source.or_else(|| {
        drag.source_id
            .map(|source_id| (drag.source_client_id, source_id))
    });
    if let Some((source_client_id, source_id)) = source {
        dispatcher.dispatch_to(
            source_client_id,
            ClientAction::DataSourceEvent {
                source_id,
                event: crate::clients::DataSourceClientEvent::Cancelled,
            },
        );
    }
}

fn apply_pointer_interactive_grab(state: &mut State, dispatcher: &mut Dispatcher) -> bool {
    crate::input::apply_pointer_interactive_grab(state, dispatcher)
}

fn finish_pointer_interactive_grab(state: &mut State, dispatcher: &mut Dispatcher, time: u32) {
    crate::input::finish_pointer_interactive_grab(state, dispatcher, time);
}

pub(crate) fn abort_pointer_interactive_grab(
    state: &mut State,
    dispatcher: &mut Dispatcher,
    time: u32,
) -> bool {
    crate::input::abort_pointer_interactive_grab(state, dispatcher, time)
}

pub(crate) fn tiled_resize_active_root_surfaces(state: &State) -> BTreeSet<ObjectId> {
    crate::input::tiled_resize_active_root_surfaces(state)
}

pub(crate) fn recover_pointer_after_interactive_grab_loss(
    state: &mut State,
    dispatcher: &mut Dispatcher,
    time: u32,
) {
    crate::input::recover_pointer_after_interactive_grab_loss(state, dispatcher, time);
}

fn begin_pointer_binding(
    state: &mut State,
    dispatcher: &mut Dispatcher,
    action: config::MouseBindingAction,
) -> bool {
    crate::input::begin_pointer_binding(state, dispatcher, action)
}

fn begin_ssd_pointer_action(state: &mut State, dispatcher: &mut Dispatcher) -> bool {
    crate::input::begin_ssd_pointer_action(state, dispatcher)
}

fn scene_pick_at(state: &State, pointer_pos: (f64, f64)) -> ScenePick {
    crate::input::scene_pick_at(state, pointer_pos)
}

fn window_hit_at(state: &State, pointer_pos: (f64, f64)) -> Option<WindowHit> {
    crate::input::window_hit_at(state, pointer_pos)
}

fn ssd_hover_cursor_icon_names(state: &State) -> Option<&'static [&'static str]> {
    crate::input::ssd_hover_cursor_icon_names(state)
}

fn update_idle_pointer_cursor(state: &mut State, dispatcher: &mut Dispatcher) {
    if state.seat.pointer_focus.is_some() {
        return;
    }

    let previous_cursor_owner = visible_cursor_owner_client(state);
    let changed = if let Some(icon_names) = ssd_hover_cursor_icon_names(state) {
        state.seat.set_compositor_named_cursor(icon_names)
    } else {
        state.seat.set_default_cursor()
    };
    if !changed {
        return;
    }

    if let Some(client_id) = previous_cursor_owner {
        dispatch_client_action(dispatcher, client_id, ClientAction::RefreshSurfaceOutputs);
    }
    schedule_outputs_for_points(state, [state.seat.pointer_position()], dispatcher);
}

pub(crate) fn refresh_pointer_focus(
    state: &mut State,
    dispatcher: &mut Dispatcher,
    time: u32,
    send_motion: bool,
) {
    crate::input::refresh_pointer_focus(state, dispatcher, time, send_motion);
    update_idle_pointer_cursor(state, dispatcher);
}

fn clamp_pointer_to_outputs(state: &mut State) {
    let x = state.seat.pointer_pos.0.round() as i32;
    let y = state.seat.pointer_pos.1.round() as i32;
    let Some((clamped_x, clamped_y)) = state.clamp_point_to_outputs(x, y) else {
        return;
    };
    state.seat.pointer_pos.0 = clamped_x as f64;
    state.seat.pointer_pos.1 = clamped_y as f64;
}

fn output_bounds(state: &State) -> Option<crate::region::RectI> {
    state.output_layout_bounds()
}

fn complete_drag_drop(state: &mut State, dispatcher: &mut Dispatcher) {
    let Some(drag) = state.seat.active_drag() else {
        return;
    };
    if state.seat.active_drag_can_drop() {
        let target = drag.target.expect("validated active drag target");
        state.seat.set_active_drag_phase(DragPhase::DropSent);
        dispatcher.dispatch_to(target.client_id, ClientAction::DataDeviceDragDrop);
        if let Some(source_id) = drag.source_id {
            dispatcher.dispatch_to(
                drag.source_client_id,
                ClientAction::DataSourceEvent {
                    source_id,
                    event: crate::clients::DataSourceClientEvent::DndDropPerformed,
                },
            );
        } else {
            finish_active_drag(state, dispatcher);
        }
    } else {
        cancel_active_drag(state, dispatcher, None);
    }
}

pub(crate) fn dispatch_client_event(
    dispatcher: &mut Dispatcher,
    client_id: ClientId,
    event: SeatClientEvent,
) {
    dispatcher.dispatch_to(client_id, ClientAction::SeatEvent { event });
}

pub(crate) fn dispatch_client_action(
    dispatcher: &mut Dispatcher,
    client_id: ClientId,
    action: ClientAction,
) {
    dispatcher.dispatch_to(client_id, action);
}

fn visible_cursor_owner_client(state: &State) -> Option<ClientId> {
    match state.seat.visible_cursor_source()? {
        CursorSourceRef::Named { owner_client_id, .. } => owner_client_id,
        CursorSourceRef::Surface { owner_client_id, .. } => Some(owner_client_id),
    }
}

fn visible_dnd_icon_owner_client(state: &State) -> Option<ClientId> {
    state
        .seat
        .active_drag()
        .and_then(|drag| drag.icon_surface_object_id.map(|_| drag.source_client_id))
}

fn refresh_visible_cursor_surface_outputs(state: &State, dispatcher: &mut Dispatcher) {
    if let Some(CursorSourceRef::Surface { owner_client_id, .. }) =
        state.seat.visible_cursor_source()
    {
        dispatch_client_action(dispatcher, owner_client_id, ClientAction::RefreshSurfaceOutputs);
    }
}

fn refresh_visible_dnd_icon_surface_outputs(state: &State, dispatcher: &mut Dispatcher) {
    if let Some(owner_client_id) = visible_dnd_icon_owner_client(state) {
        dispatch_client_action(dispatcher, owner_client_id, ClientAction::RefreshSurfaceOutputs);
    }
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
    dispatcher: &mut Dispatcher,
    client_id: ClientId,
    surface_id: WlSurfaceId,
    serial: u32,
    root_surface_object_id: ObjectId,
    is_maximized: bool,
    is_fullscreen: bool,
) -> bool {
    crate::input::request_toplevel_move(
        state,
        dispatcher,
        client_id,
        surface_id,
        serial,
        root_surface_object_id,
        is_maximized,
        is_fullscreen,
    )
}

pub fn request_toplevel_resize(
    state: &mut State,
    dispatcher: &mut Dispatcher,
    client_id: ClientId,
    surface_id: WlSurfaceId,
    serial: u32,
    root_surface_object_id: ObjectId,
    xdg_toplevel_id: XdgToplevelId,
    size_constraints: InteractiveSizeConstraints,
    is_maximized: bool,
    is_fullscreen: bool,
    edges: u32,
) -> bool {
    crate::input::request_toplevel_resize(
        state,
        dispatcher,
        client_id,
        surface_id,
        serial,
        root_surface_object_id,
        xdg_toplevel_id,
        size_constraints,
        is_maximized,
        is_fullscreen,
        edges,
    )
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
        super::surface_focus_pick_at,
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
            layer_surface_objects: Default::default(),
            inputs: Default::default(),
            drm: Default::default(),
            outputs: Default::default(),
            active_output: None,
            workspace_groups: Default::default(),
            workspaces: Default::default(),
            output_workspace_groups: Default::default(),
            root_surface_workspaces: Default::default(),
            managed_root_surfaces: Default::default(),
            xdg_toplevel_roles: Default::default(),
            next_workspace_serial: 1,
            backend_syncobj_globals: Default::default(),
            globals,
            activation_tokens: Default::default(),
            egl: EGL::new(),
            config: Default::default(),
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

        let hit = surface_focus_pick_at(&state, (5.0, 5.0)).unwrap();
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

        let fallback_hit = surface_focus_pick_at(&state, (66.0, 77.0)).unwrap();
        assert_eq!(fallback_hit.target.surface_id, WlSurfaceId::from(2));

        let parent_hit = surface_focus_pick_at(&state, (145.0, 145.0)).unwrap();
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

        let hit = surface_focus_pick_at(&state, (5.0, 5.0)).unwrap();
        assert_eq!(hit.target.surface_id, WlSurfaceId::from(1));
    }
}

#[cfg(test)]
mod xdg_seat_tests {
    use {
        super::{
            begin_popup_grab, cancel_active_drag, complete_drag_drop, handle_keyboard_key,
            handle_pointer_button, handle_pointer_motion, handle_pointer_motion_absolute,
            recover_pointer_after_interactive_grab_loss, remove_popup_grab, scene_pick_at,
            window_hit_at, HitSemantics, SeatClientEvent, SeatState, SurfaceFocus,
        },
        crate::{
            clients::{Client, ClientAction, ClientId, DataSourceClientEvent, RawEvent},
            config::RuntimeConfig,
            cursor::CursorSourceRef,
            egl::EGL,
            globals::Globals,
            object::Object,
            protocol_generated::{
                xdg_toplevel, Constants, WlSeatId, WlSubsurfaceId, WlSurfaceId, XdgPopupId,
                XdgPositionerId, XdgSurfaceId, XdgToplevelId, XdgWmBaseId,
            },
            region::RectI,
            runner::Dispatcher,
            state::{DragPhase, DragState, DragTargetState, ObjectId, State, WorkspaceGroup},
            wayland_objects::{
                wl_seat::WlSeat,
                wl_surface::{Role, WlSurface},
                xdg_popup::XdgPopup,
                xdg_positioner::{PositionerState, XdgPositioner},
                xdg_surface::{
                    xdg_toplevel::{ToplevelDecorationMode, XdgToplevel},
                    XdgSurface,
                },
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
            layer_surface_objects: Default::default(),
            inputs: Default::default(),
            drm: Default::default(),
            outputs: Default::default(),
            active_output: None,
            workspace_groups: Default::default(),
            workspaces: Default::default(),
            output_workspace_groups: Default::default(),
            root_surface_workspaces: Default::default(),
            managed_root_surfaces: Default::default(),
            xdg_toplevel_roles: Default::default(),
            next_workspace_serial: 1,
            backend_syncobj_globals: Default::default(),
            globals,
            activation_tokens: Default::default(),
            egl: EGL::new(),
            config: Default::default(),
            seat: SeatState::default(),
            keyboard,
            xkb,
        }
    }

    fn make_client() -> (Client, Dispatcher) {
        (Client::new_for_tests(ClientId::from_raw(7)), Dispatcher::default())
    }

    fn assert_named_cursor_matches_any(seat: &SeatState, expected_names: &[&str]) {
        assert!(matches!(
            seat.visible_cursor_source(),
            Some(CursorSourceRef::Named { owner_client_id: None, name, .. })
                if expected_names.contains(&name)
        ));
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
            wl_surface.set_xdg_surface_impl(xdg_surface_id);
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

    fn toplevel_move_request(
        toplevel_id: XdgToplevelId,
        seat_id: WlSeatId,
        serial: u32,
    ) -> crate::clients::RawRequest<'static> {
        let mut event = RawEvent::new(toplevel_id.into(), 5, 2);
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

    fn set_toplevel_window_state(
        state: &mut State,
        toplevel_object_id: ObjectId,
        maximized: bool,
        fullscreen: bool,
    ) {
        let toplevel = state
            .objects
            .get_mut(toplevel_object_id)
            .downcast_mut::<XdgToplevel>()
            .unwrap();
        toplevel.test_set_maximized(maximized);
        toplevel.test_set_fullscreen(fullscreen);
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
    fn compositor_mouse_move_binding_starts_drag_and_swallows_button_events() {
        let mut state = make_state();
        let (mut client, mut dispatcher) = make_client();
        let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);

        let (wl_surface_object_id, xdg_surface_object_id) = insert_xdg_surface(
            &mut state,
            &mut client,
            &mut client_dispatcher,
            WlSurfaceId::from(170),
            XdgSurfaceId::from(180),
        );
        let _toplevel_object_id = assign_toplevel_role(
            &mut state,
            &mut client,
            &mut client_dispatcher,
            xdg_surface_object_id,
            XdgSurfaceId::from(180),
            XdgToplevelId::from(190),
        );

        {
            let surface = state
                .objects
                .get_mut(wl_surface_object_id)
                .downcast_mut::<WlSurface>()
                .unwrap();
            surface.test_map(ObjectId::from(900usize), 40, 50, 120, 80);
        }

        state.config = RuntimeConfig::parse_for_test(
            r#"
            [mouse_bindings]
            "Super+mouse:left" = { action = "move-window" }
            "#,
            &state.xkb,
        )
        .unwrap();
        state.keyboard.update_key(125, true);
        state.seat.pointer_pos = (70.0, 80.0);
        assert!(state.seat.set_cursor_surface(
            client.client_id(),
            WlSurfaceId::from(999),
            ObjectId::from(998usize),
            2,
            3,
        ));

        handle_pointer_button(&mut state, &mut dispatcher, 0x110, 1, 100);

        assert!(matches!(
            state.seat.pointer_interactive_grab,
            Some(super::PointerInteractiveGrab::Move { .. })
        ));
        assert_eq!(
            state.seat.keyboard_focus,
            Some(SurfaceFocus {
                client_id: client.client_id(),
                surface_id: WlSurfaceId::from(170)
            })
        );
        assert!(state.seat.pointer_focus.is_none());
        assert!(dispatcher.test_has_queued_action_for::<ClientAction, ClientId>(
            client.client_id(),
            |action| {
                matches!(
                    action,
                    ClientAction::SeatEvent {
                        event: SeatClientEvent::PointerLeave { surface, .. },
                    } if *surface == WlSurfaceId::from(170)
                )
            }
        ));
        assert!(!matches!(
            state.seat.visible_cursor_source(),
            Some(CursorSourceRef::Surface { owner_client_id, .. })
                if owner_client_id == client.client_id()
        ));
        assert!(!dispatcher.test_has_queued_action_for::<ClientAction, ClientId>(
            client.client_id(),
            |action| {
                matches!(
                    action,
                    ClientAction::SeatEvent {
                        event: SeatClientEvent::PointerButton { button, .. },
                    } if *button == 0x110
                )
            }
        ));

        handle_pointer_button(&mut state, &mut dispatcher, 0x111, 1, 105);
        handle_pointer_button(&mut state, &mut dispatcher, 0x111, 0, 106);

        assert!(!dispatcher.test_has_queued_action_for::<ClientAction, ClientId>(
            client.client_id(),
            |action| {
                matches!(
                    action,
                    ClientAction::SeatEvent {
                        event: SeatClientEvent::PointerButton { button, .. },
                    } if *button == 0x111
                )
            }
        ));

        handle_pointer_button(&mut state, &mut dispatcher, 0x110, 0, 110);

        assert!(state.seat.pointer_interactive_grab.is_none());
        assert!(state.seat.pointer_focus.is_some());
        assert!(!dispatcher.test_has_queued_action_for::<ClientAction, ClientId>(
            client.client_id(),
            |action| {
                matches!(
                    action,
                    ClientAction::SeatEvent {
                        event: SeatClientEvent::PointerButton { button, state, .. },
                    } if *button == 0x110 && *state == 0
                )
            }
        ));
    }

    #[test]
    fn ssd_titlebar_press_starts_move_without_client_pointer_focus() {
        let mut state = make_state();
        state.config.general.decoration_insets.top = 12;
        state.config.general.decoration_insets.left = 4;
        state.config.general.decoration_insets.right = 4;
        state.config.general.decoration_insets.bottom = 4;
        let (mut client, mut dispatcher) = make_client();
        let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);

        let (wl_surface_object_id, xdg_surface_object_id) = insert_xdg_surface(
            &mut state,
            &mut client,
            &mut client_dispatcher,
            WlSurfaceId::from(171),
            XdgSurfaceId::from(181),
        );
        let _toplevel_object_id = assign_toplevel_role(
            &mut state,
            &mut client,
            &mut client_dispatcher,
            xdg_surface_object_id,
            XdgSurfaceId::from(181),
            XdgToplevelId::from(191),
        );
        {
            let toplevel = state
                .objects
                .get_mut(
                    state
                        .xdg_toplevel_object_id(client.client_id(), XdgSurfaceId::from(181))
                        .unwrap(),
                )
                .downcast_mut::<XdgToplevel>()
                .unwrap();
            toplevel.test_set_decoration_mode(ToplevelDecorationMode::ServerSide);
        }
        {
            let surface = state
                .objects
                .get_mut(wl_surface_object_id)
                .downcast_mut::<WlSurface>()
                .unwrap();
            surface.test_map(ObjectId::from(950usize), 40, 50, 120, 80);
        }

        state.seat.pointer_pos = (80.0, 44.0);
        handle_pointer_button(&mut state, &mut dispatcher, 0x110, 1, 100);

        assert!(matches!(
            state.seat.pointer_interactive_grab,
            Some(super::PointerInteractiveGrab::Move { .. })
        ));
        assert_eq!(
            state.seat.keyboard_focus.map(|focus| focus.surface_id),
            Some(WlSurfaceId::from(171))
        );
        assert!(state.seat.pointer_focus.is_none());
        assert!(state.seat.intercepted_pointer_buttons.contains(&0x110));
    }

    #[test]
    fn compositor_mouse_resize_binding_uses_pointer_thirds_for_edges() {
        let mut state = make_state();
        let (mut client, mut dispatcher) = make_client();
        let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);

        let (wl_surface_object_id, xdg_surface_object_id) = insert_xdg_surface(
            &mut state,
            &mut client,
            &mut client_dispatcher,
            WlSurfaceId::from(200),
            XdgSurfaceId::from(210),
        );
        let _toplevel_object_id = assign_toplevel_role(
            &mut state,
            &mut client,
            &mut client_dispatcher,
            xdg_surface_object_id,
            XdgSurfaceId::from(210),
            XdgToplevelId::from(220),
        );

        {
            let surface = state
                .objects
                .get_mut(wl_surface_object_id)
                .downcast_mut::<WlSurface>()
                .unwrap();
            surface.test_map(ObjectId::from(901usize), 10, 20, 80, 60);
        }

        state.config = RuntimeConfig::parse_for_test(
            r#"
            [mouse_bindings]
            "Super+mouse:right" = { action = "resize-window" }
            "#,
            &state.xkb,
        )
        .unwrap();
        state.keyboard.update_key(125, true);
        state.seat.pointer_pos = (85.0, 50.0);

        handle_pointer_button(&mut state, &mut dispatcher, 0x111, 1, 100);

        assert!(matches!(
            state.seat.pointer_interactive_grab,
            Some(super::PointerInteractiveGrab::Resize { edges, .. })
                if edges == xdg_toplevel::ResizeEdge::Right as u32
        ));
    }

    #[test]
    fn ssd_left_border_press_starts_resize_with_left_edge() {
        let mut state = make_state();
        state.config.general.decoration_insets.top = 12;
        state.config.general.decoration_insets.left = 6;
        state.config.general.decoration_insets.right = 4;
        state.config.general.decoration_insets.bottom = 4;
        let (mut client, mut dispatcher) = make_client();
        let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);

        let (wl_surface_object_id, xdg_surface_object_id) = insert_xdg_surface(
            &mut state,
            &mut client,
            &mut client_dispatcher,
            WlSurfaceId::from(201),
            XdgSurfaceId::from(211),
        );
        let _toplevel_object_id = assign_toplevel_role(
            &mut state,
            &mut client,
            &mut client_dispatcher,
            xdg_surface_object_id,
            XdgSurfaceId::from(211),
            XdgToplevelId::from(221),
        );
        {
            let toplevel = state
                .objects
                .get_mut(
                    state
                        .xdg_toplevel_object_id(client.client_id(), XdgSurfaceId::from(211))
                        .unwrap(),
                )
                .downcast_mut::<XdgToplevel>()
                .unwrap();
            toplevel.test_set_decoration_mode(ToplevelDecorationMode::ServerSide);
        }
        {
            let surface = state
                .objects
                .get_mut(wl_surface_object_id)
                .downcast_mut::<WlSurface>()
                .unwrap();
            surface.test_map(ObjectId::from(951usize), 40, 50, 120, 80);
        }

        state.seat.pointer_pos = (35.0, 90.0);
        handle_pointer_button(&mut state, &mut dispatcher, 0x110, 1, 100);

        assert!(matches!(
            state.seat.pointer_interactive_grab,
            Some(super::PointerInteractiveGrab::Resize { edges, .. })
                if edges == xdg_toplevel::ResizeEdge::Left as u32
        ));
        assert!(state.seat.intercepted_pointer_buttons.contains(&0x110));
    }

    #[test]
    fn csd_window_geometry_extends_window_pointer_target_bounds() {
        let mut state = make_state();
        let (mut client, mut dispatcher) = make_client();
        let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);

        let (wl_surface_object_id, xdg_surface_object_id) = insert_xdg_surface(
            &mut state,
            &mut client,
            &mut client_dispatcher,
            WlSurfaceId::from(230),
            XdgSurfaceId::from(240),
        );
        let _toplevel_object_id = assign_toplevel_role(
            &mut state,
            &mut client,
            &mut client_dispatcher,
            xdg_surface_object_id,
            XdgSurfaceId::from(240),
            XdgToplevelId::from(250),
        );
        {
            let surface = state
                .objects
                .get_mut(wl_surface_object_id)
                .downcast_mut::<WlSurface>()
                .unwrap();
            surface.test_map(ObjectId::from(952usize), 40, 50, 120, 80);
        }
        let ((_, child_object_id), _) = client.insert_object(
            &mut state,
            WlSurface::new(WlSurfaceId::from(231), client.client_id(), 6),
        );
        {
            let root_surface = state
                .objects
                .get_mut(wl_surface_object_id)
                .downcast_mut::<WlSurface>()
                .unwrap();
            root_surface.add_pending_child(child_object_id);
            assert!(root_surface.restack_child_relative(child_object_id, None, true));
        }
        {
            let child_surface = state
                .objects
                .get_mut(child_object_id)
                .downcast_mut::<WlSurface>()
                .unwrap();
            child_surface.assign_subsurface(WlSubsurfaceId::from(232), wl_surface_object_id);
            child_surface.set_subsurface_position(-6, -12);
            child_surface.test_map(ObjectId::from(954usize), 0, 0, 126, 92);
        }
        {
            let mut root_surface = state.objects.grab_object::<WlSurface>(wl_surface_object_id);
            assert!(root_surface.apply_pending_tree_state(&mut state));
        }
        {
            let xdg_surface = state
                .objects
                .get_mut(xdg_surface_object_id)
                .downcast_mut::<XdgSurface>()
                .unwrap();
            xdg_surface.test_set_current_window_geometry(Some(RectI {
                x: -6,
                y: -12,
                width: 126,
                height: 92,
            }));
        }

        let target = scene_pick_at(&state, (36.0, 44.0))
            .window_target
            .expect("target on CSD chrome");
        assert_eq!(target.root_surface_object_id, wl_surface_object_id);
        assert_eq!(target.interactive_rect, RectI::new(34, 38, 126, 92).unwrap());
    }

    #[test]
    fn window_pointer_hit_classifies_content_hits() {
        let mut state = make_state();
        let (mut client, mut dispatcher) = make_client();
        let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);

        let (wl_surface_object_id, xdg_surface_object_id) = insert_xdg_surface(
            &mut state,
            &mut client,
            &mut client_dispatcher,
            WlSurfaceId::from(245),
            XdgSurfaceId::from(255),
        );
        let _toplevel_object_id = assign_toplevel_role(
            &mut state,
            &mut client,
            &mut client_dispatcher,
            xdg_surface_object_id,
            XdgSurfaceId::from(255),
            XdgToplevelId::from(265),
        );
        {
            let surface = state
                .objects
                .get_mut(wl_surface_object_id)
                .downcast_mut::<WlSurface>()
                .unwrap();
            surface.test_map(ObjectId::from(955usize), 40, 50, 120, 80);
        }

        let hit = window_hit_at(&state, (80.0, 90.0)).expect("content hit");
        assert_eq!(hit.target.root_surface_object_id, wl_surface_object_id);
        assert_eq!(hit.semantics, HitSemantics::SurfaceContent);
    }

    #[test]
    fn window_hit_caches_binding_resize_edges_for_content_hits() {
        let mut state = make_state();
        let (mut client, mut dispatcher) = make_client();
        let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);

        let (wl_surface_object_id, xdg_surface_object_id) = insert_xdg_surface(
            &mut state,
            &mut client,
            &mut client_dispatcher,
            WlSurfaceId::from(246),
            XdgSurfaceId::from(256),
        );
        let _toplevel_object_id = assign_toplevel_role(
            &mut state,
            &mut client,
            &mut client_dispatcher,
            xdg_surface_object_id,
            XdgSurfaceId::from(256),
            XdgToplevelId::from(266),
        );
        {
            let surface = state
                .objects
                .get_mut(wl_surface_object_id)
                .downcast_mut::<WlSurface>()
                .unwrap();
            surface.test_map(ObjectId::from(957usize), 10, 20, 90, 60);
        }

        let hit = window_hit_at(&state, (85.0, 50.0)).expect("content hit");
        assert_eq!(hit.semantics, HitSemantics::SurfaceContent);
        assert_eq!(hit.binding_resize_edges, xdg_toplevel::ResizeEdge::Right as u32);
    }

    #[test]
    fn window_pointer_hit_classifies_ssd_titlebar_and_resize_hits() {
        let mut state = make_state();
        state.config.general.decoration_insets.top = 12;
        state.config.general.decoration_insets.left = 6;
        state.config.general.decoration_insets.right = 4;
        state.config.general.decoration_insets.bottom = 4;
        let (mut client, mut dispatcher) = make_client();
        let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);

        let (wl_surface_object_id, xdg_surface_object_id) = insert_xdg_surface(
            &mut state,
            &mut client,
            &mut client_dispatcher,
            WlSurfaceId::from(266),
            XdgSurfaceId::from(276),
        );
        let _toplevel_object_id = assign_toplevel_role(
            &mut state,
            &mut client,
            &mut client_dispatcher,
            xdg_surface_object_id,
            XdgSurfaceId::from(276),
            XdgToplevelId::from(286),
        );
        {
            let toplevel = state
                .objects
                .get_mut(
                    state
                        .xdg_toplevel_object_id(client.client_id(), XdgSurfaceId::from(276))
                        .unwrap(),
                )
                .downcast_mut::<XdgToplevel>()
                .unwrap();
            toplevel.test_set_decoration_mode(ToplevelDecorationMode::ServerSide);
        }
        {
            let surface = state
                .objects
                .get_mut(wl_surface_object_id)
                .downcast_mut::<WlSurface>()
                .unwrap();
            surface.test_map(ObjectId::from(956usize), 40, 50, 120, 80);
        }

        let titlebar_hit = window_hit_at(&state, (80.0, 44.0)).expect("titlebar hit");
        assert_eq!(titlebar_hit.semantics, HitSemantics::CompositorMove);

        let resize_hit = window_hit_at(&state, (35.0, 90.0)).expect("resize hit");
        assert_eq!(
            resize_hit.semantics,
            HitSemantics::CompositorResize(xdg_toplevel::ResizeEdge::Left as u32)
        );
    }

    #[test]
    fn ssd_hover_uses_resize_and_move_cursors_before_click() {
        let mut state = make_state();
        state.config.general.decoration_insets.top = 12;
        state.config.general.decoration_insets.left = 6;
        state.config.general.decoration_insets.right = 4;
        state.config.general.decoration_insets.bottom = 4;
        let (mut client, mut dispatcher) = make_client();
        let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);

        let (wl_surface_object_id, xdg_surface_object_id) = insert_xdg_surface(
            &mut state,
            &mut client,
            &mut client_dispatcher,
            WlSurfaceId::from(260),
            XdgSurfaceId::from(270),
        );
        let _toplevel_object_id = assign_toplevel_role(
            &mut state,
            &mut client,
            &mut client_dispatcher,
            xdg_surface_object_id,
            XdgSurfaceId::from(270),
            XdgToplevelId::from(280),
        );
        {
            let toplevel = state
                .objects
                .get_mut(
                    state
                        .xdg_toplevel_object_id(client.client_id(), XdgSurfaceId::from(270))
                        .unwrap(),
                )
                .downcast_mut::<XdgToplevel>()
                .unwrap();
            toplevel.test_set_decoration_mode(ToplevelDecorationMode::ServerSide);
        }
        {
            let surface = state
                .objects
                .get_mut(wl_surface_object_id)
                .downcast_mut::<WlSurface>()
                .unwrap();
            surface.test_map(ObjectId::from(953usize), 40, 50, 120, 80);
        }

        handle_pointer_motion_absolute(&mut state, &mut dispatcher, 35.0, 90.0, 100);
        assert_named_cursor_matches_any(&state.seat, super::POINTER_RESIZE_W_CURSOR_ICON_NAMES);

        handle_pointer_motion_absolute(&mut state, &mut dispatcher, 80.0, 40.0, 110);
        assert_named_cursor_matches_any(&state.seat, super::POINTER_RESIZE_N_CURSOR_ICON_NAMES);

        handle_pointer_motion_absolute(&mut state, &mut dispatcher, 162.0, 90.0, 120);
        assert_named_cursor_matches_any(&state.seat, super::POINTER_RESIZE_E_CURSOR_ICON_NAMES);

        handle_pointer_motion_absolute(&mut state, &mut dispatcher, 80.0, 132.0, 130);
        assert_named_cursor_matches_any(&state.seat, super::POINTER_RESIZE_S_CURSOR_ICON_NAMES);

        handle_pointer_motion_absolute(&mut state, &mut dispatcher, 80.0, 44.0, 140);
        assert_named_cursor_matches_any(&state.seat, super::POINTER_MOVE_CURSOR_ICON_NAMES);
        assert!(state.seat.pointer_focus.is_none());
    }

    #[test]
    fn compositor_mouse_move_binding_uses_window_bounds_without_pointer_focus() {
        let mut state = make_state();
        let (mut client, mut dispatcher) = make_client();
        let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);

        let (wl_surface_object_id, xdg_surface_object_id) = insert_xdg_surface(
            &mut state,
            &mut client,
            &mut client_dispatcher,
            WlSurfaceId::from(230),
            XdgSurfaceId::from(240),
        );
        let _toplevel_object_id = assign_toplevel_role(
            &mut state,
            &mut client,
            &mut client_dispatcher,
            xdg_surface_object_id,
            XdgSurfaceId::from(240),
            XdgToplevelId::from(250),
        );

        {
            let surface = state
                .objects
                .get_mut(wl_surface_object_id)
                .downcast_mut::<WlSurface>()
                .unwrap();
            surface.test_map(ObjectId::from(902usize), 15, 25, 100, 80);
            surface.test_set_input_region(crate::region::RegionValue::empty());
        }

        state.config = RuntimeConfig::parse_for_test(
            r#"
            [mouse_bindings]
            "Super+mouse:left" = { action = "move-window" }
            "#,
            &state.xkb,
        )
        .unwrap();
        state.keyboard.update_key(125, true);
        state.seat.pointer_pos = (60.0, 55.0);

        handle_pointer_button(&mut state, &mut dispatcher, 0x110, 1, 100);

        assert!(state.seat.pointer_focus.is_none());
        assert!(matches!(
            state.seat.pointer_interactive_grab,
            Some(super::PointerInteractiveGrab::Move { .. })
        ));
    }

    #[test]
    fn compositor_mouse_move_binding_rejects_maximized_windows() {
        let mut state = make_state();
        let (mut client, mut dispatcher) = make_client();
        let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);

        let (wl_surface_object_id, xdg_surface_object_id) = insert_xdg_surface(
            &mut state,
            &mut client,
            &mut client_dispatcher,
            WlSurfaceId::from(260),
            XdgSurfaceId::from(270),
        );
        let toplevel_object_id = assign_toplevel_role(
            &mut state,
            &mut client,
            &mut client_dispatcher,
            xdg_surface_object_id,
            XdgSurfaceId::from(270),
            XdgToplevelId::from(280),
        );
        {
            let surface = state
                .objects
                .get_mut(wl_surface_object_id)
                .downcast_mut::<WlSurface>()
                .unwrap();
            surface.test_map(ObjectId::from(903usize), 20, 30, 120, 90);
        }
        set_toplevel_window_state(&mut state, toplevel_object_id, true, false);

        state.config = RuntimeConfig::parse_for_test(
            r#"
            [mouse_bindings]
            "Super+mouse:left" = { action = "move-window" }
            "#,
            &state.xkb,
        )
        .unwrap();
        state.keyboard.update_key(125, true);
        state.seat.pointer_pos = (80.0, 70.0);

        handle_pointer_button(&mut state, &mut dispatcher, 0x110, 1, 100);

        assert!(state.seat.pointer_interactive_grab.is_none());
        assert!(dispatcher.test_has_queued_action_for::<ClientAction, ClientId>(
            client.client_id(),
            |action| {
                matches!(
                    action,
                    ClientAction::SeatEvent {
                        event: SeatClientEvent::PointerButton { button, state, .. },
                    } if *button == 0x110 && *state == 1
                )
            }
        ));
    }

    #[test]
    fn compositor_mouse_resize_binding_rejects_fullscreen_windows() {
        let mut state = make_state();
        let (mut client, mut dispatcher) = make_client();
        let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);

        let (wl_surface_object_id, xdg_surface_object_id) = insert_xdg_surface(
            &mut state,
            &mut client,
            &mut client_dispatcher,
            WlSurfaceId::from(290),
            XdgSurfaceId::from(300),
        );
        let toplevel_object_id = assign_toplevel_role(
            &mut state,
            &mut client,
            &mut client_dispatcher,
            xdg_surface_object_id,
            XdgSurfaceId::from(300),
            XdgToplevelId::from(310),
        );
        {
            let surface = state
                .objects
                .get_mut(wl_surface_object_id)
                .downcast_mut::<WlSurface>()
                .unwrap();
            surface.test_map(ObjectId::from(904usize), 0, 0, 200, 150);
        }
        let workspace_id = state.create_workspace(None, None);
        let group_id = state.workspace_groups.insert(WorkspaceGroup {
            outputs: Default::default(),
            workspaces: vec![workspace_id],
            active_workspace: Some(workspace_id),
        });
        state.workspaces.try_get_mut(workspace_id).unwrap().group = Some(group_id);
        assert!(state.assign_root_surface_to_workspace(wl_surface_object_id, workspace_id));
        set_toplevel_window_state(&mut state, toplevel_object_id, false, true);
        assert!(state.set_root_surface_fullscreen(wl_surface_object_id, true));

        state.config = RuntimeConfig::parse_for_test(
            r#"
            [mouse_bindings]
            "Super+mouse:right" = { action = "resize-window" }
            "#,
            &state.xkb,
        )
        .unwrap();
        state.keyboard.update_key(125, true);
        state.seat.pointer_pos = (150.0, 100.0);

        handle_pointer_button(&mut state, &mut dispatcher, 0x111, 1, 100);

        assert!(state.seat.pointer_interactive_grab.is_none());
        assert!(dispatcher.test_has_queued_action_for::<ClientAction, ClientId>(
            client.client_id(),
            |action| {
                matches!(
                    action,
                    ClientAction::SeatEvent {
                        event: SeatClientEvent::PointerButton { button, state, .. },
                    } if *button == 0x111 && *state == 1
                )
            }
        ));
    }

    #[test]
    fn compositor_mouse_resize_binding_uses_compositor_fullscreen_state() {
        let mut state = make_state();
        let (mut client, mut dispatcher) = make_client();
        let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);

        let (first_surface_object_id, first_xdg_surface_object_id) = insert_xdg_surface(
            &mut state,
            &mut client,
            &mut client_dispatcher,
            WlSurfaceId::from(320),
            XdgSurfaceId::from(330),
        );
        let first_toplevel_object_id = assign_toplevel_role(
            &mut state,
            &mut client,
            &mut client_dispatcher,
            first_xdg_surface_object_id,
            XdgSurfaceId::from(330),
            XdgToplevelId::from(340),
        );
        let (second_surface_object_id, second_xdg_surface_object_id) = insert_xdg_surface(
            &mut state,
            &mut client,
            &mut client_dispatcher,
            WlSurfaceId::from(350),
            XdgSurfaceId::from(360),
        );
        let second_toplevel_object_id = assign_toplevel_role(
            &mut state,
            &mut client,
            &mut client_dispatcher,
            second_xdg_surface_object_id,
            XdgSurfaceId::from(360),
            XdgToplevelId::from(370),
        );
        {
            let first_surface = state
                .objects
                .get_mut(first_surface_object_id)
                .downcast_mut::<WlSurface>()
                .unwrap();
            first_surface.test_map(ObjectId::from(905usize), 0, 0, 200, 150);
        }
        {
            let second_surface = state
                .objects
                .get_mut(second_surface_object_id)
                .downcast_mut::<WlSurface>()
                .unwrap();
            second_surface.test_map(ObjectId::from(906usize), 240, 0, 200, 150);
        }
        let workspace_id = state.create_workspace(None, None);
        let group_id = state.workspace_groups.insert(WorkspaceGroup {
            outputs: Default::default(),
            workspaces: vec![workspace_id],
            active_workspace: Some(workspace_id),
        });
        state.workspaces.try_get_mut(workspace_id).unwrap().group = Some(group_id);
        assert!(state.assign_root_surface_to_workspace(first_surface_object_id, workspace_id));
        assert!(state.assign_root_surface_to_workspace(second_surface_object_id, workspace_id));
        assert!(state.set_root_surface_floating(first_surface_object_id, true));
        set_toplevel_window_state(&mut state, first_toplevel_object_id, false, true);
        set_toplevel_window_state(&mut state, second_toplevel_object_id, false, false);
        assert!(state.set_root_surface_fullscreen(first_surface_object_id, true));
        assert!(state.set_root_surface_fullscreen(second_surface_object_id, true));
        assert!(state.set_root_surface_fullscreen(second_surface_object_id, false));

        state.config = RuntimeConfig::parse_for_test(
            r#"
            [mouse_bindings]
            "Super+mouse:right" = { action = "resize-window" }
            "#,
            &state.xkb,
        )
        .unwrap();
        state.keyboard.update_key(125, true);
        state.seat.pointer_pos = (150.0, 100.0);

        handle_pointer_button(&mut state, &mut dispatcher, 0x111, 1, 100);

        assert!(matches!(
            state.seat.pointer_interactive_grab,
            Some(super::PointerInteractiveGrab::Resize { .. })
        ));
    }

    #[test]
    fn compositor_mouse_grab_surface_loss_runs_full_teardown() {
        let mut state = make_state();
        let (mut client, mut dispatcher) = make_client();
        let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);

        let (wl_surface_object_id, xdg_surface_object_id) = insert_xdg_surface(
            &mut state,
            &mut client,
            &mut client_dispatcher,
            WlSurfaceId::from(320),
            XdgSurfaceId::from(330),
        );
        let _toplevel_object_id = assign_toplevel_role(
            &mut state,
            &mut client,
            &mut client_dispatcher,
            xdg_surface_object_id,
            XdgSurfaceId::from(330),
            XdgToplevelId::from(340),
        );
        {
            let surface = state
                .objects
                .get_mut(wl_surface_object_id)
                .downcast_mut::<WlSurface>()
                .unwrap();
            surface.test_map(ObjectId::from(905usize), 30, 40, 120, 90);
        }

        state.config = RuntimeConfig::parse_for_test(
            r#"
            [mouse_bindings]
            "Super+mouse:left" = { action = "move-window" }
            "#,
            &state.xkb,
        )
        .unwrap();
        state.keyboard.update_key(125, true);
        state.seat.pointer_pos = (60.0, 70.0);

        handle_pointer_button(&mut state, &mut dispatcher, 0x110, 1, 100);
        assert!(state.seat.pointer_interactive_grab.is_some());
        assert!(state.remove_object(wl_surface_object_id).is_some());

        handle_pointer_motion(&mut state, &mut dispatcher, 5.0, 0.0, 110);

        assert!(state.seat.pointer_interactive_grab.is_none());
        assert!(state.seat.intercepted_pointer_buttons.is_empty());
        assert!(matches!(
            state.seat.visible_cursor_source(),
            Some(CursorSourceRef::Named { owner_client_id: None, .. })
        ));
        assert!(state.seat.pointer_focus.is_none());
    }

    #[test]
    fn removing_grab_client_clears_intercepted_buttons_and_recovers_cursor() {
        let mut state = make_state();
        let (mut client, mut dispatcher) = make_client();
        let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);

        let (wl_surface_object_id, xdg_surface_object_id) = insert_xdg_surface(
            &mut state,
            &mut client,
            &mut client_dispatcher,
            WlSurfaceId::from(350),
            XdgSurfaceId::from(360),
        );
        let _toplevel_object_id = assign_toplevel_role(
            &mut state,
            &mut client,
            &mut client_dispatcher,
            xdg_surface_object_id,
            XdgSurfaceId::from(360),
            XdgToplevelId::from(370),
        );
        {
            let surface = state
                .objects
                .get_mut(wl_surface_object_id)
                .downcast_mut::<WlSurface>()
                .unwrap();
            surface.test_map(ObjectId::from(906usize), 10, 10, 100, 80);
        }

        state.config = RuntimeConfig::parse_for_test(
            r#"
            [mouse_bindings]
            "Super+mouse:left" = { action = "move-window" }
            "#,
            &state.xkb,
        )
        .unwrap();
        state.keyboard.update_key(125, true);
        state.seat.pointer_pos = (50.0, 50.0);

        handle_pointer_button(&mut state, &mut dispatcher, 0x110, 1, 100);
        assert!(state.seat.pointer_interactive_grab.is_some());
        assert!(!state.seat.intercepted_pointer_buttons.is_empty());

        let removal = state.seat.remove_client(client.client_id());
        assert!(removal.interactive_grab_cleared);
        assert!(state.seat.pointer_interactive_grab.is_none());
        assert!(state.seat.intercepted_pointer_buttons.is_empty());

        recover_pointer_after_interactive_grab_loss(&mut state, &mut dispatcher, 0);

        assert!(matches!(
            state.seat.visible_cursor_source(),
            Some(CursorSourceRef::Named { owner_client_id: None, .. })
        ));
    }

    #[test]
    fn tiled_resize_grab_is_cleared_on_client_removal() {
        let mut state = make_state();
        let (client, _dispatcher) = make_client();

        state.seat.pointer_interactive_grab = Some(super::PointerInteractiveGrab::ResizeTiled {
            client_id: client.client_id(),
            root_surface_object_id: ObjectId::from(999usize),
            edges: xdg_toplevel::ResizeEdge::Right as u32,
            initial_pointer_pos: (10.0, 20.0),
            resize_state: crate::tiling::test_tiled_resize_state(),
        });
        state.seat.intercepted_pointer_buttons.insert(0x111);

        let removal = state.seat.remove_client(client.client_id());

        assert!(removal.interactive_grab_cleared);
        assert!(state.seat.pointer_interactive_grab.is_none());
        assert!(state.seat.intercepted_pointer_buttons.is_empty());
    }

    #[test]
    fn compositor_binding_swallows_key_press_and_release() {
        let mut state = make_state();
        let (_client, mut dispatcher) = make_client();

        state.config = RuntimeConfig::parse_for_test(
            r#"
            [bindings]
            "Super+Return" = { action = "spawn", command = ":" }
            "#,
            &state.xkb,
        )
        .unwrap();
        state.seat.keyboard_focus = Some(SurfaceFocus {
            client_id: ClientId::from_raw(7),
            surface_id: WlSurfaceId::from(10),
        });

        handle_keyboard_key(&mut state, &mut dispatcher, 125, 1, 10);
        handle_keyboard_key(&mut state, &mut dispatcher, 28, 1, 20);
        handle_keyboard_key(&mut state, &mut dispatcher, 28, 0, 30);

        assert!(!dispatcher.test_has_queued_action_for::<ClientAction, ClientId>(
            ClientId::from_raw(7),
            |action| {
                matches!(
                    action,
                    ClientAction::SeatEvent {
                        event: SeatClientEvent::KeyboardKey { key, .. },
                    } if *key == 28
                )
            }
        ));
    }

    #[test]
    fn removing_last_popup_grab_restores_parent_focus() {
        let mut state = make_state();
        let (mut client, mut dispatcher) = make_client();

        let parent_focus = SurfaceFocus {
            client_id: ClientId::from_raw(7),
            surface_id: WlSurfaceId::from(50),
        };
        let popup_focus = SurfaceFocus {
            client_id: ClientId::from_raw(7),
            surface_id: WlSurfaceId::from(51),
        };
        insert_surface(&mut state, &mut client, parent_focus.surface_id);
        insert_surface(&mut state, &mut client, popup_focus.surface_id);

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
    fn toplevel_move_accepts_pointer_trigger_serial() {
        let mut state = make_state();
        let (mut client, mut dispatcher) = make_client();
        let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);

        client.insert_object(&mut state, WlSeat::new(WlSeatId::from(6)));

        let (wl_surface_object_id, xdg_surface_object_id) = insert_xdg_surface(
            &mut state,
            &mut client,
            &mut client_dispatcher,
            WlSurfaceId::from(101),
            XdgSurfaceId::from(111),
        );
        let toplevel_object_id = assign_toplevel_role(
            &mut state,
            &mut client,
            &mut client_dispatcher,
            xdg_surface_object_id,
            XdgSurfaceId::from(111),
            XdgToplevelId::from(121),
        );

        {
            let surface = state
                .objects
                .get_mut(wl_surface_object_id)
                .downcast_mut::<WlSurface>()
                .unwrap();
            surface.test_map(ObjectId::from(701usize), 20, 30, 90, 70);
        }

        let focus = SurfaceFocus {
            client_id: client.client_id(),
            surface_id: WlSurfaceId::from(101),
        };
        let serial = state.seat.next_serial();
        state.seat.record_pointer_button(1, serial, Some(focus));

        {
            let mut toplevel = state.objects.grab_object::<XdgToplevel>(toplevel_object_id);
            toplevel.on(
                &mut client,
                &mut state,
                toplevel_move_request(XdgToplevelId::from(121), WlSeatId::from(6), serial),
                &mut client_dispatcher,
            );
        }

        assert!(matches!(
            state.seat.pointer_interactive_grab,
            Some(super::PointerInteractiveGrab::Move { .. })
        ));
    }

    #[test]
    fn interactive_resize_clamps_left_edge_to_toplevel_constraints() {
        let mut state = make_state();
        let (mut client, mut dispatcher) = make_client();
        let mut client_dispatcher = client.test_dispatcher(&mut dispatcher);

        client.insert_object(&mut state, WlSeat::new(WlSeatId::from(5)));

        let (wl_surface_object_id, xdg_surface_object_id) = insert_xdg_surface(
            &mut state,
            &mut client,
            &mut client_dispatcher,
            WlSurfaceId::from(130),
            XdgSurfaceId::from(140),
        );
        let toplevel_object_id = assign_toplevel_role(
            &mut state,
            &mut client,
            &mut client_dispatcher,
            xdg_surface_object_id,
            XdgSurfaceId::from(140),
            XdgToplevelId::from(150),
        );

        {
            let surface = state
                .objects
                .get_mut(wl_surface_object_id)
                .downcast_mut::<WlSurface>()
                .unwrap();
            surface.test_map(ObjectId::from(800usize), 40, 50, 100, 80);
        }

        {
            let mut toplevel = state.objects.grab_object::<XdgToplevel>(toplevel_object_id);
            toplevel.test_set_interactive_size_constraints(Some((90, 0)), Some((110, 0)));
        }

        let focus = SurfaceFocus {
            client_id: client.client_id(),
            surface_id: WlSurfaceId::from(130),
        };
        let serial = state.seat.next_serial();
        state.seat.record_pointer_button(1, serial, Some(focus));
        state.seat.pointer_pos = (40.0, 90.0);

        {
            let mut toplevel = state.objects.grab_object::<XdgToplevel>(toplevel_object_id);
            toplevel.on(
                &mut client,
                &mut state,
                toplevel_resize_request(
                    XdgToplevelId::from(150),
                    WlSeatId::from(5),
                    serial,
                    xdg_toplevel::ResizeEdge::Left as u32,
                ),
                &mut client_dispatcher,
            );
        }

        handle_pointer_motion(&mut state, &mut dispatcher, 60.0, 0.0, 110);

        let surface = state
            .objects
            .get(wl_surface_object_id)
            .downcast_ref::<WlSurface>()
            .unwrap();
        assert_eq!(surface.root_position(), (50, 50));
        assert!(dispatcher.test_has_queued_action_for::<ClientAction, ClientId>(
            client.client_id(),
            |action| {
                matches!(
                    action,
                    ClientAction::InteractiveResizeConfigure {
                        xdg_toplevel_id,
                        width,
                        height,
                        resizing,
                    } if *xdg_toplevel_id == XdgToplevelId::from(150)
                        && *width == 90
                        && *height == 80
                        && *resizing
                )
            }
        ));
    }

    #[test]
    fn get_popup_allows_null_parent_for_layer_shell_workflows() {
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

        assert!(client.contains_object(XdgPopupId::from(160), &state));

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

        assert!(!client.contains_object(XdgToplevelId::from(170), &state));
    }

    #[test]
    fn no_focus_fallback_installs_default_cursor() {
        let mut seat = SeatState::default();
        assert!(seat.set_cursor_surface(
            ClientId::from_raw(7),
            WlSurfaceId::from(50),
            ObjectId::from(80usize),
            4,
            5,
        ));

        assert!(matches!(
            seat.visible_cursor_source(),
            Some(CursorSourceRef::Surface { owner_client_id, cursor })
                if owner_client_id == ClientId::from_raw(7)
                    && cursor.object_id == ObjectId::from(80usize)
        ));

        assert!(seat.test_install_default_cursor());
        assert!(matches!(
            seat.visible_cursor_source(),
            Some(CursorSourceRef::Named { owner_client_id: None, .. })
        ));
    }

    #[test]
    fn explicit_client_cursor_clear_can_be_followed_by_default_fallback() {
        let mut seat = SeatState::default();
        seat.test_install_default_cursor();
        assert!(seat.set_cursor_surface(
            ClientId::from_raw(7),
            WlSurfaceId::from(50),
            ObjectId::from(80usize),
            4,
            5,
        ));

        assert!(seat.clear_cursor_for_client(ClientId::from_raw(7)));
        assert!(seat.visible_cursor_source().is_none());

        assert!(seat.test_install_default_cursor());
        assert!(matches!(
            seat.visible_cursor_source(),
            Some(CursorSourceRef::Named { owner_client_id: None, .. })
        ));
    }

    #[test]
    fn client_cursor_stays_visible_without_default_fallback() {
        let mut seat = SeatState::default();
        assert!(seat.set_cursor_surface(
            ClientId::from_raw(7),
            WlSurfaceId::from(50),
            ObjectId::from(80usize),
            4,
            5,
        ));

        assert!(matches!(
            seat.visible_cursor_source(),
            Some(CursorSourceRef::Surface { owner_client_id, cursor })
                if owner_client_id == ClientId::from_raw(7)
                    && cursor.object_id == ObjectId::from(80usize)
        ));
    }

    #[test]
    fn pointer_cursor_request_accepts_exact_enter_serial() {
        let mut seat = SeatState::default();
        seat.pointer_focus = Some(super::PointerFocus {
            target: SurfaceFocus {
                client_id: ClientId::from_raw(7),
                surface_id: WlSurfaceId::from(50),
            },
            rect: super::SurfaceRect { x: 0, y: 0, width: 100, height: 100 },
        });
        seat.record_pointer_enter(ClientId::from_raw(7), 100);

        assert!(seat.pointer_cursor_request_allowed(ClientId::from_raw(7), 100));
        assert!(!seat.pointer_cursor_request_allowed(ClientId::from_raw(7), 99));
    }

    #[test]
    fn pointer_cursor_request_rejects_wrong_enter_serial() {
        let mut seat = SeatState::default();
        seat.pointer_focus = Some(super::PointerFocus {
            target: SurfaceFocus {
                client_id: ClientId::from_raw(7),
                surface_id: WlSurfaceId::from(50),
            },
            rect: super::SurfaceRect { x: 0, y: 0, width: 100, height: 100 },
        });
        seat.record_pointer_enter(ClientId::from_raw(7), 100);

        assert!(!seat.pointer_cursor_request_allowed(ClientId::from_raw(7), 200));
    }

    #[test]
    fn pointer_cursor_request_accepts_active_grab_serial_for_focused_client() {
        let mut seat = SeatState::default();
        let focus = SurfaceFocus {
            client_id: ClientId::from_raw(7),
            surface_id: WlSurfaceId::from(50),
        };
        seat.pointer_focus = Some(super::PointerFocus {
            target: focus,
            rect: super::SurfaceRect { x: 0, y: 0, width: 100, height: 100 },
        });
        seat.record_pointer_enter(ClientId::from_raw(7), 10);
        seat.record_pointer_button(1, 20, Some(focus));

        assert!(seat.pointer_cursor_request_allowed(ClientId::from_raw(7), 20));
        assert!(!seat.pointer_cursor_request_allowed(ClientId::from_raw(7), 19));
    }

    #[test]
    fn local_drag_drop_without_source_finishes_immediately() {
        let mut state = make_state();
        let (_client, mut dispatcher) = make_client();
        let target_client_id = ClientId::from_raw(42);

        state.seat.set_active_drag(DragState {
            source_object_id: None,
            source_id: None,
            source_client_id: target_client_id,
            origin_surface_id: WlSurfaceId::from(10),
            icon_surface_object_id: None,
            icon_surface_id: None,
            target: Some(DragTargetState {
                client_id: target_client_id,
                surface_id: WlSurfaceId::from(11),
            }),
            negotiation: Default::default(),
            phase: DragPhase::Dragging,
        });

        complete_drag_drop(&mut state, &mut dispatcher);

        assert!(state.seat.active_drag().is_none());
        assert!(dispatcher
            .test_has_queued_action_for::<ClientAction, ClientId>(target_client_id, |action| {
                matches!(action, ClientAction::DataDeviceDragDrop)
            }));
    }

    #[test]
    fn invalid_drag_drop_cancels_source() {
        let mut state = make_state();
        let (_client, mut dispatcher) = make_client();
        let source_client_id = ClientId::from_raw(50);
        let target_client_id = ClientId::from_raw(51);
        let source_id = crate::protocol_generated::WlDataSourceId::from(12);

        state.seat.set_active_drag(DragState {
            source_object_id: None,
            source_id: Some(source_id),
            source_client_id,
            origin_surface_id: WlSurfaceId::from(20),
            icon_surface_object_id: None,
            icon_surface_id: None,
            target: Some(DragTargetState {
                client_id: target_client_id,
                surface_id: WlSurfaceId::from(21),
            }),
            negotiation: Default::default(),
            phase: DragPhase::Dragging,
        });

        complete_drag_drop(&mut state, &mut dispatcher);

        assert!(state.seat.active_drag().is_none());
        assert!(dispatcher.test_has_queued_action_for::<ClientAction, ClientId>(
            source_client_id,
            |action| {
                matches!(
                    action,
                    ClientAction::DataSourceEvent {
                        source_id: queued_source_id,
                        event: DataSourceClientEvent::Cancelled,
                    } if *queued_source_id == source_id
                )
            }
        ));
    }

    #[test]
    fn cancelling_drag_with_icon_refreshes_source_outputs() {
        let mut state = make_state();
        let (_client, mut dispatcher) = make_client();
        let source_client_id = ClientId::from_raw(60);

        state.seat.set_active_drag(DragState {
            source_object_id: None,
            source_id: None,
            source_client_id,
            origin_surface_id: WlSurfaceId::from(30),
            icon_surface_object_id: Some(ObjectId::from(123usize)),
            icon_surface_id: Some(WlSurfaceId::from(31)),
            target: None,
            negotiation: Default::default(),
            phase: DragPhase::Dragging,
        });

        cancel_active_drag(&mut state, &mut dispatcher, None);

        assert!(state.seat.active_drag().is_none());
        assert!(dispatcher
            .test_has_queued_action_for::<ClientAction, ClientId>(source_client_id, |action| {
                matches!(action, ClientAction::RefreshSurfaceOutputs)
            }));
    }
}
