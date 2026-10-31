//! Process signals delivered through `signalfd`.
//!
//! The mask matches Jay's `src/sighand.rs`: `SIGINT` and `SIGTERM` request
//! shutdown, and `SIGPIPE` is blocked so a client disconnect cannot kill the
//! process. Cleanup stays on the runner thread; nothing runs in a signal handler.

use std::{fmt, os::fd::AsRawFd};

use nix::sys::{
    signal::{SigSet, Signal},
    signalfd::{SfdFlags, SignalFd},
};

use crate::{
    io_uring::{
        model::IoUringAction,
        ring::{PollCompleted, RingId},
    },
    runner::{Dispatcher, Model, Redispatcher, ShutdownReason},
    State,
};

#[derive(Debug)]
pub enum SignalInstallError {
    Block(nix::Error),
    Create(nix::Error),
}

impl fmt::Display for SignalInstallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SignalInstallError::Block(error) => write!(f, "could not block signals: {error}"),
            SignalInstallError::Create(error) => write!(f, "could not create signalfd: {error}"),
        }
    }
}

pub struct SignalHandler {
    fd: SignalFd,
    poll_op_id: Option<RingId>,
}

pub enum SignalAction {
    Start,
    Ready,
    Failed { error: std::io::Error },
    Stop,
}

impl SignalHandler {
    pub fn install() -> Result<Self, SignalInstallError> {
        let mut mask = SigSet::empty();
        mask.add(Signal::SIGINT);
        mask.add(Signal::SIGTERM);
        mask.add(Signal::SIGPIPE);
        mask.thread_block().map_err(SignalInstallError::Block)?;
        let fd = match SignalFd::with_flags(&mask, SfdFlags::SFD_CLOEXEC | SfdFlags::SFD_NONBLOCK) {
            Ok(fd) => fd,
            Err(error) => {
                let _ = mask.thread_unblock();
                return Err(SignalInstallError::Create(error));
            }
        };
        Ok(Self { fd, poll_op_id: None })
    }

    fn dispatch_poll(&mut self, dispatcher: &mut Dispatcher) {
        if self.poll_op_id.is_some() {
            return;
        }
        let poll_op_id = RingId::fresh();
        self.poll_op_id = Some(poll_op_id);
        dispatcher.dispatch(IoUringAction::Poll {
            id: Some(poll_op_id),
            fd: self.fd.as_raw_fd(),
            dispatch_to: Redispatcher::to::<Self>(|result: std::io::Result<PollCompleted>| {
                match result {
                    Ok(_) => SignalAction::Ready,
                    Err(error) => SignalAction::Failed { error },
                }
            }),
        });
    }

    fn handle_ready(
        &mut self,
        dispatcher: &mut Dispatcher,
    ) -> std::result::Result<(), crate::runner::ModelFailure> {
        self.poll_op_id = None;
        loop {
            match self.fd.read_signal() {
                Ok(Some(info)) => handle_signo(info.ssi_signo as i32, dispatcher),
                Ok(None) => break,
                Err(error) => {
                    dispatcher.request_shutdown(ShutdownReason::Fatal {
                        source: "signals",
                        message: format!("signalfd read failed: {error}"),
                    });
                    return Ok(());
                }
            }
        }
        self.dispatch_poll(dispatcher);
        Ok(())
    }
}

pub(crate) fn reason_for_signal(signo: i32) -> Option<ShutdownReason> {
    match signo {
        libc::SIGINT | libc::SIGTERM => Some(ShutdownReason::Signal(signo)),
        _ => None,
    }
}

fn handle_signo(signo: i32, dispatcher: &mut Dispatcher) {
    match reason_for_signal(signo) {
        Some(reason) => {
            log::info!("received {reason}");
            dispatcher.request_shutdown(reason);
        }
        None => log::info!("ignoring signal {signo}"),
    }
}

impl Model for SignalHandler {
    type Action = SignalAction;
    const START: Option<Self::Action> = Some(SignalAction::Start);
    const STOP: Option<Self::Action> = Some(SignalAction::Stop);

    fn process(
        &mut self,
        _state: &mut State,
        action: Self::Action,
        dispatcher: &mut Dispatcher,
    ) -> std::result::Result<(), crate::runner::ModelFailure> {
        match action {
            SignalAction::Start => self.dispatch_poll(dispatcher),
            SignalAction::Ready => return self.handle_ready(dispatcher),
            SignalAction::Failed { error } => {
                dispatcher.request_shutdown(ShutdownReason::Fatal {
                    source: "signals",
                    message: format!("signalfd poll failed: {error}"),
                });
            }
            // Cancelling the outstanding poll belongs to the io_uring drain.
            SignalAction::Stop => self.poll_op_id = None,
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signal_numbers_map_to_shell_exit_codes() {
        assert_eq!(crate::runner::exit_code(&ShutdownReason::Signal(libc::SIGINT)), 130);
        assert_eq!(crate::runner::exit_code(&ShutdownReason::Signal(libc::SIGTERM)), 143);
        assert_eq!(reason_for_signal(libc::SIGPIPE), None);
        assert_eq!(reason_for_signal(libc::SIGTERM), Some(ShutdownReason::Signal(libc::SIGTERM)));
    }

    #[test]
    fn blocked_sigterm_is_readable_from_signalfd() {
        let mut mask = SigSet::empty();
        mask.add(Signal::SIGTERM);
        mask.thread_block().unwrap();
        let mut fd =
            SignalFd::with_flags(&mask, SfdFlags::SFD_CLOEXEC | SfdFlags::SFD_NONBLOCK).unwrap();
        unsafe { libc::raise(libc::SIGTERM) };
        let info = fd.read_signal().unwrap().expect("pending SIGTERM");
        assert_eq!(info.ssi_signo, libc::SIGTERM as u32);
        mask.thread_unblock().unwrap();
    }
}
