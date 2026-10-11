//! Error type and exit codes (§12.5).

use std::fmt;
use std::io;

/// Errors returned by commands. Each variant maps to an exit code.
#[derive(Debug)]
pub enum LmmError {
    /// Usage or configuration error (exit code 2).
    Usage(String),
    /// Runtime error (exit code 1).
    Runtime(String),
    /// `lmm wait --timeout` expired (exit code 3).
    Timeout,
    /// Interrupted by a signal (exit code 128 + signal).
    Signal(i32),
    /// Exit with the given code without printing anything more
    /// (details were already reported on stderr, or the code comes from a child).
    Exit(i32),
}

pub type Result<T> = std::result::Result<T, LmmError>;

impl LmmError {
    pub fn usage(msg: impl Into<String>) -> Self {
        LmmError::Usage(msg.into())
    }

    pub fn runtime(msg: impl Into<String>) -> Self {
        LmmError::Runtime(msg.into())
    }

    pub fn exit_code(&self) -> i32 {
        match self {
            LmmError::Usage(_) => 2,
            LmmError::Runtime(_) => 1,
            LmmError::Timeout => 3,
            LmmError::Signal(sig) => 128 + sig,
            LmmError::Exit(code) => *code,
        }
    }

    /// Message to print on stderr, if any.
    pub fn message(&self) -> Option<String> {
        match self {
            LmmError::Usage(m) | LmmError::Runtime(m) => Some(m.clone()),
            LmmError::Timeout => Some("timed out".to_string()),
            LmmError::Signal(_) | LmmError::Exit(_) => None,
        }
    }
}

impl fmt::Display for LmmError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.message() {
            Some(m) => f.write_str(&m),
            None => write!(f, "exit {}", self.exit_code()),
        }
    }
}

impl std::error::Error for LmmError {}

/// Helper to convert I/O errors into runtime errors with context.
pub trait IoContext<T> {
    fn ctx(self, what: impl FnOnce() -> String) -> Result<T>;
}

impl<T> IoContext<T> for io::Result<T> {
    fn ctx(self, what: impl FnOnce() -> String) -> Result<T> {
        self.map_err(|e| LmmError::Runtime(format!("{}: {}", what(), e)))
    }
}

/// Print a warning to stderr (§12.6).
pub fn warn(msg: impl AsRef<str>) {
    eprintln!("lmm: warning: {}", msg.as_ref());
}

/// Print an error log line to stderr (§12.6).
pub fn error_log(msg: impl AsRef<str>) {
    eprintln!("lmm: error: {}", msg.as_ref());
}
