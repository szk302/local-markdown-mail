//! Local Markdown Mail (lmm): Markdown messaging between AI agents and
//! people over the local file system. See `docs/prd.md`.

pub mod address;
pub mod cli;
pub mod commands;
pub mod config;
pub mod env;
pub mod error;
pub mod frontmatter;
pub mod fsutil;
pub mod lock;
pub mod mailbox;
pub mod message;
pub mod yaml;
