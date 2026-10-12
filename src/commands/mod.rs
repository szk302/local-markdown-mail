//! Command implementations.

pub mod box_cmd;
pub mod draft;
pub mod hook;
pub mod init;
pub mod post;
pub mod receive;
pub mod show;
pub mod wait;

use std::io::Write;
use std::path::PathBuf;

use crate::cli::{BoxCommand, Cli, Command, ConfigCommand};
use crate::config::Config;
use crate::env;
use crate::error::{IoContext, LmmError, Result};
use crate::mailbox::DataDir;

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

fn config_show(dir: Option<&std::path::Path>, origin: bool) -> Result<()> {
    let cwd = env::cwd()?;
    let start = match dir {
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
    };
    // As if run in `start`.
    let ctx = Ctx::load(start)?;
    let mut out = std::io::stdout().lock();
    out.write_all(ctx.cfg.show(origin).as_bytes())
        .and_then(|_| out.flush())
        .ctx(|| "cannot write to stdout".to_string())
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
    }
}
