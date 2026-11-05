//! Lazy Xwayland server.
//!
//! The listen socket is bound at startup and `DISPLAY` is exported, but
//! Xwayland is not executed until an X client connects. The connection stays
//! in the backlog: this model polls the listen socket and never accepts.
//! Xwayland accepts on the fd it is given.
//!
//! A process that lives longer than [`MIN_LIFETIME`] is followed by a new
//! display and another wait. A faster exit, including a failed exec, leaves
//! Xwayland off until the compositor starts again.

use std::{
    io, mem,
    os::fd::{AsRawFd, FromRawFd, IntoRawFd, OwnedFd, RawFd},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use nix::{
    errno::Errno,
    fcntl::{fcntl, FcntlArg, OFlag},
    sys::socket::{self, AddressFamily, SockFlag, SockType},
    unistd,
};

use crate::{
    clients::ClientManagerAction,
    forker::owned_pipe2,
    io_uring::{model::IoUringAction, ring::RingId},
    process_manager::{ProcessManagerAction, ProcessManagerEvent},
    runner::{Dispatcher, Model, ModelFailure, Redispatcher},
    state::State,
    xwayland::xsocket::{self, XSocket},
};

/// How long Xwayland must have lived before a fresh display is claimed.
pub const MIN_LIFETIME: Duration = Duration::from_secs(5);

/// Backlog for the X listen socket. The queued connection is Xwayland's to accept.
const LISTEN_BACKLOG: usize = 4096;

const PROGRAM: &str = "Xwayland";

/// Publish `DISPLAY` to the session bus and the systemd user manager.
///
/// Intentionally uncalled. Reaching the body would abort the compositor as
/// soon as a display is exported.
#[allow(dead_code)]
fn import_display_environment(display: &str) -> ! {
    let _ = display;
    todo!("import DISPLAY into the dbus activation environment and systemd --user");
}

/// Restart only after a life longer than `min_lifetime`. Five seconds does
/// not restart; anything above it does.
pub(crate) fn should_restart(lived: Duration, min_lifetime: Duration) -> bool {
    lived > min_lifetime
}

/// Test and log signal for one generation of the lazy loop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum XwaylandNotice {
    Waiting {
        socket_path: PathBuf,
    },
    /// The Wayland client has been inserted. The tag is set by the time a
    /// probe observes this.
    Launched,
    Ready,
    Restarting,
    GaveUp,
    Failed(String),
}

pub enum XwaylandAction {
    Start,
    Stop,
    ListenReady { generation: u64 },
    DisplayReady { generation: u64 },
    SpawnEvent { generation: u64, event: ProcessManagerEvent },
}

struct Live {
    socket: XSocket,
    /// Set once a client has triggered the spawn.
    started: Option<Instant>,
    displayfd: Option<OwnedFd>,
    display_buf: Vec<u8>,
    ready: bool,
    listen_poll: Option<RingId>,
    display_poll: Option<RingId>,
}

enum Stage {
    Idle,
    Live(Live),
    GaveUp,
}

pub struct XwaylandServer {
    program: String,
    /// Arguments placed before `-rootless`. Tests use this to name a script
    /// when `program` is an interpreter.
    leading_args: Vec<String>,
    min_lifetime: Duration,
    socket_dir: PathBuf,
    lock_dir: PathBuf,
    notices: Option<Redispatcher<XwaylandNotice>>,
    /// Bumped when a generation ends so a poll completion from the previous
    /// one cannot spawn again.
    generation: u64,
    stage: Stage,
}

impl XwaylandServer {
    pub fn new() -> Self {
        Self {
            program: PROGRAM.to_string(),
            leading_args: Vec::new(),
            min_lifetime: MIN_LIFETIME,
            socket_dir: PathBuf::from(xsocket::X_SOCKET_DIR),
            lock_dir: PathBuf::from(xsocket::X_LOCK_DIR),
            notices: None,
            generation: 0,
            stage: Stage::Idle,
        }
    }

    #[cfg(test)]
    pub(crate) fn for_test(
        program: String,
        leading_args: Vec<String>,
        min_lifetime: Duration,
        socket_dir: &Path,
        lock_dir: &Path,
        notices: Redispatcher<XwaylandNotice>,
    ) -> Self {
        Self {
            program,
            leading_args,
            min_lifetime,
            socket_dir: socket_dir.to_path_buf(),
            lock_dir: lock_dir.to_path_buf(),
            notices: Some(notices),
            generation: 0,
            stage: Stage::Idle,
        }
    }

    fn notify(&self, dispatcher: &mut Dispatcher, notice: XwaylandNotice) {
        if let Some(notices) = &self.notices {
            dispatcher.redispatch_to(notices.clone(), notice);
        }
    }

    fn set_display(&self, dispatcher: &mut Dispatcher, value: Option<String>) {
        dispatcher
            .dispatch(ProcessManagerAction::SetEnv { var: "DISPLAY".to_string(), val: value });
    }

    fn begin_wait(&mut self, dispatcher: &mut Dispatcher) {
        // import_display_environment() is the session-bus step. It stays
        // uncalled until that import exists.
        let socket =
            match xsocket::allocate_in(&self.socket_dir, &self.lock_dir, xsocket::DISPLAY_RANGE) {
                Ok(socket) => socket,
                Err(error) => {
                    log::error!("could not allocate an X socket: {error}");
                    self.stage = Stage::GaveUp;
                    self.notify(dispatcher, XwaylandNotice::GaveUp);
                    return;
                }
            };
        if let Err(error) = socket::listen(&socket.fd(), LISTEN_BACKLOG) {
            log::error!("could not listen on {}: {error}", socket.socket_path().display());
            self.stage = Stage::GaveUp;
            self.notify(dispatcher, XwaylandNotice::GaveUp);
            return;
        }

        let display = socket.display_name();
        let socket_path = socket.socket_path().to_path_buf();
        self.set_display(dispatcher, Some(display.clone()));

        let generation = self.generation;
        let poll_id = RingId::fresh();
        let fd = socket.fd().as_raw_fd();
        dispatcher.dispatch(IoUringAction::Poll {
            id: Some(poll_id),
            fd,
            dispatch_to: Redispatcher::to::<Self>(move |result| match result {
                Ok(_) => XwaylandAction::ListenReady { generation },
                Err(error) => {
                    log::error!("X listen poll failed: {error}");
                    XwaylandAction::ListenReady { generation }
                }
            }),
        });
        self.stage = Stage::Live(Live {
            socket,
            started: None,
            displayfd: None,
            display_buf: Vec::new(),
            ready: false,
            listen_poll: Some(poll_id),
            display_poll: None,
        });
        log::info!("waiting for an X client on {display}");
        self.notify(dispatcher, XwaylandNotice::Waiting { socket_path });
    }

    fn on_listen(&mut self, state: &mut State, dispatcher: &mut Dispatcher, generation: u64) {
        if generation != self.generation {
            return;
        }
        let Stage::Live(live) = &self.stage else {
            return;
        };
        if live.started.is_some() {
            return;
        }
        let mut live = match mem::replace(&mut self.stage, Stage::Idle) {
            Stage::Live(live) => live,
            other => {
                self.stage = other;
                return;
            }
        };
        disarm_poll(&mut live.listen_poll, dispatcher);
        live.started = Some(Instant::now());
        if let Err(error) = self.launch(&mut live, dispatcher) {
            log::error!("could not spawn Xwayland: {error}");
            self.stage = Stage::Live(live);
            self.finish(state, dispatcher);
            return;
        }
        self.stage = Stage::Live(live);
    }

    fn launch(&mut self, live: &mut Live, dispatcher: &mut Dispatcher) -> io::Result<()> {
        let (display_read, display_write) = owned_pipe2(OFlag::O_CLOEXEC | OFlag::O_NONBLOCK)?;
        let listen = dup_cloexec(live.socket.fd().as_raw_fd())?;
        let (wayland_ours, wayland_child) = socket::socketpair(
            AddressFamily::Unix,
            SockType::Stream,
            None,
            SockFlag::SOCK_CLOEXEC,
        )?;

        let generation = self.generation;
        let poll_id = RingId::fresh();
        let display_fd = display_read.as_raw_fd();
        dispatcher.dispatch(IoUringAction::Poll {
            id: Some(poll_id),
            fd: display_fd,
            dispatch_to: Redispatcher::to::<Self>(move |_| XwaylandAction::DisplayReady {
                generation,
            }),
        });
        live.displayfd = Some(display_read);
        live.display_poll = Some(poll_id);

        log::info!("starting {}", self.program);
        let mut argv = self.leading_args.clone();
        argv.extend([
            "-rootless".to_string(),
            "-terminate".to_string(),
            "-displayfd".to_string(),
            "3".to_string(),
            "-listenfd".to_string(),
            "4".to_string(),
        ]);
        dispatcher.dispatch(ProcessManagerAction::Spawn {
            prog: self.program.clone(),
            argv,
            env: vec![("WAYLAND_SOCKET".to_string(), Some("6".to_string()))],
            fd_slots: vec![3, 4, 6],
            fds: vec![display_write, listen, wayland_child].into_boxed_slice(),
            reply_to: Redispatcher::to::<Self>(move |event| XwaylandAction::SpawnEvent {
                generation,
                event,
            }),
        });
        // The client has to exist before Xwayland connects to it.
        dispatcher.dispatch(ClientManagerAction::NewClientFromFd {
            fd: wayland_ours.into_raw_fd(),
            xwayland: true,
        });
        self.notify(dispatcher, XwaylandNotice::Launched);
        Ok(())
    }

    fn on_display(&mut self, dispatcher: &mut Dispatcher, generation: u64) {
        if generation != self.generation {
            return;
        }
        let became_ready = {
            let Stage::Live(live) = &mut self.stage else {
                return;
            };
            let Some(fd) = live.displayfd.as_ref() else {
                return;
            };
            let chunk = match read_available(fd.as_raw_fd()) {
                Ok(chunk) => chunk,
                Err(error) => {
                    log::error!("could not read the Xwayland display fd: {error}");
                    disarm_poll(&mut live.display_poll, dispatcher);
                    return;
                }
            };
            if !chunk.bytes.is_empty() {
                live.display_buf.extend_from_slice(&chunk.bytes);
            }
            let became_ready = !live.ready && live.display_buf.contains(&b'\n');
            if became_ready {
                live.ready = true;
                log::info!("Xwayland is ready");
            }
            if !live.ready && chunk.eof {
                log::error!("Xwayland closed the display fd before it was ready");
            }
            // A hung-up pipe stays readable. Drop the poll once the line has
            // arrived or the write end is gone, or it fires forever.
            if live.ready || chunk.eof {
                disarm_poll(&mut live.display_poll, dispatcher);
            }
            became_ready
        };
        if became_ready {
            self.notify(dispatcher, XwaylandNotice::Ready);
        }
    }

    fn on_spawn(
        &mut self,
        state: &mut State,
        dispatcher: &mut Dispatcher,
        generation: u64,
        event: ProcessManagerEvent,
    ) {
        if generation != self.generation {
            return;
        }
        match event {
            ProcessManagerEvent::Spawned { pid, .. } => {
                log::info!("spawned Xwayland (pid {pid})");
            }
            ProcessManagerEvent::SpawnFailed { msg, .. } => {
                log::error!("Xwayland failed to start: {msg}");
                self.notify(dispatcher, XwaylandNotice::Failed(msg));
                self.finish(state, dispatcher);
            }
            ProcessManagerEvent::Exited { pid, .. } => {
                log::info!("Xwayland (pid {pid}) exited");
                self.finish(state, dispatcher);
            }
        }
    }

    fn finish(&mut self, state: &mut State, dispatcher: &mut Dispatcher) {
        let mut live = match mem::replace(&mut self.stage, Stage::Idle) {
            Stage::Live(live) => live,
            other => {
                self.stage = other;
                return;
            }
        };
        let lived = live
            .started
            .map(|started| started.elapsed())
            .unwrap_or(Duration::ZERO);
        let restart = should_restart(lived, self.min_lifetime);
        self.generation = self.generation.wrapping_add(1);
        disarm_poll(&mut live.listen_poll, dispatcher);
        disarm_poll(&mut live.display_poll, dispatcher);
        if let Some(id) = state.xwayland_client_id {
            dispatcher.dispatch(ClientManagerAction::Close { id });
        }
        drop(live);
        self.set_display(dispatcher, None);
        if restart {
            log::info!("Xwayland ran for {lived:?}; waiting for the next X client");
            self.notify(dispatcher, XwaylandNotice::Restarting);
            self.begin_wait(dispatcher);
        } else {
            log::warn!("Xwayland ran for {lived:?}; leaving it stopped");
            self.stage = Stage::GaveUp;
            self.notify(dispatcher, XwaylandNotice::GaveUp);
        }
    }

    fn stop(&mut self, state: &mut State, dispatcher: &mut Dispatcher) {
        self.generation = self.generation.wrapping_add(1);
        if let Stage::Live(mut live) = mem::replace(&mut self.stage, Stage::Idle) {
            disarm_poll(&mut live.listen_poll, dispatcher);
            disarm_poll(&mut live.display_poll, dispatcher);
            drop(live);
        }
        if let Some(id) = state.xwayland_client_id {
            dispatcher.dispatch(ClientManagerAction::Close { id });
        }
        self.set_display(dispatcher, None);
    }
}

impl Model for XwaylandServer {
    type Action = XwaylandAction;
    const START: Option<Self::Action> = Some(Self::Action::Start);
    const STOP: Option<Self::Action> = Some(Self::Action::Stop);

    fn process(
        &mut self,
        state: &mut State,
        action: Self::Action,
        dispatcher: &mut Dispatcher,
    ) -> Result<(), ModelFailure> {
        match action {
            XwaylandAction::Start => self.begin_wait(dispatcher),
            XwaylandAction::Stop => self.stop(state, dispatcher),
            XwaylandAction::ListenReady { generation } => {
                self.on_listen(state, dispatcher, generation)
            }
            XwaylandAction::DisplayReady { generation } => self.on_display(dispatcher, generation),
            XwaylandAction::SpawnEvent { generation, event } => {
                self.on_spawn(state, dispatcher, generation, event)
            }
        }
        Ok(())
    }
}

fn disarm_poll(poll: &mut Option<RingId>, dispatcher: &mut Dispatcher) {
    if let Some(id) = poll.take() {
        dispatcher.dispatch(IoUringAction::PollRemove { target: id });
    }
}

fn dup_cloexec(fd: RawFd) -> io::Result<OwnedFd> {
    let raw = fcntl(fd, FcntlArg::F_DUPFD_CLOEXEC(0)).map_err(io::Error::from)?;
    Ok(unsafe { OwnedFd::from_raw_fd(raw) })
}

struct Chunk {
    bytes: Vec<u8>,
    eof: bool,
}

/// Read what a non-blocking fd has pending. `Ok(0)` from `read` is end-of-file;
/// `EAGAIN` means the caller already took every byte that is available.
fn read_available(fd: RawFd) -> io::Result<Chunk> {
    let mut bytes = Vec::new();
    let mut buf = [0u8; 64];
    loop {
        match unistd::read(fd, &mut buf) {
            Ok(0) => return Ok(Chunk { bytes, eof: true }),
            Ok(n) => bytes.extend_from_slice(&buf[..n]),
            Err(Errno::EAGAIN) => return Ok(Chunk { bytes, eof: false }),
            Err(errno) => return Err(errno.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        cell::{Cell, RefCell},
        fs,
        io::Read,
        os::{
            fd::{AsFd, AsRawFd},
            unix::net::UnixStream,
        },
        path::Path,
        rc::Rc,
        time::Duration,
    };

    use nix::sys::{
        socket::{self, AddressFamily, SockFlag, SockType, UnixAddr},
        timerfd::{self, TimerFlags, TimerSetTimeFlags},
    };

    use crate::{
        clients::ClientManager,
        process_manager::ProcessManager,
        runner::{Runner, ShutdownReason},
        test_support::state_for_tests,
    };

    use super::*;

    #[test]
    fn restart_requires_a_life_longer_than_the_threshold() {
        let threshold = Duration::from_secs(5);
        assert!(!should_restart(threshold, threshold));
        assert!(should_restart(threshold + Duration::from_nanos(1), threshold));
        assert!(!should_restart(Duration::ZERO, Duration::ZERO));
        assert!(should_restart(Duration::from_nanos(1), Duration::ZERO));
    }

    struct Probe {
        notices: Rc<std::cell::RefCell<Vec<XwaylandNotice>>>,
        saw_client: Rc<Cell<bool>>,
        got_byte: Rc<Cell<bool>>,
        read_error: Rc<RefCell<String>>,
        xclient: Option<UnixStream>,
        did_connect: bool,
        env_output: Rc<RefCell<String>>,
        env_read: Option<OwnedFd>,
        env_write: Option<OwnedFd>,
        stop: Stop,
    }

    enum Stop {
        /// Shut down once a second display is waiting.
        OnSecondWait,
        /// After give-up, spawn `env` and shut down when it exits.
        AfterEnv,
    }

    enum ProbeAction {
        Notice(XwaylandNotice),
        Env(ProcessManagerEvent),
        Timeout,
    }

    impl Model for Probe {
        type Action = ProbeAction;

        fn process(
            &mut self,
            state: &mut State,
            action: Self::Action,
            dispatcher: &mut Dispatcher,
        ) -> Result<(), ModelFailure> {
            match action {
                ProbeAction::Notice(notice) => {
                    if state.xwayland_client_id.is_some() {
                        self.saw_client.set(true);
                    }
                    // Read while the listen socket is still open. Doing it
                    // after the generation is dropped resets a connection the
                    // stand-in has not accepted yet.
                    if matches!(notice, XwaylandNotice::Ready) {
                        self.read_x_byte();
                    }
                    let socket_path = match &notice {
                        XwaylandNotice::Waiting { socket_path } => Some(socket_path.clone()),
                        _ => None,
                    };
                    self.notices.borrow_mut().push(notice);
                    let waits = self
                        .notices
                        .borrow()
                        .iter()
                        .filter(|notice| matches!(notice, XwaylandNotice::Waiting { .. }))
                        .count();
                    // Connect once. A restart publishes a second path, and
                    // connecting there would spawn another server.
                    if let Some(path) = socket_path {
                        if !self.did_connect {
                            self.did_connect = true;
                            self.xclient = Some(connect_x(&path));
                        }
                    }
                    match self.stop {
                        Stop::OnSecondWait if waits >= 2 => {
                            dispatcher.request_shutdown(ShutdownReason::Quit);
                        }
                        Stop::AfterEnv
                            if matches!(
                                self.notices.borrow().last(),
                                Some(XwaylandNotice::GaveUp)
                            ) =>
                        {
                            self.spawn_env(dispatcher);
                        }
                        _ => {}
                    }
                }
                ProbeAction::Env(ProcessManagerEvent::Exited { .. }) => {
                    let read = self.env_read.take().expect("env stdout");
                    let mut output = String::new();
                    fs::File::from(read).read_to_string(&mut output).unwrap();
                    *self.env_output.borrow_mut() = output;
                    dispatcher.request_shutdown(ShutdownReason::Quit);
                }
                ProbeAction::Env(_) => {}
                ProbeAction::Timeout => {
                    dispatcher.request_shutdown(ShutdownReason::Quit);
                }
            }
            Ok(())
        }
    }

    impl Probe {
        fn read_x_byte(&mut self) {
            let Some(mut stream) = self.xclient.take() else {
                return;
            };
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut buf = [0u8; 1];
            match stream.read(&mut buf) {
                Ok(1) if buf[0] == b'x' => self.got_byte.set(true),
                Ok(n) => *self.read_error.borrow_mut() = format!("read {n} bytes: {buf:?}"),
                Err(error) => *self.read_error.borrow_mut() = error.to_string(),
            }
        }

        fn spawn_env(&mut self, dispatcher: &mut Dispatcher) {
            let write = self.env_write.take().expect("env stdout");
            dispatcher.dispatch(ProcessManagerAction::Spawn {
                prog: "/usr/bin/env".to_string(),
                argv: Vec::new(),
                env: Vec::new(),
                fd_slots: vec![1],
                fds: vec![write].into_boxed_slice(),
                reply_to: Redispatcher::to::<Probe>(ProbeAction::Env),
            });
        }
    }

    /// Python run with `-c`, so the stand-in does not depend on executing a
    /// file the forker has to find.
    fn stand_in_code(report: &Path) -> String {
        let mut code = String::from("import os, socket, sys, traceback\n");
        code.push_str(&format!("report = {report:?}\n"));
        code.push_str(
            "open(report, 'w').write('\\n'.join(sys.argv) + '\\nWAYLAND_SOCKET=' + os.environ.get('WAYLAND_SOCKET', '') + '\\n')\n",
        );
        code.push_str(
            "try:\n    os.write(3, b'ready\\n')\n    conn, _ = socket.fromfd(4, socket.AF_UNIX, socket.SOCK_STREAM).accept()\n    conn.sendall(b'x')\nexcept Exception:\n    open(report, 'a').write(traceback.format_exc())\n",
        );
        code
    }

    /// Queue an X connection without waiting for accept. A blocking connect
    /// would stall the runner before Xwayland is spawned to accept it.
    fn connect_x(path: &Path) -> UnixStream {
        let fd = socket::socket(
            AddressFamily::Unix,
            SockType::Stream,
            SockFlag::SOCK_CLOEXEC | SockFlag::SOCK_NONBLOCK,
            None,
        )
        .expect("X client socket");
        let address = UnixAddr::new(path).expect("X socket path");
        match socket::connect(fd.as_raw_fd(), &address) {
            Ok(()) | Err(Errno::EINPROGRESS) => {}
            Err(error) => panic!("X client connects: {error}"),
        }
        let stream = UnixStream::from(fd);
        stream.set_nonblocking(false).expect("blocking read");
        stream
    }

    fn run_case(min_lifetime: Duration, stop: Stop) -> RunResult {
        let python = std::process::Command::new("python3")
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        assert!(
            python.map(|status| status.success()).unwrap_or(false),
            "python3 is required for the Xwayland stand-in"
        );
        let dir = mktemp::Temp::new_dir().unwrap();
        let report = dir.to_path_buf().join("report");
        let code = stand_in_code(&report);
        let notices = Rc::new(RefCell::new(Vec::new()));
        let saw_client = Rc::new(Cell::new(false));
        let got_byte = Rc::new(Cell::new(false));
        let read_error = Rc::new(RefCell::new(String::new()));
        let env_output = Rc::new(RefCell::new(String::new()));
        let (env_read, env_write) = owned_pipe2(OFlag::O_CLOEXEC).unwrap();

        let mut runner = Runner::with_state(state_for_tests());
        let Ok(ring) = crate::io_uring::model::IoUringState::try_new() else {
            panic!("io_uring is available in tests");
        };
        let wayland = dir.to_path_buf().join("wayland");
        let manager = ClientManager::bind(&wayland).unwrap();
        let proxy = crate::forker::install().expect("the forker installs");
        runner.add_model(Box::new(ring));
        runner.add_model(Box::new(manager));
        runner.add_model(Box::new(ProcessManager::new(Some(proxy))));
        runner.add_model(Box::new(Probe {
            notices: notices.clone(),
            saw_client: saw_client.clone(),
            got_byte: got_byte.clone(),
            read_error: read_error.clone(),
            xclient: None,
            did_connect: false,
            env_output: env_output.clone(),
            env_read: Some(env_read),
            env_write: Some(env_write),
            stop,
        }));
        runner.add_model(Box::new(XwaylandServer::for_test(
            "/usr/bin/python3".to_string(),
            vec!["-c".to_string(), code],
            min_lifetime,
            dir.as_ref(),
            dir.as_ref(),
            Redispatcher::to::<Probe>(ProbeAction::Notice),
        )));

        let timer = timerfd::TimerFd::new(
            timerfd::ClockId::CLOCK_MONOTONIC,
            TimerFlags::TFD_CLOEXEC | TimerFlags::TFD_NONBLOCK,
        )
        .unwrap();
        timer
            .set(
                timerfd::Expiration::OneShot(Duration::from_secs(8).into()),
                TimerSetTimeFlags::empty(),
            )
            .unwrap();

        let mut dispatcher = Dispatcher::default();
        dispatcher.start::<ProcessManager>();
        dispatcher.start::<XwaylandServer>();
        dispatcher.dispatch(IoUringAction::Poll {
            id: None,
            fd: timer.as_fd().as_raw_fd(),
            dispatch_to: Redispatcher::to::<Probe>(|_| ProbeAction::Timeout),
        });
        let outcome = runner.run(dispatcher);
        assert_eq!(outcome.code, 0, "{outcome}");

        let notices = notices.borrow().clone();
        let env_output = env_output.borrow().clone();
        let read_error = read_error.borrow().clone();
        RunResult {
            notices,
            saw_client: saw_client.get(),
            got_byte: got_byte.get(),
            read_error,
            report: fs::read_to_string(&report).unwrap_or_default(),
            env_output,
            _dir: dir,
        }
    }

    struct RunResult {
        notices: Vec<XwaylandNotice>,
        saw_client: bool,
        got_byte: bool,
        read_error: String,
        report: String,
        env_output: String,
        _dir: mktemp::Temp,
    }

    fn waiting_paths(notices: &[XwaylandNotice]) -> Vec<&Path> {
        notices
            .iter()
            .filter_map(|notice| match notice {
                XwaylandNotice::Waiting { socket_path } => Some(socket_path.as_path()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn an_immediate_exit_gives_up_and_clears_display() {
        let result = run_case(Duration::from_secs(30), Stop::AfterEnv);
        assert!(
            result.got_byte,
            "stand-in: {} report:\n{} notices: {:?}",
            result.read_error, result.report, result.notices
        );
        assert!(result.saw_client, "the Wayland client was not tagged");
        assert_spawn_report(&result.report);
        let paths = waiting_paths(&result.notices);
        assert_eq!(paths.len(), 1, "a short-lived server must not claim another display");
        assert!(!paths[0].exists(), "the listen socket was left on disk");
        assert!(result.notices.contains(&XwaylandNotice::Ready));
        assert!(result.notices.contains(&XwaylandNotice::GaveUp));
        assert!(!result.notices.contains(&XwaylandNotice::Restarting));
        assert!(
            !result
                .env_output
                .lines()
                .any(|line| line.starts_with("DISPLAY=")),
            "DISPLAY still exported:\n{}",
            result.env_output
        );
    }

    #[test]
    fn an_exit_past_the_threshold_claims_a_new_display() {
        let result = run_case(Duration::ZERO, Stop::OnSecondWait);
        assert!(result.saw_client, "the Wayland client was not tagged");
        assert!(
            result.got_byte,
            "the stand-in never accepted the X connection: {} report:\n{}",
            result.read_error, result.report
        );
        assert_spawn_report(&result.report);
        let paths = waiting_paths(&result.notices);
        assert_eq!(paths.len(), 2, "{:?}", result.notices);
        // The first socket is released before the next claim, so the same
        // display number can be used again.
        assert!(result.notices.contains(&XwaylandNotice::Restarting));
        assert!(!result.notices.contains(&XwaylandNotice::GaveUp));
    }

    fn assert_spawn_report(report: &str) {
        let mut lines = report.lines();
        // `python3 -c` puts `-c` in argv and not the program text.
        assert_eq!(lines.next(), Some("-c"));
        assert_eq!(
            lines.collect::<Vec<_>>(),
            vec![
                "-rootless",
                "-terminate",
                "-displayfd",
                "3",
                "-listenfd",
                "4",
                "WAYLAND_SOCKET=6",
            ]
        );
    }
}
