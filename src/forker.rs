//! The forker: a long-lived helper process that spawns children for the
//! compositor.
//!
//! The compositor never forks after startup. Spawn requests travel over a
//! `SOCK_SEQPACKET` socketpair to the forker; each message is one `sendmsg`,
//! so frames and their `SCM_RIGHTS` fds stay together. The forker places the
//! received fds at fixed slot numbers in the child (for example fd 3 =
//! `-displayfd`, fd 4 = `-listenfd`, fd 5 = `-wm`, fd 6 = `WAYLAND_SOCKET`
//! for a later Xwayland launch), applies environment overrides, and execs.
//! Spawned children get `/dev/null` stdio unless a spawn passes fds for
//! slots 0-2, and run in their own process group.
//!
//! Exit reporting uses pidfds: after a successful exec the forker opens a
//! pidfd for the child and hands it back with `SCM_RIGHTS`. The pidfd becomes
//! readable when the process exits. The forker ignores `SIGCHLD`, so the
//! kernel reaps its children automatically; liveness, not exit status, is
//! reported.
//!
//! Lifecycle: the forker exits when the compositor end of the socketpair
//! closes, which happens even when the compositor process is killed. There is
//! no pid-death signal: a double-forked process is a child of init, so the
//! socket is the only reliable lifecycle channel.
//!
//! Bootstrap: the forker is double-forked away from the compositor, so it is
//! never a child of the compositor and can never become a compositor-owned
//! zombie. The compositor never learns the forker's pid; it receives a pidfd
//! of the forker relayed over a datagram control socket.

use std::{
    collections::BTreeMap,
    ffi::{CString, OsString},
    io::{self, IoSlice, IoSliceMut},
    os::{
        fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd, RawFd},
        unix::ffi::OsStringExt,
    },
};

use nix::{
    errno::Errno,
    fcntl::{fcntl, open, FcntlArg, FdFlag, OFlag},
    poll::{poll, PollFd, PollFlags},
    sys::{
        signal::{signal, SigHandler, Signal},
        socket::{
            recvmsg, sendmsg, AddressFamily, ControlMessage, ControlMessageOwned, MsgFlags,
            SockFlag, SockType, UnixAddr,
        },
        stat::Mode,
        wait::{waitpid, WaitStatus},
    },
    unistd::{
        close, dup2, dup3, execvp, fork, getpid, getppid, pipe2, read, setsid, write, ForkResult,
        Pid,
    },
};

/// Largest message frame accepted in either direction. A larger frame is a
/// protocol violation and drops the connection.
pub(crate) const FRAME_MAX: usize = 32 * 1024;

/// Where the forker holds the request socket after bootstrap. A child that
/// requests slot 3 makes `map_fds` close it, which is fine: the child is
/// about to exec.
const FORKER_SOCKET_FD: RawFd = 3;

/// Requests, compositor → forker.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub enum ServerMessage {
    /// Set or unset a variable in the forker's environment. Children inherit
    /// it unless a spawn overrides it (used for `DISPLAY`).
    SetEnv { var: Vec<u8>, val: Option<Vec<u8>> },
    /// Spawn a process. `fd_slots.len()` fds must be attached to the frame,
    /// in order; the child receives them at those slot numbers.
    Spawn {
        token: u64,
        prog: String,
        argv: Vec<String>,
        env: Vec<(String, Option<String>)>,
        fd_slots: Vec<RawFd>,
    },
}

/// Replies, forker → compositor.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub enum ForkerMessage {
    /// The child exec'd successfully; its pidfd is attached as a single
    /// `SCM_RIGHTS` fd. The pidfd becomes readable when the process exits.
    Spawned { token: u64, pid: u32 },
    /// The child could not be spawned; `msg` explains why.
    SpawnFailed { token: u64, msg: String },
}

#[derive(Debug, thiserror::Error)]
pub enum ForkerInstallError {
    #[error("could not create a socketpair: {0}")]
    Socketpair(#[source] io::Error),
    #[error("could not fork: {0}")]
    Fork(#[source] io::Error),
    #[error("could not open a pidfd: {0}")]
    PidfdOpen(#[source] io::Error),
    #[error("could not relay the forker pidfd: {0}")]
    PidfdRelay(#[source] io::Error),
    #[error("could not reap the intermediate process: {0}")]
    ReapIntermediate(#[source] io::Error),
}

/// The compositor side of the forker connection.
pub struct ForkerProxy {
    socket: OwnedFd,
    pidfd: OwnedFd,
}

impl ForkerProxy {
    /// The spawn-request socket. Reads and writes whole message frames; the
    /// process-manager model owns it once wired up.
    pub fn socket(&self) -> BorrowedFd<'_> {
        self.socket.as_fd()
    }

    /// The forker process. Becomes readable when the forker exits.
    pub fn pidfd(&self) -> BorrowedFd<'_> {
        self.pidfd.as_fd()
    }
}

/// Start the forker and return the compositor side of the connection. Must be
/// called once, before the runner loop starts forking anything else. Only the
/// compositor path returns; the double-forked child keeps running the forker
/// loop.
pub fn install() -> Result<ForkerProxy, ForkerInstallError> {
    let (server, client) =
        socketpair(SockType::SeqPacket).map_err(ForkerInstallError::Socketpair)?;
    match double_fork()? {
        Some(pidfd) => Ok(ForkerProxy { socket: server, pidfd }),
        // We are the forker; this never returns.
        None => {
            drop(server);
            forker_main(client)
        }
    }
}

fn socketpair(ty: SockType) -> io::Result<(OwnedFd, OwnedFd)> {
    nix::sys::socket::socketpair(AddressFamily::Unix, ty, None, SockFlag::SOCK_CLOEXEC)
        .map_err(io::Error::from)
}

/// An owned pipe, matching the codebase's `OwnedFd` style.
fn owned_pipe2(flags: OFlag) -> io::Result<(OwnedFd, OwnedFd)> {
    let (read_end, write_end) = pipe2(flags).map_err(io::Error::from)?;
    let read_end = unsafe { OwnedFd::from_raw_fd(read_end) };
    let write_end = unsafe { OwnedFd::from_raw_fd(write_end) };
    Ok((read_end, write_end))
}

/// Open a pidfd for a process. No nix wrapper exists, so this stays a raw
/// syscall.
fn pidfd_open(pid: Pid) -> io::Result<OwnedFd> {
    let fd = unsafe {
        libc::syscall(libc::SYS_pidfd_open, pid.as_raw() as libc::c_int, 0 as libc::c_uint)
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { OwnedFd::from_raw_fd(fd as RawFd) })
}

fn dupfd_cloexec(fd: RawFd, minimum: RawFd) -> io::Result<RawFd> {
    fcntl(fd, FcntlArg::F_DUPFD_CLOEXEC(minimum))
        .map(|fd| fd as RawFd)
        .map_err(io::Error::from)
}

fn set_nonblocking(fd: RawFd) -> io::Result<()> {
    fcntl(fd, FcntlArg::F_SETFL(OFlag::O_NONBLOCK))
        .map(|_| ())
        .map_err(io::Error::from)
}

enum ForkOutcome {
    Parent { pid: Pid, pidfd: OwnedFd },
    Child { parent_pidfd: Option<OwnedFd> },
}

/// Fork, giving the parent a pidfd of the child without ever learning the
/// child's pid: the child opens a pidfd of itself and sends it over an
/// internal datagram socket. `child_gets_parent_pidfd` additionally hands the
/// child a pidfd of its parent, used to wait for an intermediate process to
/// die before proceeding.
fn fork_with_pidfd(child_gets_parent_pidfd: bool) -> Result<ForkOutcome, ForkerInstallError> {
    let (parent, child) = socketpair(SockType::Datagram).map_err(ForkerInstallError::Socketpair)?;
    let forked =
        unsafe { fork() }.map_err(|errno| ForkerInstallError::Fork(io::Error::from(errno)))?;
    match forked {
        ForkResult::Child => {
            // The child. Failures must exit the process: the parent is about
            // to block receiving a pidfd and only an EOF (not an error
            // return) unblocks it.
            drop(parent);
            let Ok(self_pidfd) = pidfd_open(getpid()) else {
                exit_now(127);
            };
            if send_fd(&child, self_pidfd.as_fd()).is_err() {
                exit_now(127);
            }
            drop(self_pidfd);
            let parent_pidfd = child_gets_parent_pidfd.then(|| {
                let Ok(pidfd) = pidfd_open(getppid()) else {
                    exit_now(127);
                };
                pidfd
            });
            Ok(ForkOutcome::Child { parent_pidfd })
        }
        ForkResult::Parent { child: pid } => {
            drop(child);
            let pidfd = recv_fd(&parent).map_err(ForkerInstallError::PidfdRelay)?;
            Ok(ForkOutcome::Parent { pid, pidfd })
        }
    }
}

/// Exit the (forked) current process. No nix wrapper exists for `_exit(2)`.
fn exit_now(status: i32) -> ! {
    unsafe { libc::_exit(status) }
}

/// Double-fork the forker away from the compositor. The compositor receives a
/// pidfd of the forker, relayed by the intermediate process, and reaps the
/// intermediate process. The forker side (the second child) waits until the
/// intermediate process is dead before returning, so the reparent to init
/// has happened by the time it serves.
fn double_fork() -> Result<Option<OwnedFd>, ForkerInstallError> {
    let (control_parent, control_child) =
        socketpair(SockType::Datagram).map_err(ForkerInstallError::Socketpair)?;
    let outcome = fork_with_pidfd(false)?;
    match outcome {
        ForkOutcome::Parent { pid: intermediate_pid, pidfd: _ } => {
            // The compositor.
            drop(control_child);
            let forker_pidfd = recv_fd(&control_parent);
            let reaped = waitpid(intermediate_pid, None);
            drop(control_parent);
            let reaped = reaped
                .map_err(|errno| ForkerInstallError::ReapIntermediate(io::Error::from(errno)))?;
            match reaped {
                WaitStatus::Exited(pid, _) | WaitStatus::Signaled(pid, _, _)
                    if pid == intermediate_pid => {}
                status => {
                    return Err(ForkerInstallError::ReapIntermediate(io::Error::new(
                        io::ErrorKind::Other,
                        format!("unexpected intermediate process status: {status:?}"),
                    )));
                }
            }
            forker_pidfd
                .map(Some)
                .map_err(ForkerInstallError::PidfdRelay)
        }
        ForkOutcome::Child { parent_pidfd: _ } => {
            // The intermediate process. Nothing in this branch may return
            // normally: an error return here would run the compositor's
            // error path inside a second process.
            drop(control_parent);
            let outcome = fork_with_pidfd(true);
            match outcome {
                Ok(ForkOutcome::Parent { pidfd: forker_pidfd, .. }) => {
                    let _ = send_fd(&control_child, forker_pidfd.as_fd());
                    exit_now(0);
                }
                Ok(ForkOutcome::Child { parent_pidfd }) => {
                    // The forker. Hold no control fds; `setup_fds` closes the
                    // rest anyway.
                    drop(control_child);
                    let Some(parent_pidfd) = parent_pidfd else {
                        exit_now(127);
                    };
                    wait_until_readable(&parent_pidfd);
                    Ok(None)
                }
                Err(_) => exit_now(127),
            }
        }
    }
}

fn wait_until_readable(fd: &OwnedFd) {
    let mut polls = [PollFd::new(fd, PollFlags::POLLIN)];
    loop {
        match poll(&mut polls, -1) {
            Ok(_) => return,
            Err(Errno::EINTR) => continue,
            Err(_) => exit_now(127),
        }
    }
}

fn send_fd(socket: &OwnedFd, fd: BorrowedFd) -> io::Result<()> {
    let control = [ControlMessage::ScmRights(&[fd.as_raw_fd()])];
    // A zero-length datagram is indistinguishable from end-of-file on the
    // receive side, so the relay carries a one-byte payload.
    let mut payload = [0u8];
    let iov = [IoSlice::new(&mut payload)];
    sendmsg(socket.as_raw_fd(), &iov, &control, MsgFlags::empty(), None::<&UnixAddr>)
        .map(|_| ())
        .map_err(io::Error::from)
}

fn recv_fd(socket: &OwnedFd) -> io::Result<OwnedFd> {
    let (payload, fds) = recv_frame(socket.as_raw_fd())?;
    if payload.len() != 1 || fds.len() != 1 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "expected a one-byte payload and a single fd",
        ));
    }
    Ok(fds.into_iter().next().unwrap())
}

/// Send one message frame with the attached fds. Blocking; for use in the
/// forker and in tests. The process-manager model uses async io_uring sends.
pub(crate) fn send_frame(socket: RawFd, bytes: &[u8], fds: &[RawFd]) -> io::Result<()> {
    let iov = [IoSlice::new(bytes)];
    let control = [ControlMessage::ScmRights(fds)];
    let sent = if fds.is_empty() {
        sendmsg(socket, &iov, &[], MsgFlags::empty(), None::<&UnixAddr>).map_err(io::Error::from)?
    } else {
        sendmsg(socket, &iov, &control, MsgFlags::empty(), None::<&UnixAddr>)
            .map_err(io::Error::from)?
    };
    if sent != bytes.len() {
        return Err(io::Error::new(io::ErrorKind::WriteZero, "short frame write"));
    }
    Ok(())
}

pub(crate) enum FrameError {
    /// The peer closed the socket.
    Eof,
    /// The frame exceeded `FRAME_MAX`.
    Truncated,
    Io(io::Error),
}

impl From<FrameError> for io::Error {
    fn from(error: FrameError) -> io::Error {
        match error {
            FrameError::Eof => io::Error::new(io::ErrorKind::UnexpectedEof, "socket closed"),
            FrameError::Truncated => {
                io::Error::new(io::ErrorKind::InvalidData, "frame larger than FRAME_MAX")
            }
            FrameError::Io(error) => error,
        }
    }
}

/// Receive one message frame and its attached fds. Blocking; for use in the
/// forker and in tests.
pub(crate) fn recv_frame(socket: RawFd) -> Result<(Vec<u8>, Vec<OwnedFd>), FrameError> {
    let mut buffer = vec![0u8; FRAME_MAX];
    let mut control = vec![0u8; FRAME_MAX];
    let (nbytes, flags, fds) = {
        let mut iov = [IoSliceMut::new(&mut buffer)];
        let message =
            recvmsg::<UnixAddr>(socket, &mut iov, Some(&mut control), MsgFlags::MSG_CMSG_CLOEXEC)
                .map_err(|errno| FrameError::Io(io::Error::from(errno)))?;
        let mut fds = Vec::new();
        for cmsg in message.cmsgs() {
            if let ControlMessageOwned::ScmRights(attached) = cmsg {
                for raw in attached {
                    fds.push(unsafe { OwnedFd::from_raw_fd(raw) });
                }
            }
        }
        (message.bytes, message.flags, fds)
    };
    if nbytes == 0 {
        return Err(FrameError::Eof);
    }
    if flags.contains(MsgFlags::MSG_TRUNC) {
        return Err(FrameError::Truncated);
    }
    buffer.truncate(nbytes);
    Ok((buffer, fds))
}

/// The forker process. Runs until the compositor end of the socketpair
/// closes, then exits.
fn forker_main(socket: OwnedFd) -> ! {
    // The kernel reaps children of a process that ignores SIGCHLD. We report
    // liveness through pidfds, so no exit status is ever collected here.
    let _ = unsafe { signal(Signal::SIGCHLD, SigHandler::SigIgn) };
    // Detach from the compositor's session so terminal signals do not reach
    // the forker or its children.
    let _ = setsid();
    let socket = setup_fds(socket);
    serve(&socket)
}

/// Move the socket to a fixed fd, replace stdio with `/dev/null` (children
/// inherit it unless a spawn passes fds for slots 0-2), and close every other
/// inherited fd so nothing leaks into children.
fn setup_fds(socket: OwnedFd) -> OwnedFd {
    let socket = move_fd(socket, FORKER_SOCKET_FD);
    if let Ok(null) = open("/dev/null", OFlag::O_RDWR, Mode::empty()) {
        for fd in libc::STDIN_FILENO..=libc::STDERR_FILENO {
            let _ = dup2(null, fd);
        }
        let _ = close(null);
    }
    // No nix wrapper for `close_range(2)`.
    let _ = unsafe { libc::close_range((FORKER_SOCKET_FD + 1) as u32, u32::MAX, 0) };
    socket
}

fn move_fd(fd: OwnedFd, target: RawFd) -> OwnedFd {
    if fd.as_raw_fd() == target {
        return fd;
    }
    let result = dup3(fd.as_raw_fd(), target, OFlag::O_CLOEXEC).map_err(io::Error::from);
    assert!(
        matches!(result, Ok(moved) if moved == target),
        "could not move an fd to slot {target}"
    );
    drop(fd);
    unsafe { OwnedFd::from_raw_fd(target) }
}

struct PendingSpawn {
    /// Read end of the exec-error pipe; becomes readable when the child
    /// exec'd (end-of-file) or failed (an error message).
    pipe: OwnedFd,
    token: u64,
    pid: Pid,
}

fn serve(socket: &OwnedFd) -> ! {
    let mut pending: Vec<PendingSpawn> = Vec::new();
    loop {
        // `PollFd` borrows the polled fds, so readiness is extracted before
        // `pending` is mutated again.
        let (socket_ready, ready_pipes) = {
            let mut polls = vec![PollFd::new(socket, PollFlags::POLLIN)];
            polls.extend(
                pending
                    .iter()
                    .map(|spawn| PollFd::new(&spawn.pipe, PollFlags::POLLIN)),
            );
            match poll(&mut polls, -1) {
                Ok(_) => {}
                Err(Errno::EINTR) => continue,
                Err(_) => std::process::exit(1),
            }
            let ready = PollFlags::POLLIN | PollFlags::POLLHUP | PollFlags::POLLERR;
            let socket_ready = polls[0]
                .revents()
                .unwrap_or(PollFlags::empty())
                .intersects(ready);
            let ready_pipes = polls[1..]
                .iter()
                .enumerate()
                .filter(|(_, poll)| {
                    poll.revents()
                        .unwrap_or(PollFlags::empty())
                        .intersects(ready)
                })
                .map(|(index, _)| index)
                .collect::<Vec<_>>();
            (socket_ready, ready_pipes)
        };
        if socket_ready {
            match recv_frame(socket.as_raw_fd()) {
                Ok((frame, fds)) => match bincode::deserialize::<ServerMessage>(&frame) {
                    Ok(message) => handle_message(socket, message, fds, &mut pending),
                    Err(_) => std::process::exit(1),
                },
                // The compositor is gone.
                Err(FrameError::Eof) => std::process::exit(0),
                Err(FrameError::Truncated) | Err(FrameError::Io(_)) => std::process::exit(1),
            }
        }
        resolve_finished_spawns(socket, &mut pending, &ready_pipes);
    }
}

fn handle_message(
    socket: &OwnedFd,
    message: ServerMessage,
    fds: Vec<OwnedFd>,
    pending: &mut Vec<PendingSpawn>,
) {
    match message {
        ServerMessage::SetEnv { var, val } => {
            let var = OsString::from_vec(var);
            match val {
                Some(val) => std::env::set_var(&var, OsString::from_vec(val)),
                None => std::env::remove_var(&var),
            }
        }
        ServerMessage::Spawn { token, prog, argv, env, fd_slots } => {
            if fds.len() != fd_slots.len() {
                reply(
                    socket,
                    ForkerMessage::SpawnFailed {
                        token,
                        msg: "spawn frame carried the wrong number of fds".into(),
                    },
                );
                return;
            }
            if fd_slots.iter().any(|slot| *slot < 0) {
                reply(
                    socket,
                    ForkerMessage::SpawnFailed {
                        token,
                        msg: "fd slots must be non-negative".into(),
                    },
                );
                return;
            }
            match start_child(token, prog, argv, env, fd_slots, fds) {
                Ok(spawn) => pending.push(spawn),
                Err(msg) => reply(socket, ForkerMessage::SpawnFailed { token, msg }),
            }
        }
    }
}

fn reply(socket: &OwnedFd, message: ForkerMessage) {
    let frame = bincode::serialize(&message).expect("forker reply serializes");
    let _ = send_frame(socket.as_raw_fd(), &frame, &[]);
}

/// Fork the child and queue its exec confirmation pipe. The child either
/// execs (empty pipe, end-of-file) or writes an error message and exits.
fn start_child(
    token: u64,
    prog: String,
    argv: Vec<String>,
    env: Vec<(String, Option<String>)>,
    fd_slots: Vec<RawFd>,
    fds: Vec<OwnedFd>,
) -> Result<PendingSpawn, String> {
    let slots: Vec<(RawFd, OwnedFd)> = fd_slots.into_iter().zip(fds).collect();
    let (pipe, write_end) =
        owned_pipe2(OFlag::O_CLOEXEC).map_err(|error| format!("pipe: {error}"))?;
    set_nonblocking(pipe.as_raw_fd()).map_err(|error| format!("pipe flags: {error}"))?;
    let forked = unsafe { fork() }.map_err(|error| format!("fork: {}", io::Error::from(error)))?;
    match forked {
        ForkResult::Child => {
            // The child; never returns.
            drop(pipe);
            run_spawned_child(FORKER_SOCKET_FD, write_end.as_raw_fd(), prog, argv, env, slots);
        }
        ForkResult::Parent { child: pid } => {
            drop(write_end);
            Ok(PendingSpawn { pipe, token, pid })
        }
    }
}

/// Consume the exec-error pipes that became readable. An empty pipe means the
/// child exec'd: open a pidfd for it and hand it to the compositor. A message
/// in the pipe explains why the spawn failed.
fn resolve_finished_spawns(socket: &OwnedFd, pending: &mut Vec<PendingSpawn>, ready: &[usize]) {
    for index in ready.iter().rev() {
        let spawn = pending.swap_remove(*index);
        let mut chunk = [0u8; 256];
        let mut error = Vec::new();
        loop {
            match read(spawn.pipe.as_raw_fd(), &mut chunk) {
                Ok(nbytes) if nbytes > 0 => {
                    error.extend_from_slice(&chunk[..nbytes]);
                }
                _ => break,
            }
        }
        let response = if error.is_empty() {
            match pidfd_open(spawn.pid) {
                Ok(pidfd) => {
                    let raw = pidfd.as_raw_fd();
                    let message = ForkerMessage::Spawned {
                        token: spawn.token,
                        pid: spawn.pid.as_raw() as u32,
                    };
                    let frame = bincode::serialize(&message).expect("forker reply serializes");
                    let _ = send_frame(socket.as_raw_fd(), &frame, &[raw]);
                    // The kernel keeps the pidfd alive in the message; our
                    // copy closes here.
                    drop(pidfd);
                    continue;
                }
                Err(io_error) => ForkerMessage::SpawnFailed {
                    token: spawn.token,
                    msg: format!("could not open a pidfd for the child: {io_error}"),
                },
            }
        } else {
            ForkerMessage::SpawnFailed {
                token: spawn.token,
                msg: String::from_utf8_lossy(&error).trim().to_string(),
            }
        };
        reply(socket, response);
    }
}

/// The spawned child, between `fork` and `exec`. Runs to `exec` or reports an
/// error through the exec-error pipe and exits.
fn run_spawned_child(
    socket: RawFd,
    err_pipe: RawFd,
    prog: String,
    argv: Vec<String>,
    env: Vec<(String, Option<String>)>,
    fds: Vec<(RawFd, OwnedFd)>,
) -> ! {
    // Keep the error pipe away from the slot numbers used below: a dup3 to a
    // slot can close whatever currently holds that number.
    let max_slot = fds.iter().map(|(slot, _)| *slot).max().unwrap_or(2);
    let err_dup = match dupfd_cloexec(err_pipe, max_slot + 1) {
        Ok(fd) => fd,
        Err(error) => {
            let message = format!("could not protect the error pipe: {error}\n");
            let _ = write(err_pipe, message.as_bytes());
            exit_now(1);
        }
    };
    let result: Result<(), String> = (|| {
        let placed = map_fds(fds)?;
        // Slots must survive exec.
        for fd in &placed {
            let flags =
                FdFlag::from_bits(fcntl(fd.as_raw_fd(), FcntlArg::F_GETFD).map_err(|error| {
                    format!("could not read fd flags: {}", io::Error::from(error))
                })?)
                .unwrap_or(FdFlag::empty());
            fcntl(fd.as_raw_fd(), FcntlArg::F_SETFD(flags & !FdFlag::FD_CLOEXEC)).map_err(
                |error| format!("could not clear O_CLOEXEC: {}", io::Error::from(error)),
            )?;
        }
        // No forker fds leak into the child.
        let _ = close(socket);
        // SIGCHLD's ignored disposition is inherited across exec: reset it so
        // the program can collect its own children.
        unsafe { signal(Signal::SIGCHLD, SigHandler::SigDfl) }
            .map_err(|error| format!("could not reset SIGCHLD: {}", io::Error::from(error)))?;
        // Own process group so terminal signals do not reach the spawned
        // program from the compositor's session.
        let _ = nix::unistd::setpgid(Pid::from_raw(0), Pid::from_raw(0));
        for (key, val) in env {
            match val {
                Some(val) => std::env::set_var(&key, &val),
                None => std::env::remove_var(&key),
            }
        }
        let mut args = vec![CString::new(prog.as_str())
            .map_err(|_| format!("program name contains a NUL byte: {prog}"))?];
        for arg in argv {
            args.push(
                CString::new(arg.as_str())
                    .map_err(|_| format!("argument contains a NUL byte: {arg}"))?,
            );
        }
        match execvp(&args[0], &args) {
            Ok(never) => match never {},
            Err(errno) => Err(format!("could not exec `{prog}`: {}", io::Error::from(errno))),
        }
    })();
    if let Err(message) = result {
        let message = format!("{message}\n");
        let mut left = message.as_bytes();
        while !left.is_empty() {
            match write(err_dup, left) {
                Ok(written) => left = &left[written..],
                Err(_) => break,
            }
        }
    }
    exit_now(1)
}

/// Place each fd at its requested slot number with `dup3`, closing the
/// received original. Slot numbers may collide with other fds' current
/// positions, so an fd occupying another fd's target is first moved out of
/// the way. Targets are filled highest first, which cannot displace a fd
/// still waiting for a lower slot.
fn map_fds(fds: Vec<(RawFd, OwnedFd)>) -> Result<Vec<OwnedFd>, String> {
    let mut targets: Vec<RawFd> = fds.iter().map(|(slot, _)| *slot).collect();
    targets.sort_unstable_by(|a, b| b.cmp(a));

    let mut current_to_target: BTreeMap<RawFd, RawFd> = fds
        .iter()
        .map(|(slot, fd)| (fd.as_raw_fd(), *slot))
        .collect();
    let mut target_to_current: BTreeMap<RawFd, OwnedFd> =
        fds.into_iter().map(|(slot, fd)| (slot, fd)).collect();

    for target in targets {
        let source = target_to_current[&target].as_raw_fd();
        if source == target {
            continue;
        }
        if let Some(occupant_target) = current_to_target.get(&target).copied() {
            // The number `target` is currently held by another fd; move it to
            // a fresh number first. The copy keeps `O_CLOEXEC` and is placed
            // at its own slot in a later iteration.
            let occupant = target_to_current
                .remove(&occupant_target)
                .expect("occupant is tracked");
            let moved = dupfd_cloexec(target, 0)
                .map_err(|error| format!("could not move an fd out of slot {target}: {error}"))?;
            current_to_target.remove(&target);
            current_to_target.insert(moved, occupant_target);
            target_to_current.insert(occupant_target, unsafe { OwnedFd::from_raw_fd(moved) });
            drop(occupant);
        }
        let source_fd = target_to_current
            .remove(&target)
            .expect("source is tracked");
        let placed = dup3(source_fd.as_raw_fd(), target, OFlag::empty()).map_err(|error| {
            format!("could not place an fd at slot {target}: {}", io::Error::from(error))
        })?;
        debug_assert_eq!(placed, target);
        current_to_target.remove(&source_fd.as_raw_fd());
        current_to_target.insert(target, target);
        target_to_current.insert(target, unsafe { OwnedFd::from_raw_fd(target) });
        drop(source_fd);
    }
    Ok(target_to_current.into_values().collect())
}

#[cfg(test)]
mod tests {
    use std::io::Read;

    use super::*;

    fn poll_readable(fd: &impl AsFd, timeout_ms: libc::c_int) -> bool {
        let mut polls = [PollFd::new(fd, PollFlags::POLLIN)];
        poll(&mut polls, timeout_ms) == Ok(1)
    }

    fn send_server(socket: RawFd, message: &ServerMessage, fds: &[RawFd]) {
        let frame = bincode::serialize(message).unwrap();
        send_frame(socket, &frame, fds).unwrap();
    }

    fn recv_forker(socket: RawFd) -> (ForkerMessage, Vec<OwnedFd>) {
        let (frame, fds) = match recv_frame(socket) {
            Ok(result) => result,
            Err(error) => panic!("could not read a forker reply: {}", io::Error::from(error)),
        };
        (bincode::deserialize(&frame).unwrap(), fds)
    }

    fn spawn_request(
        token: u64,
        prog: &str,
        argv: &[&str],
        env: Vec<(String, Option<String>)>,
        fd_slots: &[RawFd],
    ) -> ServerMessage {
        ServerMessage::Spawn {
            token,
            prog: prog.to_string(),
            argv: argv.iter().map(|arg| arg.to_string()).collect(),
            env,
            fd_slots: fd_slots.to_vec(),
        }
    }

    #[test]
    fn messages_round_trip() {
        let messages = [
            ServerMessage::SetEnv { var: b"DISPLAY".to_vec(), val: Some(b":3".to_vec()) },
            ServerMessage::SetEnv { var: b"DISPLAY".to_vec(), val: None },
            spawn_request(
                1,
                "prog",
                &["a", "b"],
                vec![("KEY".into(), Some("value".into()))],
                &[3, 4],
            ),
        ];
        for message in &messages {
            let frame = bincode::serialize(message).unwrap();
            let back: ServerMessage = bincode::deserialize(&frame).unwrap();
            assert_eq!(bincode::serialize(&back).unwrap(), frame);
        }
        let replies = [
            ForkerMessage::Spawned { token: 1, pid: 42 },
            ForkerMessage::SpawnFailed { token: 1, msg: "boom".into() },
        ];
        for reply in &replies {
            let frame = bincode::serialize(reply).unwrap();
            let back: ForkerMessage = bincode::deserialize(&frame).unwrap();
            assert_eq!(bincode::serialize(&back).unwrap(), frame);
        }
    }

    #[test]
    fn map_fds_places_fds_at_requested_slots() {
        let (r1, w1) = owned_pipe2(OFlag::O_CLOEXEC).unwrap();
        let (r2, w2) = owned_pipe2(OFlag::O_CLOEXEC).unwrap();
        // Make the two fds swap places: each one's target slot is the other's
        // current number, which exercises the move-an-occupant-out-of-the-way
        // path without pinning arbitrary fd numbers that could belong to
        // other tests running in parallel.
        let a = w1.as_raw_fd();
        let b = r2.as_raw_fd();
        let placed = map_fds(vec![(b, w1), (a, r2)]).unwrap();
        assert_eq!(placed.len(), 2);
        // `b` is now the first pipe's write end.
        assert_eq!(write(b, b"A").unwrap(), 1);
        let mut byte = [0u8; 1];
        assert_eq!(read(r1.as_raw_fd(), &mut byte).unwrap(), 1);
        assert_eq!(&byte, b"A");
        // `a` is now the second pipe's read end.
        assert_eq!(write(w2.as_raw_fd(), b"B").unwrap(), 1);
        assert_eq!(read(a, &mut byte).unwrap(), 1);
        assert_eq!(&byte, b"B");
        drop(placed);
        drop((r1, w2));
    }

    #[test]
    fn install_spawns_a_process_and_hands_back_its_pidfd() {
        let proxy = install().expect("the forker installs");
        send_server(
            proxy.socket().as_raw_fd(),
            &spawn_request(7, "/bin/true", &[], vec![], &[]),
            &[],
        );
        let (reply, fds) = recv_forker(proxy.socket().as_raw_fd());
        match reply {
            ForkerMessage::Spawned { token, pid } => {
                assert_eq!(token, 7);
                assert!(pid > 0);
                assert_eq!(fds.len(), 1);
                assert!(poll_readable(&fds[0], 5000), "the child did not exit");
            }
            other => panic!("unexpected reply: {other:?}"),
        }
    }

    #[test]
    fn spawn_failure_is_reported() {
        let proxy = install().unwrap();
        send_server(
            proxy.socket().as_raw_fd(),
            &spawn_request(3, "definitely-not-a-real-program", &[], vec![], &[]),
            &[],
        );
        let (reply, fds) = recv_forker(proxy.socket().as_raw_fd());
        match reply {
            ForkerMessage::SpawnFailed { token, msg } => {
                assert_eq!(token, 3);
                assert!(msg.contains("definitely-not-a-real-program"));
            }
            other => panic!("unexpected reply: {other:?}"),
        }
        assert!(fds.is_empty());
    }

    #[test]
    fn environment_changes_reach_children() {
        let proxy = install().unwrap();
        send_server(
            proxy.socket().as_raw_fd(),
            &ServerMessage::SetEnv {
                var: b"MY_TEST_ENV".to_vec(),
                val: Some(b"forker-value".to_vec()),
            },
            &[],
        );
        let (read_end, write_end) = owned_pipe2(OFlag::O_CLOEXEC).unwrap();
        let message = spawn_request(
            11,
            "/usr/bin/env",
            &[],
            vec![("SPAWN_ONLY".into(), Some("spawn-value".into()))],
            &[1],
        );
        send_server(proxy.socket().as_raw_fd(), &message, &[write_end.as_raw_fd()]);
        drop(write_end);
        let (reply, _) = recv_forker(proxy.socket().as_raw_fd());
        assert!(matches!(reply, ForkerMessage::Spawned { token: 11, .. }));
        let mut output = String::new();
        std::fs::File::from(read_end)
            .read_to_string(&mut output)
            .unwrap();
        assert!(output.contains("MY_TEST_ENV=forker-value"));
        assert!(output.contains("SPAWN_ONLY=spawn-value"));
    }

    #[test]
    fn closing_the_socket_stops_the_forker() {
        let proxy = install().unwrap();
        let forker_pidfd =
            unsafe { OwnedFd::from_raw_fd(dupfd_cloexec(proxy.pidfd().as_raw_fd(), 0).unwrap()) };
        drop(proxy);
        assert!(poll_readable(&forker_pidfd, 5000), "the forker did not exit");
    }
}
