//! Cancellable queue reads and mutations.
//!
//! Queue requests share the legacy API helpers' URL construction, sync-token
//! parsing, neocloud cookie selection, and response parsers. The request
//! futures are polled on an owned runtime so SIGINT can drop a stalled
//! connection. Each new network request checks cancellation first.

use crate::api::{
    batch_delete_push_message_body, batch_delete_push_message_url, neocloud_cookie_header,
    parse_envelope_with_status, parse_existing_push_doc_write_response, parse_json_document,
    parse_push_item_doc, parse_push_queue_response, push_doc_delete_url, push_doc_url,
    queue_changes_url, ApiEndpoints, ApiError,
};
use crate::cancellable_push::{
    block_on_cancellable, check_before_request, client, runtime, sync_token, with_bearer,
    RequestError,
};
use crate::models::{DashboardPushItem, UploadAuthContext};
use crate::push::{
    is_not_found_push_error, resend_fallback_resource_type, update_push_doc_for_resend_at,
    ResendError,
};
use reqwest::header::{ACCEPT, CONTENT_TYPE, COOKIE};
use serde_json::Value;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

#[derive(Debug)]
pub(crate) enum MutationError {
    Cancelled,
    Api(ApiError),
    Unknown(ApiError),
}

async fn queue_window_request(
    client: &reqwest::Client,
    endpoints: &ApiEndpoints,
    auth: &UploadAuthContext,
    flag: Option<&'static AtomicBool>,
) -> Result<Vec<DashboardPushItem>, RequestError> {
    check_before_request(flag)?;
    let (sync, set_cookies) = sync_token(client, endpoints, auth).await?;
    let url = queue_changes_url(endpoints, &auth.uid);
    let cookie = neocloud_cookie_header(endpoints, &sync, &set_cookies, &url)?;
    check_before_request(flag)?;
    let response = client.get(url).header(COOKIE, cookie).send().await?;
    let status = response.status();
    if !status.is_success() {
        return Err(RequestError::Api(ApiError::http(status.as_u16())));
    }
    let text = response.text().await?;
    // A completed read is not a successful result once cancellation was
    // observed; it must not look like a healthy queue to the caller.
    check_before_request(flag)?;
    parse_push_queue_response(&text).map_err(RequestError::Api)
}

async fn fetch_doc_request(
    client: &reqwest::Client,
    endpoints: &ApiEndpoints,
    auth: &UploadAuthContext,
    flag: Option<&'static AtomicBool>,
    doc_id: &str,
) -> Result<Value, RequestError> {
    check_before_request(flag)?;
    let (sync, set_cookies) = sync_token(client, endpoints, auth).await?;
    let url = push_doc_url(endpoints, doc_id);
    let cookie = neocloud_cookie_header(endpoints, &sync, &set_cookies, &url)?;
    check_before_request(flag)?;
    let response = client.get(url).header(COOKIE, cookie).send().await?;
    let status = response.status();
    if !status.is_success() {
        return Err(RequestError::Api(ApiError::http(status.as_u16())));
    }
    let text = response.text().await?;
    check_before_request(flag)?;
    parse_json_document(&text).map_err(RequestError::Api)
}

#[allow(clippy::too_many_arguments)]
async fn put_existing_doc_request(
    client: &reqwest::Client,
    endpoints: &ApiEndpoints,
    auth: &UploadAuthContext,
    flag: Option<&'static AtomicBool>,
    doc: &Value,
    write_started: &AtomicBool,
) -> Result<(String, String), RequestError> {
    check_before_request(flag)?;
    let doc_id = crate::util::json_field_to_string(doc, "_id")
        .ok_or_else(|| RequestError::Api(ApiError::protocol("推送消息缺少 _id")))?;
    let doc_rev = crate::util::json_field_to_string(doc, "_rev")
        .ok_or_else(|| RequestError::Api(ApiError::protocol("推送消息缺少 _rev")))?;
    let (sync, set_cookies) = sync_token(client, endpoints, auth).await?;
    let url = push_doc_url(endpoints, &doc_id);
    let cookie = neocloud_cookie_header(endpoints, &sync, &set_cookies, &url)?;
    check_before_request(flag)?;
    write_started.store(true, Ordering::Relaxed);
    let response = client
        .put(url)
        .header(COOKIE, cookie)
        .header(ACCEPT, "application/json")
        .header(CONTENT_TYPE, "application/json")
        .body(doc.to_string())
        .send()
        .await?;
    let status = response.status();
    if !status.is_success() {
        return Err(RequestError::Api(ApiError::http(status.as_u16())));
    }
    let text = response.text().await?;
    let value = parse_json_document(&text)?;
    parse_existing_push_doc_write_response(&value, doc_id, doc_rev).map_err(RequestError::Api)
}

async fn delete_doc_request(
    client: &reqwest::Client,
    endpoints: &ApiEndpoints,
    auth: &UploadAuthContext,
    flag: Option<&'static AtomicBool>,
    doc_id: &str,
    write_started: &AtomicBool,
) -> Result<(), RequestError> {
    // The lookup is a separate read-only request. Cancellation after it must
    // stop before the DELETE starts.
    let doc = fetch_doc_request(client, endpoints, auth, flag, doc_id).await?;
    let item = parse_push_item_doc(&doc)
        .ok_or_else(|| RequestError::Api(ApiError::protocol("推送记录格式不完整")))?;
    let rev = item.rev.unwrap_or_default();
    check_before_request(flag)?;
    let (sync, set_cookies) = sync_token(client, endpoints, auth).await?;
    let url = push_doc_delete_url(endpoints, doc_id, &rev);
    let cookie = neocloud_cookie_header(endpoints, &sync, &set_cookies, &url)?;
    check_before_request(flag)?;
    write_started.store(true, Ordering::Relaxed);
    let response = client.delete(url).header(COOKIE, cookie).send().await?;
    let status = response.status();
    if !status.is_success() {
        return Err(RequestError::Api(ApiError::http(status.as_u16())));
    }
    Ok(())
}

async fn batch_delete_message_request(
    client: &reqwest::Client,
    endpoints: &ApiEndpoints,
    auth: &UploadAuthContext,
    flag: Option<&'static AtomicBool>,
    doc_id: &str,
    write_started: &AtomicBool,
) -> Result<(), RequestError> {
    check_before_request(flag)?;
    let body = batch_delete_push_message_body(doc_id);
    write_started.store(true, Ordering::Relaxed);
    let response = with_bearer(
        client
            .post(batch_delete_push_message_url(endpoints))
            .header(CONTENT_TYPE, "application/json"),
        &auth.bearer,
    )
    .body(body.to_string())
    .send()
    .await?;
    let status = response.status();
    if !status.is_success() {
        return Err(RequestError::Api(ApiError::http(status.as_u16())));
    }
    let text = response.text().await?;
    let _: Value = parse_envelope_with_status(&text, Some(status.as_u16()))?;
    Ok(())
}

async fn resend_request(
    client: &reqwest::Client,
    endpoints: &ApiEndpoints,
    auth: &UploadAuthContext,
    flag: Option<&'static AtomicBool>,
    id: &str,
    write_started: &AtomicBool,
) -> Result<(), ResendError> {
    check_before_request(flag).map_err(|_| ResendError::Cancelled)?;
    let (_bucket_key, bucket) =
        crate::cancellable_prepare::default_bucket_request(client, endpoints, flag)
            .await
            .map_err(|error| match error {
                RequestError::Cancelled => ResendError::Cancelled,
                RequestError::Api(error) => ResendError::BeforeSubmission(error),
            })?;
    let mut raw_doc = fetch_doc_request(client, endpoints, auth, flag, id)
        .await
        .map_err(|error| match error {
            RequestError::Cancelled => ResendError::Cancelled,
            RequestError::Api(error) => ResendError::BeforeSubmission(error),
        })?;
    let object_key = raw_doc
        .get("storageKey")
        .and_then(Value::as_str)
        .filter(|key| !key.trim().is_empty())
        .ok_or_else(|| {
            ResendError::BeforeSubmission(ApiError::protocol("推送记录缺少有效的 storageKey"))
        })?
        .to_string();
    check_before_request(flag).map_err(|_| ResendError::Cancelled)?;
    let signed_url = crate::cancellable_prepare::signed_url_request(
        client,
        endpoints,
        auth,
        &bucket,
        &object_key,
        flag,
    )
    .await
    .map_err(|error| match error {
        RequestError::Cancelled => ResendError::Cancelled,
        RequestError::Api(error) => ResendError::BeforeSubmission(error),
    })?;
    update_push_doc_for_resend_at(&mut raw_doc, &signed_url, crate::util::unix_ms_now() as u64)
        .map_err(|_| {
            ResendError::BeforeSubmission(ApiError::protocol("推送记录无法准备为可提交的消息"))
        })?;
    let has_parent = raw_doc
        .get("parent")
        .and_then(Value::as_str)
        .is_some_and(|parent| !parent.trim().is_empty());
    // This is the write boundary: cancellation before this check cannot have
    // affected the remote queue. Once the request starts, a lost response is
    // ambiguous and must be reported as unknown without a blind retry.
    check_before_request(flag).map_err(|_| ResendError::Cancelled)?;
    let result =
        put_existing_doc_request(client, endpoints, auth, flag, &raw_doc, write_started).await;
    let result = result.map_err(|error| map_resend_request_error(error, write_started));
    if has_parent {
        match result {
            Ok((_, rev)) => {
                let rev = rev.to_string();
                crate::cancellable_push::confirm_sync_request(
                    client, endpoints, auth, flag, id, &rev,
                )
                .await
                .map_err(|error| ResendError::AfterSubmission(after_submission_error(error)))
            }
            Err(error) if resend_not_found(&error) => {
                // The explicit 404 is a confirmed rejection of the parent PUT.
                // The fallback has not written anything yet.
                write_started.store(false, Ordering::Relaxed);
                let size = raw_doc.get("size").and_then(Value::as_u64).unwrap_or(0);
                let name = raw_doc.get("name").and_then(Value::as_str).unwrap_or(id);
                let content: Value = serde_json::from_str(
                    raw_doc
                        .get("content")
                        .and_then(Value::as_str)
                        .unwrap_or("{}"),
                )
                .map_err(|_| {
                    ResendError::AfterSubmission(ApiError::protocol("推送记录 content 解析失败"))
                })?;
                let resource_type = resend_fallback_resource_type(&content);
                crate::cancellable_push::write_message_request(
                    client,
                    endpoints,
                    auth,
                    flag,
                    name,
                    size,
                    resource_type,
                    &object_key,
                    &signed_url,
                    Some(write_started),
                )
                .await
                .map(|_| ())
                .map_err(|error| match error {
                    RequestError::Cancelled if write_started.load(Ordering::Relaxed) => {
                        ResendError::AfterSubmission(ApiError::protocol(
                            "重推请求可能已提交但未收到确认",
                        ))
                    }
                    RequestError::Cancelled => ResendError::Cancelled,
                    RequestError::Api(error) => ResendError::AfterSubmission(error),
                })
            }
            Err(error) => Err(error),
        }
    } else {
        result.map(|_| ())
    }
}

fn after_submission_error(error: RequestError) -> ApiError {
    match error {
        RequestError::Cancelled => ApiError::protocol("重推已提交但同步确认被取消，结果未知"),
        RequestError::Api(error) => error,
    }
}

fn resend_not_found(error: &ResendError) -> bool {
    match error {
        // A 404 before the PUT is a preparation failure, not proof that the
        // submission was rejected; only an explicit submit response permits
        // the legacy parent-record fallback.
        ResendError::AfterSubmission(error) => is_not_found_push_error(error),
        ResendError::BeforeSubmission(_) | ResendError::Cancelled => false,
    }
}

fn map_resend_request_error(error: RequestError, write_started: &AtomicBool) -> ResendError {
    match error {
        RequestError::Cancelled if write_started.load(Ordering::Relaxed) => {
            ResendError::AfterSubmission(ApiError::protocol("重推请求可能已提交但未收到确认"))
        }
        RequestError::Cancelled => ResendError::Cancelled,
        RequestError::Api(error) if write_started.load(Ordering::Relaxed) => {
            ResendError::AfterSubmission(error)
        }
        RequestError::Api(error) => ResendError::BeforeSubmission(error),
    }
}

pub(crate) fn queue_window(
    timeout: Duration,
    endpoints: &ApiEndpoints,
    auth: &UploadAuthContext,
    flag: Option<&'static AtomicBool>,
) -> Result<Vec<DashboardPushItem>, RequestError> {
    check_before_request(flag)?;
    let runtime = runtime()?;
    let client = client(timeout)?;
    block_on_cancellable(
        &runtime,
        flag,
        queue_window_request(&client, endpoints, auth, flag),
    )?
}

pub(crate) fn resend(
    timeout: Duration,
    endpoints: &ApiEndpoints,
    auth: &UploadAuthContext,
    flag: Option<&'static AtomicBool>,
    id: &str,
) -> Result<(), ResendError> {
    check_before_request(flag).map_err(|_| ResendError::Cancelled)?;
    let runtime = runtime().map_err(|error| match error {
        RequestError::Cancelled => ResendError::Cancelled,
        RequestError::Api(error) => ResendError::BeforeSubmission(error),
    })?;
    let client = client(timeout).map_err(|error| match error {
        RequestError::Cancelled => ResendError::Cancelled,
        RequestError::Api(error) => ResendError::BeforeSubmission(error),
    })?;
    // The asynchronous request marks writes after its final pre-write check.
    // The wrapper uses a separate marker so a runner-level cancellation during
    // the drain window cannot be mislabeled as a pre-write stop.
    let write_started = AtomicBool::new(false);
    let result = block_on_cancellable(
        &runtime,
        flag,
        resend_request(&client, endpoints, auth, flag, id, &write_started),
    );
    match result {
        Ok(Ok(())) => Ok(()),
        Ok(Err(error)) => Err(error),
        Err(error) => Err(map_resend_request_error(error, &write_started)),
    }
}

pub(crate) fn delete_push_doc(
    timeout: Duration,
    endpoints: &ApiEndpoints,
    auth: &UploadAuthContext,
    flag: Option<&'static AtomicBool>,
    id: &str,
) -> Result<(), MutationError> {
    check_before_request(flag).map_err(|_| MutationError::Cancelled)?;
    let runtime = runtime().map_err(map_mutation_request_error)?;
    let client = client(timeout).map_err(map_mutation_request_error)?;
    let write_started = AtomicBool::new(false);
    let result = block_on_cancellable(
        &runtime,
        flag,
        delete_doc_request(&client, endpoints, auth, flag, id, &write_started),
    );
    flatten_delete_result(result, &write_started)
}

pub(crate) fn delete_message_record(
    timeout: Duration,
    endpoints: &ApiEndpoints,
    auth: &UploadAuthContext,
    flag: Option<&'static AtomicBool>,
    id: &str,
) -> Result<(), MutationError> {
    check_before_request(flag).map_err(|_| MutationError::Cancelled)?;
    let runtime = runtime().map_err(map_mutation_request_error)?;
    let client = client(timeout).map_err(map_mutation_request_error)?;
    let write_started = AtomicBool::new(false);
    let result = block_on_cancellable(
        &runtime,
        flag,
        batch_delete_message_request(&client, endpoints, auth, flag, id, &write_started),
    );
    flatten_delete_result(result, &write_started)
}

fn map_mutation_request_error(error: RequestError) -> MutationError {
    match error {
        RequestError::Cancelled => MutationError::Cancelled,
        RequestError::Api(error) => MutationError::Api(error),
    }
}

fn flatten_delete_result(
    result: Result<Result<(), RequestError>, RequestError>,
    write_started: &AtomicBool,
) -> Result<(), MutationError> {
    let started = write_started.load(Ordering::Relaxed);
    match result {
        Ok(Ok(())) => Ok(()),
        Ok(Err(RequestError::Cancelled)) if started => Err(MutationError::Unknown(
            ApiError::protocol("队列修改请求可能已提交但未收到确认"),
        )),
        Ok(Err(RequestError::Cancelled)) => Err(MutationError::Cancelled),
        Ok(Err(RequestError::Api(error))) if started && is_ambiguous(&error) => {
            Err(MutationError::Unknown(error))
        }
        Ok(Err(RequestError::Api(error))) => Err(MutationError::Api(error)),
        Err(RequestError::Cancelled) if started => Err(MutationError::Unknown(ApiError::protocol(
            "队列修改请求可能已提交但未收到确认",
        ))),
        Err(RequestError::Cancelled) => Err(MutationError::Cancelled),
        Err(RequestError::Api(error)) if started && is_ambiguous(&error) => {
            Err(MutationError::Unknown(error))
        }
        Err(RequestError::Api(error)) => Err(MutationError::Api(error)),
    }
}

fn is_ambiguous(error: &ApiError) -> bool {
    matches!(
        error.kind,
        crate::api::ApiErrorKind::Network
            | crate::api::ApiErrorKind::Timeout
            | crate::api::ApiErrorKind::Protocol
    )
}
