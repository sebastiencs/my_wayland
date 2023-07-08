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

use super::{
    ring::{
        AcceptCompleted, Completed, IoUring, ReadOrWrite, ReadWriteCompleted, RecvmsgCompleted,
        RecvmsgData, RingId,
    },
    types::empty_msghdr,
};

#[derive(Debug)]
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
    to_dispatch: RefCell<VecDeque<AnyAction>>,
}

pub struct IoUringModel;

// impl Model<IoUringAction> for IoUringState {
impl Model for IoUringState {
    // type State = IoUringState;
    type InitParams = ();
    type Action = IoUringAction;

    fn init(params: Self::InitParams) -> Box<Self> {
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

    fn reducer(&mut self, action: &IoUringAction) -> Result<(), ()> {
        log::trace!("io_uring action={:?}", action);
        match action {
            IoUringAction::Completion => {
                while let Some(completed) = self.completed.pop_front() {
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

                    let op = self.pending.remove(&id).unwrap();

                    log::trace!("io_uring completed={:?}", op);

                    match op {
                        Operation::NoOp => {}
                        Operation::Recvmsg {
                            id,
                            fd,
                            data,
                            dispatch_to,
                        } => {
                            let nbytes = result as usize;
                            let buffer = Box::from(&data.buffer_read[..nbytes]);
                            let action = dispatch_to(Ok(RecvmsgCompleted { fd, buffer }));
                            self.to_dispatch.get_mut().push_back(action);
                        }
                        Operation::Accept {
                            id,
                            fd,
                            dispatch_to,
                        } => {
                            let new_client: RawFd = result;
                            let action = dispatch_to(Ok(AcceptCompleted { new_client, fd }));
                            self.to_dispatch.get_mut().push_back(action);
                        }
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
                                self.to_dispatch.get_mut().push_back(action);
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
                                    self.to_dispatch.get_mut().push_back(action);
                                    continue;
                                }
                                neof += 1;
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
                                dispatch_to,
                            });
                        }
                    }
                }
            }
            IoUringAction::WaitForCompletion => {
                let completed = self.ring.wait_pop().unwrap();
                self.completed.push_back(completed);
                while let Some(completed) = self.ring.pop() {
                    self.completed.push_back(completed);
                }
            }
            IoUringAction::Read {
                fd,
                offset,
                length,
                dispatch_to,
            } => {
                let (fd, offset, length, dispatch_to) = (*fd, *offset, *length, *dispatch_to);
                let id = self.next_id();
                let buffer = Self::make_buffer(length);

                self.push_op(Operation::ReadWrite {
                    id,
                    fd,
                    offset,
                    buffer,
                    dispatch_to,
                    kind: ReadOrWrite::Read,
                    nbytes_processed: 0,
                    neof: 0,
                });
            }
            IoUringAction::Write {
                fd,
                offset,
                buffer,
                dispatch_to,
            } => {
                let (fd, offset, dispatch_to) = (*fd, *offset, *dispatch_to);
                let id = self.next_id();
                let buffer = buffer.clone();

                self.push_op(Operation::ReadWrite {
                    id,
                    fd,
                    offset,
                    buffer,
                    dispatch_to,
                    kind: ReadOrWrite::Write,
                    nbytes_processed: 0,
                    neof: 0,
                });
            }
            IoUringAction::Recvmsg { fd, dispatch_to } => {
                let (fd, dispatch_to) = (*fd, *dispatch_to);
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

                self.push_op(Operation::Recvmsg {
                    id,
                    fd,
                    data,
                    dispatch_to,
                });
            }
            IoUringAction::Accept { fd, dispatch_to } => {
                let (fd, dispatch_to) = (*fd, *dispatch_to);
                let id = self.next_id();

                self.push_op(Operation::Accept {
                    id,
                    fd,
                    dispatch_to,
                });
            }
        }

        Ok(())
    }

    fn effect(
        &self,
        action: &IoUringAction,
        // state: &Self::State,
        dispatcher: &mut Dispatcher,
    ) -> Result<(), ()> {
        use IoUringAction::*;

        match action {
            Completion => {
                for action in self.to_dispatch.borrow_mut().drain(..) {
                    dispatcher.dispatch_any(action)
                }
                // dispatcher.dispatch(WaitForCompletion);
            }
            WaitForCompletion => {
                if !self.completed.is_empty() {
                    dispatcher.dispatch(Completion);
                }
            }
            Read { .. } => {}
            Write { .. } => {}
            Recvmsg { .. } => {}
            Accept { .. } => {}
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
}
