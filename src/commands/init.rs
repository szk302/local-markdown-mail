//! `lmm init` (§10.8).

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use crate::address;
use crate::cli::InitArgs;
use crate::commands::{Ctx, box_cmd};
use crate::config::Config;
use crate::env;
use crate::error::{IoContext, LmmError, Result};
use crate::fsutil;

/// The shared config `lmm init --root` writes.
const SHARED: &str = "\
root: true
data_dir: \"{root_dir}/.lmm\"
receive:
  dir: \"{config_dir}/.lmm-inbox/{name}\"
draft:
  dir: \"{config_dir}/.lmm-drafts/{name}\"
";

/// Lines `lmm init --root` adds to `.gitignore`.
const IGNORED: [&str; 2] = [".lmm/", ".lmm.local.yml"];

/// The personal config for `name`. Without `--root`, `root: false` cancels
/// the committed `root: true` so that the project above is read (§10.2).
fn personal(root: bool, name: &str) -> String {
    let mut s = String::new();
    if !root {
        s.push_str("root: false\n");
    }
    s.push_str(&format!("name: {name}\n"));
    s
}

/// Append the lines of [`IGNORED`] that `.gitignore` lacks. Whether it changed.
fn add_ignored(p: &Path) -> Result<bool> {
    let old = match fs::read(p) {
        Ok(d) => d,
        Err(e) if e.kind() == io::ErrorKind::NotFound => Vec::new(),
        Err(e) => {
            return Err(LmmError::runtime(format!(
                "cannot read {}: {e}",
                p.display()
            )));
        }
    };
    let text = String::from_utf8_lossy(&old);
    let missing: Vec<&str> = IGNORED
        .into_iter()
        .filter(|l| !text.lines().any(|x| x.trim() == *l))
        .collect();
    if missing.is_empty() {
        return Ok(false);
    }
    let mut add = String::new();
    if !old.is_empty() && !old.ends_with(b"\n") {
        add.push('\n');
    }
    for l in missing {
        add.push_str(l);
        add.push('\n');
    }
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(p)
        .and_then(|mut f| f.write_all(add.as_bytes()))
        .ctx(|| format!("cannot write {}", p.display()))?;
    Ok(true)
}

pub fn run(a: &InitArgs) -> Result<()> {
    // Checks first: nothing is changed when they fail.
    let cwd = env::cwd()?;
    let dir = env::absolute_from(&cwd, a.dir.as_deref().unwrap_or(Path::new(".")));
    if !fs::metadata(&dir).is_ok_and(|m| m.is_dir()) {
        return Err(LmmError::usage(format!(
            "{} is not a directory",
            dir.display()
        )));
    }
    // Resolve `..` and symlinks so that walking up visits the real
    // ancestors (§10.2).
    let dir = dir
        .canonicalize()
        .ctx(|| format!("cannot resolve {}", dir.display()))?;
    env::check_no_newline(&dir, "directory")?;
    if let Some(n) = &a.name {
        address::validate(n).map_err(|e| LmmError::usage(format!("--name: {e}")))?;
    }
    if !a.root {
        // Joining: `root: false` must lead to a project above.
        let above = match dir.parent() {
            Some(p) => Config::load_dirs(p)?.root_dir,
            None => None,
        };
        if above.is_none() {
            return Err(LmmError::usage(format!(
                "no directory above {} has a config with root: true; run `lmm init --root` at the project root first",
                dir.display()
            )));
        }
    }
    let local = dir.join(".lmm.local.yml");
    let personal = a.name.as_deref().map(|n| personal(a.root, n));
    let write_local = match &personal {
        None => false,
        Some(content) => match fs::read(&local) {
            // Already set up the same way: nothing to do.
            Ok(old) if old == content.as_bytes() => false,
            Ok(_) => {
                return Err(LmmError::runtime(format!(
                    "{} already exists; edit it or remove it first",
                    local.display()
                )));
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => true,
            Err(e) => {
                return Err(LmmError::runtime(format!(
                    "cannot read {}: {e}",
                    local.display()
                )));
            }
        },
    };

    // Changes. Each changed file is printed as soon as it is written.
    let mut so = io::stdout().lock();
    let mut changed = |p: PathBuf| {
        env::write_path_line(&mut so, "", &p)
            .and_then(|_| so.flush())
            .ctx(|| "cannot write to stdout".to_string())
    };
    if a.root {
        let shared = dir.join(".lmm.yml");
        match fsutil::write_new(&shared, SHARED.as_bytes()) {
            Ok(()) => changed(shared)?,
            // An existing one is kept as is.
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => {
                return Err(LmmError::runtime(format!(
                    "cannot create {}: {e}",
                    shared.display()
                )));
            }
        }
        let gitignore = dir.join(".gitignore");
        if add_ignored(&gitignore)? {
            changed(gitignore)?;
        }
    }
    if let (Some(name), Some(content)) = (a.name.as_deref(), &personal) {
        if write_local {
            fsutil::write_new(&local, content.as_bytes())
                .ctx(|| format!("cannot create {}", local.display()))?;
            changed(local)?;
        }
        // The data directory as seen from `dir`, with the files above.
        let ctx = Ctx::load(dir)?;
        box_cmd::create(&ctx.data_dir, name, true)?;
    }
    Ok(())
}
