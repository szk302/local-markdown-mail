//! Layered configuration (§10).

use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::address;
use crate::env;
use crate::error::{LmmError, Result, warn};
use crate::frontmatter::validate_meta_key;
use crate::yaml::{self, Map, Node};

/// Where a value came from.
#[derive(Debug, Clone, PartialEq)]
pub enum Origin {
    File(PathBuf),
    Env(&'static str),
    Default,
}

impl Origin {
    pub fn describe(&self) -> String {
        match self {
            Origin::File(p) => p.display().to_string(),
            Origin::Env(name) => (*name).to_string(),
            Origin::Default => "(default)".to_string(),
        }
    }
}

/// A configured value with its origin.
#[derive(Debug, Clone)]
pub struct Val<T> {
    pub value: T,
    /// Value as YAML node (for `config show`).
    pub node: Node,
    pub origin: Origin,
    /// Directory of the file the value came from (for `{config_dir}`).
    pub config_dir: Option<PathBuf>,
    /// Literal values (environment variables) are not placeholder-expanded.
    pub literal: bool,
    /// Raw bytes of a literal value from the environment (may be non-UTF-8).
    pub raw: Option<OsString>,
}

impl<T> Val<T> {
    fn default(value: T, node: Node) -> Self {
        Val {
            value,
            node,
            origin: Origin::Default,
            config_dir: None,
            literal: false,
            raw: None,
        }
    }
}

impl Val<String> {
    /// Resolve a path-valued setting (§10.6).
    pub fn resolve_path(&self, key: &str, name: Option<&str>, cwd: &Path) -> Result<PathBuf> {
        let s: OsString = match &self.raw {
            Some(raw) => raw.clone(),
            None if self.literal => self.value.clone().into(),
            None => expand_placeholders_os(&self.value, name, cwd, self.config_dir.as_deref())
                .map_err(|e| LmmError::usage(format!("{key} ({}): {e}", self.origin.describe())))?,
        };
        let p = env::expand_tilde(&s)?;
        Ok(env::absolute_from(cwd, &p))
    }

    /// The value as raw bytes (environment values keep non-UTF-8 bytes).
    pub fn os_value(&self) -> OsString {
        self.raw
            .clone()
            .unwrap_or_else(|| self.value.clone().into())
    }

    /// Expand placeholders of a string-valued setting (literal values from
    /// environment variables are returned as is).
    pub fn resolve_str(&self, key: &str, name: Option<&str>, cwd: &Path) -> Result<String> {
        if self.literal {
            return Ok(self.value.clone());
        }
        expand_placeholders(&self.value, name, cwd, self.config_dir.as_deref())
            .map_err(|e| LmmError::usage(format!("{key} ({}): {e}", self.origin.describe())))
    }
}

/// Effective configuration.
#[derive(Debug, Clone)]
pub struct Config {
    /// Loaded files, highest priority first.
    pub files: Vec<PathBuf>,
    pub receive_dir: Val<String>,
    pub draft_dir: Val<String>,
    pub hook_cmd: Option<Val<String>>,
    pub hook_timeout: Val<f64>,
    pub hook_max_messages: Val<usize>,
    pub post_reply_to: Option<Val<String>>,
    pub post_meta: Vec<(String, Val<String>)>,
    pub max_message_size: Val<u64>,
}

pub const DEFAULT_RECEIVE_DIR: &str = ".lmm-inbox/{name}";
pub const DEFAULT_DRAFT_DIR: &str = ".lmm-drafts/{name}";

impl Default for Config {
    fn default() -> Self {
        Config {
            files: Vec::new(),
            receive_dir: Val::default(
                DEFAULT_RECEIVE_DIR.into(),
                Node::Str(DEFAULT_RECEIVE_DIR.into()),
            ),
            draft_dir: Val::default(
                DEFAULT_DRAFT_DIR.into(),
                Node::Str(DEFAULT_DRAFT_DIR.into()),
            ),
            hook_cmd: None,
            hook_timeout: Val::default(30.0, Node::Int(30, "30".into())),
            hook_max_messages: Val::default(20, Node::Int(20, "20".into())),
            post_reply_to: None,
            post_meta: Vec::new(),
            max_message_size: Val::default(1 << 30, Node::Str("1G".into())),
        }
    }
}

struct Source<'a> {
    path: &'a Path,
    global: bool,
}

impl Source<'_> {
    fn err(&self, key: &str, msg: impl std::fmt::Display) -> LmmError {
        LmmError::usage(format!(
            "config error in {}: {key}: {msg}",
            self.path.display()
        ))
    }

    fn val<T>(&self, value: T, node: &Node) -> Val<T> {
        Val {
            value,
            node: node.clone(),
            origin: Origin::File(self.path.to_path_buf()),
            config_dir: self.path.parent().map(Path::to_path_buf),
            literal: false,
            raw: None,
        }
    }
}

fn check_no_null(n: &Node, key: &str, src: &Source) -> Result<()> {
    match n {
        Node::Null => Err(src.err(key, "null is not allowed")),
        Node::Seq(v) => v.iter().try_for_each(|x| check_no_null(x, key, src)),
        Node::Map(m) => m
            .iter()
            .try_for_each(|(k, v)| check_no_null(v, &format!("{key}.{k}"), src)),
        _ => Ok(()),
    }
}

fn as_map<'a>(n: &'a Node, key: &str, src: &Source) -> Result<&'a Map> {
    match n {
        Node::Map(m) => Ok(m),
        other => Err(src.err(
            key,
            format!("must be a mapping, found {}", other.type_name()),
        )),
    }
}

fn as_string(n: &Node, key: &str, src: &Source) -> Result<String> {
    match n {
        Node::Str(s) => Ok(s.clone()),
        other => Err(src.err(
            key,
            format!("must be a string, found {}", other.type_name()),
        )),
    }
}

fn unknown(key: &str, src: &Source) {
    warn(format!(
        "unknown config key {key} in {} (ignored)",
        src.path.display()
    ));
}

/// Parse a size like `1024`, `512K`, `10M`, `1G` (§10.4).
pub fn parse_size(s: &str) -> Option<u64> {
    let s = s.trim();
    let (num, mult) = match s.as_bytes().last()? {
        b'K' | b'k' => (&s[..s.len() - 1], 1u64 << 10),
        b'M' | b'm' => (&s[..s.len() - 1], 1u64 << 20),
        b'G' | b'g' => (&s[..s.len() - 1], 1u64 << 30),
        _ => (s, 1),
    };
    if num.is_empty() || !num.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    num.parse::<u64>().ok()?.checked_mul(mult)
}

/// Placeholder expansion (§10.6) producing raw bytes, so that `{cwd}` and
/// `{config_dir}` keep non-UTF-8 path bytes intact (F03-1).
pub fn expand_placeholders_os(
    s: &str,
    name: Option<&str>,
    cwd: &Path,
    config_dir: Option<&Path>,
) -> std::result::Result<OsString, String> {
    let mut out = OsString::new();
    let mut rest = s;
    while let Some(i) = rest.find('{') {
        out.push(&rest[..i]);
        rest = &rest[i..];
        if let Some(r) = rest.strip_prefix("{{") {
            out.push("{");
            rest = r;
            continue;
        }
        let Some(end) = rest.find('}') else {
            return Err(format!(
                "unterminated placeholder in {s:?} (write `{{{{` for a literal brace)"
            ));
        };
        match &rest[1..end] {
            "name" => match name {
                Some(n) => out.push(n),
                None => return Err(format!("{{name}} in {s:?} requires LMM_NAME to be set")),
            },
            "cwd" => out.push(cwd.as_os_str()),
            "config_dir" => out.push(config_dir.unwrap_or(cwd).as_os_str()),
            other => return Err(format!("unknown placeholder {{{other}}} in {s:?}")),
        }
        rest = &rest[end + 1..];
    }
    out.push(rest);
    Ok(out)
}

/// Placeholder expansion for text values (`post.reply_to`, `post.meta`).
/// A path that is not valid UTF-8 can't be represented: config error.
pub fn expand_placeholders(
    s: &str,
    name: Option<&str>,
    cwd: &Path,
    config_dir: Option<&Path>,
) -> std::result::Result<String, String> {
    expand_placeholders_os(s, name, cwd, config_dir)?
        .into_string()
        .map_err(|_| format!("{s:?} expands to a path that is not valid UTF-8"))
}

fn check_placeholders(s: &str, key: &str, src: &Source) -> Result<()> {
    // Expand with dummy values to validate the syntax only.
    expand_placeholders(s, Some("x"), Path::new("/"), Some(Path::new("/")))
        .map(|_| ())
        .map_err(|e| src.err(key, e))
}

impl Config {
    fn apply(&mut self, map: &Map, src: &Source) -> Result<()> {
        for (k, v) in map.iter() {
            check_no_null(v, k, src)?;
        }
        for (k, v) in map.iter() {
            match k.as_str() {
                "root" => {
                    if !matches!(v, Node::Bool(_)) {
                        return Err(src.err("root", "must be a boolean"));
                    }
                }
                "receive" | "draft" => {
                    for (sk, sv) in as_map(v, k, src)?.iter() {
                        let key = format!("{k}.{sk}");
                        if sk == "dir" {
                            let s = as_string(sv, &key, src)?;
                            check_placeholders(&s, &key, src)?;
                            let val = src.val(s, sv);
                            if k == "receive" {
                                self.receive_dir = val;
                            } else {
                                self.draft_dir = val;
                            }
                        } else {
                            unknown(&key, src);
                        }
                    }
                }
                "hook" => {
                    for (sk, sv) in as_map(v, k, src)?.iter() {
                        let key = format!("hook.{sk}");
                        match sk.as_str() {
                            "cmd" => {
                                let s = as_string(sv, &key, src)?;
                                self.hook_cmd = Some(src.val(s, sv));
                            }
                            "timeout" => {
                                let t = match sv {
                                    Node::Int(i, _) => *i as f64,
                                    Node::Float(f, _) => *f,
                                    Node::BigInt(_) => {
                                        return Err(src.err(&key, "is too large"));
                                    }
                                    _ => return Err(src.err(&key, "must be a number")),
                                };
                                if !(t > 0.0 && t.is_finite()) {
                                    return Err(src.err(&key, "must be greater than 0"));
                                }
                                if Duration::try_from_secs_f64(t).is_err() {
                                    return Err(src.err(&key, "is too large"));
                                }
                                self.hook_timeout = src.val(t, sv);
                            }
                            "max_messages" => match sv {
                                Node::Int(i, _) if (1..=30).contains(i) => {
                                    self.hook_max_messages = src.val(*i as usize, sv);
                                }
                                _ => return Err(src.err(&key, "must be an integer from 1 to 30")),
                            },
                            _ => unknown(&key, src),
                        }
                    }
                }
                "post" => {
                    for (sk, sv) in as_map(v, k, src)?.iter() {
                        let key = format!("post.{sk}");
                        match sk.as_str() {
                            "reply_to" => {
                                let s = as_string(sv, &key, src)?;
                                check_placeholders(&s, &key, src)?;
                                if !s.contains('{') {
                                    address::validate(&s).map_err(|e| src.err(&key, e))?;
                                }
                                self.post_reply_to = Some(src.val(s, sv));
                            }
                            "meta" => {
                                for (mk, mv) in as_map(sv, &key, src)?.iter() {
                                    let mkey = format!("post.meta.{mk}");
                                    validate_meta_key(mk).map_err(|e| src.err(&mkey, e))?;
                                    let s = as_string(mv, &mkey, src)?;
                                    check_placeholders(&s, &mkey, src)?;
                                    let val = src.val(s, mv);
                                    if let Some(slot) =
                                        self.post_meta.iter_mut().find(|(n, _)| n == mk)
                                    {
                                        slot.1 = val;
                                    } else {
                                        self.post_meta.push((mk.clone(), val));
                                    }
                                }
                            }
                            _ => unknown(&key, src),
                        }
                    }
                }
                "max_message_size" => {
                    // Directory configs ignore the key entirely, even if the
                    // value is invalid (§10.4).
                    if !src.global {
                        warn(format!(
                            "max_message_size is only valid in the global config; ignored in {}",
                            src.path.display()
                        ));
                        continue;
                    }
                    let size = match v {
                        Node::Int(i, _) if *i >= 0 => u64::try_from(*i).ok(),
                        Node::Str(s) => parse_size(s),
                        _ => None,
                    }
                    .ok_or_else(|| {
                        src.err(
                            k,
                            "must be a number of bytes or a string like 512K, 10M, 1G",
                        )
                    })?;
                    self.max_message_size = src.val(size, v);
                }
                _ => unknown(k, src),
            }
        }
        Ok(())
    }

    fn apply_env(&mut self) {
        let lit = |raw: OsString, name: &'static str| {
            let value = raw.to_string_lossy().into_owned();
            Val {
                node: Node::Str(value.clone()),
                value,
                origin: Origin::Env(name),
                config_dir: None,
                literal: true,
                raw: Some(raw),
            }
        };
        if let Some(v) = env::var_os("LMM_RECEIVE_DIR") {
            self.receive_dir = lit(v, "LMM_RECEIVE_DIR");
        }
        if let Some(v) = env::var_os("LMM_DRAFT_DIR") {
            self.draft_dir = lit(v, "LMM_DRAFT_DIR");
        }
        if let Some(v) = env::var_os("LMM_HOOK_CMD") {
            self.hook_cmd = Some(lit(v, "LMM_HOOK_CMD"));
        }
    }

    /// Load configuration for `cwd` (§10.1, §10.2).
    pub fn load(cwd: &Path, lmm_home: &Path) -> Result<Config> {
        let mut cfg = Config::default();
        // (path, global) from weakest to strongest.
        let mut sources: Vec<(PathBuf, Map, bool)> = Vec::new();

        let global = match env::var_os("LMM_CONFIG") {
            Some(p) => {
                let p = env::absolute_from(cwd, Path::new(&p));
                if fs::metadata(&p).is_err() {
                    return Err(LmmError::usage(format!(
                        "LMM_CONFIG file does not exist: {}",
                        p.display()
                    )));
                }
                Some(p)
            }
            None => {
                let p = lmm_home.join("config.yml");
                fs::metadata(&p).is_ok().then_some(p)
            }
        };
        if let Some(p) = global
            && let Some((m, _)) = read_file(&p)?
        {
            sources.push((p, m, true));
        }

        let mut dir_sources: Vec<(PathBuf, Map)> = Vec::new();
        let mut dir = Some(cwd.to_path_buf());
        while let Some(d) = dir {
            let mut stop = false;
            for name in [".lmm.local.yml", ".lmm.yml"] {
                let p = d.join(name);
                if let Some((m, root)) = read_file(&p)? {
                    stop |= root;
                    dir_sources.push((p, m));
                }
            }
            if stop {
                break;
            }
            dir = d.parent().map(Path::to_path_buf);
        }
        // dir_sources is strongest first; apply weakest first.
        for (p, m) in dir_sources.into_iter().rev() {
            sources.push((p, m, false));
        }

        for (p, m, global) in &sources {
            cfg.apply(
                m,
                &Source {
                    path: p,
                    global: *global,
                },
            )?;
        }
        cfg.files = sources.into_iter().rev().map(|(p, _, _)| p).collect();
        cfg.apply_env();
        Ok(cfg)
    }

    /// `post.reply_to`, expanded and validated.
    pub fn reply_to(&self, name: Option<&str>, cwd: &Path) -> Result<Option<String>> {
        let Some(v) = &self.post_reply_to else {
            return Ok(None);
        };
        let s = v.resolve_str("post.reply_to", name, cwd)?;
        address::validate(&s).map_err(|e| {
            LmmError::usage(format!("post.reply_to ({}): {e}", v.origin.describe()))
        })?;
        Ok(Some(s))
    }

    /// `post.meta`, expanded.
    pub fn meta(&self, name: Option<&str>, cwd: &Path) -> Result<Vec<(String, String)>> {
        self.post_meta
            .iter()
            .map(|(k, v)| {
                Ok((
                    k.clone(),
                    v.resolve_str(&format!("post.meta.{k}"), name, cwd)?,
                ))
            })
            .collect()
    }

    /// Render for `lmm config show` (§10.7).
    pub fn show(&self, origin: bool) -> String {
        let mut out = String::new();
        if self.files.is_empty() {
            out.push_str("# Configuration files: (none)\n");
        } else {
            out.push_str("# Configuration files (highest priority first):\n");
            for f in &self.files {
                out.push_str(&format!("#   {}\n", f.display()));
            }
        }
        let line = |out: &mut String, indent: usize, key: &str, node: &Node, o: &Origin| {
            let pad = " ".repeat(indent);
            out.push_str(&pad);
            out.push_str(&yaml::emit_key(key, &pad));
            out.push(' ');
            out.push_str(&yaml::emit_inline(node).unwrap_or_default());
            if origin {
                out.push_str("  # ");
                out.push_str(&o.describe());
            }
            out.push('\n');
        };
        out.push_str("receive:\n");
        line(
            &mut out,
            2,
            "dir",
            &self.receive_dir.node,
            &self.receive_dir.origin,
        );
        out.push_str("draft:\n");
        line(
            &mut out,
            2,
            "dir",
            &self.draft_dir.node,
            &self.draft_dir.origin,
        );
        out.push_str("hook:\n");
        if let Some(c) = &self.hook_cmd {
            line(&mut out, 2, "cmd", &c.node, &c.origin);
        }
        line(
            &mut out,
            2,
            "timeout",
            &self.hook_timeout.node,
            &self.hook_timeout.origin,
        );
        line(
            &mut out,
            2,
            "max_messages",
            &self.hook_max_messages.node,
            &self.hook_max_messages.origin,
        );
        if self.post_reply_to.is_some() || !self.post_meta.is_empty() {
            out.push_str("post:\n");
            if let Some(r) = &self.post_reply_to {
                line(&mut out, 2, "reply_to", &r.node, &r.origin);
            }
            if !self.post_meta.is_empty() {
                out.push_str("  meta:\n");
                for (k, v) in &self.post_meta {
                    line(&mut out, 4, k, &v.node, &v.origin);
                }
            }
        }
        line(
            &mut out,
            0,
            "max_message_size",
            &self.max_message_size.node,
            &self.max_message_size.origin,
        );
        out
    }
}

/// Read and parse a config file. `Ok(None)` if missing or unreadable
/// (unreadable files produce a warning). Returns the map and `root`.
fn read_file(p: &Path) -> Result<Option<(Map, bool)>> {
    let data = match fs::read(p) {
        Ok(d) => d,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) if e.kind() == io::ErrorKind::NotADirectory => return Ok(None),
        Err(e) => {
            warn(format!("cannot read config {} (skipped): {e}", p.display()));
            return Ok(None);
        }
    };
    let text = String::from_utf8(data).map_err(|_| {
        LmmError::usage(format!("config error in {}: not valid UTF-8", p.display()))
    })?;
    let map = yaml::parse_map(&text)
        .map_err(|e| LmmError::usage(format!("config error in {}: {e}", p.display())))?;
    let root = matches!(map.get("root"), Some(Node::Bool(true)));
    Ok(Some((map, root)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes() {
        assert_eq!(parse_size("1G"), Some(1 << 30));
        assert_eq!(parse_size("512K"), Some(512 * 1024));
        assert_eq!(parse_size("10M"), Some(10 << 20));
        assert_eq!(parse_size("100"), Some(100));
        assert_eq!(parse_size("1.5G"), None);
        assert_eq!(parse_size("G"), None);
    }

    #[test]
    fn placeholders() {
        let cwd = Path::new("/w");
        let cd = Some(Path::new("/c"));
        assert_eq!(
            expand_placeholders("{config_dir}/.lmm-inbox/{name}", Some("a"), cwd, cd).unwrap(),
            "/c/.lmm-inbox/a"
        );
        assert_eq!(
            expand_placeholders("{cwd}/{{x}", None, cwd, cd).unwrap(),
            "/w/{x}"
        );
        assert!(expand_placeholders("{nope}", None, cwd, cd).is_err());
        assert!(expand_placeholders("{name}", None, cwd, cd).is_err());
        assert!(expand_placeholders("{name", Some("a"), cwd, cd).is_err());
    }
}
