//! Front matter of `index.md` (§5.3, §5.4).

use crate::yaml::{self, Map, Node};

/// System keys in output order (§5.3).
pub const LMM_KEY_ORDER: &[&str] = &[
    "lmm_id",
    "lmm_from",
    "lmm_to",
    "lmm_created_at",
    "lmm_subject",
    "lmm_thread_id",
    "lmm_in_reply_to",
    "lmm_forwarded",
];

/// A parsed `index.md`.
#[derive(Debug, Clone, Default)]
pub struct Document {
    /// Whether a front matter block was present.
    pub has_frontmatter: bool,
    /// Front matter (empty if absent).
    pub fm: Map,
    /// Body bytes after the closing `---` line (or the whole file).
    pub body: Vec<u8>,
}

/// Split `index.md` into front matter text and body (§5.4).
/// Returns `(None, body)` when there is no front matter.
pub fn split(data: &[u8]) -> Result<(Option<&[u8]>, &[u8]), String> {
    let data = data.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(data);
    let (first, rest) = next_line(data);
    if !is_delim(first) {
        return Ok((None, data));
    }
    let Some(rest) = rest else {
        return Err("front matter is not closed (missing `---` line)".into());
    };
    let mut pos = 0;
    let mut cur = rest;
    loop {
        let (line, next) = next_line(cur);
        if is_delim(line) {
            let fm = &rest[..pos];
            let body = next.unwrap_or(&[]);
            return Ok((Some(fm), body));
        }
        match next {
            Some(n) => {
                pos += cur.len() - n.len();
                cur = n;
            }
            None => return Err("front matter is not closed (missing `---` line)".into()),
        }
    }
}

/// Returns the line (without terminator) and the remainder after `\n`
/// (`None` if the line was not terminated).
fn next_line(data: &[u8]) -> (&[u8], Option<&[u8]>) {
    match data.iter().position(|&b| b == b'\n') {
        Some(i) => (&data[..i], Some(&data[i + 1..])),
        None => (data, None),
    }
}

fn is_delim(line: &[u8]) -> bool {
    let line = line.strip_suffix(b"\r").unwrap_or(line);
    line == b"---"
}

/// Parse `index.md` content.
pub fn parse(data: &[u8]) -> Result<Document, String> {
    let (fm, body) = split(data)?;
    match fm {
        None => Ok(Document {
            has_frontmatter: false,
            fm: Map::new(),
            body: body.to_vec(),
        }),
        Some(text) => {
            let text = std::str::from_utf8(text)
                .map_err(|_| "front matter is not valid UTF-8".to_string())?;
            let text = text.replace("\r\n", "\n");
            let fm = yaml::parse_map(&text).map_err(|e| format!("invalid front matter: {e}"))?;
            Ok(Document {
                has_frontmatter: true,
                fm,
                body: body.to_vec(),
            })
        }
    }
}

/// Check the types of `lmm_` keys (§5.4).
pub fn validate_lmm_types(fm: &Map) -> Result<(), String> {
    for (k, v) in fm.iter() {
        if !k.starts_with("lmm_") {
            continue;
        }
        if k == "lmm_forwarded" {
            validate_forwarded_shape(v)?;
        } else if v.as_str().is_none() {
            return Err(format!("{k} must be a string, found {}", v.type_name()));
        }
    }
    Ok(())
}

/// Keys of each `lmm_forwarded` element (§6.6).
pub const FORWARD_KEYS: &[&str] = &["id", "from", "to", "created_at"];

fn validate_forwarded_shape(v: &Node) -> Result<(), String> {
    let Node::Seq(items) = v else {
        return Err(format!(
            "lmm_forwarded must be a sequence, found {}",
            v.type_name()
        ));
    };
    for (i, item) in items.iter().enumerate() {
        let Node::Map(m) = item else {
            return Err(format!("lmm_forwarded[{i}] must be a mapping"));
        };
        for key in FORWARD_KEYS {
            match m.get(key) {
                Some(Node::Str(_)) => {}
                Some(other) => {
                    return Err(format!(
                        "lmm_forwarded[{i}].{key} must be a string, found {}",
                        other.type_name()
                    ));
                }
                None => return Err(format!("lmm_forwarded[{i}] is missing {key}")),
            }
        }
        if let Some((k, _)) = m.iter().find(|(k, _)| !FORWARD_KEYS.contains(&k.as_str())) {
            return Err(format!("lmm_forwarded[{i}] has an unknown key {k:?}"));
        }
    }
    Ok(())
}

/// Order a front matter: known `lmm_` keys in table order, then the rest
/// in input order.
pub fn ordered(fm: &Map) -> Map {
    let mut out = Map::new();
    for k in LMM_KEY_ORDER {
        if let Some(v) = fm.get(k) {
            out.insert(*k, v.clone());
        }
    }
    for (k, v) in fm.iter() {
        if !LMM_KEY_ORDER.contains(&k.as_str()) {
            out.insert(k.clone(), v.clone());
        }
    }
    out
}

/// Render `index.md` with front matter and body.
pub fn render(fm: &Map, body: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(b"---\n");
    out.extend_from_slice(yaml::emit_map(&ordered(fm)).as_bytes());
    out.extend_from_slice(b"---\n");
    out.extend_from_slice(body);
    out
}

/// Custom key rule for `--meta` and `post.meta` (§6.2).
pub fn validate_meta_key(key: &str) -> Result<(), String> {
    if key.is_empty() {
        return Err("meta key must not be empty".into());
    }
    if !key
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
    {
        return Err(format!(
            "invalid meta key {key:?}: only A-Z a-z 0-9 _ . - are allowed"
        ));
    }
    if key.starts_with("lmm_") {
        return Err(format!("invalid meta key {key:?}: lmm_ keys are reserved"));
    }
    Ok(())
}

/// Parse a `KEY=VALUE` option.
pub fn parse_meta_arg(s: &str) -> Result<(String, String), String> {
    let (k, v) = s
        .split_once('=')
        .ok_or_else(|| format!("invalid --meta {s:?}: expected KEY=VALUE"))?;
    validate_meta_key(k)?;
    Ok((k.to_string(), v.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_variants() {
        assert_eq!(split(b"hello\n").unwrap(), (None, &b"hello\n"[..]));
        assert_eq!(
            split(b"----\nx\n----\n").unwrap(),
            (None, &b"----\nx\n----\n"[..])
        );
        let (fm, body) = split(b"\xEF\xBB\xBF---\r\na: 1\r\n---\r\nbody\r\n---\n").unwrap();
        assert_eq!(fm.unwrap(), b"a: 1\r\n");
        assert_eq!(body, b"body\r\n---\n");
        let (fm, body) = split(b"---\n---\n").unwrap();
        assert_eq!(fm.unwrap(), b"");
        assert_eq!(body, b"");
        let (fm, body) = split(b"---\na: 1\n---").unwrap();
        assert_eq!(fm.unwrap(), b"a: 1\n");
        assert_eq!(body, b"");
        assert!(split(b"---\na: 1\n").is_err());
        assert!(split(b"---\na: 1\n...\n").is_err());
        assert!(split(b"---").is_err());
    }

    #[test]
    fn parse_and_types() {
        let d = parse(b"---\n---\nbody").unwrap();
        assert!(d.has_frontmatter && d.fm.is_empty());
        assert_eq!(d.body, b"body");
        let d = parse(b"---\nlmm_thread_id: 42\n---\n").unwrap();
        assert!(validate_lmm_types(&d.fm).is_err());
        let d = parse(b"---\nlmm_subject: no\n---\n").unwrap();
        assert!(validate_lmm_types(&d.fm).is_ok());
        assert_eq!(d.fm.get_str("lmm_subject"), Some("no"));
        let d = parse(
            b"---\nlmm_forwarded:\n  - id: a\n    from: b\n    to: c\n    created_at: d\n---\n",
        )
        .unwrap();
        assert!(validate_lmm_types(&d.fm).is_ok());
        let d = parse(b"---\nlmm_forwarded:\n  - id: a\n---\n").unwrap();
        assert!(validate_lmm_types(&d.fm).is_err());
        assert!(parse(b"---\n- a\n---\n").is_err());
        assert!(parse(b"---\na: 1\na: 2\n---\n").is_err());
    }

    #[test]
    fn render_orders_keys() {
        let d = parse(b"---\ntags: [x]\nlmm_subject: s\nlmm_id: i\nz: no\n---\nbody\n---\nmore\n")
            .unwrap();
        let out = String::from_utf8(render(&d.fm, &d.body)).unwrap();
        assert_eq!(
            out,
            "---\nlmm_id: i\nlmm_subject: s\ntags:\n  - x\nz: \"no\"\n---\nbody\n---\nmore\n"
        );
    }

    #[test]
    fn meta_keys() {
        assert!(parse_meta_arg("a.B-c_1=x=y").unwrap().1 == "x=y");
        assert!(parse_meta_arg("lmm_x=1").is_err());
        assert!(parse_meta_arg("a b=1").is_err());
        assert!(parse_meta_arg("novalue").is_err());
        assert!(parse_meta_arg("=v").is_err());
    }
}
