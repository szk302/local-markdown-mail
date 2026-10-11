//! Command implementations.

pub mod box_cmd;
pub mod draft;
pub mod hook;
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
use crate::mailbox::Home;

/// Common execution context.
pub struct Ctx {
    pub cwd: PathBuf,
    pub home: Home,
}

impl Ctx {
    pub fn from_env() -> Result<Ctx> {
        let cwd = env::cwd()?;
        let home = Home::new(env::lmm_home(&cwd)?);
        Ok(Ctx { cwd, home })
    }
}

fn config_show(ctx: &Ctx, dir: Option<&std::path::Path>, origin: bool) -> Result<()> {
    let start = match dir {
        Some(d) => {
            let d = env::absolute_from(&ctx.cwd, d);
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
        None => ctx.cwd.clone(),
    };
    let cfg = Config::load(&start, &ctx.home.root)?;
    let mut out = std::io::stdout().lock();
    out.write_all(cfg.show(origin).as_bytes())
        .and_then(|_| out.flush())
        .ctx(|| "cannot write to stdout".to_string())
}

/// Run a parsed command line.
pub fn run(cli: &Cli) -> Result<()> {
    // `lmm hook` must check LMM_NAME before anything that can fail, such as
    // getting the current directory (§8.3).
    if let Command::Hook(a) = &cli.command {
        return hook::run(a);
    }
    let ctx = Ctx::from_env()?;
    match &cli.command {
        Command::Post(a) => post::run(&ctx, a),
        Command::Draft(a) => draft::run(&ctx, a),
        Command::Receive(a) => receive::run(&ctx, a),
        Command::Wait(a) => wait::run(&ctx, a),
        Command::Show(a) => show::run(&ctx, a),
        Command::Hook(a) => hook::run(a),
        Command::Box(BoxCommand::Create { address, exist_ok }) => {
            box_cmd::create(&ctx.home, address, *exist_ok)
        }
        Command::Box(BoxCommand::List) => box_cmd::list(&ctx.home),
        Command::Box(BoxCommand::Remove { address, force }) => {
            box_cmd::remove(&ctx.home, address, *force)
        }
        Command::Config(ConfigCommand::Show { dir, origin }) => {
            config_show(&ctx, dir.as_deref(), *origin)
        }
    }
}
