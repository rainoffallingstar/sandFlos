//! Cancellable read-only requests that prepare an upload.
//!
//! Account, storage, bucket, STS, and signed-URL verification requests do not
//! commit remote state. They share the blocking API helpers' URL construction
//! and response parsing, but poll the request future so SIGINT can drop the
//! connection instead of waiting for the user-configured request timeout.

use crate::api::{
    apply_storage_quota, buckets_url, parse_auth_identity, parse_buckets,
    parse_envelope_with_status, parse_oss_sts, select_default_bucket, storage_url, sts_url,
    user_me_url, ApiEndpoints, ApiError, ApiResult,
};
use crate::cancellable_push::{
    block_on_cancellable, check_before_request, client, runtime, with_bearer, RequestError,
};
use crate::models::{BucketConfig, OssSts, UploadAuthContext};
use crate::push::{
    signed_download_url_with_endpoints, sts_remaining_seconds,
    MIN_STS_REMAINING_SECS_FOR_SIGNED_URL,
};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

const SIGNED_URL_ATTEMPTS: usize = 3;

pub(crate) async fn get_json(
    client: &reqwest::Client,
    url: &str,
    bearer: Option<&str>,
) -> ApiResult<Value> {
    let request = match bearer {
        Some(bearer) => with_bearer(client.get(url), bearer),
        None => client.get(url),
    };
    envelope_json(request.send().await?).await
}

/// Parse the status-first API envelope from an in-flight request. Query
/// helpers add their query parameters to the passed request first.
pub(crate) async fn envelope_json(response: reqwest::Response) -> ApiResult<Value> {
    let status = response.status();
    if !status.is_success() {
        return Err(ApiError::http(status.as_u16()));
    }
    let text = response.text().await?;
    parse_envelope_with_status(&text, Some(status.as_u16()))
}

async fn auth_context_request(
    client: &reqwest::Client,
    endpoints: &ApiEndpoints,
    credential: &str,
    flag: Option<&'static AtomicBool>,
) -> Result<UploadAuthContext, RequestError> {
    check_before_request(flag)?;
    let credential = credential.trim();
    if credential.is_empty() {
        return Err(RequestError::Api(ApiError::authentication(
            "认证凭据不能为空",
        )));
    }
    let base = endpoints.api_base();
    let user = get_json(client, &user_me_url(base), Some(credential)).await?;
    // The storage request is a separate network step: a signal after the
    // account response must stop here after preserving the parsed identity.
    check_before_request(flag)?;
    let mut auth = parse_auth_identity(&user, credential.to_string())?;
    let storage = get_json(client, &storage_url(base), Some(&auth.bearer)).await?;
    apply_storage_quota(&mut auth, &storage);
    Ok(auth)
}

pub(crate) async fn default_bucket_request(
    client: &reqwest::Client,
    endpoints: &ApiEndpoints,
    flag: Option<&'static AtomicBool>,
) -> Result<(String, BucketConfig), RequestError> {
    check_before_request(flag)?;
    let data = get_json(client, &buckets_url(endpoints.api_base()), None).await?;
    let buckets: HashMap<String, BucketConfig> = parse_buckets(data)?;
    Ok(select_default_bucket(buckets)?)
}

async fn sts_request(
    client: &reqwest::Client,
    endpoints: &ApiEndpoints,
    auth: &UploadAuthContext,
    flag: Option<&'static AtomicBool>,
) -> Result<OssSts, RequestError> {
    check_before_request(flag)?;
    let data = get_json(client, &sts_url(endpoints.api_base()), Some(&auth.bearer)).await?;
    Ok(parse_oss_sts(data)?)
}

async fn verify_range(
    client: &reqwest::Client,
    signed_url: &str,
    flag: Option<&'static AtomicBool>,
) -> Result<(), RequestError> {
    check_before_request(flag)?;
    let response = client
        .get(signed_url)
        .header("Range", "bytes=0-0")
        .send()
        .await?;
    let status = response.status();
    if status.as_u16() == 200 || status.as_u16() == 206 {
        Ok(())
    } else {
        Err(RequestError::Api(ApiError::http(status.as_u16())))
    }
}

pub(crate) async fn signed_url_request(
    client: &reqwest::Client,
    endpoints: &ApiEndpoints,
    auth: &UploadAuthContext,
    bucket: &BucketConfig,
    object_key: &str,
    flag: Option<&'static AtomicBool>,
) -> Result<String, RequestError> {
    let mut last_error = None;
    for attempt in 0..SIGNED_URL_ATTEMPTS {
        // A cancelled verification must not issue a fresh STS or object
        // request on the next read-only retry.
        check_before_request(flag)?;
        let sts = sts_request(client, endpoints, auth, flag).await?;
        let remaining = sts_remaining_seconds(&sts);
        if remaining
            .map(|value| value <= MIN_STS_REMAINING_SECS_FOR_SIGNED_URL)
            .unwrap_or(false)
        {
            last_error = Some(ApiError::protocol(format!(
                "STS 剩余有效期过短: {}s",
                remaining.unwrap_or_default()
            )));
        } else {
            let signed_url =
                match signed_download_url_with_endpoints(endpoints, bucket, &sts, object_key) {
                    Ok(url) => url,
                    Err(error) => {
                        last_error = Some(ApiError::protocol(error));
                        if attempt + 1 < SIGNED_URL_ATTEMPTS {
                            tokio::time::sleep(Duration::from_millis(400 * (attempt as u64 + 1)))
                                .await;
                        }
                        continue;
                    }
                };
            match verify_range(client, &signed_url, flag).await {
                Ok(()) => return Ok(signed_url),
                Err(RequestError::Cancelled) => return Err(RequestError::Cancelled),
                Err(RequestError::Api(error)) => last_error = Some(error),
            }
        }
        if attempt + 1 < SIGNED_URL_ATTEMPTS {
            tokio::time::sleep(Duration::from_millis(400 * (attempt as u64 + 1))).await;
        }
    }
    Err(RequestError::Api(last_error.unwrap_or_else(|| {
        ApiError::protocol("生成可用的签名下载链接失败")
    })))
}

pub(crate) fn auth_context(
    timeout: Duration,
    endpoints: &ApiEndpoints,
    credential: &str,
    flag: Option<&'static AtomicBool>,
) -> Result<UploadAuthContext, RequestError> {
    check_before_request(flag)?;
    let runtime = runtime()?;
    let client = client(timeout)?;
    block_on_cancellable(
        &runtime,
        flag,
        auth_context_request(&client, endpoints, credential, flag),
    )?
}

pub(crate) fn default_bucket(
    timeout: Duration,
    endpoints: &ApiEndpoints,
    flag: Option<&'static AtomicBool>,
) -> Result<(String, BucketConfig), RequestError> {
    check_before_request(flag)?;
    let runtime = runtime()?;
    let client = client(timeout)?;
    block_on_cancellable(
        &runtime,
        flag,
        default_bucket_request(&client, endpoints, flag),
    )?
}

pub(crate) fn sts(
    timeout: Duration,
    endpoints: &ApiEndpoints,
    auth: &UploadAuthContext,
    flag: Option<&'static AtomicBool>,
) -> Result<OssSts, RequestError> {
    check_before_request(flag)?;
    let runtime = runtime()?;
    let client = client(timeout)?;
    block_on_cancellable(&runtime, flag, sts_request(&client, endpoints, auth, flag))?
}

pub(crate) fn signed_url(
    timeout: Duration,
    endpoints: &ApiEndpoints,
    auth: &UploadAuthContext,
    bucket: &BucketConfig,
    object_key: &str,
    flag: Option<&'static AtomicBool>,
) -> Result<String, RequestError> {
    check_before_request(flag)?;
    let runtime = runtime()?;
    let client = client(timeout)?;
    block_on_cancellable(
        &runtime,
        flag,
        signed_url_request(&client, endpoints, auth, bucket, object_key, flag),
    )?
}
