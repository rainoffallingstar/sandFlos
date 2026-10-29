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
use error::CliError;
use std::process::ExitCode;

fn run() -> Result<(), CliError> {
    let raw = std::env::args().skip(1).collect::<Vec<_>>();
    let json_requested = raw.iter().any(|arg| arg == "--json");
    let parsed = match args::parse(raw) {
        Ok(parsed) => parsed,
        Err(error) => {
            output::emit_error(json_requested, &error);
            return Err(error);
        }
    };
    match parsed {
        Parsed::Help => {
            print!("{}", help::HELP);
            Ok(())
        }
        Parsed::Version => {
            println!("sandflos {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Parsed::Command {
            options,
            command,
            args,
        } => {
            cancel::install();
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
                        "sandflos: error code={} stable_code={} exit={}",
                        error.code.as_str(),
                        error.stable_code.as_deref().unwrap_or("-"),
                        error.code.exit_code()
                    );
                }
            }
            result
        }
    }
}

fn main() -> ExitCode {
    let result = run();
    if cancel::requested() {
        return ExitCode::from(130);
    }
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => ExitCode::from(error.code.exit_code() as u8),
    }
}
