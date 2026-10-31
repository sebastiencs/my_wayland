use std::{
    collections::{BTreeMap, BTreeSet},
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
    path::Path,
    sync::{Arc, Mutex},
};

use input::{
    event::{
        keyboard::{KeyState, KeyboardEvent, KeyboardEventTrait},
        pointer::{Axis, ButtonState, PointerEvent, PointerEventTrait, PointerScrollEvent},
        EventTrait,
    },
    AsRaw,
};

use crate::{
    io_uring::{
        model::IoUringAction,
        ring::{PollCompleted, RingId},
    },
    runner::{Dispatcher, Model, Redispatcher},
    seat::{
        self, PointerAxisChange, PointerScrollDirection, PointerScrollFrame, PointerScrollSource,
    },
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
            dispatch_to: Redispatcher::to::<Libinput>(|result: std::io::Result<PollCompleted>| {
                let PollCompleted {} = result.unwrap();
                LibinputAction::Event
            }),
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
    ) -> std::result::Result<(), crate::runner::ModelFailure> {
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
                                let Some(bounds) = current_output_layout_bounds(state) else {
                                    continue;
                                };
                                let raw_x = bounds.x as f64
                                    + motion.absolute_x_transformed(bounds.width as u32) as f64;
                                let raw_y = bounds.y as f64
                                    + motion.absolute_y_transformed(bounds.height as u32) as f64;
                                let (x, y) = state
                                    .clamp_point_to_outputs(
                                        raw_x.round() as i32,
                                        raw_y.round() as i32,
                                    )
                                    .map(|(x, y)| (x as f64, y as f64))
                                    .unwrap_or((raw_x, raw_y));
                                // log::debug!(
                                //     "libinput pointer motion absolute x={} y={} time={}",
                                //     x,
                                //     y,
                                //     motion.time()
                                // );
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
                            PointerEvent::ScrollWheel(wheel) => {
                                deliver_wheel_scroll(state, dispatcher, &wheel)
                            }
                            PointerEvent::ScrollFinger(finger) => deliver_smooth_scroll(
                                state,
                                dispatcher,
                                PointerScrollSource::Finger,
                                &finger,
                            ),
                            PointerEvent::ScrollContinuous(continuous) => deliver_smooth_scroll(
                                state,
                                dispatcher,
                                PointerScrollSource::Continuous,
                                &continuous,
                            ),
                            // libinput emits the deprecated axis event in addition to the
                            // scroll events above. Forwarding both would scroll twice.
                            #[allow(deprecated)]
                            PointerEvent::Axis(_) => {}
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

fn current_output_layout_bounds(state: &State) -> Option<crate::region::RectI> {
    state.output_layout_bounds()
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

/// One reported libinput axis. `value120` is present for wheel events.
struct LibinputScrollAxis {
    value: f64,
    value120: Option<f64>,
}

/// Build one scroll frame from one libinput scroll event.
///
/// `value` is libinput's `scroll_value`: degrees for a wheel, pointer-motion units for
/// finger and continuous sources. Wheel frames also carry `scroll_value_v120` and the
/// whole clicks that distance contains. A reported axis whose value is zero is a stop.
/// An axis libinput did not report stays absent, so a wheel timeout that produces no
/// event does not invent motion.
///
/// wlroots' `handle_pointer_axis_value120` forwards this same pair. Jay turns `value120`
/// into pixels with a per-device wheel scale; this milestone keeps libinput's values.
/// Relative direction follows the device natural-scroll setting, which both wlroots and
/// Jay report, because libinput has already applied that transform to the values.
fn pointer_scroll_frame(
    time: u32,
    source: PointerScrollSource,
    natural_scroll: bool,
    vertical: Option<LibinputScrollAxis>,
    horizontal: Option<LibinputScrollAxis>,
) -> Option<PointerScrollFrame> {
    let frame = PointerScrollFrame {
        time,
        source,
        vertical: vertical.map(|axis| pointer_axis_change(natural_scroll, axis)),
        horizontal: horizontal.map(|axis| pointer_axis_change(natural_scroll, axis)),
    };
    (frame.vertical.is_some() || frame.horizontal.is_some()).then_some(frame)
}

fn pointer_axis_change(natural_scroll: bool, axis: LibinputScrollAxis) -> PointerAxisChange {
    if axis.value == 0.0 {
        return PointerAxisChange::Stop;
    }
    let value120 = axis.value120.and_then(nonzero_value120);
    let discrete = value120.and_then(|value| {
        let steps = value / 120;
        (steps != 0).then_some(steps)
    });
    PointerAxisChange::Scroll {
        value: axis.value as f32,
        discrete,
        value120,
        relative_direction: Some(if natural_scroll {
            PointerScrollDirection::Inverted
        } else {
            PointerScrollDirection::Identical
        }),
    }
}

fn nonzero_value120(value: f64) -> Option<i32> {
    let value = value.round() as i32;
    (value != 0).then_some(value)
}

fn deliver_wheel_scroll(
    state: &mut State,
    dispatcher: &mut Dispatcher,
    event: &input::event::pointer::PointerScrollWheelEvent,
) {
    deliver_pointer_scroll(
        state,
        dispatcher,
        event.time(),
        PointerScrollSource::Wheel,
        event.device().config_scroll_natural_scroll_enabled(),
        wheel_axis(event, Axis::Vertical),
        wheel_axis(event, Axis::Horizontal),
    );
}

fn deliver_smooth_scroll<E>(
    state: &mut State,
    dispatcher: &mut Dispatcher,
    source: PointerScrollSource,
    event: &E,
) where
    E: PointerScrollEvent + PointerEventTrait + EventTrait,
{
    deliver_pointer_scroll(
        state,
        dispatcher,
        event.time(),
        source,
        event.device().config_scroll_natural_scroll_enabled(),
        smooth_axis(event, Axis::Vertical),
        smooth_axis(event, Axis::Horizontal),
    );
}

fn deliver_pointer_scroll(
    state: &mut State,
    dispatcher: &mut Dispatcher,
    time: u32,
    source: PointerScrollSource,
    natural_scroll: bool,
    vertical: Option<LibinputScrollAxis>,
    horizontal: Option<LibinputScrollAxis>,
) {
    let Some(frame) = pointer_scroll_frame(time, source, natural_scroll, vertical, horizontal)
    else {
        return;
    };
    log::debug!(
        "libinput pointer scroll source={:?} vertical={:?} horizontal={:?} time={}",
        frame.source,
        frame.vertical,
        frame.horizontal,
        frame.time
    );
    seat::handle_pointer_scroll(state, dispatcher, frame);
}

fn wheel_axis(
    event: &input::event::pointer::PointerScrollWheelEvent,
    axis: Axis,
) -> Option<LibinputScrollAxis> {
    event.has_axis(axis).then(|| LibinputScrollAxis {
        value: event.scroll_value(axis),
        value120: Some(event.scroll_value_v120(axis)),
    })
}

fn smooth_axis<E: PointerScrollEvent>(event: &E, axis: Axis) -> Option<LibinputScrollAxis> {
    event
        .has_axis(axis)
        .then(|| LibinputScrollAxis { value: event.scroll_value(axis), value120: None })
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

#[cfg(test)]
mod tests {
    use {
        super::{pointer_scroll_frame, LibinputScrollAxis},
        crate::seat::{
            PointerAxisChange, PointerScrollDirection, PointerScrollFrame, PointerScrollSource,
        },
    };

    fn axis(value: f64, value120: Option<f64>) -> LibinputScrollAxis {
        LibinputScrollAxis { value, value120 }
    }

    fn frame(
        source: PointerScrollSource,
        natural_scroll: bool,
        vertical: Option<LibinputScrollAxis>,
        horizontal: Option<LibinputScrollAxis>,
    ) -> Option<PointerScrollFrame> {
        pointer_scroll_frame(12, source, natural_scroll, vertical, horizontal)
    }

    #[test]
    fn wheel_click_carries_degrees_and_value120() {
        let frame = frame(PointerScrollSource::Wheel, false, Some(axis(-15.0, Some(-120.0))), None)
            .expect("wheel click");
        assert_eq!(
            frame.vertical,
            Some(PointerAxisChange::Scroll {
                value: -15.0,
                discrete: Some(-1),
                value120: Some(-120),
                relative_direction: Some(PointerScrollDirection::Identical),
            })
        );
        assert!(frame.horizontal.is_none());
    }

    #[test]
    fn natural_scroll_marks_the_logical_direction_inverted() {
        let frame = frame(PointerScrollSource::Wheel, true, None, Some(axis(15.0, Some(120.0))))
            .expect("horizontal wheel");
        assert_eq!(
            frame.horizontal,
            Some(PointerAxisChange::Scroll {
                value: 15.0,
                discrete: Some(1),
                value120: Some(120),
                relative_direction: Some(PointerScrollDirection::Inverted),
            })
        );
    }

    #[test]
    fn partial_high_resolution_wheel_omits_a_whole_click() {
        let frame = frame(PointerScrollSource::Wheel, false, Some(axis(-3.75, Some(-30.0))), None)
            .expect("partial wheel");
        assert_eq!(
            frame.vertical,
            Some(PointerAxisChange::Scroll {
                value: -3.75,
                discrete: None,
                value120: Some(-30),
                relative_direction: Some(PointerScrollDirection::Identical),
            })
        );
    }

    #[test]
    fn two_wheel_clicks_keep_both_step_counts() {
        let frame = frame(PointerScrollSource::Wheel, false, Some(axis(30.0, Some(240.0))), None)
            .expect("two clicks");
        assert_eq!(
            frame.vertical,
            Some(PointerAxisChange::Scroll {
                value: 30.0,
                discrete: Some(2),
                value120: Some(240),
                relative_direction: Some(PointerScrollDirection::Identical),
            })
        );
    }

    #[test]
    fn finger_motion_stays_smooth_and_a_zero_axis_stops() {
        let frame =
            frame(PointerScrollSource::Finger, false, Some(axis(0.0, None)), Some(axis(4.5, None)))
                .expect("finger frame");
        assert_eq!(frame.vertical, Some(PointerAxisChange::Stop));
        assert_eq!(
            frame.horizontal,
            Some(PointerAxisChange::Scroll {
                value: 4.5,
                discrete: None,
                value120: None,
                relative_direction: Some(PointerScrollDirection::Identical),
            })
        );
    }

    #[test]
    fn continuous_stop_only_frame_has_no_distance() {
        let frame = frame(PointerScrollSource::Continuous, true, Some(axis(0.0, None)), None)
            .expect("continuous stop");
        assert_eq!(frame.vertical, Some(PointerAxisChange::Stop));
        assert!(frame.horizontal.is_none());
    }

    #[test]
    fn an_event_with_no_reported_axis_is_dropped() {
        assert!(frame(PointerScrollSource::Wheel, false, None, None).is_none());
    }
}
