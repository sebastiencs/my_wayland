use std::{
    collections::BTreeMap,
    os::{
        fd::{AsRawFd, RawFd},
        unix::prelude::OsStrExt,
    },
    path::PathBuf,
};

use nix::sys::stat::{major, minor};

use crate::{
    dbus::DbusAction,
    io_uring::{model::IoUringAction, ring::PollCompleted},
    state::{AnyAction, Dispatcher, Model, Redispatcher},
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

pub struct Udev {
    udev_monitor: udev::MonitorSocket,
    pending: BTreeMap<libc::dev_t, udev::Device>,
}

pub enum UdevAction {
    Start,
    Event,
    DeviceTaken { fd: RawFd, inactive: bool, devnum: libc::dev_t },
}

impl Model for Udev {
    type InitParams = ();
    type Action = UdevAction;
    const START: Option<Self::Action> = Some(Self::Action::Start);

    fn init(_params: Self::InitParams) -> Box<Self> {
        todo!()
    }

    fn process(&mut self, action: Self::Action, dispatcher: &mut Dispatcher) -> Result<(), ()> {
        match action {
            UdevAction::Start => {
                self.dispatch_poll(dispatcher);
                self.enumerate(dispatcher).expect("Failed to enumerate");
            }
            UdevAction::Event => {
                for event in self.udev_monitor.iter() {
                    self.handle_event(event, dispatcher)
                }
            }
            UdevAction::DeviceTaken { fd, inactive, devnum } => {
                let device = self.pending.remove(&devnum).unwrap();
                self.on_taken(device, fd, inactive, devnum);
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
            dispatch_to: Redispatcher::new(Self::on_udev_event),
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
        self.pending.insert(devnum, device);
    }

    fn handle_new_drm(&mut self, device: udev::Device, dispatcher: &mut Dispatcher) {
        if !device.is_initialized() {
            return;
        }
        let Some(devnum) = device.devnum() else {
            return;
        };
        dispatcher.dispatch(DbusAction::TakeDevice { devnum });
        self.pending.insert(devnum, device);
    }

    fn on_taken(&mut self, device: udev::Device, fd: RawFd, inactive: bool, devnum: libc::dev_t) {
        if inactive {
            return;
        }
    }
}
