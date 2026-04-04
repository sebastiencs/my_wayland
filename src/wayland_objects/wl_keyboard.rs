use crate::{
    clients::{Client, ClientDispatcher, EventWriter, RawRequest, Resolved},
    object::{Object, Parser},
    protocol_generated::{wl_keyboard, Array, WaylandId, WlKeyboardId, WlSurfaceId},
    seat::empty_keys,
    state::State,
    wayland_objects::wl_display::WlDisplay,
    xkb::Xkb,
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
        &mut self,
        client: &mut Client,
        state: &mut State,
        req: RawRequest,
        dispatcher: &mut ClientDispatcher,
    ) -> crate::object::RequestResult {
        use wl_keyboard::Request::*;

        match client.parse::<Self>(req).unwrap() {
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
        let keymap = xkb.keymap_fd();

        self.send_event(
            wl_keyboard::Event::Keymap {
                format: wl_keyboard::KeymapFormat::XkbV1 as u32,
                fd: keymap.raw_fd(),
                size: keymap.len() as u32,
            },
            dispatcher,
        )
    }

    pub fn send_repeat_info(&self, dispatcher: &mut ClientDispatcher) {
        self.send_event(wl_keyboard::Event::RepeatInfo { rate: 25, delay: 250 }, dispatcher)
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
