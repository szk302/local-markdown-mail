//! `lmm post` (§6).

use std::ffi::OsString;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::address;
use crate::cli::PostArgs;
use crate::commands::Ctx;
use crate::config::Config;
use crate::env;
use crate::error::{IoContext, LmmError, Result, warn};
use crate::frontmatter::{self, FORWARD_KEYS};
use crate::fsutil::{self, CopyError};
use crate::lock::{Kind, Mode};
use crate::mailbox::{self, Home, WorkDir};
use crate::message::{self, INDEX};
use crate::yaml::{Map, Node};

/// Maximum number of `lmm_forwarded` entries (§6.6).
const MAX_FORWARDED: usize = 100;

struct ReplySource {
    id: String,
    from: String,
    subject: Option<String>,
    thread: String,
}

fn load_reply(cwd: &Path, p: &Path) -> Result<ReplySource> {
    let p = env::absolute_from(cwd, p);
    let index = if fs::metadata(&p).is_ok_and(|m| m.is_dir()) {
        p.join(INDEX)
    } else {
        p.clone()
    };
    let data = fs::read(&index).ctx(|| format!("cannot read reply source {}", index.display()))?;
    let doc = frontmatter::parse(&data)
        .map_err(|e| LmmError::runtime(format!("reply source {}: {e}", index.display())))?;
    let err = |m: String| LmmError::runtime(format!("reply source {}: {m}", index.display()));
    frontmatter::validate_lmm_types(&doc.fm).map_err(err)?;
    let id = doc
        .fm
        .get_str("lmm_id")
        .ok_or_else(|| err("missing lmm_id".into()))?;
    let from = doc
        .fm
        .get_str("lmm_from")
        .ok_or_else(|| err("missing lmm_from".into()))?;
    if !message::is_uuid(id) {
        return Err(err(format!("lmm_id is not a UUID: {id:?}")));
    }
    address::validate(from).map_err(|e| err(format!("lmm_from: {e}")))?;
    let thread = match doc.fm.get_str("lmm_thread_id") {
        Some(t) => {
            message::validate_thread_id(t).map_err(|e| err(format!("lmm_thread_id: {e}")))?;
            t.to_string()
        }
        None => id.to_string(),
    };
    Ok(ReplySource {
        id: id.to_string(),
        from: from.to_string(),
        subject: doc.fm.get_str("lmm_subject").map(str::to_string),
        thread,
    })
}

fn reply_subject(s: &str) -> String {
    if s.starts_with("Re: ") {
        s.to_string()
    } else {
        format!("Re: {s}")
    }
}

fn check_history_entry(m: &Map, what: &str) -> Result<()> {
    let err = |msg: String| LmmError::runtime(format!("--forward: {what}: {msg}"));
    let id = m.get_str("id").unwrap_or_default();
    if !message::is_uuid(id) {
        return Err(err(format!("id is not a UUID: {id:?}")));
    }
    for k in ["from", "to"] {
        address::validate(m.get_str(k).unwrap_or_default())
            .map_err(|e| err(format!("{k}: {e}")))?;
    }
    let ts = m.get_str("created_at").unwrap_or_default();
    if message::parse_created_at(ts).is_none() {
        return Err(err(format!("created_at has an invalid format: {ts:?}")));
    }
    Ok(())
}

/// Compute `lmm_forwarded` for `--forward` (§6.6).
fn forwarded(input: &Map) -> Result<Node> {
    let mut entry = Map::new();
    for (key, src) in FORWARD_KEYS
        .iter()
        .zip(["lmm_id", "lmm_from", "lmm_to", "lmm_created_at"])
    {
        let v = input.get_str(src).ok_or_else(|| {
            LmmError::runtime(format!(
                "--forward: the input is not a received message (missing {src})"
            ))
        })?;
        entry.insert(*key, Node::Str(v.to_string()));
    }
    check_history_entry(&entry, "input message")?;
    let mut items = match input.get("lmm_forwarded") {
        Some(Node::Seq(v)) => v.clone(),
        Some(_) => return Err(LmmError::runtime("--forward: invalid lmm_forwarded")),
        None => Vec::new(),
    };
    for (i, item) in items.iter().enumerate() {
        let Node::Map(m) = item else {
            return Err(LmmError::runtime("--forward: invalid lmm_forwarded"));
        };
        check_history_entry(m, &format!("lmm_forwarded[{i}]"))?;
    }
    items.push(Node::Map(entry));
    if items.len() > MAX_FORWARDED {
        return Err(LmmError::runtime(format!(
            "--forward: forwarding history exceeds {MAX_FORWARDED} entries"
        )));
    }
    Ok(Node::Seq(items))
}

/// Where a delivery is published.
#[derive(Clone, Copy)]
pub enum Target {
    Inbox,
    Sent,
}

/// Write a message into a work directory of `addr` and publish it (§6.4 steps 4-6).
fn deliver(
    home: &Home,
    addr: &str,
    index: &[u8],
    src_dir: &Path,
    attachments: &[OsString],
    base: &str,
    target: Target,
) -> Result<PathBuf> {
    let wd = WorkDir::create(home, addr)?;
    let msg = wd.msg();
    fsutil::write_new(&msg.join(INDEX), index)
        .ctx(|| format!("cannot write {}", msg.join(INDEX).display()))?;
    for name in attachments {
        let src = src_dir.join(name);
        let dst = msg.join(name);
        match fsutil::copy_file_new(&src, &dst) {
            Ok(_) => {}
            Err(CopyError::Dest(e)) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                return Err(LmmError::runtime(format!(
                    "file name collision while writing {} (the file system treats different names as the same file)",
                    dst.display()
                )));
            }
            Err(e) => {
                return Err(LmmError::runtime(format!(
                    "cannot copy {}: {e}",
                    src.display()
                )));
            }
        }
    }
    fsutil::fsync_dir(&msg).ctx(|| format!("cannot fsync {}", msg.display()))?;

    let lock = home.lock(Kind::Box, addr, Mode::Shared)?;
    if !home.exists(addr) {
        return Err(LmmError::runtime(format!(
            "mailbox {addr:?} was removed during delivery"
        )));
    }
    let (dir, other) = match target {
        Target::Inbox => (home.new_dir(addr), Some(home.cur_dir(addr))),
        Target::Sent => (home.sent_dir(addr), None),
    };
    let path = fsutil::rename_unique(&msg, &dir, base.as_ref(), |n| {
        other.as_deref().is_some_and(|o| mailbox::name_exists(o, n))
    })
    .ctx(|| format!("cannot publish into {}", dir.display()))?;
    // The rename is the commit point; later failures are warnings (§6.4 step 5).
    if let Err(e) = fsutil::fsync_dir(&dir) {
        warn(format!("cannot fsync {}: {e}", dir.display()));
    }
    drop(lock);
    if let Err(e) = wd.remove() {
        warn(format!("cannot remove work directory: {e}"));
    }
    Ok(path)
}

pub fn run(ctx: &Ctx, a: &PostArgs) -> Result<()> {
    let name = env::require_name()?;
    env::check_no_newline(&ctx.home.root, "LMM_HOME")?;

    // Usage checks before touching anything.
    if a.dir.as_os_str() == "-" {
        return Err(LmmError::usage(
            "standard input is not accepted; pass a message directory",
        ));
    }
    let dir = env::absolute_from(&ctx.cwd, &a.dir);
    match fs::metadata(&dir) {
        Ok(m) if m.is_dir() => {}
        Ok(_) => {
            return Err(LmmError::usage(format!(
                "{} is not a directory; pass a message directory containing index.md (see `lmm draft`)",
                dir.display()
            )));
        }
        Err(e) => {
            return Err(LmmError::runtime(format!(
                "cannot access {}: {e}",
                dir.display()
            )));
        }
    }
    if let Some(t) = &a.to {
        address::validate(t).map_err(|e| LmmError::usage(format!("--to: {e}")))?;
    }
    if let Some(t) = &a.thread {
        message::validate_thread_id(t).map_err(|e| LmmError::usage(format!("--thread: {e}")))?;
    }
    let metas = a
        .meta
        .iter()
        .map(|m| frontmatter::parse_meta_arg(m).map_err(LmmError::Usage))
        .collect::<Result<Vec<_>>>()?;

    // 1. Sender.
    ctx.home.require(&name, "sender")?;
    let cfg = Config::load(&ctx.cwd, &ctx.home.root)?;

    // 2. Input and values.
    let listing = message::list_followed(&dir).map_err(|e| {
        LmmError::runtime(format!("invalid message directory {}: {e}", dir.display()))
    })?;
    let data =
        message::read_index(&dir).ctx(|| format!("cannot read {}", dir.join(INDEX).display()))?;
    let doc = frontmatter::parse(&data)
        .map_err(|e| LmmError::runtime(format!("{}: {e}", dir.join(INDEX).display())))?;
    frontmatter::validate_lmm_types(&doc.fm)
        .map_err(|e| LmmError::runtime(format!("{}: {e}", dir.join(INDEX).display())))?;
    let reply = a
        .reply
        .as_deref()
        .map(|p| load_reply(&ctx.cwd, p))
        .transpose()?;

    let to = match (&a.to, &reply) {
        (Some(t), _) => t.clone(),
        (None, Some(r)) => r.from.clone(),
        (None, None) => cfg.reply_to(Some(&name), &ctx.cwd)?.ok_or_else(|| {
            LmmError::usage("no destination: use -t/--to, --reply, or set post.reply_to")
        })?,
    };
    let subject = a
        .subject
        .clone()
        .or_else(|| {
            reply
                .as_ref()
                .and_then(|r| r.subject.as_deref().map(reply_subject))
        })
        .or_else(|| doc.fm.get_str("lmm_subject").map(str::to_string));
    let thread = match (&a.thread, &reply) {
        (Some(t), _) => Some(t.clone()),
        (None, Some(r)) => Some(r.thread.clone()),
        (None, None) => match doc.fm.get_str("lmm_thread_id") {
            Some(t) => {
                message::validate_thread_id(t)
                    .map_err(|e| LmmError::runtime(format!("input lmm_thread_id: {e}")))?;
                Some(t.to_string())
            }
            None => None,
        },
    };
    let fwd = if a.forward {
        Some(forwarded(&doc.fm)?)
    } else {
        None
    };

    let id = message::new_id();
    let created = message::now_micros();
    let mut fm = Map::new();
    fm.insert("lmm_id", Node::Str(id.clone()));
    fm.insert("lmm_from", Node::Str(name.clone()));
    fm.insert("lmm_to", Node::Str(to.clone()));
    fm.insert(
        "lmm_created_at",
        Node::Str(message::format_created_at(created)),
    );
    if let Some(s) = subject {
        fm.insert("lmm_subject", Node::Str(s));
    }
    if let Some(t) = thread {
        fm.insert("lmm_thread_id", Node::Str(t));
    }
    if let Some(r) = &reply {
        fm.insert("lmm_in_reply_to", Node::Str(r.id.clone()));
    }
    if let Some(f) = fwd {
        fm.insert("lmm_forwarded", f);
    }
    let mut custom = Map::new();
    for (k, v) in cfg.meta(Some(&name), &ctx.cwd)? {
        custom.insert(k, Node::Str(v));
    }
    for (k, v) in doc.fm.iter() {
        if !k.starts_with("lmm_") {
            custom.insert(k.clone(), v.clone());
        }
    }
    for (k, v) in metas {
        custom.insert(k, Node::Str(v));
    }
    for (k, v) in custom.iter() {
        fm.insert(k.clone(), v.clone());
    }
    let index = frontmatter::render(&fm, &doc.body);
    // What we deliver must pass the receiver's inspection (§7.3): the
    // generated keys can push a front matter over the parser limits (§5.4).
    frontmatter::parse(&index)
        .and_then(|d| message::check_frontmatter(&d.fm))
        .map_err(|e| {
            LmmError::runtime(format!(
                "the resulting message could not be received ({e}); nothing was sent"
            ))
        })?;

    // 3. Validation.
    if !ctx.home.exists(&to) {
        return Err(LmmError::runtime(format!(
            "destination mailbox {to:?} does not exist; create it with `lmm box create {to}`"
        )));
    }
    let total = index.len() as u64 + (listing.total_size - listing.index_size);
    let limit = cfg.max_message_size.value;
    if total > limit {
        return Err(LmmError::runtime(format!(
            "message too large: {total} bytes exceeds max_message_size ({limit} bytes)"
        )));
    }

    // 4-5. Write and publish.
    let base = message::dir_name(created, &name, &to, &id);
    let path = deliver(
        &ctx.home,
        &to,
        &index,
        &dir,
        &listing.attachments,
        &base,
        Target::Inbox,
    )?;

    // 6. Sent copy (failures are warnings).
    if let Err(e) = deliver(
        &ctx.home,
        &name,
        &index,
        &dir,
        &listing.attachments,
        &base,
        Target::Sent,
    ) {
        warn(format!(
            "message delivered but the sent copy was not saved: {e}"
        ));
    }

    // 7. Result. Delivery is already committed, so a failure to write the
    // result (e.g. EPIPE) is a warning, not an error (§6.4 step 5).
    let mut so = std::io::stdout().lock();
    if let Err(e) = env::write_path_line(&mut so, &format!("{id} "), &path).and_then(|_| so.flush())
    {
        warn(format!(
            "message {id} delivered to {} but the result could not be written: {e}",
            path.display()
        ));
    }
    Ok(())
}
