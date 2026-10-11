//! Address rules (§3.1).

/// Maximum address length.
pub const MAX_LEN: usize = 64;

/// Reserved address that can't be used for a mailbox.
pub const RESERVED: &str = "lmm";

/// Validate an address. Returns a human readable reason on failure.
pub fn validate(addr: &str) -> std::result::Result<(), String> {
    if addr.is_empty() || addr.len() > MAX_LEN {
        return Err(format!(
            "invalid address {addr:?}: length must be 1-{MAX_LEN} characters"
        ));
    }
    let bytes = addr.as_bytes();
    if !(bytes[0].is_ascii_lowercase() || bytes[0].is_ascii_digit()) {
        return Err(format!(
            "invalid address {addr:?}: must start with a-z or 0-9"
        ));
    }
    if !bytes
        .iter()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'_' | b'-'))
    {
        return Err(format!(
            "invalid address {addr:?}: only a-z 0-9 . _ - are allowed"
        ));
    }
    if addr == RESERVED {
        return Err(format!("invalid address {addr:?}: reserved by the system"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn is_valid(addr: &str) -> bool {
        validate(addr).is_ok()
    }

    #[test]
    fn valid_addresses() {
        for a in [
            "alpha",
            "a",
            "0",
            "reviewer-042",
            "a.b_c-d",
            &"a".repeat(64),
        ] {
            assert!(is_valid(a), "{a}");
        }
    }

    #[test]
    fn invalid_addresses() {
        for a in [
            "",
            ".",
            "..",
            ".hidden",
            "-x",
            "_x",
            "Alpha",
            "lmm",
            "../x",
            "a/b",
            "a b",
            "aé",
            &"a".repeat(65),
        ] {
            assert!(!is_valid(a), "{a}");
        }
    }
}
