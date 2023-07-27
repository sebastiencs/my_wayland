use std::{
    collections::{BTreeMap, VecDeque},
    io::ErrorKind::UnexpectedEof,
    ops::Range,
    os::fd::RawFd,
};

use {libc::cmsghdr, log::error};

use crate::{
    io_uring::ring::{Operation, PollCompleted},
    runner::{Dispatcher, Model, Redispatcher},
    state::State,
};

use super::{
    ring::{
        AcceptCompleted, Completed, IoUring, ReadOrWrite, ReadWriteCompleted, RecvmsgCompleted,
        RecvmsgData, RingId,
    },
    types::empty_msghdr,
};

pub enum IoUringAction {
    Completion,
    WaitForCompletion,
    Read {
        fd: RawFd,
        offset: usize,
        length: usize,
        dispatch_to: Redispatcher<std::io::Result<ReadWriteCompleted>>,
    },
    Write {
        fd: RawFd,
        offset: usize,
        buffer: Box<[u8]>,
        dispatch_to: Redispatcher<std::io::Result<ReadWriteCompleted>>,
    },
    Recvmsg {
        fd: RawFd,
        dispatch_to: Redispatcher<std::io::Result<RecvmsgCompleted>>,
    },
    Accept {
        fd: RawFd,
        dispatch_to: Redispatcher<std::io::Result<AcceptCompleted>>,
    },
    Poll {
        fd: RawFd,
        dispatch_to: Redispatcher<std::io::Result<PollCompleted>>,
    },
}

impl std::fmt::Debug for IoUringAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Completion => write!(f, "Completion"),
            Self::WaitForCompletion => write!(f, "WaitForCompletion"),
            Self::Read { fd, offset, length, dispatch_to } => f
                .debug_struct("Read")
                .field("fd", fd)
                .field("offset", offset)
                .field("length", length)
                .field("dispatch_to", dispatch_to)
                .finish(),
            Self::Write { fd, offset, buffer, dispatch_to } => f
                .debug_struct("Write")
                .field("fd", fd)
                .field("offset", offset)
                .field("buffer_len", &buffer.len())
                .field("dispatch_to", dispatch_to)
                .finish(),
            Self::Recvmsg { fd, dispatch_to } => f
                .debug_struct("Recvmsg")
                .field("fd", fd)
                .field("dispatch_to", dispatch_to)
                .finish(),
            Self::Accept { fd, dispatch_to } => f
                .debug_struct("Accept")
                .field("fd", fd)
                .field("dispatch_to", dispatch_to)
                .finish(),
            Self::Poll { fd, dispatch_to } => f
                .debug_struct("Poll")
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
    completed: VecDeque<Completed>,
    /// Operations that needs to be pushed to the sqe
    to_push: VecDeque<Operation>,
    current_ring_id: u32,
}

impl Default for IoUringState {
    fn default() -> Self {
        IoUringState {
            ring: IoUring::new(256).unwrap(),
            need_submit: false,
            pending: BTreeMap::default(),
            completed: VecDeque::with_capacity(128),
            current_ring_id: 0,
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
    ) -> Result<(), ()> {
        log::trace!("io_uring action={:?}", action);
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
            IoUringAction::Read { fd, offset, length, dispatch_to } => {
                let id = self.next_id();
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
            IoUringAction::Recvmsg { fd, dispatch_to } => {
                let id = self.next_id();
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
            IoUringAction::Accept { fd, dispatch_to } => {
                let id = self.next_id();
                self.push_op(Operation::Accept { id, fd, dispatch_to });
            }
            IoUringAction::Poll { fd, dispatch_to } => {
                let id = self.next_id();
                self.push_op(Operation::Poll { id, fd, dispatch_to });
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
        let current = self.current_ring_id;
        self.current_ring_id = self.current_ring_id.wrapping_add(1);
        RingId(current)
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

        if result < 0 {
            error!("io_uring Operation failed error_code={:?}", result);
            // return Some((user_data, Err(std::io::ErrorKind::Other.into())));
            panic!()
        }
        let op = self.pending.remove(&id).unwrap();
        log::trace!("io_uring result={} completed={:?}", result, op);
        match op {
            Operation::NoOp => {}
            Operation::Poll { id, fd, dispatch_to: to } => {
                let dispatch_to = to.clone();
                self.pending
                    .insert(id, Operation::Poll { id, fd, dispatch_to });
                dispatcher.redispatch_to(to, Ok(PollCompleted {}))
            }
            Operation::Recvmsg { id: _, fd, data, dispatch_to: to } => {
                let nbytes = result as usize;
                let buffer = Box::from(&data.buffer_read[..nbytes]);
                let fds = read_fds(&data.buffer_aux[..data.msghdr.msg_controllen])
                    .unwrap_or_else(|_| Vec::new())
                    .into();
                dispatcher.redispatch_to(to, Ok(RecvmsgCompleted { fd, buffer, fds }));
            }
            Operation::Accept { id: _, fd, dispatch_to: to } => {
                let new_client: RawFd = result;
                dispatcher.redispatch_to(to, Ok(AcceptCompleted { new_client, fd }));
            }
            Operation::ReadWrite {
                id: _,
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
                let id = self.next_id();
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

fn read_fds(mut data: &[u8]) -> std::io::Result<Vec<RawFd>> {
    fn align(size: usize) -> usize {
        const ALIGN: usize = std::mem::size_of::<usize>() - 1;
        (size + ALIGN) & !ALIGN
    }

    let mut fds = Vec::default();

    while !data.is_empty() {
        let (header, msg_data) = read_cmsg(data)?;
        data = &data[align(header.cmsg_len)..];

        if header.cmsg_type != libc::SCM_RIGHTS {
            continue;
        }

        let chunks = msg_data
            .array_chunks::<4>()
            .map(|chunk| i32::from_ne_bytes(*chunk));
        fds.extend(chunks);
    }

    Ok(fds)
}

fn read_cmsg(data: &[u8]) -> std::io::Result<(cmsghdr, &[u8])> {
    use std::io::ErrorKind::InvalidData;

    const HEADER_LENGTH: usize = 16;
    static_assertions::assert_eq_size!(usize, libc::size_t);
    static_assertions::assert_eq_size!(i32, libc::c_int);

    let read_n = |range: Range<usize>| -> std::io::Result<&[u8]> {
        data.get(range).ok_or_else(|| InvalidData.into())
    };
    if data.len() < HEADER_LENGTH {
        return Err(InvalidData.into());
    }
    let header = cmsghdr {
        cmsg_len: usize::from_ne_bytes(read_n(0..8)?.try_into().unwrap()),
        cmsg_level: i32::from_ne_bytes(read_n(8..12)?.try_into().unwrap()),
        cmsg_type: i32::from_ne_bytes(read_n(12..16)?.try_into().unwrap()),
    };
    if header.cmsg_len < HEADER_LENGTH {
        return Err(InvalidData.into());
    }
    let msg = read_n(HEADER_LENGTH..header.cmsg_len)?;

    Ok((header, msg))
}
