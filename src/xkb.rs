use std::{
    ffi::CStr,
    os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd, RawFd},
    ptr::NonNull,
};

use xkbcommon::xkb;

use crate::{input::ForeignStr, wayland_objects::wl_shm_pool::sys};

pub struct Xkb {
    ctx: xkb::Context,
    keymap: xkb::Keymap,
    pub keymap_fd: MemFd,
}

impl Xkb {
    pub fn new() -> Self {
        let ctx = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
        let keymap =
            xkb::Keymap::new_from_names(&ctx, "", "", "us", "", None, xkb::KEYMAP_COMPILE_NO_FLAGS)
                .unwrap();

        let keymap_fd = {
            let s = keymap_as_cstr(&keymap);
            let s_slice = s.to_bytes_with_nul();
            new_memfd(s_slice.as_ptr(), s_slice.len()).unwrap()
        };

        Self { ctx, keymap, keymap_fd }
    }

    pub fn keymap_fd(&self) -> &MemFd {
        &self.keymap_fd
    }
}

fn keymap_as_cstr<'a>(keymap: &xkb::Keymap) -> ForeignStr<'a> {
    unsafe {
        let s =
            xkb::ffi::xkb_keymap_get_as_string(keymap.get_raw_ptr(), xkb::KEYMAP_FORMAT_TEXT_V1);
        ForeignStr::from(CStr::from_ptr(s))
    }
}

pub struct MemFd {
    fd: OwnedFd,
    size: usize,
}

impl MemFd {
    pub fn raw_fd(&self) -> RawFd {
        self.fd.as_raw_fd()
    }
    pub fn len(&self) -> usize {
        self.size
    }
}

fn new_memfd(data: *const u8, size: usize) -> std::io::Result<MemFd> {
    const READONLY_SEALS: core::ffi::c_int =
        libc::F_SEAL_SHRINK | libc::F_SEAL_GROW | libc::F_SEAL_WRITE | libc::F_SEAL_SEAL;

    let fd = unsafe {
        let fd = {
            libc::memfd_create(
                c"my_wayland-anon".as_ptr(),
                libc::MFD_CLOEXEC | libc::MFD_ALLOW_SEALING,
            )
        };
        if fd == -1 {
            return Err(std::io::Error::last_os_error());
        }
        let mut ret = libc::EINTR;
        while ret == libc::EINTR {
            ret = libc::posix_fallocate(fd, 0, size as _);
        }
        if ret != 0 {
            return Err(std::io::Error::last_os_error());
        }

        let map = sys::mmap(fd, size)?;
        map.copy_from_nonoverlapping(NonNull::new(data.cast_mut()).unwrap(), size);
        sys::munmap(map, size)?;

        libc::fcntl(fd, libc::F_ADD_SEALS, READONLY_SEALS);

        OwnedFd::from_raw_fd(fd)
    };

    Ok(MemFd { fd, size })
}
