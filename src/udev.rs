use std::os::fd::{AsRawFd, RawFd};

use crate::{
    io_uring::{model::IoUringAction, ring::PollCompleted},
    state::{AnyAction, Dispatcher, Model, Redispatcher},
};

pub struct Device {}

pub struct Udev {
    udev_monitor: udev::MonitorSocket,
    // fd: RawFd,
}

pub enum UdevAction {
    Start,
    Event,
}

impl Model for Udev {
    type InitParams = ();
    type Action = UdevAction;

    fn init(params: Self::InitParams) -> Box<Self> {
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

        Ok(Box::new(Udev { udev_monitor }))
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

    fn enumerate(&self, dispatcher: &mut Dispatcher) -> std::io::Result<()> {
        let mut enumerator = udev::Enumerator::new()?;
        enumerator.match_subsystem("input")?;
        enumerator.match_subsystem("drm")?;
        for device in enumerator.scan_devices()? {
            self.handle_new_device(device, dispatcher)
        }
        Ok(())
    }

    fn handle_event(&self, event: udev::Event, dispatcher: &mut Dispatcher) {
        use udev::EventType::*;

        match event.event_type() {
            Add => self.handle_new_device(event.device(), dispatcher),
            Change | Remove | Bind | Unbind | Unknown => {
                // TODO
                log::trace!("Ignored event {:?}", event.action())
            }
        }
    }

    fn handle_new_device(&self, device: udev::Device, dispatcher: &mut Dispatcher) {
        if !device.is_initialized() {
            return;
        }
        dbg!(device.devnum(), device.devnode());
    }
}
