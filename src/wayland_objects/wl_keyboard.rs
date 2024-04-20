use crate::{
    clients::{Client, ClientDispatcher, EventWriter, RawRequest},
    object::{Object, Parser},
    protocol_generated::{wl_keyboard, WaylandId, WlKeyboardId},
    state::State,
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

    fn on(&mut self, _: &mut Client, _: &mut State, req: RawRequest, _: &mut ClientDispatcher) {
        use wl_keyboard::Request::*;

        match Self::parse(req).unwrap() {
            Release => todo!(),
        }
    }

    fn as_any(&mut self) -> &mut dyn std::any::Any {
        self
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
}
