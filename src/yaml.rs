//! Minimal YAML layer (§5.4): YAML 1.2 core schema parsing on top of
//! `saphyr-parser` events, an order preserving node tree, an emitter that
//! quotes strings ambiguous to YAML 1.1 parsers, and JSON conversion.

use saphyr_parser::{Event, Parser, ScalarStyle, Tag};

/// Maximum number of nodes (keys and values), counted after alias
/// expansion (§5.4).
pub const MAX_NODES: usize = 100_000;
/// Maximum collection nesting depth, after alias expansion (§5.4).
pub const MAX_DEPTH: usize = 128;

/// A YAML value.
#[derive(Debug, Clone, PartialEq)]
pub enum Node {
    Null,
    Bool(bool),
    /// Integer with its source representation.
    Int(i128, String),
    /// Integer too large for `i128`, kept as its source representation
    /// (core schema integer syntax: decimal, `0x...` or `0o...`).
    BigInt(String),
    /// Float with its source representation.
    Float(f64, String),
    Str(String),
    Seq(Vec<Node>),
    Map(Map),
}

/// An insertion ordered map with string keys.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Map {
    entries: Vec<(String, Node)>,
}

impl Map {
    pub fn new() -> Self {
        Map::default()
    }

    pub fn get(&self, key: &str) -> Option<&Node> {
        self.entries.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    pub fn contains_key(&self, key: &str) -> bool {
        self.get(key).is_some()
    }

    /// Insert or replace (keeping the original position).
    pub fn insert(&mut self, key: impl Into<String>, value: Node) {
        let key = key.into();
        if let Some(slot) = self.entries.iter_mut().find(|(k, _)| *k == key) {
            slot.1 = value;
        } else {
            self.entries.push((key, value));
        }
    }

    pub fn remove(&mut self, key: &str) -> Option<Node> {
        let idx = self.entries.iter().position(|(k, _)| k == key)?;
        Some(self.entries.remove(idx).1)
    }

    pub fn retain(&mut self, mut f: impl FnMut(&str, &Node) -> bool) {
        self.entries.retain(|(k, v)| f(k, v));
    }

    pub fn iter(&self) -> impl Iterator<Item = (&String, &Node)> {
        self.entries.iter().map(|(k, v)| (k, v))
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn get_str(&self, key: &str) -> Option<&str> {
        self.get(key).and_then(Node::as_str)
    }
}

impl Node {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Node::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn type_name(&self) -> &'static str {
        match self {
            Node::Null => "null",
            Node::Bool(_) => "boolean",
            Node::Int(..) | Node::BigInt(_) => "integer",
            Node::Float(..) => "float",
            Node::Str(_) => "string",
            Node::Seq(_) => "sequence",
            Node::Map(_) => "mapping",
        }
    }
}

/// Parse a YAML stream containing at most one document.
/// Returns `None` if the stream has no document (empty or comments only).
pub fn parse(text: &str) -> Result<Option<Node>, String> {
    let mut b = Builder::default();
    let parser = Parser::new_from_str(text);
    let mut docs = 0;
    for ev in parser {
        let (ev, span) = ev.map_err(|e| e.to_string())?;
        match ev {
            Event::DocumentStart(_) => {
                docs += 1;
                if docs > 1 {
                    return Err("multiple YAML documents".into());
                }
            }
            other => b
                .event(other)
                .map_err(|m| format!("{m} at line {}", span.start.line()))?,
        }
    }
    Ok(b.root)
}

/// Parse a YAML document whose top level must be a mapping.
/// An empty document is an empty mapping.
pub fn parse_map(text: &str) -> Result<Map, String> {
    match parse(text)? {
        None => Ok(Map::new()),
        Some(Node::Null) if is_blank(text) => Ok(Map::new()),
        Some(Node::Map(m)) => Ok(m),
        Some(other) => Err(format!(
            "top level must be a mapping, found {}",
            other.type_name()
        )),
    }
}

fn is_blank(text: &str) -> bool {
    text.lines()
        .all(|l| l.trim().is_empty() || l.trim_start().starts_with('#'))
}

enum Frame {
    Seq(Vec<Node>, usize),
    Map(Map, Option<String>, usize),
}

#[derive(Default)]
struct Builder {
    stack: Vec<Frame>,
    anchors: std::collections::HashMap<usize, Node>,
    root: Option<Node>,
    count: usize,
}

fn count_nodes(n: &Node) -> usize {
    match n {
        Node::Seq(v) => 1 + v.iter().map(count_nodes).sum::<usize>(),
        Node::Map(m) => 1 + m.iter().map(|(_, v)| 1 + count_nodes(v)).sum::<usize>(),
        _ => 1,
    }
}

/// Collection nesting depth: scalars 0, `[x]` 1, `[[x]]` 2, ...
fn node_depth(n: &Node) -> usize {
    match n {
        Node::Seq(v) => 1 + v.iter().map(node_depth).max().unwrap_or(0),
        Node::Map(m) => 1 + m.iter().map(|(_, v)| node_depth(v)).max().unwrap_or(0),
        _ => 0,
    }
}

impl Builder {
    fn event(&mut self, ev: Event<'_>) -> Result<(), String> {
        match ev {
            Event::StreamStart | Event::StreamEnd | Event::DocumentEnd | Event::Nothing => Ok(()),
            Event::DocumentStart(_) => Ok(()),
            Event::Alias(id) => {
                let node = self
                    .anchors
                    .get(&id)
                    .cloned()
                    .ok_or_else(|| "unknown alias".to_string())?;
                self.count += count_nodes(&node);
                // The expanded tree must respect the depth limit too (R03-3).
                if self.stack.len() + node_depth(&node) > MAX_DEPTH {
                    return Err("nesting too deep".into());
                }
                self.push(node, 0)
            }
            Event::Scalar(value, style, anchor, tag) => {
                let node = resolve_scalar(&value, style, tag.as_deref())?;
                self.push(node, anchor)
            }
            Event::SequenceStart(anchor, tag) => {
                check_collection_tag(tag.as_deref(), "seq")?;
                if self.stack.len() >= MAX_DEPTH {
                    return Err("nesting too deep".into());
                }
                self.stack.push(Frame::Seq(Vec::new(), anchor));
                Ok(())
            }
            Event::MappingStart(anchor, tag) => {
                check_collection_tag(tag.as_deref(), "map")?;
                if self.stack.len() >= MAX_DEPTH {
                    return Err("nesting too deep".into());
                }
                self.stack.push(Frame::Map(Map::new(), None, anchor));
                Ok(())
            }
            Event::SequenceEnd => match self.stack.pop() {
                Some(Frame::Seq(v, anchor)) => self.push(Node::Seq(v), anchor),
                _ => Err("unbalanced sequence".into()),
            },
            Event::MappingEnd => match self.stack.pop() {
                Some(Frame::Map(m, None, anchor)) => self.push(Node::Map(m), anchor),
                _ => Err("unbalanced mapping".into()),
            },
        }
    }

    fn push(&mut self, node: Node, anchor: usize) -> Result<(), String> {
        self.count += 1;
        if self.count > MAX_NODES {
            return Err("document too large (alias expansion limit)".into());
        }
        if anchor != 0 {
            self.anchors.insert(anchor, node.clone());
        }
        match self.stack.last_mut() {
            None => {
                self.root = Some(node);
                Ok(())
            }
            Some(Frame::Seq(v, _)) => {
                v.push(node);
                Ok(())
            }
            Some(Frame::Map(m, pending, _)) => match pending.take() {
                None => match node {
                    Node::Str(k) => {
                        if m.contains_key(&k) {
                            return Err(format!("duplicate key {k:?}"));
                        }
                        *pending = Some(k);
                        Ok(())
                    }
                    other => Err(format!(
                        "mapping keys must be strings, found {}",
                        other.type_name()
                    )),
                },
                Some(k) => {
                    m.entries.push((k, node));
                    Ok(())
                }
            },
        }
    }
}

/// The non-specific tag `!` (saphyr-parser: empty handle, suffix `!`).
fn is_non_specific(t: &Tag) -> bool {
    (t.handle.is_empty() && t.suffix == "!") || (t.handle == "!" && t.suffix.is_empty())
}

fn check_collection_tag(tag: Option<&Tag>, expected: &str) -> Result<(), String> {
    match tag {
        None => Ok(()),
        Some(t) if t.is_yaml_core_schema() && t.suffix == expected => Ok(()),
        Some(t) if is_non_specific(t) => Ok(()),
        Some(t) => Err(format!("unsupported tag {t}")),
    }
}

fn resolve_scalar(value: &str, style: ScalarStyle, tag: Option<&Tag>) -> Result<Node, String> {
    if let Some(t) = tag {
        if is_non_specific(t) {
            // Non-specific tag `!`: always a string.
            return Ok(Node::Str(value.to_string()));
        }
        if !t.is_yaml_core_schema() {
            return Err(format!("unsupported tag {t}"));
        }
        return match t.suffix.as_str() {
            "str" => Ok(Node::Str(value.to_string())),
            "null" => match resolve_plain(value) {
                Node::Null => Ok(Node::Null),
                _ => Err(format!("invalid !!null value {value:?}")),
            },
            "bool" => match resolve_plain(value) {
                n @ Node::Bool(_) => Ok(n),
                _ => Err(format!("invalid !!bool value {value:?}")),
            },
            "int" => match resolve_plain(value) {
                n @ (Node::Int(..) | Node::BigInt(_)) => Ok(n),
                _ => Err(format!("invalid !!int value {value:?}")),
            },
            "float" => match resolve_plain(value) {
                n @ Node::Float(..) => Ok(n),
                Node::Int(i, repr) => Ok(Node::Float(i as f64, repr)),
                Node::BigInt(repr) if !repr.contains(['x', 'o']) => {
                    let f = repr.parse::<f64>().unwrap_or(f64::NAN);
                    Ok(Node::Float(f, repr))
                }
                _ => Err(format!("invalid !!float value {value:?}")),
            },
            other => Err(format!("unsupported tag !!{other}")),
        };
    }
    match style {
        ScalarStyle::Plain => Ok(resolve_plain(value)),
        _ => Ok(Node::Str(value.to_string())),
    }
}

/// YAML 1.2 core schema resolution of a plain scalar.
pub fn resolve_plain(s: &str) -> Node {
    match s {
        "" | "~" | "null" | "Null" | "NULL" => return Node::Null,
        "true" | "True" | "TRUE" => return Node::Bool(true),
        "false" | "False" | "FALSE" => return Node::Bool(false),
        ".inf" | ".Inf" | ".INF" | "+.inf" | "+.Inf" | "+.INF" => {
            return Node::Float(f64::INFINITY, s.to_string());
        }
        "-.inf" | "-.Inf" | "-.INF" => return Node::Float(f64::NEG_INFINITY, s.to_string()),
        ".nan" | ".NaN" | ".NAN" => return Node::Float(f64::NAN, s.to_string()),
        _ => {}
    }
    match parse_core_int(s) {
        CoreInt::Fits(i) => return Node::Int(i, s.to_string()),
        CoreInt::TooLarge => return Node::BigInt(s.to_string()),
        CoreInt::NotInt => {}
    }
    if is_core_float(s)
        && let Ok(f) = s.parse::<f64>()
    {
        return Node::Float(f, s.to_string());
    }
    Node::Str(s.to_string())
}

enum CoreInt {
    Fits(i128),
    /// Matches the core schema integer syntax but exceeds `i128`.
    TooLarge,
    NotInt,
}

fn parse_core_int(s: &str) -> CoreInt {
    let (radix, text) = if let Some(h) = s.strip_prefix("0x") {
        if h.is_empty() || !h.bytes().all(|b| b.is_ascii_hexdigit()) {
            return CoreInt::NotInt;
        }
        (16, h)
    } else if let Some(o) = s.strip_prefix("0o") {
        if o.is_empty() || !o.bytes().all(|b| (b'0'..=b'7').contains(&b)) {
            return CoreInt::NotInt;
        }
        (8, o)
    } else {
        let digits = s.strip_prefix(['-', '+']).unwrap_or(s);
        if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return CoreInt::NotInt;
        }
        (10, s.strip_prefix('+').unwrap_or(s))
    };
    match i128::from_str_radix(text, radix) {
        Ok(i) => CoreInt::Fits(i),
        Err(_) => CoreInt::TooLarge,
    }
}

/// `[-+]?(\.[0-9]+|[0-9]+(\.[0-9]*)?)([eE][-+]?[0-9]+)?`
fn is_core_float(s: &str) -> bool {
    let b = s.as_bytes();
    let mut i = 0;
    if i < b.len() && (b[i] == b'-' || b[i] == b'+') {
        i += 1;
    }
    let int_start = i;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    let int_digits = i - int_start;
    if i < b.len() && b[i] == b'.' {
        i += 1;
        let fs = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        if int_digits == 0 && i == fs {
            return false;
        }
    } else if int_digits == 0 {
        return false;
    }
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        i += 1;
        if i < b.len() && (b[i] == b'-' || b[i] == b'+') {
            i += 1;
        }
        let es = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        if i == es {
            return false;
        }
    }
    i == b.len()
}

// ---------------------------------------------------------------------------
// Emitter
// ---------------------------------------------------------------------------

/// Words resolved to booleans or null by YAML 1.1 parsers.
const YAML11_WORDS: &[&str] = &[
    "y", "Y", "yes", "Yes", "YES", "n", "N", "no", "No", "NO", "true", "True", "TRUE", "false",
    "False", "FALSE", "on", "On", "ON", "off", "Off", "OFF", "null", "Null", "NULL", "~", "<<",
    "=",
];

/// Whether a string can be written as a plain scalar and read back as the
/// same string by both YAML 1.2 and YAML 1.1 parsers.
pub fn plain_safe(s: &str) -> bool {
    if s.is_empty() || YAML11_WORDS.contains(&s) {
        return false;
    }
    if !matches!(resolve_plain(s), Node::Str(_)) {
        return false;
    }
    let first = s.chars().next().unwrap_or(' ');
    let last = s.chars().last().unwrap_or(' ');
    // Numbers, dates, times, sexagesimals and special floats all start with
    // a digit, a sign or a dot in YAML 1.1.
    if first.is_ascii_digit() || matches!(first, '-' | '+' | '.') {
        return false;
    }
    if first.is_whitespace() || last.is_whitespace() {
        return false;
    }
    // Indicators and characters with special meaning anywhere.
    for c in s.chars() {
        if matches!(
            c,
            ':' | '#'
                | '{'
                | '}'
                | '['
                | ']'
                | ','
                | '&'
                | '*'
                | '!'
                | '|'
                | '>'
                | '\''
                | '"'
                | '%'
                | '@'
                | '`'
                | '\\'
                | '?'
        ) {
            return false;
        }
        if c.is_control() || needs_escape(c) {
            return false;
        }
    }
    true
}

fn needs_escape(c: char) -> bool {
    c.is_control() || matches!(c, '\u{FEFF}' | '\u{2028}' | '\u{2029}' | '\u{85}')
}

/// Double-quoted YAML string.
pub fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            '\0' => out.push_str("\\0"),
            c if needs_escape(c) => {
                let v = c as u32;
                if v <= 0xFF {
                    out.push_str(&format!("\\x{v:02X}"));
                } else {
                    out.push_str(&format!("\\u{v:04X}"));
                }
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Emit a string scalar.
pub fn emit_str(s: &str) -> String {
    if plain_safe(s) {
        s.to_string()
    } else {
        quote(s)
    }
}

fn format_float(f: f64) -> String {
    if f.is_nan() {
        return ".nan".into();
    }
    if f.is_infinite() {
        return if f > 0.0 {
            ".inf".into()
        } else {
            "-.inf".into()
        };
    }
    // Rust's shortest round trip representation, made acceptable to both
    // YAML 1.1 (requires a dot and a signed exponent) and 1.2.
    let s = format!("{f:?}");
    match s.split_once('e') {
        None => {
            if s.contains('.') {
                s
            } else {
                format!("{s}.0")
            }
        }
        Some((m, e)) => {
            let m = if m.contains('.') {
                m.to_string()
            } else {
                format!("{m}.0")
            };
            let e = if e.starts_with('-') || e.starts_with('+') {
                e.to_string()
            } else {
                format!("+{e}")
            };
            format!("{m}e{e}")
        }
    }
}

/// Emit a scalar or flow-empty collection. `None` for non-empty collections.
pub fn emit_inline(n: &Node) -> Option<String> {
    Some(match n {
        Node::Null => "null".into(),
        Node::Bool(b) => b.to_string(),
        Node::Int(i, _) => i.to_string(),
        Node::BigInt(repr) => big_int_decimal(repr),
        Node::Float(f, _) => format_float(*f),
        Node::Str(s) => emit_str(s),
        Node::Seq(v) if v.is_empty() => "[]".into(),
        Node::Map(m) if m.is_empty() => "{}".into(),
        _ => return None,
    })
}

/// Longest key written in implicit `key: value` form. YAML allows 1024
/// characters including the `:` indicator; stay a little below.
const MAX_IMPLICIT_KEY: usize = 1000;

/// A mapping key followed by `:`. Keys longer than the implicit key limit
/// (1024 characters, YAML 1.2.2 §8.2.2) use the explicit form
/// `? key` / `<pad>:`. `pad` is the indentation of the key line.
pub fn emit_key(key: &str, pad: &str) -> String {
    let k = emit_str(key);
    if k.chars().count() > MAX_IMPLICIT_KEY {
        format!("? {k}\n{pad}:")
    } else {
        format!("{k}:")
    }
}

fn emit_block(n: &Node, indent: usize, out: &mut String) {
    let pad = " ".repeat(indent);
    match n {
        Node::Map(m) => {
            for (k, v) in m.iter() {
                out.push_str(&pad);
                out.push_str(&emit_key(k, &pad));
                match emit_inline(v) {
                    Some(s) => {
                        out.push(' ');
                        out.push_str(&s);
                        out.push('\n');
                    }
                    None => {
                        out.push('\n');
                        emit_block(v, indent + 2, out);
                    }
                }
            }
        }
        Node::Seq(items) => {
            for item in items {
                match emit_inline(item) {
                    Some(s) => {
                        out.push_str(&pad);
                        out.push_str("- ");
                        out.push_str(&s);
                        out.push('\n');
                    }
                    None => {
                        let mut child = String::new();
                        emit_block(item, indent + 2, &mut child);
                        // Replace the first line's indentation with "- ".
                        out.push_str(&pad);
                        out.push_str("- ");
                        out.push_str(&child[indent + 2..]);
                    }
                }
            }
        }
        other => {
            out.push_str(&pad);
            out.push_str(&emit_inline(other).unwrap_or_default());
            out.push('\n');
        }
    }
}

/// Emit a mapping as block YAML (each line ends with `\n`).
pub fn emit_map(m: &Map) -> String {
    let mut out = String::new();
    emit_block(&Node::Map(m.clone()), 0, &mut out);
    out
}

// ---------------------------------------------------------------------------
// JSON conversion
// ---------------------------------------------------------------------------

pub fn to_json(n: &Node) -> serde_json::Value {
    use serde_json::Value;
    match n {
        Node::Null => Value::Null,
        Node::Bool(b) => Value::Bool(*b),
        Node::Int(i, _) => json_number(&i.to_string()),
        Node::BigInt(repr) => json_number(&big_int_decimal(repr)),
        Node::Float(f, repr) => match serde_json::Number::from_f64(*f) {
            Some(num) => Value::Number(num),
            None => Value::String(repr.clone()),
        },
        Node::Str(s) => Value::String(s.clone()),
        Node::Seq(v) => Value::Array(v.iter().map(to_json).collect()),
        Node::Map(m) => map_to_json(m),
    }
}

/// An exact JSON number from a canonical decimal integer
/// (`serde_json` is built with `arbitrary_precision`).
fn json_number(decimal: &str) -> serde_json::Value {
    decimal
        .parse::<serde_json::Number>()
        .map(serde_json::Value::Number)
        .unwrap_or_else(|_| serde_json::Value::String(decimal.to_string()))
}

/// Canonical decimal form (no `+`, no leading zeros) of a core schema
/// integer literal of any size.
fn big_int_decimal(repr: &str) -> String {
    let (neg, radix, digits) = if let Some(h) = repr.strip_prefix("0x") {
        (false, 16, h)
    } else if let Some(o) = repr.strip_prefix("0o") {
        (false, 8, o)
    } else if let Some(d) = repr.strip_prefix('-') {
        (true, 10, d)
    } else {
        (false, 10, repr.strip_prefix('+').unwrap_or(repr))
    };
    // Little-endian base-10 digits, built by repeated multiply-add.
    let mut dec: Vec<u32> = vec![0];
    for c in digits.chars() {
        let Some(d) = c.to_digit(radix) else {
            return repr.to_string();
        };
        let mut carry = d;
        for x in dec.iter_mut() {
            let v = *x * radix + carry;
            *x = v % 10;
            carry = v / 10;
        }
        while carry > 0 {
            dec.push(carry % 10);
            carry /= 10;
        }
    }
    while dec.len() > 1 && dec.last() == Some(&0) {
        dec.pop();
    }
    let mut out = String::with_capacity(dec.len() + 1);
    if neg && dec != [0] {
        out.push('-');
    }
    out.extend(
        dec.iter()
            .rev()
            .map(|d| char::from_digit(*d, 10).unwrap_or('0')),
    );
    out
}

pub fn map_to_json(m: &Map) -> serde_json::Value {
    let mut obj = serde_json::Map::new();
    for (k, v) in m.iter() {
        obj.insert(k.clone(), to_json(v));
    }
    serde_json::Value::Object(obj)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(text: &str) -> Map {
        parse_map(text).unwrap()
    }

    #[test]
    fn core_schema_strings() {
        let map = m("a: no\nb: on\nc: 2026-10-05\nd: yes\ne: off\nf: 12:30\n");
        for k in ["a", "b", "c", "d", "e", "f"] {
            assert!(matches!(map.get(k), Some(Node::Str(_))), "{k}");
        }
    }

    #[test]
    fn core_schema_types() {
        let map = m(
            "a: 1\nb: 1.10\nc: true\nd: null\ne: ~\nf:\ng: .inf\nh: 0x1F\ni: 0o17\nj: \"1\"\nk: 1e3\nl: -.5\n",
        );
        assert!(matches!(map.get("a"), Some(Node::Int(1, _))));
        assert!(matches!(map.get("b"), Some(Node::Float(f, _)) if (*f - 1.1).abs() < 1e-9));
        assert_eq!(map.get("c"), Some(&Node::Bool(true)));
        assert_eq!(map.get("d"), Some(&Node::Null));
        assert_eq!(map.get("e"), Some(&Node::Null));
        assert_eq!(map.get("f"), Some(&Node::Null));
        assert!(matches!(map.get("g"), Some(Node::Float(f, _)) if f.is_infinite()));
        assert!(matches!(map.get("h"), Some(Node::Int(31, _))));
        assert!(matches!(map.get("i"), Some(Node::Int(15, _))));
        assert_eq!(map.get("j"), Some(&Node::Str("1".into())));
        assert!(matches!(map.get("k"), Some(Node::Float(..))));
        assert!(matches!(map.get("l"), Some(Node::Float(..))));
    }

    #[test]
    fn duplicate_and_non_string_keys() {
        assert!(parse_map("a: 1\na: 2\n").unwrap_err().contains("duplicate"));
        assert!(parse_map("1: x\n").is_err());
        assert!(parse_map("true: x\n").is_err());
        assert!(parse_map("? [a]\n: x\n").is_err());
        assert!(parse_map("\"1\": x\n").is_ok());
        assert!(parse_map("a:\n  b: 1\n  b: 2\n").is_err());
    }

    #[test]
    fn top_level_must_be_map() {
        assert!(parse_map("- a\n").is_err());
        assert!(parse_map("hello\n").is_err());
        assert!(parse_map("").unwrap().is_empty());
        assert!(parse_map("# only comment\n").unwrap().is_empty());
        assert!(parse_map("a: 1\n---\nb: 2\n").is_err());
    }

    #[test]
    fn quoting_of_ambiguous_strings() {
        for s in [
            "no",
            "yes",
            "on",
            "off",
            "y",
            "N",
            "1.10",
            "2026-10-05",
            "1_000",
            "0x1F",
            "0o17",
            "1:20",
            ".inf",
            ".nan",
            "null",
            "~",
            "",
            "123",
            "-1",
            "+1",
            "<<",
            "=",
            " a",
            "a ",
            "a: b",
            "a #b",
            "[x]",
            "{x}",
            "*x",
            "&x",
            "!x",
            "|",
            ">",
            "'x",
            "\"x",
            "%x",
            "@x",
            "`x",
            "-x",
            "? x",
            "a\nb",
            "\u{FEFF}",
            "2026-10-05T14:30:05.123456Z",
        ] {
            assert!(!plain_safe(s), "{s:?} should be quoted");
        }
        for s in [
            "alpha",
            "Task complete",
            "レビュー結果",
            "a-b_c.d",
            "Re foo",
        ] {
            assert!(plain_safe(s), "{s:?} should be plain");
        }
    }

    #[test]
    fn round_trip() {
        let src = "b: no\na: 1.10\nc:\n  - x\n  - k: v\n    k2: [1, 2]\n  - []\nd: {}\ne: \"multi\\nline \\\"q\\\"\"\nf: .nan\ng: 1e300\nh: -0.0\ni: true\nj: null\n";
        let map = m(src);
        let out = emit_map(&map);
        let back = m(&out);
        // NaN never equals itself; compare everything else.
        let mut a = map.clone();
        let mut b = back.clone();
        assert!(matches!(a.remove("f"), Some(Node::Float(f, _)) if f.is_nan()));
        assert!(matches!(b.remove("f"), Some(Node::Float(f, _)) if f.is_nan()));
        let strip = |n: &Map| -> String { emit_map(n) };
        assert_eq!(strip(&a), strip(&b), "{out}");
        assert!(out.contains("b: \"no\""), "{out}");
        assert!(out.contains("g: 1.0e+300"), "{out}");
        // Key order is preserved.
        let keys: Vec<_> = back.iter().map(|(k, _)| k.clone()).collect();
        assert_eq!(keys, ["b", "a", "c", "d", "e", "f", "g", "h", "i", "j"]);
    }

    #[test]
    fn big_integers_keep_integer_type() {
        let big = "123456789012345678901234567890123456789012";
        let map = m(&format!(
            "a: {big}\nb: -{big}\nc: +0{big}\nd: 0x{}\ne: !!float {big}\n",
            "f".repeat(40)
        ));
        assert_eq!(map.get("a"), Some(&Node::BigInt(big.into())));
        assert!(matches!(map.get("d"), Some(Node::BigInt(_))));
        assert!(matches!(map.get("e"), Some(Node::Float(..))));
        // Emitted unquoted and read back as the same integer.
        let out = emit_map(&map);
        assert!(out.contains(&format!("a: {big}\n")), "{out}");
        let back = m(&out);
        assert_eq!(back.get("a"), map.get("a"));
        assert_eq!(back.get("b"), map.get("b"));
        // Non-canonical forms are written in canonical decimal, like
        // `Node::Int`: `0o...` is a string and a leading `0` means octal in
        // YAML 1.1 (§5.4: other tools must read the same value).
        let leading_zero = format!("0{big}");
        let oct = "7".repeat(46);
        let src = format!("c: {leading_zero}\nf: 0o{oct}\ng: +{big}\nh: -0{big}\n");
        let out = emit_map(&m(&src));
        let f_dec = big_int_decimal(&format!("0o{oct}"));
        assert!(f_dec.bytes().all(|b| b.is_ascii_digit()) && !f_dec.starts_with('0'));
        assert_eq!(
            out,
            format!("c: {big}\nf: {f_dec}\ng: {big}\nh: -{big}\n"),
            "{out}"
        );
        // Re-parsing gives integers with the same decimal value.
        let back = m(&out);
        assert_eq!(back.get("f"), Some(&Node::BigInt(f_dec.clone())));
        assert_eq!(back.get("c"), Some(&Node::BigInt(big.into())));
        // JSON keeps every digit.
        let j = serde_json::to_string(&map_to_json(&map)).unwrap();
        assert!(j.contains(&format!("\"a\":{big}")), "{j}");
        assert!(j.contains(&format!("\"b\":-{big}")), "{j}");
        assert!(j.contains(&format!("\"c\":{big}")), "{j}");
        let hex = u128::MAX.to_string(); // 0xff..ff (32 digits) fits u128 but not i128
        let j =
            serde_json::to_string(&map_to_json(&m(&format!("h: 0x{}\n", "f".repeat(32))))).unwrap();
        assert_eq!(j, format!("{{\"h\":{hex}}}"));
        // i128-range integers beyond i64 are exact too.
        let j = serde_json::to_string(&map_to_json(&m("x: 99999999999999999999\n"))).unwrap();
        assert_eq!(j, "{\"x\":99999999999999999999}");
    }

    #[test]
    fn long_keys_use_explicit_form() {
        for len in [999, 1000, 1001, 1024, 1025, 5000] {
            let k = "a".repeat(len);
            let quoted = format!("{}:x", "b".repeat(len));
            let mut map = Map::new();
            map.insert(k.clone(), Node::Str("v".into()));
            map.insert(quoted.clone(), Node::Seq(vec![Node::Str("x".into())]));
            let mut inner = Map::new();
            inner.insert(k.clone(), Node::Int(1, "1".into()));
            map.insert("nested", Node::Seq(vec![Node::Map(inner)]));
            let out = emit_map(&map);
            let back = parse_map(&out).unwrap_or_else(|e| panic!("{len}: {e}\n{out}"));
            assert_eq!(back, map, "{len}");
            let explicit_plain = out.contains(&format!("? {k}\n"));
            assert_eq!(explicit_plain, len > MAX_IMPLICIT_KEY, "{len}");
        }
        // An explicit key in the input is accepted and kept.
        let src = format!("? {}\n: value\n", "a".repeat(1025));
        let map = m(&src);
        assert_eq!(m(&emit_map(&map)), map);
    }

    #[test]
    fn alias_expansion_respects_depth_limit() {
        // Syntactically shallow, 130 levels deep once expanded.
        let mut src = String::from("k0: &a0 [value]\n");
        for i in 1..130 {
            src.push_str(&format!("k{i}: &a{i} [*a{}]\n", i - 1));
        }
        assert!(parse_map(&src).unwrap_err().contains("too deep"));
        // Exactly at the limit is fine: map (1) + k126's 127 sequences.
        let mut src = String::from("k0: &a0 [value]\n");
        for i in 1..127 {
            src.push_str(&format!("k{i}: &a{i} [*a{}]\n", i - 1));
        }
        assert!(parse_map(&src).is_ok());
    }

    #[test]
    fn tags() {
        for ok in [
            "a: !!str 1\n",
            "a: !!int 1\n",
            "a: !!float 1\n",
            "a: !!bool true\n",
            "a: !!null null\n",
            "a: !!seq [x]\n",
            "a: !!map {x: 1}\n",
            "a: ! 1\n",
        ] {
            assert!(parse_map(ok).is_ok(), "{ok}");
        }
        assert_eq!(m("a: !!str 1\n").get("a"), Some(&Node::Str("1".into())));
        assert_eq!(m("a: ! 1\n").get("a"), Some(&Node::Str("1".into())));
        for bad in [
            "a: !!binary aGVsbG8=\n",
            "a: !!timestamp 2026-10-05\n",
            "a: !foo x\n",
            "a: !foo [x]\n",
            "a: !!int x\n",
        ] {
            assert!(parse_map(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn json_conversion() {
        let map = m("a: .inf\nb: -.Inf\nc: .NaN\nd: 1\ne: [x, null]\n");
        let j = map_to_json(&map);
        assert_eq!(j["a"], ".inf");
        assert_eq!(j["b"], "-.Inf");
        assert_eq!(j["c"], ".NaN");
        assert_eq!(j["d"], 1);
        assert_eq!(j["e"], serde_json::json!(["x", null]));
    }

    #[test]
    fn aliases_expand_with_limit() {
        let map = m("a: &x [1, 2]\nb: *x\n");
        assert_eq!(map.get("a"), map.get("b"));
        let mut bomb = String::from("a: &a [x, x, x, x, x, x, x, x, x, x]\n");
        let names = ["b", "c", "d", "e", "f", "g", "h"];
        let mut prev = "a";
        for n in names {
            bomb.push_str(&format!(
                "{n}: &{n} [*{prev}, *{prev}, *{prev}, *{prev}, *{prev}, *{prev}, *{prev}, *{prev}, *{prev}, *{prev}]\n"
            ));
            prev = n;
        }
        assert!(parse_map(&bomb).is_err());
    }
}
