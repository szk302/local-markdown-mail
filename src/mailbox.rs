//! Data directory layout (§4.2), work directories and their cleanup (§6.4).

use std::ffi::OsStr;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::error::{IoContext, LmmError, Result, warn};
use crate::lock::{self, Guard, Kind, Mode, Probe};

/// Paths inside the data directory.
#[derive(Debug, Clone)]
pub struct Home {
    pub root: PathBuf,
}

impl Home {
    pub fn new(root: PathBuf) -> Self {
        Home { root }
    }

    pub fn boxes(&self) -> PathBuf {
        self.root.join("boxes")
    }

    pub fn mailbox(&self, addr: &str) -> PathBuf {
        self.boxes().join(addr)
    }

    pub fn inbox(&self, addr: &str) -> PathBuf {
        self.mailbox(addr).join("inbox")
    }

    pub fn new_dir(&self, addr: &str) -> PathBuf {
        self.inbox(addr).join("new")
    }

    pub fn cur_dir(&self, addr: &str) -> PathBuf {
        self.inbox(addr).join("cur")
    }

    pub fn failed_dir(&self, addr: &str) -> PathBuf {
        self.inbox(addr).join("failed")
    }

    pub fn tmp_dir(&self, addr: &str) -> PathBuf {
        self.inbox(addr).join(".tmp")
    }

    pub fn sent_dir(&self, addr: &str) -> PathBuf {
        self.mailbox(addr).join("sent")
    }

    /// A mailbox exists when `boxes/<addr>` is a directory (§3.1).
    pub fn exists(&self, addr: &str) -> bool {
        fs::symlink_metadata(self.mailbox(addr)).is_ok_and(|m| m.is_dir())
    }

    /// Error used when a mailbox doesn't exist (§3.2, §6.4).
    pub fn missing_error(&self, addr: &str, role: &str) -> LmmError {
        LmmError::runtime(format!(
            "{role} mailbox {addr:?} does not exist ({}); create it with `lmm box create {addr}`",
            self.mailbox(addr).display()
        ))
    }

    pub fn require(&self, addr: &str, role: &str) -> Result<()> {
        if self.exists(addr) {
            Ok(())
        } else {
            Err(self.missing_error(addr, role))
        }
    }

    pub fn lock(&self, kind: Kind, addr: &str, mode: Mode) -> Result<Guard> {
        lock::acquire(&self.root, kind, addr, mode)
    }
}

/// A work directory `inbox/.tmp/<UUID>/` holding `lock` and `msg/`.
/// Removed (with the lock held) on drop unless already removed.
pub struct WorkDir {
    path: PathBuf,
    guard: Option<Guard>,
    removed: bool,
}

impl WorkDir {
    /// Create a work directory in `addr`'s `.tmp` (§6.4 step 4).
    pub fn create(home: &Home, addr: &str) -> Result<WorkDir> {
        let tmp = home.tmp_dir(addr);
        let prepare = home.lock(Kind::Prepare, addr, Mode::Shared)?;
        if !fs::symlink_metadata(&tmp).is_ok_and(|m| m.is_dir()) {
            return Err(LmmError::runtime(format!(
                "{} does not exist (mailbox {addr:?} removed?)",
                tmp.display()
            )));
        }
        let path = tmp.join(uuid::Uuid::new_v4().hyphenated().to_string());
        fs::create_dir(&path).ctx(|| format!("cannot create {}", path.display()))?;
        let mut wd = WorkDir {
            path,
            guard: None,
            removed: false,
        };
        let guard = lock::create_workdir_lock(&wd.path.join("lock"))
            .ctx(|| format!("cannot create lock in {}", wd.path.display()))?;
        wd.guard = Some(guard);
        drop(prepare);
        fs::create_dir(wd.msg()).ctx(|| format!("cannot create {}", wd.msg().display()))?;
        Ok(wd)
    }

    pub fn msg(&self) -> PathBuf {
        self.path.join("msg")
    }

    /// Remove the work directory while holding its lock.
    pub fn remove(mut self) -> io::Result<()> {
        self.removed = true;
        let r = fs::remove_dir_all(&self.path);
        self.guard = None;
        r
    }
}

impl Drop for WorkDir {
    fn drop(&mut self) {
        if !self.removed {
            if let Err(e) = crate::fsutil::remove_dir_all_if_exists(&self.path) {
                warn(format!(
                    "cannot remove work directory {}: {e}",
                    self.path.display()
                ));
            }
            self.guard = None;
        }
    }
}

/// Remove abandoned work directories in `addr`'s `.tmp` (§6.4 cleanup).
/// Errors are reported as warnings.
pub fn cleanup_abandoned(home: &Home, addr: &str) {
    let prepare = match lock::try_acquire(&home.root, Kind::Prepare, addr, Mode::Exclusive) {
        Ok(Some(g)) => g,
        Ok(None) => return, // a send is preparing; skip this time
        Err(e) => {
            warn(format!("cleanup skipped: {e}"));
            return;
        }
    };
    let tmp = home.tmp_dir(addr);
    let mut abandoned: Vec<(PathBuf, Option<Guard>)> = Vec::new();
    match fs::read_dir(&tmp) {
        Ok(rd) => {
            for entry in rd.flatten() {
                let p = entry.path();
                if !entry.file_type().is_ok_and(|t| t.is_dir()) {
                    continue;
                }
                match lock::probe_workdir_lock(&p.join("lock")) {
                    Ok(Probe::Missing) => abandoned.push((p, None)),
                    Ok(Probe::Acquired(g)) => abandoned.push((p, Some(g))),
                    Ok(Probe::Busy) => {}
                    Err(e) => warn(format!("cannot probe {}: {e}", p.display())),
                }
            }
        }
        Err(e) => {
            warn(format!("cannot read {}: {e}", tmp.display()));
            return;
        }
    }
    drop(prepare);
    for (p, guard) in abandoned {
        if let Err(e) = fs::remove_dir_all(&p) {
            warn(format!(
                "cannot remove abandoned work directory {}: {e}",
                p.display()
            ));
        }
        drop(guard);
    }
}

/// Probe all work directories of a mailbox without waiting.
/// Returns (live count, guards of abandoned ones).
pub fn probe_workdirs(home: &Home, addr: &str) -> Result<(usize, Vec<Guard>)> {
    let tmp = home.tmp_dir(addr);
    let mut live = 0;
    let mut guards = Vec::new();
    let rd = match fs::read_dir(&tmp) {
        Ok(rd) => rd,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok((0, guards)),
        Err(e) => {
            return Err(LmmError::runtime(format!(
                "cannot read {}: {e}",
                tmp.display()
            )));
        }
    };
    for entry in rd {
        let entry = entry.ctx(|| format!("cannot read {}", tmp.display()))?;
        if !entry.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        match lock::probe_workdir_lock(&entry.path().join("lock")) {
            Ok(Probe::Busy) => live += 1,
            Ok(Probe::Acquired(g)) => guards.push(g),
            Ok(Probe::Missing) => {}
            Err(e) => {
                return Err(LmmError::runtime(format!(
                    "cannot probe {}: {e}",
                    entry.path().display()
                )));
            }
        }
    }
    Ok((live, guards))
}

/// Sorted entry names of a directory (hidden entries excluded).
pub fn sorted_entries(dir: &Path) -> io::Result<Vec<std::ffi::OsString>> {
    let mut names = Vec::new();
    for e in fs::read_dir(dir)? {
        let name = e?.file_name();
        if name.as_encoded_bytes().first() == Some(&b'.') {
            continue;
        }
        names.push(name);
    }
    names.sort();
    Ok(names)
}

/// Whether `name` exists in `dir` (without following symlinks).
pub fn name_exists(dir: &Path, name: &OsStr) -> bool {
    fs::symlink_metadata(dir.join(name)).is_ok()
}
