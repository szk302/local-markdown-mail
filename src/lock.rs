//! flock(2) based locks (§4.3).

use std::fs::{self, File, OpenOptions, TryLockError};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use crate::error::{LmmError, Result};

/// Default lock wait limit.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);
const POLL: Duration = Duration::from_millis(20);

/// Mailbox lock kinds; each lives in its own directory under `locks/`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Box,
    Receive,
    Prepare,
}

impl Kind {
    fn dir(self) -> &'static str {
        match self {
            Kind::Box => "box",
            Kind::Receive => "receive",
            Kind::Prepare => "prepare",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Kind::Box => "mailbox",
            Kind::Receive => "receive",
            Kind::Prepare => "prepare",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Shared,
    Exclusive,
}

/// A held lock. Dropping it closes the file and releases the lock.
#[derive(Debug)]
pub struct Guard {
    _file: File,
}

/// Path of a mailbox lock file.
pub fn lock_path(data_dir: &Path, kind: Kind, addr: &str) -> PathBuf {
    data_dir
        .join("locks")
        .join(kind.dir())
        .join(format!("{addr}.lock"))
}

fn open_lock_file(path: &Path, create: bool) -> io::Result<File> {
    if create && let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(create)
        .truncate(false)
        .open(path)
}

fn try_once(file: &File, mode: Mode) -> io::Result<bool> {
    let r = match mode {
        Mode::Shared => file.try_lock_shared(),
        Mode::Exclusive => file.try_lock(),
    };
    match r {
        Ok(()) => Ok(true),
        Err(TryLockError::WouldBlock) => Ok(false),
        Err(TryLockError::Error(e)) => Err(e),
    }
}

/// Lock an already opened file, waiting up to `timeout`.
/// `None` timeout means "try once without waiting".
/// Returns `Ok(false)` if the lock could not be obtained.
pub fn lock_file(file: &File, mode: Mode, timeout: Option<Duration>) -> io::Result<bool> {
    Ok(matches!(
        lock_file_until(file, mode, timeout, None)?,
        Wait::Locked
    ))
}

enum Wait {
    Locked,
    TimedOut,
    Interrupted(i32),
}

/// Like [`lock_file`], but gives up as soon as `stop` holds a non-zero
/// signal number.
fn lock_file_until(
    file: &File,
    mode: Mode,
    timeout: Option<Duration>,
    stop: Option<&AtomicUsize>,
) -> io::Result<Wait> {
    let deadline = timeout.and_then(|t| Instant::now().checked_add(t));
    loop {
        if try_once(file, mode)? {
            return Ok(Wait::Locked);
        }
        if let Some(s) = stop.map(|s| s.load(Ordering::SeqCst)).filter(|&s| s != 0) {
            return Ok(Wait::Interrupted(s as i32));
        }
        match (timeout, deadline) {
            (None, _) => return Ok(Wait::TimedOut),
            (Some(_), Some(d)) if Instant::now() >= d => return Ok(Wait::TimedOut),
            _ => thread::sleep(POLL),
        }
    }
}

/// Acquire a mailbox lock, waiting up to `timeout` (error on expiry).
pub fn acquire_with(
    data_dir: &Path,
    kind: Kind,
    addr: &str,
    mode: Mode,
    timeout: Duration,
) -> Result<Guard> {
    acquire_interruptible(data_dir, kind, addr, mode, timeout, None)
}

/// Acquire a mailbox lock, waiting up to `timeout`. While waiting, a
/// non-zero signal number in `stop` aborts with [`LmmError::Signal`]
/// (`lmm wait`, §7.4).
pub fn acquire_interruptible(
    data_dir: &Path,
    kind: Kind,
    addr: &str,
    mode: Mode,
    timeout: Duration,
    stop: Option<&AtomicUsize>,
) -> Result<Guard> {
    let path = lock_path(data_dir, kind, addr);
    let file = open_lock_file(&path, true)
        .map_err(|e| LmmError::runtime(format!("cannot open lock file {}: {e}", path.display())))?;
    match lock_file_until(&file, mode, Some(timeout), stop) {
        Ok(Wait::Locked) => Ok(Guard { _file: file }),
        Ok(Wait::Interrupted(s)) => Err(LmmError::Signal(s)),
        Ok(Wait::TimedOut) => Err(LmmError::runtime(format!(
            "timed out waiting for the {} lock of {addr} ({})",
            kind.label(),
            path.display()
        ))),
        Err(e) => Err(LmmError::runtime(format!(
            "cannot lock {}: {e}",
            path.display()
        ))),
    }
}

/// Acquire a mailbox lock with the default timeout (10 s).
pub fn acquire(data_dir: &Path, kind: Kind, addr: &str, mode: Mode) -> Result<Guard> {
    acquire_with(data_dir, kind, addr, mode, DEFAULT_TIMEOUT)
}

/// Try a mailbox lock once without waiting. `Ok(None)` if busy.
pub fn try_acquire(data_dir: &Path, kind: Kind, addr: &str, mode: Mode) -> Result<Option<Guard>> {
    let path = lock_path(data_dir, kind, addr);
    let file = open_lock_file(&path, true)
        .map_err(|e| LmmError::runtime(format!("cannot open lock file {}: {e}", path.display())))?;
    match lock_file(&file, mode, None) {
        Ok(true) => Ok(Some(Guard { _file: file })),
        Ok(false) => Ok(None),
        Err(e) => Err(LmmError::runtime(format!(
            "cannot lock {}: {e}",
            path.display()
        ))),
    }
}

/// State of a work directory `lock` file probed without waiting.
pub enum Probe {
    /// No `lock` file (abandoned).
    Missing,
    /// Lock obtained: abandoned. Keep the guard while removing.
    Acquired(Guard),
    /// Held by a live writer.
    Busy,
}

/// Try the exclusive lock of an existing work directory `lock` file once.
pub fn probe_workdir_lock(path: &Path) -> io::Result<Probe> {
    let file = match open_lock_file(path, false) {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Probe::Missing),
        Err(e) => return Err(e),
    };
    if lock_file(&file, Mode::Exclusive, None)? {
        Ok(Probe::Acquired(Guard { _file: file }))
    } else {
        Ok(Probe::Busy)
    }
}

/// Create a new work directory `lock` file and lock it exclusively without
/// waiting (§4.3: a sender never waits for this lock). The only process that
/// can hold it at this point is `lmm box remove` probing work directories of
/// a mailbox being removed, in which case the send fails anyway.
pub fn create_workdir_lock(path: &Path) -> io::Result<Guard> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(path)?;
    if lock_file(&file, Mode::Exclusive, None)? {
        Ok(Guard { _file: file })
    } else {
        Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            "work directory lock is held by another process (mailbox being removed?)",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_locks_coexist_and_exclusive_times_out() {
        let t = tempfile::tempdir().unwrap();
        let data_dir = t.path();
        let a = acquire(data_dir, Kind::Box, "alpha", Mode::Shared).unwrap();
        let b = acquire(data_dir, Kind::Box, "alpha", Mode::Shared).unwrap();
        assert!(
            try_acquire(data_dir, Kind::Box, "alpha", Mode::Exclusive)
                .unwrap()
                .is_none()
        );
        let start = Instant::now();
        let r = acquire_with(
            data_dir,
            Kind::Box,
            "alpha",
            Mode::Exclusive,
            Duration::from_millis(200),
        );
        assert!(r.is_err());
        assert!(start.elapsed() >= Duration::from_millis(200));
        // Different kinds don't interfere.
        assert!(
            try_acquire(data_dir, Kind::Receive, "alpha", Mode::Exclusive)
                .unwrap()
                .is_some()
        );
        drop(a);
        drop(b);
        assert!(
            try_acquire(data_dir, Kind::Box, "alpha", Mode::Exclusive)
                .unwrap()
                .is_some()
        );
        assert!(lock_path(data_dir, Kind::Box, "alpha").exists());
    }

    #[test]
    fn interruptible_acquire_stops_on_signal() {
        let t = tempfile::tempdir().unwrap();
        let data_dir = t.path();
        let _held = acquire(data_dir, Kind::Receive, "a", Mode::Exclusive).unwrap();
        let stop = AtomicUsize::new(0);
        std::thread::scope(|sc| {
            sc.spawn(|| {
                thread::sleep(Duration::from_millis(100));
                stop.store(libc::SIGTERM as usize, Ordering::SeqCst);
            });
            let start = Instant::now();
            let r = acquire_interruptible(
                data_dir,
                Kind::Receive,
                "a",
                Mode::Exclusive,
                DEFAULT_TIMEOUT,
                Some(&stop),
            );
            assert!(matches!(r, Err(LmmError::Signal(s)) if s == libc::SIGTERM));
            assert!(start.elapsed() < Duration::from_secs(2));
        });
    }

    #[test]
    fn workdir_probe() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("lock");
        assert!(matches!(probe_workdir_lock(&p).unwrap(), Probe::Missing));
        let g = create_workdir_lock(&p).unwrap();
        assert!(matches!(probe_workdir_lock(&p).unwrap(), Probe::Busy));
        drop(g);
        assert!(matches!(
            probe_workdir_lock(&p).unwrap(),
            Probe::Acquired(_)
        ));
        // An existing file is never reused.
        assert_eq!(
            create_workdir_lock(&p).unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
    }

    #[test]
    fn workdir_lock_does_not_wait() {
        // Simulate `lmm box remove` grabbing the lock right after creation:
        // the sender must fail immediately instead of waiting.
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("lock");
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&p)
            .unwrap();
        let holder = OpenOptions::new().read(true).write(true).open(&p).unwrap();
        holder.lock().unwrap();
        let start = Instant::now();
        assert!(!lock_file(&file, Mode::Exclusive, None).unwrap());
        assert!(start.elapsed() < Duration::from_secs(1));
    }
}
