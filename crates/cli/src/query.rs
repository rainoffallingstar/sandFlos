use crate::args::{Command, GlobalOptions};
use crate::error::{CliError, CliErrorCode};
use boox_core::device::{is_local_transfer_host, normalize_transfer_host_url};
use boox_core::services::{HttpQueryBackend, QueryService};
use std::time::Duration;

pub fn run(options: &GlobalOptions, command: Command, args: &[String]) -> Result<(), CliError> {
    match command {
        Command::Devices => devices(options, args),
        Command::Storage => storage(options, args),
        Command::Reading => reading(options, args),
        _ => unreachable!("query module only handles query commands"),
    }
}

fn devices(options: &GlobalOptions, args: &[String]) -> Result<(), CliError> {
    let Some(subcommand) = args.first().map(String::as_str) else {
        return Err(CliError::usage(
            "devices requires a subcommand: list or transfer-url",
        ));
    };
    match subcommand {
        "list" if args.len() == 1 => devices_list(options),
        "transfer-url" => transfer_url(options, &args[1..]),
        "list" => Err(CliError::usage("usage: sandflos devices list")),
        other => Err(CliError::usage(format!(
            "unknown devices subcommand: {other}; expected list or transfer-url"
        ))),
    }
}

fn devices_list(options: &GlobalOptions) -> Result<(), CliError> {
    let devices = query_service(options)?.devices()?;
    if options.json {
        emit_json(serde_json::json!({ "devices": devices }))?;
    } else if devices.is_empty() {
        println!("no devices");
    } else {
        for device in devices {
            println!(
                "{}\t{}\t{}\t{}",
                device.id.as_deref().unwrap_or("-"),
                device.model.as_deref().unwrap_or("-"),
                device.login_status.as_deref().unwrap_or("-"),
                device.transfer_host.as_deref().unwrap_or("-")
            );
        }
    }
    Ok(())
}

fn transfer_url(options: &GlobalOptions, args: &[String]) -> Result<(), CliError> {
    let id = match args {
        [id] if !id.starts_with('-') => id.as_str(),
        _ => return Err(CliError::usage("usage: sandflos devices transfer-url ID")),
    };
    let devices = query_service(options)?.devices()?;
    let device = devices
        .into_iter()
        .find(|device| device.id.as_deref() == Some(id))
        .ok_or_else(|| {
            CliError::usage(format!("device not found: {id}")).with_stable_code("device_not_found")
        })?;
    let host = device.transfer_host.ok_or_else(|| {
        CliError::new(
            CliErrorCode::Network,
            "device is not reachable on the local network",
        )
        .with_stable_code("transfer_url_unavailable")
    })?;
    let url = validate_transfer_url(&host)?;
    if options.json {
        emit_json(serde_json::json!({ "id": id, "transfer_url": url }))?;
    } else {
        println!("{url}");
    }
    Ok(())
}

fn storage(options: &GlobalOptions, args: &[String]) -> Result<(), CliError> {
    if args != ["status"] {
        return Err(CliError::usage("usage: sandflos storage status"));
    }
    let reading = query_service(options)?.storage()?;
    if options.json {
        emit_json(serde_json::to_value(&reading).map_err(serialization_error)?)?;
    } else {
        println!(
            "used={}\nlimit={}\npercent={}",
            optional_u64(reading.used),
            optional_u64(reading.limit),
            reading
                .percent
                .map(|value| format!("{value:.2}"))
                .unwrap_or_else(|| "-".to_string())
        );
    }
    Ok(())
}

fn reading(options: &GlobalOptions, args: &[String]) -> Result<(), CliError> {
    if args.first().map(String::as_str) != Some("stats") {
        return Err(CliError::usage(
            "usage: sandflos reading stats --date YYYY-MM-DD",
        ));
    }
    let mut date = None;
    let mut index = 1;
    while index < args.len() {
        match args[index].as_str() {
            "--date" => {
                let value = args
                    .get(index + 1)
                    .ok_or_else(|| CliError::usage("--date requires YYYY-MM-DD"))?;
                date = Some(value.as_str());
                index += 2;
            }
            other => return Err(CliError::usage(format!("unknown reading option: {other}"))),
        }
    }
    let date = date.ok_or_else(|| CliError::usage("reading stats requires --date YYYY-MM-DD"))?;
    boox_core::services::validate_reading_date(date)?;
    let stats = query_service(options)?.reading(date)?;
    if options.json {
        emit_json(serde_json::to_value(&stats).map_err(serialization_error)?)?;
    } else {
        println!("date={}", stats.date);
        println!(
            "timezone={}",
            stats.timezone.as_deref().unwrap_or("unspecified")
        );
        println!("reading_info={}", stats.reading_info);
        println!("week={}", stats.week);
        println!("day={}", stats.day);
    }
    Ok(())
}

fn query_service(options: &GlobalOptions) -> Result<QueryService<HttpQueryBackend>, CliError> {
    let credential = crate::auth::credential(options)?;
    let timeout = Duration::from_secs(options.timeout_secs.unwrap_or(30));
    let backend = match crate::auth::test_api_endpoints()? {
        Some((api, oss)) => HttpQueryBackend::with_test_endpoints(timeout, credential, &api, &oss),
        None => HttpQueryBackend::new(timeout, credential),
    }
    .map_err(CliError::from_service)?
    .with_cancel_flag(&crate::cancel::FLAG);
    Ok(QueryService::new(backend))
}

fn validate_transfer_url(host: &str) -> Result<String, CliError> {
    let normalized = normalize_transfer_host_url(host).ok_or_else(|| {
        CliError::new(CliErrorCode::Network, "device transfer address is invalid")
            .with_stable_code("invalid_transfer_url")
    })?;
    let parsed = url::Url::parse(&normalized).map_err(|_| {
        CliError::new(CliErrorCode::Network, "device transfer address is invalid")
            .with_stable_code("invalid_transfer_url")
    })?;
    let hostname = parsed.host_str().ok_or_else(|| {
        CliError::new(
            CliErrorCode::Network,
            "device transfer address lacks a host",
        )
        .with_stable_code("invalid_transfer_url")
    })?;
    if !is_local_transfer_host(hostname) {
        return Err(CliError::new(
            CliErrorCode::Network,
            "device transfer address is not on the local network",
        )
        .with_stable_code("unsafe_transfer_url"));
    }
    Ok(normalized)
}

fn emit_json(data: serde_json::Value) -> Result<(), CliError> {
    println!(
        "{}",
        serde_json::to_string(&crate::output::success_envelope(data))
            .map_err(serialization_error)?
    );
    Ok(())
}

fn serialization_error(_: serde_json::Error) -> CliError {
    CliError::new(CliErrorCode::Internal, "cannot serialize output")
}

fn optional_u64(value: Option<u64>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "-".to_string())
}
