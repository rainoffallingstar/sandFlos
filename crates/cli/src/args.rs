use crate::error::CliError;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    Auth,
    Push,
    Queue,
    Devices,
    Storage,
    Reading,
    Calibre,
    Zotero,
    Diagnostics,
    Config,
    Help,
}

#[derive(Debug, Default)]
pub struct GlobalOptions {
    pub config_dir: Option<PathBuf>,
    pub json: bool,
    pub no_input: bool,
    pub timeout_secs: Option<u64>,
    pub verbose: bool,
}

#[derive(Debug)]
pub enum Parsed {
    Help,
    Version,
    Command {
        options: GlobalOptions,
        command: Command,
        args: Vec<String>,
    },
}

fn command_from_name(name: &str) -> Option<Command> {
    match name {
        "auth" => Some(Command::Auth),
        "push" => Some(Command::Push),
        "queue" => Some(Command::Queue),
        "devices" => Some(Command::Devices),
        "storage" => Some(Command::Storage),
        "reading" => Some(Command::Reading),
        "calibre" => Some(Command::Calibre),
        "zotero" => Some(Command::Zotero),
        "diagnostics" => Some(Command::Diagnostics),
        "config" => Some(Command::Config),
        "help" => Some(Command::Help),
        _ => None,
    }
}

fn value_after(args: &[String], index: usize, flag: &str) -> Result<String, CliError> {
    args.get(index + 1)
        .filter(|value| !value.starts_with('-'))
        .cloned()
        .ok_or_else(|| CliError::usage(format!("{flag} requires a value")))
}

pub fn parse(raw: Vec<String>) -> Result<Parsed, CliError> {
    let mut options = GlobalOptions::default();
    let mut index = 0;
    let mut command: Option<Command> = None;
    let mut command_args = Vec::new();
    let mut literal = false;

    while index < raw.len() {
        let arg = &raw[index];
        if literal {
            if command.is_none() {
                command = Some(
                    command_from_name(arg)
                        .ok_or_else(|| CliError::usage(format!("unknown command: {arg}")))?,
                );
            } else {
                command_args.push(arg.clone());
            }
            index += 1;
            continue;
        }
        match arg.as_str() {
            "--" => {
                // `push` owns the literal terminator so it can accept file
                // names beginning with a dash. Other commands treat it as the
                // global end-of-options marker.
                if command == Some(Command::Push) {
                    command_args.push(arg.clone());
                }
                literal = true;
            }
            "-h" | "--help" => return Ok(Parsed::Help),
            "-V" | "--version" => return Ok(Parsed::Version),
            "--json" => options.json = true,
            "--no-input" => options.no_input = true,
            "--verbose" => options.verbose = true,
            "--config-dir" => {
                let value = value_after(&raw, index, "--config-dir")?;
                options.config_dir = Some(PathBuf::from(value));
                index += 1;
            }
            "--timeout" => {
                let value = value_after(&raw, index, "--timeout")?;
                let seconds = value
                    .parse::<u64>()
                    .map_err(|_| CliError::usage("--timeout must be a positive integer"))?;
                if seconds == 0 {
                    return Err(CliError::usage("--timeout must be a positive integer"));
                }
                options.timeout_secs = Some(seconds);
                index += 1;
            }
            _ if command.is_none() && arg.starts_with('-') => {
                return Err(CliError::usage(format!("unknown option: {arg}")));
            }
            _ if command.is_none() => {
                command = Some(
                    command_from_name(arg)
                        .ok_or_else(|| CliError::usage(format!("unknown command: {arg}")))?,
                );
            }
            _ => command_args.push(arg.clone()),
        }
        index += 1;
    }

    let command = command.ok_or_else(|| CliError::usage("a command is required"))?;
    if command == Command::Help {
        return Ok(Parsed::Help);
    }
    Ok(Parsed::Command {
        options,
        command,
        args: command_args,
    })
}
