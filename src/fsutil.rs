//! File system primitives (Appendix A): non-replacing rename, exclusive
//! creation, symlink-safe copies, and fsync of directories.

use std::ffi::{CString, OsStr};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

fn cstr(p: &Path) -> io::Result<CString> {
    CString::new(p.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains NUL"))
}

/// Rename `from` to `to`, failing with `AlreadyExists` if `to` exists
/// (even if it is an empty directory).
#[cfg(target_os = "linux")]
pub fn rename_noreplace(from: &Path, to: &Path) -> io::Result<()> {
    let f = cstr(from)?;
    let t = cstr(to)?;
    // SAFETY: valid NUL-terminated strings; renameat2 does not retain pointers.
    let r = unsafe {
        libc::syscall(
            libc::SYS_renameat2,
            libc::AT_FDCWD,
            f.as_ptr(),
            libc::AT_FDCWD,
            t.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if r == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// Rename `from` to `to`, failing with `AlreadyExists` if `to` exists.
#[cfg(target_os = "macos")]
pub fn rename_noreplace(from: &Path, to: &Path) -> io::Result<()> {
    let f = cstr(from)?;
    let t = cstr(to)?;
    // SAFETY: valid NUL-terminated strings.
    let r = unsafe { libc::renamex_np(f.as_ptr(), t.as_ptr(), libc::RENAME_EXCL) };
    if r == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// Whether a rename error means the file system doesn't support
/// non-replacing renames (§12.1).
pub fn is_unsupported(e: &io::Error) -> bool {
    e.raw_os_error()
        .is_some_and(|c| [libc::EINVAL, libc::ENOSYS, libc::ENOTSUP, libc::EOPNOTSUPP].contains(&c))
}

/// Whether an error means the destination exists.
pub fn is_exists(e: &io::Error) -> bool {
    e.kind() == io::ErrorKind::AlreadyExists
        || matches!(e.raw_os_error(), Some(libc::EEXIST) | Some(libc::ENOTEMPTY))
}

/// Name with a numeric suffix: `base`, `base_1`, `base_2`, ...
pub fn suffixed(base: &OsStr, n: usize) -> std::ffi::OsString {
    if n == 0 {
        base.to_os_string()
    } else {
        let mut s = base.to_os_string();
        s.push(format!("_{n}"));
        s
    }
}

/// Upper bound for numeric suffix retries.
pub const MAX_SUFFIX: usize = 100_000;

/// Move `from` into `dir` under `base` (or `base_N`), never replacing.
/// `taken` can report names to skip before trying (e.g. names present in
/// another directory). Returns the final path.
pub fn rename_unique(
    from: &Path,
    dir: &Path,
    base: &OsStr,
    mut taken: impl FnMut(&OsStr) -> bool,
) -> io::Result<PathBuf> {
    for n in 0..MAX_SUFFIX {
        let name = suffixed(base, n);
        // Check the target directory first, then `taken` (§6.4 step 5:
        // `new/` before `cur/`).
        let to = dir.join(&name);
        if fs::symlink_metadata(&to).is_ok() {
            continue;
        }
        if taken(&name) {
            continue;
        }
        match rename_noreplace(from, &to) {
            Ok(()) => return Ok(to),
            Err(e) if is_exists(&e) => continue,
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::other("too many name collisions"))
}

/// fsync a directory.
pub fn fsync_dir(dir: &Path) -> io::Result<()> {
    // `sync_all` uses F_FULLFSYNC on macOS.
    File::open(dir)?.sync_all()
}

/// Open a file for reading without following a symlink at the last component.
pub fn open_nofollow(p: &Path) -> io::Result<File> {
    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(p)
}

/// Create `p` exclusively (O_CREAT|O_EXCL) and write `data`, then fsync.
pub fn write_new(p: &Path, data: &[u8]) -> io::Result<()> {
    let mut f = OpenOptions::new().write(true).create_new(true).open(p)?;
    f.write_all(data)?;
    f.sync_all()
}

/// Which side of a copy failed.
#[derive(Debug)]
pub enum CopyError {
    /// Reading the source failed.
    Source(io::Error),
    /// Writing the destination failed.
    Dest(io::Error),
}

impl std::fmt::Display for CopyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CopyError::Source(e) => write!(f, "read failed: {e}"),
            CopyError::Dest(e) => write!(f, "write failed: {e}"),
        }
    }
}

/// Copy a regular file without following symlinks, creating the
/// destination exclusively, then fsync it.
pub fn copy_file_new(src: &Path, dst: &Path) -> std::result::Result<u64, CopyError> {
    let mut input = open_nofollow(src).map_err(CopyError::Source)?;
    let meta = input.metadata().map_err(CopyError::Source)?;
    if !meta.is_file() {
        return Err(CopyError::Source(io::Error::new(
            io::ErrorKind::InvalidData,
            "not a regular file",
        )));
    }
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(dst)
        .map_err(CopyError::Dest)?;
    let mut buf = vec![0u8; 64 * 1024];
    let mut total = 0u64;
    loop {
        let n = match input.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(CopyError::Source(e)),
        };
        output.write_all(&buf[..n]).map_err(CopyError::Dest)?;
        total += n as u64;
    }
    output.sync_all().map_err(CopyError::Dest)?;
    Ok(total)
}

/// Remove a directory tree if it exists.
pub fn remove_dir_all_if_exists(p: &Path) -> io::Result<()> {
    match fs::symlink_metadata(p) {
        Ok(m) if m.is_dir() => fs::remove_dir_all(p),
        Ok(_) => fs::remove_file(p),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// Create a `.gitignore` with `*` in the parent of `dir` when the parent is
/// named `parent_name` and has none (§6.7, §7.2). Failures are warnings.
pub fn ensure_gitignore(dir: &Path, parent_name: &str) {
    let Some(parent) = dir.parent() else { return };
    if parent.file_name() != Some(OsStr::new(parent_name)) {
        return;
    }
    let gi = parent.join(".gitignore");
    if fs::symlink_metadata(&gi).is_ok() {
        return;
    }
    match write_new(&gi, b"*\n") {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
        Err(e) => crate::error::warn(format!("cannot create {}: {e}", gi.display())),
    }
}

/// Whether `dir` has any entry.
pub fn dir_has_entries(dir: &Path) -> io::Result<bool> {
    Ok(fs::read_dir(dir)?.next().transpose()?.is_some())
}

/// Count entries in `dir` whose names don't start with `.` (0 if it does not
/// exist). Hidden entries such as `.DS_Store` are never messages; receiving
/// skips them as well (`mailbox::sorted_entries`).
pub fn count_visible_entries(dir: &Path) -> io::Result<usize> {
    match fs::read_dir(dir) {
        Ok(rd) => {
            let mut n = 0;
            for e in rd {
                if e?.file_name().as_bytes().first() != Some(&b'.') {
                    n += 1;
                }
            }
            Ok(n)
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(0),
        Err(e) => Err(e),
    }
}

/// Whether an error indicates that a whole destination is unusable
/// (§7.3: ENOSPC, EDQUOT, EROFS).
pub fn is_destination_wide(e: &io::Error) -> bool {
    matches!(
        e.raw_os_error(),
        Some(libc::ENOSPC) | Some(libc::EDQUOT) | Some(libc::EROFS)
    )
}

/// EACCES / EPERM.
pub fn is_permission_denied(e: &io::Error) -> bool {
    matches!(e.raw_os_error(), Some(libc::EACCES) | Some(libc::EPERM))
}

/// Whether an fsync error means the file system doesn't support fsync on
/// this object (seen on some FUSE, 9p and CIFS mounts for directories).
pub fn is_fsync_unsupported(e: &io::Error) -> bool {
    e.raw_os_error()
        .is_some_and(|c| [libc::EINVAL, libc::ENOTSUP, libc::EOPNOTSUPP].contains(&c))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_classes() {
        let os = io::Error::from_raw_os_error;
        assert!(is_permission_denied(&os(libc::EACCES)));
        assert!(is_permission_denied(&os(libc::EPERM)));
        assert!(!is_permission_denied(&os(libc::ENOENT)));
        for c in [libc::EINVAL, libc::ENOTSUP, libc::EOPNOTSUPP] {
            assert!(is_fsync_unsupported(&os(c)), "{c}");
        }
        assert!(!is_fsync_unsupported(&os(libc::EIO)));
        assert!(is_destination_wide(&os(libc::ENOSPC)));
        assert!(!is_destination_wide(&os(libc::EACCES)));
    }

    #[test]
    fn noreplace_does_not_replace_empty_dir() {
        let t = tempfile::tempdir().unwrap();
        let a = t.path().join("a");
        let b = t.path().join("b");
        fs::create_dir(&a).unwrap();
        fs::create_dir(&b).unwrap();
        let e = rename_noreplace(&a, &b).unwrap_err();
        assert!(is_exists(&e), "{e:?}");
        assert!(a.exists());
        let c = t.path().join("c");
        rename_noreplace(&a, &c).unwrap();
        assert!(c.exists() && !a.exists());
    }

    #[test]
    fn unique_rename_adds_suffix() {
        let t = tempfile::tempdir().unwrap();
        let dir = t.path().join("dst");
        fs::create_dir(&dir).unwrap();
        fs::create_dir(dir.join("m")).unwrap();
        fs::create_dir(dir.join("m_1")).unwrap();
        let src = t.path().join("src");
        fs::create_dir(&src).unwrap();
        let p = rename_unique(&src, &dir, OsStr::new("m"), |n| n == "m_2").unwrap();
        assert_eq!(p, dir.join("m_3"));
    }

    #[test]
    fn copy_new_refuses_existing_and_symlink() {
        let t = tempfile::tempdir().unwrap();
        let src = t.path().join("s");
        fs::write(&src, b"hello").unwrap();
        let dst = t.path().join("d");
        assert_eq!(copy_file_new(&src, &dst).unwrap(), 5);
        assert!(matches!(
            copy_file_new(&src, &dst),
            Err(CopyError::Dest(ref e)) if e.kind() == io::ErrorKind::AlreadyExists
        ));
        let link = t.path().join("l");
        std::os::unix::fs::symlink(&src, &link).unwrap();
        assert!(matches!(
            copy_file_new(&link, &t.path().join("x")),
            Err(CopyError::Source(_))
        ));
    }
}
