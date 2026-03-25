use std::{
    collections::{BTreeMap, BTreeSet},
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
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
    io_uring::{
        model::IoUringAction,
        ring::{PollCompleted, RingId},
    },
    runner::{Dispatcher, Model, Redispatcher},
    seat,
    state::{InputId, State},
};

use {input::LibinputInterface, nix::unistd::dup};

struct Interface {
    dev_to_fd: Arc<Mutex<BTreeMap<u64, OwnedFd>>>,
}

impl LibinputInterface for Interface {
    #[allow(clippy::bad_bit_mask)]
    fn open_restricted(&mut self, path: &Path, _flags: i32) -> Result<OwnedFd, i32> {
        let Ok(stat) = nix::sys::stat::stat(path) else {
            log::error!("Failed to `stat(2)` {:?}", path);
            return Err(-1);
        };
        let fd = {
            let dev_to_fd = self.dev_to_fd.lock().unwrap();
            let Some(fd) = dev_to_fd.get(&stat.st_rdev) else {
                log::error!("No session fd cached for libinput device {:?}", path);
                return Err(-1);
            };
            match dup(fd.as_raw_fd()) {
                Ok(fd) => unsafe { OwnedFd::from_raw_fd(fd) },
                Err(e) => {
                    log::error!("Failed to dup libinput device fd for {:?}: {:?}", path, e);
                    return Err(-1);
                }
            }
        };
        Ok(fd)
    }

    fn close_restricted(&mut self, fd: OwnedFd) {
        drop(fd);
    }
}

pub struct Libinput {
    libinput: input::Libinput,
    /// We put that in an `Arc<Mutex<_>>` because not sure if libinput
    /// open the files in another thread or not
    dev_to_fd: Arc<Mutex<BTreeMap<u64, OwnedFd>>>,
    inputs: BTreeSet<InputId>,
    poll_op_id: Option<RingId>,
}

impl Libinput {
    pub fn new() -> Box<Self> {
        let dev_to_fd = Arc::default();
        let libinput =
            input::Libinput::new_from_path(Interface { dev_to_fd: Arc::clone(&dev_to_fd) });
        set_logger(&libinput);
        let inputs = Default::default();

        Box::new(Self { libinput, dev_to_fd, inputs, poll_op_id: None })
    }

    pub fn dispatch(&mut self) {
        self.libinput.dispatch().unwrap();
    }

    pub fn next_event(&mut self) -> Option<input::Event> {
        self.libinput.next()
    }

    fn dispatch_poll_libinput(&mut self, dispatcher: &mut Dispatcher) {
        let poll_op_id = RingId::fresh();
        self.poll_op_id = Some(poll_op_id);
        dispatcher.dispatch(IoUringAction::Poll {
            id: Some(poll_op_id),
            fd: self.libinput.as_raw_fd(),
            dispatch_to: Redispatcher::into::<LibinputAction>(),
        });
    }

    fn insert_device_fd(&mut self, fd: OwnedFd, devnum: u64) {
        let mut dev_to_fd = self.dev_to_fd.lock().unwrap();
        dev_to_fd.insert(devnum, fd);
    }
}

#[derive(Debug)]
pub enum LibinputAction {
    Start,
    Event,
    OpenInput { devnode: String, fd: OwnedFd, devnum: u64 },
    Stop,
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
    const STOP: Option<Self::Action> = Some(Self::Action::Stop);

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
                            PointerEvent::Motion(motion) => {
                                log::debug!(
                                    "libinput pointer motion dx={} dy={} time={}",
                                    motion.dx(),
                                    motion.dy(),
                                    motion.time()
                                );
                                seat::handle_pointer_motion(
                                    state,
                                    dispatcher,
                                    motion.dx(),
                                    motion.dy(),
                                    motion.time(),
                                )
                            }
                            PointerEvent::MotionAbsolute(motion) => {
                                let Some((width, height)) = current_output_size(state) else {
                                    continue;
                                };
                                let x = motion.absolute_x_transformed(width);
                                let y = motion.absolute_y_transformed(height);
                                log::debug!(
                                    "libinput pointer motion absolute x={} y={} time={}",
                                    x,
                                    y,
                                    motion.time()
                                );
                                seat::handle_pointer_motion_absolute(
                                    state,
                                    dispatcher,
                                    x,
                                    y,
                                    motion.time(),
                                );
                            }
                            PointerEvent::Button(button) => {
                                let button_state = match button.button_state() {
                                    ButtonState::Pressed => 1,
                                    ButtonState::Released => 0,
                                };
                                log::debug!(
                                    "libinput pointer button button={} state={} time={}",
                                    button.button(),
                                    button_state,
                                    button.time()
                                );
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
                                log::debug!(
                                    "libinput keyboard key key={} state={} time={}",
                                    key.key(),
                                    key_state,
                                    key.time()
                                );
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
            Stop => {
                if let Some(poll_op_id) = self.poll_op_id.take() {
                    dispatcher.dispatch(IoUringAction::PollRemove { target: poll_op_id });
                }
            }
        }
        Ok(())
    }
}

pub struct Input {
    device: input::Device,
}

fn current_output_size(state: &State) -> Option<(u32, u32)> {
    let (_, drm) = state.drm.iter().next()?;
    let mode = drm.output.in_use.connector.preferred_mode.as_ref()?;
    let (width, height) = mode.size();
    Some((width as u32, height as u32))
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
