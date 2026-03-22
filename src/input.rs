use std::{
    collections::{BTreeMap, BTreeSet},
    os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd},
    path::Path,
    sync::{Arc, Mutex},
};

use input::{
    event::{
        keyboard::{KeyState, KeyboardEvent, KeyboardEventTrait},
        pointer::{ButtonState, PointerEvent, PointerEventTrait},
    },
    AsRaw,
};

use crate::{
    io_uring::{model::IoUringAction, ring::PollCompleted},
    runner::{Dispatcher, Model, Redispatcher},
    seat,
    state::{InputId, State},
};

use {input::LibinputInterface, nix::unistd::close};

struct Interface {
    dev_to_fd: Arc<Mutex<BTreeMap<u64, RawFd>>>,
}

impl LibinputInterface for Interface {
    #[allow(clippy::bad_bit_mask)]
    fn open_restricted(&mut self, path: &Path, _flags: i32) -> Result<OwnedFd, i32> {
        let Ok(stat) = nix::sys::stat::stat(path) else {
            log::error!("Failed to `stat(2)` {:?}", path);
            return Err(-1);
        };
        // TODO: Handle errors
        let fd = {
            let dev_to_fd = self.dev_to_fd.lock().unwrap();
            dev_to_fd.get(&stat.st_rdev).copied().unwrap()
        };
        Ok(unsafe { OwnedFd::from_raw_fd(fd) })
    }

    fn close_restricted(&mut self, fd: OwnedFd) {
        if let Err(e) = close(fd.as_raw_fd()) {
            log::error!("Failed to close libinput fd: {:?}", e);
        }
    }
}

pub struct Libinput {
    libinput: input::Libinput,
    /// We put that in an `Arc<Mutex<_>>` because not sure if libinput
    /// open the files in another thread or not
    dev_to_fd: Arc<Mutex<BTreeMap<u64, RawFd>>>,
    inputs: BTreeSet<InputId>,
}

impl Libinput {
    pub fn new() -> Box<Self> {
        let dev_to_fd = Arc::default();
        let libinput =
            input::Libinput::new_from_path(Interface { dev_to_fd: Arc::clone(&dev_to_fd) });
        set_logger(&libinput);
        let inputs = Default::default();

        Box::new(Self { libinput, dev_to_fd, inputs })
    }

    pub fn dispatch(&mut self) {
        self.libinput.dispatch().unwrap();
    }

    pub fn next_event(&mut self) -> Option<input::Event> {
        self.libinput.next()
    }

    fn dispatch_poll_libinput(&self, dispatcher: &mut Dispatcher) {
        dispatcher.dispatch(IoUringAction::Poll {
            fd: self.libinput.as_raw_fd(),
            dispatch_to: Redispatcher::into::<LibinputAction>(),
        });
    }

    fn insert_device_fd(&mut self, fd: RawFd, devnum: u64) {
        let mut dev_to_fd = self.dev_to_fd.lock().unwrap();
        dev_to_fd.insert(devnum, fd);
    }
}

#[derive(Debug)]
pub enum LibinputAction {
    Start,
    Event,
    OpenInput { devnode: String, fd: RawFd, devnum: u64 },
}

impl From<std::io::Result<PollCompleted>> for LibinputAction {
    fn from(result: std::io::Result<PollCompleted>) -> Self {
        let PollCompleted {} = result.unwrap();
        Self::Event
    }
}

impl Model for Libinput {
    type Action = LibinputAction;

    const START: Option<Self::Action> = Some(Self::Action::Start);

    fn process(
        &mut self,
        state: &mut State,
        action: Self::Action,
        dispatcher: &mut Dispatcher,
    ) -> Result<(), ()> {
        use LibinputAction::*;

        match action {
            Start => {
                self.dispatch_poll_libinput(dispatcher);
            }
            Event => {
                self.dispatch();
                while let Some(event) = self.next_event() {
                    match event {
                        input::Event::Pointer(pointer) => match pointer {
                            PointerEvent::Motion(motion) => seat::handle_pointer_motion(
                                state,
                                dispatcher,
                                motion.dx(),
                                motion.dy(),
                                motion.time(),
                            ),
                            PointerEvent::Button(button) => {
                                let button_state = match button.button_state() {
                                    ButtonState::Pressed => 1,
                                    ButtonState::Released => 0,
                                };
                                seat::handle_pointer_button(
                                    state,
                                    dispatcher,
                                    button.button(),
                                    button_state,
                                    button.time(),
                                );
                            }
                            _ => {}
                        },
                        input::Event::Keyboard(keyboard) => match keyboard {
                            KeyboardEvent::Key(key) => {
                                let key_state = match key.key_state() {
                                    KeyState::Pressed => 1,
                                    KeyState::Released => 0,
                                };
                                seat::handle_keyboard_key(
                                    state,
                                    dispatcher,
                                    key.key(),
                                    key_state,
                                    key.time(),
                                );
                            }
                            _ => {}
                        },
                        _ => {}
                    }
                }
                self.dispatch_poll_libinput(dispatcher);
            }
            OpenInput { devnode, fd, devnum } => {
                self.insert_device_fd(fd, devnum);
                let device = match self.libinput.path_add_device(&devnode) {
                    Some(device) => device,
                    None => {
                        log::error!("Failed to get {:?}", devnode);
                        return Ok(());
                    }
                };

                let input_id = state.inputs.insert(Input { device });
                self.inputs.insert(input_id);
            }
        }
        Ok(())
    }
}

pub struct Input {
    device: input::Device,
}

#[derive(derive_more::Deref, derive_more::From)]
pub struct ForeignStr<'a>(&'a std::ffi::CStr);
impl<'a> Drop for ForeignStr<'a> {
    fn drop(&mut self) {
        unsafe { libc::free(self.as_ptr() as *mut _) }
    }
}

unsafe extern "C" fn log_handler(
    _libinput: *mut input::ffi::libinput,
    priority: input::ffi::libinput_log_priority,
    format: *const libc::c_char,
    args: std::ffi::VaList,
) {
    extern "C" {
        fn vasprintf(
            strp: *mut *mut libc::c_char,
            fmt: *const libc::c_char,
            ap: std::ffi::VaList,
        ) -> libc::c_int;
    }

    let s: ForeignStr = {
        let mut strp = std::ptr::null_mut();
        if vasprintf(&mut strp, format, args) == -1 {
            return log::error!("Unable to format libinput logs");
        }
        std::ffi::CStr::from_ptr(strp).into()
    };

    let priority = match priority {
        input::ffi::libinput_log_priority_LIBINPUT_LOG_PRIORITY_DEBUG => log::Level::Debug,
        input::ffi::libinput_log_priority_LIBINPUT_LOG_PRIORITY_INFO => log::Level::Info,
        _ => log::Level::Error,
    };

    log::log!(priority, "libinput: {}", s.to_string_lossy().trim());
}

fn set_logger(libinput: &input::Libinput) {
    #[link(name = "input")]
    extern "C" {
        pub fn libinput_log_set_handler(
            libinput: *mut libc::c_void,
            log_handler: *mut libc::c_void,
        );
    }

    unsafe {
        libinput_log_set_handler(libinput.as_raw() as *mut _, log_handler as _);

        input::ffi::libinput_log_set_priority(
            libinput.as_raw() as *mut _,
            input::ffi::libinput_log_priority_LIBINPUT_LOG_PRIORITY_DEBUG,
        );
    }
}
