use crate::api::create_client;
use crate::api::{
    batch_delete_push_message_with_endpoints, fetch_auth_context_and_user_with_endpoints,
    fetch_default_bucket_with_endpoints, fetch_push_doc_value_with_endpoints,
    fetch_push_doc_with_endpoints, fetch_sts_for_auth_with_endpoints,
    put_existing_push_message_doc_with_endpoints, put_push_message_doc_with_endpoints,
    save_and_push_with_endpoints,
};
use crate::dashboard::build_dashboard_snapshot;
use crate::device::{is_local_transfer_host, normalize_transfer_host_url};
use crate::models::{BucketConfig, DashboardSnapshot, OssSts, UploadAuthContext};
use crate::state::{
    finish_upload_task, set_dashboard_cache, update_dashboard_cache_after_delete,
    update_upload_runtime_state,
};
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use reqwest::blocking::Client;
use reqwest::header::{AUTHORIZATION, CONTENT_LENGTH, CONTENT_TYPE};
use serde_json::{json, Value};
use sha1::Sha1;
use std::{
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use uuid::Uuid;

const MAX_SINGLE_UPLOAD_BYTES: u64 = 200 * 1024 * 1024;
const MIN_STS_REMAINING_SECS_FOR_SIGNED_URL: i64 = 120;

#[derive(Debug, Clone)]
pub struct UploadItem {
    pub path: PathBuf,
    pub display_name: Option<String>,
}

struct PreparedUploadFile {
    _original_path: PathBuf,
    upload_path: PathBuf,
    display_name: Option<String>,
}

pub struct UploadExecutionSummary {
    pub success: usize,
    pub failed: usize,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

impl PreparedUploadFile {
    fn file_name(&self) -> Result<&str, String> {
        if let Some(name) = self.display_name.as_deref() {
            return Ok(name);
        }
        self.upload_path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| format!("{}: 文件名解析失败", self.upload_path.to_string_lossy()))
    }
}

fn prepare_upload_file(path: &Path) -> Result<PreparedUploadFile, String> {
    Ok(PreparedUploadFile {
        _original_path: path.to_path_buf(),
        upload_path: path.to_path_buf(),
        display_name: None,
    })
}

fn prepare_upload_item(item: &UploadItem) -> Result<PreparedUploadFile, String> {
    let mut prepared = prepare_upload_file(&item.path)?;
    prepared.display_name = item.display_name.clone();
    Ok(prepared)
}

fn validate_storage_quota(auth: &UploadAuthContext, files: &[UploadItem]) -> Result<(), String> {
    let total_size: u64 = files
        .iter()
        .filter_map(|item| fs::metadata(&item.path).ok().map(|meta| meta.len()))
        .sum();
    if let (Some(limit), Some(used)) = (auth.storage_limit, auth.storage_used) {
        let remaining = limit.saturating_sub(used);
        if total_size > remaining {
            return Err(format!(
                "可用空间不足，剩余 {} 字节，待上传 {} 字节",
                remaining, total_size
            ));
        }
    }
    Ok(())
}

fn validate_storage_quota_for_size(
    auth: &UploadAuthContext,
    uploaded_size: u64,
    next_size: u64,
) -> Result<(), String> {
    if let (Some(limit), Some(used)) = (auth.storage_limit, auth.storage_used) {
        let remaining = limit.saturating_sub(used);
        let planned = uploaded_size.saturating_add(next_size);
        if planned > remaining {
            return Err(format!(
                "可用空间不足，剩余 {} 字节，当前任务需 {} 字节",
                remaining, planned
            ));
        }
    }
    Ok(())
}

pub(crate) fn content_type_for(path: &Path) -> String {
    mime_guess::from_path(path)
        .first_or_octet_stream()
        .essence_str()
        .to_string()
}

pub(crate) fn build_oss_host(bucket: &BucketConfig) -> Result<(String, String), String> {
    let bucket_name = bucket
        .bucket
        .clone()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| "bucket 缺少 bucket 字段".to_string())?;
    let endpoint = bucket
        .ali_endpoint
        .clone()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| "bucket 缺少 aliEndpoint".to_string())?;
    let normalized = endpoint
        .trim()
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_matches('/');
    Ok((bucket_name.clone(), format!("{bucket_name}.{normalized}")))
}

pub(crate) fn sign_oss(secret: &str, string_to_sign: &str) -> Result<String, String> {
    let mut mac = Hmac::<Sha1>::new_from_slice(secret.as_bytes()).map_err(|err| err.to_string())?;
    mac.update(string_to_sign.as_bytes());
    Ok(BASE64_STANDARD.encode(mac.finalize().into_bytes()))
}

fn build_object_key(uid: &str, path: &Path) -> (String, String) {
    let ext = path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("bin")
        .to_ascii_lowercase();
    let object_key = format!("{uid}/push/{}.{}", Uuid::new_v4(), ext);
    (object_key, ext)
}

fn read_upload_bytes_with_progress<F>(
    file_path: &Path,
    mut on_progress: F,
) -> Result<Vec<u8>, String>
where
    F: FnMut(u64, u64, Duration),
{
    let mut file = File::open(file_path).map_err(|err| err.to_string())?;
    let file_size = file.metadata().map_err(|err| err.to_string())?.len();
    let mut bytes = Vec::with_capacity(file_size.min(8 * 1024 * 1024) as usize);
    let mut buffer = [0_u8; 256 * 1024];
    let mut sent = 0_u64;
    let mut last_emit = Instant::now();
    let started = Instant::now();

    loop {
        let read = file.read(&mut buffer).map_err(|err| err.to_string())?;
        if read == 0 {
            on_progress(sent, file_size, started.elapsed());
            break;
        }
        bytes.extend_from_slice(&buffer[..read]);
        sent = sent.saturating_add(read as u64).min(file_size);
        let now = Instant::now();
        if now.duration_since(last_emit) >= Duration::from_millis(120) || sent >= file_size {
            last_emit = now;
            on_progress(sent, file_size, now.duration_since(started));
        }
    }

    Ok(bytes)
}

fn upload_to_oss<F>(
    client: &Client,
    bucket: &BucketConfig,
    sts: &OssSts,
    object_key: &str,
    file_path: &Path,
    on_progress: F,
) -> Result<(), String>
where
    F: FnMut(u64, u64, Duration) + Send + 'static,
{
    let (bucket_name, host) = build_oss_host(bucket)?;
    let canonical_resource = format!("/{bucket_name}/{object_key}");
    let date = httpdate::fmt_http_date(SystemTime::now());
    let content_type = content_type_for(file_path);
    let string_to_sign = format!(
        "PUT

{content_type}
{date}
x-oss-security-token:{}
{canonical_resource}",
        sts.security_token
    );
    let signature = sign_oss(&sts.access_key_secret, &string_to_sign)?;
    let authorization = format!("OSS {}:{}", sts.access_key_id, signature);
    let file_size = fs::metadata(file_path)
        .map_err(|err| err.to_string())?
        .len();
    let body = read_upload_bytes_with_progress(file_path, on_progress)?;
    let url = format!("https://{host}/{object_key}");

    let response = client
        .put(&url)
        .header("Date", date)
        .header(CONTENT_TYPE, content_type)
        .header(CONTENT_LENGTH, file_size.to_string())
        .header("x-oss-security-token", sts.security_token.clone())
        .header(AUTHORIZATION, authorization)
        .body(body)
        .send()
        .map_err(|err| err.to_string())?;
    if response.status().is_success() {
        Ok(())
    } else {
        let status = response.status();
        let body = response.text().unwrap_or_default();
        Err(format!("OSS 上传失败 ({status}): {body}"))
    }
}

fn signed_download_url_with_endpoints(
    endpoints: &crate::api::ApiEndpoints,
    bucket: &BucketConfig,
    sts: &OssSts,
    object_key: &str,
) -> Result<String, String> {
    let (bucket_name, host) = build_oss_host(bucket)?;
    let expires = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|err| err.to_string())?
        .as_secs()
        + 10_000;
    let response_disposition = "attachment";
    let mut subresources = [
        (
            "response-content-disposition",
            response_disposition.to_string(),
        ),
        ("security-token", sts.security_token.clone()),
    ];
    subresources.sort_by(|a, b| a.0.cmp(b.0));
    let canonical_subresources = subresources
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join("&");
    let canonical_resource = format!("/{bucket_name}/{object_key}?{canonical_subresources}");
    let string_to_sign = format!(
        "GET


{expires}
{canonical_resource}"
    );
    let signature = sign_oss(&sts.access_key_secret, &string_to_sign)?;
    let signed_query = [
        ("OSSAccessKeyId", sts.access_key_id.clone()),
        ("Expires", expires.to_string()),
        ("Signature", signature),
        (
            "response-content-disposition",
            response_disposition.to_string(),
        ),
        ("security-token", sts.security_token.clone()),
    ];
    let encoded_pairs = signed_query
        .into_iter()
        .map(|(key, value)| {
            format!(
                "{}={}",
                urlencoding::encode(key),
                urlencoding::encode(&value)
            )
        })
        .collect::<Vec<_>>();
    Ok(format!(
        "{}?{}",
        endpoints.oss_url(&format!("https://{host}/{object_key}")),
        encoded_pairs.join("&")
    ))
}

fn diagnostic_excerpt(text: &str, max_chars: usize) -> String {
    let mut out = String::new();
    for (idx, ch) in text.chars().enumerate() {
        if idx >= max_chars {
            out.push('…');
            break;
        }
        out.push(ch);
    }
    out
}

fn verify_oss_object_via_signed_url(client: &Client, signed_url: &str) -> Result<(), String> {
    let response = client
        .get(signed_url)
        .header("Range", "bytes=0-0")
        .send()
        .map_err(|err| err.to_string())?;
    let status = response.status();
    if status.as_u16() == 200 || status.as_u16() == 206 {
        return Ok(());
    }
    let body = response.text().unwrap_or_default();
    Err(format!("HTTP {status}: {}", diagnostic_excerpt(&body, 220)))
}

fn sts_remaining_seconds(sts: &OssSts) -> Option<i64> {
    let expiration = DateTime::parse_from_rfc3339(&sts.expiration).ok()?;
    Some(expiration.timestamp() - Utc::now().timestamp())
}

pub(crate) fn build_verified_signed_download_url_with_endpoints(
    endpoints: &crate::api::ApiEndpoints,
    client: &Client,
    auth: &UploadAuthContext,
    bucket: &BucketConfig,
    object_key: &str,
) -> Result<String, String> {
    let mut last_err = None;

    for attempt in 0..3 {
        let sts = fetch_sts_for_auth_with_endpoints(endpoints, client, auth)?;
        let remaining_secs = sts_remaining_seconds(&sts);

        if remaining_secs
            .map(|value| value <= MIN_STS_REMAINING_SECS_FOR_SIGNED_URL)
            .unwrap_or(false)
        {
            let err = format!(
                "STS 剩余有效期过短: {}s",
                remaining_secs.unwrap_or_default()
            );
            last_err = Some(err);
        } else {
            let signed_url =
                signed_download_url_with_endpoints(endpoints, bucket, &sts, object_key)?;
            match verify_oss_object_via_signed_url(client, &signed_url) {
                Ok(()) => return Ok(signed_url),
                Err(err) => last_err = Some(err),
            }
        }

        if attempt < 2 {
            thread::sleep(Duration::from_millis(400 * (attempt as u64 + 1)));
        }
    }

    Err(last_err.unwrap_or_else(|| "生成可用的签名下载链接失败".to_string()))
}

pub(crate) fn wait_for_remote_push_doc_ready_with_endpoints(
    endpoints: &crate::api::ApiEndpoints,
    client: &Client,
    auth: &UploadAuthContext,
    doc_id: &str,
    expected_rev: &str,
) -> Result<(), String> {
    let mut last_err = None;
    for attempt in 0..8 {
        match fetch_push_doc_with_endpoints(endpoints, client, auth, doc_id) {
            Ok(detail) if detail.rev == expected_rev => return Ok(()),
            Ok(detail) => {
                last_err = Some(format!(
                    "推送消息 rev 尚未稳定，期望 {expected_rev}，实际 {}",
                    detail.rev
                ));
            }
            Err(err) => last_err = Some(err),
        }
        if attempt < 7 {
            thread::sleep(Duration::from_millis(250 * (attempt as u64 + 1)));
        }
    }
    Err(last_err.unwrap_or_else(|| "推送消息未能及时同步到 neocloud".to_string()))
}

fn update_push_doc_for_resend_at(
    doc: &mut Value,
    signed_url: &str,
    now_ms: u64,
) -> Result<(), String> {
    let mut content = match doc.get("content") {
        Some(Value::String(text)) => serde_json::from_str::<Value>(text)
            .map_err(|err| format!("推送记录 content 解析失败: {err}"))?,
        Some(Value::Object(_)) => doc.get("content").cloned().unwrap_or(Value::Null),
        _ => return Err("推送记录缺少 content".to_string()),
    };
    let format = content
        .get("formats")
        .and_then(Value::as_array)
        .and_then(|formats| formats.first())
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(|value| value.to_ascii_lowercase())
        .unwrap_or_else(|| "pdf".to_string());

    content["url"] = json!(signed_url);
    content["format"] = json!(format);
    content["updatedAt"] = json!(now_ms);

    let serialized =
        serde_json::to_string(&content).map_err(|err| format!("序列化 content 失败: {err}"))?;
    doc["content"] = Value::String(serialized);
    doc["updatedAt"] = json!(now_ms);
    Ok(())
}

pub fn update_upload_transfer_metrics(
    seq: usize,
    total: usize,
    file_name: &str,
    sent: u64,
    file_total: u64,
    speed_bps: f64,
) {
    let percent = if file_total > 0 {
        ((sent as f64 / file_total as f64) * 100.0).clamp(0.0, 100.0)
    } else {
        0.0
    };
    let eta_seconds = if speed_bps > 0.0 && file_total >= sent {
        ((file_total - sent) as f64 / speed_bps).max(0.0)
    } else {
        0.0
    };
    let status_text = format!(
        "上传中 ({}/{}): {:.0}% ({})",
        seq,
        total,
        percent,
        crate::util::truncate_menu_title(file_name)
    );
    let name_owned = file_name.to_string();
    update_upload_runtime_state(move |state| {
        state.status_text = status_text;
        state.current_file = Some(name_owned);
        state.bytes_sent = Some(sent);
        state.bytes_total = Some(file_total);
        state.progress_percent = Some(percent);
        state.speed_bps = Some(speed_bps);
        state.eta_seconds = Some(eta_seconds);
    });
}

pub fn clear_upload_transfer_metrics() {
    update_upload_runtime_state(|state| {
        state.current_file = None;
        state.bytes_sent = None;
        state.bytes_total = None;
        state.progress_percent = None;
        state.speed_bps = None;
        state.eta_seconds = None;
    });
}

pub fn set_upload_progress_label(text: &str) {
    let text_owned = text.to_string();
    update_upload_runtime_state(move |state| {
        state.status_text = text_owned;
    });
}

#[allow(clippy::too_many_arguments)]
fn upload_single_file_with_metrics(
    client: &Client,
    bucket: &BucketConfig,
    sts: &OssSts,
    object_key: &str,
    file_path: &Path,
    seq: usize,
    total: usize,
    file_name: &str,
) -> Result<(), String> {
    let name_for_progress = file_name.to_string();
    upload_to_oss(
        client,
        bucket,
        sts,
        object_key,
        file_path,
        move |sent, file_total, elapsed| {
            let speed_bps = if elapsed.as_secs_f64() > 0.05 {
                sent as f64 / elapsed.as_secs_f64()
            } else {
                0.0
            };
            update_upload_transfer_metrics(
                seq,
                total,
                &name_for_progress,
                sent,
                file_total,
                speed_bps,
            );
        },
    )
}

pub fn perform_native_uploads(files: &[UploadItem]) -> Result<UploadExecutionSummary, String> {
    let endpoints = &crate::api::ApiEndpoints::production();
    if files.is_empty() {
        return Err("未选择任何文件".to_string());
    }

    let client = create_client(60)?;
    let (auth, _) = fetch_auth_context_and_user_with_endpoints(endpoints, &client)?;
    validate_storage_quota(&auth, files)?;
    let (bucket_key, bucket) = fetch_default_bucket_with_endpoints(endpoints, &client)?;

    let mut success = 0;
    let mut failed = 0;
    let mut errors = Vec::new();
    let mut warnings = Vec::new();
    let mut uploaded_size = 0_u64;

    for (idx, item) in files.iter().enumerate() {
        let seq = idx + 1;
        let prepared = match prepare_upload_item(item) {
            Ok(file) => file,
            Err(err) => {
                failed += 1;
                errors.push(err);
                continue;
            }
        };

        let file_name = match prepared.file_name() {
            Ok(name) => name.to_string(),
            Err(err) => {
                failed += 1;
                errors.push(err);
                continue;
            }
        };

        let file_size = match fs::metadata(&prepared.upload_path) {
            Ok(meta) => meta.len(),
            Err(err) => {
                failed += 1;
                errors.push(format!("{file_name}: 读取文件大小失败 ({err})"));
                continue;
            }
        };

        if file_size > MAX_SINGLE_UPLOAD_BYTES {
            failed += 1;
            errors.push(format!("{file_name}: 超过单文件 200MB 限制"));
            continue;
        }

        if let Err(err) = validate_storage_quota_for_size(&auth, uploaded_size, file_size) {
            failed += 1;
            errors.push(format!("{file_name}: {err}"));
            continue;
        }

        let sts = match fetch_sts_for_auth_with_endpoints(endpoints, &client, &auth) {
            Ok(sts) => sts,
            Err(err) => {
                failed += 1;
                errors.push(format!("{file_name}: 获取 STS 失败 ({err})"));
                continue;
            }
        };

        let (object_key, ext) = build_object_key(&auth.uid, &prepared.upload_path);
        if let Err(err) = upload_single_file_with_metrics(
            &client,
            &bucket,
            &sts,
            &object_key,
            &prepared.upload_path,
            seq,
            files.len(),
            &file_name,
        ) {
            failed += 1;
            errors.push(format!("{file_name}: 上传至 OSS 失败 ({err})"));
            continue;
        }

        let signed_url = match build_verified_signed_download_url_with_endpoints(
            endpoints,
            &client,
            &auth,
            &bucket,
            &object_key,
        ) {
            Ok(url) => url,
            Err(err) => {
                failed += 1;
                errors.push(format!("{file_name}: 生成签名下载链接失败 ({err})"));
                continue;
            }
        };

        let resource_type = ext.as_str();
        let (cb_id, cb_rev) = match put_push_message_doc_with_endpoints(
            endpoints,
            &client,
            &auth,
            &file_name,
            file_size,
            resource_type,
            &object_key,
            &signed_url,
        ) {
            Ok(pair) => pair,
            Err(err) => {
                failed += 1;
                errors.push(format!("{file_name}: 写入推送消息失败: {err}"));
                continue;
            }
        };

        if let Err(err) = wait_for_remote_push_doc_ready_with_endpoints(
            endpoints, &client, &auth, &cb_id, &cb_rev,
        ) {
            failed += 1;
            errors.push(format!("{file_name}: 推送消息同步确认失败: {err}"));
            continue;
        }

        let save_result = save_and_push_with_endpoints(
            endpoints,
            &client,
            &auth,
            &bucket_key,
            &object_key,
            &file_name,
            resource_type,
            &cb_id,
            &cb_rev,
        );
        if let Err(err) = save_result {
            warnings.push(format!("{file_name}: 保存/推送时提示 {err}"));
        }

        success += 1;
        uploaded_size = uploaded_size.saturating_add(file_size);
    }

    Ok(UploadExecutionSummary {
        success,
        failed,
        errors,
        warnings,
    })
}

pub fn finalize_upload_result(
    result: Result<UploadExecutionSummary, String>,
) -> Result<DashboardSnapshot, String> {
    finish_upload_task();
    match result {
        Ok(summary) if summary.success == 0 && summary.failed > 0 => {
            set_upload_progress_label("上传进度: 全部失败");
            clear_upload_transfer_metrics();
            let details = summary
                .errors
                .iter()
                .take(2)
                .cloned()
                .collect::<Vec<_>>()
                .join(
                    "
",
                );
            let details_for_state = details.clone();
            update_upload_runtime_state(move |state| {
                state.last_error = Some(details_for_state.clone());
            });
            Err(if details.is_empty() {
                "所有文件均未成功上传".to_string()
            } else {
                details
            })
        }
        Ok(summary) => {
            if summary.failed > 0 {
                set_upload_progress_label(&format!(
                    "上传进度: 成功{} 失败{}",
                    summary.success, summary.failed
                ));
                let details = summary
                    .errors
                    .iter()
                    .chain(summary.warnings.iter())
                    .take(2)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(
                        "
",
                    );
                let details_for_state = details.clone();
                update_upload_runtime_state(move |state| {
                    state.last_error = Some(details_for_state.clone());
                });
            } else {
                set_upload_progress_label("上传进度: 全部完成");
                update_upload_runtime_state(|state| {
                    state.last_error = None;
                });
            }
            let snapshot = build_dashboard_snapshot();
            set_dashboard_cache(snapshot.clone());
            Ok(snapshot)
        }
        Err(err) => {
            set_upload_progress_label("上传进度: 失败");
            clear_upload_transfer_metrics();
            let err_for_state = err.clone();
            update_upload_runtime_state(move |state| {
                state.last_error = Some(err_for_state.clone());
            });
            Err(err)
        }
    }
}

pub fn upload_files_blocking_with_active_task(
    files: Vec<PathBuf>,
) -> Result<DashboardSnapshot, String> {
    let items = files
        .into_iter()
        .map(|path| UploadItem {
            path,
            display_name: None,
        })
        .collect::<Vec<_>>();
    upload_items_blocking_with_active_task(items)
}

pub fn upload_items_blocking_with_active_task(
    files: Vec<UploadItem>,
) -> Result<DashboardSnapshot, String> {
    let result = perform_native_uploads(&files);
    finalize_upload_result(result)
}

pub fn dashboard_push_resend_inner(id: &str) -> Result<(), String> {
    let endpoints = &crate::api::ApiEndpoints::production();
    let client = create_client(60)?;
    let (auth, _) = fetch_auth_context_and_user_with_endpoints(endpoints, &client)?;
    push_resend_with_auth_with_endpoints(endpoints, &client, &auth, id)
        .map_err(|error| error.to_string())
}

/// Re-submit an existing push record using an explicitly resolved credential.
pub fn push_resend_with_auth(
    client: &Client,
    auth: &UploadAuthContext,
    id: &str,
) -> Result<(), String> {
    push_resend_with_auth_with_endpoints(&crate::api::ApiEndpoints::production(), client, auth, id)
        .map_err(|error| error.to_string())
}

/// Distinguish rejected preparation from a write whose result is unconfirmed.
#[derive(Debug)]
pub enum ResendError {
    BeforeSubmission(String),
    AfterSubmission(String),
}
impl From<String> for ResendError {
    fn from(error: String) -> Self {
        Self::BeforeSubmission(error)
    }
}
impl std::fmt::Display for ResendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BeforeSubmission(error) | Self::AfterSubmission(error) => f.write_str(error),
        }
    }
}
impl std::error::Error for ResendError {}

pub fn push_resend_with_auth_with_endpoints(
    endpoints: &crate::api::ApiEndpoints,
    client: &Client,
    auth: &UploadAuthContext,
    id: &str,
) -> Result<(), ResendError> {
    if id.trim().is_empty() {
        return Err(ResendError::BeforeSubmission("重推记录 id 不能为空".into()));
    }
    let (_bucket_key, bucket) = fetch_default_bucket_with_endpoints(endpoints, client)?;
    let mut raw_doc = fetch_push_doc_value_with_endpoints(endpoints, client, auth, id)?;
    let object_key = raw_doc
        .get("storageKey")
        .and_then(Value::as_str)
        .filter(|key| !key.trim().is_empty())
        .ok_or_else(|| "推送记录缺少有效的 storageKey".to_string())?
        .to_string();
    let signed_url = build_verified_signed_download_url_with_endpoints(
        endpoints,
        client,
        auth,
        &bucket,
        &object_key,
    )?;
    update_push_doc_for_resend_at(&mut raw_doc, &signed_url, crate::util::unix_ms_now() as u64)?;
    // Do not repeat a write after a gateway error or lost response. The
    // service exposes an unknown result so callers can inspect the queue.
    let result = put_existing_push_message_doc_with_endpoints(endpoints, client, auth, &raw_doc);
    if raw_doc
        .get("parent")
        .and_then(Value::as_str)
        .is_some_and(|parent| !parent.trim().is_empty())
    {
        match result {
            Ok((_, rev)) => {
                wait_for_remote_push_doc_ready_with_endpoints(endpoints, client, auth, id, &rev)
                    .map_err(ResendError::AfterSubmission)
            }
            // An explicit 404 is a confirmed rejection and allows the legacy
            // parent-record fallback; ambiguous errors never reach it.
            Err(error) if is_not_found_push_error(&error) => {
                let size = raw_doc.get("size").and_then(Value::as_u64).unwrap_or(0);
                let name = raw_doc.get("name").and_then(Value::as_str).unwrap_or(id);
                let content: Value = serde_json::from_str(
                    raw_doc
                        .get("content")
                        .and_then(Value::as_str)
                        .unwrap_or("{}"),
                )
                .map_err(|_| "推送记录 content 解析失败".to_string())?;
                let resource_type = content
                    .get("format")
                    .and_then(Value::as_str)
                    .unwrap_or("pdf");
                put_push_message_doc_with_endpoints(
                    endpoints,
                    client,
                    auth,
                    name,
                    size,
                    resource_type,
                    &object_key,
                    &signed_url,
                )
                .map(|_| ())
                .map_err(ResendError::AfterSubmission)
            }
            Err(error) => Err(ResendError::AfterSubmission(error)),
        }
    } else {
        result.map(|_| ()).map_err(ResendError::AfterSubmission)
    }
}

fn is_not_found_push_error(error: &str) -> bool {
    error
        .strip_prefix("HTTP ")
        .and_then(|rest| rest.split_whitespace().next())
        == Some("404")
}

pub fn dashboard_push_delete_inner(doc_id: &str) -> Result<(), String> {
    let endpoints = &crate::api::ApiEndpoints::production();
    if doc_id.trim().is_empty() {
        return Err("删除记录 id 不能为空".to_string());
    }
    let client = create_client(60)?;
    let (auth, _) = fetch_auth_context_and_user_with_endpoints(endpoints, &client)?;
    let _ = crate::api::delete_push_doc_with_endpoints(endpoints, &client, &auth, doc_id);
    batch_delete_push_message_with_endpoints(endpoints, &client, &auth, doc_id)?;
    Ok(())
}

pub fn dashboard_push_resend(id: String) -> Result<DashboardSnapshot, String> {
    dashboard_push_resend_inner(&id)?;
    let snapshot = build_dashboard_snapshot();
    set_dashboard_cache(snapshot.clone());
    Ok(snapshot)
}

pub fn dashboard_push_delete(id: String) -> Result<DashboardSnapshot, String> {
    dashboard_push_delete_inner(&id)?;
    if let Some(snapshot) = update_dashboard_cache_after_delete(&id) {
        return Ok(snapshot);
    }
    let snapshot = build_dashboard_snapshot();
    set_dashboard_cache(snapshot.clone());
    Ok(snapshot)
}

pub fn dashboard_open_transfer_host<F>(host: String, open_url: F) -> Result<(), String>
where
    F: Fn(&str) -> Result<(), String>,
{
    let normalized =
        normalize_transfer_host_url(&host).ok_or_else(|| "设备互传地址无效".to_string())?;
    let parsed = url::Url::parse(&normalized).map_err(|err| err.to_string())?;
    let host_name = parsed
        .host_str()
        .ok_or_else(|| "设备互传地址缺少主机名".to_string())?;
    if !is_local_transfer_host(host_name) {
        return Err("仅允许打开局域网 BOOX 设备地址".to_string());
    }
    open_url(&normalized)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn build_object_key_matches_web_upload_prefix() {
        let (key, ext) = build_object_key("user-123", Path::new("/tmp/test.pdf"));
        assert!(key.starts_with("user-123/push/"));
        assert!(key.ends_with(".pdf"));
        assert_eq!(ext, "pdf");
    }

    #[test]
    fn build_oss_host_formats_bucket_and_endpoint() {
        let bucket = BucketConfig {
            bucket: Some("onyx-cloud".to_string()),
            ali_endpoint: Some("https://oss-cn-shanghai.aliyuncs.com".to_string()),
            ..BucketConfig::default()
        };
        let (name, host) = build_oss_host(&bucket).unwrap();
        assert_eq!(name, "onyx-cloud");
        assert_eq!(host, "onyx-cloud.oss-cn-shanghai.aliyuncs.com");
    }

    #[test]
    fn update_push_doc_for_resend_at_updates_content_and_updated_at() {
        let mut doc = json!({
            "content": "{\"formats\":[\"epub\"],\"url\":\"old-url\"}",
            "updatedAt": 100
        });
        update_push_doc_for_resend_at(&mut doc, "https://example.com/new.epub", 200).unwrap();
        assert_eq!(doc["updatedAt"], 200);
        let parsed: Value = serde_json::from_str(doc["content"].as_str().unwrap()).unwrap();
        assert_eq!(parsed["url"], "https://example.com/new.epub");
        assert_eq!(parsed["format"], "epub");
        assert_eq!(parsed["updatedAt"], 200);
    }
}
