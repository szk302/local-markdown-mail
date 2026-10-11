//! Message directories (§5.1, §5.2) and the receive-time inspection (§7.3).

use std::ffi::OsString;
use std::fs;
use std::io::{self, Read};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use time::OffsetDateTime;
use time::format_description::FormatItem;
use time::macros::format_description;

use crate::address;
use crate::frontmatter::{self, Document};
use crate::fsutil;
use crate::yaml::Map;

pub const INDEX: &str = "index.md";

/// Errors while reading a message directory.
#[derive(Debug)]
pub enum MsgError {
    /// The message violates the rules (permanent).
    Invalid(String),
    /// I/O error (possibly transient).
    Io(io::Error),
}

impl std::fmt::Display for MsgError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MsgError::Invalid(m) => f.write_str(m),
            MsgError::Io(e) => write!(f, "I/O error: {e}"),
        }
    }
}

impl From<io::Error> for MsgError {
    fn from(e: io::Error) -> Self {
        MsgError::Io(e)
    }
}

/// Contents of a valid message directory.
#[derive(Debug, Clone)]
pub struct Listing {
    /// Attachment file names (everything except `index.md`), sorted.
    pub attachments: Vec<OsString>,
    /// Total size of all files including `index.md`.
    pub total_size: u64,
    /// Size of `index.md`.
    pub index_size: u64,
}

/// Validate a file name (§5.1).
pub fn validate_file_name(name: &[u8]) -> Result<(), String> {
    let shown = String::from_utf8_lossy(name);
    if name.is_empty() || name.len() > 255 {
        return Err(format!(
            "invalid file name {shown:?}: length must be 1-255 bytes"
        ));
    }
    if name.contains(&b'/') || name.contains(&0) {
        return Err(format!("invalid file name {shown:?}"));
    }
    if name[0] == b'.' {
        return Err(format!(
            "invalid file name {shown:?}: must not start with '.'"
        ));
    }
    Ok(())
}

/// List and validate a message directory (§5.1).
pub fn list(dir: &Path) -> Result<Listing, MsgError> {
    let meta = fs::symlink_metadata(dir)?;
    if !meta.is_dir() {
        return Err(MsgError::Invalid(format!(
            "{} is not a directory",
            dir.display()
        )));
    }
    list_followed(dir)
}

/// Like [`list`] but `dir` itself may be a symlink to a directory.
pub fn list_followed(dir: &Path) -> Result<Listing, MsgError> {
    let mut has_index = false;
    let mut names: Vec<OsString> = Vec::new();
    let mut lower: Vec<Vec<u8>> = Vec::new();
    let mut total = 0u64;
    let mut index_size = 0u64;
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let bytes = name.as_bytes();
        let shown = String::from_utf8_lossy(bytes).into_owned();
        let ft = entry.file_type()?;
        if ft.is_symlink() {
            return Err(MsgError::Invalid(format!(
                "symbolic link not allowed: {shown}"
            )));
        }
        if ft.is_dir() {
            return Err(MsgError::Invalid(format!(
                "subdirectory not allowed: {shown}"
            )));
        }
        if !ft.is_file() {
            return Err(MsgError::Invalid(format!(
                "special file not allowed: {shown}"
            )));
        }
        validate_file_name(bytes).map_err(MsgError::Invalid)?;
        let lc = bytes.to_ascii_lowercase();
        if lower.contains(&lc) {
            return Err(MsgError::Invalid(format!(
                "file names differ only in case: {shown}"
            )));
        }
        lower.push(lc);
        let size = entry.metadata()?.len();
        total += size;
        if bytes == INDEX.as_bytes() {
            has_index = true;
            index_size = size;
        } else {
            names.push(name);
        }
    }
    if !has_index {
        return Err(MsgError::Invalid(format!("{INDEX} not found")));
    }
    names.sort();
    Ok(Listing {
        attachments: names,
        total_size: total,
        index_size,
    })
}

/// Read `index.md` (without following a symlink).
pub fn read_index(dir: &Path) -> io::Result<Vec<u8>> {
    let mut f = fsutil::open_nofollow(&dir.join(INDEX))?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf)?;
    Ok(buf)
}

static NAME_TS: &[FormatItem<'static>] =
    format_description!("[year][month][day]-[hour][minute][second].[subsecond digits:6]");
static RFC3339_MICRO: &[FormatItem<'static>] =
    format_description!("[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond digits:6]Z");

/// Current UTC time truncated to microseconds.
pub fn now_micros() -> OffsetDateTime {
    let now = OffsetDateTime::now_utc();
    let micros = now.microsecond();
    now.replace_microsecond(micros).unwrap_or(now)
}

/// `lmm_created_at` representation (§5.3).
pub fn format_created_at(t: OffsetDateTime) -> String {
    t.format(RFC3339_MICRO)
        .expect("formatting a valid timestamp")
}

/// Parse `lmm_created_at` in the exact form `YYYY-MM-DDTHH:MM:SS.ffffffZ`.
pub fn parse_created_at(s: &str) -> Option<OffsetDateTime> {
    if s.len() != 27 {
        return None;
    }
    time::PrimitiveDateTime::parse(s, RFC3339_MICRO)
        .ok()
        .map(|p| p.assume_utc())
}

/// Directory name (§5.2): `{YYYYMMDD-HHMMSS.ffffff}_{From}_to_{To}_{ShortID}`.
pub fn dir_name(created_at: OffsetDateTime, from: &str, to: &str, id: &str) -> String {
    let ts = created_at
        .format(NAME_TS)
        .expect("formatting a valid timestamp");
    let short: String = id.chars().take(8).collect();
    format!("{ts}_{from}_to_{to}_{short}")
}

/// UUID in lowercase hyphenated form (§5.3).
pub fn is_uuid(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 36
        && b.iter().enumerate().all(|(i, &c)| match i {
            8 | 13 | 18 | 23 => c == b'-',
            _ => c.is_ascii_digit() || (b'a'..=b'f').contains(&c),
        })
}

/// New `lmm_id`.
pub fn new_id() -> String {
    uuid::Uuid::new_v4().hyphenated().to_string()
}

/// Thread ID rule (§5.3).
pub fn validate_thread_id(s: &str) -> Result<(), String> {
    let n = s.chars().count();
    if n == 0 || n > 128 {
        return Err(format!(
            "invalid thread id {s:?}: length must be 1-128 characters"
        ));
    }
    if s.contains(['\n', '\r']) {
        return Err("invalid thread id: must not contain newlines".into());
    }
    Ok(())
}

/// A message that passed the inspection.
#[derive(Debug, Clone)]
pub struct Inspected {
    pub listing: Listing,
    pub doc: Document,
    pub id: String,
    pub from: String,
    pub to: String,
}

impl Inspected {
    pub fn fm(&self) -> &Map {
        &self.doc.fm
    }
}

/// Check required keys and value formats of a front matter (§7.3).
pub fn check_frontmatter(fm: &Map) -> Result<(), String> {
    frontmatter::validate_lmm_types(fm)?;
    for key in ["lmm_id", "lmm_from", "lmm_to", "lmm_created_at"] {
        if !fm.contains_key(key) {
            return Err(format!("missing required key {key}"));
        }
    }
    let id = fm.get_str("lmm_id").unwrap_or_default();
    if !is_uuid(id) {
        return Err(format!("lmm_id is not a UUID: {id:?}"));
    }
    for key in ["lmm_from", "lmm_to"] {
        address::validate(fm.get_str(key).unwrap_or_default())
            .map_err(|e| format!("{key}: {e}"))?;
    }
    let ts = fm.get_str("lmm_created_at").unwrap_or_default();
    if parse_created_at(ts).is_none() {
        return Err(format!("lmm_created_at has an invalid format: {ts:?}"));
    }
    // §7.3 checks only the types of the optional keys; the `lmm_thread_id`
    // value rule is enforced when sending, not when receiving.
    Ok(())
}

/// Receive-time inspection of an original message (§7.3).
pub fn inspect(dir: &Path) -> Result<Inspected, MsgError> {
    let listing = list(dir)?;
    let data = read_index(dir)?;
    let doc = frontmatter::parse(&data).map_err(MsgError::Invalid)?;
    if !doc.has_frontmatter {
        return Err(MsgError::Invalid("index.md has no front matter".into()));
    }
    check_frontmatter(&doc.fm).map_err(MsgError::Invalid)?;
    let id = doc.fm.get_str("lmm_id").unwrap_or_default().to_string();
    let from = doc.fm.get_str("lmm_from").unwrap_or_default().to_string();
    let to = doc.fm.get_str("lmm_to").unwrap_or_default().to_string();
    Ok(Inspected {
        listing,
        doc,
        id,
        from,
        to,
    })
}

/// Best effort read of `lmm_id` from a message directory.
pub fn read_id(dir: &Path) -> Option<String> {
    let data = read_index(dir).ok()?;
    let doc = frontmatter::parse(&data).ok()?;
    doc.fm.get_str("lmm_id").map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn good_fm() -> &'static str {
        "---\nlmm_id: a1b2c3d4-e5f6-4890-abcd-ef1234567890\nlmm_from: alpha\nlmm_to: beta\nlmm_created_at: \"2026-10-05T14:30:05.123456Z\"\n---\nhi\n"
    }

    #[test]
    fn naming() {
        let t = parse_created_at("2026-10-05T14:30:05.123456Z").unwrap();
        assert_eq!(
            dir_name(t, "alpha", "beta", "a1b2c3d4-e5f6-4890-abcd-ef1234567890"),
            "20261005-143005.123456_alpha_to_beta_a1b2c3d4"
        );
        assert_eq!(format_created_at(t), "2026-10-05T14:30:05.123456Z");
        assert!(parse_created_at("2026-10-05T14:30:05Z").is_none());
        assert!(parse_created_at("2026-10-05T14:30:05.123456+00:00").is_none());
        assert!(parse_created_at("2026-13-05T14:30:05.123456Z").is_none());
        let now = format_created_at(now_micros());
        assert!(parse_created_at(&now).is_some(), "{now}");
    }

    #[test]
    fn uuid_and_thread() {
        assert!(is_uuid(&new_id()));
        assert!(!is_uuid("A1B2C3D4-E5F6-4890-ABCD-EF1234567890"));
        assert!(!is_uuid("a1b2c3d4e5f64890abcdef1234567890"));
        assert!(validate_thread_id("task-042").is_ok());
        assert!(validate_thread_id("").is_err());
        assert!(validate_thread_id(&"x".repeat(129)).is_err());
        assert!(validate_thread_id("a\nb").is_err());
    }

    #[test]
    fn listing_rules() {
        let t = tempfile::tempdir().unwrap();
        let d = t.path().join("m");
        fs::create_dir(&d).unwrap();
        assert!(matches!(list(&d), Err(MsgError::Invalid(_))));
        fs::write(d.join("Index.md"), "x").unwrap();
        assert!(
            matches!(list(&d), Err(MsgError::Invalid(_))),
            "Index.md is not index.md"
        );
        fs::write(d.join("index.md"), "x").unwrap();
        assert!(matches!(list(&d), Err(MsgError::Invalid(m)) if m.contains("case")));
        fs::remove_file(d.join("Index.md")).unwrap();
        fs::write(d.join("a.txt"), "yy").unwrap();
        let l = list(&d).unwrap();
        assert_eq!(l.attachments, vec![OsString::from("a.txt")]);
        assert_eq!(l.total_size, 3);
        fs::write(d.join(".hidden"), "").unwrap();
        assert!(matches!(list(&d), Err(MsgError::Invalid(_))));
        fs::remove_file(d.join(".hidden")).unwrap();
        fs::create_dir(d.join("sub")).unwrap();
        assert!(matches!(list(&d), Err(MsgError::Invalid(_))));
        fs::remove_dir(d.join("sub")).unwrap();
        std::os::unix::fs::symlink("/etc/passwd", d.join("link")).unwrap();
        assert!(matches!(list(&d), Err(MsgError::Invalid(_))));
        fs::remove_file(d.join("link")).unwrap();
        assert!(validate_file_name(&[b'a'; 256]).is_err());
    }

    #[test]
    fn inspection() {
        let t = tempfile::tempdir().unwrap();
        let d = t.path().join("m");
        fs::create_dir(&d).unwrap();
        fs::write(d.join("index.md"), good_fm()).unwrap();
        let i = inspect(&d).unwrap();
        assert_eq!(i.from, "alpha");
        // §7.3 does not check the lmm_thread_id value rule (only its type).
        let long_thread = good_fm().replace(
            "lmm_to: beta",
            &format!("lmm_to: beta\nlmm_thread_id: {}", "x".repeat(129)),
        );
        fs::write(d.join("index.md"), long_thread).unwrap();
        assert!(inspect(&d).is_ok());
        for bad in [
            "no front matter",
            "---\nlmm_id: x\n---\n",
            &good_fm().replace("alpha", "Alpha"),
            &good_fm().replace("a1b2c3d4-", "zzzz"),
            &good_fm().replace(".123456Z", "Z"),
            &good_fm().replace("lmm_to: beta", "lmm_to: beta\nlmm_thread_id: 42"),
        ] {
            fs::write(d.join("index.md"), bad).unwrap();
            assert!(matches!(inspect(&d), Err(MsgError::Invalid(_))), "{bad}");
        }
    }
}
