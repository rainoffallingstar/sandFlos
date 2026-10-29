use crate::args::GlobalOptions;
use crate::error::{CliError, CliErrorCode};
use boox_core::services::{
    validate_local_push_files, HttpPushBackend, PushProgress, PushReport, PushService,
};
use std::{path::PathBuf, time::Duration};

pub fn run(options: &GlobalOptions, args: &[String]) -> Result<(), CliError> {
    let mut files = Vec::new();
    let mut literal = false;
    for arg in args {
        if !literal && arg == "--" {
            literal = true;
            continue;
        }
        if !literal && arg.starts_with('-') {
            return Err(CliError::usage(format!("unknown push option: {arg}")));
        }
        files.push(PathBuf::from(arg));
    }
    if files.is_empty() {
        return Err(CliError::usage("push requires at least one file"));
    }
    push_paths(options, &files)
}

/// Run the shared upload/push pipeline for files selected by another command.
pub fn push_paths(options: &GlobalOptions, files: &[PathBuf]) -> Result<(), CliError> {
    if let Some(report) = validate_local_push_files(files) {
        return finish_report(options, &report);
    }

    let credential = crate::auth::credential(options)?;
    let timeout = Duration::from_secs(options.timeout_secs.unwrap_or(120));
    let backend = HttpPushBackend::new(timeout, credential)
        .map_err(CliError::from_service)?
        .with_cancel_flag(&crate::cancel::FLAG);
    let mut service = PushService::new(backend);
    let report = service.push_files(files, |event| {
        if !options.json {
            emit_progress(&event);
        }
    });
    finish_report(options, &report)
}

fn finish_report(options: &GlobalOptions, report: &PushReport) -> Result<(), CliError> {
    if options.json {
        let data = serde_json::to_value(report)
            .map_err(|_| CliError::new(CliErrorCode::Internal, "cannot serialize push report"))?;
        if report.has_problems() {
            return Err(CliError::from_exit_code(
                report.exit_code(),
                format!(
                    "{} accepted, {} failed, {} unknown",
                    report.accepted, report.failed, report.unknown
                ),
            )
            .with_stable_code("push_incomplete")
            .with_data(data));
        }
        println!(
            "{}",
            serde_json::to_string(&crate::output::success_envelope(data)).map_err(|_| {
                CliError::new(CliErrorCode::Internal, "cannot serialize push report")
            })?
        );
    } else {
        for file in &report.files {
            match file.outcome {
                boox_core::services::PushOutcome::Accepted => println!(
                    "accepted\t{}\t{}",
                    file.path,
                    file.message_id.as_deref().unwrap_or("-")
                ),
                boox_core::services::PushOutcome::Failed => println!(
                    "failed\t{}\t{}",
                    file.path,
                    file.error
                        .as_ref()
                        .map(|error| error.message.as_str())
                        .unwrap_or("unknown failure")
                ),
                boox_core::services::PushOutcome::Unknown => println!(
                    "unknown\t{}\t{}",
                    file.path,
                    file.error
                        .as_ref()
                        .map(|error| error.message.as_str())
                        .unwrap_or("outcome unknown")
                ),
            }
        }
    }

    if report.has_problems() {
        return Err(CliError::from_exit_code(
            report.exit_code(),
            format!(
                "{} accepted, {} failed, {} unknown",
                report.accepted, report.failed, report.unknown
            ),
        )
        .with_stable_code("push_incomplete"));
    }
    Ok(())
}

fn emit_progress(event: &PushProgress) {
    match event {
        PushProgress::FileStarted {
            index,
            total,
            path,
            size,
        } => eprintln!("[{}/{}] {path} ({size} bytes)", index + 1, total),
        PushProgress::Phase {
            index,
            total,
            path,
            stage,
        } => eprintln!("[{}/{}] {path}: {stage:?}", index + 1, total),
        PushProgress::Uploaded {
            index,
            total,
            path,
            bytes,
            ..
        } => eprintln!("[{}/{}] {path}: uploaded {bytes} bytes", index + 1, total),
        PushProgress::Finished(file) => eprintln!("[done] {}\t{:?}", file.path, file.outcome),
    }
}
