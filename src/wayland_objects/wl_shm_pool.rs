use std::{
    os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd},
    ptr::NonNull,
    rc::Rc,
};

use crate::{
    clients::{Client, ClientDispatcher, RawRequest},
    object::{Object, Parser},
    protocol_generated::{wl_shm, wl_shm_pool, WaylandId, WlShmPoolId},
    state::State,
    wayland_objects::{
        wl_buffer::{NewWlBufferParams, WlBuffer},
        wl_display::WlDisplay,
        wl_shm::SHM_FORMATS,
    },
};

#[derive(Debug)]
pub struct WlShmPool {
    id: WlShmPoolId,
    fd: OwnedFd,
    size: usize,
    mem: Rc<SharedMemory>,
}

impl WlShmPool {
    pub fn new(id: WlShmPoolId, fd: RawFd, size: usize) -> Self {
        let fd = unsafe { OwnedFd::from_raw_fd(fd) };
        let mem = SharedMemory::mmap(fd.as_raw_fd(), size);
        Self { id, fd, size, mem }
    }
}

impl Object for WlShmPool {
    fn id(&self) -> WaylandId {
        self.id.into()
    }

    fn on(
        &mut self,
        client: &mut Client,
        state: &mut State,
        req: RawRequest,
        dispatcher: &mut ClientDispatcher,
    ) {
        use wl_shm_pool::Request::*;

        match client.parse::<Self>(req).unwrap() {
            CreateBuffer { id, offset, width, height, stride, format } => {
                let Some(format) = SHM_FORMATS.iter().find(|f| f.wayland_id == format).copied()
                else {
                    client.send_protocol_error(
                        dispatcher,
                        self.id(),
                        wl_shm::Error::InvalidFormat as u32,
                        "invalid SHM buffer format",
                    );
                    return;
                };
                let mem = self.mem.clone();
                let wl_buffer = match WlBuffer::new(NewWlBufferParams {
                    id,
                    offset,
                    width,
                    height,
                    stride,
                    format,
                    mem,
                }) {
                    Ok(b) => b,
                    Err(_) => {
                        client.send_protocol_error(
                            dispatcher,
                            self.id(),
                            wl_shm::Error::InvalidStride as u32,
                            "invalid SHM buffer parameters",
                        );
                        return;
                    }
                };
                client.insert_object(state, wl_buffer);
            }
            Destroy => {
                client.delete_id(&WlDisplay::default(), state, dispatcher, self.id.into());
            }
            Resize { size } => todo!(),
        }
    }
}

#[derive(Debug)]
pub struct SharedMemory {
    ptr: NonNull<u8>,
    length: usize,
}

impl Drop for SharedMemory {
    fn drop(&mut self) {
        if let Err(e) = sys::munmap(self.ptr, self.length) {
            log::error!("Failed to unmap SharedMemory: {:?}", e);
        }
    }
}

impl SharedMemory {
    fn mmap(fd: RawFd, size: usize) -> Rc<Self> {
        let ptr = sys::mmap(fd, size).unwrap();
        Rc::new(Self { ptr, length: size })
    }

    pub fn slice(self: Rc<Self>) -> SliceSharedMemory {
        let length = self.length;
        self.slice_at(0, length)
    }

    pub fn slice_at(self: Rc<Self>, offset: usize, length: usize) -> SliceSharedMemory {
        assert!(offset + length <= self.length);
        SliceSharedMemory { mem: self.clone(), offset, length }
    }
}

#[derive(Debug)]
pub struct SliceSharedMemory {
    mem: Rc<SharedMemory>,
    offset: usize,
    length: usize,
}

impl SliceSharedMemory {
    pub fn as_ptr(&self) -> *mut u8 {
        self.ptr().as_ptr()
    }

    pub fn ptr(&self) -> NonNull<u8> {
        unsafe { self.mem.ptr.byte_add(self.offset) }
    }

    fn as_slice(&self) -> &[u8] {
        unsafe { NonNull::slice_from_raw_parts(self.ptr(), self.length).as_ref() }
    }
}

pub mod sys {
    use super::*;

    pub fn mmap(fd: RawFd, size: usize) -> std::io::Result<NonNull<u8>> {
        let ptr = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                size,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                fd,
                0,
            )
        };

        if ptr == libc::MAP_FAILED {
            return Err(std::io::Error::last_os_error());
        }

        Ok(NonNull::new(ptr as *mut _).unwrap())
    }

    pub fn munmap<T>(ptr: NonNull<T>, size: usize) -> std::io::Result<()> {
        let res = unsafe { libc::munmap(ptr.as_ptr() as *mut core::ffi::c_void, size) };

        if res == -1 {
            return Err(std::io::Error::last_os_error());
        }

        Ok(())
    }
}
