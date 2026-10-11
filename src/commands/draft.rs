//! `lmm draft` (§6.7).

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use time::macros::format_description;

use crate::cli::DraftArgs;
use crate::commands::Ctx;
use crate::config::Config;
use crate::env;
use crate::error::{IoContext, LmmError, Result};
use crate::frontmatter;
use crate::fsutil;
use crate::message::{self, INDEX};
use crate::yaml::{Map, Node};

fn random8() -> String {
    const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
    // Skip byte 6 (version) and byte 8 (variant) of the UUID v4, whose
    // high bits are fixed.
    let bytes = *uuid::Uuid::new_v4().as_bytes();
    [0usize, 1, 2, 3, 4, 5, 7, 9]
        .iter()
        .map(|&i| bytes[i])
        .map(|b| ALPHABET[b as usize % ALPHABET.len()] as char)
        .collect()
}

fn check_location(ctx: &Ctx, p: &Path) -> Result<()> {
    env::check_no_newline(p, "draft directory")?;
    env::check_outside_home(p, &ctx.home.root, "draft directory")
}

pub fn run(ctx: &Ctx, a: &DraftArgs) -> Result<()> {
    if let Some(t) = &a.thread {
        message::validate_thread_id(t).map_err(|e| LmmError::usage(format!("--thread: {e}")))?;
    }
    let metas = a
        .meta
        .iter()
        .map(|m| frontmatter::parse_meta_arg(m).map_err(LmmError::Usage))
        .collect::<Result<Vec<_>>>()?;
    let name = env::lmm_name()?;

    let mut fm = Map::new();
    if let Some(s) = &a.subject {
        fm.insert("lmm_subject", Node::Str(s.clone()));
    }
    if let Some(t) = &a.thread {
        fm.insert("lmm_thread_id", Node::Str(t.clone()));
    }
    for (k, v) in metas {
        fm.insert(k, Node::Str(v));
    }
    let content = if fm.is_empty() {
        Vec::new()
    } else {
        frontmatter::render(&fm, b"")
    };

    let dir: PathBuf = match &a.dir {
        Some(d) => {
            let d = env::absolute_from(&ctx.cwd, d);
            check_location(ctx, &d)?;
            if let Some(parent) = d.parent() {
                fs::create_dir_all(parent).ctx(|| format!("cannot create {}", parent.display()))?;
            }
            match fs::create_dir(&d) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                    return Err(LmmError::runtime(format!("{} already exists", d.display())));
                }
                Err(e) => {
                    return Err(LmmError::runtime(format!(
                        "cannot create {}: {e}",
                        d.display()
                    )));
                }
            }
            fsutil::ensure_gitignore(&d, ".lmm-drafts");
            d
        }
        None => {
            let cfg = Config::load(&ctx.cwd, &ctx.home.root)?;
            let base = cfg
                .draft_dir
                .resolve_path("draft.dir", name.as_deref(), &ctx.cwd)?;
            check_location(ctx, &base)?;
            fs::create_dir_all(&base).ctx(|| format!("cannot create {}", base.display()))?;
            fsutil::ensure_gitignore(&base, ".lmm-drafts");
            let ts = message::now_micros()
                .format(format_description!(
                    "[year][month][day]-[hour][minute][second]"
                ))
                .expect("formatting a valid timestamp");
            let mut created = None;
            for _ in 0..100 {
                let d = base.join(format!("{ts}_{}", random8()));
                match fs::create_dir(&d) {
                    Ok(()) => {
                        created = Some(d);
                        break;
                    }
                    Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                    Err(e) => {
                        return Err(LmmError::runtime(format!(
                            "cannot create {}: {e}",
                            d.display()
                        )));
                    }
                }
            }
            created.ok_or_else(|| LmmError::runtime("cannot find a free draft name"))?
        }
    };
    fsutil::write_new(&dir.join(INDEX), &content)
        .ctx(|| format!("cannot write {}", dir.join(INDEX).display()))?;
    let mut so = std::io::stdout().lock();
    env::write_path_line(&mut so, "", &dir)
        .and_then(|_| so.flush())
        .ctx(|| {
            format!(
                "draft {} created but its path could not be written",
                dir.display()
            )
        })?;
    Ok(())
}
