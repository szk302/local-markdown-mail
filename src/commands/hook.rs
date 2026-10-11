//! `lmm hook` (§8).

use std::ffi::{OsStr, OsString};
use std::io::{self, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use crate::cli::HookArgs;
use crate::commands::Ctx;
use crate::commands::receive::{self, Options};
use crate::config::Config;
use crate::env;
use crate::error::{LmmError, Result, error_log, warn};
use crate::fsutil;

/// Map any error of `lmm hook` itself to exit code 0 or 1 (§8.3).
/// Signal interruptions while the notification command runs keep
/// 128 + signal (§8.4).
pub fn run(a: &HookArgs) -> Result<()> {
    match run_inner(a) {
        Ok(()) => Ok(()),
        Err(LmmError::Usage(m)) => Err(LmmError::Runtime(m)),
        Err(e @ (LmmError::Exit(_) | LmmError::Runtime(_) | LmmError::Signal(_))) => Err(e),
        Err(e) => Err(LmmError::Runtime(e.to_string())),
    }
}

fn run_inner(a: &HookArgs) -> Result<()> {
    // 1. Participation. Checked before anything else (current directory,
    // HOME, ...) so that non-participating sessions always exit 0.
    let Some(name) = env::lmm_name()? else {
        return Ok(());
    };
    let ctx = &Ctx::from_env()?;
    if !ctx.home.exists(&name) {
        warn(format!(
            "mailbox {name:?} does not exist; nothing to receive (create it with `lmm box create {name}`)"
        ));
        return Ok(());
    }
    // 2. Cheap check without the lock.
    let new = ctx.home.new_dir(&name);
    match fsutil::count_visible_entries(&new) {
        Ok(0) => return Ok(()),
        Ok(_) => {}
        Err(e) => {
            return Err(LmmError::runtime(format!(
                "cannot read {}: {e}",
                new.display()
            )));
        }
    }
    // 3. Receive.
    let cfg = Config::load(&ctx.cwd, &ctx.home.root)?;
    let dest = receive::resolve_dest(ctx, &cfg, a.dir.as_deref(), &name)?;
    receive::prepare_dest(&dest)?;
    let opts = Options {
        max: Some(cfg.hook_max_messages.value),
        print: false,
        ..Default::default()
    };
    let out = receive::receive_locked(&ctx.home, &name, &dest, &opts)?;
    let failed = out.failures + out.moved_to_failed > 0;
    if out.received.is_empty() {
        return if failed {
            Err(LmmError::Exit(1))
        } else {
            Ok(())
        };
    }
    // 4. Notify.
    match cfg.hook_cmd.as_ref().map(|v| v.os_value()) {
        None => {
            // Always 0 after the built-in output, even if some messages
            // failed: events that consume it ignore stdout on non-zero exit
            // codes (§8.3). Failures were already logged on stderr.
            let mut text: Vec<u8> = Vec::new();
            let n = out.received.len();
            text.extend_from_slice(
                format!(
                    "You have {n} new message{} (LMM). Read index.md in each directory below and respond:\n",
                    if n == 1 { "" } else { "s" }
                )
                .as_bytes(),
            );
            for p in &out.received {
                text.extend_from_slice(p.as_os_str().as_bytes());
                text.push(b'\n');
            }
            if out.pending > 0 {
                text.extend_from_slice(
                    format!(
                        "{} more message(s) are waiting and will be delivered next time.\n",
                        out.pending
                    )
                    .as_bytes(),
                );
            }
            let mut so = io::stdout().lock();
            so.write_all(&text)
                .and_then(|_| so.flush())
                .map_err(|e| LmmError::runtime(format!("cannot write to stdout: {e}")))?;
            Ok(())
        }
        Some(cmd) => {
            let mut received = OsString::new();
            for (i, p) in out.received.iter().enumerate() {
                if i > 0 {
                    received.push("\n");
                }
                received.push(p.as_os_str());
            }
            let envs: [(&str, OsString); 5] = [
                ("LMM_NAME", name.into()),
                ("LMM_RECEIVE_DIR", dest.into_os_string()),
                ("LMM_RECEIVED", received),
                ("LMM_RECEIVED_COUNT", out.received.len().to_string().into()),
                ("LMM_PENDING_COUNT", out.pending.to_string().into()),
            ];
            // Validated when loading the config; fall back to "no limit".
            let timeout =
                Duration::try_from_secs_f64(cfg.hook_timeout.value).unwrap_or(Duration::MAX);
            let code = run_notify(&cmd, &envs, timeout)?;
            Err(LmmError::Exit(code))
        }
    }
}

/// Signals forwarded to the notification command's process group (§8.4).
const FORWARDED: [i32; 3] = [libc::SIGTERM, libc::SIGINT, libc::SIGHUP];
const POLL: Duration = Duration::from_millis(10);
const GRACE: Duration = Duration::from_secs(2);

fn kill_group(pgid: i32, sig: i32) {
    // SAFETY: plain kill(2) on a process group we created.
    unsafe {
        libc::kill(-pgid, sig);
    }
}

fn group_alive(pgid: i32) -> bool {
    // SAFETY: signal 0 only checks for existence.
    unsafe { libc::kill(-pgid, 0) == 0 }
}

fn exit_code(status: ExitStatus) -> i32 {
    status
        .code()
        .unwrap_or_else(|| 128 + status.signal().unwrap_or(0))
}

/// Wait up to `limit` for the child and its whole group to go away,
/// then SIGKILL the group. Always reaps the child.
fn terminate(child: &mut Child, pgid: i32, limit: Duration) {
    let until = Instant::now().checked_add(limit);
    let mut reaped = false;
    while until.is_none_or(|u| Instant::now() < u) {
        if !reaped && matches!(child.try_wait(), Ok(Some(_))) {
            reaped = true;
        }
        if reaped && !group_alive(pgid) {
            return;
        }
        thread::sleep(POLL);
    }
    if group_alive(pgid) {
        kill_group(pgid, libc::SIGKILL);
    }
    if !reaped {
        let _ = child.wait();
    }
}

/// Run the notification command (§8.4). Returns its exit code.
fn run_notify(cmd: &OsStr, envs: &[(&str, OsString)], timeout: Duration) -> Result<i32> {
    let _ = io::stdout().flush();
    // Record signals instead of dying, so they can be forwarded to the
    // command's process group (which doesn't receive them otherwise).
    let sig = Arc::new(AtomicUsize::new(0));
    let mut ids = Vec::new();
    for s in FORWARDED {
        let id = signal_hook::flag::register_usize(s, Arc::clone(&sig), s as usize)
            .map_err(|e| LmmError::runtime(format!("cannot install signal handler: {e}")))?;
        ids.push(id);
    }
    let result = run_notify_inner(cmd, envs, timeout, &sig);
    for id in ids {
        signal_hook::low_level::unregister(id);
    }
    result
}

fn run_notify_inner(
    cmd: &OsStr,
    envs: &[(&str, OsString)],
    timeout: Duration,
    sig: &AtomicUsize,
) -> Result<i32> {
    let mut child = Command::new("/bin/sh")
        .arg("-c")
        .arg(cmd)
        .envs(envs.iter().map(|(k, v)| (*k, v.as_os_str())))
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .process_group(0)
        .spawn()
        .map_err(|e| LmmError::runtime(format!("cannot run notification command: {e}")))?;
    let pgid = child.id() as i32;
    let start = Instant::now();
    // A deadline beyond what `Instant` can represent means "no limit".
    let deadline = start.checked_add(timeout);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(exit_code(status)),
            Ok(None) => {}
            Err(e) => {
                kill_group(pgid, libc::SIGKILL);
                let _ = child.wait();
                return Err(LmmError::runtime(format!(
                    "cannot wait for notification command: {e}"
                )));
            }
        }
        let s = sig.load(Ordering::SeqCst) as i32;
        if s != 0 {
            // Forward, then wait min(remaining timeout, 2 s) before SIGKILL.
            kill_group(pgid, s);
            let remaining = timeout.saturating_sub(start.elapsed());
            terminate(&mut child, pgid, remaining.min(GRACE));
            return Err(LmmError::Signal(s));
        }
        if deadline.is_some_and(|d| Instant::now() >= d) {
            break;
        }
        thread::sleep(POLL);
    }
    // Timed out: SIGTERM the whole group, then SIGKILL after 2 seconds.
    kill_group(pgid, libc::SIGTERM);
    terminate(&mut child, pgid, GRACE);
    error_log(format!(
        "notification command timed out after {timeout:?} and was terminated"
    ));
    Ok(1)
}
