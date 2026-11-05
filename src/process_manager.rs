//! The process manager: the Runner model that fronts the forker.
//!
//! Other subsystems request spawns through `ProcessManagerAction::Spawn`,
//! carrying a `reply_to` redispatcher that receives `Spawned`, `SpawnFailed`,
//! and `Exited` events for that one child. The `token` in the events
//! correlates the spawn reply with the later exit.
//!
//! I/O rides the shared io_uring ring: one `Recvmsg` is always armed on the
//! forker socket, spawn requests are `Sendmsg`s (fds travel as
//! `SCM_RIGHTS`), and each child's pidfd has a persistent `Poll`. A pidfd
//! stays readable after its process exits, so the `ChildExited` handler must
//! `PollRemove` its poll before anything else, or the re-armed poll fires
//! again immediately.
//!
//! Lifecycle: when the forker dies the model degrades — pending spawns get a
//! `SpawnFailed` reply and new spawns fail, but already-running children keep
//! reporting exits through their pidfds. Stopping the model closes the
//! forker socket (the forker exits on the end-of-file) and drops the
//! pidfds; the children themselves survive, reparented to init.

use std::{
    collections::BTreeMap,
    io,
    os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd},
    rc::Rc,
};

use crate::{
    forker::{ForkerMessage, ForkerProxy, ServerMessage},
    io_uring::{
        model::IoUringAction,
        ring::{RecvmsgCompleted, RingId, SendmsgCompleted},
    },
    runner::{AnyAction, Dispatcher, Model, ModelFailure, Redispatcher},
    state::State,
};

/// Events for one spawn, delivered to its `reply_to`.
#[derive(Debug)]
pub enum ProcessManagerEvent {
    /// The child exec'd; `pid` is its process id. An `Exited` with the same
    /// `token` follows when it terminates.
    Spawned { token: u64, pid: u32 },
    /// The child could not be spawned; no `Exited` follows.
    SpawnFailed { token: u64, msg: String },
    /// The child exited. The exit status is not available: the forker's
    /// auto-reap collects it.
    Exited { token: u64, pid: u32 },
}

pub enum ProcessManagerAction {
    Start,
    /// Spawn a process through the forker. `fd_slots.len()` must equal
    /// `fds.len()`; the child receives the fds at those slot numbers.
    Spawn {
        prog: String,
        argv: Vec<String>,
        env: Vec<(String, Option<String>)>,
        fd_slots: Vec<RawFd>,
        /// The fds to attach, in `fd_slots` order. The ring holds these
        /// references until the `Sendmsg` completes.
        fds: Vec<Rc<OwnedFd>>,
        reply_to: Redispatcher<ProcessManagerEvent>,
    },
    /// Set or unset (`None`) a variable in the forker's environment, which
    /// children inherit unless a spawn overrides it.
    SetEnv {
        var: String,
        val: Option<String>,
    },
    /// One message arrived from the forker. An empty frame is end-of-file.
    Incoming {
        frame: Box<[u8]>,
        fds: Vec<OwnedFd>,
    },
    /// A spawn request's `Sendmsg` completed. A failure is reported to the requester.
    SendSettled {
        token: u64,
        error: Option<io::Error>,
    },
    /// The poll of a child's pidfd became readable: the child exited.
    ChildExited {
        token: u64,
    },
    /// The forker exited or its socket closed.
    ForkerDied,
    /// Log one spawn outcome. Used by requesters that have no model of their
    /// own to receive events.
    SpawnResult {
        command: String,
        event: ProcessManagerEvent,
    },
    Stop,
}

struct Child {
    reply_to: Redispatcher<ProcessManagerEvent>,
    pid: u32,
    pidfd: OwnedFd,
    poll_op_id: RingId,
}

pub struct ProcessManager {
    socket: Option<OwnedFd>,
    forker_pidfd: Option<OwnedFd>,
    next_token: u64,
    recvmsg_op_id: Option<RingId>,
    forker_poll_op_id: Option<RingId>,
    /// Spawns waiting for the forker's reply, by token.
    awaiting: BTreeMap<u64, Redispatcher<ProcessManagerEvent>>,
    /// Children that exec'd; one armed pidfd poll each, by token.
    children: BTreeMap<u64, Child>,
}

impl ProcessManager {
    pub fn new(proxy: Option<ForkerProxy>) -> Self {
        let (socket, forker_pidfd) = match proxy {
            Some(proxy) => {
                let (socket, pidfd) = proxy.into_parts();
                (Some(socket), Some(pidfd))
            }
            None => (None, None),
        };
        Self {
            socket,
            forker_pidfd,
            next_token: 1,
            recvmsg_op_id: None,
            forker_poll_op_id: None,
            awaiting: BTreeMap::new(),
            children: BTreeMap::new(),
        }
    }

    fn arm_recvmsg(&mut self, dispatcher: &mut Dispatcher) {
        if self.recvmsg_op_id.is_some() || self.socket.is_none() {
            return;
        }
        let op_id = RingId::fresh();
        self.recvmsg_op_id = Some(op_id);
        dispatcher.dispatch(IoUringAction::Recvmsg {
            id: Some(op_id),
            fd: self.socket.as_ref().expect("socket is present").as_raw_fd(),
            dispatch_to: Redispatcher::to::<Self>(|result| match result {
                Ok(RecvmsgCompleted { fd: _, buffer, fds }) => {
                    ProcessManagerAction::Incoming { frame: buffer, fds }
                }
                Err(_) => ProcessManagerAction::ForkerDied,
            }),
        });
    }

    fn spawn(
        &mut self,
        dispatcher: &mut Dispatcher,
        prog: String,
        argv: Vec<String>,
        env: Vec<(String, Option<String>)>,
        fd_slots: Vec<RawFd>,
        fds: Vec<Rc<OwnedFd>>,
        reply_to: Redispatcher<ProcessManagerEvent>,
    ) {
        let token = self.next_token;
        self.next_token = self.next_token.saturating_add(1);

        let Some(socket) = self.socket.as_ref() else {
            dispatcher.redispatch_to(
                reply_to,
                ProcessManagerEvent::SpawnFailed { token, msg: "the forker is not running".into() },
            );
            return;
        };

        if fd_slots.len() != fds.len() {
            dispatcher.redispatch_to(
                reply_to,
                ProcessManagerEvent::SpawnFailed {
                    token,
                    msg: "the number of fd slots does not match the number of fds".into(),
                },
            );
            return;
        }

        self.awaiting.insert(token, reply_to);

        let message = ServerMessage::Spawn { token, prog, argv, env, fd_slots };
        let frame: Box<[u8]> = bincode::serialize(&message)
            .expect("spawn request serializes")
            .into();
        dispatcher.dispatch(IoUringAction::Sendmsg {
            fd: socket.as_raw_fd(),
            buffer: frame,
            fds,
            dispatch_to: Redispatcher::to_any(move |result: std::io::Result<SendmsgCompleted>| {
                AnyAction::from(ProcessManagerAction::SendSettled { token, error: result.err() })
            }),
        });
    }

    fn set_env(&mut self, dispatcher: &mut Dispatcher, var: String, val: Option<String>) {
        let Some(socket) = self.socket.as_ref() else {
            return;
        };
        let message = ServerMessage::SetEnv { var: var.into_bytes(), val: val.map(Into::into) };
        let frame: Box<[u8]> = bincode::serialize(&message)
            .expect("set-env request serializes")
            .into();
        dispatcher.dispatch(IoUringAction::Sendmsg {
            fd: socket.as_raw_fd(),
            buffer: frame,
            fds: Vec::new(),
            dispatch_to: Redispatcher::None,
        });
    }

    fn handle_incoming(
        &mut self,
        dispatcher: &mut Dispatcher,
        frame: Box<[u8]>,
        fds: Vec<OwnedFd>,
    ) {
        self.recvmsg_op_id = None;
        if frame.is_empty() {
            // End-of-file on the forker socket.
            self.forker_died(dispatcher);
            return;
        }
        let message: ForkerMessage = match bincode::deserialize(&frame) {
            Ok(message) => message,
            Err(error) => {
                log::error!("undecodable message from the forker: {error}");
                self.forker_died(dispatcher);
                return;
            }
        };
        match message {
            ForkerMessage::Spawned { token, pid } => {
                let Some(reply_to) = self.awaiting.remove(&token) else {
                    log::warn!("spawn reply for unknown token {token}");
                    self.arm_recvmsg(dispatcher);
                    return;
                };
                if fds.len() != 1 {
                    log::error!("spawn reply carried {} fds instead of one", fds.len());
                    dispatcher.redispatch_to(
                        reply_to,
                        ProcessManagerEvent::SpawnFailed {
                            token,
                            msg: "the spawn reply carried no pidfd".into(),
                        },
                    );
                    self.arm_recvmsg(dispatcher);
                    return;
                }
                let pidfd = fds.into_iter().next().expect("one fd");
                let poll_op_id = RingId::fresh();
                dispatcher.dispatch(IoUringAction::Poll {
                    id: Some(poll_op_id),
                    fd: pidfd.as_raw_fd(),
                    dispatch_to: Redispatcher::to::<Self>(move |_| {
                        ProcessManagerAction::ChildExited { token }
                    }),
                });
                self.children
                    .insert(token, Child { reply_to: reply_to.clone(), pid, pidfd, poll_op_id });
                dispatcher.redispatch_to(reply_to, ProcessManagerEvent::Spawned { token, pid });
            }
            ForkerMessage::SpawnFailed { token, msg } => {
                let Some(reply_to) = self.awaiting.remove(&token) else {
                    log::warn!("spawn reply for unknown token {token}");
                    self.arm_recvmsg(dispatcher);
                    return;
                };
                dispatcher.redispatch_to(reply_to, ProcessManagerEvent::SpawnFailed { token, msg });
            }
        }
        self.arm_recvmsg(dispatcher);
    }

    fn child_exited(&mut self, dispatcher: &mut Dispatcher, token: u64) {
        let Some(child) = self.children.remove(&token) else {
            log::warn!("exit event for unknown child token {token}");
            return;
        };
        // A pidfd stays readable after the process exits; remove the
        // re-armed poll before it fires again.
        dispatcher.dispatch(IoUringAction::PollRemove { target: child.poll_op_id });
        let Child { reply_to, pid, pidfd: _, poll_op_id: _ } = child;
        dispatcher.redispatch_to(reply_to, ProcessManagerEvent::Exited { token, pid });
    }

    fn forker_died(&mut self, dispatcher: &mut Dispatcher) {
        if self.socket.is_none() {
            // Already degraded; a second notification (pidfd poll and
            // end-of-file both fire) is not an error.
            return;
        }
        log::error!("the forker exited; process spawning is disabled until restart");
        self.socket = None;
        self.forker_pidfd = None;
        if let Some(op_id) = self.forker_poll_op_id.take() {
            dispatcher.dispatch(IoUringAction::PollRemove { target: op_id });
        }
        if let Some(op_id) = self.recvmsg_op_id.take() {
            dispatcher.dispatch(IoUringAction::Cancel { target: op_id });
        }
        // Pending spawns can never complete.
        let awaiting = std::mem::take(&mut self.awaiting);
        for (token, reply_to) in awaiting {
            dispatcher.redispatch_to(
                reply_to,
                ProcessManagerEvent::SpawnFailed { token, msg: "the forker exited".into() },
            );
        }
        // Children keep their pidfd polls: exit reports still work.
    }

    fn stop(&mut self, dispatcher: &mut Dispatcher) {
        if let Some(op_id) = self.recvmsg_op_id.take() {
            dispatcher.dispatch(IoUringAction::Cancel { target: op_id });
        }
        if let Some(op_id) = self.forker_poll_op_id.take() {
            dispatcher.dispatch(IoUringAction::PollRemove { target: op_id });
        }
        let children = std::mem::take(&mut self.children);
        for child in children.values() {
            dispatcher.dispatch(IoUringAction::PollRemove { target: child.poll_op_id });
        }
        drop(children);
        let awaiting = std::mem::take(&mut self.awaiting);
        for (token, reply_to) in awaiting {
            dispatcher.redispatch_to(
                reply_to,
                ProcessManagerEvent::SpawnFailed {
                    token,
                    msg: "the process manager is stopping".into(),
                },
            );
        }
        // Closing the socket stops the forker. Children survive: they run in
        // their own process groups and are reparented to init.
        self.socket = None;
        self.forker_pidfd = None;
    }
}

impl Model for ProcessManager {
    type Action = ProcessManagerAction;
    const START: Option<Self::Action> = Some(Self::Action::Start);
    const STOP: Option<Self::Action> = Some(Self::Action::Stop);

    fn process(
        &mut self,
        _state: &mut State,
        action: ProcessManagerAction,
        dispatcher: &mut Dispatcher,
    ) -> std::result::Result<(), ModelFailure> {
        match action {
            ProcessManagerAction::Start => {
                if self.socket.is_none() {
                    log::warn!("no forker; process spawning is unavailable");
                    return Err(ModelFailure::StopModel);
                }
                self.arm_recvmsg(dispatcher);
                let op_id = RingId::fresh();
                self.forker_poll_op_id = Some(op_id);
                dispatcher.dispatch(IoUringAction::Poll {
                    id: Some(op_id),
                    fd: self
                        .forker_pidfd
                        .as_ref()
                        .expect("pidfd is present")
                        .as_raw_fd(),
                    dispatch_to: Redispatcher::to::<Self>(|_| ProcessManagerAction::ForkerDied),
                });
            }
            ProcessManagerAction::Spawn { prog, argv, env, fd_slots, fds, reply_to } => {
                self.spawn(dispatcher, prog, argv, env, fd_slots, fds, reply_to);
            }
            ProcessManagerAction::SetEnv { var, val } => self.set_env(dispatcher, var, val),
            ProcessManagerAction::Incoming { frame, fds } => {
                self.handle_incoming(dispatcher, frame, fds);
            }
            ProcessManagerAction::SendSettled { token, error } => {
                if let Some(error) = error {
                    if let Some(reply_to) = self.awaiting.remove(&token) {
                        dispatcher.redispatch_to(
                            reply_to,
                            ProcessManagerEvent::SpawnFailed {
                                token,
                                msg: format!("could not send the spawn request: {error}"),
                            },
                        );
                    }
                }
            }
            ProcessManagerAction::ChildExited { token } => {
                self.child_exited(dispatcher, token);
            }
            ProcessManagerAction::ForkerDied => self.forker_died(dispatcher),
            ProcessManagerAction::SpawnResult { command, event } => match event {
                ProcessManagerEvent::Spawned { pid, .. } => {
                    log::info!("spawned `{command}` (pid {pid})")
                }
                ProcessManagerEvent::SpawnFailed { msg, .. } => {
                    log::warn!("failed to spawn `{command}`: {msg}")
                }
                ProcessManagerEvent::Exited { pid, .. } => {
                    log::info!("`{command}` (pid {pid}) exited")
                }
            },
            ProcessManagerAction::Stop => self.stop(dispatcher),
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, rc::Rc};

    use nix::{
        fcntl::{fcntl, FcntlArg, OFlag},
        poll::{poll, PollFd, PollFlags},
    };

    use {
        super::*,
        crate::{
            runner::{Runner, ShutdownReason},
            test_support::state_for_tests,
        },
    };

    /// A test model that records spawn events and requests shutdown when its
    /// trigger condition is met.
    struct SpawnProbe {
        events: Rc<RefCell<Vec<ProcessManagerEvent>>>,
        shutdown_on: ShutdownOn,
    }

    #[derive(Clone, Copy, PartialEq)]
    enum ShutdownOn {
        Exit,
        SpawnFailed,
    }

    enum SpawnProbeAction {
        Event(ProcessManagerEvent),
    }

    impl Model for SpawnProbe {
        type Action = SpawnProbeAction;

        fn process(
            &mut self,
            _state: &mut State,
            action: SpawnProbeAction,
            dispatcher: &mut Dispatcher,
        ) -> std::result::Result<(), ModelFailure> {
            let SpawnProbeAction::Event(event) = action;
            let trigger = match (&event, self.shutdown_on) {
                (ProcessManagerEvent::Exited { .. }, ShutdownOn::Exit)
                | (ProcessManagerEvent::SpawnFailed { .. }, ShutdownOn::SpawnFailed) => true,
                _ => false,
            };
            self.events.borrow_mut().push(event);
            if trigger {
                dispatcher.request_shutdown(ShutdownReason::Quit);
            }
            Ok(())
        }
    }

    fn setup_runner(
        events: Rc<RefCell<Vec<ProcessManagerEvent>>>,
        shutdown_on: ShutdownOn,
        proxy: crate::forker::ForkerProxy,
    ) -> (Runner, Dispatcher) {
        let mut runner = Runner::with_state(state_for_tests());
        let Ok(ring) = crate::io_uring::model::IoUringState::try_new() else {
            panic!("io_uring is available in tests");
        };
        runner.add_model(Box::new(ring));
        runner.add_model(Box::new(SpawnProbe { events, shutdown_on }));
        runner.add_model(Box::new(ProcessManager::new(Some(proxy))));
        let mut dispatcher = Dispatcher::default();
        dispatcher.start::<ProcessManager>();
        (runner, dispatcher)
    }

    #[test]
    fn a_spawn_reports_spawned_and_exited() {
        let proxy = crate::forker::install().expect("the forker installs");
        let events = Rc::new(RefCell::new(Vec::new()));
        let (mut runner, mut dispatcher) = setup_runner(events.clone(), ShutdownOn::Exit, proxy);
        dispatcher.dispatch(ProcessManagerAction::Spawn {
            prog: "/bin/true".to_string(),
            argv: Vec::new(),
            env: Vec::new(),
            fd_slots: Vec::new(),
            fds: Vec::new(),
            reply_to: Redispatcher::to::<SpawnProbe>(|event| SpawnProbeAction::Event(event)),
        });
        let outcome = runner.run(dispatcher);
        assert_eq!(outcome.code, 0);
        let events = events.borrow();
        assert_eq!(events.len(), 2);
        let (
            ProcessManagerEvent::Spawned { token, pid },
            ProcessManagerEvent::Exited { token: exit_token, pid: exit_pid },
        ) = (&events[0], &events[1])
        else {
            panic!("unexpected events: {events:?}");
        };
        assert_eq!(token, exit_token);
        assert_eq!(pid, exit_pid);
    }

    #[test]
    fn a_spawn_failure_reaches_the_requester() {
        let proxy = crate::forker::install().expect("the forker installs");
        let events = Rc::new(RefCell::new(Vec::new()));
        let (mut runner, mut dispatcher) =
            setup_runner(events.clone(), ShutdownOn::SpawnFailed, proxy);
        dispatcher.dispatch(ProcessManagerAction::Spawn {
            prog: "definitely-not-a-real-program".to_string(),
            argv: Vec::new(),
            env: Vec::new(),
            fd_slots: Vec::new(),
            fds: Vec::new(),
            reply_to: Redispatcher::to::<SpawnProbe>(|event| SpawnProbeAction::Event(event)),
        });
        let outcome = runner.run(dispatcher);
        assert_eq!(outcome.code, 0);
        let events = events.borrow();
        assert_eq!(events.len(), 1);
        let ProcessManagerEvent::SpawnFailed { msg, .. } = &events[0] else {
            panic!("unexpected event: {events:?}");
        };
        assert!(msg.contains("definitely-not-a-real-program"));
    }

    #[test]
    fn fds_and_the_forker_environment_reach_children() {
        let proxy = crate::forker::install().expect("the forker installs");
        let events = Rc::new(RefCell::new(Vec::new()));
        let (read_end, write_end) = crate::forker::owned_pipe2(OFlag::O_CLOEXEC).expect("pipe");
        let (mut runner, mut dispatcher) = setup_runner(events.clone(), ShutdownOn::Exit, proxy);
        dispatcher.dispatch(ProcessManagerAction::SetEnv {
            var: "PM_TEST_ENV".to_string(),
            val: Some("manager-value".to_string()),
        });
        dispatcher.dispatch(ProcessManagerAction::Spawn {
            prog: "/usr/bin/env".to_string(),
            argv: Vec::new(),
            env: vec![("PM_SPAWN_ONLY".to_string(), Some("spawn-value".to_string()))],
            fd_slots: vec![1],
            fds: vec![Rc::new(write_end)],
            reply_to: Redispatcher::to::<SpawnProbe>(|event| SpawnProbeAction::Event(event)),
        });
        let outcome = runner.run(dispatcher);
        assert_eq!(outcome.code, 0);
        let mut output = String::new();
        std::io::Read::read_to_string(&mut std::fs::File::from(read_end), &mut output).unwrap();
        assert!(output.contains("PM_TEST_ENV=manager-value"));
        assert!(output.contains("PM_SPAWN_ONLY=spawn-value"));
    }

    #[test]
    fn stopping_the_manager_stops_the_forker() {
        let proxy = crate::forker::install().expect("the forker installs");
        let forker_pidfd = fcntl(proxy.pidfd().as_raw_fd(), FcntlArg::F_DUPFD_CLOEXEC(0))
            .map(|fd| unsafe { OwnedFd::from_raw_fd(fd as RawFd) })
            .expect("could not dup the forker pidfd");
        let mut runner = Runner::with_state(state_for_tests());
        let Ok(ring) = crate::io_uring::model::IoUringState::try_new() else {
            panic!("io_uring is available in tests");
        };
        runner.add_model(Box::new(ring));
        runner.add_model(Box::new(ProcessManager::new(Some(proxy))));
        let mut dispatcher = Dispatcher::default();
        dispatcher.start::<ProcessManager>();
        // Shut down immediately; the teardown sequence stops the process
        // manager, which must close the forker socket.
        dispatcher.request_shutdown(ShutdownReason::Quit);
        let outcome = runner.run(dispatcher);
        assert_eq!(outcome.code, 0);
        let mut polls = [PollFd::new(&forker_pidfd, PollFlags::POLLIN)];
        assert_eq!(poll(&mut polls, 5000), Ok(1), "the forker did not exit");
    }
}
