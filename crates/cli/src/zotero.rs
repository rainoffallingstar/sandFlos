use crate::args::GlobalOptions;
use crate::error::{CliError, CliErrorCode};
use boox_core::cli_libraries::{
    configure_webdav, configure_zotero, zotero_attachment, zotero_directory, zotero_list,
};
use std::{
    io::{self, Read},
    path::PathBuf,
    time::Duration,
};

pub fn run(options: &GlobalOptions, args: &[String]) -> Result<(), CliError> {
    let Some(subcommand) = args.first().map(String::as_str) else {
        return Err(CliError::usage(
            "zotero requires a subcommand: configure, detect, status, list, webdav, or push",
        ));
    };
    let rest = &args[1..];
    match subcommand {
        "configure" => configure(options, rest),
        "detect" => detect(options, rest),
        "status" => status(options, rest),
        "list" => list(options, rest),
        "webdav" => webdav(options, rest),
        "push" => push(options, rest),
        other => Err(CliError::usage(format!(
            "unknown zotero subcommand: {other}; expected configure, detect, status, list, webdav, or push"
        ))),
    }
}

fn configure(options: &GlobalOptions, args: &[String]) -> Result<(), CliError> {
    let data_dir = single_value(args, "--data-dir", "zotero configure --data-dir PATH")?;
    let config_dir = crate::auth::config_dir(options)?;
    configure_zotero(&config_dir, &PathBuf::from(data_dir)).map_err(local_library_error)?;
    emit_status(options)
}

fn detect(_options: &GlobalOptions, args: &[String]) -> Result<(), CliError> {
    if !args.is_empty() {
        return Err(CliError::usage("usage: booxctl zotero detect"));
    }
    Err(CliError::new(
        CliErrorCode::LocalIo,
        "automatic Zotero detection is not available in the CLI; use `booxctl zotero configure --data-dir PATH`",
    )
    .with_stable_code("zotero_detection_unsupported"))
}

fn status(options: &GlobalOptions, args: &[String]) -> Result<(), CliError> {
    if !args.is_empty() {
        return Err(CliError::usage("usage: booxctl zotero status"));
    }
    emit_status(options)
}

fn emit_status(options: &GlobalOptions) -> Result<(), CliError> {
    let config_dir = crate::auth::config_dir(options)?;
    let data_dir = zotero_directory(&config_dir).map_err(local_library_error)?;
    let data = serde_json::json!({
        "configured": data_dir.is_some(),
        "data_dir": data_dir
            .as_ref()
            .map(|path| path.to_string_lossy().to_string()),
    });
    if options.json {
        emit_json(data)
    } else if let Some(path) = data_dir {
        println!("configured=true");
        println!("data_dir={}", path.display());
        Ok(())
    } else {
        println!("configured=false");
        Ok(())
    }
}

fn list(options: &GlobalOptions, args: &[String]) -> Result<(), CliError> {
    let (limit, offset, search) = paging_options(args, "zotero list")?;
    let config_dir = crate::auth::config_dir(options)?;
    let items = zotero_list(&config_dir, limit, offset, search).map_err(local_library_error)?;
    if options.json {
        emit_json(serde_json::json!({ "items": items }))?;
    } else if items.is_empty() {
        println!("no Zotero items");
    } else {
        for item in items {
            for attachment in item.attachments {
                println!(
                    "{}\t{}\t{}\t{}",
                    item.item_id,
                    attachment.attachment_item_id,
                    item.title,
                    attachment
                        .file_name
                        .unwrap_or_else(|| attachment.attachment_key.clone())
                );
            }
        }
    }
    Ok(())
}

fn webdav(options: &GlobalOptions, args: &[String]) -> Result<(), CliError> {
    if args.first().map(String::as_str) != Some("configure") {
        return Err(CliError::usage(
            "usage: booxctl zotero webdav configure --url URL --username USER --password-stdin",
        ));
    }
    let args = &args[1..];
    let mut url = None;
    let mut username = None;
    let mut password_stdin = false;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--url" => {
                url = Some(
                    args.get(index + 1)
                        .filter(|value| !value.starts_with('-'))
                        .ok_or_else(|| CliError::usage("--url requires a value"))?
                        .clone(),
                );
                index += 2;
            }
            "--username" => {
                username = Some(
                    args.get(index + 1)
                        .filter(|value| !value.starts_with('-'))
                        .ok_or_else(|| CliError::usage("--username requires a value"))?
                        .clone(),
                );
                index += 2;
            }
            "--password-stdin" => {
                password_stdin = true;
                index += 1;
            }
            other => {
                return Err(CliError::usage(format!(
                    "unknown zotero webdav configure option: {other}"
                )))
            }
        }
    }
    let url = url.ok_or_else(|| CliError::usage("--url is required"))?;
    let username = username.ok_or_else(|| CliError::usage("--username is required"))?;
    if !password_stdin {
        return Err(CliError::usage(
            "--password-stdin is required; passwords cannot be passed in argv",
        ));
    }
    let mut password = String::new();
    io::stdin().read_to_string(&mut password).map_err(|error| {
        CliError::new(
            CliErrorCode::LocalIo,
            format!("cannot read password: {error}"),
        )
    })?;
    let password = password.trim_end_matches(['\r', '\n']);
    let config_dir = crate::auth::config_dir(options)?;
    configure_webdav(&config_dir, &url, &username, password).map_err(local_library_error)?;
    if options.json {
        emit_json(serde_json::json!({ "configured": true, "url": url, "username": username }))
    } else {
        println!("configured=true");
        Ok(())
    }
}

fn push(options: &GlobalOptions, args: &[String]) -> Result<(), CliError> {
    let value = single_value(args, "--attachment-id", "zotero push --attachment-id ID")?;
    let id = value
        .parse::<i64>()
        .map_err(|_| CliError::usage("--attachment-id requires an integer"))?;
    let config_dir = crate::auth::config_dir(options)?;
    let timeout = Duration::from_secs(options.timeout_secs.unwrap_or(90));
    let attachment = zotero_attachment(&config_dir, id, timeout).map_err(local_library_error)?;
    crate::push::push_paths(options, std::slice::from_ref(&attachment.path))
}

type PagingOptions = (Option<usize>, Option<usize>, Option<String>);

fn paging_options(args: &[String], usage: &str) -> Result<PagingOptions, CliError> {
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
            other => return Err(CliError::usage(format!("unknown {usage} option: {other}"))),
        }
    }
    Ok((limit, offset, search))
}

fn single_value<'a>(args: &'a [String], flag: &str, usage: &str) -> Result<&'a str, CliError> {
    match args {
        [key, value] if key == flag && !value.starts_with('-') => Ok(value.as_str()),
        _ => Err(CliError::usage(format!("usage: booxctl {usage}"))),
    }
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
