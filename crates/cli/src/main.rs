mod args;
mod auth;
mod calibre;
mod cancel;
mod config;
mod diagnostics;
mod error;
mod help;
mod output;
mod push;
mod query;
mod queue;
mod zotero;

use args::{Command, Parsed};
use error::{CliError, CliErrorCode};
use std::process::ExitCode;

/// What the run observed: the fully parsed command (when argument parsing
/// succeeded) and the command result. The command is kept as parsed metadata so
/// exit-code precedence never depends on rescanning raw argv.
struct RunOutcome {
    command: Option<Command>,
    result: Result<(), CliError>,
}

fn run() -> RunOutcome {
    let raw = std::env::args().skip(1).collect::<Vec<_>>();
    let json_requested = raw.iter().any(|arg| arg == "--json");
    let parsed = match args::parse(raw) {
        Ok(parsed) => parsed,
        Err(error) => {
            output::emit_error(json_requested, &error);
            return RunOutcome {
                command: None,
                result: Err(error),
            };
        }
    };
    match parsed {
        Parsed::Help => {
            print!("{}", help::HELP);
            RunOutcome {
                command: None,
                result: Ok(()),
            }
        }
        Parsed::Version => {
            println!("booxctl {}", env!("CARGO_PKG_VERSION"));
            RunOutcome {
                command: None,
                result: Ok(()),
            }
        }
        Parsed::Command {
            options,
            command,
            args,
        } => {
            let result = match command {
                Command::Auth => auth::run(&options, &args),
                Command::Push => push::run(&options, &args),
                Command::Queue => queue::run(&options, &args),
                Command::Devices | Command::Storage | Command::Reading => {
                    query::run(&options, command, &args)
                }
                Command::Calibre => calibre::run(&options, &args),
                Command::Zotero => zotero::run(&options, &args),
                Command::Diagnostics => diagnostics::run(&options, &args),
                Command::Config => config::run(&options, &args),
                Command::Help => unreachable!("help is parsed separately"),
            };
            if let Err(error) = &result {
                output::emit_error(options.json, error);
                if options.verbose {
                    eprintln!(
                        "booxctl: error code={} stable_code={} exit={}",
                        error.code.as_str(),
                        error.stable_code.as_deref().unwrap_or("-"),
                        error.code.exit_code()
                    );
                }
            }
            RunOutcome {
                command: Some(command),
                result,
            }
        }
    }
}

fn main() -> ExitCode {
    cancel::install();
    let outcome = run();
    if cancel::requested() {
        let queue_outcome = match (&outcome.result, outcome.command) {
            // Queue mutations define their own post-write outcome: a write may
            // have landed without confirmation, which the documented contract
            // reports as exit 6 (`unknown`/`partial`). A confirmed queue
            // success is likewise retained. Every other command keeps the
            // pre-existing SIGINT contract of exit 130, including push.
            (Ok(()), Some(Command::Queue)) => true,
            (Err(error), Some(Command::Queue)) if error.code == CliErrorCode::Partial => true,
            _ => false,
        };
        if !queue_outcome {
            return ExitCode::from(130);
        }
    }
    match outcome.result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => ExitCode::from(error.code.exit_code() as u8),
    }
}
