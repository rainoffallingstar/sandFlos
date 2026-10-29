use crate::args::GlobalOptions;
use crate::error::{CliError, CliErrorCode};
use boox_core::{
    models::UploadAuthContext,
    services::{
        HttpQueueMutationBackend, HttpQueueReadBackend, QueueMutationBackend, QueueMutationResult,
        QueueReadBackend, QueueService, ServiceError, ServiceErrorKind, ServiceResult,
    },
};
use std::time::Duration;

struct DeferredQueueReadBackend {
    options: GlobalOptions,
    timeout: Duration,
    inner: Option<HttpQueueReadBackend>,
}

impl DeferredQueueReadBackend {
    fn new(options: &GlobalOptions, timeout: Duration) -> Self {
        Self {
            options: GlobalOptions {
                config_dir: options.config_dir.clone(),
                json: options.json,
                no_input: options.no_input,
                timeout_secs: options.timeout_secs,
                verbose: options.verbose,
            },
            timeout,
            inner: None,
        }
    }

    fn backend(&mut self) -> ServiceResult<&mut HttpQueueReadBackend> {
        if self.inner.is_none() {
            let credential = crate::auth::credential(&self.options).map_err(|_| {
                ServiceError::new(
                    ServiceErrorKind::Authentication,
                    "missing_credentials",
                    "auth",
                    "尚未登录，请先导入 token 或完成扫码登录",
                )
            })?;
            self.inner = Some(HttpQueueReadBackend::new(self.timeout, credential)?);
        }
        Ok(self.inner.as_mut().expect("initialized queue backend"))
    }
}

impl QueueReadBackend for DeferredQueueReadBackend {
    fn auth_context(&mut self) -> ServiceResult<UploadAuthContext> {
        self.backend()?.auth_context()
    }

    fn queue_window(
        &mut self,
        auth: &UploadAuthContext,
    ) -> ServiceResult<Vec<boox_core::models::DashboardPushItem>> {
        self.backend()?.queue_window(auth)
    }
}

struct DeferredQueueMutationBackend {
    options: GlobalOptions,
    timeout: Duration,
    inner: Option<HttpQueueMutationBackend>,
}

impl DeferredQueueMutationBackend {
    fn new(options: &GlobalOptions, timeout: Duration) -> Self {
        Self {
            options: GlobalOptions {
                config_dir: options.config_dir.clone(),
                json: options.json,
                no_input: options.no_input,
                timeout_secs: options.timeout_secs,
                verbose: options.verbose,
            },
            timeout,
            inner: None,
        }
    }

    fn backend(&mut self) -> ServiceResult<&mut HttpQueueMutationBackend> {
        if self.inner.is_none() {
            let credential = crate::auth::credential(&self.options).map_err(|_| {
                ServiceError::new(
                    ServiceErrorKind::Authentication,
                    "missing_credentials",
                    "auth",
                    "尚未登录，请先导入 token 或完成扫码登录",
                )
            })?;
            self.inner = Some(HttpQueueMutationBackend::new(self.timeout, credential)?);
        }
        Ok(self.inner.as_mut().expect("initialized queue backend"))
    }
}

impl QueueMutationBackend for DeferredQueueMutationBackend {
    fn auth_context(&mut self) -> ServiceResult<UploadAuthContext> {
        self.backend()?.auth_context()
    }

    fn resend(&mut self, auth: &UploadAuthContext, id: &str) -> ServiceResult<()> {
        self.backend()?.resend(auth, id)
    }

    fn delete_push_doc(&mut self, auth: &UploadAuthContext, id: &str) -> ServiceResult<()> {
        self.backend()?.delete_push_doc(auth, id)
    }

    fn delete_message_record(&mut self, auth: &UploadAuthContext, id: &str) -> ServiceResult<()> {
        self.backend()?.delete_message_record(auth, id)
    }
}

pub fn run(options: &GlobalOptions, args: &[String]) -> Result<(), CliError> {
    let Some(subcommand) = args.first().map(String::as_str) else {
        return Err(CliError::usage(
            "queue requires a subcommand: list, resend, or delete",
        ));
    };
    let rest = &args[1..];
    match subcommand {
        "list" => list(options, rest),
        "resend" => resend(options, rest),
        "delete" => delete(options, rest),
        other => Err(CliError::usage(format!(
            "unknown queue subcommand: {other}; expected list, resend, or delete"
        ))),
    }
}

fn timeout(options: &GlobalOptions) -> Duration {
    Duration::from_secs(options.timeout_secs.unwrap_or(30))
}

fn list(options: &GlobalOptions, args: &[String]) -> Result<(), CliError> {
    let mut limit = None;
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
            other => {
                return Err(CliError::usage(format!(
                    "unknown queue list option: {other}"
                )))
            }
        }
    }

    let backend = DeferredQueueReadBackend::new(options, timeout(options));
    let mut service = QueueService::new(backend);
    let listing = service.list(limit).map_err(CliError::from_service)?;

    if options.json {
        let data = serde_json::to_value(&listing)
            .map_err(|_| CliError::new(CliErrorCode::Internal, "cannot serialize queue listing"))?;
        crate::output::emit_success(true, data);
    } else {
        if listing.items.is_empty() {
            println!("queue is empty");
        } else {
            for item in &listing.items {
                println!(
                    "{}\t{}\t{}",
                    item.id,
                    item.name,
                    item.size
                        .map(|size| size.to_string())
                        .unwrap_or_else(|| "-".to_string())
                );
            }
        }
        if listing.truncated {
            eprintln!(
                "showing {} of the upstream window ({} items)",
                listing.limit, listing.upstream_window
            );
        }
    }
    Ok(())
}

fn resend(options: &GlobalOptions, args: &[String]) -> Result<(), CliError> {
    let id = single_id(args, "queue resend ID")?;
    let backend = DeferredQueueMutationBackend::new(options, timeout(options));
    let mut service = QueueService::new(backend);
    service.resend(id).map_err(CliError::from_service)?;

    if options.json {
        crate::output::emit_success(true, serde_json::json!({ "id": id, "resent": true }));
    } else {
        println!("resent: {id}");
    }
    Ok(())
}

fn delete(options: &GlobalOptions, args: &[String]) -> Result<(), CliError> {
    let (id, confirmed) = match args {
        [id] => (id.as_str(), false),
        [id, flag] if flag == "--yes" => (id.as_str(), true),
        [flag, id] if flag == "--yes" => (id.as_str(), true),
        _ => return Err(CliError::usage("usage: sandflos queue delete ID --yes")),
    };
    let backend = DeferredQueueMutationBackend::new(options, timeout(options));
    let mut service = QueueService::new(backend);
    let outcome = service
        .delete(id, confirmed)
        .map_err(CliError::from_service)?;
    report_deleted(options, id, outcome)
}

fn report_deleted(
    options: &GlobalOptions,
    id: &str,
    outcome: QueueMutationResult,
) -> Result<(), CliError> {
    let data = serde_json::to_value(&outcome)
        .map_err(|_| CliError::new(CliErrorCode::Internal, "cannot serialize delete result"))?;
    if !outcome.complete {
        let detail = outcome
            .error
            .as_ref()
            .map(|error| error.message.as_str())
            .unwrap_or("remote state could not be confirmed");
        return Err(
            CliError::from_exit_code(6, format!("delete incomplete for {id}: {detail}"))
                .with_stable_code("queue_delete_incomplete")
                .with_data(data),
        );
    }
    if options.json {
        crate::output::emit_success(true, data);
    } else {
        println!("deleted: {id} (complete=true)");
    }
    Ok(())
}

fn single_id<'a>(args: &'a [String], usage: &str) -> Result<&'a str, CliError> {
    match args {
        [id] if !id.starts_with('-') => Ok(id.as_str()),
        _ => Err(CliError::usage(format!("usage: sandflos {usage}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use boox_core::services::{QueueMutationResult, ServiceError};

    fn outcome(complete: bool) -> QueueMutationResult {
        QueueMutationResult {
            id_checked: true,
            remote_delete_attempted: true,
            message_delete_attempted: true,
            complete,
            unknown: !complete,
            error: if complete {
                None
            } else {
                Some(ServiceError::protocol(
                    "queue.delete",
                    "record delete failed",
                ))
            },
        }
    }

    #[test]
    fn incomplete_delete_is_exit_6_and_keeps_structured_result() {
        let options = GlobalOptions::default();
        let error = report_deleted(&options, "doc-1", outcome(false)).unwrap_err();
        assert_eq!(error.code.exit_code(), 6);
        assert_eq!(
            error.stable_code.as_deref(),
            Some("queue_delete_incomplete")
        );
        assert_eq!(error.data.as_ref().unwrap()["complete"], false);
    }

    #[test]
    fn complete_delete_is_success() {
        let options = GlobalOptions::default();
        assert!(report_deleted(&options, "doc-1", outcome(true)).is_ok());
    }
}
