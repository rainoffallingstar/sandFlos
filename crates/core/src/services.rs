//! Typed, side-effect free services used by the CLI and desktop adapters.
//!
//! The services own orchestration and result semantics but not rendering. An
//! HTTP implementation lives at the edge; tests can supply the same traits with
//! deterministic fakes and observe only the public results.

use crate::api::{
    batch_delete_push_message_with_endpoints, create_client, delete_push_doc_with_endpoints,
    fetch_auth_context_for_token_with_endpoints, fetch_day_read_with_endpoints,
    fetch_default_bucket_with_endpoints, fetch_devices_with_endpoints,
    fetch_identity_for_token_with_endpoints, fetch_push_queue_for_dashboard_with_endpoints,
    fetch_read_time_info_with_endpoints, fetch_reading_info_with_endpoints,
    fetch_storage_with_endpoints, fetch_sts_for_auth_with_endpoints,
    put_push_message_doc_with_endpoints, save_and_push_with_endpoints,
};
use crate::device::build_dashboard_devices;
use crate::models::{BucketConfig, DashboardDevice, DashboardPushItem, OssSts, UploadAuthContext};
use crate::push::{
    build_oss_host, build_verified_signed_download_url_with_endpoints, content_type_for, sign_oss,
    wait_for_remote_push_doc_ready_with_endpoints,
};
use reqwest::blocking::Client;
use reqwest::header::{AUTHORIZATION, CONTENT_LENGTH, CONTENT_TYPE};
use serde::Serialize;
use serde_json::Value;
use std::{
    fmt, fs,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};
use uuid::Uuid;

pub const MAX_SINGLE_UPLOAD_BYTES: u64 = 200 * 1024 * 1024;
pub const QUEUE_WINDOW_LIMIT: usize = 30;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceErrorKind {
    Input,
    Authentication,
    LocalIo,
    Network,
    Protocol,
    Cancelled,
    Partial,
    Unknown,
    NotFound,
    Internal,
}

impl ServiceErrorKind {
    pub fn exit_code(self) -> i32 {
        match self {
            Self::Input | Self::NotFound => 2,
            Self::Authentication => 3,
            Self::LocalIo => 5,
            Self::Network | Self::Protocol => 4,
            Self::Cancelled => 130,
            Self::Partial | Self::Unknown => 6,
            Self::Internal => 1,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ServiceError {
    pub kind: ServiceErrorKind,
    pub code: &'static str,
    pub stage: &'static str,
    pub message: String,
}

impl ServiceError {
    pub fn new(
        kind: ServiceErrorKind,
        code: &'static str,
        stage: &'static str,
        message: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            code,
            stage,
            message: message.into(),
        }
    }

    pub fn input(code: &'static str, stage: &'static str, message: impl Into<String>) -> Self {
        Self::new(ServiceErrorKind::Input, code, stage, message)
    }

    pub fn local_io(code: &'static str, stage: &'static str, message: impl Into<String>) -> Self {
        Self::new(ServiceErrorKind::LocalIo, code, stage, message)
    }

    pub fn network(stage: &'static str, message: impl Into<String>) -> Self {
        Self::new(ServiceErrorKind::Network, "network_error", stage, message)
    }

    pub fn protocol(stage: &'static str, message: impl Into<String>) -> Self {
        Self::new(ServiceErrorKind::Protocol, "protocol_error", stage, message)
    }
}

impl fmt::Display for ServiceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for ServiceError {}

pub type ServiceResult<T> = Result<T, ServiceError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PushStage {
    Validate,
    Prepare,
    UploadSignedUrl,
    UploadOss,
    WriteMessage,
    ConfirmSync,
    SaveAndPush,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PushOutcome {
    Accepted,
    Failed,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PushFileResult {
    pub path: String,
    pub display_name: String,
    pub stage: PushStage,
    pub outcome: PushOutcome,
    pub message_id: Option<String>,
    pub rev: Option<String>,
    pub resource_key: Option<String>,
    pub size: Option<u64>,
    pub error: Option<ServiceError>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PushReport {
    pub files: Vec<PushFileResult>,
    pub accepted: usize,
    pub failed: usize,
    pub unknown: usize,
}

impl PushReport {
    pub fn from_files(files: Vec<PushFileResult>) -> Self {
        let accepted = files
            .iter()
            .filter(|item| item.outcome == PushOutcome::Accepted)
            .count();
        let failed = files
            .iter()
            .filter(|item| item.outcome == PushOutcome::Failed)
            .count();
        let unknown = files
            .iter()
            .filter(|item| item.outcome == PushOutcome::Unknown)
            .count();
        Self {
            files,
            accepted,
            failed,
            unknown,
        }
    }

    pub fn exit_code(&self) -> i32 {
        if self.files.iter().any(|item| {
            item.error
                .as_ref()
                .is_some_and(|error| error.kind == ServiceErrorKind::Cancelled)
        }) {
            return 130;
        }
        if self.files.is_empty() {
            return 2;
        }
        if self.unknown > 0 {
            return 6;
        }
        if self.failed == 0 {
            return 0;
        }
        if self.accepted > 0 {
            return 6;
        }
        let mut codes = self
            .files
            .iter()
            .filter_map(|item| item.error.as_ref().map(|e| e.kind.exit_code()));
        let first = codes.next().unwrap_or(1);
        // Authentication, network, then local I/O dominate input failures.
        let priority = |code: i32| match code {
            3 => 0,
            4 => 1,
            5 => 2,
            2 => 3,
            130 => 4,
            _ => 5,
        };
        let mut selected = first;
        for code in codes {
            if priority(code) < priority(selected) {
                selected = code;
            }
        }
        selected
    }

    pub fn has_problems(&self) -> bool {
        self.failed > 0 || self.unknown > 0
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum PushProgress {
    FileStarted {
        index: usize,
        total: usize,
        path: String,
        size: u64,
    },
    Phase {
        index: usize,
        total: usize,
        path: String,
        stage: PushStage,
    },
    Uploaded {
        index: usize,
        total: usize,
        path: String,
        bytes: u64,
        total_bytes: u64,
    },
    Finished(PushFileResult),
}

#[allow(clippy::too_many_arguments)]
pub trait PushBackend {
    fn auth_context(&mut self) -> ServiceResult<UploadAuthContext>;
    fn default_bucket(&mut self) -> ServiceResult<(String, BucketConfig)>;
    fn sts(&mut self, auth: &UploadAuthContext) -> ServiceResult<OssSts>;
    fn upload(
        &mut self,
        bucket: &BucketConfig,
        sts: &OssSts,
        object_key: &str,
        path: &Path,
        content_type: &str,
    ) -> ServiceResult<()>;
    fn signed_url(
        &mut self,
        auth: &UploadAuthContext,
        bucket: &BucketConfig,
        object_key: &str,
    ) -> ServiceResult<String>;
    fn write_message(
        &mut self,
        auth: &UploadAuthContext,
        path: &Path,
        file_name: &str,
        size: u64,
        resource_type: &str,
        object_key: &str,
        signed_url: &str,
    ) -> ServiceResult<(String, String)>;
    fn confirm_sync(
        &mut self,
        auth: &UploadAuthContext,
        doc_id: &str,
        rev: &str,
    ) -> ServiceResult<()>;
    fn save_and_push(
        &mut self,
        auth: &UploadAuthContext,
        bucket_key: &str,
        object_key: &str,
        file_name: &str,
        resource_type: &str,
        doc_id: &str,
        rev: &str,
    ) -> ServiceResult<()>;
}

fn fail_file<F>(
    on_progress: &mut F,
    result: &mut PushFileResult,
    stage: PushStage,
    error: ServiceError,
    outcome: PushOutcome,
) where
    F: FnMut(PushProgress),
{
    result.stage = stage;
    result.outcome = outcome;
    result.error = Some(error);
    on_progress(PushProgress::Finished(result.clone()));
}

fn prepare_push_files<F>(
    files: &[PathBuf],
    on_progress: &mut F,
) -> (Vec<Option<PushFileResult>>, Vec<(usize, u64)>)
where
    F: FnMut(PushProgress),
{
    let mut results = Vec::with_capacity(files.len());
    let mut valid = Vec::new();

    // Validate the whole batch locally before authenticating. A missing,
    // unreadable, empty, or oversized file is an input/IO failure even when
    // the caller has no usable credentials.
    for (index, path) in files.iter().enumerate() {
        let display_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("(invalid-name)")
            .to_string();
        let mut result = PushFileResult {
            path: path.to_string_lossy().to_string(),
            display_name: display_name.clone(),
            stage: PushStage::Prepare,
            outcome: PushOutcome::Failed,
            message_id: None,
            rev: None,
            resource_key: None,
            size: None,
            error: None,
        };

        let metadata = match fs::metadata(path) {
            Ok(meta) => meta,
            Err(error) => {
                fail_file(
                    on_progress,
                    &mut result,
                    PushStage::Prepare,
                    ServiceError::local_io(
                        "file_unreadable",
                        "prepare",
                        format!("无法读取文件 {display_name}: {error}"),
                    ),
                    PushOutcome::Failed,
                );
                results.push(Some(result));
                continue;
            }
        };
        if !metadata.is_file() {
            fail_file(
                on_progress,
                &mut result,
                PushStage::Prepare,
                ServiceError::input(
                    "not_a_file",
                    "prepare",
                    format!("{display_name} 不是普通文件"),
                ),
                PushOutcome::Failed,
            );
            results.push(Some(result));
            continue;
        }
        if let Err(error) = fs::File::open(path) {
            fail_file(
                on_progress,
                &mut result,
                PushStage::Prepare,
                ServiceError::local_io(
                    "file_unreadable",
                    "prepare",
                    format!("无法读取文件 {display_name}: {error}"),
                ),
                PushOutcome::Failed,
            );
            results.push(Some(result));
            continue;
        }
        let size = metadata.len();
        result.size = Some(size);
        if size == 0 {
            fail_file(
                on_progress,
                &mut result,
                PushStage::Validate,
                ServiceError::input("empty_file", "validate", format!("{display_name} 是空文件")),
                PushOutcome::Failed,
            );
            results.push(Some(result));
            continue;
        }
        if size > MAX_SINGLE_UPLOAD_BYTES {
            fail_file(
                on_progress,
                &mut result,
                PushStage::Validate,
                ServiceError::input(
                    "file_too_large",
                    "validate",
                    format!("{display_name} 超过单文件 200 MiB 限制"),
                ),
                PushOutcome::Failed,
            );
            results.push(Some(result));
            continue;
        }

        valid.push((index, size));
        results.push(Some(result));
    }

    (results, valid)
}

fn fail_valid<F>(
    results: &mut [Option<PushFileResult>],
    indices: &[usize],
    on_progress: &mut F,
    stage: PushStage,
    error: ServiceError,
) where
    F: FnMut(PushProgress),
{
    for &index in indices {
        let result = results[index]
            .as_mut()
            .expect("validated push result must be initialized");
        fail_file(
            on_progress,
            result,
            stage,
            error.clone(),
            PushOutcome::Failed,
        );
    }
}

fn finish_results(results: Vec<Option<PushFileResult>>) -> PushReport {
    PushReport::from_files(
        results
            .into_iter()
            .map(|result| result.expect("every push input must produce a result"))
            .collect(),
    )
}

/// Validate local push inputs without touching credentials or the network.
///
/// Returns a report only when no input can be pushed. Callers can use this to
/// preserve argument and local-I/O error precedence before resolving a token.
pub fn validate_local_push_files(files: &[PathBuf]) -> Option<PushReport> {
    if files.is_empty() {
        return None;
    }
    let (results, valid) = prepare_push_files(files, &mut |_| {});
    if valid.is_empty() {
        Some(finish_results(results))
    } else {
        None
    }
}

pub struct PushService<B> {
    backend: B,
}

pub fn upload_object_key(uid: &str, path: &Path) -> (String, String) {
    let ext = path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("bin")
        .to_ascii_lowercase();
    (format!("{uid}/push/{}.{}", Uuid::new_v4(), ext), ext)
}

impl<B: PushBackend> PushService<B> {
    pub fn new(backend: B) -> Self {
        Self { backend }
    }

    pub fn backend(&self) -> &B {
        &self.backend
    }

    pub fn push_files<F>(&mut self, files: &[PathBuf], mut on_progress: F) -> PushReport
    where
        F: FnMut(PushProgress),
    {
        if files.is_empty() {
            return PushReport::from_files(vec![PushFileResult {
                path: String::new(),
                display_name: String::new(),
                stage: PushStage::Validate,
                outcome: PushOutcome::Failed,
                message_id: None,
                rev: None,
                resource_key: None,
                size: None,
                error: Some(ServiceError::input(
                    "no_files",
                    "validate",
                    "未选择任何文件",
                )),
            }]);
        }

        let total = files.len();
        let (mut results, valid) = prepare_push_files(files, &mut on_progress);

        if valid.is_empty() {
            return finish_results(results);
        }

        let auth = match self.backend.auth_context() {
            Ok(value) => value,
            Err(error) => {
                fail_valid(
                    &mut results,
                    &valid.iter().map(|(index, _)| *index).collect::<Vec<_>>(),
                    &mut on_progress,
                    PushStage::Validate,
                    error,
                );
                return finish_results(results);
            }
        };
        let (bucket_key, bucket) = match self.backend.default_bucket() {
            Ok(value) => value,
            Err(error) => {
                fail_valid(
                    &mut results,
                    &valid.iter().map(|(index, _)| *index).collect::<Vec<_>>(),
                    &mut on_progress,
                    PushStage::Validate,
                    error,
                );
                return finish_results(results);
            }
        };

        let planned_bytes = valid
            .iter()
            .fold(0_u64, |total, (_, size)| total.saturating_add(*size));
        if let (Some(limit), Some(used)) = (auth.storage_limit, auth.storage_used) {
            let remaining = limit.saturating_sub(used);
            if planned_bytes > remaining {
                fail_valid(
                    &mut results,
                    &valid.iter().map(|(index, _)| *index).collect::<Vec<_>>(),
                    &mut on_progress,
                    PushStage::Validate,
                    ServiceError::input(
                        "quota_exceeded",
                        "validate",
                        format!("可用空间不足，剩余 {remaining} 字节，待上传 {planned_bytes} 字节"),
                    ),
                );
                return finish_results(results);
            }
        }

        for (index, size) in valid {
            let path = files[index].as_path();
            let display_name = results[index]
                .as_ref()
                .map(|result| result.display_name.clone())
                .unwrap_or_default();

            let mut result = results[index]
                .take()
                .expect("validated push result must be initialized");
            on_progress(PushProgress::FileStarted {
                index,
                total,
                path: result.path.clone(),
                size,
            });

            let sts = match self.backend.sts(&auth) {
                Ok(value) => value,
                Err(error) => {
                    fail_file(
                        &mut on_progress,
                        &mut result,
                        PushStage::UploadSignedUrl,
                        error,
                        PushOutcome::Failed,
                    );
                    results[index] = Some(result);
                    continue;
                }
            };
            let (object_key, resource_type) = upload_object_key(&auth.uid, path);
            result.resource_key = Some(object_key.clone());
            on_progress(PushProgress::Phase {
                index,
                total,
                path: result.path.clone(),
                stage: PushStage::UploadOss,
            });
            let content_type = content_type_for(path);
            if let Err(error) = self
                .backend
                .upload(&bucket, &sts, &object_key, path, &content_type)
            {
                fail_file(
                    &mut on_progress,
                    &mut result,
                    PushStage::UploadOss,
                    error,
                    PushOutcome::Failed,
                );
                results[index] = Some(result);
                continue;
            }
            on_progress(PushProgress::Uploaded {
                index,
                total,
                path: result.path.clone(),
                bytes: size,
                total_bytes: size,
            });

            let signed_url = match self.backend.signed_url(&auth, &bucket, &object_key) {
                Ok(value) => value,
                Err(error) => {
                    fail_file(
                        &mut on_progress,
                        &mut result,
                        PushStage::UploadSignedUrl,
                        error,
                        PushOutcome::Failed,
                    );
                    results[index] = Some(result);
                    continue;
                }
            };
            on_progress(PushProgress::Phase {
                index,
                total,
                path: result.path.clone(),
                stage: PushStage::WriteMessage,
            });
            let (doc_id, rev) = match self.backend.write_message(
                &auth,
                path,
                &display_name,
                size,
                &resource_type,
                &object_key,
                &signed_url,
            ) {
                Ok(value) => value,
                Err(error) => {
                    // A lost write response can hide a committed message.
                    // Keep the uploaded resource key for queue reconciliation.
                    let outcome = if matches!(
                        error.kind,
                        ServiceErrorKind::Network
                            | ServiceErrorKind::Protocol
                            | ServiceErrorKind::Unknown
                            | ServiceErrorKind::Partial
                    ) {
                        PushOutcome::Unknown
                    } else {
                        PushOutcome::Failed
                    };
                    fail_file(
                        &mut on_progress,
                        &mut result,
                        PushStage::WriteMessage,
                        error,
                        outcome,
                    );
                    results[index] = Some(result);
                    continue;
                }
            };
            result.message_id = Some(doc_id.clone());
            result.rev = Some(rev.clone());
            on_progress(PushProgress::Phase {
                index,
                total,
                path: result.path.clone(),
                stage: PushStage::ConfirmSync,
            });
            if let Err(error) = self.backend.confirm_sync(&auth, &doc_id, &rev) {
                fail_file(
                    &mut on_progress,
                    &mut result,
                    PushStage::ConfirmSync,
                    error,
                    PushOutcome::Unknown,
                );
                results[index] = Some(result);
                continue;
            }
            on_progress(PushProgress::Phase {
                index,
                total,
                path: result.path.clone(),
                stage: PushStage::SaveAndPush,
            });
            match self.backend.save_and_push(
                &auth,
                &bucket_key,
                &object_key,
                &display_name,
                &resource_type,
                &doc_id,
                &rev,
            ) {
                Ok(()) => {
                    result.stage = PushStage::SaveAndPush;
                    result.outcome = PushOutcome::Accepted;
                    result.error = None;
                }
                Err(error) => {
                    result.stage = PushStage::SaveAndPush;
                    result.outcome = PushOutcome::Unknown;
                    result.error = Some(error);
                }
            }
            on_progress(PushProgress::Finished(result.clone()));
            results[index] = Some(result);
        }

        finish_results(results)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct QueueListing {
    pub items: Vec<DashboardPushItem>,
    pub limit: usize,
    pub truncated: bool,
    pub upstream_window: usize,
}

pub trait QueueReadBackend {
    fn auth_context(&mut self) -> ServiceResult<UploadAuthContext>;
    fn queue_window(&mut self, auth: &UploadAuthContext) -> ServiceResult<Vec<DashboardPushItem>>;
}

pub trait QueueMutationBackend {
    fn auth_context(&mut self) -> ServiceResult<UploadAuthContext>;
    fn resend(&mut self, auth: &UploadAuthContext, id: &str) -> ServiceResult<()>;
    fn delete_push_doc(&mut self, auth: &UploadAuthContext, id: &str) -> ServiceResult<()>;
    fn delete_message_record(&mut self, auth: &UploadAuthContext, id: &str) -> ServiceResult<()>;
}

pub struct QueueService<B> {
    backend: B,
}

impl<B> QueueService<B> {
    pub fn new(backend: B) -> Self {
        Self { backend }
    }

    pub fn backend(&self) -> &B {
        &self.backend
    }
}

impl<B: QueueReadBackend> QueueService<B> {
    pub fn list(&mut self, limit: Option<usize>) -> ServiceResult<QueueListing> {
        if matches!(limit, Some(0)) {
            return Err(ServiceError::input(
                "invalid_limit",
                "queue.list",
                "limit 必须大于 0",
            ));
        }
        let requested = limit.unwrap_or(QUEUE_WINDOW_LIMIT);
        let auth = self.backend.auth_context()?;
        let mut items = self.backend.queue_window(&auth)?;
        items.sort_by_key(|item| std::cmp::Reverse(item.updated_at));
        let upstream = items.len().min(QUEUE_WINDOW_LIMIT);
        let effective = requested.min(upstream);
        items.truncate(effective);
        Ok(QueueListing {
            truncated: requested > effective || upstream == QUEUE_WINDOW_LIMIT,
            limit: effective,
            upstream_window: UPSTREAM_WINDOW,
            items,
        })
    }
}

/// The upstream neocloud `_changes` request asks for 200 docs, but the public
/// queue contract intentionally exposes no more than the existing 30-item view.
const UPSTREAM_WINDOW: usize = QUEUE_WINDOW_LIMIT;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct QueueMutationResult {
    pub id_checked: bool,
    pub remote_delete_attempted: bool,
    pub message_delete_attempted: bool,
    pub complete: bool,
    pub unknown: bool,
    pub error: Option<ServiceError>,
}

impl<B: QueueMutationBackend> QueueService<B> {
    pub fn resend(&mut self, id: &str) -> ServiceResult<()> {
        validate_queue_id(id)?;
        let auth = self.backend.auth_context()?;
        self.backend.resend(&auth, id)
    }

    pub fn delete(&mut self, id: &str, confirmed: bool) -> ServiceResult<QueueMutationResult> {
        validate_queue_id(id)?;
        if !confirmed {
            return Err(ServiceError::input(
                "confirmation_required",
                "queue.delete",
                "删除需要显式 --yes 确认",
            ));
        }
        let auth = self.backend.auth_context()?;
        match self.backend.delete_push_doc(&auth, id) {
            Ok(()) => match self.backend.delete_message_record(&auth, id) {
                Ok(()) => Ok(QueueMutationResult {
                    id_checked: true,
                    remote_delete_attempted: true,
                    message_delete_attempted: true,
                    complete: true,
                    unknown: false,
                    error: None,
                }),
                Err(error) if error.kind == ServiceErrorKind::NotFound => Ok(QueueMutationResult {
                    id_checked: true,
                    remote_delete_attempted: true,
                    message_delete_attempted: true,
                    complete: true,
                    unknown: false,
                    error: None,
                }),
                Err(error) => Ok(QueueMutationResult {
                    id_checked: true,
                    remote_delete_attempted: true,
                    message_delete_attempted: true,
                    complete: false,
                    unknown: true,
                    error: Some(error),
                }),
            },
            Err(error) if error.kind == ServiceErrorKind::NotFound => {
                match self.backend.delete_message_record(&auth, id) {
                    Ok(()) => Ok(QueueMutationResult {
                        id_checked: true,
                        remote_delete_attempted: false,
                        message_delete_attempted: true,
                        complete: true,
                        unknown: false,
                        error: None,
                    }),
                    Err(error) if error.kind == ServiceErrorKind::NotFound => {
                        Ok(QueueMutationResult {
                            id_checked: true,
                            remote_delete_attempted: false,
                            message_delete_attempted: true,
                            complete: true,
                            unknown: false,
                            error: None,
                        })
                    }
                    Err(error) => Err(error),
                }
            }
            Err(error) => Err(error),
        }
    }
}

fn validate_queue_id(id: &str) -> ServiceResult<()> {
    if id.trim().is_empty() {
        return Err(ServiceError::input(
            "invalid_id",
            "queue",
            "推送记录 id 不能为空",
        ));
    }
    if id.chars().any(char::is_whitespace) || id.chars().any(char::is_control) {
        return Err(ServiceError::input(
            "invalid_id",
            "queue",
            "推送记录 id 不能包含空白或控制字符",
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StorageReading {
    pub used: Option<u64>,
    pub limit: Option<u64>,
    pub percent: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ReadingStats {
    pub date: String,
    pub reading_info: Value,
    pub week: Value,
    pub day: Value,
}

pub trait QueryBackend {
    fn auth_context(&mut self) -> ServiceResult<UploadAuthContext>;
    fn devices(&mut self, auth: &UploadAuthContext) -> ServiceResult<Vec<DashboardDevice>>;
    fn storage(&mut self, auth: &UploadAuthContext) -> ServiceResult<StorageReading>;
    fn reading(&mut self, auth: &UploadAuthContext, date: &str) -> ServiceResult<ReadingStats>;
}

pub struct QueryService<B> {
    backend: B,
}

impl<B: QueryBackend> QueryService<B> {
    pub fn new(backend: B) -> Self {
        Self { backend }
    }
    pub fn backend(&self) -> &B {
        &self.backend
    }
    pub fn devices(&mut self) -> ServiceResult<Vec<DashboardDevice>> {
        let auth = self.backend.auth_context()?;
        self.backend.devices(&auth)
    }
    pub fn storage(&mut self) -> ServiceResult<StorageReading> {
        let auth = self.backend.auth_context()?;
        self.backend.storage(&auth)
    }
    pub fn reading(&mut self, date: &str) -> ServiceResult<ReadingStats> {
        validate_reading_date(date)?;
        let auth = self.backend.auth_context()?;
        self.backend.reading(&auth, date)
    }
}

/// Validate a reading date before resolving credentials or creating a client.
pub fn validate_reading_date(date: &str) -> ServiceResult<()> {
    let bytes = date.as_bytes();
    let shaped = bytes.len() == 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes
            .iter()
            .enumerate()
            .all(|(i, b)| i == 4 || i == 7 || b.is_ascii_digit());
    if !shaped || !chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").is_ok() {
        return Err(ServiceError::input(
            "invalid_date",
            "reading",
            "日期必须使用 YYYY-MM-DD 格式",
        ));
    }
    Ok(())
}

// ---- HTTP edge adapter -------------------------------------------------

pub struct HttpPushBackend {
    endpoints: crate::api::ApiEndpoints,
    client: Client,
    timeout: Duration,
    credential: String,
    cancel_flag: Option<&'static std::sync::atomic::AtomicBool>,
}

impl HttpPushBackend {
    pub fn with_test_endpoints(
        timeout: Duration,
        credential: impl Into<String>,
        api: &str,
        oss: &str,
    ) -> ServiceResult<Self> {
        let endpoints = crate::api::ApiEndpoints::loopback(api, oss).map_err(|_| {
            ServiceError::input(
                "invalid_test_endpoint",
                "client",
                "test endpoints must be literal loopback origins",
            )
        })?;
        let mut value = Self::new(timeout, credential)?;
        value.endpoints = endpoints;
        Ok(value)
    }

    pub fn new(timeout: Duration, credential: impl Into<String>) -> ServiceResult<Self> {
        let client = create_client(timeout.as_secs().max(1))
            .map_err(|_| ServiceError::network("client", "无法创建 HTTP 客户端"))?;
        Ok(Self {
            endpoints: crate::api::ApiEndpoints::production(),
            client,
            timeout,
            credential: credential.into(),
            cancel_flag: None,
        })
    }

    pub fn with_cancel_flag(mut self, flag: &'static std::sync::atomic::AtomicBool) -> Self {
        self.cancel_flag = Some(flag);
        self
    }
    fn check_cancelled(&self) -> ServiceResult<()> {
        if self
            .cancel_flag
            .is_some_and(|f| f.load(std::sync::atomic::Ordering::Relaxed))
        {
            Err(ServiceError::new(
                ServiceErrorKind::Cancelled,
                "cancelled",
                "request",
                "operation cancelled",
            ))
        } else {
            Ok(())
        }
    }
    pub fn client(&self) -> &Client {
        &self.client
    }

    pub fn timeout(&self) -> Duration {
        self.timeout
    }

    fn auth(&self) -> ServiceResult<UploadAuthContext> {
        fetch_auth_context_for_token_with_endpoints(&self.endpoints, &self.client, &self.credential)
            .map(|(auth, _)| auth)
            .map_err(|error| classify("auth", error))
    }
}

fn classify(stage: &'static str, error: String) -> ServiceError {
    if let Some(rest) = error.strip_prefix("HTTP ") {
        let status = rest
            .split_whitespace()
            .next()
            .and_then(|value| value.parse::<u16>().ok());
        return match status {
            Some(401) | Some(403) => ServiceError::new(
                ServiceErrorKind::Authentication,
                "credential_rejected",
                stage,
                "认证凭据无效或已过期",
            ),
            // A missing queue document is an idempotent delete case, not a
            // transport failure. Other stages keep 404 in the network bucket
            // because their missing resource may indicate a different fault.
            Some(404) if stage.starts_with("queue.") => ServiceError::new(
                ServiceErrorKind::NotFound,
                "not_found",
                stage,
                "请求的推送记录不存在",
            ),
            Some(code) => ServiceError::network(stage, format!("服务端返回 HTTP {code}")),
            None => ServiceError::protocol(stage, "服务端返回了无法识别的响应"),
        };
    }
    let lower = error.to_ascii_lowercase();
    if lower.contains("未登录") || lower.contains("token") && lower.contains("缺少") {
        return ServiceError::new(
            ServiceErrorKind::Authentication,
            "missing_credentials",
            stage,
            "authentication is required",
        );
    }
    if lower.contains("timeout") || lower.contains("timed out") {
        return ServiceError::network(stage, "网络请求超时");
    }
    ServiceError::protocol(
        stage,
        "remote request failed or returned an invalid response",
    )
}

impl PushBackend for HttpPushBackend {
    fn auth_context(&mut self) -> ServiceResult<UploadAuthContext> {
        self.check_cancelled()?;
        self.auth()
    }
    fn default_bucket(&mut self) -> ServiceResult<(String, BucketConfig)> {
        self.check_cancelled()?;
        fetch_default_bucket_with_endpoints(&self.endpoints, &self.client)
            .map_err(|error| classify("bucket", error))
    }
    fn sts(&mut self, auth: &UploadAuthContext) -> ServiceResult<OssSts> {
        self.check_cancelled()?;
        fetch_sts_for_auth_with_endpoints(&self.endpoints, &self.client, auth)
            .map_err(|error| classify("sts", error))
    }
    fn upload(
        &mut self,
        bucket: &BucketConfig,
        sts: &OssSts,
        object_key: &str,
        path: &Path,
        content_type: &str,
    ) -> ServiceResult<()> {
        self.check_cancelled()?;
        let (bucket_name, host) =
            build_oss_host(bucket).map_err(|error| classify("upload", error))?;
        let canonical_resource = format!("/{bucket_name}/{object_key}");
        let date = httpdate::fmt_http_date(SystemTime::now());
        let string_to_sign = format!(
            "PUT\n\n{content_type}\n{date}\nx-oss-security-token:{}\n{canonical_resource}",
            sts.security_token
        );
        let signature = sign_oss(&sts.access_key_secret, &string_to_sign)
            .map_err(|error| classify("upload", error))?;
        let authorization = format!("OSS {}:{}", sts.access_key_id, signature);
        let file_size = fs::metadata(path)
            .map_err(|error| {
                ServiceError::local_io("file_unreadable", "upload", error.to_string())
            })?
            .len();
        let file = fs::File::open(path).map_err(|error| {
            ServiceError::local_io("file_unreadable", "upload", error.to_string())
        })?;
        let body = reqwest::blocking::Body::sized(file, file_size);
        let url = self
            .endpoints
            .oss_url(&format!("https://{host}/{object_key}"));
        let response = self
            .client
            .put(&url)
            .header("Date", date)
            .header(CONTENT_TYPE, content_type)
            .header(CONTENT_LENGTH, file_size.to_string())
            .header("x-oss-security-token", sts.security_token.clone())
            .header(AUTHORIZATION, authorization)
            .body(body)
            .send()
            .map_err(|error| classify("upload", error.to_string()))?;
        if response.status().is_success() {
            Ok(())
        } else {
            Err(classify("upload", format!("HTTP {}", response.status())))
        }
    }
    fn signed_url(
        &mut self,
        auth: &UploadAuthContext,
        bucket: &BucketConfig,
        object_key: &str,
    ) -> ServiceResult<String> {
        self.check_cancelled()?;
        build_verified_signed_download_url_with_endpoints(
            &self.endpoints,
            &self.client,
            auth,
            bucket,
            object_key,
        )
        .map_err(|error| classify("signed_url", error))
    }
    fn write_message(
        &mut self,
        auth: &UploadAuthContext,
        _path: &Path,
        file_name: &str,
        size: u64,
        resource_type: &str,
        object_key: &str,
        signed_url: &str,
    ) -> ServiceResult<(String, String)> {
        self.check_cancelled()?;
        put_push_message_doc_with_endpoints(
            &self.endpoints,
            &self.client,
            auth,
            file_name,
            size,
            resource_type,
            object_key,
            signed_url,
        )
        .map_err(|error| classify("write_message", error))
    }
    fn confirm_sync(
        &mut self,
        auth: &UploadAuthContext,
        doc_id: &str,
        rev: &str,
    ) -> ServiceResult<()> {
        self.check_cancelled()?;
        wait_for_remote_push_doc_ready_with_endpoints(
            &self.endpoints,
            &self.client,
            auth,
            doc_id,
            rev,
        )
        .map_err(|error| classify("confirm_sync", error))
    }
    fn save_and_push(
        &mut self,
        auth: &UploadAuthContext,
        bucket_key: &str,
        object_key: &str,
        file_name: &str,
        resource_type: &str,
        doc_id: &str,
        rev: &str,
    ) -> ServiceResult<()> {
        self.check_cancelled()?;
        save_and_push_with_endpoints(
            &self.endpoints,
            &self.client,
            auth,
            bucket_key,
            object_key,
            file_name,
            resource_type,
            doc_id,
            rev,
        )
        .map(|_| ())
        .map_err(|error| classify("save_and_push", error))
    }
}

pub struct HttpQueueReadBackend {
    endpoints: crate::api::ApiEndpoints,
    client: Client,
    credential: String,
}
impl HttpQueueReadBackend {
    pub fn with_test_endpoints(
        timeout: Duration,
        credential: impl Into<String>,
        api: &str,
        oss: &str,
    ) -> ServiceResult<Self> {
        let endpoints = crate::api::ApiEndpoints::loopback(api, oss).map_err(|_| {
            ServiceError::input(
                "invalid_test_endpoint",
                "client",
                "test endpoints must be literal loopback origins",
            )
        })?;
        let mut value = Self::new(timeout, credential)?;
        value.endpoints = endpoints;
        Ok(value)
    }

    pub fn new(timeout: Duration, credential: impl Into<String>) -> ServiceResult<Self> {
        Ok(Self {
            endpoints: crate::api::ApiEndpoints::production(),
            client: create_client(timeout.as_secs().max(1))
                .map_err(|_| ServiceError::network("client", "无法创建 HTTP 客户端"))?,
            credential: credential.into(),
        })
    }
}
impl QueueReadBackend for HttpQueueReadBackend {
    fn auth_context(&mut self) -> ServiceResult<UploadAuthContext> {
        fetch_identity_for_token_with_endpoints(&self.endpoints, &self.client, &self.credential)
            .map(|(auth, _)| auth)
            .map_err(|e| classify("auth", e))
    }
    fn queue_window(&mut self, auth: &UploadAuthContext) -> ServiceResult<Vec<DashboardPushItem>> {
        fetch_push_queue_for_dashboard_with_endpoints(&self.endpoints, &self.client, auth)
            .map_err(|e| classify("queue.list", e))
    }
}

pub struct HttpQueueMutationBackend {
    endpoints: crate::api::ApiEndpoints,
    client: Client,
    credential: String,
    cancel_flag: Option<&'static std::sync::atomic::AtomicBool>,
}
impl HttpQueueMutationBackend {
    pub fn with_test_endpoints(
        timeout: Duration,
        credential: impl Into<String>,
        api: &str,
        oss: &str,
    ) -> ServiceResult<Self> {
        let endpoints = crate::api::ApiEndpoints::loopback(api, oss).map_err(|_| {
            ServiceError::input(
                "invalid_test_endpoint",
                "client",
                "test endpoints must be literal loopback origins",
            )
        })?;
        let mut value = Self::new(timeout, credential)?;
        value.endpoints = endpoints;
        Ok(value)
    }

    pub fn with_cancel_flag(mut self, flag: &'static std::sync::atomic::AtomicBool) -> Self {
        self.cancel_flag = Some(flag);
        self
    }
    fn check_cancelled(&self) -> ServiceResult<()> {
        if self
            .cancel_flag
            .is_some_and(|f| f.load(std::sync::atomic::Ordering::Relaxed))
        {
            Err(ServiceError::new(
                ServiceErrorKind::Cancelled,
                "cancelled",
                "queue",
                "operation cancelled",
            ))
        } else {
            Ok(())
        }
    }

    pub fn new(timeout: Duration, credential: impl Into<String>) -> ServiceResult<Self> {
        Ok(Self {
            endpoints: crate::api::ApiEndpoints::production(),
            client: create_client(timeout.as_secs().max(1))
                .map_err(|_| ServiceError::network("client", "无法创建 HTTP 客户端"))?,
            credential: credential.into(),
            cancel_flag: None,
        })
    }
}
impl QueueMutationBackend for HttpQueueMutationBackend {
    fn auth_context(&mut self) -> ServiceResult<UploadAuthContext> {
        self.check_cancelled()?;
        fetch_identity_for_token_with_endpoints(&self.endpoints, &self.client, &self.credential)
            .map(|(auth, _)| auth)
            .map_err(|e| classify("auth", e))
    }
    fn resend(&mut self, auth: &UploadAuthContext, id: &str) -> ServiceResult<()> {
        self.check_cancelled()?;
        crate::push::push_resend_with_auth_with_endpoints(&self.endpoints, &self.client, auth, id)
            .map_err(|error| {
                let submitted = matches!(&error, crate::push::ResendError::AfterSubmission(_));
                let mut classified = classify("queue.resend", error.to_string());
                if submitted
                    && matches!(
                        classified.kind,
                        ServiceErrorKind::Network | ServiceErrorKind::Protocol
                    )
                {
                    classified.kind = ServiceErrorKind::Unknown;
                    classified.code = "resend_unknown";
                    classified.message = "重推提交结果未知，请先查询队列再决定是否重试".into();
                }
                classified
            })
    }
    fn delete_push_doc(&mut self, auth: &UploadAuthContext, id: &str) -> ServiceResult<()> {
        self.check_cancelled()?;
        delete_push_doc_with_endpoints(&self.endpoints, &self.client, auth, id)
            .map_err(|e| classify("queue.delete", e))
    }
    fn delete_message_record(&mut self, auth: &UploadAuthContext, id: &str) -> ServiceResult<()> {
        self.check_cancelled()?;
        batch_delete_push_message_with_endpoints(&self.endpoints, &self.client, auth, id)
            .map_err(|e| classify("queue.delete", e))
    }
}

pub struct HttpQueryBackend {
    endpoints: crate::api::ApiEndpoints,
    client: Client,
    credential: String,
}
impl HttpQueryBackend {
    pub fn with_test_endpoints(
        timeout: Duration,
        credential: impl Into<String>,
        api: &str,
        oss: &str,
    ) -> ServiceResult<Self> {
        let endpoints = crate::api::ApiEndpoints::loopback(api, oss).map_err(|_| {
            ServiceError::input(
                "invalid_test_endpoint",
                "client",
                "test endpoints must be literal loopback origins",
            )
        })?;
        let mut value = Self::new(timeout, credential)?;
        value.endpoints = endpoints;
        Ok(value)
    }

    pub fn new(timeout: Duration, credential: impl Into<String>) -> ServiceResult<Self> {
        Ok(Self {
            endpoints: crate::api::ApiEndpoints::production(),
            client: create_client(timeout.as_secs().max(1))
                .map_err(|_| ServiceError::network("client", "无法创建 HTTP 客户端"))?,
            credential: credential.into(),
        })
    }
}
impl QueryBackend for HttpQueryBackend {
    fn auth_context(&mut self) -> ServiceResult<UploadAuthContext> {
        fetch_identity_for_token_with_endpoints(&self.endpoints, &self.client, &self.credential)
            .map(|(auth, _)| auth)
            .map_err(|e| classify("auth", e))
    }
    fn devices(&mut self, auth: &UploadAuthContext) -> ServiceResult<Vec<DashboardDevice>> {
        let raw = fetch_devices_with_endpoints(&self.endpoints, &self.client, &auth.bearer)
            .map_err(|e| classify("devices", e))?;
        Ok(build_dashboard_devices(raw, Vec::new()))
    }
    fn storage(&mut self, auth: &UploadAuthContext) -> ServiceResult<StorageReading> {
        let value = fetch_storage_with_endpoints(&self.endpoints, &self.client, &auth.bearer)
            .map_err(|e| classify("storage", e))?;
        let used = crate::util::parse_u64_field(&value, "usedSize")
            .or_else(|| crate::util::parse_u64_field(&value, "used"));
        let limit = crate::util::parse_u64_field(&value, "totalSize")
            .or_else(|| crate::util::parse_u64_field(&value, "storageLimit"));
        let percent = match (used, limit) {
            (Some(u), Some(l)) if l > 0 => Some(u as f64 * 100.0 / l as f64),
            _ => None,
        };
        Ok(StorageReading {
            used,
            limit,
            percent,
        })
    }
    fn reading(&mut self, auth: &UploadAuthContext, date: &str) -> ServiceResult<ReadingStats> {
        let reading_info =
            fetch_reading_info_with_endpoints(&self.endpoints, &self.client, &auth.bearer)
                .map_err(|e| classify("reading", e))?;
        let week = fetch_read_time_info_with_endpoints(
            &self.endpoints,
            &self.client,
            &auth.bearer,
            date,
            "week",
        )
        .map_err(|e| classify("reading", e))?;
        let day =
            fetch_day_read_with_endpoints(&self.endpoints, &self.client, &auth.bearer, date, date)
                .map_err(|e| classify("reading", e))?;
        Ok(ReadingStats {
            date: date.to_string(),
            reading_info,
            week,
            day,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::cell::RefCell;

    fn auth() -> UploadAuthContext {
        UploadAuthContext {
            bearer: "token".into(),
            uid: "user".into(),
            storage_limit: None,
            storage_used: None,
        }
    }

    fn sts() -> OssSts {
        OssSts {
            access_key_id: "id".into(),
            access_key_secret: "secret".into(),
            security_token: "sts".into(),
            expiration: "2099-01-01T00:00:00Z".into(),
        }
    }

    struct FakePush {
        save_ok: bool,
        confirm_ok: bool,
        upload_ok: bool,
        auth_ok: bool,
        storage: Option<(u64, u64)>,
        wrote: RefCell<Vec<String>>,
        auth_calls: RefCell<usize>,
        sts_calls: RefCell<usize>,
        upload_calls: RefCell<usize>,
    }

    impl FakePush {
        fn ok() -> Self {
            Self {
                save_ok: true,
                confirm_ok: true,
                upload_ok: true,
                auth_ok: true,
                storage: None,
                wrote: RefCell::new(vec![]),
                auth_calls: RefCell::new(0),
                sts_calls: RefCell::new(0),
                upload_calls: RefCell::new(0),
            }
        }
    }

    impl PushBackend for FakePush {
        fn auth_context(&mut self) -> ServiceResult<UploadAuthContext> {
            *self.auth_calls.borrow_mut() += 1;
            if !self.auth_ok {
                return Err(ServiceError::new(
                    ServiceErrorKind::Authentication,
                    "credential_rejected",
                    "auth",
                    "credentials rejected",
                ));
            }
            let mut auth = auth();
            if let Some((limit, used)) = self.storage {
                auth.storage_limit = Some(limit);
                auth.storage_used = Some(used);
            }
            Ok(auth)
        }
        fn default_bucket(&mut self) -> ServiceResult<(String, BucketConfig)> {
            Ok(("onyx-cloud".into(), BucketConfig::default()))
        }
        fn sts(&mut self, _: &UploadAuthContext) -> ServiceResult<OssSts> {
            *self.sts_calls.borrow_mut() += 1;
            Ok(sts())
        }
        fn upload(
            &mut self,
            _: &BucketConfig,
            _: &OssSts,
            _: &str,
            _: &Path,
            _: &str,
        ) -> ServiceResult<()> {
            *self.upload_calls.borrow_mut() += 1;
            if self.upload_ok {
                Ok(())
            } else {
                Err(ServiceError::network("upload", "upload failed"))
            }
        }
        fn signed_url(
            &mut self,
            _: &UploadAuthContext,
            _: &BucketConfig,
            _: &str,
        ) -> ServiceResult<String> {
            Ok("https://signed.example/file".into())
        }
        fn write_message(
            &mut self,
            _: &UploadAuthContext,
            _: &Path,
            _: &str,
            _: u64,
            _: &str,
            _: &str,
            _: &str,
        ) -> ServiceResult<(String, String)> {
            Ok(("doc-1".into(), "1-rev".into()))
        }
        fn confirm_sync(&mut self, _: &UploadAuthContext, _: &str, _: &str) -> ServiceResult<()> {
            if self.confirm_ok {
                Ok(())
            } else {
                Err(ServiceError::network("confirm_sync", "sync failed"))
            }
        }
        fn save_and_push(
            &mut self,
            _: &UploadAuthContext,
            _: &str,
            _: &str,
            name: &str,
            _: &str,
            _: &str,
            _: &str,
        ) -> ServiceResult<()> {
            self.wrote.borrow_mut().push(name.into());
            if self.save_ok {
                Ok(())
            } else {
                Err(ServiceError::new(
                    ServiceErrorKind::Unknown,
                    "submit_unknown",
                    "save_and_push",
                    "save failed",
                ))
            }
        }
    }

    fn temp_file(bytes: &[u8]) -> PathBuf {
        let path = std::env::temp_dir().join(format!("sandflos-services-{}.pdf", Uuid::new_v4()));
        fs::write(&path, bytes).unwrap();
        path
    }

    #[test]
    fn one_pdf_returns_accepted_message_id_without_dashboard_work() {
        let path = temp_file(b"%PDF-1.7");
        let mut service = PushService::new(FakePush::ok());
        let report = service.push_files(std::slice::from_ref(&path), |_| {});
        fs::remove_file(path).ok();
        assert_eq!(report.accepted, 1);
        assert_eq!(report.exit_code(), 0);
        assert_eq!(report.files[0].message_id.as_deref(), Some("doc-1"));
        assert_eq!(service.backend().wrote.borrow().len(), 1);
        assert!(service.backend().wrote.borrow()[0].ends_with(".pdf"));
    }

    #[test]
    fn accepted_push_reports_each_remote_stage_in_order() {
        let path = temp_file(b"%PDF-1.7");
        let mut service = PushService::new(FakePush::ok());
        let mut events = Vec::new();
        let report = service.push_files(std::slice::from_ref(&path), |event| events.push(event));
        fs::remove_file(path).ok();

        assert_eq!(report.accepted, 1);
        assert!(matches!(
            events.first(),
            Some(PushProgress::FileStarted {
                index: 0,
                total: 1,
                ..
            })
        ));
        let stages = events
            .iter()
            .filter_map(|event| match event {
                PushProgress::Phase { stage, .. } => Some(*stage),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            stages,
            vec![
                PushStage::UploadOss,
                PushStage::WriteMessage,
                PushStage::ConfirmSync,
                PushStage::SaveAndPush,
            ]
        );
        assert!(events
            .iter()
            .any(|event| matches!(event, PushProgress::Uploaded { bytes: 8, .. })));
        assert!(matches!(
            events.last(),
            Some(PushProgress::Finished(PushFileResult {
                outcome: PushOutcome::Accepted,
                ..
            }))
        ));
    }

    #[test]
    fn save_and_push_failure_is_unknown_not_success() {
        let path = temp_file(b"%PDF-1.7");
        let mut service = PushService::new(FakePush {
            save_ok: false,
            ..FakePush::ok()
        });
        let report = service.push_files(std::slice::from_ref(&path), |_| {});
        fs::remove_file(path).ok();
        assert_eq!(report.accepted, 0);
        assert_eq!(report.unknown, 1);
        assert_eq!(report.exit_code(), 6);
        assert_eq!(report.files[0].message_id.as_deref(), Some("doc-1"));
    }

    #[test]
    fn unconfirmed_sync_is_unknown_and_keeps_message_identity() {
        let path = temp_file(b"%PDF-1.7");
        let mut service = PushService::new(FakePush {
            confirm_ok: false,
            ..FakePush::ok()
        });
        let report = service.push_files(std::slice::from_ref(&path), |_| {});
        fs::remove_file(path).ok();

        assert_eq!(report.accepted, 0);
        assert_eq!(report.unknown, 1);
        assert_eq!(report.exit_code(), 6);
        assert_eq!(report.files[0].stage, PushStage::ConfirmSync);
        assert_eq!(report.files[0].message_id.as_deref(), Some("doc-1"));
        assert_eq!(
            report.files[0].error.as_ref().unwrap().kind,
            ServiceErrorKind::Network
        );
    }

    #[test]
    fn second_file_failure_returns_partial_report() {
        let first = temp_file(b"ok");
        let second = temp_file(b"");
        let mut service = PushService::new(FakePush::ok());
        let report = service.push_files(&[first.clone(), second.clone()], |_| {});
        fs::remove_file(first).ok();
        fs::remove_file(second).ok();
        assert_eq!(report.accepted, 1);
        assert_eq!(report.failed, 1);
        assert_eq!(report.exit_code(), 6);
        assert_eq!(report.files[1].error.as_ref().unwrap().code, "empty_file");
    }

    #[test]
    fn all_failures_pick_a_stable_priority_exit_code() {
        let path = temp_file(b"");
        let mut service = PushService::new(FakePush::ok());
        let report = service.push_files(std::slice::from_ref(&path), |_| {});
        fs::remove_file(path).ok();
        assert_eq!(report.exit_code(), 2);
        assert_eq!(report.failed, 1);
    }

    #[test]
    fn missing_files_fail_before_authentication() {
        let missing = std::env::temp_dir().join(format!("sandflos-missing-{}.pdf", Uuid::new_v4()));
        let mut service = PushService::new(FakePush {
            auth_ok: false,
            ..FakePush::ok()
        });
        let report = service.push_files(&[missing], |_| {});
        assert_eq!(*service.backend().auth_calls.borrow(), 0);
        assert_eq!(
            report.files[0].error.as_ref().unwrap().code,
            "file_unreadable"
        );
        assert_eq!(
            report.files[0].error.as_ref().unwrap().kind,
            ServiceErrorKind::LocalIo
        );
        assert_eq!(report.exit_code(), 5);
    }

    #[test]
    fn local_failures_keep_their_input_position_when_auth_fails() {
        let missing = std::env::temp_dir().join(format!("sandflos-missing-{}.pdf", Uuid::new_v4()));
        let valid = temp_file(b"valid");
        let mut service = PushService::new(FakePush {
            auth_ok: false,
            ..FakePush::ok()
        });
        let report = service.push_files(&[missing, valid.clone()], |_| {});
        fs::remove_file(valid).ok();
        assert_eq!(*service.backend().auth_calls.borrow(), 1);
        assert_eq!(report.files.len(), 2);
        assert_eq!(
            report.files[0].error.as_ref().unwrap().code,
            "file_unreadable"
        );
        assert_eq!(
            report.files[1].error.as_ref().unwrap().kind,
            ServiceErrorKind::Authentication
        );
        assert_eq!(report.exit_code(), 3);
    }

    #[test]
    fn oversized_file_is_rejected_before_authentication() {
        let path = std::env::temp_dir().join(format!("sandflos-large-{}.pdf", Uuid::new_v4()));
        let file = fs::File::create(&path).unwrap();
        file.set_len(MAX_SINGLE_UPLOAD_BYTES + 1).unwrap();
        drop(file);
        let mut service = PushService::new(FakePush {
            auth_ok: false,
            ..FakePush::ok()
        });
        let report = service.push_files(std::slice::from_ref(&path), |_| {});
        fs::remove_file(path).ok();

        assert_eq!(*service.backend().auth_calls.borrow(), 0);
        assert_eq!(
            report.files[0].error.as_ref().unwrap().code,
            "file_too_large"
        );
        assert_eq!(report.exit_code(), 2);
    }

    #[test]
    fn quota_equal_to_planned_bytes_is_allowed() {
        let path = temp_file(b"");
        fs::write(&path, b"1234567890").unwrap();
        let mut service = PushService::new(FakePush {
            storage: Some((10, 0)),
            ..FakePush::ok()
        });
        let report = service.push_files(std::slice::from_ref(&path), |_| {});
        fs::remove_file(path).ok();

        assert_eq!(report.accepted, 1);
        assert_eq!(*service.backend().sts_calls.borrow(), 1);
        assert_eq!(report.exit_code(), 0);
    }

    #[test]
    fn quota_is_checked_before_sts_and_upload() {
        let path = temp_file(b"1234567890");
        let mut service = PushService::new(FakePush {
            storage: Some((10, 5)),
            ..FakePush::ok()
        });
        let report = service.push_files(std::slice::from_ref(&path), |_| {});
        fs::remove_file(path).ok();
        assert_eq!(
            report.files[0].error.as_ref().unwrap().code,
            "quota_exceeded"
        );
        assert_eq!(*service.backend().sts_calls.borrow(), 0);
        assert_eq!(*service.backend().upload_calls.borrow(), 0);
        assert_eq!(report.exit_code(), 2);
    }

    struct FakeQueue {
        items: Vec<DashboardPushItem>,
        deleted: RefCell<Vec<&'static str>>,
    }
    fn item(id: &str, updated_at: i64) -> DashboardPushItem {
        DashboardPushItem {
            id: id.into(),
            rev: None,
            name: id.into(),
            size: None,
            updated_at: Some(updated_at),
            format: None,
            resource_key: None,
        }
    }
    impl QueueReadBackend for FakeQueue {
        fn auth_context(&mut self) -> ServiceResult<UploadAuthContext> {
            Ok(auth())
        }
        fn queue_window(&mut self, _: &UploadAuthContext) -> ServiceResult<Vec<DashboardPushItem>> {
            Ok(self.items.clone())
        }
    }
    impl QueueMutationBackend for FakeQueue {
        fn auth_context(&mut self) -> ServiceResult<UploadAuthContext> {
            Ok(auth())
        }
        fn resend(&mut self, _: &UploadAuthContext, _: &str) -> ServiceResult<()> {
            Ok(())
        }
        fn delete_push_doc(&mut self, _: &UploadAuthContext, _: &str) -> ServiceResult<()> {
            Ok(())
        }
        fn delete_message_record(&mut self, _: &UploadAuthContext, id: &str) -> ServiceResult<()> {
            let leaked: &'static str = Box::leak(id.to_string().into_boxed_str());
            self.deleted.borrow_mut().push(leaked);
            Ok(())
        }
    }

    #[test]
    fn queue_list_is_bounded_and_marks_truncation() {
        let items = (0..35).map(|i| item(&format!("id-{i}"), i)).collect();
        let mut service = QueueService::new(FakeQueue {
            items,
            deleted: RefCell::new(vec![]),
        });
        let listing = service.list(Some(10)).unwrap();
        assert_eq!(listing.items.len(), 10);
        assert_eq!(listing.upstream_window, 30);
        assert!(listing.truncated);
        assert_eq!(listing.items[0].id, "id-34");
    }

    #[test]
    fn delete_without_yes_performs_no_remote_mutation() {
        let mut service = QueueService::new(FakeQueue {
            items: vec![],
            deleted: RefCell::new(vec![]),
        });
        let error = service.delete("id-1", false).unwrap_err();
        assert_eq!(error.code, "confirmation_required");
        assert!(service.backend().deleted.borrow().is_empty());
    }

    struct TwoStepQueue {
        remote: ServiceResult<()>,
        record: ServiceResult<()>,
        calls: RefCell<Vec<&'static str>>,
    }
    impl QueueReadBackend for TwoStepQueue {
        fn auth_context(&mut self) -> ServiceResult<UploadAuthContext> {
            Ok(auth())
        }
        fn queue_window(&mut self, _: &UploadAuthContext) -> ServiceResult<Vec<DashboardPushItem>> {
            Ok(vec![])
        }
    }
    impl QueueMutationBackend for TwoStepQueue {
        fn auth_context(&mut self) -> ServiceResult<UploadAuthContext> {
            Ok(auth())
        }
        fn resend(&mut self, _: &UploadAuthContext, _: &str) -> ServiceResult<()> {
            Ok(())
        }
        fn delete_push_doc(&mut self, _: &UploadAuthContext, _: &str) -> ServiceResult<()> {
            self.calls.borrow_mut().push("remote");
            self.remote.clone()
        }
        fn delete_message_record(&mut self, _: &UploadAuthContext, _: &str) -> ServiceResult<()> {
            self.calls.borrow_mut().push("record");
            self.record.clone()
        }
    }

    #[test]
    fn delete_reports_partial_completion_when_second_step_fails() {
        let mut service = QueueService::new(TwoStepQueue {
            remote: Ok(()),
            record: Err(ServiceError::protocol(
                "queue.delete",
                "record delete failed",
            )),
            calls: RefCell::new(vec![]),
        });
        let outcome = service.delete("id-1", true).unwrap();
        assert!(outcome.remote_delete_attempted);
        assert!(!outcome.complete);
        assert!(outcome.unknown);
        assert_eq!(
            service.backend().calls.borrow().as_slice(),
            &["remote", "record"]
        );
    }

    #[test]
    fn delete_treats_missing_doc_and_record_as_idempotent_success() {
        let mut service = QueueService::new(TwoStepQueue {
            remote: Err(ServiceError::new(
                ServiceErrorKind::NotFound,
                "not_found",
                "queue.delete",
                "gone",
            )),
            record: Err(ServiceError::new(
                ServiceErrorKind::NotFound,
                "not_found",
                "queue.delete",
                "gone",
            )),
            calls: RefCell::new(vec![]),
        });
        let outcome = service.delete("id-1", true).unwrap();
        assert!(outcome.complete);
        assert!(!outcome.remote_delete_attempted);
        assert!(outcome.message_delete_attempted);
    }

    struct FakeQuery {
        calls: RefCell<Vec<String>>,
    }
    impl QueryBackend for FakeQuery {
        fn auth_context(&mut self) -> ServiceResult<UploadAuthContext> {
            Ok(auth())
        }
        fn devices(&mut self, _: &UploadAuthContext) -> ServiceResult<Vec<DashboardDevice>> {
            self.calls.borrow_mut().push("devices".into());
            Ok(vec![])
        }
        fn storage(&mut self, _: &UploadAuthContext) -> ServiceResult<StorageReading> {
            self.calls.borrow_mut().push("storage".into());
            Ok(StorageReading {
                used: Some(10),
                limit: Some(100),
                percent: Some(10.0),
            })
        }
        fn reading(&mut self, _: &UploadAuthContext, date: &str) -> ServiceResult<ReadingStats> {
            self.calls.borrow_mut().push("reading".into());
            Ok(ReadingStats {
                date: date.into(),
                reading_info: json!({}),
                week: json!({}),
                day: json!({}),
            })
        }
    }

    #[test]
    fn queue_not_found_is_classified_for_idempotent_delete() {
        let error = classify("queue.delete", "HTTP 404 Not Found: gone".to_string());
        assert_eq!(error.kind, ServiceErrorKind::NotFound);
        assert_eq!(error.code, "not_found");
    }

    #[test]
    fn devices_list_does_not_fetch_reading_metrics() {
        let mut service = QueryService::new(FakeQuery {
            calls: RefCell::new(vec![]),
        });
        service.devices().unwrap();
        assert_eq!(
            service.backend().calls.borrow().as_slice(),
            &["devices".to_string()]
        );
    }

    #[test]
    fn reading_rejects_invalid_dates_before_network() {
        let mut service = QueryService::new(FakeQuery {
            calls: RefCell::new(vec![]),
        });
        assert_eq!(
            service.reading("2026-02-30").unwrap_err().code,
            "invalid_date"
        );
        assert!(service.backend().calls.borrow().is_empty());
    }
}
