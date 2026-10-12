//! Environment variables and path helpers (§7.2, §12.6).

use std::env;
use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::OsStrExt;
use std::path::{Component, Path, PathBuf};

use crate::error::{LmmError, Result};

/// Read an environment variable as raw bytes, treating an empty value as
/// unset. Use this for paths and commands: non-UTF-8 values are valid on
/// Unix and must never be mistaken for "unset" (R03-5).
pub fn var_os(name: &str) -> Option<OsString> {
    match env::var_os(name) {
        Some(v) if !v.is_empty() => Some(v),
        _ => None,
    }
}

/// The user's home directory.
pub fn home_dir() -> Result<PathBuf> {
    var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| LmmError::usage("HOME is not set"))
}

/// The current working directory.
pub fn cwd() -> Result<PathBuf> {
    env::current_dir().map_err(|e| LmmError::runtime(format!("cannot get current directory: {e}")))
}

/// Make `p` absolute relative to `base` without resolving symlinks.
pub fn absolute_from(base: &Path, p: &Path) -> PathBuf {
    let joined = if p.is_absolute() {
        p.to_path_buf()
    } else {
        base.join(p)
    };
    // Drop `.` components; keep `..` (they can't be resolved without following links).
    let mut out = PathBuf::new();
    for c in joined.components() {
        match c {
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Paths written to stdout must not contain newlines (§12.6).
pub fn check_no_newline(p: &Path, what: &str) -> Result<()> {
    if p.as_os_str()
        .as_bytes()
        .iter()
        .any(|&b| b == b'\n' || b == b'\r')
    {
        return Err(LmmError::usage(format!(
            "{what} contains a newline: {}",
            p.display()
        )));
    }
    Ok(())
}

/// Write `prefix`, the raw bytes of `p` and a newline. Paths are written
/// byte for byte so that non-UTF-8 paths stay usable (§12.6).
pub fn write_path_line(w: &mut impl std::io::Write, prefix: &str, p: &Path) -> std::io::Result<()> {
    let mut line = Vec::with_capacity(prefix.len() + p.as_os_str().len() + 1);
    line.extend_from_slice(prefix.as_bytes());
    line.extend_from_slice(p.as_os_str().as_bytes());
    line.push(b'\n');
    w.write_all(&line)
}

/// Expand a leading `~/` (or a bare `~`) to the home directory.
pub fn expand_tilde(s: &OsStr) -> Result<PathBuf> {
    let b = s.as_bytes();
    if b == b"~" {
        return home_dir();
    }
    if let Some(rest) = b.strip_prefix(b"~/") {
        return Ok(home_dir()?.join(OsStr::from_bytes(rest)));
    }
    Ok(PathBuf::from(s))
}

/// Why a path could not be resolved for the inside-the-data-directory check.
#[derive(Debug)]
pub enum ResolveError {
    /// A symlink whose target does not exist.
    Dangling(PathBuf),
    /// Any other failure (symlink loop, permission denied, ...).
    Other(PathBuf, std::io::Error),
}

/// Resolve `p` the way the kernel will once the missing parts are created
/// with `create_dir_all` (§7.2):
///
/// * existing components are resolved, following symlinks;
/// * missing components are kept literally (they will be real directories);
/// * `..` pops the already resolved prefix, so symlinks reached after
///   walking back into existing directories are resolved again.
///
/// Anything that can't be judged is an error, never "outside".
pub fn resolve_physical(p: &Path) -> std::result::Result<PathBuf, ResolveError> {
    let mut cur = PathBuf::from("/");
    for c in p.components() {
        match c {
            Component::RootDir | Component::Prefix(_) => cur = PathBuf::from("/"),
            Component::CurDir => {}
            Component::ParentDir => {
                cur.pop();
            }
            Component::Normal(name) => {
                let cand = cur.join(name);
                match std::fs::symlink_metadata(&cand) {
                    Ok(m) if m.file_type().is_symlink() => match cand.canonicalize() {
                        Ok(r) => cur = r,
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                            return Err(ResolveError::Dangling(cand));
                        }
                        Err(e) => return Err(ResolveError::Other(cand, e)),
                    },
                    Ok(_) => cur = cand,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => cur = cand,
                    Err(e) => return Err(ResolveError::Other(cand, e)),
                }
            }
        }
    }
    Ok(cur)
}

/// Whether `p` points inside (or at) `dir`, judged on symlink-resolved paths.
/// Unresolvable paths count as inside (fail closed).
pub fn is_inside(p: &Path, dir: &Path) -> bool {
    match (resolve_physical(p), resolve_physical(dir)) {
        (Ok(a), Ok(b)) => a.starts_with(b),
        _ => true,
    }
}

/// §6.7 / §7.2: `p` must not point inside the data directory `data_dir`, and
/// must be resolvable. Usage error otherwise.
pub fn check_outside_data_dir(p: &Path, data_dir: &Path, what: &str) -> Result<()> {
    let explain = |e: ResolveError, whose: &Path| match e {
        ResolveError::Dangling(link) => LmmError::usage(format!(
            "{what} {} goes through a dangling symbolic link {}; cannot check whether it is inside the data directory",
            whose.display(),
            link.display()
        )),
        ResolveError::Other(at, e) => LmmError::usage(format!(
            "cannot resolve {} ({}: {e}); cannot check whether {what} {} is inside the data directory",
            whose.display(),
            at.display(),
            p.display()
        )),
    };
    let rp = resolve_physical(p).map_err(|e| explain(e, p))?;
    let rh = resolve_physical(data_dir).map_err(|e| explain(e, data_dir))?;
    if rp.starts_with(&rh) {
        return Err(LmmError::usage(format!(
            "{what} {} is inside the data directory {}",
            p.display(),
            data_dir.display()
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absolute_drops_curdir() {
        assert_eq!(
            absolute_from(Path::new("/a/b"), Path::new("./c/./d")),
            PathBuf::from("/a/b/c/d")
        );
        assert_eq!(
            absolute_from(Path::new("/a"), Path::new("/x/../y")),
            PathBuf::from("/x/../y")
        );
    }

    #[test]
    fn inside_detection() {
        let t = tempfile::tempdir().unwrap();
        let data_dir = t.path().join("data_dir");
        std::fs::create_dir(&data_dir).unwrap();
        let link = t.path().join("link");
        std::os::unix::fs::symlink(&data_dir, &link).unwrap();
        assert!(is_inside(&data_dir.join("x/y"), &data_dir));
        assert!(is_inside(&link.join("x"), &data_dir));
        assert!(is_inside(&t.path().join("other/../data_dir/z"), &data_dir));
        assert!(!is_inside(&t.path().join("other"), &data_dir));
        assert!(!is_inside(&t.path().join("data_dirs"), &data_dir));
    }

    #[test]
    fn dangling_symlinks() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().canonicalize().unwrap();
        std::os::unix::fs::symlink(root.join("missing"), root.join("dang")).unwrap();
        std::os::unix::fs::symlink(&root, root.join("ok")).unwrap();
        assert!(
            matches!(resolve_physical(&root.join("dang")), Err(ResolveError::Dangling(l)) if l == root.join("dang"))
        );
        assert!(matches!(
            resolve_physical(&root.join("dang/x/y")),
            Err(ResolveError::Dangling(_))
        ));
        assert_eq!(
            resolve_physical(&root.join("ok/x/y")).unwrap(),
            root.join("x/y")
        );
        assert_eq!(
            resolve_physical(&root.join("nope/x")).unwrap(),
            root.join("nope/x")
        );
        assert!(check_outside_data_dir(&root.join("dang/x"), &root.join("data_dir"), "d").is_err());
        assert!(check_outside_data_dir(&root.join("ok/x"), &root.join("data_dir"), "d").is_ok());
        // A loop can't be judged.
        std::os::unix::fs::symlink(root.join("loop"), root.join("loop")).unwrap();
        assert!(matches!(
            resolve_physical(&root.join("loop/x")),
            Err(ResolveError::Other(..))
        ));
        assert!(is_inside(&root.join("loop/x"), &root.join("data_dir")));
    }

    /// R03-1: a symlink reached through `..` after a missing component.
    #[test]
    fn dotdot_after_missing_component_resolves_later_links() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().canonicalize().unwrap();
        let data_dir = root.join("data");
        std::fs::create_dir(&data_dir).unwrap();
        std::os::unix::fs::symlink(&data_dir, root.join("data-link")).unwrap();
        let p = root.join("not-yet/../data-link/boxes/x");
        assert_eq!(resolve_physical(&p).unwrap(), data_dir.join("boxes/x"));
        assert!(is_inside(&p, &data_dir));
        assert!(check_outside_data_dir(&p, &data_dir, "d").is_err());
        assert!(!root.join("not-yet").exists());
        // `..` above a symlinked directory goes to the target's parent,
        // as the kernel does.
        let sub = root.join("a/b");
        std::fs::create_dir_all(&sub).unwrap();
        std::os::unix::fs::symlink(&sub, root.join("lb")).unwrap();
        assert_eq!(
            resolve_physical(&root.join("lb/../c")).unwrap(),
            root.join("a/c")
        );
    }

    #[test]
    fn newline_check() {
        assert!(check_no_newline(Path::new("/a/b"), "x").is_ok());
        assert!(check_no_newline(Path::new("/a\n/b"), "x").is_err());
    }
}
