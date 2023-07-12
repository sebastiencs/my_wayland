use std::{
    fs::{File, OpenOptions},
    os::{
        fd::{AsRawFd, OwnedFd, RawFd},
        unix::prelude::OpenOptionsExt,
    },
    path::Path,
};

use input::Libinput;

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

pub fn init(seat_id: String) -> Libinput {
    let mut input = Libinput::new_with_udev(Interface);
    input.udev_assign_seat(&seat_id).unwrap();

    // loop {
    //     input.dispatch().unwrap();
    //     for event in &mut input {
    //         println!("Got event: {:?}", event);
    //     }
    // }

    input
}
