use crate::models::{
    ApiEnvelope, BucketConfig, DashboardPushItem, OssSts, PhoneOrEmailLoginRequest, PushDocDetail,
    QrCheckResponse, QrCreateResponse, SyncToken, UploadAuthContext, VerifyCodeRequest,
};
use crate::state::get_auth_state;
use crate::util::{json_field_to_string, normalize_optional, parse_u64_field};
use reqwest::blocking::{Client, RequestBuilder};
use reqwest::cookie::{CookieStore, Jar};
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, COOKIE, SET_COOKIE};
use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use std::time::Duration;
use url::Url;
use uuid::Uuid;

const BASE_URL: &str = "https://send2boox.com";
const DEFAULT_BUCKET_KEY: &str = "onyx-cloud";
const PUSH_QUEUE_LIMIT: usize = 30;

/// Explicit service origins. Production clients never consult test environment variables.
#[derive(Clone, Debug)]
pub struct ApiEndpoints {
    api: String,
    oss: Option<String>,
}
impl ApiEndpoints {
    pub fn production() -> Self {
        Self {
            api: BASE_URL.into(),
            oss: None,
        }
    }
    pub fn loopback(api: &str, oss: &str) -> Result<Self, String> {
        fn validate(value: &str) -> Result<String, String> {
            let u = Url::parse(value).map_err(|_| "invalid test endpoint")?;
            let local = match u.host() {
                Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
                Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
                _ => false,
            };
            if !local
                || !matches!(u.scheme(), "http" | "https")
                || !u.username().is_empty()
                || u.password().is_some()
                || u.query().is_some()
                || u.fragment().is_some()
                || u.path() != "/"
            {
                return Err("test endpoint must be a literal loopback origin".into());
            }
            Ok(u.origin().ascii_serialization())
        }
        Ok(Self {
            api: validate(api)?,
            oss: Some(validate(oss)?),
        })
    }
    pub(crate) fn api_base(&self) -> &str {
        &self.api
    }
    pub(crate) fn oss_url(&self, url: &str) -> String {
        match &self.oss {
            Some(base) => {
                let path = Url::parse(url).expect("generated OSS URL");
                format!("{base}{}", path.path())
            }
            None => url.into(),
        }
    }
}

pub fn create_client(timeout_secs: u64) -> Result<Client, String> {
    Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(timeout_secs))
        .build()
        .map_err(|err| err.to_string())
}

pub fn create_client_with_base_url(timeout_secs: u64, base_url: &str) -> Result<Client, String> {
    let _ = Url::parse(base_url).map_err(|err| err.to_string())?;
    create_client(timeout_secs)
}

fn with_optional_token(request: RequestBuilder, token: Option<&str>) -> RequestBuilder {
    match token.filter(|value| !value.trim().is_empty()) {
        Some(token) => request.header(AUTHORIZATION, format!("Bearer {token}")),
        None => request,
    }
}

fn parse_envelope<T: DeserializeOwned>(text: &str) -> Result<T, String> {
    let value: ApiEnvelope<T> = serde_json::from_str(text).map_err(|err| err.to_string())?;
    if value.result_code != 0 {
        return Err(value
            .message
            .unwrap_or_else(|| format!("result_code={}", value.result_code)));
    }
    Ok(value.data)
}

pub fn api_get_value(client: &Client, url: &str, token: Option<&str>) -> Result<Value, String> {
    let response = with_optional_token(client.get(url), token)
        .send()
        .map_err(|err| err.to_string())?;
    let status = response.status();
    let text = response.text().map_err(|err| err.to_string())?;
    if !status.is_success() {
        return Err(format!("HTTP {status}: {text}"));
    }
    parse_envelope(&text)
}

pub fn api_post_value(
    client: &Client,
    url: &str,
    token: Option<&str>,
    body: &Value,
) -> Result<Value, String> {
    let response = with_optional_token(
        client.post(url).header(CONTENT_TYPE, "application/json"),
        token,
    )
    .body(body.to_string())
    .send()
    .map_err(|err| err.to_string())?;
    let status = response.status();
    let text = response.text().map_err(|err| err.to_string())?;
    if !status.is_success() {
        return Err(format!("HTTP {status}: {text}"));
    }
    parse_envelope(&text)
}

pub fn create_qr_login(client: &Client) -> Result<QrCreateResponse, String> {
    create_qr_login_at(client, BASE_URL)
}

pub fn create_qr_login_at(client: &Client, base_url: &str) -> Result<QrCreateResponse, String> {
    let response = client
        .post(format!("{base_url}/api/1/auth/qrcode/create"))
        .header(CONTENT_TYPE, "application/json")
        .body(json!({ "clientType": "web" }).to_string())
        .send()
        .map_err(|err| err.to_string())?;
    let status = response.status();
    let text = response.text().map_err(|err| err.to_string())?;
    if !status.is_success() {
        return Err(format!("HTTP {status}: {text}"));
    }
    let data = parse_envelope::<Value>(&text)?;
    parse_qr_create_response(data)
}

fn parse_qr_create_response(data: Value) -> Result<QrCreateResponse, String> {
    if let Some(raw) = data
        .as_str()
        .and_then(|value| normalize_optional(Some(value.to_string())))
    {
        return Ok(QrCreateResponse {
            qrcode_id: raw.clone(),
            qrcode_data: raw,
        });
    }

    let qrcode_id = json_field_to_string(&data, "qrcode_id")
        .or_else(|| json_field_to_string(&data, "qrcodeId"))
        .or_else(|| json_field_to_string(&data, "id"))
        .or_else(|| json_field_to_string(&data, "qrcode"))
        .ok_or_else(|| "二维码接口返回缺少 qrcodeId".to_string())?;
    let qrcode_data = json_field_to_string(&data, "data")
        .or_else(|| json_field_to_string(&data, "qrcode"))
        .or_else(|| json_field_to_string(&data, "qrcodeData"))
        .unwrap_or_else(|| qrcode_id.clone());

    Ok(QrCreateResponse {
        qrcode_id,
        qrcode_data,
    })
}

pub fn check_qr_login(client: &Client, qrcode_id: &str) -> Result<QrCheckResponse, String> {
    check_qr_login_at(client, BASE_URL, qrcode_id)
}

pub fn check_qr_login_at(
    client: &Client,
    base_url: &str,
    qrcode_id: &str,
) -> Result<QrCheckResponse, String> {
    let normalized_qrcode_id = normalize_qr_check_id(qrcode_id);
    let response = client
        .get(format!("{base_url}/api/1/auth/qrcode/check"))
        .query(&[("clientType", "web"), ("qrcodeId", normalized_qrcode_id)])
        .send()
        .map_err(|err| err.to_string())?;
    let status = response.status();
    let text = response.text().map_err(|err| err.to_string())?;
    if !status.is_success() {
        return Err(format!("HTTP {status}: {text}"));
    }
    parse_envelope(&text)
}

pub fn send_verify_code(client: &Client, payload: &VerifyCodeRequest) -> Result<(), String> {
    send_verify_code_with_endpoints(&crate::api::ApiEndpoints::production(), client, payload)
}

pub fn send_verify_code_with_endpoints(
    endpoints: &crate::api::ApiEndpoints,
    client: &Client,
    payload: &VerifyCodeRequest,
) -> Result<(), String> {
    let base_url = endpoints.api_base();
    let response = client
        .post(format!("{base_url}/api/1/users/sendVerifyCode"))
        .header(CONTENT_TYPE, "application/json")
        .body(serde_json::to_string(payload).map_err(|err| err.to_string())?)
        .send()
        .map_err(|err| err.to_string())?;
    let status = response.status();
    let text = response.text().map_err(|err| err.to_string())?;
    if !status.is_success() {
        return Err(format!("HTTP {status}: {text}"));
    }
    let _: Value = parse_envelope(&text)?;
    Ok(())
}

pub fn signup_by_phone_or_email(
    client: &Client,
    payload: &PhoneOrEmailLoginRequest,
) -> Result<String, String> {
    signup_by_phone_or_email_with_endpoints(
        &crate::api::ApiEndpoints::production(),
        client,
        payload,
    )
}

pub fn signup_by_phone_or_email_with_endpoints(
    endpoints: &crate::api::ApiEndpoints,
    client: &Client,
    payload: &PhoneOrEmailLoginRequest,
) -> Result<String, String> {
    let base_url = endpoints.api_base();
    let response = client
        .post(format!("{base_url}/api/1/users/signupByPhoneOrEmail"))
        .header(CONTENT_TYPE, "application/json")
        .body(serde_json::to_string(payload).map_err(|err| err.to_string())?)
        .send()
        .map_err(|err| err.to_string())?;
    let status = response.status();
    let text = response.text().map_err(|err| err.to_string())?;
    if !status.is_success() {
        return Err(format!("HTTP {status}: {text}"));
    }
    let data = parse_envelope::<Value>(&text)?;
    parse_phone_or_email_login_token(data)
}

fn normalize_qr_check_id(qrcode_id: &str) -> &str {
    qrcode_id
        .split_once("---")
        .map(|(id, _)| id)
        .filter(|id| !id.trim().is_empty())
        .unwrap_or(qrcode_id)
}

fn parse_phone_or_email_login_token(data: Value) -> Result<String, String> {
    if let Some(token) = data
        .as_str()
        .and_then(|value| normalize_optional(Some(value.to_string())))
    {
        return Ok(token);
    }

    json_field_to_string(&data, "token")
        .or_else(|| json_field_to_string(&data, "access_token"))
        .ok_or_else(|| "登录接口返回缺少 token".to_string())
}

pub fn auth_source_text(auth: &UploadAuthContext) -> String {
    if auth.bearer.trim().is_empty() {
        "unknown".to_string()
    } else {
        "token".to_string()
    }
}

pub fn fetch_auth_context_and_user(client: &Client) -> Result<(UploadAuthContext, Value), String> {
    fetch_auth_context_and_user_with_endpoints(&crate::api::ApiEndpoints::production(), client)
}

pub fn fetch_auth_context_and_user_with_endpoints(
    endpoints: &crate::api::ApiEndpoints,
    client: &Client,
) -> Result<(UploadAuthContext, Value), String> {
    let auth = get_auth_state();
    let value = normalize_optional(auth.token).ok_or_else(|| "当前未登录".to_string())?;
    fetch_auth_context_for_token_with_endpoints(endpoints, client, &value)
}

/// Resolve the account and storage context for an already authenticated credential.
///
/// CLI adapters use this entry point so `auth import` and `SANDFLOS_TOKEN` are
/// honoured without consulting the desktop process's global auth cache.
pub fn fetch_auth_context_for_token(
    client: &Client,
    value: &str,
) -> Result<(UploadAuthContext, Value), String> {
    fetch_auth_context_for_token_with_endpoints(
        &crate::api::ApiEndpoints::production(),
        client,
        value,
    )
}

pub fn fetch_auth_context_for_token_with_endpoints(
    endpoints: &crate::api::ApiEndpoints,
    client: &Client,
    value: &str,
) -> Result<(UploadAuthContext, Value), String> {
    let (mut auth, user) = fetch_identity_for_token_with_endpoints(endpoints, client, value)?;
    // Uploads must not proceed when the quota request failed.
    let storage = fetch_storage_with_endpoints(endpoints, client, &auth.bearer)?;
    auth.storage_limit = parse_u64_field(&storage, "totalSize")
        .or_else(|| parse_u64_field(&storage, "storageLimit"))
        .or(auth.storage_limit);
    auth.storage_used = parse_u64_field(&storage, "usedSize")
        .or_else(|| parse_u64_field(&storage, "used"))
        .or(auth.storage_used);
    Ok((auth, user))
}

/// Resolve identity without making unrelated statistics requests.
pub fn fetch_identity_for_token_with_endpoints(
    endpoints: &crate::api::ApiEndpoints,
    client: &Client,
    value: &str,
) -> Result<(UploadAuthContext, Value), String> {
    let value = normalize_optional(Some(value.to_string()))
        .ok_or_else(|| "认证凭据不能为空".to_string())?;
    let user = fetch_user_me_at(client, endpoints.api_base(), &value)?;
    let uid = json_field_to_string(&user, "uid").ok_or_else(|| "登录信息缺少 uid".to_string())?;
    let auth = UploadAuthContext {
        bearer: value,
        uid,
        storage_limit: parse_u64_field(&user, "storage_limit"),
        storage_used: parse_u64_field(&user, "storage_used"),
    };
    Ok((auth, user))
}

pub fn fetch_user_me(client: &Client, token: &str) -> Result<Value, String> {
    fetch_user_me_at(client, BASE_URL, token)
}

pub fn fetch_user_me_at(client: &Client, base_url: &str, token: &str) -> Result<Value, String> {
    api_get_value(client, &format!("{base_url}/api/1/users/me"), Some(token))
}

pub fn fetch_storage(client: &Client, token: &str) -> Result<Value, String> {
    fetch_storage_with_endpoints(&crate::api::ApiEndpoints::production(), client, token)
}

pub fn fetch_storage_with_endpoints(
    endpoints: &crate::api::ApiEndpoints,
    client: &Client,
    token: &str,
) -> Result<Value, String> {
    let base_url = endpoints.api_base();
    api_get_value(
        client,
        &format!("{base_url}/api/1/statistics/v2/user/storage"),
        Some(token),
    )
}

pub fn fetch_devices(client: &Client, token: &str) -> Result<Value, String> {
    fetch_devices_with_endpoints(&crate::api::ApiEndpoints::production(), client, token)
}

pub fn fetch_devices_with_endpoints(
    endpoints: &crate::api::ApiEndpoints,
    client: &Client,
    token: &str,
) -> Result<Value, String> {
    let base_url = endpoints.api_base();
    api_get_value(
        client,
        &format!("{base_url}/api/1/users/getDevice"),
        Some(token),
    )
}

pub fn fetch_reading_info(client: &Client, token: &str) -> Result<Value, String> {
    fetch_reading_info_with_endpoints(&crate::api::ApiEndpoints::production(), client, token)
}

pub fn fetch_reading_info_with_endpoints(
    endpoints: &crate::api::ApiEndpoints,
    client: &Client,
    token: &str,
) -> Result<Value, String> {
    let base_url = endpoints.api_base();
    api_get_value(
        client,
        &format!("{base_url}/api/1/statistics/readingInfo"),
        Some(token),
    )
}

pub fn fetch_read_time_info(
    client: &Client,
    token: &str,
    date: &str,
    range: &str,
) -> Result<Value, String> {
    fetch_read_time_info_with_endpoints(
        &crate::api::ApiEndpoints::production(),
        client,
        token,
        date,
        range,
    )
}

pub fn fetch_read_time_info_with_endpoints(
    endpoints: &crate::api::ApiEndpoints,
    client: &Client,
    token: &str,
    date: &str,
    range: &str,
) -> Result<Value, String> {
    let base_url = endpoints.api_base();
    let response = with_optional_token(
        client.get(format!("{base_url}/api/1/statistics/readTimeInfo")),
        Some(token),
    )
    .query(&[("date", date), ("range", range)])
    .send()
    .map_err(|err| err.to_string())?;
    let status = response.status();
    let text = response.text().map_err(|err| err.to_string())?;
    if !status.is_success() {
        return Err(format!("HTTP {status}: {text}"));
    }
    parse_envelope(&text)
}

pub fn fetch_day_read(
    client: &Client,
    token: &str,
    start_time: &str,
    end_time: &str,
) -> Result<Value, String> {
    fetch_day_read_with_endpoints(
        &crate::api::ApiEndpoints::production(),
        client,
        token,
        start_time,
        end_time,
    )
}

pub fn fetch_day_read_with_endpoints(
    endpoints: &crate::api::ApiEndpoints,
    client: &Client,
    token: &str,
    start_time: &str,
    end_time: &str,
) -> Result<Value, String> {
    let base_url = endpoints.api_base();
    let response = with_optional_token(
        client.get(format!("{base_url}/api/1/statistics/dayRead")),
        Some(token),
    )
    .query(&[("startTime", start_time), ("endTime", end_time)])
    .send()
    .map_err(|err| err.to_string())?;
    let status = response.status();
    let text = response.text().map_err(|err| err.to_string())?;
    if !status.is_success() {
        return Err(format!("HTTP {status}: {text}"));
    }
    parse_envelope(&text)
}

pub fn fetch_buckets(
    client: &Client,
) -> Result<std::collections::HashMap<String, BucketConfig>, String> {
    fetch_buckets_with_endpoints(&crate::api::ApiEndpoints::production(), client)
}

pub fn fetch_buckets_with_endpoints(
    endpoints: &crate::api::ApiEndpoints,
    client: &Client,
) -> Result<std::collections::HashMap<String, BucketConfig>, String> {
    let base_url = endpoints.api_base();
    let data = api_get_value(client, &format!("{base_url}/api/1/config/buckets"), None)?;
    serde_json::from_value(data).map_err(|err| err.to_string())
}

pub fn fetch_default_bucket(client: &Client) -> Result<(String, BucketConfig), String> {
    fetch_default_bucket_with_endpoints(&crate::api::ApiEndpoints::production(), client)
}

pub fn fetch_default_bucket_with_endpoints(
    endpoints: &crate::api::ApiEndpoints,
    client: &Client,
) -> Result<(String, BucketConfig), String> {
    let buckets = fetch_buckets_with_endpoints(endpoints, client)?;
    buckets
        .get_key_value(DEFAULT_BUCKET_KEY)
        .or_else(|| buckets.iter().next())
        .map(|(key, value)| (key.clone(), value.clone()))
        .ok_or_else(|| "未获取到可用 bucket 配置".to_string())
}

pub fn fetch_sts_for_auth(client: &Client, auth: &UploadAuthContext) -> Result<OssSts, String> {
    fetch_sts_for_auth_with_endpoints(&crate::api::ApiEndpoints::production(), client, auth)
}

pub fn fetch_sts_for_auth_with_endpoints(
    endpoints: &crate::api::ApiEndpoints,
    client: &Client,
    auth: &UploadAuthContext,
) -> Result<OssSts, String> {
    let base_url = endpoints.api_base();
    let data = api_get_value(
        client,
        &format!("{base_url}/api/1/config/stss"),
        Some(&auth.bearer),
    )?;
    serde_json::from_value(data).map_err(|err| err.to_string())
}

fn sync_cookie_header(sync: &SyncToken) -> Result<String, String> {
    let cookie_name = sync
        .cookie_name
        .as_deref()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or("SyncGatewaySession");
    let session_id = sync
        .session_id
        .as_deref()
        .filter(|v| !v.trim().is_empty())
        .ok_or_else(|| "syncToken 缺少 session_id".to_string())?;
    Ok(format!("{cookie_name}={session_id}"))
}

fn fetch_sync_token_materials_for_auth_with_endpoints(
    endpoints: &crate::api::ApiEndpoints,
    client: &Client,
    auth: &UploadAuthContext,
) -> Result<(SyncToken, Vec<String>), String> {
    let base_url = endpoints.api_base();
    let response = with_optional_token(
        client.get(format!("{base_url}/api/1/users/syncToken")),
        Some(&auth.bearer),
    )
    .send()
    .map_err(|err| err.to_string())?;
    let headers = response.headers().clone();
    let status = response.status();
    let text = response.text().map_err(|err| err.to_string())?;
    if !status.is_success() {
        return Err(format!("HTTP {status}: {text}"));
    }
    let data = parse_envelope::<Value>(&text)?;
    let sync: SyncToken = serde_json::from_value(data).map_err(|err| err.to_string())?;
    let set_cookies = headers
        .get_all(SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok().map(ToString::to_string))
        .collect::<Vec<_>>();
    Ok((sync, set_cookies))
}

fn neocloud_request(
    endpoints: &crate::api::ApiEndpoints,
    client: &Client,
    sync: &SyncToken,
    set_cookies: &[String],
    method: reqwest::Method,
    url: &str,
) -> Result<RequestBuilder, String> {
    let cookie_url = Url::parse(endpoints.api_base()).map_err(|err| err.to_string())?;
    let request_url = Url::parse(url).map_err(|err| err.to_string())?;
    let jar = Jar::default();
    jar.add_cookie_str(&sync_cookie_header(sync)?, &cookie_url);
    for set_cookie in set_cookies {
        jar.add_cookie_str(set_cookie, &cookie_url);
    }
    let cookie = jar
        .cookies(&request_url)
        .ok_or_else(|| "syncToken 缺少可用 cookie".to_string())?;
    // Reuse the caller's client so timeout and redirect policy also cover
    // neocloud requests. Cookie selection still honours domain/path/secure.
    Ok(client.request(method, request_url).header(COOKIE, cookie))
}

fn parse_push_content(doc: &Value) -> Value {
    match doc.get("content") {
        Some(Value::String(text)) => serde_json::from_str(text).unwrap_or(Value::Null),
        Some(Value::Object(_)) => doc.get("content").cloned().unwrap_or(Value::Null),
        _ => Value::Null,
    }
}

fn parse_push_item_doc(doc: &Value) -> Option<DashboardPushItem> {
    let content = parse_push_content(doc);
    let formats = content
        .get("formats")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let format = formats
        .first()
        .and_then(Value::as_str)
        .map(ToString::to_string);
    let storage = format
        .as_deref()
        .and_then(|fmt| content.get("storage").and_then(|s| s.get(fmt)).cloned())
        .unwrap_or(Value::Null);
    let oss = storage.get("oss").cloned().unwrap_or(Value::Null);
    let id = json_field_to_string(doc, "_id")
        .or_else(|| json_field_to_string(doc, "id"))
        .or_else(|| json_field_to_string(&content, "_id"))?;
    Some(DashboardPushItem {
        id,
        rev: json_field_to_string(doc, "_rev"),
        name: json_field_to_string(doc, "name")
            .or_else(|| json_field_to_string(&content, "name"))
            .or_else(|| json_field_to_string(&content, "title"))
            .unwrap_or_else(|| "(未命名文件)".to_string()),
        size: doc
            .get("size")
            .and_then(Value::as_u64)
            .or_else(|| content.get("size").and_then(Value::as_u64))
            .or_else(|| oss.get("size").and_then(Value::as_u64)),
        updated_at: doc
            .get("updatedAt")
            .and_then(Value::as_i64)
            .or_else(|| content.get("updatedAt").and_then(Value::as_i64))
            .or_else(|| doc.get("createdAt").and_then(Value::as_i64)),
        format,
        resource_key: json_field_to_string(&oss, "key")
            .or_else(|| json_field_to_string(&content, "resourceKey")),
    })
}

pub fn fetch_push_queue_for_dashboard(
    client: &Client,
    auth: &UploadAuthContext,
) -> Result<Vec<DashboardPushItem>, String> {
    fetch_push_queue_for_dashboard_with_endpoints(
        &crate::api::ApiEndpoints::production(),
        client,
        auth,
    )
}

pub fn fetch_push_queue_for_dashboard_with_endpoints(
    endpoints: &crate::api::ApiEndpoints,
    client: &Client,
    auth: &UploadAuthContext,
) -> Result<Vec<DashboardPushItem>, String> {
    let base_url = endpoints.api_base();
    let (sync, set_cookies) =
        fetch_sync_token_materials_for_auth_with_endpoints(endpoints, client, auth)?;
    let channel = format!("{}-MESSAGE", auth.uid);
    let url = format!(
        "{base_url}/neocloud/_changes?filter=sync_gateway/bychannel&channels={}&include_docs=true&descending=true&limit=200",
        urlencoding::encode(&channel)
    );

    let response = neocloud_request(
        endpoints,
        client,
        &sync,
        &set_cookies,
        reqwest::Method::GET,
        &url,
    )?
    .send()
    .map_err(|err| err.to_string())?;
    let status = response.status();
    let text = response.text().map_err(|err| err.to_string())?;
    if !status.is_success() {
        return Err(format!("HTTP {status}: {text}"));
    }

    let value: Value = serde_json::from_str(&text).map_err(|err| err.to_string())?;
    let rows = value
        .get("results")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let mut items = Vec::new();
    for row in rows {
        let doc = row.get("doc").cloned().unwrap_or(Value::Null);
        if doc.get("msgType").and_then(Value::as_i64) != Some(2)
            || doc.get("contentType").and_then(Value::as_str) != Some("digital_content")
        {
            continue;
        }
        if let Some(item) = parse_push_item_doc(&doc) {
            items.push(item);
        }
    }
    items.sort_by_key(|item| std::cmp::Reverse(item.updated_at));
    items.truncate(PUSH_QUEUE_LIMIT);
    Ok(items)
}

pub fn fetch_push_doc_value(
    client: &Client,
    auth: &UploadAuthContext,
    doc_id: &str,
) -> Result<Value, String> {
    fetch_push_doc_value_with_endpoints(
        &crate::api::ApiEndpoints::production(),
        client,
        auth,
        doc_id,
    )
}

pub fn fetch_push_doc_value_with_endpoints(
    endpoints: &crate::api::ApiEndpoints,
    client: &Client,
    auth: &UploadAuthContext,
    doc_id: &str,
) -> Result<Value, String> {
    let base_url = endpoints.api_base();
    let (sync, set_cookies) =
        fetch_sync_token_materials_for_auth_with_endpoints(endpoints, client, auth)?;
    let url = format!("{base_url}/neocloud/{}", urlencoding::encode(doc_id));
    let response = neocloud_request(
        endpoints,
        client,
        &sync,
        &set_cookies,
        reqwest::Method::GET,
        &url,
    )?
    .send()
    .map_err(|err| err.to_string())?;
    let status = response.status();
    let text = response.text().map_err(|err| err.to_string())?;
    if !status.is_success() {
        crate::diagnostics::warn(
            "api.fetch_push_doc_value",
            format!("doc_id={doc_id} status={status} body={}", "[redacted]"),
        );
        return Err(format!("HTTP {status}: {text}"));
    }
    serde_json::from_str(&text).map_err(|err| err.to_string())
}

pub fn fetch_push_doc(
    client: &Client,
    auth: &UploadAuthContext,
    doc_id: &str,
) -> Result<PushDocDetail, String> {
    fetch_push_doc_with_endpoints(
        &crate::api::ApiEndpoints::production(),
        client,
        auth,
        doc_id,
    )
}

pub fn fetch_push_doc_with_endpoints(
    endpoints: &crate::api::ApiEndpoints,
    client: &Client,
    auth: &UploadAuthContext,
    doc_id: &str,
) -> Result<PushDocDetail, String> {
    let doc = fetch_push_doc_value_with_endpoints(endpoints, client, auth, doc_id)?;
    let item = parse_push_item_doc(&doc).ok_or_else(|| "推送记录格式不完整".to_string())?;
    Ok(PushDocDetail {
        rev: item.rev.unwrap_or_default(),
        resource_key: item.resource_key.unwrap_or_default(),
    })
}

pub fn delete_push_doc(
    client: &Client,
    auth: &UploadAuthContext,
    doc_id: &str,
) -> Result<(), String> {
    delete_push_doc_with_endpoints(
        &crate::api::ApiEndpoints::production(),
        client,
        auth,
        doc_id,
    )
}

pub fn delete_push_doc_with_endpoints(
    endpoints: &crate::api::ApiEndpoints,
    client: &Client,
    auth: &UploadAuthContext,
    doc_id: &str,
) -> Result<(), String> {
    let base_url = endpoints.api_base();
    let detail = fetch_push_doc_with_endpoints(endpoints, client, auth, doc_id)?;
    let (sync, set_cookies) =
        fetch_sync_token_materials_for_auth_with_endpoints(endpoints, client, auth)?;
    let url = format!(
        "{base_url}/neocloud/{}?rev={}",
        urlencoding::encode(doc_id),
        urlencoding::encode(&detail.rev)
    );
    let response = neocloud_request(
        endpoints,
        client,
        &sync,
        &set_cookies,
        reqwest::Method::DELETE,
        &url,
    )?
    .send()
    .map_err(|err| err.to_string())?;
    let status = response.status();
    let text = response.text().map_err(|err| err.to_string())?;
    if !status.is_success() {
        return Err(format!("HTTP {status}: {text}"));
    }
    Ok(())
}

pub fn batch_delete_push_message(
    client: &Client,
    auth: &UploadAuthContext,
    doc_id: &str,
) -> Result<(), String> {
    batch_delete_push_message_with_endpoints(
        &crate::api::ApiEndpoints::production(),
        client,
        auth,
        doc_id,
    )
}

pub fn batch_delete_push_message_with_endpoints(
    endpoints: &crate::api::ApiEndpoints,
    client: &Client,
    auth: &UploadAuthContext,
    doc_id: &str,
) -> Result<(), String> {
    let base_url = endpoints.api_base();
    let _ = api_post_value(
        client,
        &format!("{base_url}/api/1/push/message/batchDelete"),
        Some(&auth.bearer),
        &json!({ "ids": [doc_id] }),
    )?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn save_and_push(
    client: &Client,
    auth: &UploadAuthContext,
    bucket_key: &str,
    object_key: &str,
    file_name: &str,
    resource_type: &str,
    cb_id: &str,
    cb_rev: &str,
) -> Result<Value, String> {
    save_and_push_with_endpoints(
        &crate::api::ApiEndpoints::production(),
        client,
        auth,
        bucket_key,
        object_key,
        file_name,
        resource_type,
        cb_id,
        cb_rev,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn save_and_push_with_endpoints(
    endpoints: &crate::api::ApiEndpoints,
    client: &Client,
    auth: &UploadAuthContext,
    bucket_key: &str,
    object_key: &str,
    file_name: &str,
    resource_type: &str,
    cb_id: &str,
    cb_rev: &str,
) -> Result<Value, String> {
    let base_url = endpoints.api_base();
    let body = json!({
        "data": {
            "name": file_name,
            "resourceDisplayName": file_name,
            "resourceKey": object_key,
            "bucket": bucket_key,
            "resourceType": resource_type,
            "title": file_name,
            "parent": Value::Null
        },
        "cbMsg": {
            "id": cb_id,
            "rev": cb_rev
        }
    });

    // Submit exactly once. A timeout or gateway error here is ambiguous: the
    // server may already have accepted the message, so retrying in-process can
    // publish the same file twice. The caller surfaces the outcome as
    // `unknown` and the user resolves it by inspecting the queue first.
    api_post_value(
        client,
        &format!("{base_url}/api/1/push/saveAndPush"),
        Some(&auth.bearer),
        &body,
    )
}

pub fn put_existing_push_message_doc(
    client: &Client,
    auth: &UploadAuthContext,
    doc: &Value,
) -> Result<(String, String), String> {
    put_existing_push_message_doc_with_endpoints(
        &crate::api::ApiEndpoints::production(),
        client,
        auth,
        doc,
    )
}

pub fn put_existing_push_message_doc_with_endpoints(
    endpoints: &crate::api::ApiEndpoints,
    client: &Client,
    auth: &UploadAuthContext,
    doc: &Value,
) -> Result<(String, String), String> {
    let base_url = endpoints.api_base();
    let doc_id = json_field_to_string(doc, "_id").ok_or_else(|| "推送消息缺少 _id".to_string())?;
    let doc_rev =
        json_field_to_string(doc, "_rev").ok_or_else(|| "推送消息缺少 _rev".to_string())?;
    let (sync, set_cookies) =
        fetch_sync_token_materials_for_auth_with_endpoints(endpoints, client, auth)?;
    let url = format!("{base_url}/neocloud/{}", urlencoding::encode(&doc_id));
    let response = neocloud_request(
        endpoints,
        client,
        &sync,
        &set_cookies,
        reqwest::Method::PUT,
        &url,
    )?
    .header(ACCEPT, "application/json")
    .header(CONTENT_TYPE, "application/json")
    .body(doc.to_string())
    .send()
    .map_err(|err| err.to_string())?;
    let status = response.status();
    let text = response.text().map_err(|err| err.to_string())?;
    if !status.is_success() {
        crate::diagnostics::warn(
            "api.put_existing_push_message_doc",
            format!(
                "doc_id={doc_id} doc_rev={doc_rev} status={status} body={}",
                "[redacted]"
            ),
        );
        return Err(format!("HTTP {status}"));
    }
    let value: Value = serde_json::from_str(&text).map_err(|err| err.to_string())?;
    if json_field_to_string(&value, "error").is_some() {
        crate::diagnostics::warn(
            "api.put_existing_push_message_doc",
            format!("doc_id={doc_id} doc_rev={doc_rev} remote_error=true"),
        );
        return Err("remote message update rejected".to_string());
    }
    let id = json_field_to_string(&value, "id").unwrap_or(doc_id);
    let rev = json_field_to_string(&value, "rev")
        .or_else(|| json_field_to_string(doc, "_rev"))
        .unwrap_or(doc_rev);
    Ok((id, rev))
}

pub fn put_push_message_doc(
    client: &Client,
    auth: &UploadAuthContext,
    file_name: &str,
    file_size: u64,
    resource_type: &str,
    object_key: &str,
    signed_url: &str,
) -> Result<(String, String), String> {
    put_push_message_doc_with_endpoints(
        &crate::api::ApiEndpoints::production(),
        client,
        auth,
        file_name,
        file_size,
        resource_type,
        object_key,
        signed_url,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn put_push_message_doc_with_endpoints(
    endpoints: &crate::api::ApiEndpoints,
    client: &Client,
    auth: &UploadAuthContext,
    file_name: &str,
    file_size: u64,
    resource_type: &str,
    object_key: &str,
    signed_url: &str,
) -> Result<(String, String), String> {
    let base_url = endpoints.api_base();
    let (sync, set_cookies) =
        fetch_sync_token_materials_for_auth_with_endpoints(endpoints, client, auth)?;
    let message_id = Uuid::new_v4().to_string();
    let now_ms = crate::util::unix_ms_now() as u64;

    let mut storage_bucket = serde_json::Map::new();
    storage_bucket.insert(
        resource_type.to_string(),
        json!({
            "oss": {
                "displayName": file_name,
                "expires": 0,
                "key": object_key,
                "provider": "oss",
                "size": file_size,
                "url": signed_url
            }
        }),
    );

    let push_file_doc = json!({
        "_id": message_id,
        "createdAt": now_ms,
        "distributeChannel": "onyx",
        "formats": [resource_type],
        "guid": message_id,
        "name": file_name,
        "ownerId": auth.uid,
        "size": file_size,
        "md5": "",
        "storage": Value::Object(storage_bucket),
        "title": file_name,
        "updatedAt": now_ms
    });
    let message_doc = json!({
        "contentType": "digital_content",
        "content": push_file_doc.to_string(),
        "msgType": 2,
        "dbId": format!("{}-MESSAGE", auth.uid),
        "user": auth.uid,
        "name": file_name,
        "size": file_size,
        "uniqueId": message_id,
        "createdAt": now_ms,
        "updatedAt": now_ms,
        "_id": message_id
    });

    let url = format!("{base_url}/neocloud/{}", urlencoding::encode(&message_id));
    let response = neocloud_request(
        endpoints,
        client,
        &sync,
        &set_cookies,
        reqwest::Method::PUT,
        &url,
    )?
    .header(ACCEPT, "application/json")
    .header(CONTENT_TYPE, "application/json")
    .body(message_doc.to_string())
    .send()
    .map_err(|err| err.to_string())?;
    let status = response.status();
    let text = response.text().map_err(|err| err.to_string())?;
    if !status.is_success() {
        crate::diagnostics::warn(
            "api.put_push_message_doc",
            format!(
                "message_id={message_id} status={status} body={}",
                "[redacted]"
            ),
        );
        return Err(format!("HTTP {status}"));
    }
    let value: Value = serde_json::from_str(&text).map_err(|err| err.to_string())?;
    if json_field_to_string(&value, "error").is_some() {
        crate::diagnostics::warn(
            "api.put_push_message_doc",
            format!("message_id={message_id} remote_error=true"),
        );
        return Err("remote message write rejected".to_string());
    }
    let id = json_field_to_string(&value, "id").unwrap_or(message_id);
    let rev = json_field_to_string(&value, "rev")
        .ok_or_else(|| format!("neocloud 返回缺少 rev: {text}"))?;
    Ok((id, rev))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_qr_create_response_from_string_payload() {
        let parsed = parse_qr_create_response(json!("abc---web---send2boox.com")).expect("parsed");
        assert_eq!(parsed.qrcode_id, "abc---web---send2boox.com");
        assert_eq!(parsed.qrcode_data, "abc---web---send2boox.com");
    }

    #[test]
    fn parses_qr_create_response_from_object_payload() {
        let parsed = parse_qr_create_response(json!({
            "qrcodeId": "abc",
            "qrcode": "display-value"
        }))
        .expect("parsed");
        assert_eq!(parsed.qrcode_id, "abc");
        assert_eq!(parsed.qrcode_data, "display-value");
    }

    #[test]
    fn normalizes_qr_check_id_to_match_web_login_polling() {
        assert_eq!(
            normalize_qr_check_id("abc123---web---send2boox.com"),
            "abc123"
        );
        assert_eq!(normalize_qr_check_id("plain-id"), "plain-id");
        assert_eq!(normalize_qr_check_id("---broken"), "---broken");
    }

    #[test]
    fn parses_phone_or_email_login_token_from_object_payload() {
        let token = parse_phone_or_email_login_token(json!({
            "token": "abc123"
        }))
        .expect("token");
        assert_eq!(token, "abc123");
    }

    #[test]
    fn parses_phone_or_email_login_token_from_string_payload() {
        let token = parse_phone_or_email_login_token(json!("abc123")).expect("token");
        assert_eq!(token, "abc123");
    }

    #[test]
    fn sync_cookie_header_avoids_duplicate_sync_cookie_names() {
        let header = sync_cookie_header(&SyncToken {
            cookie_name: Some("SyncGatewaySession".to_string()),
            session_id: Some("abc".to_string()),
        })
        .expect("cookie");
        assert_eq!(header, "SyncGatewaySession=abc");

        let header = sync_cookie_header(&SyncToken {
            cookie_name: Some("custom_sync".to_string()),
            session_id: Some("def".to_string()),
        })
        .expect("cookie");
        assert_eq!(header, "custom_sync=def");
    }
}
