//! `lmm show` (§7.5).

use std::ffi::OsStr;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::cli::ShowArgs;
use crate::commands::Ctx;
use crate::env;
use crate::error::{IoContext, LmmError, Result, warn};
use crate::frontmatter::{self, Document};
use crate::mailbox;
use crate::message::INDEX;
use crate::yaml;

/// Find a message by `lmm_id` in the user's own mailbox.
fn find_by_id(ctx: &Ctx, id: &str) -> Result<PathBuf> {
    let name = ctx.cfg.require_name()?;
    ctx.data_dir.require(&name, "your")?;
    let h = &ctx.data_dir;
    for dir in [
        h.new_dir(&name),
        h.cur_dir(&name),
        h.failed_dir(&name),
        h.sent_dir(&name),
    ] {
        let entries = match mailbox::sorted_entries(&dir) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => {
                warn(format!("cannot read {}: {e}", dir.display()));
                continue;
            }
        };
        for entry in entries {
            let p = dir.join(&entry);
            let data = match fs::read(p.join(INDEX)) {
                Ok(d) => d,
                Err(e) => {
                    warn(format!("skipping {}: {e}", p.display()));
                    continue;
                }
            };
            match frontmatter::parse(&data) {
                Ok(doc) => {
                    if doc.fm.get_str("lmm_id") == Some(id) {
                        return Ok(p);
                    }
                }
                Err(e) => warn(format!("skipping {}: {e}", p.display())),
            }
        }
    }
    Err(LmmError::runtime(format!(
        "message {id} not found in mailbox {name:?}"
    )))
}

/// Split a PATH argument into (message dir, index.md path).
/// PATH must be a message directory or a file named exactly `index.md`
/// (§7.5); other existing files are usage errors.
fn locate(ctx: &Ctx, p: &Path) -> Result<(PathBuf, PathBuf)> {
    let p = env::absolute_from(&ctx.cwd, p);
    match fs::metadata(&p) {
        Ok(m) if m.is_dir() => {
            let index = p.join(INDEX);
            Ok((p, index))
        }
        Ok(_) if p.file_name() != Some(OsStr::new(INDEX)) => Err(LmmError::usage(format!(
            "{} is neither a message directory nor its {INDEX}",
            p.display()
        ))),
        Ok(_) => {
            let dir = p
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| p.clone());
            Ok((dir, p))
        }
        Err(e) => Err(LmmError::runtime(format!(
            "cannot access {}: {e}",
            p.display()
        ))),
    }
}

fn attachments(dir: &Path, index: &Path) -> Result<Vec<String>> {
    let index_name = index.file_name();
    let mut names = Vec::new();
    for e in fs::read_dir(dir).ctx(|| format!("cannot read {}", dir.display()))? {
        let e = e.ctx(|| format!("cannot read {}", dir.display()))?;
        if !e.file_type().is_ok_and(|t| t.is_file()) {
            continue;
        }
        let n = e.file_name();
        if Some(n.as_os_str()) == index_name {
            continue;
        }
        names.push(n);
    }
    names.sort();
    Ok(names
        .into_iter()
        .map(|n| n.to_string_lossy().into_owned())
        .collect())
}

pub fn run(ctx: &Ctx, a: &ShowArgs) -> Result<()> {
    let (dir, index) = match (&a.path, &a.id) {
        (Some(p), None) => locate(ctx, p)?,
        (None, Some(id)) => {
            let d = find_by_id(ctx, id)?;
            let i = d.join(INDEX);
            (d, i)
        }
        _ => return Err(LmmError::usage("specify exactly one of PATH or --id")),
    };
    let data = fs::read(&index).ctx(|| format!("cannot read {}", index.display()))?;
    let doc: Document = frontmatter::parse(&data)
        .map_err(|e| LmmError::runtime(format!("{}: {e}", index.display())))?;
    let mut out = std::io::stdout().lock();
    if a.json {
        let v = serde_json::json!({
            "path": dir.to_string_lossy(),
            "frontmatter": yaml::map_to_json(&doc.fm),
            "body": String::from_utf8_lossy(&doc.body),
            "attachments": attachments(&dir, &index)?,
        });
        writeln!(out, "{v}").ctx(|| "cannot write to stdout".into())?;
    } else if let Some(key) = &a.field {
        let Some(v) = doc.fm.get(key) else {
            return Err(LmmError::Exit(1));
        };
        match v {
            yaml::Node::Str(s) => writeln!(out, "{s}"),
            other => writeln!(out, "{}", yaml::to_json(other)),
        }
        .ctx(|| "cannot write to stdout".into())?;
    } else {
        out.write_all(&data)
            .ctx(|| "cannot write to stdout".into())?;
    }
    Ok(())
}
