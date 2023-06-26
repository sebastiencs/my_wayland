use std::{
    cell::RefCell,
    collections::{BTreeMap, VecDeque},
    os::fd::RawFd,
};

use log::error;

use crate::{
    io_uring::ring::Operation,
    state::{AnyAction, Dispatcher, Model, Redispatcher},
};

use super::ring::{Completed, IoUring, ReadOrWrite, ReadWriteCompleted, RingId};

enum IoUringAction {
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
}

struct IoUringState {
    ring: IoUring,
    need_submit: bool,
    /// Pushed to the sq
    pending: BTreeMap<RingId, Operation>,
    completed: VecDeque<Completed>,
    /// Operations that needs to be pushed to the sqe
    to_push: VecDeque<Operation>,
    current_ring_id: u32,
    to_dispatch: RefCell<VecDeque<AnyAction>>,
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
            to_push: VecDeque::with_capacity(32),
            to_dispatch: RefCell::new(VecDeque::with_capacity(32)),
        })
    }

    fn reducer(state: &mut Self::State, action: &IoUringAction) -> Result<(), ()> {
        match action {
            IoUringAction::Completion => {
                while let Some(completed) = state.completed.pop_front() {
                    let Completed {
                        user_data,
                        id,
                        result,
                    } = completed;

                    if result < 0 {
                        error!("Operation failed error_code={:?}", result);
                        // return Some((user_data, Err(std::io::ErrorKind::Other.into())));
                        panic!()
                    }

                    match state.pending.remove(&id).unwrap() {
                        Operation::NoOp => {}
                        Operation::ReadWrite {
                            id,
                            buffer,
                            fd,
                            mut nbytes_processed,
                            offset,
                            mut neof,
                            kind,
                            dispatch_to,
                        } => {
                            // Number of bytes read/writen in this call
                            let nbytes = result as usize;
                            nbytes_processed += nbytes;

                            if nbytes_processed == buffer.len() {
                                let action =
                                    dispatch_to(Ok(ReadWriteCompleted { fd, offset, buffer }));
                                state.to_dispatch.get_mut().push_back(action);
                                continue;
                            }

                            // Handle partial read/write

                            assert!(nbytes_processed < buffer.len());
                            if nbytes == 0 {
                                // Try twice
                                // https://github.com/facebook/rocksdb/pull/6441#issuecomment-589843435
                                if neof == 1 {
                                    // EOF reached
                                    let action =
                                        dispatch_to(Err(std::io::ErrorKind::UnexpectedEof.into()));
                                    state.to_dispatch.get_mut().push_back(action);
                                    continue;
                                }
                                neof += 1;
                            }

                            let id = state.next_id();
                            state.to_push.push_back(Operation::ReadWrite {
                                id,
                                fd,
                                offset,
                                kind,
                                nbytes_processed,
                                neof,
                                buffer,
                                dispatch_to,
                            });
                            state
                                .ring
                                .push_entries(|| state.to_push.pop_front(), &mut state.pending);
                        }
                    }
                }
            }
            IoUringAction::WaitForCompletion => {
                let completed = state.ring.wait_pop().unwrap();
                state.completed.push_back(completed);
                while let Some(completed) = state.ring.pop() {
                    state.completed.push_back(completed);
                }
            }
            IoUringAction::Read {
                fd,
                offset,
                length,
                dispatch_to,
            } => {
                let (fd, offset, length, dispatch_to) = (*fd, *offset, *length, *dispatch_to);
                let id = state.next_id();
                let buffer = Self::State::make_buffer(length);

                state.to_push.push_back(Operation::ReadWrite {
                    id,
                    fd,
                    offset,
                    buffer,
                    dispatch_to,
                    kind: ReadOrWrite::Read,
                    nbytes_processed: 0,
                    neof: 0,
                });
                state
                    .ring
                    .push_entries(|| state.to_push.pop_front(), &mut state.pending);
            }
            IoUringAction::Write {
                fd,
                offset,
                buffer,
                dispatch_to,
            } => {
                let (fd, offset, dispatch_to) = (*fd, *offset, *dispatch_to);
                let id = state.next_id();
                let buffer = buffer.clone();

                state.to_push.push_back(Operation::ReadWrite {
                    id,
                    fd,
                    offset,
                    buffer,
                    dispatch_to,
                    kind: ReadOrWrite::Write,
                    nbytes_processed: 0,
                    neof: 0,
                });
                state
                    .ring
                    .push_entries(|| state.to_push.pop_front(), &mut state.pending);
            }
        }

        Ok(())
    }

    fn effect(
        action: &IoUringAction,
        state: &Self::State,
        dispatcher: &mut Dispatcher,
    ) -> Result<(), ()> {
        use IoUringAction::*;

        match action {
            Completion => {
                for action in state.to_dispatch.borrow_mut().drain(..) {
                    dispatcher.dispatch(action)
                }
            }
            WaitForCompletion => {
                if state.completed.is_empty() {
                    dispatcher.dispatch(WaitForCompletion);
                } else {
                    dispatcher.dispatch(Completion);
                }
            }
            Read { .. } => {}
            Write { .. } => {}
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
}
