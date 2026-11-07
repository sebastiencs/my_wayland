pub const KEY_REPEAT_RATE: i32 = 25;
pub const KEY_REPEAT_DELAY: i32 = 250;

use crate::{
    clients::{Client, ClientDispatcher, EventWriter, RawRequest, Resolved},
    object::{Object, Parser, RequestResult},
    protocol_generated::{wl_keyboard, Array, WaylandId, WlKeyboardId, WlSurfaceId},
    seat::empty_keys,
    state::{GrabbedObject, State},
    wayland_objects::wl_display::WlDisplay,
    xkb::{KeymapBlob, Xkb},
};

#[derive(Debug)]
pub struct WlKeyboard {
    id: WlKeyboardId,
}

impl WlKeyboard {
    pub fn new(id: WlKeyboardId) -> Self {
        Self { id }
    }
}

impl Object for WlKeyboard {
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
        use wl_keyboard::Request::*;

        match client.parse::<Self>(req, dispatcher)? {
            Release => {
                if let Some(keyboard) = client.resolve(self.id, state) {
                    state.seat.unregister_keyboard(keyboard);
                }
                client.delete_id(&WlDisplay::default(), state, dispatcher, self.id.into());
            }
        }
        Ok(())
    }
}

impl WlKeyboard {
    pub fn send_keymap(&self, xkb: &Xkb, dispatcher: &mut ClientDispatcher) {
        self.send_keymap_blob(&xkb.keymap_blob(), dispatcher);
    }

    pub fn send_keymap_blob(&self, keymap: &KeymapBlob, dispatcher: &mut ClientDispatcher) {
        self.send_event(
            wl_keyboard::Event::Keymap {
                format: wl_keyboard::KeymapFormat::XkbV1 as u32,
                fd: keymap.fd.rc(),
                size: keymap.fd.len() as u32,
            },
            dispatcher,
        )
    }

    pub fn send_repeat_info(&self, dispatcher: &mut ClientDispatcher) {
        self.send_event(
            wl_keyboard::Event::RepeatInfo { rate: KEY_REPEAT_RATE, delay: KEY_REPEAT_DELAY },
            dispatcher,
        )
    }

    pub fn send_enter(
        &self,
        serial: u32,
        surface: Resolved<WlSurfaceId>,
        keys: Array,
        dispatcher: &mut ClientDispatcher,
    ) {
        self.send_event(
            wl_keyboard::Event::Enter { serial, surface: surface.wayland_id(), keys },
            dispatcher,
        );
    }

    pub fn send_leave(
        &self,
        serial: u32,
        surface: Resolved<WlSurfaceId>,
        dispatcher: &mut ClientDispatcher,
    ) {
        self.send_event(
            wl_keyboard::Event::Leave { serial, surface: surface.wayland_id() },
            dispatcher,
        );
    }

    pub fn send_key(
        &self,
        serial: u32,
        time: u32,
        key: u32,
        state: u32,
        dispatcher: &mut ClientDispatcher,
    ) {
        self.send_event(wl_keyboard::Event::Key { serial, time, key, state }, dispatcher);
    }

    pub fn send_modifiers(
        &self,
        serial: u32,
        mods_depressed: u32,
        mods_latched: u32,
        mods_locked: u32,
        group: u32,
        dispatcher: &mut ClientDispatcher,
    ) {
        self.send_event(
            wl_keyboard::Event::Modifiers {
                serial,
                mods_depressed,
                mods_latched,
                mods_locked,
                group,
            },
            dispatcher,
        );
    }

    pub fn send_empty_enter(
        &self,
        serial: u32,
        surface: Resolved<WlSurfaceId>,
        dispatcher: &mut ClientDispatcher,
    ) {
        self.send_enter(serial, surface, empty_keys(), dispatcher);
    }
}
