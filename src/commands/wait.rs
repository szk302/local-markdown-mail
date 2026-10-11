//! `lmm wait` (§7.4).

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use crate::address;
use crate::cli::WaitArgs;
use crate::commands::Ctx;
use crate::commands::receive::{self, Options};
use crate::config::Config;
use crate::env;
use crate::error::{LmmError, Result};
use crate::frontmatter;
use crate::fsutil;
use crate::message::Inspected;
use crate::yaml::Node;

const POLL_INTERVAL: Duration = Duration::from_secs(1);
const TICK: Duration = Duration::from_millis(50);

struct Filter {
    from: Option<String>,
    thread: Option<String>,
    in_reply_to: Option<String>,
    meta: Vec<(String, String)>,
}

impl Filter {
    fn matches(&self, i: &Inspected) -> bool {
        let fm = i.fm();
        if self.from.as_deref().is_some_and(|f| i.from != f) {
            return false;
        }
        if self
            .thread
            .as_deref()
            .is_some_and(|t| fm.get_str("lmm_thread_id") != Some(t))
        {
            return false;
        }
        if self
            .in_reply_to
            .as_deref()
            .is_some_and(|r| fm.get_str("lmm_in_reply_to") != Some(r))
        {
            return false;
        }
        self.meta
            .iter()
            .all(|(k, v)| matches!(fm.get(k), Some(Node::Str(s)) if s == v))
    }
}

pub fn run(ctx: &Ctx, a: &WaitArgs) -> Result<()> {
    let name = env::require_name()?;
    if let Some(f) = &a.from {
        address::validate(f).map_err(|e| LmmError::usage(format!("--from: {e}")))?;
    }
    let meta = a
        .meta
        .iter()
        .map(|m| frontmatter::parse_meta_arg(m).map_err(LmmError::Usage))
        .collect::<Result<Vec<_>>>()?;
    let timeout = match a.timeout {
        Some(t) if !(t.is_finite() && t >= 0.0) => {
            return Err(LmmError::usage("--timeout must be a non-negative number"));
        }
        Some(t) => Some(
            Duration::try_from_secs_f64(t)
                .map_err(|_| LmmError::usage(format!("--timeout {t} is too large")))?,
        ),
        None => None,
    };
    ctx.home.require(&name, "your")?;
    let cfg = Config::load(&ctx.cwd, &ctx.home.root)?;
    let dest = receive::resolve_dest(ctx, &cfg, a.dir.as_deref(), &name)?;

    // Install handlers before creating the receive directory: once it
    // exists, signals are handled (tests rely on this ordering).
    let sig = Arc::new(AtomicUsize::new(0));
    for s in [libc::SIGINT, libc::SIGTERM] {
        signal_hook::flag::register_usize(s, Arc::clone(&sig), s as usize)
            .map_err(|e| LmmError::runtime(format!("cannot install signal handler: {e}")))?;
    }
    receive::prepare_dest(&dest)?;
    let interrupted = || match sig.load(Ordering::SeqCst) {
        0 => None,
        s => Some(LmmError::Signal(s as i32)),
    };

    let filter = Filter {
        from: a.from.clone(),
        thread: a.thread.clone(),
        in_reply_to: a.in_reply_to.clone(),
        meta,
    };
    let f = |i: &Inspected| filter.matches(i);
    let start = Instant::now();
    let new = ctx.home.new_dir(&name);
    loop {
        if let Some(e) = interrupted() {
            return Err(e);
        }
        ctx.home.require(&name, "your")?;
        let has = fsutil::count_visible_entries(&new)
            .map_err(|e| LmmError::runtime(format!("cannot read {}: {e}", new.display())))?
            > 0;
        if has {
            let opts = Options {
                // One message by default, like `lmm receive` (§7.4).
                max: if a.all {
                    None
                } else {
                    Some(a.max.unwrap_or(1) as usize)
                },
                filter: Some(&f),
                print: true,
                stop: Some(&sig),
            };
            // A recorded signal wins over any error from receiving (e.g. a
            // lock timeout that raced with the signal), §7.4.
            let out = match receive::receive_locked(&ctx.home, &name, &dest, &opts) {
                Ok(out) => out,
                Err(e) => return Err(interrupted().unwrap_or(e)),
            };
            if let Some(e) = interrupted() {
                return Err(e);
            }
            if out.failures > 0 {
                return Err(LmmError::Exit(1));
            }
            if !out.received.is_empty() {
                return Ok(());
            }
        }
        // Sleep until the next poll, staying responsive to signals and the timeout.
        let next = Instant::now() + POLL_INTERVAL;
        loop {
            if let Some(t) = timeout
                && start.elapsed() >= t
            {
                return Err(LmmError::Timeout);
            }
            if interrupted().is_some() || Instant::now() >= next {
                break;
            }
            thread::sleep(TICK);
        }
    }
}
