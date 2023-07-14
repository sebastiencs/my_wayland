use std::{
    fs::{File, OpenOptions},
    os::{
        fd::{AsRawFd, OwnedFd},
        unix::prelude::OpenOptionsExt,
    },
    path::Path,
};

use {
    input::LibinputInterface,
    libc::{O_RDONLY, O_RDWR, O_WRONLY},
};

struct Interface;

impl LibinputInterface for Interface {
    #[allow(clippy::bad_bit_mask)]
    fn open_restricted(&mut self, path: &Path, flags: i32) -> Result<OwnedFd, i32> {
        eprintln!("OPEN_RESTRICTED {:?} {:?}", path, flags);
        OpenOptions::new()
            .custom_flags(flags)
            .read((flags & O_RDONLY != 0) | (flags & O_RDWR != 0))
            .write((flags & O_WRONLY != 0) | (flags & O_RDWR != 0))
            .open(path)
            .map(|file| file.into())
            .map_err(|err| err.raw_os_error().unwrap())
    }
    fn close_restricted(&mut self, fd: OwnedFd) {
        eprintln!("CLOSE_RESTRICTED");
        drop(File::from(fd));
    }
}

pub struct Libinput {
    libinput: input::Libinput,
}

impl AsRawFd for Libinput {
    fn as_raw_fd(&self) -> std::os::fd::RawFd {
        self.libinput.as_raw_fd()
    }
}

impl Libinput {
    pub fn new() -> Self {
        // let mut input = Libinput::new_with_udev(Interface);
        // input.udev_assign_seat(&seat_id).unwrap();
        let libinput = input::Libinput::new_from_path(Interface);
        Self { libinput }
    }

    pub fn dispatch(&mut self) {
        self.libinput.dispatch().unwrap();
    }

    pub fn next_event(&mut self) -> Option<input::Event> {
        self.libinput.next()
    }
}
