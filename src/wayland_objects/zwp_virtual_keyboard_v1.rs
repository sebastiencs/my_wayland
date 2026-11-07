//! `zwp_virtual_keyboard_v1`.
//!
//! Keys and modifiers are delivered to the focused client. They do not enter
//! `handle_keyboard_key`, so compositor bindings and the input-method grab
//! never see them.

use std::{collections::BTreeSet, os::fd::AsRawFd, rc::Rc};

use crate::{
    clients::{Client, ClientDispatcher, RawRequest},
    object::{Object, Parser, RequestResult},
    protocol_generated::{
        wl_display, wl_keyboard, zwp_virtual_keyboard_v1, WaylandId, ZwpVirtualKeyboardV1Id,
    },
    seat::{self, monotonic_time_msec},
    state::{GrabbedObject, State},
    wayland_objects::wl_display::WlDisplay,
    xkb::{KeymapBlob, Xkb},
};

const MAX_KEYMAP_BYTES: u32 = 1024 * 1024;

#[derive(Debug)]
pub struct ZwpVirtualKeyboardV1 {
    id: ZwpVirtualKeyboardV1Id,
    version: u32,
    keymap: Option<Rc<KeymapBlob>>,
    pressed: BTreeSet<u32>,
}

impl ZwpVirtualKeyboardV1 {
    pub(crate) fn new(id: ZwpVirtualKeyboardV1Id, version: u32) -> Self {
        Self { id, version, keymap: None, pressed: BTreeSet::new() }
    }

    fn keymap_error(&self, client: &mut Client, dispatcher: &mut ClientDispatcher, message: &str) {
        client.protocol_violation_disconnect(
            dispatcher,
            self.id.into(),
            zwp_virtual_keyboard_v1::Error::NoKeymap as u32,
            message,
        );
    }

    fn invalid(&self, client: &mut Client, dispatcher: &mut ClientDispatcher, message: &str) {
        client.protocol_violation_disconnect(
            dispatcher,
            self.id.into(),
            wl_display::Error::InvalidMethod as u32,
            message,
        );
    }

    /// Releases still held by this keyboard, under the keymap the client has.
    fn release_pressed(&mut self, state: &mut State, dispatcher: &mut crate::runner::Dispatcher) {
        if self.pressed.is_empty() {
            return;
        }
        let keys: Vec<u32> = self.pressed.iter().copied().collect();
        self.pressed.clear();
        let Some(focus) = state.seat.keyboard_focus() else {
            return;
        };
        let time = monotonic_time_msec();
        for key in keys {
            let serial = state.seat.next_serial();
            seat::dispatch_client_event(
                dispatcher,
                focus.client_id(),
                seat::SeatClientEvent::KeyboardKey { serial, time, key, state: 0 },
            );
        }
    }
}

impl Object for ZwpVirtualKeyboardV1 {
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
        use zwp_virtual_keyboard_v1::Request::*;

        match client.parse::<Self>(req, dispatcher)? {
            Keymap { format, fd, size } => {
                if format != wl_keyboard::KeymapFormat::XkbV1 as u32 {
                    self.invalid(
                        client,
                        dispatcher,
                        "zwp_virtual_keyboard_v1.keymap format must be xkb_v1",
                    );
                    return Ok(());
                }
                if size == 0 || size > MAX_KEYMAP_BYTES {
                    self.invalid(
                        client,
                        dispatcher,
                        "zwp_virtual_keyboard_v1.keymap size must be between 1 and 1048576 bytes",
                    );
                    return Ok(());
                }
                let text = match map_keymap(fd.as_raw_fd(), size as usize) {
                    Ok(text) => text,
                    Err(message) => {
                        self.invalid(client, dispatcher, &message);
                        return Ok(());
                    }
                };
                let keymap = match Xkb::compile_keymap_from_string(&text) {
                    Ok(keymap) => keymap,
                    Err(message) => {
                        self.invalid(
                            client,
                            dispatcher,
                            &format!("zwp_virtual_keyboard_v1.keymap: {message}"),
                        );
                        return Ok(());
                    }
                };
                self.release_pressed(state, dispatcher);
                self.keymap = Some(keymap);
            }
            Key { time, key, state: key_state } => {
                let Some(keymap) = self.keymap.clone() else {
                    self.keymap_error(
                        client,
                        dispatcher,
                        "zwp_virtual_keyboard_v1.key requires a keymap",
                    );
                    return Ok(());
                };
                let pressed = key_state == 1;
                if key_state > 1 {
                    self.invalid(
                        client,
                        dispatcher,
                        "zwp_virtual_keyboard_v1.key state must be released or pressed",
                    );
                    return Ok(());
                }
                let legal = if pressed {
                    !self.pressed.contains(&key)
                } else {
                    self.pressed.contains(&key)
                };
                if !legal {
                    return Ok(());
                }
                let Some(focus) = state.seat.keyboard_focus() else {
                    return Ok(());
                };
                let held = state.keyboard.pressed_keys();
                let modifiers = state.keyboard.modifiers();
                seat::ensure_client_keymap(state, dispatcher, focus, keymap, held, modifiers);
                if pressed {
                    self.pressed.insert(key);
                } else {
                    self.pressed.remove(&key);
                }
                let serial = state.seat.next_serial();
                seat::dispatch_client_event(
                    dispatcher,
                    focus.client_id(),
                    seat::SeatClientEvent::KeyboardKey { serial, time, key, state: key_state },
                );
            }
            Modifiers { mods_depressed, mods_latched, mods_locked, group } => {
                let Some(keymap) = self.keymap.clone() else {
                    self.keymap_error(
                        client,
                        dispatcher,
                        "zwp_virtual_keyboard_v1.modifiers requires a keymap",
                    );
                    return Ok(());
                };
                let Some(focus) = state.seat.keyboard_focus() else {
                    return Ok(());
                };
                let held = state.keyboard.pressed_keys();
                seat::ensure_client_keymap(
                    state,
                    dispatcher,
                    focus,
                    keymap,
                    held,
                    crate::xkb::KeyboardModifiers {
                        mods_depressed,
                        mods_latched,
                        mods_locked,
                        group,
                    },
                );
                let serial = state.seat.next_serial();
                seat::dispatch_client_event(
                    dispatcher,
                    focus.client_id(),
                    seat::SeatClientEvent::KeyboardModifiers {
                        serial,
                        mods_depressed,
                        mods_latched,
                        mods_locked,
                        group,
                    },
                );
            }
            Destroy => {
                self.release_pressed(state, dispatcher);
                client.delete_id(&WlDisplay::default(), state, dispatcher, self.id.into());
            }
        }
        let _ = self.version;
        Ok(())
    }

    fn on_client_disconnect(
        &mut self,
        _client: &mut Client,
        state: &mut State,
        dispatcher: &mut ClientDispatcher,
    ) {
        self.release_pressed(state, dispatcher);
    }
}

fn map_keymap(fd: std::os::fd::RawFd, size: usize) -> Result<String, String> {
    let ptr = unsafe {
        libc::mmap(std::ptr::null_mut(), size, libc::PROT_READ, libc::MAP_PRIVATE, fd, 0)
    };
    if ptr == libc::MAP_FAILED {
        return Err(format!(
            "zwp_virtual_keyboard_v1.keymap mmap failed: {}",
            std::io::Error::last_os_error()
        ));
    }
    let bytes = unsafe { std::slice::from_raw_parts(ptr as *const u8, size) }.to_vec();
    let unmap = unsafe { libc::munmap(ptr, size) };
    if unmap == -1 {
        return Err(format!(
            "zwp_virtual_keyboard_v1.keymap munmap failed: {}",
            std::io::Error::last_os_error()
        ));
    }
    if bytes.last() != Some(&0) {
        return Err("zwp_virtual_keyboard_v1.keymap is not NUL terminated".to_string());
    }
    let text = std::str::from_utf8(&bytes[..bytes.len() - 1])
        .map_err(|_| "zwp_virtual_keyboard_v1.keymap is not UTF-8".to_string())?;
    Ok(text.to_string())
}

#[cfg(test)]
mod tests {
    use std::{
        collections::VecDeque,
        os::fd::{FromRawFd, OwnedFd},
    };

    use crate::{
        clients::{Arguments, Client, ClientAction, ClientId, RawRequest},
        input::SurfaceFocus,
        io_uring::model::IoUringAction,
        object::{Object, RequestResult},
        protocol_generated::{
            wl_display, wl_keyboard, WlSeatId, WlSurfaceId, ZwpVirtualKeyboardManagerV1Id,
        },
        runner::{Dispatcher, Model},
        state::{ObjectId, State},
        test_support::state_for_tests,
        wayland_objects::{
            wl_keyboard::WlKeyboard, wl_seat::WlSeat, wl_surface::WlSurface,
            zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1,
        },
    };

    const KEYMAP: &str = r#"xkb_keymap {
xkb_keycodes "(unnamed)" {
    minimum = 8;
    maximum = 255;
    <ESC> = 9;
};
xkb_types "(unnamed)" {
    virtual_modifiers NumLock;
    type "ONE_LEVEL" {
        modifiers= none;
        level_name[Level1]= "Any";
    };
};
xkb_compatibility "(unnamed)" {
};
xkb_symbols "(unnamed)" {
    name[group1]="Virtual";
    key <ESC> { [ Escape ] };
};
};
"#;

    struct Fixture {
        state: State,
        dispatcher: Dispatcher,
        app: Client,
        vk: Client,
    }

    fn new_fixture() -> Fixture {
        let mut state = state_for_tests();
        let mut dispatcher = Dispatcher::default();
        let mut app = Client::new_for_tests(ClientId::from_raw(1));
        let surface = {
            let (surface, _) = app.insert_object(
                &mut state,
                WlSurface::new(WlSurfaceId::from(5), app.client_id(), 6),
            );
            state.register_root_surface(surface);
            surface
        };
        let _ = app
            .insert_object(&mut state, WlSeat::new(WlSeatId::from(4)))
            .0;
        let mut vk = Client::new_for_tests(ClientId::from_raw(2));
        let _ = vk
            .insert_object(&mut state, WlSeat::new(WlSeatId::from(4)))
            .0;
        let _ = vk
            .insert_object(
                &mut state,
                ZwpVirtualKeyboardManagerV1::new(ZwpVirtualKeyboardManagerV1Id::from(6), 1),
            )
            .0;
        let mut fixture = Fixture { state, dispatcher, app, vk };
        call(&mut fixture.app, &mut fixture.state, &mut fixture.dispatcher, 4, 1, &[8], None)
            .unwrap();
        let surface = surface;
        crate::input::set_keyboard_focus(
            &mut fixture.state,
            &mut fixture.dispatcher,
            Some(SurfaceFocus::new(surface)),
        );
        drain(&mut fixture.app, &mut fixture.state, &mut fixture.dispatcher);
        call(&mut fixture.vk, &mut fixture.state, &mut fixture.dispatcher, 6, 0, &[4, 40], None)
            .unwrap();
        fixture
    }

    fn request(object_id: u32, request_id: u16, words: &[u32]) -> RawRequest<'static> {
        RawRequest {
            object_id,
            request_id,
            arguments: Arguments::from_words(VecDeque::from(words.to_vec())),
            fds: None,
        }
    }

    fn call(
        client: &mut Client,
        state: &mut State,
        dispatcher: &mut Dispatcher,
        wire_id: u32,
        request_id: u16,
        words: &[u32],
        fd: Option<OwnedFd>,
    ) -> RequestResult {
        let object = client
            .object_id_for_wayland(wire_id)
            .unwrap_or_else(|| panic!("missing object {wire_id}"));
        let mut fds = fd.into_iter().collect::<VecDeque<_>>();
        let mut request = request(wire_id, request_id, words);
        request.fds = Some(&mut fds);
        let mut client_dispatcher = client.test_dispatcher(dispatcher);
        client_dispatcher.set_requester_object_id_for_tests(wire_id.into());
        state
            .objects
            .grab(object)
            .on(client, state, request, &mut client_dispatcher)
    }

    fn drain(client: &mut Client, state: &mut State, dispatcher: &mut Dispatcher) {
        let actions =
            dispatcher.test_take_actions_for::<ClientAction, ClientId>(client.client_id());
        for action in actions {
            if matches!(action, ClientAction::SeatEvent { .. }) {
                client.process(state, action, dispatcher).unwrap();
            }
        }
    }

    fn text_fd(text: &str) -> (OwnedFd, u32) {
        let mut bytes = text.as_bytes().to_vec();
        bytes.push(0);
        let fd = unsafe { libc::memfd_create(c"keymap".as_ptr(), libc::MFD_CLOEXEC) };
        assert!(fd >= 0);
        let written = unsafe { libc::write(fd, bytes.as_ptr().cast(), bytes.len()) };
        assert_eq!(written, bytes.len() as isize);
        (unsafe { OwnedFd::from_raw_fd(fd) }, bytes.len() as u32)
    }

    fn dummy_fd() -> OwnedFd {
        text_fd("x").0
    }

    fn send_keymap(fixture: &mut Fixture, text: &str) {
        let (fd, size) = text_fd(text);
        call(
            &mut fixture.vk,
            &mut fixture.state,
            &mut fixture.dispatcher,
            40,
            0,
            &[wl_keyboard::KeymapFormat::XkbV1 as u32, size],
            Some(fd),
        )
        .unwrap();
    }

    fn events(dispatcher: &Dispatcher, object: u32) -> Vec<(u32, Vec<u32>)> {
        let mut events = Vec::new();
        dispatcher.test_for_each_queued_action::<IoUringAction>(|action| {
            if let IoUringAction::Sendmsg { buffer, .. } = action {
                if buffer.len() >= 8
                    && u32::from_ne_bytes(buffer[0..4].try_into().unwrap()) == object
                {
                    let opcode = u32::from_ne_bytes(buffer[4..8].try_into().unwrap()) & 0xffff;
                    let mut args = Vec::new();
                    let mut offset = 8;
                    while offset + 4 <= buffer.len() {
                        args.push(u32::from_ne_bytes(
                            buffer[offset..offset + 4].try_into().unwrap(),
                        ));
                        offset += 4;
                    }
                    events.push((opcode, args));
                }
            }
        });
        events
    }

    fn closed(dispatcher: &Dispatcher, client: ClientId) -> bool {
        dispatcher.test_has_queued_action(|action: &crate::clients::ClientManagerAction| {
            matches!(action, crate::clients::ClientManagerAction::Close { id } if *id == client)
        })
    }

    fn error_code(dispatcher: &Dispatcher) -> Option<u32> {
        events(dispatcher, 1)
            .into_iter()
            .find_map(|(opcode, args)| (opcode == 0 && args.len() >= 2).then_some(args[1]))
    }

    fn keymap_id(fixture: &Fixture) -> u64 {
        fixture
            .state
            .seat
            .client_keymap(fixture.app.client_id())
            .map(|keymap| keymap.id)
            .unwrap_or(0)
    }

    #[test]
    fn keymap_validation_errors_disconnect_the_client() {
        let mut fixture = new_fixture();
        call(
            &mut fixture.vk,
            &mut fixture.state,
            &mut fixture.dispatcher,
            40,
            0,
            &[0, 1],
            Some(dummy_fd()),
        )
        .unwrap();
        assert!(closed(&fixture.dispatcher, fixture.vk.client_id()));
        assert_eq!(error_code(&fixture.dispatcher), Some(wl_display::Error::InvalidMethod as u32));

        let mut fixture = new_fixture();
        call(
            &mut fixture.vk,
            &mut fixture.state,
            &mut fixture.dispatcher,
            40,
            0,
            &[1, 0],
            Some(dummy_fd()),
        )
        .unwrap();
        assert!(closed(&fixture.dispatcher, fixture.vk.client_id()));

        let mut fixture = new_fixture();
        call(
            &mut fixture.vk,
            &mut fixture.state,
            &mut fixture.dispatcher,
            40,
            0,
            &[1, 1024 * 1024 + 1],
            Some(dummy_fd()),
        )
        .unwrap();
        assert!(closed(&fixture.dispatcher, fixture.vk.client_id()));

        let mut fixture = new_fixture();
        send_keymap(&mut fixture, "not a keymap");
        assert!(closed(&fixture.dispatcher, fixture.vk.client_id()));
        assert_eq!(error_code(&fixture.dispatcher), Some(wl_display::Error::InvalidMethod as u32));

        let mut fixture = new_fixture();
        call(&mut fixture.vk, &mut fixture.state, &mut fixture.dispatcher, 40, 1, &[0, 1, 1], None)
            .unwrap();
        assert!(closed(&fixture.dispatcher, fixture.vk.client_id()));
        assert_eq!(error_code(&fixture.dispatcher), Some(0));
    }

    #[test]
    fn a_virtual_key_switches_the_keymap_and_a_hardware_key_switches_back() {
        let mut fixture = new_fixture();
        let before = events(&fixture.dispatcher, 8).len();
        send_keymap(&mut fixture, KEYMAP);
        call(
            &mut fixture.vk,
            &mut fixture.state,
            &mut fixture.dispatcher,
            40,
            1,
            &[10, 1, 1],
            None,
        )
        .unwrap();
        drain(&mut fixture.app, &mut fixture.state, &mut fixture.dispatcher);
        let switched: Vec<u32> = events(&fixture.dispatcher, 8)[before..]
            .iter()
            .map(|(opcode, _)| *opcode)
            .collect();
        assert_eq!(switched, vec![0, 4, 3]);
        assert_ne!(keymap_id(&fixture), 0);
        assert!(fixture.state.keyboard.pressed_keys().0.is_empty());

        let before_mods = events(&fixture.dispatcher, 8).len();
        call(
            &mut fixture.vk,
            &mut fixture.state,
            &mut fixture.dispatcher,
            40,
            2,
            &[5, 0, 0, 0],
            None,
        )
        .unwrap();
        drain(&mut fixture.app, &mut fixture.state, &mut fixture.dispatcher);
        let mods = &events(&fixture.dispatcher, 8)[before_mods..];
        assert_eq!(mods.len(), 1);
        assert_eq!(mods[0].0, 4);
        assert_eq!(mods[0].1[1], 5);

        let before_hardware = events(&fixture.dispatcher, 8).len();
        crate::seat::handle_keyboard_key(&mut fixture.state, &mut fixture.dispatcher, 30, 1, 20);
        drain(&mut fixture.app, &mut fixture.state, &mut fixture.dispatcher);
        let back: Vec<u32> = events(&fixture.dispatcher, 8)[before_hardware..]
            .iter()
            .map(|(opcode, _)| *opcode)
            .collect();
        assert_eq!(back.first().copied(), Some(0));
        assert_eq!(keymap_id(&fixture), 0);
    }

    #[test]
    fn held_keys_wrap_the_keymap_in_leave_and_enter() {
        let mut fixture = new_fixture();
        crate::seat::handle_keyboard_key(&mut fixture.state, &mut fixture.dispatcher, 30, 1, 5);
        drain(&mut fixture.app, &mut fixture.state, &mut fixture.dispatcher);
        let before = events(&fixture.dispatcher, 8).len();
        send_keymap(&mut fixture, KEYMAP);
        call(
            &mut fixture.vk,
            &mut fixture.state,
            &mut fixture.dispatcher,
            40,
            1,
            &[11, 16, 1],
            None,
        )
        .unwrap();
        drain(&mut fixture.app, &mut fixture.state, &mut fixture.dispatcher);
        let opcodes: Vec<u32> = events(&fixture.dispatcher, 8)[before..]
            .iter()
            .map(|(opcode, _)| *opcode)
            .collect();
        assert_eq!(opcodes, vec![2, 0, 1, 4, 3]);
    }

    #[test]
    fn destroying_the_virtual_keyboard_releases_stuck_keys() {
        let mut fixture = new_fixture();
        send_keymap(&mut fixture, KEYMAP);
        call(
            &mut fixture.vk,
            &mut fixture.state,
            &mut fixture.dispatcher,
            40,
            1,
            &[7, 16, 1],
            None,
        )
        .unwrap();
        drain(&mut fixture.app, &mut fixture.state, &mut fixture.dispatcher);
        let before = events(&fixture.dispatcher, 8).len();
        call(&mut fixture.vk, &mut fixture.state, &mut fixture.dispatcher, 40, 3, &[], None)
            .unwrap();
        drain(&mut fixture.app, &mut fixture.state, &mut fixture.dispatcher);
        let release = events(&fixture.dispatcher, 8)[before..]
            .iter()
            .find(|(opcode, _)| *opcode == 3)
            .cloned();
        let (_, args) = release.expect("release");
        assert_eq!(args[2], 16);
        assert_eq!(args[3], 0);
    }

    #[test]
    fn a_key_with_no_focus_is_dropped() {
        let mut fixture = new_fixture();
        crate::input::set_keyboard_focus(&mut fixture.state, &mut fixture.dispatcher, None);
        drain(&mut fixture.app, &mut fixture.state, &mut fixture.dispatcher);
        send_keymap(&mut fixture, KEYMAP);
        let before = events(&fixture.dispatcher, 8).len();
        call(
            &mut fixture.vk,
            &mut fixture.state,
            &mut fixture.dispatcher,
            40,
            1,
            &[4, 16, 1],
            None,
        )
        .unwrap();
        drain(&mut fixture.app, &mut fixture.state, &mut fixture.dispatcher);
        assert_eq!(events(&fixture.dispatcher, 8).len(), before);
        let surface = fixture
            .state
            .seat
            .keyboard_focus_history()
            .last()
            .copied()
            .expect("remembered focus");
        crate::input::set_keyboard_focus(
            &mut fixture.state,
            &mut fixture.dispatcher,
            Some(surface),
        );
        drain(&mut fixture.app, &mut fixture.state, &mut fixture.dispatcher);
        let before = events(&fixture.dispatcher, 8).len();
        call(&mut fixture.vk, &mut fixture.state, &mut fixture.dispatcher, 40, 3, &[], None)
            .unwrap();
        drain(&mut fixture.app, &mut fixture.state, &mut fixture.dispatcher);
        assert!(events(&fixture.dispatcher, 8)[before..]
            .iter()
            .all(|(opcode, _)| *opcode != 3));
    }

    #[test]
    fn get_keyboard_sends_the_clients_current_keymap() {
        let mut fixture = new_fixture();
        send_keymap(&mut fixture, KEYMAP);
        call(&mut fixture.vk, &mut fixture.state, &mut fixture.dispatcher, 40, 1, &[9, 1, 1], None)
            .unwrap();
        drain(&mut fixture.app, &mut fixture.state, &mut fixture.dispatcher);
        let current = fixture
            .state
            .seat
            .client_keymap(fixture.app.client_id())
            .expect("virtual keymap")
            .fd
            .len() as u32;
        call(&mut fixture.app, &mut fixture.state, &mut fixture.dispatcher, 8, 0, &[], None)
            .unwrap();
        call(&mut fixture.app, &mut fixture.state, &mut fixture.dispatcher, 4, 1, &[9], None)
            .unwrap();
        let sizes: Vec<u32> = events(&fixture.dispatcher, 9)
            .into_iter()
            .filter_map(|(opcode, args)| (opcode == 0).then(|| args[1]))
            .collect();
        assert_eq!(sizes, vec![current]);
    }
}
