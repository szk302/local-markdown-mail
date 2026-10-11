//! `lmm receive` (§7.1-§7.3) and the shared receive engine used by
//! `lmm wait` and `lmm hook`.

use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::cli::ReceiveArgs;
use crate::commands::Ctx;
use crate::config::Config;
use crate::env;
use crate::error::{LmmError, Result, error_log, warn};
use crate::fsutil::{self, CopyError};
use crate::lock::{self, Kind, Mode};
use crate::mailbox::{self, Home};
use crate::message::{self, INDEX, Inspected, MsgError};

/// Resolve the receive directory (§7.2). Errors are usage errors.
pub fn resolve_dest(ctx: &Ctx, cfg: &Config, arg: Option<&Path>, name: &str) -> Result<PathBuf> {
    let p = match arg {
        Some(a) => env::absolute_from(&ctx.cwd, a),
        None => cfg
            .receive_dir
            .resolve_path("receive.dir", Some(name), &ctx.cwd)?,
    };
    env::check_no_newline(&p, "receive directory")?;
    env::check_outside_home(&p, &ctx.home.root, "receive directory")?;
    Ok(p)
}

/// Create the receive directory and its `.gitignore`.
pub fn prepare_dest(dest: &Path) -> Result<()> {
    fs::create_dir_all(dest).map_err(|e| {
        LmmError::runtime(format!(
            "cannot create receive directory {}: {e}",
            dest.display()
        ))
    })?;
    fsutil::ensure_gitignore(dest, ".lmm-inbox");
    Ok(())
}

/// Options for [`process`].
#[derive(Default)]
pub struct Options<'a> {
    /// Maximum number of successful extractions.
    pub max: Option<usize>,
    /// Only messages for which this returns true are extracted.
    pub filter: Option<&'a dyn Fn(&Inspected) -> bool>,
    /// Print each extracted path on stdout.
    pub print: bool,
    /// Stop between messages when non-zero (signal number).
    pub stop: Option<&'a AtomicUsize>,
}

/// Result of [`process`].
#[derive(Debug, Default)]
pub struct Outcome {
    pub received: Vec<PathBuf>,
    /// Matching messages whose extraction failed (or unreadable messages).
    pub failures: usize,
    /// Messages moved to `failed/` by the inspection.
    pub moved_to_failed: usize,
    /// Entries left in `new/` because `max` was reached.
    pub pending: usize,
}

enum ExtractError {
    /// The whole destination is unusable: stop processing.
    DestWide(String),
    /// Only this message failed.
    Message(String),
}

fn classify_dest(e: io::Error, what: String) -> ExtractError {
    let msg = format!("{what}: {e}");
    if fsutil::is_destination_wide(&e) {
        ExtractError::DestWide(msg)
    } else {
        ExtractError::Message(msg)
    }
}

/// Copy the files of `src` into a fresh `tmp`.
fn copy_into(src: &Path, files: &[OsString], tmp: &Path) -> std::result::Result<(), ExtractError> {
    fsutil::remove_dir_all_if_exists(tmp)
        .map_err(|e| classify_dest(e, format!("cannot remove {}", tmp.display())))?;
    // Failing to create an entry directly in the destination (e.g. EACCES on
    // a read-only-permission directory) affects every message (§7.3).
    fs::create_dir(tmp).map_err(|e| {
        let msg = format!("cannot create {}: {e}", tmp.display());
        if fsutil::is_destination_wide(&e) || fsutil::is_permission_denied(&e) {
            ExtractError::DestWide(msg)
        } else {
            ExtractError::Message(msg)
        }
    })?;
    for f in files {
        let from = src.join(f);
        let to = tmp.join(f);
        match fsutil::copy_file_new(&from, &to) {
            Ok(_) => {}
            Err(CopyError::Source(e)) => {
                return Err(ExtractError::Message(format!(
                    "cannot read {} (left in new/): {e}",
                    from.display()
                )));
            }
            Err(CopyError::Dest(e)) if e.kind() == io::ErrorKind::AlreadyExists => {
                return Err(ExtractError::Message(format!(
                    "file name collision while writing {} (the receive file system treats different names as the same file)",
                    to.display()
                )));
            }
            Err(CopyError::Dest(e)) => {
                return Err(classify_dest(e, format!("cannot write {}", to.display())));
            }
        }
    }
    // Directory fsync is best effort (§12.4). Some file systems used for
    // bind-mounted destinations (FUSE, 9p, CIFS, ...) reject it with EINVAL
    // or ENOTSUP; that must not fail every extraction.
    match fsutil::fsync_dir(tmp) {
        Ok(()) => Ok(()),
        Err(e) if fsutil::is_fsync_unsupported(&e) => {
            warn(format!("cannot fsync {} (unsupported): {e}", tmp.display()));
            Ok(())
        }
        Err(e) => Err(classify_dest(e, format!("cannot fsync {}", tmp.display()))),
    }
}

/// Extract one inspected message (§7.3 steps i-iv). Returns the path.
fn extract(
    home: &Home,
    name: &str,
    src: &Path,
    entry: &OsStr,
    insp: &Inspected,
    dest: &Path,
) -> std::result::Result<PathBuf, ExtractError> {
    let mut files = vec![OsString::from(INDEX)];
    files.extend(insp.listing.attachments.iter().cloned());
    let mut n = 0;
    let target = loop {
        if n >= fsutil::MAX_SUFFIX {
            return Err(ExtractError::Message("too many name collisions".into()));
        }
        let cand = fsutil::suffixed(entry, n);
        let target = dest.join(&cand);
        match fs::symlink_metadata(&target) {
            Ok(_) => {
                if message::read_id(&target).as_deref() == Some(insp.id.as_str()) {
                    // Already extracted by an interrupted earlier run.
                    break target;
                }
                n += 1;
                continue;
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => {
                return Err(ExtractError::DestWide(format!(
                    "cannot access {}: {e}",
                    target.display()
                )));
            }
        }
        let mut tmp_name = OsString::from(".");
        tmp_name.push(&cand);
        tmp_name.push(".tmp");
        let tmp = dest.join(tmp_name);
        copy_into(src, &files, &tmp)?;
        match fsutil::rename_noreplace(&tmp, &target) {
            Ok(()) => {}
            Err(e) if fsutil::is_exists(&e) => {
                let _ = fsutil::remove_dir_all_if_exists(&tmp);
                n += 1;
                continue;
            }
            Err(e) if fsutil::is_unsupported(&e) => {
                fs::rename(&tmp, &target).map_err(|e| {
                    classify_dest(e, format!("cannot rename into {}", target.display()))
                })?;
            }
            Err(e) => {
                return Err(classify_dest(
                    e,
                    format!("cannot rename into {}", target.display()),
                ));
            }
        }
        if let Err(e) = fsutil::fsync_dir(dest) {
            warn(format!("cannot fsync {}: {e}", dest.display()));
        }
        break target;
    };
    // iv. Move the original to cur/.
    let cur = home.cur_dir(name);
    fsutil::rename_unique(src, &cur, entry, |_| false).map_err(|e| {
        ExtractError::Message(format!(
            "cannot move {} to {}: {e}",
            src.display(),
            cur.display()
        ))
    })?;
    for d in [home.new_dir(name), cur] {
        if let Err(e) = fsutil::fsync_dir(&d) {
            warn(format!("cannot fsync {}: {e}", d.display()));
        }
    }
    Ok(target)
}

fn move_to_failed(home: &Home, name: &str, src: &Path, entry: &OsStr, reason: &str) -> bool {
    let failed = home.failed_dir(name);
    match fsutil::rename_unique(src, &failed, entry, |_| false) {
        Ok(p) => {
            error_log(format!(
                "invalid message {}: {reason}; moved to {}",
                src.display(),
                p.display()
            ));
            if let Err(e) = fsutil::fsync_dir(&failed) {
                warn(format!("cannot fsync {}: {e}", failed.display()));
            }
            true
        }
        Err(e) => {
            error_log(format!(
                "invalid message {}: {reason}; cannot move to {}: {e}",
                src.display(),
                failed.display()
            ));
            false
        }
    }
}

/// Process `new/` (§7.3 step 2). The receive lock must be held.
pub fn process(home: &Home, name: &str, dest: &Path, opts: &Options) -> Result<Outcome> {
    let new = home.new_dir(name);
    let entries = mailbox::sorted_entries(&new)
        .map_err(|e| LmmError::runtime(format!("cannot read {}: {e}", new.display())))?;
    let mut out = Outcome::default();
    let total = entries.len();
    for (idx, entry) in entries.into_iter().enumerate() {
        if opts.stop.is_some_and(|s| s.load(Ordering::SeqCst) != 0) {
            break;
        }
        if opts.max.is_some_and(|m| out.received.len() >= m) {
            out.pending = total - idx;
            break;
        }
        let src = new.join(&entry);
        let insp = match message::inspect(&src) {
            Ok(i) => i,
            Err(MsgError::Invalid(reason)) => {
                if move_to_failed(home, name, &src, &entry, &reason) {
                    out.moved_to_failed += 1;
                } else {
                    out.failures += 1;
                }
                continue;
            }
            Err(MsgError::Io(e)) if e.kind() == io::ErrorKind::NotFound => {
                // Gone meanwhile (e.g. removed by the user); nothing to do.
                continue;
            }
            Err(MsgError::Io(e)) => {
                error_log(format!("cannot read {} (left in new/): {e}", src.display()));
                out.failures += 1;
                continue;
            }
        };
        if let Some(f) = opts.filter
            && !f(&insp)
        {
            continue;
        }
        match extract(home, name, &src, &entry, &insp, dest) {
            Ok(p) => {
                if opts.print {
                    let mut so = io::stdout().lock();
                    let _ = env::write_path_line(&mut so, "", &p);
                    let _ = so.flush();
                }
                out.received.push(p);
            }
            Err(ExtractError::Message(m)) => {
                error_log(format!("cannot extract {}: {m}", src.display()));
                out.failures += 1;
            }
            Err(ExtractError::DestWide(m)) => {
                error_log(format!(
                    "cannot extract {}: {m}; stopping (remaining messages stay in new/)",
                    src.display()
                ));
                out.failures += 1;
                break;
            }
        }
    }
    Ok(out)
}

/// Lock, re-check, clean up, and process (§7.3 steps 1-3).
pub fn receive_locked(home: &Home, name: &str, dest: &Path, opts: &Options) -> Result<Outcome> {
    let _lock = lock::acquire_interruptible(
        &home.root,
        Kind::Receive,
        name,
        Mode::Exclusive,
        lock::DEFAULT_TIMEOUT,
        opts.stop,
    )?;
    home.require(name, "your")?;
    mailbox::cleanup_abandoned(home, name);
    process(home, name, dest, opts)
}

pub fn run(ctx: &Ctx, a: &ReceiveArgs) -> Result<()> {
    let name = env::require_name()?;
    ctx.home.require(&name, "your")?;
    let cfg = Config::load(&ctx.cwd, &ctx.home.root)?;
    let dest = resolve_dest(ctx, &cfg, a.dir.as_deref(), &name)?;
    prepare_dest(&dest)?;
    let opts = Options {
        // One message by default, oldest first (§7.1); `--all` lifts the limit.
        max: if a.all {
            None
        } else {
            Some(a.max.unwrap_or(1) as usize)
        },
        print: true,
        ..Default::default()
    };
    let out = receive_locked(&ctx.home, &name, &dest, &opts)?;
    if out.failures + out.moved_to_failed > 0 {
        return Err(LmmError::Exit(1));
    }
    Ok(())
}
