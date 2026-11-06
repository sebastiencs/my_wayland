use std::{
    io,
    os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd},
    sync::atomic::AtomicUsize,
};

use crate::drm::{Format, Modifier};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct DmaBufId(usize);

impl DmaBufId {
    pub fn next() -> Self {
        static ID: AtomicUsize = AtomicUsize::new(0);
        let next = ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Self(next)
    }
}

#[derive(Debug)]
pub struct DmaBufPlane {
    pub offset: u32,
    pub stride: u32,
    /// Owned `dma-buf` plane fd from GBM (closed on drop).
    pub fd: OwnedFd,
}

#[derive(Debug)]
pub struct DmaBuf {
    pub id: DmaBufId,
    pub width: i32,
    pub height: i32,
    pub format: &'static Format,
    pub modifier: Modifier,
    pub planes: Box<[DmaBufPlane]>,
}

/// `DMA_BUF_SYNC_READ` from `linux/dma-buf.h`. A read fence blocks later writers.
pub const DMA_BUF_SYNC_READ: u32 = 1;
/// `DMA_BUF_SYNC_WRITE` from `linux/dma-buf.h`. A write fence blocks every later user.
pub const DMA_BUF_SYNC_WRITE: u32 = 2;

#[repr(C)]
struct DmaBufSyncFile {
    flags: u32,
    fd: i32,
}

// `_IOWR('b', 2, struct dma_buf_export_sync_file)` and `_IOW('b', 3, ...)`.
// wlroots wraps the same ioctls in `render/dmabuf_linux.c`.
nix::ioctl_readwrite!(dma_buf_export_sync_file, b'b', 2, DmaBufSyncFile);
nix::ioctl_write_ptr!(dma_buf_import_sync_file, b'b', 3, DmaBufSyncFile);

/// Snapshot of the dma-buf's current fences. The kernel returns a signaled file
/// when nothing is pending.
pub fn export_sync_file(dmabuf: &impl AsFd, flags: u32) -> io::Result<OwnedFd> {
    let mut data = DmaBufSyncFile { flags, fd: -1 };
    unsafe { dma_buf_export_sync_file(dmabuf.as_fd().as_raw_fd(), &mut data) }
        .map_err(io::Error::from)?;
    if data.fd < 0 {
        return Err(io::Error::other("dma-buf exported no sync file"));
    }
    Ok(unsafe { OwnedFd::from_raw_fd(data.fd) })
}

/// Insert `sync_file` into the dma-buf. The ioctl does not take ownership of it,
/// so one file can be imported onto every plane. wlroots does the same.
pub fn import_sync_file(dmabuf: &impl AsFd, flags: u32, sync_file: &impl AsFd) -> io::Result<()> {
    let data = DmaBufSyncFile { flags, fd: sync_file.as_fd().as_raw_fd() };
    unsafe { dma_buf_import_sync_file(dmabuf.as_fd().as_raw_fd(), &data) }
        .map(|_| ())
        .map_err(io::Error::from)
}
