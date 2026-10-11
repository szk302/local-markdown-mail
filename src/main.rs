use std::ffi::OsStr;
use std::process::ExitCode;

use clap::Parser;
use clap::error::ErrorKind;

use lmm::cli::Cli;
use lmm::error::LmmError;

fn exit(code: i32) -> ExitCode {
    ExitCode::from(u8::try_from(code).unwrap_or(1))
}

fn main() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(c) => c,
        Err(e) => {
            let _ = e.print();
            return match e.kind() {
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion => exit(0),
                // `lmm hook` never returns 2 by itself (§8.3). `args_os`
                // because `args` panics on non-UTF-8 arguments.
                _ if std::env::args_os().nth(1).as_deref() == Some(OsStr::new("hook")) => exit(1),
                _ => exit(2),
            };
        }
    };
    let is_hook = matches!(cli.command, lmm::cli::Command::Hook(_));
    match lmm::commands::run(&cli) {
        Ok(()) => exit(0),
        Err(e) => {
            if let Some(m) = e.message() {
                eprintln!("lmm: error: {m}");
            }
            let code = e.exit_code();
            if is_hook && code == 2 && !matches!(e, LmmError::Exit(_)) {
                return exit(1);
            }
            exit(code)
        }
    }
}
