use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    io::ErrorKind::UnexpectedEof,
    ops::Range,
    os::fd::{FromRawFd, OwnedFd, RawFd},
};

use {libc::cmsghdr, log::error};

use crate::{
    io_uring::ring::{Operation, PollCompleted, SendmsgData},
    runner::{Dispatcher, Model, Redispatcher},
    state::State,
};

use super::{
    ring::{
        AcceptCompleted, Completed, IoUring, ReadOrWrite, ReadWriteCompleted, RecvmsgCompleted,
        RecvmsgData, RingId, SendmsgCompleted,
    },
    types::empty_msghdr,
};

pub enum IoUringAction {
    Completion,
    WaitForCompletion,
    Read {
        id: Option<RingId>,
        fd: RawFd,
        offset: usize,
        length: usize,
        dispatch_to: Redispatcher<std::io::Result<ReadWriteCompleted>>,
    },
    ReadSome {
        id: Option<RingId>,
        fd: RawFd,
        offset: usize,
        length: usize,
        dispatch_to: Redispatcher<std::io::Result<ReadWriteCompleted>>,
    },
    Recvmsg {
        id: Option<RingId>,
        fd: RawFd,
        dispatch_to: Redispatcher<std::io::Result<RecvmsgCompleted>>,
    },
    Cancel {
        target: RingId,
    },
    Poll {
        id: Option<RingId>,
        fd: RawFd,
        dispatch_to: Redispatcher<std::io::Result<PollCompleted>>,
    },
    PollRemove {
        target: RingId,
    },
    Write {
        fd: RawFd,
        offset: usize,
        buffer: Box<[u8]>,
        dispatch_to: Redispatcher<std::io::Result<ReadWriteCompleted>>,
    },
    Sendmsg {
        fd: RawFd,
        buffer: Box<[u8]>,
        fds: Box<[RawFd]>,
        dispatch_to: Redispatcher<std::io::Result<SendmsgCompleted>>,
    },
    Accept {
        fd: RawFd,
        dispatch_to: Redispatcher<std::io::Result<AcceptCompleted>>,
    },
}

impl std::fmt::Debug for IoUringAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Completion => write!(f, "Completion"),
            Self::WaitForCompletion => write!(f, "WaitForCompletion"),
            Self::Read { id, fd, offset, length, dispatch_to } => f
                .debug_struct("Read")
                .field("id", id)
                .field("fd", fd)
                .field("offset", offset)
                .field("length", length)
                .field("dispatch_to", dispatch_to)
                .finish(),
            Self::ReadSome { id, fd, offset, length, dispatch_to } => f
                .debug_struct("ReadSome")
                .field("id", id)
                .field("fd", fd)
                .field("offset", offset)
                .field("length", length)
                .field("dispatch_to", dispatch_to)
                .finish(),
            Self::Recvmsg { id, fd, dispatch_to } => f
                .debug_struct("Recvmsg")
                .field("id", id)
                .field("fd", fd)
                .field("dispatch_to", dispatch_to)
                .finish(),
            Self::Cancel { target } => f.debug_struct("Cancel").field("target", target).finish(),
            Self::Poll { id, fd, dispatch_to } => f
                .debug_struct("Poll")
                .field("id", id)
                .field("fd", fd)
                .field("dispatch_to", dispatch_to)
                .finish(),
            Self::PollRemove { target } => f
                .debug_struct("PollRemove")
                .field("target", target)
                .finish(),
            Self::Write { fd, offset, buffer, dispatch_to } => f
                .debug_struct("Write")
                .field("fd", fd)
                .field("offset", offset)
                .field("buffer_len", &buffer.len())
                .field("dispatch_to", dispatch_to)
                .finish(),
            Self::Sendmsg { fd, dispatch_to, buffer, fds } => f
                .debug_struct("Sendmsg")
                .field("fd", fd)
                .field("buffer_len", &buffer.len())
                .field("fds", fds)
                .field("dispatch_to", dispatch_to)
                .finish(),
            Self::Accept { fd, dispatch_to } => f
                .debug_struct("Accept")
                .field("fd", fd)
                .field("dispatch_to", dispatch_to)
                .finish(),
        }
    }
}

pub struct IoUringState {
    ring: IoUring,
    need_submit: bool,
    /// Pushed to the sq
    pending: BTreeMap<RingId, Operation>,
    ignored_completions: BTreeSet<RingId>,
    completed: VecDeque<Completed>,
    /// Operations that needs to be pushed to the sqe
    to_push: VecDeque<Operation>,
}

impl Default for IoUringState {
    fn default() -> Self {
        IoUringState {
            ring: IoUring::new(256).unwrap(),
            need_submit: false,
            pending: BTreeMap::default(),
            ignored_completions: BTreeSet::default(),
            completed: VecDeque::with_capacity(128),
            to_push: VecDeque::with_capacity(32),
        }
    }
}

impl Model for IoUringState {
    type Action = IoUringAction;

    fn process(
        &mut self,
        _state: &mut State,
        action: IoUringAction,
        dispatcher: &mut Dispatcher,
    ) -> std::result::Result<(), crate::runner::ModelFailure> {
        // log::trace!("action={:?}", action);
        match action {
            IoUringAction::Completion => {
                while let Some(completed) = self.completed.pop_front() {
                    self.process_completed(completed, dispatcher);
                }
            }
            IoUringAction::WaitForCompletion => {
                let mut completed = Some(self.ring.wait_pop().unwrap());
                while let Some(c) = completed {
                    self.completed.push_back(c);
                    completed = self.ring.pop();
                }
                if !self.completed.is_empty() {
                    dispatcher.dispatch(IoUringAction::Completion);
                }
            }
            IoUringAction::Read { id, fd, offset, length, dispatch_to } => {
                let id = id.unwrap_or_else(RingId::fresh);
                let buffer = Self::make_buffer(length);
                let kind = ReadOrWrite::Read;
                let nbytes_processed = 0;
                let neof = 0;
                self.push_op(Operation::ReadWrite {
                    id,
                    fd,
                    offset,
                    buffer,
                    dispatch_to,
                    kind,
                    nbytes_processed,
                    neof,
                });
            }
            IoUringAction::ReadSome { id, fd, offset, length, dispatch_to } => {
                let id = id.unwrap_or_else(RingId::fresh);
                let buffer = Self::make_buffer(length);
                self.push_op(Operation::ReadSome { id, fd, offset, buffer, dispatch_to });
            }
            IoUringAction::Cancel { target } => {
                self.ignored_completions.insert(target);
                let id = self.next_id();
                self.push_op(Operation::Cancel { id, target });
            }
            IoUringAction::Write { fd, offset, buffer, dispatch_to } => {
                let id = self.next_id();
                let kind = ReadOrWrite::Write;
                let nbytes_processed = 0;
                let neof = 0;
                self.push_op(Operation::ReadWrite {
                    fd,
                    offset,
                    buffer,
                    dispatch_to,
                    id,
                    kind,
                    nbytes_processed,
                    neof,
                });
            }
            IoUringAction::Recvmsg { id, fd, dispatch_to } => {
                let id = id.unwrap_or_else(RingId::fresh);
                let buffer_read = vec![0; 32 * 1024];
                let buffer_aux = vec![0; 32 * 1024];
                let mut data = Box::new(RecvmsgData {
                    iovec: libc::iovec {
                        iov_base: buffer_read.as_ptr() as *mut _,
                        iov_len: buffer_read.len(),
                    },
                    msghdr: empty_msghdr(),
                    buffer_aux,
                    buffer_read,
                });
                data.msghdr.msg_iov = &data.iovec as *const _ as *mut _;
                data.msghdr.msg_iovlen = 1;
                data.msghdr.msg_control = data.buffer_aux.as_ptr() as *mut _;
                data.msghdr.msg_controllen = data.buffer_aux.len();
                self.push_op(Operation::Recvmsg { id, fd, data, dispatch_to });
            }
            IoUringAction::Sendmsg { fd, buffer, fds, dispatch_to } => {
                let id = self.next_id();
                let buffer_aux = write_fds(&*fds);
                let mut data = Box::new(SendmsgData {
                    iovec: libc::iovec {
                        iov_base: buffer.as_ptr() as *mut _,
                        iov_len: buffer.len(),
                    },
                    msghdr: empty_msghdr(),
                    buffer_aux,
                    buffer,
                });
                data.msghdr.msg_iov = &data.iovec as *const _ as *mut _;
                data.msghdr.msg_iovlen = 1;
                if !data.buffer_aux.is_empty() {
                    data.msghdr.msg_control = data.buffer_aux.as_ptr() as *mut _;
                    data.msghdr.msg_controllen = data.buffer_aux.len();
                }
                self.push_op(Operation::Sendmsg { id, fd, data, dispatch_to });
            }
            IoUringAction::Accept { fd, dispatch_to } => {
                let id = self.next_id();
                self.push_op(Operation::Accept { id, fd, dispatch_to });
            }
            IoUringAction::Poll { id, fd, dispatch_to } => {
                let id = id.unwrap_or_else(RingId::fresh);
                self.push_op(Operation::Poll { id, fd, dispatch_to });
            }
            IoUringAction::PollRemove { target } => {
                self.ignored_completions.insert(target);
                let id = self.next_id();
                self.push_op(Operation::PollRemove { id, target });
            }
        }

        Ok(())
    }
}

impl IoUringState {
    fn make_buffer(length: usize) -> Box<[u8]> {
        Box::<[u8]>::from(vec![0; length])
    }

    fn next_id(&mut self) -> RingId {
        RingId::fresh()
    }

    fn push_op(&mut self, op: Operation) {
        self.to_push.push_back(op);
        self.ring
            .push_entries(|| self.to_push.pop_front(), &mut self.pending);
    }

    fn is_eof(nbytes: usize, neof: &mut u8) -> bool {
        // Try twice
        // https://github.com/facebook/rocksdb/pull/6441#issuecomment-589843435
        // EOF reached when `neof` == 2
        if nbytes == 0 {
            *neof += 1;
            return *neof == 2;
        }
        false
    }

    fn process_completed(&mut self, completed: Completed, dispatcher: &mut Dispatcher) {
        let Completed { user_data: _, id, result } = completed;
        let op = self.pending.remove(&id).unwrap();

        if self.ignored_completions.remove(&id) {
            return;
        }

        if result < 0 {
            let io_err = || std::io::Error::from_raw_os_error(-result);
            match op {
                Operation::Cancel { target, .. } => {
                    if !self.pending.contains_key(&target) {
                        self.ignored_completions.remove(&target);
                    }
                    if result != -libc::ENOENT {
                        error!(
                            "io_uring async cancel failed target={:?} error_code={:?}",
                            target, result
                        );
                    }
                    return;
                }
                Operation::PollRemove { target, .. } => {
                    if !self.pending.contains_key(&target) {
                        self.ignored_completions.remove(&target);
                    }
                    if result != -libc::ENOENT {
                        error!(
                            "io_uring poll remove failed target={:?} error_code={:?}",
                            target, result
                        );
                    }
                    return;
                }
                Operation::ReadSome { dispatch_to, .. } => {
                    dispatcher.redispatch_to(dispatch_to, Err(io_err()));
                    return;
                }
                Operation::ReadWrite { dispatch_to, .. } => {
                    dispatcher.redispatch_to(dispatch_to, Err(io_err()));
                    return;
                }
                Operation::Recvmsg { dispatch_to, .. } => {
                    dispatcher.redispatch_to(dispatch_to, Err(io_err()));
                    return;
                }
                Operation::Sendmsg { dispatch_to, .. } => {
                    dispatcher.redispatch_to(dispatch_to, Err(io_err()));
                    return;
                }
                Operation::Accept { dispatch_to, .. } => {
                    dispatcher.redispatch_to(dispatch_to, Err(io_err()));
                    return;
                }
                Operation::Poll { dispatch_to, .. } => {
                    dispatcher.redispatch_to(dispatch_to, Err(io_err()));
                    return;
                }
                Operation::NoOp => {
                    error!("io_uring NoOp failed error_code={result:?}");
                    return;
                }
            }
        }
        // log::trace!("result={} completed={:?}", result, op);
        match op {
            Operation::NoOp => {}
            Operation::Cancel { target, .. } => {
                if !self.pending.contains_key(&target) {
                    self.ignored_completions.remove(&target);
                }
            }
            Operation::PollRemove { target, .. } => {
                if !self.pending.contains_key(&target) {
                    self.ignored_completions.remove(&target);
                }
            }
            Operation::Poll { id, fd, dispatch_to: to } => {
                let dispatch_to = to.clone();
                self.pending
                    .insert(id, Operation::Poll { id, fd, dispatch_to });
                dispatcher.redispatch_to(to, Ok(PollCompleted {}))
            }
            Operation::Recvmsg { id: _, fd, data, dispatch_to: to } => {
                let nbytes = result as usize;
                let buffer = Box::from(&data.buffer_read[..nbytes]);
                let fds =
                    read_fds(&data.buffer_aux[..data.msghdr.msg_controllen]).unwrap_or_default();
                dispatcher.redispatch_to(to, Ok(RecvmsgCompleted { fd, buffer, fds }));
            }
            Operation::ReadSome { id: _, fd, offset, buffer, dispatch_to: to } => {
                let nbytes = result as usize;
                if nbytes == 0 {
                    dispatcher.redispatch_to(to, Err(UnexpectedEof.into()));
                    return;
                }
                let buffer = Box::from(&buffer[..nbytes]);
                dispatcher.redispatch_to(to, Ok(ReadWriteCompleted { fd, offset, buffer }));
            }
            Operation::Sendmsg { id: _, fd, data: _, dispatch_to: to } => {
                dispatcher.redispatch_to(to, Ok(SendmsgCompleted { fd }));
            }
            Operation::Accept { id: _, fd, dispatch_to: to } => {
                let new_client: RawFd = result;
                dispatcher.redispatch_to(to, Ok(AcceptCompleted { new_client, fd }));
            }
            Operation::ReadWrite {
                id,
                buffer,
                fd,
                mut nbytes_processed,
                offset,
                mut neof,
                kind,
                dispatch_to: to,
            } => {
                // Number of bytes read/writen in this call
                let nbytes = result as usize;
                nbytes_processed += nbytes;
                if nbytes_processed == buffer.len() {
                    dispatcher.redispatch_to(to, Ok(ReadWriteCompleted { fd, offset, buffer }));
                    return;
                }
                // Handle partial read/write
                assert!(nbytes_processed < buffer.len());
                if Self::is_eof(nbytes, &mut neof) {
                    dispatcher.redispatch_to(to, Err(UnexpectedEof.into()));
                    return;
                }
                self.push_op(Operation::ReadWrite {
                    id,
                    fd,
                    offset,
                    kind,
                    nbytes_processed,
                    neof,
                    buffer,
                    dispatch_to: to,
                });
            }
        }
    }
}

const CMSG_HEADER_LENGTH: usize = 16;

fn align(size: usize) -> usize {
    size.next_multiple_of(std::mem::size_of::<usize>())
}

fn write_fds(fds: &[RawFd]) -> Box<[u8]> {
    if fds.is_empty() {
        return Box::new([]);
    }

    let header = cmsghdr {
        cmsg_len: CMSG_HEADER_LENGTH + align(std::mem::size_of_val(fds)),
        cmsg_level: libc::SOL_SOCKET,
        cmsg_type: libc::SCM_RIGHTS,
    };
    let mut bytes = Vec::with_capacity(header.cmsg_len);

    // Write header
    bytes.extend(header.cmsg_len.to_ne_bytes());
    bytes.extend(header.cmsg_level.to_ne_bytes());
    bytes.extend(header.cmsg_type.to_ne_bytes());
    // write fds
    bytes.extend(fds.iter().copied().flat_map(i32::to_ne_bytes));
    // paddding to alignment
    bytes.extend(std::iter::repeat(0).take(header.cmsg_len - bytes.len()));

    bytes.into_boxed_slice()
}

fn read_fds(mut data: &[u8]) -> std::io::Result<Vec<OwnedFd>> {
    let mut fds = Vec::default();

    while !data.is_empty() {
        let (header, msg_data) = read_cmsg(data)?;
        data = &data[align(header.cmsg_len)..];

        if header.cmsg_type != libc::SCM_RIGHTS {
            continue;
        }

        for raw in msg_data
            .iter()
            .copied()
            .array_chunks::<4>()
            .map(i32::from_ne_bytes)
        {
            if fds.capacity() == 0 {
                fds.reserve(8); // avoid reallocations
            }
            fds.push(unsafe { OwnedFd::from_raw_fd(raw) });
        }
    }

    Ok(fds)
}

fn read_cmsg(data: &[u8]) -> std::io::Result<(cmsghdr, &[u8])> {
    use std::io::ErrorKind::InvalidData;

    static_assertions::assert_eq_size!(usize, libc::size_t);
    static_assertions::assert_eq_size!(i32, libc::c_int);

    let read_n = |range: Range<usize>| -> std::io::Result<&[u8]> {
        data.get(range).ok_or_else(|| InvalidData.into())
    };
    if data.len() < CMSG_HEADER_LENGTH {
        return Err(InvalidData.into());
    }
    let header = cmsghdr {
        cmsg_len: usize::from_ne_bytes(read_n(0..8)?.try_into().unwrap()),
        cmsg_level: i32::from_ne_bytes(read_n(8..12)?.try_into().unwrap()),
        cmsg_type: i32::from_ne_bytes(read_n(12..16)?.try_into().unwrap()),
    };
    if header.cmsg_len < CMSG_HEADER_LENGTH {
        return Err(InvalidData.into());
    }
    let msg = read_n(CMSG_HEADER_LENGTH..header.cmsg_len)?;

    Ok((header, msg))
}
