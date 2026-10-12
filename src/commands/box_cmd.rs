//! `lmm box` (§9).

use std::fs;
use std::io::Write;

use crate::address;
use crate::error::{IoContext, LmmError, Result, warn};
use crate::fsutil;
use crate::lock::{Kind, Mode};
use crate::mailbox::{self, DataDir};

fn validate(addr: &str) -> Result<()> {
    address::validate(addr).map_err(LmmError::Usage)
}

fn ensure_dir(p: &std::path::Path) -> Result<()> {
    fs::create_dir_all(p).ctx(|| format!("cannot create {}", p.display()))
}

/// `lmm box create` (§9.1).
pub fn create(data_dir: &DataDir, addr: &str, exist_ok: bool) -> Result<()> {
    validate(addr)?;
    let boxes = data_dir.boxes();
    ensure_dir(&boxes)?;
    let _lock = data_dir.lock(Kind::Box, addr, Mode::Exclusive)?;
    if data_dir.exists(addr) {
        if exist_ok {
            return Ok(());
        }
        return Err(LmmError::runtime(format!(
            "mailbox {addr:?} already exists (use --exist-ok to ignore)"
        )));
    }
    let creating = boxes.join(".creating");
    let removing = boxes.join(".removing");
    ensure_dir(&creating)?;
    ensure_dir(&removing)?;
    let staged = creating.join(addr);
    for leftover in [&staged, &removing.join(addr)] {
        fsutil::remove_dir_all_if_exists(leftover)
            .ctx(|| format!("cannot remove leftover {}", leftover.display()))?;
    }
    let inbox = staged.join("inbox");
    let dirs = [
        staged.clone(),
        inbox.clone(),
        inbox.join(".tmp"),
        inbox.join("new"),
        inbox.join("cur"),
        inbox.join("failed"),
        staged.join("sent"),
    ];
    for d in &dirs {
        fs::create_dir(d).ctx(|| format!("cannot create {}", d.display()))?;
    }
    for d in dirs.iter().rev() {
        fsutil::fsync_dir(d).ctx(|| format!("cannot fsync {}", d.display()))?;
    }
    fsutil::fsync_dir(&creating).ctx(|| format!("cannot fsync {}", creating.display()))?;
    let target = data_dir.mailbox(addr);
    fsutil::rename_noreplace(&staged, &target)
        .map_err(|e| LmmError::runtime(format!("cannot create {}: {e}", target.display())))?;
    if let Err(e) = fsutil::fsync_dir(&boxes) {
        warn(format!("cannot fsync {}: {e}", boxes.display()));
    }
    Ok(())
}

/// `lmm box list` (§9.2).
pub fn list(data_dir: &DataDir) -> Result<()> {
    let boxes = data_dir.boxes();
    let names = match mailbox::sorted_entries(&boxes) {
        Ok(n) => n,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => {
            return Err(LmmError::runtime(format!(
                "cannot read {}: {e}",
                boxes.display()
            )));
        }
    };
    let mut out = std::io::stdout().lock();
    for name in names {
        let Some(addr) = name.to_str() else { continue };
        if !data_dir.exists(addr) {
            continue;
        }
        let new = fsutil::count_visible_entries(&data_dir.new_dir(addr))
            .ctx(|| format!("cannot read {}", data_dir.new_dir(addr).display()))?;
        let failed = fsutil::count_visible_entries(&data_dir.failed_dir(addr))
            .ctx(|| format!("cannot read {}", data_dir.failed_dir(addr).display()))?;
        writeln!(out, "{addr}\tnew={new}\tfailed={failed}")
            .ctx(|| "cannot write to stdout".to_string())?;
    }
    Ok(())
}

/// `lmm box remove` (§9.3).
pub fn remove(data_dir: &DataDir, addr: &str, force: bool) -> Result<()> {
    validate(addr)?;
    let boxes = data_dir.boxes();
    let removing = boxes.join(".removing");
    let staged = removing.join(addr);
    let missing = || LmmError::runtime(format!("mailbox {addr:?} does not exist"));
    if !data_dir.exists(addr) && fs::symlink_metadata(&staged).is_err() {
        return Err(missing());
    }
    let _recv = data_dir.lock(Kind::Receive, addr, Mode::Exclusive)?;
    let _box = data_dir.lock(Kind::Box, addr, Mode::Exclusive)?;
    if !data_dir.exists(addr) {
        // A previous removal was committed (step 2) but stopped before
        // step 3: finish it (§9.3).
        if fs::symlink_metadata(&staged).is_err() {
            return Err(missing());
        }
        fs::remove_dir_all(&staged)
            .ctx(|| format!("cannot delete leftover {}", staged.display()))?;
        return Ok(());
    }
    let new_dir = data_dir.new_dir(addr);
    let pending = fsutil::count_visible_entries(&new_dir)
        .ctx(|| format!("cannot read {}", new_dir.display()))?;
    // Keep guards of abandoned work directories until the mailbox is gone.
    let (live, _abandoned) = mailbox::probe_workdirs(data_dir, addr)?;
    if !force {
        if pending > 0 {
            return Err(LmmError::runtime(format!(
                "mailbox {addr:?} has {pending} unreceived message(s) in {} (use --force to remove anyway)",
                new_dir.display()
            )));
        }
        if live > 0 {
            return Err(LmmError::runtime(format!(
                "mailbox {addr:?} has {live} send(s) in progress (use --force to remove anyway)"
            )));
        }
    }
    ensure_dir(&removing)?;
    fsutil::remove_dir_all_if_exists(&staged)
        .ctx(|| format!("cannot remove leftover {}", staged.display()))?;
    fsutil::rename_noreplace(&data_dir.mailbox(addr), &staged).map_err(|e| {
        LmmError::runtime(format!(
            "cannot remove {}: {e}",
            data_dir.mailbox(addr).display()
        ))
    })?;
    if let Err(e) = fsutil::fsync_dir(&boxes) {
        warn(format!("cannot fsync {}: {e}", boxes.display()));
    }
    if let Err(e) = fs::remove_dir_all(&staged) {
        warn(format!("cannot delete {}: {e}", staged.display()));
    }
    Ok(())
}
