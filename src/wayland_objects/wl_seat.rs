use crate::{
    clients::{Client, ClientDispatcher, EventWriter, RawRequest},
    globals::Global,
    object::{Object, Parser},
    protocol_generated::{wl_keyboard, wl_seat, Constants, NewId, WaylandId, WlSeatId},
    state::State,
    wayland_objects::wl_keyboard::WlKeyboard,
};

#[derive(Debug)]
pub struct WlSeatGlobal;

impl WlSeatGlobal {
    pub fn new() -> Box<dyn Global> {
        Box::new(Self {})
    }
}

impl Global for WlSeatGlobal {
    fn interface(&self) -> (&'static str, u32) {
        (WlSeat::INTERFACE_NAME, WlSeat::INTERFACE_VERSION)
    }
    fn bind(
        &mut self,
        client: &mut Client,
        state: &mut State,
        id: NewId,
        _version: u32,
        dispatcher: &mut ClientDispatcher,
    ) {
        let id = WlSeatId::from(id);
        let wl_seat = WlSeat { id };

        wl_seat.send_capabilities(dispatcher);
        // TODO: Check version
        wl_seat.send_name(dispatcher);

        client.insert_object(state, wl_seat);
    }
}

#[derive(Debug)]
pub struct WlSeat {
    id: WlSeatId,
}

impl WlSeat {
    pub fn new(id: WlSeatId) -> Self {
        Self { id }
    }
}

impl Object for WlSeat {
    fn id(&self) -> WaylandId {
        self.id.into()
    }

    fn on(
        &mut self,
        client: &mut Client,
        state: &mut State,
        req: RawRequest,
        dispatcher: &mut ClientDispatcher,
    ) {
        use wl_seat::Request::*;

        match client.parse::<Self>(req).unwrap() {
            GetPointer { id } => todo!(),
            GetKeyboard { id } => {
                let wl_keyboard = WlKeyboard::new(id);

                wl_keyboard.send_keymap(&state.xkb, dispatcher);
                wl_keyboard.send_repeat_info(dispatcher);

                client.insert_object(state, wl_keyboard);
            }
            GetTouch { id } => todo!(),
            Release => todo!(),
        }
    }
}

impl WlSeat {
    pub fn send_capabilities(&self, dispatcher: &mut ClientDispatcher) {
        use wl_seat::Capability::{Keyboard, Pointer};

        let capabilites =
            wl_seat::Event::Capabilities { capabilities: Keyboard as u32 | Pointer as u32 };
        self.send_event(capabilites, dispatcher);
    }

    pub fn send_name(&self, dispatcher: &mut ClientDispatcher) {
        let name = wl_seat::Event::Name { name: "my_seat".to_string() };
        self.send_event(name, dispatcher);
    }
}
