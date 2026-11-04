//! X socket allocation for Xwayland.
//!
//! Claiming an X display means two things: creating a lock file
//! `/tmp/.X{n}-lock` (holding the owner pid, right-aligned in 10 columns,
//! newline-terminated) and binding a unix socket at `/tmp/.X11-unix/X{n}`.
//!
//! The socket is bound but **not listening**. The caller decides when to
//! `listen(2)`: the lazy Xwayland trigger listens on it, and later hands the
//! same fd to Xwayland via `-listenfd`, which accepts on an already-listening
//! socket.

use std::{
    io::{self, Write},
    ops::RangeInclusive,
    os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd},
    path::{Path, PathBuf},
};

use nix::{
    errno::Errno,
    fcntl::{open, OFlag},
    sys::{
        signal::kill,
        socket::{bind, socket, AddressFamily, SockFlag, SockType, UnixAddr},
        stat::Mode,
    },
    unistd::Pid,
};

/// Where X clients look for the per-display unix socket.
pub const X_SOCKET_DIR: &str = "/tmp/.X11-unix";
/// Where the per-display lock files live.
pub const X_LOCK_DIR: &str = "/tmp";
/// X displays are searched in this range. The high start avoids colliding
/// with a real X server on `:0`.
pub const DISPLAY_RANGE: RangeInclusive<u32> = 500..=1499;

/// Lock files are read-only so nobody scribbles over them afterwards.
const LOCK_MODE: u32 = 0o444;

#[derive(Debug, thiserror::Error)]
pub enum XSocketError {
    #[error("could not create the X11 socket directory {}: {source}", path.display())]
    CreateSocketDir {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("the X11 socket directory {} is not a directory", .0.display())]
    SocketDirNotADirectory(PathBuf),
    #[error("could not create a unix socket: {source}")]
    SocketCreate {
        #[source]
        source: io::Error,
    },
    #[error("could not create X lock file {}: {source}", path.display())]
    CreateLock {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("could not bind X socket {}: {source}", path.display())]
    Bind {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("could not write X lock file {}: {source}", path.display())]
    WriteLock {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("all X display numbers in the search range are in use")]
    AddressesInUse,
}

/// One claimed X display: a lock file, a bound (not listening) socket, and a
/// `Drop` that releases both.
#[derive(Debug)]
pub struct XSocket {
    pub id: u32,
    socket_path: PathBuf,
    lock_path: PathBuf,
    fd: OwnedFd,
}

impl Drop for XSocket {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.socket_path);
        let _ = std::fs::remove_file(&self.lock_path);
    }
}

impl XSocket {
    /// The `DISPLAY` value for this socket, e.g. `:503`.
    pub fn display_name(&self) -> String {
        format!(":{}", self.id)
    }

    /// The bound socket. The caller is responsible for `listen(2)`.
    pub fn fd(&self) -> BorrowedFd<'_> {
        self.fd.as_fd()
    }
}

/// Claim a free X display under `/tmp`.
pub fn allocate() -> Result<XSocket, XSocketError> {
    allocate_in(Path::new(X_SOCKET_DIR), Path::new(X_LOCK_DIR), DISPLAY_RANGE)
}

/// Claim a free display number, following the X display conventions: create
/// the lock file, then bind the socket, then write our pid into the lock.
///
/// Stale locks (dead pid) are reclaimed; live ones are skipped. The socket
/// and lock directories are parameters so that tests can run against
/// temporary directories instead of the real `/tmp/.X11-unix`.
pub fn allocate_in(
    socket_dir: &Path,
    lock_dir: &Path,
    range: RangeInclusive<u32>,
) -> Result<XSocket, XSocketError> {
    ensure_socket_dir(socket_dir)?;
    for id in range {
        match claim_display(socket_dir, lock_dir, id) {
            Ok(socket) => return Ok(socket),
            Err(ClaimError::InUse) => {}
            Err(ClaimError::Fatal(error)) => return Err(error),
        }
    }
    Err(XSocketError::AddressesInUse)
}

enum ClaimError {
    /// The display number is taken (or unusable); try the next one.
    InUse,
    /// Something is broken system-wide; do not keep searching.
    Fatal(XSocketError),
}

fn ensure_socket_dir(path: &Path) -> Result<(), XSocketError> {
    match std::fs::create_dir(path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(source) => return Err(XSocketError::CreateSocketDir { path: path.into(), source }),
    }
    match std::fs::metadata(path) {
        Ok(metadata) if metadata.is_dir() => Ok(()),
        _ => Err(XSocketError::SocketDirNotADirectory(path.into())),
    }
}

fn claim_display(socket_dir: &Path, lock_dir: &Path, id: u32) -> Result<XSocket, ClaimError> {
    let lock_path = lock_dir.join(format!(".X{id}-lock"));
    let Ok(Some(lock_fd)) = acquire_lock(&lock_path) else {
        return Err(ClaimError::InUse);
    };

    let socket_path = socket_dir.join(format!("X{id}"));
    // A leftover socket file would make `bind` fail even though the display
    // is free.
    let _ = std::fs::remove_file(&socket_path);
    let fd = match bind_socket(&socket_path) {
        Ok(fd) => fd,
        Err(source) => {
            // Bind failures for one display are not fatal: release the lock
            // and try the next number.
            let _ = std::fs::remove_file(&lock_path);
            log::warn!("could not bind X socket {}: {source}", socket_path.display());
            return Err(ClaimError::Fatal(XSocketError::Bind { path: socket_path, source }));
        }
    };

    // The lock file is visible (but empty) from creation until now; the pid
    // is written last, matching the X display conventions.
    let pid = format!("{:10}\n", std::process::id());
    let mut lock_file = std::fs::File::from(lock_fd);
    if let Err(source) = lock_file.write_all(pid.as_bytes()) {
        let _ = std::fs::remove_file(&lock_path);
        let _ = std::fs::remove_file(&socket_path);
        return Err(ClaimError::Fatal(XSocketError::WriteLock { path: lock_path, source }));
    }
    drop(lock_file);

    log::debug!("claimed X display :{id}");
    Ok(XSocket { id, socket_path, lock_path, fd })
}

/// Create the lock file, or reclaim it if it belongs to a dead process.
/// Returns `Ok(None)` when the display is in use.
fn acquire_lock(path: &Path) -> Result<Option<OwnedFd>, ClaimError> {
    let mode = Mode::from_bits(LOCK_MODE).expect("valid mode");
    for _ in 0..2 {
        let oflags = OFlag::O_CREAT | OFlag::O_EXCL | OFlag::O_WRONLY | OFlag::O_CLOEXEC;
        match open(path, oflags, mode) {
            Ok(fd) => return Ok(Some(unsafe { OwnedFd::from_raw_fd(fd) })),
            Err(Errno::EEXIST) => {}
            Err(errno) => {
                return Err(ClaimError::Fatal(XSocketError::CreateLock {
                    path: path.into(),
                    source: errno.into(),
                }));
            }
        }

        let Some(pid) = read_lock_pid(path) else {
            log::warn!("unparsable X lock file {}; treating display as in use", path.display());
            return Ok(None);
        };
        match kill(Pid::from_raw(pid), None) {
            Err(Errno::ESRCH) => {
                // Stale: remove it and retry the same display once.
                let _ = std::fs::remove_file(path);
            }
            _ => return Ok(None), // alive, or owned by someone else
        }
    }
    // The retry lost the race for the lock.
    Ok(None)
}

fn read_lock_pid(path: &Path) -> Option<i32> {
    let content = std::fs::read_to_string(path).ok()?;
    let pid = content.trim().parse::<i32>().ok()?;
    (pid > 0).then_some(pid)
}

fn bind_socket(path: &Path) -> io::Result<OwnedFd> {
    let addr = UnixAddr::new(path)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid unix socket path"))?;
    // The fd is bound but not listening: `listen(2)` stays with the caller.
    let fd = socket(AddressFamily::Unix, SockType::Stream, SockFlag::SOCK_CLOEXEC, None)?;
    bind(fd.as_raw_fd(), &addr)?;
    Ok(fd)
}

#[cfg(test)]
mod tests {
    use std::{
        io::ErrorKind,
        os::unix::fs::{FileTypeExt, PermissionsExt},
        path::PathBuf,
    };

    use super::*;

    fn temp_dir() -> (mktemp::Temp, PathBuf) {
        let dir = mktemp::Temp::new_dir().unwrap();
        let path = dir.to_path_buf();
        (dir, path)
    }

    fn test_range() -> RangeInclusive<u32> {
        10..=15
    }

    /// A pid that is guaranteed not to exist right now.
    ///
    /// The kernel hands out the lowest free pid at or above the last one it
    /// allocated, so scanning upward would return exactly the pid the next
    /// spawned test thread receives. Scanning downward stays below the
    /// allocation high-water mark, where a dead pid cannot be reused until
    /// the whole pid space wraps around.
    fn dead_pid() -> i32 {
        let mut pid = std::process::id() as i32 - 2;
        while pid > 0 {
            match kill(Pid::from_raw(pid), None) {
                Err(Errno::ESRCH) => return pid,
                _ => pid -= 1,
            }
        }
        panic!("could not find a free pid below {}", std::process::id());
    }

    fn socket_path(dir: &Path, id: u32) -> PathBuf {
        dir.join(format!("X{id}"))
    }

    fn lock_path(dir: &Path, id: u32) -> PathBuf {
        dir.join(format!(".X{id}-lock"))
    }

    #[test]
    fn allocation_claims_the_first_free_display() {
        let (_dir, tmp) = temp_dir();
        let xsocket = allocate_in(&tmp, &tmp, test_range()).unwrap();
        assert_eq!(xsocket.id, 10);
        assert_eq!(xsocket.display_name(), ":10");

        // The lock file holds our pid, right-aligned in 10 columns.
        let content = std::fs::read_to_string(lock_path(&tmp, 10)).unwrap();
        assert_eq!(content, format!("{:10}\n", std::process::id()));
        let metadata = std::fs::metadata(lock_path(&tmp, 10)).unwrap();
        assert_eq!(metadata.permissions().mode() & 0o777, 0o444);

        // The socket file exists and is a socket.
        let metadata = std::fs::metadata(socket_path(&tmp, 10)).unwrap();
        assert!(metadata.file_type().is_socket());
    }

    #[test]
    fn the_socket_is_bound_but_not_listening() {
        use std::os::unix::net::UnixStream;

        let (_dir, tmp) = temp_dir();
        let xsocket = allocate_in(&tmp, &tmp, test_range()).unwrap();
        // No `listen(2)` yet: connects are refused instead of being queued
        // for the future Xwayland to accept.
        let error = UnixStream::connect(socket_path(&tmp, xsocket.id)).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::ConnectionRefused);
    }

    #[test]
    fn drop_releases_the_lock_and_the_socket() {
        let (_dir, tmp) = temp_dir();
        let xsocket = allocate_in(&tmp, &tmp, test_range()).unwrap();
        let socket_path = xsocket.socket_path.clone();
        let lock_path = xsocket.lock_path.clone();
        assert!(socket_path.exists());
        assert!(lock_path.exists());
        drop(xsocket);
        assert!(!socket_path.exists());
        assert!(!lock_path.exists());
    }

    #[test]
    fn a_stale_lock_with_a_dead_pid_is_reclaimed() {
        let (_dir, tmp) = temp_dir();
        std::fs::write(lock_path(&tmp, 10), format!("{:10}\n", dead_pid())).unwrap();
        let xsocket = allocate_in(&tmp, &tmp, test_range()).unwrap();
        assert_eq!(xsocket.id, 10);
        // The lock now holds our pid.
        let content = std::fs::read_to_string(lock_path(&tmp, 10)).unwrap();
        assert_eq!(content, format!("{:10}\n", std::process::id()));
    }

    #[test]
    fn a_live_lock_is_skipped() {
        let (_dir, tmp) = temp_dir();
        std::fs::write(lock_path(&tmp, 10), format!("{:10}\n", std::process::id())).unwrap();
        let xsocket = allocate_in(&tmp, &tmp, test_range()).unwrap();
        assert_eq!(xsocket.id, 11);
    }

    #[test]
    fn an_unparsable_lock_is_not_stolen() {
        let (_dir, tmp) = temp_dir();
        std::fs::write(lock_path(&tmp, 10), "not a pid").unwrap();
        let xsocket = allocate_in(&tmp, &tmp, test_range()).unwrap();
        assert_eq!(xsocket.id, 11);
    }

    #[test]
    fn an_exhausted_range_is_an_error() {
        let (_dir, tmp) = temp_dir();
        let error = allocate_in(&tmp, &tmp, RangeInclusive::new(10, 9)).unwrap_err();
        assert!(matches!(error, XSocketError::AddressesInUse));
    }

    #[test]
    fn two_allocations_do_not_collide() {
        let (_dir, tmp) = temp_dir();
        let first = allocate_in(&tmp, &tmp, test_range()).unwrap();
        let second = allocate_in(&tmp, &tmp, test_range()).unwrap();
        assert_ne!(first.id, second.id);
    }
}
