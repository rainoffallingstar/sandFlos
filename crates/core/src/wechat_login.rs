//! Native WeChat QR login against the official Web QR login endpoints.
//!
//! The interactive QR flow is owned by this module so CLI callers need no
//! browser, local HTTP portal, or desktop process. Token persistence stays in
//! `AuthService`; this module never writes credentials itself.

use crate::api::create_client;
use crate::auth_service::{AuthError, AuthService, AuthStatus};
use crate::util::normalize_optional;
use reqwest::blocking::Client;
use reqwest::header::CONTENT_TYPE;
use serde_json::Value;
use std::{
    io::Read,
    net::{TcpStream, ToSocketAddrs},
    thread,
    time::{Duration, Instant},
};
use tungstenite::{client::IntoClientRequest, stream::MaybeTlsStream, Message, WebSocket};
use url::Url;
use uuid::Uuid;

const WECHAT_APP_ID: &str = "wxdef46042a22e2e89";
const WECHAT_SCOPE: &str = "snsapi_login";
const WECHAT_REDIRECT_URI: &str = "https://send2boox.com/api/1/passport/wechat/pc/login";
const WECHAT_QR_PAGE: &str = "https://open.weixin.qq.com/connect/qrconnect";
const WECHAT_QR_IMAGE_BASE: &str = "https://open.weixin.qq.com/connect/qrcode/";
const WECHAT_LONGPOLL_BASE: &str = "https://long.open.weixin.qq.com/connect/l/qrconnect";
const BOOX_SOCKET_BASE: &str = "wss://send2boox.com/ws/socketio/";
const BOOX_CALLBACK_BASE: &str = "https://send2boox.com/api/1/passport/wechat/pc/login";
const STATE_PREFIX: &str = "bind_login_";

const MAX_QR_IMAGE_BYTES: usize = 2 * 1024 * 1024;
const MAX_XML_BYTES: usize = 64 * 1024;
const MAX_LONGPOLL_SCRIPT_BYTES: usize = 16 * 1024;
const MAX_SOCKET_MESSAGE_BYTES: usize = 64 * 1024;
const MAX_QR_DIMENSION: u32 = 2048;
const MIN_QR_DIMENSION: u32 = 64;
const MAX_QR_MODULES: u32 = 177;
const MIN_QR_MODULES: u32 = 21;
const READ_POLL: Duration = Duration::from_millis(50);
const SOCKET_WAIT: Duration = Duration::from_millis(200);

/// Service origins used by the native WeChat login flow.
///
/// Production origins are fixed. A test caller may substitute loopback HTTP
/// and WebSocket origins; every URL is then derived below those origins.
#[derive(Clone, Debug)]
pub struct WechatLoginEndpoints {
    qr_page: String,
    image_base: String,
    longpoll: String,
    socket: String,
    callback: String,
    loopback: bool,
}

impl WechatLoginEndpoints {
    pub fn production() -> Self {
        Self {
            qr_page: WECHAT_QR_PAGE.to_string(),
            image_base: WECHAT_QR_IMAGE_BASE.to_string(),
            longpoll: WECHAT_LONGPOLL_BASE.to_string(),
            socket: BOOX_SOCKET_BASE.to_string(),
            callback: BOOX_CALLBACK_BASE.to_string(),
            loopback: false,
        }
    }

    /// Creates an endpoint set whose HTTP and WebSocket origins must both be
    /// literal loopback origins. Paths mirror the production protocol.
    pub fn loopback(base_http: &str, socket_ws: &str) -> Result<Self, AuthError> {
        let http = loopback_http_origin(base_http)?;
        let ws = loopback_ws_origin(socket_ws)?;
        Ok(Self {
            qr_page: format!("{http}/connect/qrconnect"),
            image_base: format!("{http}/connect/qrcode/"),
            longpoll: format!("{http}/connect/l/qrconnect"),
            socket: format!("{ws}/ws/socketio/"),
            callback: format!("{http}/api/1/passport/wechat/pc/login"),
            loopback: true,
        })
    }

    fn qr_page_url(&self, state: &str, nonce: &str) -> String {
        let mut url = Url::parse(&self.qr_page).expect("valid QR page endpoint");
        url.query_pairs_mut()
            .append_pair("appid", WECHAT_APP_ID)
            .append_pair("scope", WECHAT_SCOPE)
            .append_pair("redirect_uri", WECHAT_REDIRECT_URI)
            .append_pair("state", state)
            .append_pair("login_type", "jssdk")
            .append_pair("style", "black")
            .append_pair("self_redirect", "true")
            .append_pair("f", "xml")
            .append_pair("ts", nonce);
        url.to_string()
    }

    fn image_url(&self, uuid: &str) -> String {
        format!("{}{}", self.image_base, urlencoding::encode(uuid))
    }

    fn longpoll_url(&self, uuid: &str, last: Option<&str>) -> String {
        let mut url = Url::parse(&self.longpoll).expect("valid longpoll endpoint");
        url.query_pairs_mut().append_pair("uuid", uuid);
        if let Some(last) = last {
            url.query_pairs_mut().append_pair("last", last);
        }
        url.to_string()
    }

    fn socket_url(&self, state: &str) -> String {
        let mut url = Url::parse(&self.socket).expect("valid socket endpoint");
        let nonce = state.strip_prefix(STATE_PREFIX).unwrap_or(state);
        url.query_pairs_mut().append_pair("state", nonce);
        url.to_string()
    }

    fn callback_url(&self, code: &str, state: &str) -> String {
        let mut url = Url::parse(&self.callback).expect("valid callback endpoint");
        url.query_pairs_mut()
            .append_pair("code", code)
            .append_pair("state", state);
        url.to_string()
    }
}

fn loopback_http_origin(raw: &str) -> Result<String, AuthError> {
    let url = Url::parse(raw).map_err(|_| invalid_endpoint())?;
    if !is_loopback_host(&url)
        || url.scheme() != "http"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
    {
        return Err(invalid_endpoint());
    }
    Ok(url.origin().ascii_serialization())
}

fn loopback_ws_origin(raw: &str) -> Result<String, AuthError> {
    let url = Url::parse(raw).map_err(|_| invalid_endpoint())?;
    if !is_loopback_host(&url)
        || url.scheme() != "ws"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
    {
        return Err(invalid_endpoint());
    }
    Ok(url
        .origin()
        .ascii_serialization()
        .replacen("http://", "ws://", 1))
}

fn is_loopback_host(url: &Url) -> bool {
    match url.host() {
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        _ => false,
    }
}

fn invalid_endpoint() -> AuthError {
    AuthError::input(
        "invalid_endpoint",
        "微信登录测试端点必须是字面回环 http/ws 源",
    )
}

/// Normalized QR image: one boolean per module, including the image's quiet
/// border. `true` means a dark module.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WechatQr {
    pub modules: Vec<Vec<bool>>,
}

impl WechatQr {
    pub fn width(&self) -> usize {
        self.modules.first().map_or(0, Vec::len)
    }

    pub fn height(&self) -> usize {
        self.modules.len()
    }
}

/// Displays an official authorization URL after joining the BOOX login room.
/// The caller opens the URL on another machine; only outbound connections are
/// required here. The callback must return promptly. No token is persisted until
/// AuthService validates it. `session_timeout` bounds the browser waiting period.
pub fn login_with_wechat_url<F, C>(
    service: &AuthService,
    endpoints: WechatLoginEndpoints,
    request_timeout: Duration,
    session_timeout: Duration,
    on_url: F,
    cancel: C,
) -> Result<AuthStatus, AuthError>
where
    F: FnOnce(&str) -> Result<(), AuthError>,
    C: Fn() -> bool,
{
    if service.environment_token_active() {
        return Err(AuthError::input(
            "environment_token_active",
            "BOOXCTL_TOKEN 已生效；请清除该环境变量后重试",
        ));
    }
    if cancel() {
        return Err(AuthError::cancelled());
    }
    let deadline = Instant::now() + session_timeout;
    let nonce = Uuid::new_v4().to_string();
    let state = format!("{STATE_PREFIX}{nonce}");
    let mut socket = WechatSocket::connect(
        &endpoints,
        &state,
        Instant::now() + request_timeout.min(remaining(deadline)),
        &cancel,
    )?;
    socket.wait_for_room(
        &nonce,
        Instant::now() + request_timeout.min(remaining(deadline)),
        &cancel,
    )?;
    if cancel() {
        return Err(AuthError::cancelled());
    }
    let mut url =
        Url::parse(&endpoints.qr_page_url(&state, &nonce)).map_err(|_| protocol_error())?;
    let pairs: Vec<(String, String)> = url
        .query_pairs()
        .filter(|(key, _)| key != "f" && key != "ts")
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect();
    url.query_pairs_mut().clear().extend_pairs(pairs);
    on_url(url.as_str())?;
    let token = socket
        .receive_token(deadline, &cancel)?
        .ok_or_else(protocol_error)?;
    service.import_token_cancellable(&token, &cancel)
}

/// Runs the native WeChat login flow.
///
/// `on_qr` is called once, after the WebSocket is connected and before the QR
/// is polled. It receives a normalized module raster and must return promptly.
/// Cancellation is checked before any request, between requests, while waiting
/// for socket data, and again immediately before token verification and
/// persistence. Verification and persistence are delegated to
/// [`AuthService::import_token_cancellable`].
pub fn login_with_wechat<F, C>(
    service: &AuthService,
    mut endpoints: WechatLoginEndpoints,
    timeout: Duration,
    on_qr: F,
    cancel: C,
) -> Result<AuthStatus, AuthError>
where
    F: FnOnce(&WechatQr) -> Result<(), AuthError>,
    C: Fn() -> bool,
{
    if service.environment_token_active() {
        return Err(AuthError::input(
            "environment_token_active",
            "BOOXCTL_TOKEN 已生效；请清除该环境变量后重试",
        ));
    }
    let deadline = Instant::now() + Duration::from_secs(300);
    let nonce = Uuid::new_v4().to_string();
    let state = format!("{STATE_PREFIX}{nonce}");

    if cancel() {
        return Err(AuthError::cancelled());
    }

    let mut socket = WechatSocket::connect(
        &endpoints,
        &state,
        Instant::now() + timeout.min(remaining(deadline)),
        &cancel,
    )?;

    if cancel() {
        return Err(AuthError::cancelled());
    }
    socket.wait_for_room(
        &nonce,
        Instant::now() + timeout.min(remaining(deadline)),
        &cancel,
    )?;
    let client = WechatHttp {
        client: create_client(timeout.as_secs().max(1))
            .map_err(|_| AuthError::network("client_error", "无法创建微信登录 HTTP 客户端"))?,
        timeout,
    };

    let uuid = create_qr_uuid(&client, &mut endpoints, &nonce, deadline, &cancel)?;
    let image = fetch_qr_image(&client, &endpoints, &uuid, deadline, &cancel)?;
    let qr = decode_qr_image(&image)?;
    on_qr(&qr)?;

    let mut last: Option<String> = None;
    loop {
        if cancel() {
            return Err(AuthError::cancelled());
        }
        if Instant::now() >= deadline {
            return Err(AuthError::timeout("等待微信扫码登录超时"));
        }
        let poll = poll_longpoll(&client, &endpoints, &uuid, last.clone(), deadline, &cancel)?;
        match poll {
            LongPoll::Pending => {
                last = Some("408".into());
                let wait = remaining(deadline).min(Duration::from_secs(2));
                sleep_cancellable(wait, &cancel)?;
            }
            LongPoll::Scanned => {
                last = Some("404".into());
                let wait = remaining(deadline).min(Duration::from_millis(100));
                sleep_cancellable(wait, &cancel)?;
            }
            LongPoll::Cancelled => {
                return Err(AuthError::authentication(
                    "wechat_login_cancelled",
                    "微信登录已取消，请重新扫码",
                ))
            }
            LongPoll::Expired => {
                return Err(AuthError::authentication(
                    "wechat_login_expired",
                    "微信二维码已过期，请重新登录",
                ))
            }
            LongPoll::Confirmed { code } => {
                if cancel() {
                    return Err(AuthError::cancelled());
                }
                let status = callback_url(&client, &endpoints, &code, &state, deadline, &cancel)?;
                if !status.is_success() {
                    return Err(AuthError::network(
                        "wechat_callback_rejected",
                        format!("微信登录回调返回 HTTP {}", status.as_u16()),
                    ));
                }
                if let Some(token) = socket.receive_token(deadline, &cancel)? {
                    if cancel() {
                        return Err(AuthError::cancelled());
                    }
                    return service.import_token_cancellable(&token, &cancel);
                }
            }
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum LongPoll {
    Pending,
    Scanned,
    Confirmed { code: String },
    Cancelled,
    Expired,
}

fn remaining(deadline: Instant) -> Duration {
    deadline.saturating_duration_since(Instant::now())
}

fn remaining_secs(deadline: Instant) -> Result<u64, AuthError> {
    let duration = remaining(deadline);
    if duration.is_zero() {
        return Err(AuthError::timeout("等待微信扫码登录超时"));
    }
    Ok(duration.as_secs().max(1))
}

fn sleep_cancellable<C>(duration: Duration, cancel: &C) -> Result<(), AuthError>
where
    C: Fn() -> bool,
{
    let end = Instant::now() + duration;
    while Instant::now() < end {
        if cancel() {
            return Err(AuthError::cancelled());
        }
        let wait = end.saturating_duration_since(Instant::now()).min(READ_POLL);
        if !wait.is_zero() {
            thread::sleep(wait);
        }
    }
    Ok(())
}

struct WechatHttp {
    client: Client,
    timeout: Duration,
}

impl WechatHttp {
    fn get(&self, url: &str, deadline: Instant) -> reqwest::blocking::RequestBuilder {
        self.client
            .get(url)
            .timeout(self.timeout.min(remaining(deadline)))
    }
}

fn create_qr_uuid<C>(
    client: &WechatHttp,
    endpoints: &mut WechatLoginEndpoints,
    nonce: &str,
    deadline: Instant,
    cancel: &C,
) -> Result<String, AuthError>
where
    C: Fn() -> bool,
{
    let url = endpoints.qr_page_url(&format!("{STATE_PREFIX}{nonce}"), nonce);
    let text = http_get_text(client, &url, MAX_XML_BYTES, deadline, cancel)?;
    if !endpoints.loopback && text.contains("<usenewdomain>1</usenewdomain>") {
        endpoints.longpoll = "https://lp.open.weixin.qq.com/connect/l/qrconnect".into();
    }
    parse_qr_uuid(&text)
}

fn parse_qr_uuid(text: &str) -> Result<String, AuthError> {
    let start = text.find("<uuid>").ok_or_else(protocol_error)?;
    let rest = &text[start + "<uuid>".len()..];
    let end = rest.find("</uuid>").ok_or_else(protocol_error)?;
    let raw = rest[..end].trim();
    let raw = raw
        .strip_prefix("<![CDATA[")
        .and_then(|value| value.strip_suffix("]]>"))
        .unwrap_or(raw)
        .trim();
    if raw.is_empty() || raw.len() > 512 {
        return Err(protocol_error());
    }
    Ok(raw.to_string())
}

fn fetch_qr_image<C>(
    client: &WechatHttp,
    endpoints: &WechatLoginEndpoints,
    uuid: &str,
    deadline: Instant,
    cancel: &C,
) -> Result<Vec<u8>, AuthError>
where
    C: Fn() -> bool,
{
    let url = endpoints.image_url(uuid);
    http_get_bytes(client, &url, MAX_QR_IMAGE_BYTES, deadline, cancel)
}

fn http_get_text<C>(
    client: &WechatHttp,
    url: &str,
    limit: usize,
    deadline: Instant,
    cancel: &C,
) -> Result<String, AuthError>
where
    C: Fn() -> bool,
{
    let bytes = http_get_bytes(client, url, limit, deadline, cancel)?;
    String::from_utf8(bytes).map_err(|_| protocol_error())
}

fn http_get_bytes<C>(
    client: &WechatHttp,
    url: &str,
    limit: usize,
    deadline: Instant,
    cancel: &C,
) -> Result<Vec<u8>, AuthError>
where
    C: Fn() -> bool,
{
    if cancel() {
        return Err(AuthError::cancelled());
    }
    remaining_secs(deadline)?;
    let request = client.get(url, deadline);
    let response = request.send().map_err(map_http_error)?;
    let status = response.status();
    if !status.is_success() {
        return Err(AuthError::network(
            "http_error",
            format!("微信登录服务返回 HTTP {}", status.as_u16()),
        ));
    }
    let mut response = response;
    let mut body = Vec::new();
    let mut buffer = [0_u8; 8192];
    loop {
        if cancel() {
            return Err(AuthError::cancelled());
        }
        if Instant::now() >= deadline {
            return Err(AuthError::timeout("等待微信扫码登录超时"));
        }
        let read = response
            .read(&mut buffer)
            .map_err(|_| AuthError::network("network_error", "无法读取微信登录服务响应"))?;
        if read == 0 {
            break;
        }
        if body.len() + read > limit {
            return Err(AuthError::network(
                "response_too_large",
                "微信登录服务响应超出大小限制",
            ));
        }
        body.extend_from_slice(&buffer[..read]);
    }
    Ok(body)
}

fn map_http_error(error: reqwest::Error) -> AuthError {
    if error.is_timeout() {
        AuthError::timeout("微信登录服务请求超时")
    } else {
        AuthError::network("network_error", "无法完成微信登录服务请求")
    }
}

fn callback_url<C>(
    client: &WechatHttp,
    endpoints: &WechatLoginEndpoints,
    code: &str,
    state: &str,
    deadline: Instant,
    cancel: &C,
) -> Result<reqwest::StatusCode, AuthError>
where
    C: Fn() -> bool,
{
    if cancel() {
        return Err(AuthError::cancelled());
    }
    let url = endpoints.callback_url(code, state);
    remaining_secs(deadline)?;
    let response = client
        .get(&url, deadline)
        .header(CONTENT_TYPE, "application/json")
        .send()
        .map_err(map_http_error)?;
    Ok(response.status())
}

fn poll_longpoll<C>(
    client: &WechatHttp,
    endpoints: &WechatLoginEndpoints,
    uuid: &str,
    last: Option<String>,
    deadline: Instant,
    cancel: &C,
) -> Result<LongPoll, AuthError>
where
    C: Fn() -> bool,
{
    let url = endpoints.longpoll_url(uuid, last.as_deref());
    let text = http_get_text(client, &url, MAX_LONGPOLL_SCRIPT_BYTES, deadline, cancel)?;
    parse_longpoll(&text)
}

fn parse_longpoll(text: &str) -> Result<LongPoll, AuthError> {
    let code = parse_js_number(text, "wx_errcode").ok_or_else(protocol_error)?;
    let wx_code = parse_js_string(text, "wx_code");
    match code {
        405 => {
            let code = wx_code.ok_or_else(protocol_error)?;
            Ok(LongPoll::Confirmed { code })
        }
        408 => Ok(LongPoll::Pending),
        404 => Ok(LongPoll::Scanned),
        403 => Ok(LongPoll::Cancelled),
        402 | 500 => Ok(LongPoll::Expired),
        _ => Err(protocol_error()),
    }
}

fn parse_js_number(text: &str, name: &str) -> Option<i64> {
    let needle = name.to_string();
    let start = text.find(&needle)? + needle.len();
    let rest = text[start..].trim_start();
    let rest = rest.strip_prefix('=')?.trim_start();
    let digits: String = rest
        .chars()
        .take_while(|ch| ch.is_ascii_digit() || *ch == '-')
        .collect();
    digits.parse().ok()
}

fn parse_js_string(text: &str, name: &str) -> Option<String> {
    let needle = name.to_string();
    let start = text.find(&needle)? + needle.len();
    let rest = text[start..].trim_start();
    let rest = rest.strip_prefix('=')?.trim_start();
    let quote = rest.chars().next()?;
    if quote != '\'' && quote != '"' {
        return None;
    }
    let rest = &rest[quote.len_utf8()..];
    let end = rest.find(quote)?;
    let raw = &rest[..end];
    if raw.is_empty() || raw.len() > 4096 {
        return None;
    }
    Some(raw.replace("\\'", "'").replace("\\\"", "\""))
}

fn protocol_error() -> AuthError {
    AuthError::network("protocol_error", "微信登录服务返回了无法识别的响应")
}

struct WechatSocket {
    socket: WebSocket<MaybeTlsStream<TcpStream>>,
}

impl WechatSocket {
    fn connect<C>(
        endpoints: &WechatLoginEndpoints,
        state: &str,
        deadline: Instant,
        cancel: &C,
    ) -> Result<Self, AuthError>
    where
        C: Fn() -> bool,
    {
        if cancel() {
            return Err(AuthError::cancelled());
        }
        let url = endpoints.socket_url(state);
        let request = url
            .into_client_request()
            .map_err(|_| AuthError::network("socket_error", "无法建立微信登录连接"))?;
        let uri = request.uri().clone();
        let host = uri
            .host()
            .ok_or_else(|| AuthError::network("socket_error", "无法建立微信登录连接"))?;
        let port = uri.port_u16().unwrap_or(if uri.scheme_str() == Some("ws") {
            80
        } else {
            443
        });
        let addrs: Vec<_> = (host, port)
            .to_socket_addrs()
            .map_err(|_| AuthError::network("socket_error", "无法建立微信登录连接"))?
            .collect();
        if remaining(deadline).is_zero() {
            return Err(AuthError::timeout("等待微信扫码登录超时"));
        }
        let mut stream = None;
        for addr in addrs {
            if remaining(deadline).is_zero() {
                return Err(AuthError::timeout("等待微信扫码登录超时"));
            }
            let left = remaining(deadline)
                .min(Duration::from_secs(5))
                .max(Duration::from_millis(1));
            match TcpStream::connect_timeout(&addr, left) {
                Ok(value) => {
                    stream = Some(value);
                    break;
                }
                Err(_) if cancel() => return Err(AuthError::cancelled()),
                Err(_) => {}
            }
        }
        let stream =
            stream.ok_or_else(|| AuthError::network("socket_error", "无法建立微信登录连接"))?;
        let _ = stream.set_read_timeout(Some(remaining(deadline).max(Duration::from_millis(1))));
        let _ = stream.set_write_timeout(Some(
            remaining(deadline)
                .min(Duration::from_secs(5))
                .max(Duration::from_millis(1)),
        ));
        let config = tungstenite::protocol::WebSocketConfig {
            max_message_size: Some(MAX_SOCKET_MESSAGE_BYTES),
            max_frame_size: Some(MAX_SOCKET_MESSAGE_BYTES),
            ..Default::default()
        };
        let (mut socket, _) =
            tungstenite::client_tls_with_config(request, stream, Some(config), None)
                .map_err(|_| AuthError::network("socket_error", "无法建立微信登录连接"))?;
        match socket.get_mut() {
            MaybeTlsStream::Plain(stream) => stream.set_read_timeout(Some(SOCKET_WAIT)),
            MaybeTlsStream::Rustls(stream) => stream.sock.set_read_timeout(Some(SOCKET_WAIT)),
            _ => return Err(protocol_error()),
        }
        .map_err(|_| protocol_error())?;
        Ok(Self { socket })
    }

    fn wait_for_room<C: Fn() -> bool>(
        &mut self,
        nonce: &str,
        deadline: Instant,
        cancel: &C,
    ) -> Result<(), AuthError> {
        let text = self.receive_text(deadline, cancel)?;
        let value: Value = serde_json::from_str(&text).map_err(|_| protocol_error())?;
        if value["action"] != "message"
            || value["room"] != nonce
            || value["message"] != format!("{nonce} joined!")
        {
            return Err(protocol_error());
        }
        Ok(())
    }

    fn receive_token<C: Fn() -> bool>(
        &mut self,
        deadline: Instant,
        cancel: &C,
    ) -> Result<Option<String>, AuthError> {
        let text = self.receive_text(deadline, cancel)?;
        token_from_socket(&text)
    }

    fn receive_text<C>(&mut self, deadline: Instant, cancel: &C) -> Result<String, AuthError>
    where
        C: Fn() -> bool,
    {
        loop {
            if cancel() {
                return Err(AuthError::cancelled());
            }
            if Instant::now() >= deadline {
                return Err(AuthError::timeout("等待微信扫码登录超时"));
            }
            match self.socket.read() {
                Ok(Message::Text(text)) => {
                    if text.len() > MAX_SOCKET_MESSAGE_BYTES {
                        return Err(protocol_error());
                    }
                    return Ok(text);
                }
                Ok(Message::Binary(bytes)) => {
                    if bytes.len() > MAX_SOCKET_MESSAGE_BYTES {
                        return Err(protocol_error());
                    }
                    let text = String::from_utf8(bytes).map_err(|_| protocol_error())?;
                    return Ok(text);
                }
                Ok(Message::Ping(payload)) => {
                    self.socket
                        .send(Message::Pong(payload))
                        .map_err(|_| AuthError::network("socket_error", "微信登录连接已中断"))?;
                }
                Ok(Message::Pong(_)) | Ok(Message::Frame(_)) => {}
                Ok(Message::Close(_)) => {
                    return Err(AuthError::network("socket_closed", "微信登录连接已关闭"))
                }
                Err(tungstenite::Error::Io(err))
                    if matches!(
                        err.kind(),
                        std::io::ErrorKind::WouldBlock
                            | std::io::ErrorKind::TimedOut
                            | std::io::ErrorKind::Interrupted
                    ) => {}
                Err(_) => return Err(AuthError::network("socket_error", "微信登录连接已中断")),
            }
        }
    }
}

fn token_from_socket(text: &str) -> Result<Option<String>, AuthError> {
    let value: Value = match serde_json::from_str(text) {
        Ok(value) => value,
        Err(_) => return Err(protocol_error()),
    };
    if value.get("action").and_then(Value::as_str) != Some("pc_login_finish") {
        return Err(protocol_error());
    }
    let token = value
        .get("data")
        .and_then(|data| data.get("token"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .ok_or_else(protocol_error)?;
    Ok(normalize_optional(Some(token.to_string())))
}

fn decode_qr_image(bytes: &[u8]) -> Result<WechatQr, AuthError> {
    let pixels = decode_image_gray(bytes)?;
    modules_from_gray(pixels)
}

#[derive(Debug)]
struct GrayImage {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

fn decode_image_gray(bytes: &[u8]) -> Result<GrayImage, AuthError> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
        let limits = png::Limits {
            bytes: 16 * 1024 * 1024,
        };
        decoder.set_limits(limits);
        decoder.set_transformations(png::Transformations::normalize_to_color8());
        let mut reader = decoder.read_info().map_err(|_| image_error())?;
        let info = reader.info();
        check_dimensions(info.width, info.height)?;
        let mut buffer = vec![0; reader.output_buffer_size()];
        let output = reader.next_frame(&mut buffer).map_err(|_| image_error())?;
        let (width, height) = (output.width, output.height);
        check_dimensions(width, height)?;
        let data = &buffer[..output.buffer_size()];
        let pixels = match output.color_type {
            png::ColorType::Grayscale => data.to_vec(),
            png::ColorType::GrayscaleAlpha => data.chunks(2).map(|chunk| chunk[0]).collect(),
            png::ColorType::Rgb => data.chunks(3).map(luminance).collect(),
            png::ColorType::Rgba => data.chunks(4).map(|chunk| luminance(&chunk[..3])).collect(),
            _ => return Err(image_error()),
        };
        Ok(GrayImage {
            width,
            height,
            pixels,
        })
    } else if bytes.starts_with(b"\xff\xd8") {
        let mut decoder = jpeg_decoder::Decoder::new(std::io::Cursor::new(bytes));
        decoder.set_max_decoding_buffer_size(16 * 1024 * 1024);
        decoder.read_info().map_err(|_| image_error())?;
        let info = decoder.info().ok_or_else(image_error)?;
        let (width, height) = (u32::from(info.width), u32::from(info.height));
        check_dimensions(width, height)?;
        let data = decoder.decode().map_err(|_| image_error())?;
        let pixels = match info.pixel_format {
            jpeg_decoder::PixelFormat::L8 => data,
            jpeg_decoder::PixelFormat::L16 => data.chunks(2).map(|chunk| chunk[0]).collect(),
            jpeg_decoder::PixelFormat::RGB24 => data.chunks(3).map(luminance).collect(),
            jpeg_decoder::PixelFormat::CMYK32 => {
                return Err(image_error());
            }
        };
        Ok(GrayImage {
            width,
            height,
            pixels,
        })
    } else {
        Err(image_error())
    }
}

fn check_dimensions(width: u32, height: u32) -> Result<(), AuthError> {
    if width != height
        || !(MIN_QR_DIMENSION..=MAX_QR_DIMENSION).contains(&width)
        || (width as usize) * (height as usize) > 4 * 1024 * 1024
    {
        return Err(image_error());
    }
    Ok(())
}

fn luminance(rgb: &[u8]) -> u8 {
    let total = 299 * u32::from(rgb[0]) + 587 * u32::from(rgb[1]) + 114 * u32::from(rgb[2]);
    (total / 1000) as u8
}

fn image_error() -> AuthError {
    AuthError::network("qr_image_invalid", "微信登录二维码图片无效")
}

fn modules_from_gray(image: GrayImage) -> Result<WechatQr, AuthError> {
    let width = image.width as usize;
    let height = image.height as usize;
    let threshold = otsu_threshold(&image.pixels)?;
    let dark = |x: usize, y: usize| image.pixels[y * width + x] < threshold;
    let mut runs = Vec::new();
    for y in 0..height {
        collect_runs((0..width).map(|x| dark(x, y)), &mut runs);
    }
    for x in 0..width {
        collect_runs((0..height).map(|y| dark(x, y)), &mut runs);
    }
    let scale = gcd_all(&runs);
    if scale == 0 {
        return Err(image_error());
    }
    let modules = width / scale;
    if !width.is_multiple_of(scale)
        || !height.is_multiple_of(scale)
        || modules != height / scale
        || !(MIN_QR_MODULES as usize..=MAX_QR_MODULES as usize).contains(&modules)
    {
        return Err(image_error());
    }
    let mut raster = Vec::with_capacity(modules);
    for row in 0..modules {
        let mut output = Vec::with_capacity(modules);
        for col in 0..modules {
            let x = col * scale + scale / 2;
            let y = row * scale + scale / 2;
            output.push(dark(x, y));
        }
        raster.push(output);
    }
    Ok(WechatQr { modules: raster })
}

fn collect_runs<I>(values: I, runs: &mut Vec<usize>)
where
    I: IntoIterator<Item = bool>,
{
    let mut current = None;
    let mut length = 0usize;
    for value in values {
        if current == Some(value) {
            length += 1;
        } else {
            if length > 0 {
                runs.push(length);
            }
            current = Some(value);
            length = 1;
        }
    }
    if length > 0 {
        runs.push(length);
    }
}

fn gcd_all(values: &[usize]) -> usize {
    values.iter().fold(0, |acc, &value| gcd(acc, value))
}

fn gcd(a: usize, b: usize) -> usize {
    if b == 0 {
        a
    } else {
        gcd(b, a % b)
    }
}

fn otsu_threshold(pixels: &[u8]) -> Result<u8, AuthError> {
    let mut histogram = [0usize; 256];
    for &pixel in pixels {
        histogram[pixel as usize] += 1;
    }
    let total = pixels.len();
    if total == 0 {
        return Err(image_error());
    }
    let sum: usize = histogram
        .iter()
        .enumerate()
        .map(|(value, &count)| value * count)
        .sum();
    let mut sum_b = 0usize;
    let mut weight_b = 0usize;
    let mut best = 0usize;
    let mut best_variance = 0f64;
    for (value, count) in histogram.iter().enumerate() {
        weight_b += count;
        if weight_b == 0 {
            continue;
        }
        let weight_f = total - weight_b;
        if weight_f == 0 {
            break;
        }
        sum_b += value * count;
        let mean_b = sum_b as f64 / weight_b as f64;
        let mean_f = (sum - sum_b) as f64 / weight_f as f64;
        let variance = weight_b as f64 * weight_f as f64 * (mean_b - mean_f).powi(2);
        if variance > best_variance {
            best_variance = variance;
            best = value;
        }
    }
    if best_variance == 0.0 || best == 255 {
        return Err(image_error());
    }
    let upper = histogram
        .iter()
        .enumerate()
        .skip(best + 1)
        .find(|(_, count)| **count > 0)
        .map(|(i, _)| i)
        .ok_or_else(image_error)?;
    Ok(((best + upper).div_ceil(2)) as u8)
}
