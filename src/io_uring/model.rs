use std::{os::fd::RawFd, collections::{BTreeMap, VecDeque}, ptr::NonNull, cell::Cell};

use log::error;

use crate::{state::{Model, Dispatcher}, io_uring::ring::Operation};

use super::ring::{IoUring, Completed};

#[derive(Debug, Copy, Clone, Hash, Eq, PartialEq, PartialOrd, Ord)]
pub struct RingId(u32);

impl std::fmt::Display for RingId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "RingId {}", self.0)
    }
}

impl From<&Completed> for RingId {
    fn from(c: &Completed) -> Self {
        let id = c.user_data & 0xFFFFFFFF;
        Self(id.try_into().unwrap())
    }
}

impl From<u32> for RingId {
    fn from(n: u32) -> Self {
        Self(n)
    }
}

impl From<RingId> for u64 {
    fn from(id: RingId) -> Self {
        id.0 as u64
    }
}

enum IoUringAction {
    Completion,
    WaitForCompletion,
    Read {
        fd: RawFd,
        offset: usize,
        length: usize,
    },
    Write {
        fd: RawFd,
        offset: usize,
        buffer: Box<[u8]>,
    },
}

#[derive(Copy, Clone)]
pub(super) enum OpKind {
    Write,
    Read,
}

enum PendingOperation {
    ReadWrite {
        fd: RawFd,
        data: Box<[u8]>,
        // data: NonNull<u8>,
        data_length: usize,
        /// number of bytes read/written
        nbytes_processed: usize,
        offset: usize,
        /// Number of EOF (res == 0) received
        /// Consider EOF reached after 2
        /// https://github.com/facebook/rocksdb/pull/6441#issuecomment-589843435
        neof: u8,
        kind: OpKind,
    },
    Register,
}

struct IoUringState {
    ring: IoUring,
    need_submit: bool,
    /// Pushed to the sq
    pending: BTreeMap<RingId, PendingOperation>,
    completed: VecDeque<Completed>,
    /// Operations that needs to be pushed to the sqe
    to_be_pushed: VecDeque<Operation>,
    current_ring_id: u32,
}

struct IoUringModel;

impl Model<IoUringAction> for IoUringModel {
    type State = IoUringState;
    type InitParams = ();

    fn init(params: Self::InitParams) -> Box<Self::State> {
        Box::new(IoUringState {
            ring: IoUring::new(256).unwrap(),
            need_submit: false,
            pending: BTreeMap::default(),
            completed: VecDeque::with_capacity(128),
            current_ring_id: 0,
            to_be_pushed: VecDeque::with_capacity(32),
        })
    }

    fn reducer(state: &mut Self::State, action: &IoUringAction) -> Result<(), ()> {
        match action {
            IoUringAction::Completion => {
                while let Some(completed) = state.completed.pop_front() {
                    let Completed { user_data, result } = completed;
                    let id: RingId = (&completed).into();

                    if result < 0 {
                        error!("Operation failed error_code={:?}", result);
                        // return Some((user_data, Err(std::io::ErrorKind::Other.into())));
                    }
                }
            },
            IoUringAction::WaitForCompletion => {
                let completed = state.ring.wait_pop().unwrap();
                state.completed.push_back(completed);
                while let Some(completed) = state.ring.pop() {
                    state.completed.push_back(completed);
                }
            },
            IoUringAction::Read { fd, offset, length } => {
                let (fd, offset, length) = (*fd, *offset, *length);
                let id = state.next_id();
                let buffer = Self::State::make_buffer(length);

                state.to_be_pushed.push_back(Operation::Read {
                    id,
                    fd,
                    offset,
                    length,
                    buffer_ptr: buffer.as_ptr(),
                });
                state.ring.push_entries(|| {
                    state.to_be_pushed.pop_front()
                });
                state.pending.insert(id, PendingOperation::ReadWrite {
                    fd,
                    offset,
                    data_length: buffer.len(),
                    data: buffer,
                    nbytes_processed: 0,
                    neof: 0,
                    kind: OpKind::Read
                });
            },
            IoUringAction::Write { fd, offset, buffer } => {
                let (fd, offset) = (*fd, *offset);
                let id = state.next_id();
                let buffer = buffer.clone();

                state.to_be_pushed.push_back(Operation::Write {
                    id,
                    fd,
                    offset,
                    length: buffer.len(),
                    buffer_ptr: buffer.as_ptr(),
                });
                state.ring.push_entries(|| {
                    state.to_be_pushed.pop_front()
                });
                state.pending.insert(id, PendingOperation::ReadWrite {
                    fd,
                    offset,
                    data_length: buffer.len(),
                    data: buffer,
                    nbytes_processed: 0,
                    neof: 0,
                    kind: OpKind::Write
                });
            },
        }

        Ok(())
    }

    fn effect(action: &IoUringAction, dispatcher: &mut Dispatcher) -> Result<(), ()> {
        match action {
            IoUringAction::Completion => todo!(),
            IoUringAction::WaitForCompletion => {
                dispatcher.dispatch(IoUringAction::Completion);
            },
            IoUringAction::Read { fd, offset, length } => todo!(),
            IoUringAction::Write { fd, offset, buffer } => todo!(),
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

    fn maybe_push_old_ops() {

    }
}
