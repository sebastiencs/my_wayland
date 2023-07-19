use std::{
    collections::BTreeMap,
    os::fd::{AsRawFd, BorrowedFd, OwnedFd, RawFd},
    path::Path,
    sync::{Arc, Mutex},
};

use crate::{
    io_uring::{model::IoUringAction, ring::PollCompleted},
    state::{Dispatcher, Model, Redispatcher},
};

use {input::LibinputInterface, nix::unistd::close};

struct Interface {
    dev_to_fd: Arc<Mutex<BTreeMap<u64, RawFd>>>,
}

impl LibinputInterface for Interface {
    #[allow(clippy::bad_bit_mask)]
    fn open_restricted(&mut self, path: &Path, flags: i32) -> Result<OwnedFd, i32> {
        let Ok(stat) = nix::sys::stat::stat(path) else {
            log::error!("Failed to `stat(2)` {:?}", path);
            return Err(-1);
        };
        // TODO: Handle errors
        let fd = {
            let dev_to_fd = self.dev_to_fd.lock().unwrap();
            dev_to_fd.get(&stat.st_rdev).copied().unwrap()
        };
        let borrowed_fd = unsafe { BorrowedFd::borrow_raw(fd) };
        Ok(borrowed_fd.try_clone_to_owned().unwrap())
    }

    fn close_restricted(&mut self, fd: OwnedFd) {
        if let Err(e) = close(fd.as_raw_fd()) {
            log::error!("Failed to close libinput fd: {:?}", e);
        }
    }
}

pub struct Libinput {
    libinput: input::Libinput,
    dev_to_fd: Arc<Mutex<BTreeMap<u64, RawFd>>>,
}

impl Libinput {
    pub fn new() -> Box<Self> {
        let dev_to_fd = Arc::default();
        let libinput =
            input::Libinput::new_from_path(Interface { dev_to_fd: Arc::clone(&dev_to_fd) });
        Box::new(Self { libinput, dev_to_fd })
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
    type InitParams = ();
    type Action = LibinputAction;

    const START: Option<Self::Action> = Some(Self::Action::Start);

    fn init(_params: Self::InitParams) -> Box<Self> {
        todo!()
    }

    fn process(&mut self, action: Self::Action, dispatcher: &mut Dispatcher) -> Result<(), ()> {
        use LibinputAction::*;

        match action {
            Start => {
                self.dispatch_poll_libinput(dispatcher);
            }
            Event => {
                log::info!("LIBINPUT EVENT")
            }
            OpenInput { devnode, fd, devnum } => {
                self.insert_device_fd(fd, devnum);
                let device = self.libinput.path_add_device(&devnode);
                dbg!(&device);
            }
        }
        Ok(())
    }
}
