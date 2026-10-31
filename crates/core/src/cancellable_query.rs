//! Cancellable read-only account queries.
//!
//! These requests do not mutate remote state. They reuse the blocking API
//! helpers' URL construction, query parameters, and envelope parsing, but poll
//! the request future so SIGINT drops the connection promptly. Each network
//! step checks the same cancellation flag first; the reading report checks
//! between its three sequential requests.

use crate::api::{
    day_read_params, day_read_url, devices_url, parse_auth_identity, read_time_info_params,
    read_time_info_url, reading_info_url, storage_url, user_me_url, ApiEndpoints, ApiError,
};
use crate::cancellable_prepare::envelope_json;
use crate::cancellable_push::{
    block_on_cancellable, check_before_request, client, runtime, with_bearer, RequestError,
};
use crate::models::UploadAuthContext;
use serde_json::Value;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

async fn identity_request(
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
    let response = with_bearer(client.get(user_me_url(endpoints.api_base())), credential)
        .send()
        .await?;
    let user = envelope_json(response).await?;
    // A completed response must not turn cancellation into a successful
    // identity resolution or permit the caller to start a query.
    check_before_request(flag)?;
    Ok(parse_auth_identity(&user, credential.to_string())?)
}

async fn devices_request(
    client: &reqwest::Client,
    endpoints: &ApiEndpoints,
    auth: &UploadAuthContext,
    flag: Option<&'static AtomicBool>,
) -> Result<Value, RequestError> {
    check_before_request(flag)?;
    let response = with_bearer(client.get(devices_url(endpoints.api_base())), &auth.bearer)
        .send()
        .await?;
    let raw = envelope_json(response).await?;
    check_before_request(flag)?;
    Ok(raw)
}

async fn storage_request(
    client: &reqwest::Client,
    endpoints: &ApiEndpoints,
    auth: &UploadAuthContext,
    flag: Option<&'static AtomicBool>,
) -> Result<Value, RequestError> {
    check_before_request(flag)?;
    let response = with_bearer(client.get(storage_url(endpoints.api_base())), &auth.bearer)
        .send()
        .await?;
    let value = envelope_json(response).await?;
    check_before_request(flag)?;
    Ok(value)
}

async fn reading_request(
    client: &reqwest::Client,
    endpoints: &ApiEndpoints,
    auth: &UploadAuthContext,
    date: &str,
    flag: Option<&'static AtomicBool>,
) -> Result<(Value, Value, Value), RequestError> {
    check_before_request(flag)?;
    let response = with_bearer(
        client.get(reading_info_url(endpoints.api_base())),
        &auth.bearer,
    )
    .send()
    .await?;
    let reading_info = envelope_json(response).await?;
    // These are separate read-only requests. A signal after one response must
    // stop the remaining requests rather than letting the sequence continue.
    check_before_request(flag)?;
    let response = with_bearer(
        client
            .get(read_time_info_url(endpoints.api_base()))
            .query(&read_time_info_params(date, "week")),
        &auth.bearer,
    )
    .send()
    .await?;
    let week = envelope_json(response).await?;

    check_before_request(flag)?;
    let response = with_bearer(
        client
            .get(day_read_url(endpoints.api_base()))
            .query(&day_read_params(date, date)),
        &auth.bearer,
    )
    .send()
    .await?;
    let day = envelope_json(response).await?;
    check_before_request(flag)?;
    Ok((reading_info, week, day))
}

fn prepare(
    timeout: Duration,
    flag: Option<&'static AtomicBool>,
) -> Result<(tokio::runtime::Runtime, reqwest::Client), RequestError> {
    check_before_request(flag)?;
    Ok((runtime()?, client(timeout)?))
}

pub(crate) fn identity(
    timeout: Duration,
    endpoints: &ApiEndpoints,
    credential: &str,
    flag: Option<&'static AtomicBool>,
) -> Result<UploadAuthContext, RequestError> {
    let (runtime, client) = prepare(timeout, flag)?;
    block_on_cancellable(
        &runtime,
        flag,
        identity_request(&client, endpoints, credential, flag),
    )?
}

pub(crate) fn devices(
    timeout: Duration,
    endpoints: &ApiEndpoints,
    auth: &UploadAuthContext,
    flag: Option<&'static AtomicBool>,
) -> Result<Value, RequestError> {
    let (runtime, client) = prepare(timeout, flag)?;
    block_on_cancellable(
        &runtime,
        flag,
        devices_request(&client, endpoints, auth, flag),
    )?
}

pub(crate) fn storage(
    timeout: Duration,
    endpoints: &ApiEndpoints,
    auth: &UploadAuthContext,
    flag: Option<&'static AtomicBool>,
) -> Result<Value, RequestError> {
    let (runtime, client) = prepare(timeout, flag)?;
    block_on_cancellable(
        &runtime,
        flag,
        storage_request(&client, endpoints, auth, flag),
    )?
}

pub(crate) fn reading(
    timeout: Duration,
    endpoints: &ApiEndpoints,
    auth: &UploadAuthContext,
    date: &str,
    flag: Option<&'static AtomicBool>,
) -> Result<(Value, Value, Value), RequestError> {
    let (runtime, client) = prepare(timeout, flag)?;
    block_on_cancellable(
        &runtime,
        flag,
        reading_request(&client, endpoints, auth, date, flag),
    )?
}
