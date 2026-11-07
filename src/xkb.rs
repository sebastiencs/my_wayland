use std::{
    ffi::CStr,
    os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd},
    ptr::NonNull,
    rc::Rc,
    sync::atomic::{AtomicU64, Ordering},
};

use xkbcommon::xkb;

use crate::{libinput::ForeignStr, protocol_generated::Array, wayland_objects::wl_shm_pool::sys};

pub struct Xkb {
    ctx: xkb::Context,
    keymap: xkb::Keymap,
    keymap_blob: Rc<KeymapBlob>,
}

/// One compiled keymap and the sealed memfd clients map.
///
/// The hardware keymap is id 0. Every keymap compiled from a client string
/// gets the next id.
#[derive(Debug)]
pub struct KeymapBlob {
    pub id: u64,
    pub fd: MemFd,
}

static NEXT_KEYMAP_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct KeyboardModifiers {
    pub mods_depressed: u32,
    pub mods_latched: u32,
    pub mods_locked: u32,
    pub group: u32,
}

pub struct KeyboardState {
    state: xkb::State,
    pressed_keys: Vec<u32>,
    modifiers: KeyboardModifiers,
}

impl Xkb {
    pub fn try_new() -> Result<Self, String> {
        let ctx = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
        let keymap =
            xkb::Keymap::new_from_names(&ctx, "", "", "us", "", None, xkb::KEYMAP_COMPILE_NO_FLAGS)
                .ok_or_else(|| "failed to compile the us keymap".to_string())?;

        let keymap_blob = Rc::new(KeymapBlob {
            id: 0,
            fd: keymap_memfd(&keymap).map_err(|error| format!("keymap memfd: {error}"))?,
        });

        Ok(Self { ctx, keymap, keymap_blob })
    }

    /// Compile a keymap string and serialize that compiled map into a new memfd.
    pub fn compile_keymap_from_string(text: &str) -> Result<Rc<KeymapBlob>, String> {
        if text.contains('\0') {
            return Err("keymap contains an interior NUL".to_string());
        }
        let ctx = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
        let keymap = xkb::Keymap::new_from_string(
            &ctx,
            text.to_string(),
            xkb::KEYMAP_FORMAT_TEXT_V1,
            xkb::KEYMAP_COMPILE_NO_FLAGS,
        )
        .ok_or_else(|| "failed to compile keymap".to_string())?;
        let fd = keymap_memfd(&keymap).map_err(|error| format!("keymap memfd: {error}"))?;
        let id = NEXT_KEYMAP_ID.fetch_add(1, Ordering::Relaxed);
        Ok(Rc::new(KeymapBlob { id, fd }))
    }

    /// The us keymap is present in tests. Startup uses [`Self::try_new`].
    pub fn new() -> Self {
        Self::try_new().expect("us keymap")
    }

    pub fn keymap_fd(&self) -> &MemFd {
        &self.keymap_blob.fd
    }

    pub fn keymap_blob(&self) -> Rc<KeymapBlob> {
        Rc::clone(&self.keymap_blob)
    }

    pub fn keymap(&self) -> &xkb::Keymap {
        &self.keymap
    }

    pub fn create_keyboard_state(&self) -> KeyboardState {
        KeyboardState::new(&self.keymap)
    }
}

impl KeyboardState {
    fn new(keymap: &xkb::Keymap) -> Self {
        let state = xkb::State::new(keymap);
        let modifiers = serialize_modifiers(&state);
        Self { state, pressed_keys: Vec::new(), modifiers }
    }

    pub fn pressed_keys(&self) -> Array {
        Array(self.pressed_keys.clone())
    }

    pub fn modifiers(&self) -> KeyboardModifiers {
        self.modifiers
    }

    pub fn effective_modifiers(&self) -> u32 {
        self.state.serialize_mods(xkb::STATE_MODS_EFFECTIVE)
    }

    pub fn key_get_one_sym(&self, key: u32) -> xkb::Keysym {
        self.state.key_get_one_sym((key + 8).into())
    }

    pub fn update_key(&mut self, key: u32, is_pressed: bool) -> Option<KeyboardModifiers> {
        let changed = if is_pressed {
            if self.pressed_keys.contains(&key) {
                false
            } else {
                self.pressed_keys.push(key);
                self.pressed_keys.sort_unstable();
                true
            }
        } else {
            let old_len = self.pressed_keys.len();
            self.pressed_keys.retain(|pressed| *pressed != key);
            self.pressed_keys.len() != old_len
        };

        if !changed {
            return None;
        }

        let direction = if is_pressed { xkb::KeyDirection::Down } else { xkb::KeyDirection::Up };
        self.state.update_key((key + 8).into(), direction);

        let modifiers = serialize_modifiers(&self.state);
        if modifiers == self.modifiers {
            return None;
        }

        self.modifiers = modifiers;
        Some(modifiers)
    }
}

fn serialize_modifiers(state: &xkb::State) -> KeyboardModifiers {
    KeyboardModifiers {
        mods_depressed: state.serialize_mods(xkb::STATE_MODS_DEPRESSED),
        mods_latched: state.serialize_mods(xkb::STATE_MODS_LATCHED),
        mods_locked: state.serialize_mods(xkb::STATE_MODS_LOCKED),
        group: state.serialize_layout(xkb::STATE_LAYOUT_EFFECTIVE),
    }
}

fn keymap_memfd(keymap: &xkb::Keymap) -> std::io::Result<MemFd> {
    let s = keymap_as_cstr(keymap);
    let bytes = s.to_bytes_with_nul();
    new_memfd(bytes.as_ptr(), bytes.len())
}

fn keymap_as_cstr<'a>(keymap: &xkb::Keymap) -> ForeignStr<'a> {
    unsafe {
        let s =
            xkb::ffi::xkb_keymap_get_as_string(keymap.get_raw_ptr(), xkb::KEYMAP_FORMAT_TEXT_V1);
        ForeignStr::from(CStr::from_ptr(s))
    }
}

#[derive(Debug)]
pub struct MemFd {
    fd: Rc<OwnedFd>,
    size: usize,
}

impl MemFd {
    pub fn raw_fd(&self) -> RawFd {
        self.fd.as_raw_fd()
    }
    /// A shared reference to the fd. The keymap is sent to every client: the
    /// sendmsg holds such a reference while the message is in flight, and the
    /// original stays open with the owner.
    pub fn rc(&self) -> Rc<OwnedFd> {
        Rc::clone(&self.fd)
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
            libc::memfd_create(c"tema-anon".as_ptr(), libc::MFD_CLOEXEC | libc::MFD_ALLOW_SEALING)
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

    Ok(MemFd { fd: Rc::new(fd), size })
}
