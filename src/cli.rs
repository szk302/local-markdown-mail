//! Command line interface definitions.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "lmm",
    version,
    about = "Local Markdown Mail: exchange Markdown messages through the local file system"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Send a message directory
    Post(PostArgs),
    /// Create a draft message directory for `lmm post`
    Draft(DraftArgs),
    /// Extract new messages into the receive directory
    Receive(ReceiveArgs),
    /// Wait for matching messages and extract them
    Wait(WaitArgs),
    /// Show a message
    Show(ShowArgs),
    /// Receive and notify from an AI CLI hook
    Hook(HookArgs),
    /// Manage mailboxes
    #[command(name = "box", subcommand)]
    Box(BoxCommand),
    /// Inspect configuration
    #[command(subcommand)]
    Config(ConfigCommand),
}

#[derive(Debug, Args)]
pub struct PostArgs {
    /// Destination address
    #[arg(short = 't', long = "to", value_name = "ADDRESS")]
    pub to: Option<String>,
    /// Subject
    #[arg(short = 's', long = "subject", value_name = "TEXT")]
    pub subject: Option<String>,
    /// Message directory (or its index.md) being replied to
    #[arg(
        short = 'r',
        long = "reply",
        value_name = "PATH",
        conflicts_with = "forward"
    )]
    pub reply: Option<PathBuf>,
    /// Thread ID
    #[arg(long = "thread", value_name = "ID")]
    pub thread: Option<String>,
    /// Record forwarding history (input is a received message)
    #[arg(long = "forward")]
    pub forward: bool,
    /// Custom front matter key (repeatable)
    #[arg(short = 'm', long = "meta", value_name = "KEY=VALUE")]
    pub meta: Vec<String>,
    /// Message directory to send
    #[arg(value_name = "DIR")]
    pub dir: PathBuf,
}

#[derive(Debug, Args)]
pub struct DraftArgs {
    /// Subject
    #[arg(short = 's', long = "subject", value_name = "TEXT")]
    pub subject: Option<String>,
    /// Thread ID
    #[arg(long = "thread", value_name = "ID")]
    pub thread: Option<String>,
    /// Custom front matter key (repeatable)
    #[arg(short = 'm', long = "meta", value_name = "KEY=VALUE")]
    pub meta: Vec<String>,
    /// Directory to create (default: a new directory in the draft location)
    #[arg(value_name = "DIR")]
    pub dir: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub struct ReceiveArgs {
    /// Maximum number of messages to extract
    #[arg(long = "max", value_name = "N", value_parser = clap::value_parser!(u64).range(1..))]
    pub max: Option<u64>,
    /// Receive directory
    #[arg(value_name = "DIR")]
    pub dir: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub struct WaitArgs {
    /// Only messages from this address
    #[arg(long = "from", value_name = "ADDRESS")]
    pub from: Option<String>,
    /// Only messages in this thread
    #[arg(long = "thread", value_name = "ID")]
    pub thread: Option<String>,
    /// Only replies to this message ID
    #[arg(long = "in-reply-to", value_name = "ID")]
    pub in_reply_to: Option<String>,
    /// Only messages whose custom key equals the value (repeatable)
    #[arg(long = "meta", value_name = "KEY=VALUE")]
    pub meta: Vec<String>,
    /// Give up after SEC seconds (exit code 3)
    #[arg(long = "timeout", value_name = "SEC")]
    pub timeout: Option<f64>,
    /// Maximum number of messages to extract
    #[arg(long = "max", value_name = "N", value_parser = clap::value_parser!(u64).range(1..))]
    pub max: Option<u64>,
    /// Receive directory
    #[arg(value_name = "DIR")]
    pub dir: Option<PathBuf>,
}

#[derive(Debug, Args)]
#[command(group(clap::ArgGroup::new("target").required(true).args(["path", "id"])))]
pub struct ShowArgs {
    /// Message directory or its index.md
    #[arg(value_name = "PATH")]
    pub path: Option<PathBuf>,
    /// Find by lmm_id in your own mailbox
    #[arg(long = "id", value_name = "ID")]
    pub id: Option<String>,
    /// Output JSON
    #[arg(long = "json", conflicts_with = "field")]
    pub json: bool,
    /// Output a single front matter value
    #[arg(long = "field", value_name = "KEY")]
    pub field: Option<String>,
}

#[derive(Debug, Args)]
pub struct HookArgs {
    /// Receive directory
    #[arg(value_name = "DIR")]
    pub dir: Option<PathBuf>,
}

#[derive(Debug, Subcommand)]
pub enum BoxCommand {
    /// Create a mailbox
    Create {
        #[arg(value_name = "ADDRESS")]
        address: String,
        /// Succeed if the mailbox already exists
        #[arg(long = "exist-ok")]
        exist_ok: bool,
    },
    /// List mailboxes
    List,
    /// Remove a mailbox with its history
    Remove {
        #[arg(value_name = "ADDRESS")]
        address: String,
        /// Remove even if there are unreceived messages or sends in progress
        #[arg(long = "force")]
        force: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum ConfigCommand {
    /// Show the effective configuration
    Show {
        /// Start directory (default: current directory)
        #[arg(short = 'C', value_name = "DIR")]
        dir: Option<PathBuf>,
        /// Annotate each value with where it was defined
        #[arg(long = "origin")]
        origin: bool,
    },
}
