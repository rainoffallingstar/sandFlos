use crate::args::GlobalOptions;
use crate::error::{CliError, CliErrorCode};
use boox_core::cli_libraries::{
    calibre_directories, calibre_file, calibre_list, configure_calibre,
};
use std::path::PathBuf;

pub fn run(options: &GlobalOptions, args: &[String]) -> Result<(), CliError> {
    let Some(subcommand) = args.first().map(String::as_str) else {
        return Err(CliError::usage(
            "calibre requires a subcommand: configure, status, list, or push",
        ));
    };
    let rest = &args[1..];
    match subcommand {
        "configure" => configure(options, rest),
        "status" => status(options, rest),
        "list" => list(options, rest),
        "push" => push(options, rest),
        other => Err(CliError::usage(format!(
            "unknown calibre subcommand: {other}; expected configure, status, list, or push"
        ))),
    }
}

fn configure(options: &GlobalOptions, args: &[String]) -> Result<(), CliError> {
    let mut libraries = Vec::new();
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--library" => {
                let value = args
                    .get(index + 1)
                    .ok_or_else(|| CliError::usage("--library requires a path"))?;
                if value.starts_with('-') {
                    return Err(CliError::usage("--library requires a path"));
                }
                libraries.push(PathBuf::from(value));
                index += 2;
            }
            other => {
                return Err(CliError::usage(format!(
                    "unknown calibre configure option: {other}"
                )))
            }
        }
    }
    if libraries.is_empty() {
        return Err(CliError::usage(
            "usage: sandflos calibre configure --library PATH [--library PATH...]",
        ));
    }
    let config_dir = crate::auth::config_dir(options)?;
    configure_calibre(&config_dir, libraries).map_err(local_library_error)?;
    emit_status(options, &config_dir)
}

fn status(options: &GlobalOptions, args: &[String]) -> Result<(), CliError> {
    if !args.is_empty() {
        return Err(CliError::usage("usage: sandflos calibre status"));
    }
    let config_dir = crate::auth::config_dir(options)?;
    emit_status(options, &config_dir)
}

fn emit_status(options: &GlobalOptions, config_dir: &std::path::Path) -> Result<(), CliError> {
    let libraries = calibre_directories(config_dir).map_err(local_library_error)?;
    let data = serde_json::json!({
        "configured": !libraries.is_empty(),
        "libraries": libraries
            .iter()
            .map(|path| path.to_string_lossy().to_string())
            .collect::<Vec<_>>(),
    });
    if options.json {
        emit_json(data)
    } else if libraries.is_empty() {
        println!("configured=false");
        Ok(())
    } else {
        println!("configured=true");
        for library in libraries {
            println!("{}", library.display());
        }
        Ok(())
    }
}

fn list(options: &GlobalOptions, args: &[String]) -> Result<(), CliError> {
    let mut limit = None;
    let mut offset = None;
    let mut search = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--limit" => {
                let value = args
                    .get(index + 1)
                    .ok_or_else(|| CliError::usage("--limit requires a positive integer"))?;
                let parsed = value
                    .parse::<usize>()
                    .map_err(|_| CliError::usage("--limit requires a positive integer"))?;
                if parsed == 0 {
                    return Err(CliError::usage("--limit requires a positive integer"));
                }
                limit = Some(parsed);
                index += 2;
            }
            "--offset" => {
                let value = args
                    .get(index + 1)
                    .ok_or_else(|| CliError::usage("--offset requires an integer"))?;
                offset = Some(
                    value
                        .parse::<usize>()
                        .map_err(|_| CliError::usage("--offset requires an integer"))?,
                );
                index += 2;
            }
            "--search" => {
                let value = args
                    .get(index + 1)
                    .ok_or_else(|| CliError::usage("--search requires text"))?;
                search = Some(value.clone());
                index += 2;
            }
            other => {
                return Err(CliError::usage(format!(
                    "unknown calibre list option: {other}"
                )))
            }
        }
    }
    let config_dir = crate::auth::config_dir(options)?;
    let books = calibre_list(&config_dir, limit, offset, search).map_err(local_library_error)?;
    if options.json {
        emit_json(serde_json::json!({ "books": books }))?;
    } else if books.is_empty() {
        println!("no books");
    } else {
        for book in books {
            let formats = book
                .formats
                .iter()
                .map(|format| format!("{}:{}", format.format, format.data_id))
                .collect::<Vec<_>>()
                .join(",");
            println!(
                "{}\t{}\t{}\t{}",
                book.library_dir, book.book_id, book.title, formats
            );
        }
    }
    Ok(())
}

fn push(options: &GlobalOptions, args: &[String]) -> Result<(), CliError> {
    let mut library = None;
    let mut data_id = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--library" => {
                let value = args
                    .get(index + 1)
                    .ok_or_else(|| CliError::usage("--library requires a path"))?;
                if value.starts_with('-') {
                    return Err(CliError::usage("--library requires a path"));
                }
                library = Some(PathBuf::from(value));
                index += 2;
            }
            "--data-id" => {
                let value = args
                    .get(index + 1)
                    .ok_or_else(|| CliError::usage("--data-id requires an integer"))?;
                data_id = Some(
                    value
                        .parse::<i64>()
                        .map_err(|_| CliError::usage("--data-id requires an integer"))?,
                );
                index += 2;
            }
            other => {
                return Err(CliError::usage(format!(
                    "unknown calibre push option: {other}"
                )))
            }
        }
    }
    let library = library.ok_or_else(|| {
        CliError::usage("usage: sandflos calibre push --library PATH --data-id ID")
    })?;
    let data_id = data_id.ok_or_else(|| {
        CliError::usage("usage: sandflos calibre push --library PATH --data-id ID")
    })?;
    let file = calibre_file(&library, data_id).map_err(local_library_error)?;
    crate::push::push_paths(options, &[file])
}

fn local_library_error(error: boox_core::cli_libraries::LibraryError) -> CliError {
    use boox_core::services::ServiceErrorKind;
    let code = match error.kind {
        ServiceErrorKind::Input | ServiceErrorKind::NotFound => CliErrorCode::Usage,
        ServiceErrorKind::Authentication => CliErrorCode::Auth,
        ServiceErrorKind::Network | ServiceErrorKind::Protocol => CliErrorCode::Network,
        ServiceErrorKind::Cancelled => CliErrorCode::Cancelled,
        ServiceErrorKind::Partial | ServiceErrorKind::Unknown => CliErrorCode::Partial,
        ServiceErrorKind::LocalIo | ServiceErrorKind::Internal => CliErrorCode::LocalIo,
    };
    CliError::new(code, error.message).with_stable_code("library_error")
}

fn emit_json(value: serde_json::Value) -> Result<(), CliError> {
    println!(
        "{}",
        serde_json::to_string(&crate::output::success_envelope(value))
            .map_err(|_| CliError::new(CliErrorCode::Internal, "cannot serialize output"))?
    );
    Ok(())
}
