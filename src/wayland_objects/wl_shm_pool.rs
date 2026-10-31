use std::{
    os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd},
    ptr::NonNull,
    rc::Rc,
};

use crate::{
    clients::{Client, ClientDispatcher, RawRequest},
    object::{Object, Parser, RequestResult},
    protocol_generated::{wl_shm, wl_shm_pool, WaylandId, WlShmPoolId},
    state::{GrabbedObject, State},
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

    fn validate_create_buffer(
        &self,
        offset: i32,
        width: i32,
        height: i32,
        stride: i32,
        format_bpp: usize,
    ) -> Result<(), ()> {
        let offset = usize::try_from(offset).map_err(|_| ())?;
        let width = usize::try_from(width).map_err(|_| ())?;
        let height = usize::try_from(height).map_err(|_| ())?;
        let stride = usize::try_from(stride).map_err(|_| ())?;

        if width == 0 || height == 0 || stride == 0 {
            return Err(());
        }

        let min_stride = width.checked_mul(format_bpp).ok_or(())?;
        if stride < min_stride {
            return Err(());
        }

        let bytes = stride.checked_mul(height).ok_or(())?;
        let end = offset.checked_add(bytes).ok_or(())?;
        if end > self.size {
            return Err(());
        }

        Ok(())
    }
}

impl Object for WlShmPool {
    fn id(&self) -> WaylandId {
        self.id.into()
    }

    fn on(
        mut self: GrabbedObject<Self>,
        client: &mut Client,
        state: &mut State,
        req: RawRequest,
        dispatcher: &mut ClientDispatcher,
    ) -> RequestResult {
        use wl_shm_pool::Request::*;

        match client.parse::<Self>(req, dispatcher)? {
            CreateBuffer { id, offset, width, height, stride, format } => {
                let Some(format) = SHM_FORMATS.iter().find(|f| f.wayland_id == format).copied()
                else {
                    client.send_protocol_error(
                        dispatcher,
                        self.id(),
                        wl_shm::Error::InvalidFormat as u32,
                        "invalid SHM buffer format",
                    );
                    return Ok(());
                };
                if self
                    .validate_create_buffer(offset, width, height, stride, format.bpp as usize)
                    .is_err()
                {
                    client.send_protocol_error(
                        dispatcher,
                        self.id(),
                        wl_shm::Error::InvalidStride as u32,
                        "invalid SHM buffer parameters",
                    );
                    return Ok(());
                }
                let mem = self.mem.clone();
                let wl_buffer = match WlBuffer::new(NewWlBufferParams {
                    id,
                    owner_client_id: client.client_id(),
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
                        return Ok(());
                    }
                };
                client.insert_object(state, wl_buffer);
            }
            Destroy => {
                client.delete_id(&WlDisplay::default(), state, dispatcher, self.id.into());
            }
            Resize { size } => {
                if size <= 0 {
                    client.send_protocol_error(
                        dispatcher,
                        self.id(),
                        wl_shm::Error::InvalidStride as u32,
                        "invalid SHM pool size",
                    );
                    return Ok(());
                }

                let size = usize::try_from(size).unwrap();
                if size < self.size {
                    client.send_protocol_error(
                        dispatcher,
                        self.id(),
                        wl_shm::Error::InvalidStride as u32,
                        "shrinking a SHM pool is forbidden",
                    );
                    return Ok(());
                }

                if size == self.size {
                    return Ok(());
                }

                match SharedMemory::try_mmap(self.fd.as_raw_fd(), size) {
                    Ok(mem) => {
                        self.mem = mem;
                        self.size = size;
                    }
                    Err(err) => {
                        log::warn!("failed to resize SHM pool {:?}: {:?}", self.id, err);
                        client.send_protocol_error(
                            dispatcher,
                            self.id(),
                            wl_shm::Error::InvalidFd as u32,
                            "failed to resize SHM pool mapping",
                        );
                    }
                }
            }
        }
        Ok(())
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
        Self::try_mmap(fd, size).unwrap()
    }

    fn try_mmap(fd: RawFd, size: usize) -> std::io::Result<Rc<Self>> {
        let ptr = sys::mmap(fd, size)?;
        Ok(Rc::new(Self { ptr, length: size }))
    }

    pub fn slice(self: Rc<Self>) -> SliceSharedMemory {
        let length = self.length;
        self.slice_at(0, length)
    }

    pub fn slice_at(self: Rc<Self>, offset: usize, length: usize) -> SliceSharedMemory {
        assert!(offset
            .checked_add(length)
            .is_some_and(|end| end <= self.length));
        SliceSharedMemory { mem: self.clone(), offset, length }
    }
}

#[cfg(test)]
pub(crate) fn shared_memory_from_bytes(bytes: &[u8]) -> Rc<SharedMemory> {
    let size = bytes.len().max(1);
    let name = std::ffi::CString::new("cursor-test-shm").unwrap();
    let raw_fd = unsafe { libc::memfd_create(name.as_ptr(), libc::MFD_CLOEXEC) };
    assert!(raw_fd >= 0, "memfd_create failed: {:?}", std::io::Error::last_os_error());
    let fd = unsafe { OwnedFd::from_raw_fd(raw_fd) };
    let truncate_result = unsafe { libc::ftruncate(fd.as_raw_fd(), size as libc::off_t) };
    assert_eq!(truncate_result, 0, "ftruncate failed: {:?}", std::io::Error::last_os_error());
    let write_result = unsafe { libc::write(fd.as_raw_fd(), bytes.as_ptr().cast(), bytes.len()) };
    assert!(write_result >= 0, "write failed: {:?}", std::io::Error::last_os_error());
    SharedMemory::try_mmap(fd.as_raw_fd(), size).unwrap()
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

    pub(crate) fn as_slice(&self) -> &[u8] {
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
