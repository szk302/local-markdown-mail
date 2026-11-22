//! Command implementations.

pub mod box_cmd;
pub mod draft;
pub mod hook;
pub mod init;
pub mod post;
pub mod receive;
pub mod show;
pub mod wait;

use std::ffi::OsString;
use std::io::Write;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use crate::cli::{BoxCommand, Cli, Command, ConfigCommand};
use crate::config::Config;
use crate::env;
use crate::error::{IoContext, LmmError, Result};
use crate::mailbox::DataDir;
use crate::yaml;

/// Common execution context.
pub struct Ctx {
    pub cwd: PathBuf,
    pub data_dir: DataDir,
    pub cfg: Config,
}

impl Ctx {
    /// The configuration (§10) and the data directory (§4.1) for `cwd`.
    pub fn load(cwd: PathBuf) -> Result<Ctx> {
        let cfg = Config::load_dirs(&cwd)?;
        Ctx::with_dirs(cwd, cfg)
    }

    /// Finish [`Ctx::load`] from what [`Config::load_dirs`] returned.
    pub fn with_dirs(cwd: PathBuf, mut cfg: Config) -> Result<Ctx> {
        let data_dir = DataDir::new(cfg.data_dir(&cwd)?);
        cfg.load_global(&data_dir.root, &cwd)?;
        Ok(Ctx { cwd, data_dir, cfg })
    }
}

/// The start directory of `lmm config` (`-C`, §10.7).
fn start_dir(dir: Option<&Path>) -> Result<PathBuf> {
    let cwd = env::cwd()?;
    Ok(match dir {
        Some(d) => {
            let d = env::absolute_from(&cwd, d);
            if !std::fs::metadata(&d).is_ok_and(|m| m.is_dir()) {
                return Err(LmmError::usage(format!(
                    "{} is not a directory",
                    d.display()
                )));
            }
            // Resolve `..` and symlinks so that walking up with
            // `Path::parent` visits the real ancestors (§10.2).
            d.canonicalize()
                .ctx(|| format!("cannot resolve {}", d.display()))?
        }
        None => cwd,
    })
}

fn config_show(dir: Option<&Path>, origin: bool) -> Result<()> {
    // As if run in the start directory.
    let ctx = Ctx::load(start_dir(dir)?)?;
    let mut out = std::io::stdout().lock();
    out.write_all(ctx.cfg.show(origin).as_bytes())
        .and_then(|_| out.flush())
        .ctx(|| "cannot write to stdout".to_string())
}

const PATH_KEYS: [&str; 4] = ["name", "data_dir", "receive_dir", "draft_dir"];

/// `lmm config paths` (§10.9): your address and the directories, resolved
/// the way the commands resolve them, so that other tools need not copy the
/// rules.
fn config_paths(dir: Option<&Path>, field: Option<&str>) -> Result<()> {
    if let Some(f) = field
        && !PATH_KEYS.contains(&f)
    {
        return Err(LmmError::usage(format!(
            "unknown field {f:?} (use one of {})",
            PATH_KEYS.join(", ")
        )));
    }
    let ctx = Ctx::load(start_dir(dir)?)?;
    let name = ctx.cfg.name()?;
    // Like `lmm receive` and `lmm draft` without DIR. A directory that
    // needs your address is unknown when you have none.
    let resolve = |v: &crate::config::Val<String>, key: &str| -> Result<Option<PathBuf>> {
        match v.resolve_path(key, name.as_deref(), &ctx.cwd) {
            Ok(p) => Ok(Some(p)),
            Err(_) if name.is_none() => Ok(None),
            Err(e) => Err(e),
        }
    };
    // Resolved one at a time, so that --field does not fail on a key it
    // does not show (§10.9).
    let value = |key: &str| -> Result<Option<OsString>> {
        Ok(match key {
            "name" => name.clone().map(OsString::from),
            "data_dir" => Some(ctx.data_dir.root.clone().into_os_string()),
            "receive_dir" => {
                resolve(&ctx.cfg.receive_dir, "receive.dir")?.map(PathBuf::into_os_string)
            }
            _ => resolve(&ctx.cfg.draft_dir, "draft.dir")?.map(PathBuf::into_os_string),
        })
    };
    let mut out = std::io::stdout().lock();
    match field {
        Some(f) => {
            let Some(v) = value(f)? else {
                return Err(LmmError::Exit(1));
            };
            out.write_all(v.as_bytes())
                .and_then(|_| out.write_all(b"\n"))
                .ctx(|| "cannot write to stdout".to_string())?;
        }
        None => {
            let mut text = String::new();
            for k in PATH_KEYS {
                if let Some(v) = value(k)? {
                    let s = v.to_string_lossy().into_owned();
                    text.push_str(&format!(
                        "{k}: {}\n",
                        yaml::emit_inline(&yaml::Node::Str(s.clone())).unwrap_or(s)
                    ));
                }
            }
            out.write_all(text.as_bytes())
                .ctx(|| "cannot write to stdout".to_string())?;
        }
    }
    out.flush().ctx(|| "cannot write to stdout".to_string())
}

/// Run a parsed command line.
pub fn run(cli: &Cli) -> Result<()> {
    let ctx = || Ctx::load(env::cwd()?);
    match &cli.command {
        Command::Init(a) => init::run(a),
        Command::Post(a) => post::run(&ctx()?, a),
        Command::Draft(a) => draft::run(&ctx()?, a),
        Command::Receive(a) => receive::run(&ctx()?, a),
        Command::Wait(a) => wait::run(&ctx()?, a),
        Command::Show(a) => show::run(&ctx()?, a),
        // `lmm hook` decides whether it participates before anything else
        // can fail (§8.3).
        Command::Hook(a) => hook::run(a),
        Command::Box(BoxCommand::Create { address, exist_ok }) => {
            box_cmd::create(&ctx()?.data_dir, address, *exist_ok)
        }
        Command::Box(BoxCommand::List) => box_cmd::list(&ctx()?.data_dir),
        Command::Box(BoxCommand::Remove { address, force }) => {
            box_cmd::remove(&ctx()?.data_dir, address, *force)
        }
        Command::Config(ConfigCommand::Show { dir, origin }) => {
            config_show(dir.as_deref(), *origin)
        }
        Command::Config(ConfigCommand::Paths { dir, field }) => {
            config_paths(dir.as_deref(), field.as_deref())
        }
    }
}
