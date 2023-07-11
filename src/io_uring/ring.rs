use std::{
    collections::BTreeMap,
    fmt::Debug,
    os::unix::io::RawFd,
    ptr::NonNull,
    sync::atomic::{
        AtomicU32,
        Ordering::{Acquire, Relaxed, Release},
    },
};

use {
    libc::c_uint,
    log::{info, warn},
};

use libc::msghdr;

use crate::{io_uring::types::IORING_OP_RECVMSG, state::Redispatcher};

use super::{
    syscalls::{io_uring_enter, io_uring_register, io_uring_setup, mmap, munmap},
    types::{
        io_uring_files_update, io_uring_params, io_uring_probe, CompletionQueueEntry, EnterFlags,
        FeaturesFlags, IoUringFd, SubmissionQueueEntry, IORING_OFF_CQ_RING, IORING_OFF_SQES,
        IORING_OFF_SQ_RING, IORING_OP_ACCEPT, IORING_OP_NOP, IORING_OP_READ, IORING_OP_WRITE,
        IORING_REGISTER_FILES, IORING_REGISTER_FILES_UPDATE, IORING_REGISTER_PROBE,
        IORING_UNREGISTER_FILES,
    },
};

fn display_io_uring_features(features: u32) {
    let flags = FeaturesFlags::from_bits_truncate(features);

    info!("[io_uring] Available features: {:?}", flags);
}

#[derive(Debug, Copy, Clone, Hash, Eq, PartialEq, PartialOrd, Ord)]
pub struct RingId(pub u32);

impl RingId {
    pub fn to_user_data(self) -> u64 {
        self.0 as u64
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) enum ReadOrWrite {
    Read,
    Write,
}

#[derive(Debug)]
pub struct ReadWriteCompleted {
    pub fd: RawFd,
    pub offset: usize,
    pub buffer: Box<[u8]>,
}

#[derive(Debug)]
pub struct RecvmsgCompleted {
    pub fd: RawFd,
    pub buffer: Box<[u8]>,
}

#[derive(Debug)]
pub struct AcceptCompleted {
    pub fd: RawFd,
    pub new_client: RawFd,
}

pub struct RecvmsgData {
    pub buffer_read: Vec<u8>,
    pub buffer_aux: Vec<u8>,
    pub iovec: libc::iovec,
    pub msghdr: msghdr,
}

impl Debug for RecvmsgData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RecvmsgData")
            .field("ncontrol", &self.msghdr.msg_controllen)
            .field("flags", &self.msghdr.msg_flags)
            // .field("buffer_read_len", &self.buffer_read.len())
            // .field("buffer_aux_len", &self.buffer_aux.len())
            // .field("iovec", &self.iovec)
            // .field("msghdr", &self.msghdr)
            .finish()
    }
}

#[derive(Debug)]
pub(super) enum Operation {
    // ReadFixed {
    //     fd: RawFd,
    //     offset: usize,
    //     length: usize,
    //     buf_index: u16,
    // },
    // RegisterFiles {
    //     id: RingId,
    //     files: &'a [i32],
    //     link_next: bool,
    // },
    ReadWrite {
        id: RingId,
        fd: RawFd,
        offset: usize,
        buffer: Box<[u8]>,
        kind: ReadOrWrite,
        /// number of bytes read/written
        nbytes_processed: usize,
        /// Number of EOF (res == 0) received
        /// Consider EOF reached after 2
        /// https://github.com/facebook/rocksdb/pull/6441#issuecomment-589843435
        neof: u8,
        dispatch_to: Redispatcher<std::io::Result<ReadWriteCompleted>>,
        // fd: FileDescriptor,
        // buffer_ptr: *const u8,
        // length: usize,
        // data: &'a mut [u8],
    },
    Recvmsg {
        id: RingId,
        fd: RawFd,
        data: Box<RecvmsgData>,
        dispatch_to: Redispatcher<std::io::Result<RecvmsgCompleted>>,
    },
    Accept {
        id: RingId,
        fd: RawFd,
        dispatch_to: Redispatcher<std::io::Result<AcceptCompleted>>,
    },

    // fd: RawFd,
    // data: Box<[u8]>,
    // // data: NonNull<u8>,
    // data_length: usize,
    // /// number of bytes read/written
    // nbytes_processed: usize,
    // offset: usize,
    // /// Number of EOF (res == 0) received
    // /// Consider EOF reached after 2
    // /// https://github.com/facebook/rocksdb/pull/6441#issuecomment-589843435
    // neof: u8,
    // kind: ReadOrWrite,
    // dispatch_to: Redispatcher<std::io::Result<ReadWriteCompleted>>,

    // Write {
    //     id: RingId,
    //     fd: RawFd,
    //     offset: usize,
    //     buffer_ptr: *const u8,
    //     length: usize,
    // },
    // OpenAt {
    //     path: &'a Path,
    // },
    // TimeOut {
    //     spec: &'a mut timespec,
    // },
    NoOp,
}

impl Operation {
    fn id(&self) -> Option<RingId> {
        match self {
            Operation::ReadWrite { id, .. } => Some(*id),
            Operation::NoOp => None,
            Operation::Recvmsg { id, .. } => Some(*id),
            Operation::Accept { id, .. } => Some(*id),
        }
    }
}

impl SubmissionQueueEntry {
    fn apply_operation(&mut self, op: &Operation) {
        // TODO: Remove this
        *self = Self::default();

        match op {
            // Operation::RegisterFiles {
            //     id,
            //     files,
            //     link_next,
            // } => {
            //     self.opcode = IORING_OP_FILES_UPDATE;
            //     self.fd = -1;
            //     self.addr_splice_off_in = files.as_ptr() as u64;
            //     self.off_addr2 = 0;
            //     self.len = files.len() as u32;
            //     self.user_data = id.into();
            //     self.sqe_flags.set(SqeFlags::IOSQE_IO_LINK, link_next);
            //     //self.flags.rw_flags.set(RWFlags::IOSQE_IO_LINK, link_next);
            // }
            // Operation::ReadFixed {
            //     fd,
            //     offset,
            //     length,
            //     buf_index,
            // } => {
            //     self.fd = fd;
            //     self.opcode = IORING_OP_READ_FIXED;
            //     self.off_addr2 = offset as u64;
            //     // self.addr_splice_off_in = iovecs.as_ptr() as u64;
            //     self.buf_index = buf_index;
            //     self.len = length as u32;
            // }
            Operation::ReadWrite {
                id,
                fd,
                offset,
                // buffer: data,
                // buffer_ptr,
                // length,
                kind,
                buffer,
                nbytes_processed,
                neof: _,
                dispatch_to: _,
            } => {
                self.opcode = match kind {
                    ReadOrWrite::Read => IORING_OP_READ,
                    ReadOrWrite::Write => IORING_OP_WRITE,
                };
                self.fd = *fd;
                self.off_addr2 = (offset + nbytes_processed) as u64;
                self.addr_splice_off_in = unsafe { buffer.as_ptr().add(*nbytes_processed) } as u64;
                self.len = (buffer.len() - nbytes_processed) as u32;
                self.user_data = id.to_user_data();
            }
            // Operation::OpenAt { path } => {
            //     let path = path.to_str().unwrap().as_ptr();
            //     let flags = libc::O_CREAT | libc::O_RDWR;
            //     let mode = libc::S_IRUSR | libc::S_IWUSR | libc::S_IRGRP | libc::S_IROTH;

            //     self.opcode = IORING_OP_OPENAT;
            //     self.fd = libc::AT_FDCWD;
            //     self.addr_splice_off_in = path as u64;
            //     self.flags = flags as u32;
            //     //self.flags.open_flags = flags as u32;
            //     self.len = mode;
            // }
            // Operation::TimeOut { spec } => {
            //     self.opcode = IORING_OP_TIMEOUT;
            //     self.addr_splice_off_in = spec as *mut _ as u64;
            //     self.len = 1;
            // }
            Operation::NoOp => {
                self.opcode = IORING_OP_NOP;
            }
            Operation::Recvmsg {
                id,
                fd,
                data,
                dispatch_to: _,
            } => {
                self.opcode = IORING_OP_RECVMSG;
                self.fd = *fd;
                self.addr_splice_off_in = &data.msghdr as *const _ as u64;
                self.flags = libc::MSG_CMSG_CLOEXEC as u32;
                self.user_data = id.to_user_data();
            }
            Operation::Accept {
                id,
                fd,
                dispatch_to: _,
            } => {
                self.opcode = IORING_OP_ACCEPT;
                self.fd = *fd;
                // self.addr_splice_off_in = 0;
                // self.off_addr2 = 0;
                self.flags = libc::SOCK_CLOEXEC as u32;
                self.user_data = id.to_user_data();
            }
        }
    }
}

#[derive(Debug)]
pub struct Completed {
    pub user_data: u32,
    pub id: RingId,
    pub result: i32,
}

impl From<&CompletionQueueEntry> for Completed {
    fn from(e: &CompletionQueueEntry) -> Self {
        Self {
            user_data: (e.user_data >> 32) as u32,
            id: RingId((e.user_data & 0xFFFF) as u32),
            result: e.res,
        }
    }
}

pub enum Submit {
    All,
    None,
    N(u32),
}

impl From<u32> for Submit {
    fn from(n: u32) -> Self {
        Submit::N(n)
    }
}

#[derive(Debug)]
pub struct IoUring {
    fd: IoUringFd,
    params: io_uring_params,

    sq_ptr: NonNull<u8>,
    cq_ptr: NonNull<u8>,
    sq_size: u32,
    cq_size: u32,
    sqes_size: u32,

    pub(super) sq_entries: u32,
    sq_mask: u32,
    sq_tail_local: u32,
    sq_head: NonNull<AtomicU32>,
    sq_tail: NonNull<AtomicU32>,
    sqes_ptr: NonNull<SubmissionQueueEntry>,

    pub(super) cq_entries: u32,
    cq_mask: u32,
    cq_head: NonNull<AtomicU32>,
    cq_tail: NonNull<AtomicU32>,
    cqes_ptr: NonNull<CompletionQueueEntry>,
}

unsafe impl Send for IoUring {}

impl IoUring {
    pub fn new(len: u32) -> std::io::Result<Self> {
        let mut params = io_uring_params::default();
        let io_ring_fd = io_uring_setup(len, &mut params)?;

        display_io_uring_features(params.features);

        if !Self::is_ops_supported(io_ring_fd)? {
            warn!("Kernel doesn't support our io_uring operations");
            return Err(std::io::ErrorKind::Other.into());
        }

        let features = FeaturesFlags::from_bits_truncate(params.features);
        let has_single_mmap = features.contains(FeaturesFlags::IORING_FEAT_SINGLE_MMAP);

        assert!(features.contains(FeaturesFlags::IORING_FEAT_NODROP));

        let mut sq_ring_size =
            params.sq_off.array + params.sq_entries * std::mem::size_of::<c_uint>() as u32;
        let mut cq_ring_size = params.cq_off.cqes
            + params.cq_entries * std::mem::size_of::<CompletionQueueEntry>() as u32;

        if has_single_mmap {
            if cq_ring_size > sq_ring_size {
                sq_ring_size = cq_ring_size;
            }
            cq_ring_size = sq_ring_size;
        }

        let sq_ring_ptr = mmap(&io_ring_fd, sq_ring_size, IORING_OFF_SQ_RING)?;

        unsafe {
            // Let's write `array` values now.
            // Each value is an index in `sqes`.
            // Our implementation won't modify them and the kernel too.
            // https://github.com/torvalds/linux/blob/71c5f03154ac1cb27423b984743ccc2f5d11d14d/fs/io_uring.c#L274
            let array_ptr = (sq_ring_ptr.as_ptr() as *mut u8).add(params.sq_off.array as usize);
            let array_ptr = array_ptr as *mut u32;

            for index in 0..params.sq_entries {
                array_ptr.add(index as usize).write(index);
            }
        }

        let cq_ring_ptr = if has_single_mmap {
            sq_ring_ptr
        } else {
            match mmap(&io_ring_fd, cq_ring_size, IORING_OFF_CQ_RING) {
                Ok(ptr) => ptr,
                Err(e) => {
                    munmap(sq_ring_ptr, sq_ring_size).unwrap();
                    return Err(e);
                }
            }
        };

        let sqes_size = params.sq_entries * std::mem::size_of::<SubmissionQueueEntry>() as u32;
        let sqes = match mmap(&io_ring_fd, sqes_size, IORING_OFF_SQES) {
            Ok(ptr) => ptr,
            Err(e) => {
                munmap(sq_ring_ptr, sq_ring_size).unwrap();
                if sq_ring_ptr != cq_ring_ptr {
                    munmap(cq_ring_ptr, cq_ring_size).unwrap();
                }
                return Err(e);
            }
        };

        let (sq_head, sq_tail, sq_entries, sq_mask, sq_tail_local) = unsafe {
            let sq_ptr = sq_ring_ptr.as_ptr() as *const u8;
            let head = sq_ptr.add(params.sq_off.head as usize) as *mut AtomicU32;
            let tail = sq_ptr.add(params.sq_off.tail as usize) as *mut AtomicU32;
            let entries = *(sq_ptr.add(params.sq_off.ring_entries as usize) as *const u32);
            let mask = *(sq_ptr.add(params.sq_off.ring_mask as usize) as *const u32);
            let tail_local = (*tail).load(Relaxed);

            (head, tail, entries, mask, tail_local)
        };

        let (cq_head, cq_tail, cq_entries, cq_mask, cqes_ptr) = unsafe {
            let cq_ptr = cq_ring_ptr.as_ptr() as *const u8;
            let head = cq_ptr.add(params.cq_off.head as usize) as *mut AtomicU32;
            let tail = cq_ptr.add(params.cq_off.tail as usize) as *mut AtomicU32;
            let entries = *(cq_ptr.add(params.cq_off.ring_entries as usize) as *const u32);
            let mask = *(cq_ptr.add(params.cq_off.ring_mask as usize) as *const u32);
            let cqes_ptr = cq_ptr.add(params.cq_off.cqes as usize) as *mut CompletionQueueEntry;

            (head, tail, entries, mask, cqes_ptr)
        };

        Ok(IoUring {
            fd: io_ring_fd,
            sq_ptr: sq_ring_ptr,
            cq_ptr: cq_ring_ptr,
            sq_size: sq_ring_size,
            cq_size: cq_ring_size,
            sqes_ptr: sqes,
            sqes_size,
            params,
            sq_mask,
            sq_entries,
            sq_tail_local,
            sq_tail: NonNull::new(sq_tail).unwrap(),
            sq_head: NonNull::new(sq_head).unwrap(),
            cq_entries,
            cq_mask,
            cq_tail: NonNull::new(cq_tail).unwrap(),
            cq_head: NonNull::new(cq_head).unwrap(),
            cqes_ptr: NonNull::new(cqes_ptr).unwrap(),
        })
    }

    fn is_ops_supported(fd: IoUringFd) -> std::io::Result<bool> {
        let mut probe = io_uring_probe::default();
        io_uring_register(
            fd,
            IORING_REGISTER_PROBE,
            &mut probe as *mut io_uring_probe as *mut _,
            0,
        )?;

        Ok(probe.last_op >= IORING_OP_WRITE)
    }

    pub(super) fn features(&self) -> FeaturesFlags {
        FeaturesFlags::from_bits_truncate(self.params.features)
    }

    pub(super) fn register_files(&self, files: &[i32]) -> std::io::Result<()> {
        io_uring_register(
            self.fd,
            IORING_REGISTER_FILES,
            files.as_ptr() as *mut _,
            files.len() as u32,
        )
    }

    /// Update a single entry
    pub(super) fn register_file_update(&self, files: &[i32], offset: usize) -> std::io::Result<()> {
        let update = io_uring_files_update {
            offset: offset as u32,
            resv: 0,
            fds: files.as_ptr() as u64,
        };
        io_uring_register(
            self.fd,
            IORING_REGISTER_FILES_UPDATE,
            &update as *const _ as *mut _,
            files.len() as u32,
        )
    }

    pub(super) fn register_files_update(&self, files: &[i32]) -> std::io::Result<()> {
        let update = io_uring_files_update {
            offset: 0,
            resv: 0,
            fds: files.as_ptr() as u64,
        };
        io_uring_register(
            self.fd,
            IORING_REGISTER_FILES_UPDATE,
            &update as *const _ as *mut _,
            files.len() as u32,
        )
    }

    pub(super) fn unregister_files(&self) -> std::io::Result<()> {
        io_uring_register(self.fd, IORING_UNREGISTER_FILES, std::ptr::null_mut(), 0)
    }

    fn sq_refs(&self) -> (&AtomicU32, &AtomicU32) {
        unsafe { (&*self.sq_head.as_ptr(), &*self.sq_tail.as_ptr()) }
    }

    fn cq_refs(&self) -> (&AtomicU32, &AtomicU32) {
        unsafe { (&*self.cq_head.as_ptr(), &*self.cq_tail.as_ptr()) }
    }

    /// Number of entries pushed by us, waiting to be read by the kernel
    pub(super) fn sq_pending(&self) -> u32 {
        let (head_ref, _) = self.sq_refs();

        let head = head_ref.load(Acquire);
        let tail = self.sq_tail_local;

        tail.wrapping_sub(head)
    }

    pub(super) fn push_entry(&mut self, op: Operation) -> Result<(), Operation> {
        let (head_ref, _) = self.sq_refs();

        let head = head_ref.load(Acquire);
        let tail = self.sq_tail_local;

        if tail == head.wrapping_add(self.sq_entries) {
            return Err(op);
        }

        let index = tail & self.sq_mask;
        assert!(index < self.sq_entries);

        // Safety:
        // The index doesn't go past sq_entries
        let entry = unsafe {
            let sqes = self.sqes_ptr.as_ptr() as *mut SubmissionQueueEntry;
            &mut *sqes.add(index as usize)
        };
        entry.apply_operation(&op);

        self.sq_tail_local = tail.wrapping_add(1);

        Ok(())
    }

    pub(super) fn push_entries<F>(
        &mut self,
        mut fun: F,
        pending: &mut BTreeMap<RingId, Operation>,
    ) -> usize
    where
        F: FnMut() -> Option<Operation>,
    {
        let (head_ref, _) = self.sq_refs();

        let head = head_ref.load(Acquire);
        let mut tail = self.sq_tail_local;
        let mask = self.sq_mask;
        let end = head.wrapping_add(self.sq_entries);

        let mut npushed = 0;

        while tail != end {
            let Some(op) = fun() else { break };

            // Safety:
            // The index doesn't go past sq_entries
            assert!((tail & mask) < self.sq_entries);
            let entry = unsafe {
                let sqes = self.sqes_ptr.as_ptr() as *mut SubmissionQueueEntry;
                &mut *sqes.add((tail & mask) as usize)
            };
            entry.apply_operation(&op);

            if let Some(id) = op.id() {
                pending.insert(id, op);
            };

            npushed += 1;
            tail = tail.wrapping_add(1);
        }

        self.sq_tail_local = tail;

        npushed
    }

    pub(super) fn submit<S: Into<Submit>>(&self, n: S) -> std::io::Result<()> {
        let (head_ref, tail_ref) = self.sq_refs();
        let submit = n.into();

        let head = head_ref.load(Acquire);
        let tail = self.sq_tail_local;

        // These might wrap around
        let pending = tail.wrapping_sub(head);

        let to_submit = match submit {
            Submit::All => pending,
            Submit::N(n) => n.min(pending),
            Submit::None => 0,
        };

        dbg!(to_submit);

        if to_submit > 0 {
            tail_ref.store(tail, Release);
            io_uring_enter(self.fd, to_submit, 0, EnterFlags::empty())?;
        }

        Ok(())
    }

    pub(super) fn pop(&self) -> Option<Completed> {
        let (head_ref, tail_ref) = self.cq_refs();

        let tail = tail_ref.load(Acquire);
        let head = head_ref.load(Relaxed);

        if tail == head {
            return None;
        }

        let index = head & self.cq_mask;
        assert!(index < self.cq_entries);

        // Safety:
        // The index doesn't go past cq_entries
        let entry = unsafe {
            let cqes = self.cqes_ptr.as_ptr() as *const CompletionQueueEntry;
            Completed::from(&*cqes.add(index as usize))
        };

        // Atomic: The entry must be read before modifying the head
        head_ref.store(head.wrapping_add(1), Release);

        Some(entry)
    }

    /// Wait for completion, and submit at the same time
    #[allow(clippy::unnecessary_unwrap)]
    pub(super) fn wait_pop(&self) -> std::io::Result<Completed> {
        loop {
            let completed = self.pop();
            let nto_submit = self.sq_pending();

            if nto_submit == 0 && completed.is_some() {
                return Ok(completed.unwrap());
            }

            let (min_complete, enter_flags) = match completed.is_some() {
                true => (0, EnterFlags::empty()),
                false => (1, EnterFlags::IORING_ENTER_GETEVENTS),
            };

            if nto_submit > 0 {
                let (_head_ref, tail_ref) = self.sq_refs();
                tail_ref.store(self.sq_tail_local, Release);
            }

            io_uring_enter(self.fd, nto_submit, min_complete, enter_flags)?;

            if let Some(completed) = completed {
                return Ok(completed);
            };
        }
    }

    /// Number of entries pushed by the kernel, but not yet read by us
    pub(super) fn cq_pending(&self) -> u32 {
        let (head_ref, tail_ref) = self.cq_refs();

        let tail = tail_ref.load(Acquire);
        let head = head_ref.load(Relaxed);

        tail.wrapping_sub(head)
    }
}

impl Drop for IoUring {
    fn drop(&mut self) {
        // TODO: Make sure the completion queue is empty before unmapping
        // Or the kernel will work on unmapped memory
        munmap(self.sqes_ptr, self.sqes_size).unwrap();
        munmap(self.sq_ptr, self.sq_size).unwrap();
        if self.sq_ptr != self.cq_ptr {
            munmap(self.cq_ptr, self.cq_size).unwrap();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::iter::repeat_with;

    use crate::io_uring::types::timespec;

    use super::{IoUring, Operation, Submit};

    #[test]
    fn simple() {
        let mut iou = match IoUring::new(4) {
            Ok(iou) => iou,
            Err(_) => return, // not supported
        };

        iou.push_entry(Operation::NoOp).unwrap();
        iou.push_entry(Operation::NoOp).unwrap();
        iou.push_entry(Operation::NoOp).unwrap();
        iou.push_entry(Operation::NoOp).unwrap();
        iou.push_entry(Operation::NoOp).unwrap_err();

        iou.submit(Submit::All).unwrap();

        iou.push_entry(Operation::NoOp).unwrap();
        iou.push_entry(Operation::NoOp).unwrap();

        iou.submit(Submit::All).unwrap();

        iou.push_entry(Operation::NoOp).unwrap();
        iou.push_entry(Operation::NoOp).unwrap();
        iou.push_entry(Operation::NoOp).unwrap();
        iou.push_entry(Operation::NoOp).unwrap();
        iou.push_entry(Operation::NoOp).unwrap_err();
    }

    #[test]
    fn multiple() {
        let mut iou = match IoUring::new(4) {
            Ok(iou) => iou,
            Err(_) => return, // not supported
        };

        let mut pending = Default::default();

        let mut iter = repeat_with(|| Operation::NoOp).take(4);
        let n = iou.push_entries(|| iter.next(), &mut pending);
        assert_eq!(n, 4);

        let n = iou.push_entries(|| Some(Operation::NoOp), &mut pending);
        assert_eq!(n, 0);
    }

    #[test]
    fn pop() {
        let mut iou = match IoUring::new(4) {
            Ok(iou) => iou,
            Err(_) => return, // not supported
        };

        let mut pending = Default::default();

        let mut iter = repeat_with(|| Operation::NoOp).take(4);
        let n = iou.push_entries(|| iter.next(), &mut pending);
        assert_eq!(n, 4);
        iou.submit(Submit::All).unwrap();

        assert!(iou.cq_pending() > 0);

        iou.pop().unwrap();
        iou.pop().unwrap();
        iou.pop().unwrap();
        iou.pop().unwrap();
        assert!(iou.pop().is_none());

        let _spec = timespec {
            tv_sec: 0,
            tv_nsec: 100000000, // 0,1 sec
        };

        // iou.push_entry(Operation::TimeOut { spec: &mut spec })
        //     .unwrap();
        // iou.submit(Submit::All).unwrap();

        // assert!(iou.pop().is_none());

        // let val = iou.wait_pop().unwrap();
        // assert_eq!(val.result, -libc::ETIME);

        // iou.push_entry(Operation::NoOp).unwrap();
        // iou.push_entry(Operation::NoOp).unwrap();
        // iou.submit(Submit::All).unwrap();
        // iou.submit(Submit::All).unwrap();

        // std::thread::sleep(std::time::Duration::from_millis(100));

        // let _ = iou.wait_pop().unwrap();
        // let val = iou.wait_pop().unwrap();

        // println!("val={:?}", val);
        // println!("iou={:#?}", iou);
    }

    // #[test]
    // fn register_files() {
    //     let mut iou = match IoUring::new(4) {
    //         Ok(iou) => iou,
    //         Err(_) => return, // not supported
    //     };

    //     let temp_file = mktemp::Temp::new_file().unwrap();
    //     let fd = std::fs::OpenOptions::new()
    //         .write(true)
    //         .create(true)
    //         .read(true)
    //         .open(&temp_file)
    //         .unwrap();

    //     let registered_files = vec![-1; 512];
    //     let err = iou.register_files(&registered_files);

    //     if err.is_err() {
    //         return;
    //     }

    //     let files = vec![fd.as_raw_fd()];

    //     iou.push_entry(Operation::RegisterFiles {
    //         id: 101.into(),
    //         files: files.as_slice(),
    //         link_next: false,
    //     })
    //     .unwrap();
    //     iou.submit(Submit::All).unwrap();

    //     let completed = iou.wait_pop().unwrap();

    //     assert_eq!(completed.result, 1);

    //     println!("completed={:?}", completed);
    // }
}
