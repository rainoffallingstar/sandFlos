//! Cancellable async transport for the post-upload push phases.
//!
//! The blocking API helpers remain the compatibility path. This module builds
//! the same request documents and parses their responses with the same rules,
//! but polls the in-flight request future on an owned runtime so cancellation
//! can drop the connection instead of waiting for the user's request timeout.

use crate::api::{
    neocloud_cookie_header, parse_envelope_with_status, parse_push_item_doc,
    parse_push_message_write_response, push_doc_url, push_message_doc, save_and_push_body,
    save_and_push_url, sync_token_url, ApiEndpoints, ApiError, ApiResult,
};
use crate::models::{SyncToken, UploadAuthContext};
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, COOKIE, SET_COOKIE};
use serde_json::Value;
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

const CANCEL_POLL_INTERVAL: Duration = Duration::from_millis(25);
// Once cancellation is observed, give an in-flight response a short bounded
// chance to finish. A confirmed submission must remain accepted even when the
// signal arrives as the server writes its success response.
const CANCEL_DRAIN_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Debug)]
pub(crate) enum RequestError {
    Cancelled,
    Api(ApiError),
}

impl From<ApiError> for RequestError {
    fn from(error: ApiError) -> Self {
        Self::Api(error)
    }
}

impl From<reqwest::Error> for RequestError {
    fn from(error: reqwest::Error) -> Self {
        Self::Api(ApiError::from(error))
    }
}

pub(crate) fn runtime() -> Result<tokio::runtime::Runtime, RequestError> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| RequestError::Api(ApiError::network("无法启动请求运行时")))
}

pub(crate) fn client(timeout: Duration) -> Result<reqwest::Client, RequestError> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(timeout)
        .build()
        .map_err(|_| RequestError::Api(ApiError::network("无法创建请求客户端")))
}

fn cancelled(flag: Option<&'static AtomicBool>) -> bool {
    flag.is_some_and(|value| value.load(Ordering::Relaxed))
}

/// Reject a new request attempt once cancellation has been observed. A call
/// that has already sent its request may still finish inside the bounded drain
/// window and preserve a confirmed response.
pub(crate) fn check_before_request(flag: Option<&'static AtomicBool>) -> Result<(), RequestError> {
    if cancelled(flag) {
        Err(RequestError::Cancelled)
    } else {
        Ok(())
    }
}

/// Poll a request future in short slices. When cancellation is observed the
/// future is dropped here, closing its connection while the owned runtime is
/// still alive; no blocking worker or detached uploader outlives the call.
pub(crate) fn block_on_cancellable<T, F>(
    runtime: &tokio::runtime::Runtime,
    flag: Option<&'static AtomicBool>,
    future: F,
) -> Result<T, RequestError>
where
    F: Future<Output = T>,
{
    runtime.block_on(async move {
        tokio::pin!(future);
        loop {
            match tokio::time::timeout(CANCEL_POLL_INTERVAL, &mut future).await {
                Ok(value) => return Ok(value),
                Err(_) if cancelled(flag) => {
                    return match tokio::time::timeout(CANCEL_DRAIN_INTERVAL, &mut future).await {
                        Ok(value) => Ok(value),
                        Err(_) => Err(RequestError::Cancelled),
                    };
                }
                Err(_) => {}
            }
        }
    })
}

pub(crate) fn with_bearer(
    mut request: reqwest::RequestBuilder,
    bearer: &str,
) -> reqwest::RequestBuilder {
    if !bearer.trim().is_empty() {
        request = request.header(AUTHORIZATION, format!("Bearer {bearer}"));
    }
    request
}

pub(crate) async fn sync_token(
    client: &reqwest::Client,
    endpoints: &ApiEndpoints,
    auth: &UploadAuthContext,
) -> ApiResult<(SyncToken, Vec<String>)> {
    let response = with_bearer(client.get(sync_token_url(endpoints)), &auth.bearer)
        .send()
        .await?;
    let headers = response.headers().clone();
    let status = response.status();
    if !status.is_success() {
        return Err(ApiError::http(status.as_u16()));
    }
    let text = response.text().await?;
    let data = parse_envelope_with_status::<Value>(&text, Some(status.as_u16()))?;
    let sync: SyncToken = serde_json::from_value(data)
        .map_err(|_| ApiError::protocol("服务端返回了无法识别的响应"))?;
    let set_cookies = headers
        .get_all(SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok().map(ToString::to_string))
        .collect();
    Ok((sync, set_cookies))
}

async fn parse_json_response(response: reqwest::Response) -> ApiResult<Value> {
    let status = response.status();
    if !status.is_success() {
        return Err(ApiError::http(status.as_u16()));
    }
    let text = response.text().await?;
    serde_json::from_str(&text).map_err(|_| ApiError::protocol("服务端返回了无法识别的响应"))
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn write_message_request(
    client: &reqwest::Client,
    endpoints: &ApiEndpoints,
    auth: &UploadAuthContext,
    flag: Option<&'static AtomicBool>,
    file_name: &str,
    size: u64,
    resource_type: &str,
    object_key: &str,
    signed_url: &str,
    write_started: Option<&AtomicBool>,
) -> Result<(String, String), RequestError> {
    check_before_request(flag)?;
    let (sync, set_cookies) = sync_token(client, endpoints, auth).await?;
    let (message_id, message_doc) =
        push_message_doc(auth, file_name, size, resource_type, object_key, signed_url);
    let url = push_doc_url(endpoints, &message_id);
    let cookie = neocloud_cookie_header(endpoints, &sync, &set_cookies, &url)?;
    check_before_request(flag)?;
    if let Some(write_started) = write_started {
        write_started.store(true, Ordering::Relaxed);
    }
    let response = client
        .put(url)
        .header(COOKIE, cookie)
        .header(ACCEPT, "application/json")
        .header(CONTENT_TYPE, "application/json")
        .body(message_doc.to_string())
        .send()
        .await?;
    let value = parse_json_response(response).await?;
    parse_push_message_write_response(&value, message_id).map_err(RequestError::Api)
}

async fn fetch_push_doc_for_sync(
    client: &reqwest::Client,
    endpoints: &ApiEndpoints,
    auth: &UploadAuthContext,
    flag: Option<&'static AtomicBool>,
    doc_id: &str,
    expected_rev: &str,
) -> Result<(), RequestError> {
    check_before_request(flag)?;
    let (sync, set_cookies) = sync_token(client, endpoints, auth).await?;
    let url = push_doc_url(endpoints, doc_id);
    let cookie = neocloud_cookie_header(endpoints, &sync, &set_cookies, &url)?;
    check_before_request(flag)?;
    let response = client.get(url).header(COOKIE, cookie).send().await?;
    let doc = parse_json_response(response).await?;
    match parse_push_item_doc(&doc) {
        Some(item) if item.rev.as_deref() == Some(expected_rev) => Ok(()),
        Some(_) => Err(RequestError::Api(ApiError::protocol(
            "推送消息版本尚未同步到预期版本",
        ))),
        None => Err(RequestError::Api(ApiError::protocol("推送记录格式不完整"))),
    }
}

pub(crate) async fn confirm_sync_request(
    client: &reqwest::Client,
    endpoints: &ApiEndpoints,
    auth: &UploadAuthContext,
    flag: Option<&'static AtomicBool>,
    doc_id: &str,
    expected_rev: &str,
) -> Result<(), RequestError> {
    let mut last_error = None;
    for attempt in 0..8 {
        if cancelled(flag) {
            return Err(RequestError::Cancelled);
        }
        match fetch_push_doc_for_sync(client, endpoints, auth, flag, doc_id, expected_rev).await {
            Ok(()) => return Ok(()),
            Err(RequestError::Cancelled) => return Err(RequestError::Cancelled),
            Err(RequestError::Api(error)) => last_error = Some(error),
        }
        if cancelled(flag) {
            return Err(RequestError::Cancelled);
        }
        if attempt < 7 {
            tokio::time::sleep(Duration::from_millis(250 * (attempt as u64 + 1))).await;
        }
    }
    Err(RequestError::Api(last_error.unwrap_or_else(|| {
        ApiError::protocol("推送消息未能及时同步到 neocloud")
    })))
}

#[allow(clippy::too_many_arguments)]
async fn save_and_push_request(
    client: &reqwest::Client,
    endpoints: &ApiEndpoints,
    auth: &UploadAuthContext,
    flag: Option<&'static AtomicBool>,
    bucket_key: &str,
    object_key: &str,
    file_name: &str,
    resource_type: &str,
    doc_id: &str,
    rev: &str,
) -> Result<(), RequestError> {
    check_before_request(flag)?;
    let body = save_and_push_body(
        bucket_key,
        object_key,
        file_name,
        resource_type,
        doc_id,
        rev,
    );
    let response = with_bearer(
        client
            .post(save_and_push_url(endpoints))
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

#[allow(clippy::too_many_arguments)]
pub(crate) fn write_message(
    timeout: Duration,
    endpoints: &ApiEndpoints,
    auth: &UploadAuthContext,
    flag: Option<&'static AtomicBool>,
    file_name: &str,
    size: u64,
    resource_type: &str,
    object_key: &str,
    signed_url: &str,
) -> Result<(String, String), RequestError> {
    check_before_request(flag)?;
    let runtime = runtime()?;
    let client = client(timeout)?;
    block_on_cancellable(
        &runtime,
        flag,
        write_message_request(
            &client,
            endpoints,
            auth,
            flag,
            file_name,
            size,
            resource_type,
            object_key,
            signed_url,
            None,
        ),
    )?
}

pub(crate) fn confirm_sync(
    timeout: Duration,
    endpoints: &ApiEndpoints,
    auth: &UploadAuthContext,
    flag: Option<&'static AtomicBool>,
    doc_id: &str,
    rev: &str,
) -> Result<(), RequestError> {
    check_before_request(flag)?;
    let runtime = runtime()?;
    let client = client(timeout)?;
    block_on_cancellable(
        &runtime,
        flag,
        confirm_sync_request(&client, endpoints, auth, flag, doc_id, rev),
    )?
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn save_and_push(
    timeout: Duration,
    endpoints: &ApiEndpoints,
    auth: &UploadAuthContext,
    flag: Option<&'static AtomicBool>,
    bucket_key: &str,
    object_key: &str,
    file_name: &str,
    resource_type: &str,
    doc_id: &str,
    rev: &str,
) -> Result<(), RequestError> {
    check_before_request(flag)?;
    let runtime = runtime()?;
    let client = client(timeout)?;
    block_on_cancellable(
        &runtime,
        flag,
        save_and_push_request(
            &client,
            endpoints,
            auth,
            flag,
            bucket_key,
            object_key,
            file_name,
            resource_type,
            doc_id,
            rev,
        ),
    )?
}
