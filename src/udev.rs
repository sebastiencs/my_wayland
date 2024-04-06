use std::{
    collections::BTreeMap,
    os::{
        fd::{AsRawFd, RawFd},
        unix::prelude::OsStrExt,
    },
    path::PathBuf,
};

use nix::{
    fcntl::{
        self,
        FcntlArg::{self, F_GETFL},
        OFlag,
    },
    sys::stat::{major, minor},
};

use crate::{
    dbus::DbusAction,
    drm::Drm,
    input::LibinputAction,
    io_uring::{model::IoUringAction, ring::PollCompleted},
    runner::{AnyAction, Dispatcher, Model, Redispatcher},
    state::State,
};

#[derive(Debug)]
pub struct Device {
    pub devnum: libc::dev_t,
    pub path_node: PathBuf,
    pub fd: RawFd,
}

impl Device {
    pub fn major(&self) -> u32 {
        major(self.devnum).try_into().unwrap()
    }

    pub fn minor(&self) -> u32 {
        minor(self.devnum).try_into().unwrap()
    }
}

enum PendingControl {
    Drm(udev::Device),
    Input(udev::Device),
}

pub struct Udev {
    udev_monitor: udev::MonitorSocket,
    pending: BTreeMap<libc::dev_t, PendingControl>,
}

pub enum UdevAction {
    Start,
    Event,
    DeviceTaken { fd: RawFd, inactive: bool, devnum: libc::dev_t },
}

impl Model for Udev {
    type Action = UdevAction;
    const START: Option<Self::Action> = Some(Self::Action::Start);

    fn process(
        &mut self,
        state: &mut State,
        action: Self::Action,
        dispatcher: &mut Dispatcher,
    ) -> Result<(), ()> {
        match action {
            UdevAction::Start => {
                self.dispatch_poll(dispatcher);
                self.enumerate(dispatcher).expect("Failed to enumerate");
            }
            UdevAction::Event => {
                // TODO: Remove `collect()`
                for event in self.udev_monitor.iter().collect::<Vec<_>>() {
                    self.handle_event(event, dispatcher)
                }
            }
            UdevAction::DeviceTaken { fd, inactive, devnum } => {
                match self.pending.remove(&devnum).unwrap() {
                    PendingControl::Drm(device) => {
                        self.on_drm_taken(device, fd, inactive, state, dispatcher)
                    }
                    PendingControl::Input(device) => {
                        self.on_input_taken(device, fd, inactive, devnum, dispatcher)
                    }
                }
                // self.on_taken(device, fd, inactive, devnum);
                // let Some(path_node) = device.devnode().map(PathBuf::from) else {
                //     return Ok(());
                // };
                // log::warn!("DEVICE TAKEN devum={:?} fd={:?} inactive={:?} {:?}", devnum, fd, inactive, path_node);
            }
        }
        Ok(())
    }
}

impl Udev {
    pub fn new() -> Result<Box<Udev>, std::io::Error> {
        let udev_monitor = udev::MonitorBuilder::new()?
            .match_subsystem("input")?
            .match_subsystem("drm")?
            .listen()?;
        let pending = BTreeMap::default();

        Ok(Box::new(Udev { udev_monitor, pending }))
    }

    fn on_udev_event(result: std::io::Result<PollCompleted>) -> AnyAction {
        let PollCompleted {} = result.unwrap();
        AnyAction::from(UdevAction::Event)
    }

    fn dispatch_poll(&self, dispatcher: &mut Dispatcher) {
        dispatcher.dispatch(IoUringAction::Poll {
            fd: self.udev_monitor.as_raw_fd(),
            dispatch_to: Redispatcher::new::<Self>(Self::on_udev_event),
        });
    }

    fn enumerate(&mut self, dispatcher: &mut Dispatcher) -> std::io::Result<()> {
        let mut enumerator = udev::Enumerator::new()?;
        enumerator.match_subsystem("input")?;
        enumerator.match_subsystem("drm")?;
        for device in enumerator.scan_devices()? {
            self.handle_new_device(device, dispatcher);
        }
        Ok(())
    }

    fn handle_event(&mut self, event: udev::Event, dispatcher: &mut Dispatcher) {
        use udev::EventType::*;

        match event.event_type() {
            Add => self.handle_new_device(event.device(), dispatcher),
            Change | Remove | Bind | Unbind | Unknown => {
                // TODO
                log::trace!("Ignored event {:?}", event.action())
            }
        }
    }

    fn handle_new_device(&mut self, device: udev::Device, dispatcher: &mut Dispatcher) {
        let Some(subsystem) = device.subsystem() else {
            return;
        };

        match subsystem.as_bytes() {
            b"input" => self.handle_new_input(device, dispatcher),
            b"drm" => self.handle_new_drm(device, dispatcher),
            _ => {}
        }
    }

    fn handle_new_input(&mut self, device: udev::Device, dispatcher: &mut Dispatcher) {
        if !device.is_initialized() {
            return;
        }
        let Some(devnum) = device.devnum() else {
            return;
        };
        dispatcher.dispatch(DbusAction::TakeDevice { devnum });
        self.pending.insert(devnum, PendingControl::Input(device));
    }

    fn handle_new_drm(&mut self, device: udev::Device, dispatcher: &mut Dispatcher) {
        if !device.is_initialized() {
            return;
        }
        let Some(devnum) = device.devnum() else {
            return;
        };

        // From xf86drm.h
        // #define DRM_DIR_NAME  "/dev/dri"
        // #define DRM_PRIMARY_MINOR_NAME  "card"
        // #define DRM_CONTROL_MINOR_NAME  "controlD"
        // #define DRM_RENDER_MINOR_NAME   "renderD"
        // #define DRM_PROC_NAME "/proc/dri/" /* For backward Linux compatibility */
        dispatcher.dispatch(DbusAction::TakeDevice { devnum });
        self.pending.insert(devnum, PendingControl::Drm(device));
    }

    fn on_input_taken(
        &mut self,
        device: udev::Device,
        fd: RawFd,
        inactive: bool,
        devnum: libc::dev_t,
        dispatcher: &mut Dispatcher,
    ) {
        if inactive {
            return;
        }
        let Some(devnode) = device.devnode().and_then(|devnode| devnode.to_str()) else {
            return;
        };
        let devnode = devnode.to_string();
        set_non_blocking(fd);

        dispatcher.dispatch(LibinputAction::OpenInput { devnode, fd, devnum });
    }

    fn on_drm_taken(
        &mut self,
        device: udev::Device,
        fd: RawFd,
        inactive: bool,
        state: &mut State,
        dispatcher: &mut Dispatcher,
    ) {
        if inactive {
            return;
        }

        eprintln!("DRM TAKEN {:?}", device.devnode());

        let drm = Drm::create_drm(fd, device, &state.egl);

        let drm_model = drm.make_model();
        dispatcher.start_model2(drm_model);

        state.drm.insert(drm);
    }
}

fn set_non_blocking(fd: RawFd) {
    let flags = fcntl::fcntl(fd, F_GETFL).unwrap();
    let flags = OFlag::from_bits_retain(flags);

    if !flags.contains(OFlag::O_NONBLOCK) {
        fcntl::fcntl(fd, FcntlArg::F_SETFL(OFlag::O_NONBLOCK | flags)).unwrap();
    }
}
