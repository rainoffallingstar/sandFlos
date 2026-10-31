use crate::api::{
    check_qr_login_at, create_client, create_qr_login_at, send_verify_code_with_endpoints,
    signup_by_phone_or_email_with_endpoints, ApiEndpoints,
};
use crate::models::{PhoneOrEmailLoginRequest, QrCheckResponse, VerifyCodeRequest};
use crate::state::{set_auth_state, LoginPortalState};
use crate::util::{normalize_optional, unix_ms_now};
use qrcodegen::{QrCode, QrCodeEcc};
use std::{
    collections::HashMap,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, OnceLock,
    },
    thread,
    time::{Duration, Instant},
};
use uuid::Uuid;

const LOGIN_PORTAL_TTL_MS: u128 = 10 * 60 * 1000;
const WECHAT_LOGIN_APP_ID: &str = "wxdef46042a22e2e89";
const WECHAT_LOGIN_SCOPE: &str = "snsapi_login";
const WECHAT_LOGIN_REDIRECT_URI: &str = "https://send2boox.com/api/1/passport/wechat/pc/login";
const WECHAT_LOGIN_SOCKET_URL: &str = "wss://send2boox.com/ws/socketio/";
const WECHAT_LOGIN_STATE_PREFIX: &str = "bind_login_";
const PORTAL_READ_POLL: Duration = Duration::from_millis(50);
const PORTAL_REQUEST_HEAD_DEADLINE: Duration = Duration::from_secs(2);
const PORTAL_WRITE_TIMEOUT: Duration = Duration::from_millis(250);

/// Owned portal retained by the legacy `start_login_flow` adapter. Replacing
/// this entry joins the previous portal before the new one becomes visible.
fn legacy_portal_holder() -> &'static Mutex<Option<LoginPortal>> {
    static HOLDER: OnceLock<Mutex<Option<LoginPortal>>> = OnceLock::new();
    HOLDER.get_or_init(|| Mutex::new(None))
}

/// Owned browser-login portal.
///
/// The application supplies the URL opener and the token delivery callback.
/// The callback is the only successful-login delivery path and is invoked at
/// most once unless it returns an error, in which case a browser retry may
/// deliver the same token again.
///
/// Token callbacks run synchronously on the portal worker and must return
/// within a short bounded duration. Shutdown joins that worker, so a callback
/// that blocks indefinitely also blocks shutdown; callbacks must not wait for
/// unbounded persistence or network work.
///
/// Dropping or calling [`LoginPortal::shutdown`] stops the listener and joins
/// the worker. Portal-owned request reads and response writes have short
/// deadlines and repeatedly observe the stop flag. An upstream login request
/// already in progress is bounded only by its existing 20-second HTTP timeout.
pub struct LoginPortal {
    port: u16,
    expected_state: String,
    stop: Arc<AtomicBool>,
    delivery: Arc<TokenDelivery>,
    worker: Option<thread::JoinHandle<()>>,
}

impl LoginPortal {
    pub fn start<F, D>(open_url: F, deliver_token: D) -> Result<Self, String>
    where
        F: Fn(&str) -> Result<(), String> + Send + Sync + 'static,
        D: Fn(String) -> Result<(), String> + Send + Sync + 'static,
    {
        Self::start_configured(open_url, deliver_token, ApiEndpoints::production(), false)
    }

    /// Starts a portal against explicit service origins. Applications normally
    /// use [`LoginPortal::start`]; this variant exists for callers that need a
    /// non-default deployment and for loopback integration tests.
    pub fn start_with_endpoints<F, D>(
        open_url: F,
        deliver_token: D,
        endpoints: ApiEndpoints,
    ) -> Result<Self, String>
    where
        F: Fn(&str) -> Result<(), String> + Send + Sync + 'static,
        D: Fn(String) -> Result<(), String> + Send + Sync + 'static,
    {
        Self::start_configured(open_url, deliver_token, endpoints, false)
    }

    fn start_configured<F, D>(
        open_url: F,
        deliver_token: D,
        endpoints: ApiEndpoints,
        clear_global_on_close: bool,
    ) -> Result<Self, String>
    where
        F: Fn(&str) -> Result<(), String> + Send + Sync + 'static,
        D: Fn(String) -> Result<(), String> + Send + Sync + 'static,
    {
        let login_nonce = Uuid::new_v4().to_string();
        let state = build_wechat_login_state(&login_nonce);
        let listener = TcpListener::bind("127.0.0.1:0").map_err(|err| err.to_string())?;
        listener
            .set_nonblocking(true)
            .map_err(|err| err.to_string())?;
        let port = listener.local_addr().map_err(|err| err.to_string())?.port();

        let state_for_server = state.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let stop_for_worker = stop.clone();
        let delivery = Arc::new(TokenDelivery::new(deliver_token));
        let delivery_for_worker = delivery.clone();
        let worker = thread::spawn(move || {
            serve_login_portal(
                listener,
                state_for_server,
                delivery_for_worker,
                stop_for_worker,
                endpoints,
                clear_global_on_close,
            )
        });

        let bridge_url = format!("http://127.0.0.1:{port}/auth/start?state={state}");
        if let Err(err) = open_url(&bridge_url) {
            delivery.request_stop(&stop);
            let _ = worker.join();
            return Err(err);
        }
        Ok(Self {
            port,
            expected_state: state,
            stop,
            delivery,
            worker: Some(worker),
        })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn bridge_url(&self) -> String {
        format!(
            "http://127.0.0.1:{}/auth/start?state={}",
            self.port, self.expected_state
        )
    }

    pub fn shutdown(self) {
        drop(self);
    }

    fn stop_and_join(&mut self) {
        // Serialize the stop flag with delivery. A callback already running is
        // allowed to finish (its bounded return is part of the public
        // contract), but no new callback can begin after this store.
        self.delivery.request_stop(&self.stop);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl Drop for LoginPortal {
    fn drop(&mut self) {
        self.stop_and_join();
    }
}

type TokenCallback = dyn Fn(String) -> Result<(), String> + Send + Sync + 'static;

struct TokenDelivery {
    callback: Box<TokenCallback>,
    delivered: AtomicBool,
    gate: Mutex<()>,
}

impl TokenDelivery {
    fn request_stop(&self, stop: &AtomicBool) {
        let _guard = self.gate.lock();
        stop.store(true, Ordering::Release);
    }

    fn new<D>(callback: D) -> Self
    where
        D: Fn(String) -> Result<(), String> + Send + Sync + 'static,
    {
        Self {
            callback: Box::new(callback),
            delivered: AtomicBool::new(false),
            gate: Mutex::new(()),
        }
    }

    /// Delivers a token unless an earlier delivery already succeeded. A failed
    /// callback remains retryable; callback error text is intentionally
    /// discarded so it cannot reach the browser response.
    fn deliver(&self, token: String, stop: &AtomicBool) -> Result<(), ()> {
        let _guard = self.gate.lock().map_err(|_| ())?;
        if stop.load(Ordering::Acquire) {
            return Err(());
        }
        if self.delivered.load(Ordering::Acquire) {
            return Ok(());
        }
        (self.callback)(token).map_err(|_| ())?;
        self.delivered.store(true, Ordering::Release);
        Ok(())
    }
}

/// Compatibility entry point that keeps the old global-auth behavior.
pub fn start_login_flow<F>(open_url: F) -> Result<(), String>
where
    F: Fn(&str) -> Result<(), String> + Send + Sync + 'static,
{
    let portal = LoginPortal::start_configured(
        open_url,
        |token| {
            set_auth_state(Some(token));
            Ok(())
        },
        ApiEndpoints::production(),
        true,
    )?;
    let state = portal.expected_state.clone();
    let port = portal.port;
    let previous = legacy_portal_holder()
        .lock()
        .map_err(|_| "登录服务状态不可用".to_string())?
        .replace(portal);
    // Joining happens outside the holder lock. The old worker may clear the
    // old global metadata, so publish the replacement metadata afterwards.
    drop(previous);
    if let Ok(mut login_portal) = crate::state::global_state().login_portal.lock() {
        *login_portal = Some(LoginPortalState {
            _state: state,
            _port: port,
            _started_ms: unix_ms_now(),
        });
    }
    Ok(())
}

fn serve_login_portal(
    listener: TcpListener,
    expected_state: String,
    delivery: Arc<TokenDelivery>,
    stop: Arc<AtomicBool>,
    endpoints: ApiEndpoints,
    clear_global_on_close: bool,
) {
    let started_at = unix_ms_now();
    loop {
        if stop.load(Ordering::Acquire)
            || unix_ms_now().saturating_sub(started_at) > LOGIN_PORTAL_TTL_MS
        {
            if clear_global_on_close {
                clear_login_portal();
            }
            return;
        }

        match listener.accept() {
            Ok((stream, _)) => {
                if stop.load(Ordering::Acquire) {
                    if clear_global_on_close {
                        clear_login_portal();
                    }
                    return;
                }
                handle_connection(
                    stream,
                    &expected_state,
                    &delivery,
                    stop.as_ref(),
                    &endpoints,
                    clear_global_on_close,
                );
            }
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(10));
            }
            Err(_) => {
                if clear_global_on_close {
                    clear_login_portal();
                }
                return;
            }
        }
    }
}

fn handle_connection(
    mut stream: TcpStream,
    expected_state: &str,
    delivery: &TokenDelivery,
    stop: &AtomicBool,
    endpoints: &ApiEndpoints,
    clear_global_on_close: bool,
) {
    let _ = stream.set_read_timeout(Some(PORTAL_READ_POLL));
    let _ = stream.set_write_timeout(Some(PORTAL_WRITE_TIMEOUT));
    let request = match read_request_head(&mut stream, stop) {
        Ok(text) => text,
        Err(_) => return,
    };
    if stop.load(Ordering::Acquire) {
        return;
    }
    let Some(first_line) = request.lines().next() else {
        return;
    };
    let mut parts = first_line.split_whitespace();
    let method = parts.next().unwrap_or_default();
    let target = parts.next().unwrap_or_default();
    if method != "GET" {
        write_response(
            &mut stream,
            "405 Method Not Allowed",
            "text/plain; charset=utf-8",
            "method not allowed".to_string(),
        );
        return;
    }

    let (path, query) = split_target(target);
    let params = parse_query(query);
    if !request_has_valid_state(path, &params, expected_state) {
        write_response(
            &mut stream,
            "400 Bad Request",
            "text/plain; charset=utf-8",
            "invalid state".to_string(),
        );
        return;
    }

    match path {
        "/auth/start" => write_response(
            &mut stream,
            "200 OK",
            "text/html; charset=utf-8",
            login_page_html(expected_state),
        ),
        "/auth/qrcode/create" => {
            let body = match create_client(20)
                .and_then(|client| create_qr_login_at(&client, endpoints.api_base()))
            {
                Ok(data) => match render_qr_svg(&data.qrcode_data) {
                    Ok(qrcode_svg) => serde_json::json!({
                        "ok": true,
                        "qrcodeId": data.qrcode_id,
                        "qrcodeData": data.qrcode_data,
                        "qrcodeSvg": qrcode_svg,
                    })
                    .to_string(),
                    Err(err) => serde_json::json!({ "ok": false, "error": err }).to_string(),
                },
                Err(err) => serde_json::json!({ "ok": false, "error": err }).to_string(),
            };
            write_response(
                &mut stream,
                "200 OK",
                "application/json; charset=utf-8",
                body,
            );
        }
        "/auth/qrcode/check" => {
            let qrcode_id = params.get("qrcodeId").cloned().unwrap_or_default();
            let body = if qrcode_id.trim().is_empty() {
                serde_json::json!({ "ok": false, "error": "missing qrcodeId" }).to_string()
            } else {
                match create_client(20)
                    .and_then(|client| check_qr_login_at(&client, endpoints.api_base(), &qrcode_id))
                {
                    Ok(result) => {
                        if result.status == 1 {
                            if let Some(token) = extract_qr_login_token(&result) {
                                if delivery.deliver(token, stop).is_err() {
                                    return write_response(
                                        &mut stream,
                                        "200 OK",
                                        "application/json; charset=utf-8",
                                        serde_json::json!({
                                            "ok": false,
                                            "error": "无法保存登录凭据，请重试"
                                        })
                                        .to_string(),
                                    );
                                }
                            } else {
                                return write_response(
                                    &mut stream,
                                    "200 OK",
                                    "application/json; charset=utf-8",
                                    serde_json::json!({
                                        "ok": false,
                                        "error": "扫码登录成功，但未返回有效 token"
                                    })
                                    .to_string(),
                                );
                            }
                        }
                        serde_json::json!({
                            "ok": true,
                            "status": result.status,
                        })
                        .to_string()
                    }
                    Err(err) => serde_json::json!({ "ok": false, "error": err }).to_string(),
                }
            };
            write_response(
                &mut stream,
                "200 OK",
                "application/json; charset=utf-8",
                body,
            );
        }
        "/auth/wechat/callback" => {
            let token = normalized_query_value(&params, "token").unwrap_or_default();
            let body = if token.is_empty() {
                serde_json::json!({
                    "ok": false,
                    "error": "missing token"
                })
                .to_string()
            } else if delivery.deliver(token, stop).is_err() {
                serde_json::json!({ "ok": false, "error": "无法保存登录凭据，请重试" }).to_string()
            } else {
                serde_json::json!({ "ok": true }).to_string()
            };
            write_response(
                &mut stream,
                "200 OK",
                "application/json; charset=utf-8",
                body,
            );
        }
        "/auth/login/send-code" => {
            let body = match handle_send_code_request(&params, endpoints) {
                Ok(()) => serde_json::json!({ "ok": true }).to_string(),
                Err(err) => serde_json::json!({ "ok": false, "error": err }).to_string(),
            };
            write_response(
                &mut stream,
                "200 OK",
                "application/json; charset=utf-8",
                body,
            );
        }
        "/auth/login/submit" => {
            let body = match handle_phone_or_email_login(&params, delivery, stop, endpoints) {
                Ok(()) => serde_json::json!({ "ok": true }).to_string(),
                Err(err) => serde_json::json!({ "ok": false, "error": err }).to_string(),
            };
            write_response(
                &mut stream,
                "200 OK",
                "application/json; charset=utf-8",
                body,
            );
        }
        "/auth/callback" => {
            if clear_global_on_close {
                clear_login_portal();
            }

            write_response(
                &mut stream,
                "200 OK",
                "text/html; charset=utf-8",
                callback_page_html(),
            );
        }
        "/favicon.ico" => {
            write_response(&mut stream, "204 No Content", "image/x-icon", String::new())
        }
        _ => write_response(
            &mut stream,
            "404 Not Found",
            "text/plain; charset=utf-8",
            "not found".to_string(),
        ),
    }
}

fn clear_login_portal() {
    if let Ok(mut login_portal) = crate::state::global_state().login_portal.lock() {
        *login_portal = None;
    }
}

fn read_request_head(stream: &mut TcpStream, stop: &AtomicBool) -> Result<String, String> {
    let deadline = Instant::now() + PORTAL_REQUEST_HEAD_DEADLINE;
    let mut buffer = [0_u8; 4096];
    let mut request = Vec::new();
    loop {
        if stop.load(Ordering::Acquire) {
            return Err("portal stopped".to_string());
        }
        match stream.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => request.extend_from_slice(&buffer[..read]),
            Err(err)
                if matches!(
                    err.kind(),
                    std::io::ErrorKind::WouldBlock
                        | std::io::ErrorKind::TimedOut
                        | std::io::ErrorKind::Interrupted
                ) =>
            {
                if Instant::now() >= deadline {
                    return Err("request head deadline exceeded".to_string());
                }
                continue;
            }
            Err(err) => return Err(err.to_string()),
        }
        if request.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
        if request.len() > 32 * 1024 {
            break;
        }
        if Instant::now() >= deadline {
            return Err("request head deadline exceeded".to_string());
        }
    }
    String::from_utf8(request).map_err(|err| err.to_string())
}

fn write_response(stream: &mut TcpStream, status: &str, content_type: &str, body: String) {
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\n\r\n{}",
        body.len(),
        body
    );
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
}

fn split_target(target: &str) -> (&str, &str) {
    match target.split_once('?') {
        Some((path, query)) => (path, query),
        None => (target, ""),
    }
}

fn request_has_valid_state(
    path: &str,
    params: &HashMap<String, String>,
    expected_state: &str,
) -> bool {
    path == "/favicon.ico" || params.get("state").map(String::as_str) == Some(expected_state)
}

fn extract_qr_login_token(result: &QrCheckResponse) -> Option<String> {
    normalize_optional(
        result
            .user_info
            .as_ref()
            .and_then(|item| item.token.clone()),
    )
}

fn render_qr_svg(content: &str) -> Result<String, String> {
    let qr = QrCode::encode_text(content, QrCodeEcc::Medium).map_err(|err| err.to_string())?;
    Ok(qrcode_to_svg_string(&qr, 4))
}

fn qrcode_path_data(qr: &QrCode, border: i32) -> String {
    let mut path = String::new();
    for y in 0..qr.size() {
        for x in 0..qr.size() {
            if qr.get_module(x, y) {
                path.push_str(&format!("M{},{}h1v1h-1z ", x + border, y + border));
            }
        }
    }
    path.trim_end().to_string()
}

fn qrcode_svg_viewbox(qr: &QrCode, border: i32) -> i32 {
    qr.size() + border * 2
}

fn qrcode_to_svg_string(qr: &QrCode, border: i32) -> String {
    let viewbox = qrcode_svg_viewbox(qr, border);
    let path = qrcode_path_data(qr, border);
    format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" version=\"1.1\" viewBox=\"0 0 {viewbox} {viewbox}\" shape-rendering=\"crispEdges\" aria-hidden=\"true\"><rect width=\"100%\" height=\"100%\" fill=\"#ffffff\"/><path d=\"{path}\" fill=\"#111827\"/></svg>"
    )
}

fn parse_query(query: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for pair in query.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        let key = urlencoding::decode(key)
            .map(|value| value.into_owned())
            .unwrap_or_default();
        let value = urlencoding::decode(value)
            .map(|value| value.into_owned())
            .unwrap_or_default();
        out.insert(key, value);
    }
    out
}

fn build_wechat_login_state(login_nonce: &str) -> String {
    format!("{WECHAT_LOGIN_STATE_PREFIX}{login_nonce}")
}

fn normalized_query_value(params: &HashMap<String, String>, key: &str) -> Option<String> {
    params
        .get(key)
        .cloned()
        .and_then(|value| normalize_optional(Some(value)))
}

fn normalized_area_code(value: Option<String>) -> Option<String> {
    normalize_optional(value).map(|value| {
        if value.starts_with('+') {
            value
        } else {
            format!("+{value}")
        }
    })
}

fn verify_code_request_from_params(
    params: &HashMap<String, String>,
) -> Result<VerifyCodeRequest, String> {
    let mode = normalized_query_value(params, "mode").unwrap_or_else(|| "phone".to_string());
    let mobi = normalized_query_value(params, "mobi").ok_or_else(|| "缺少账号信息".to_string())?;
    let verify = normalized_query_value(params, "verify")
        .ok_or_else(|| "图形验证码校验结果缺失，请重新获取验证码".to_string())?;
    let scene =
        normalized_query_value(params, "scene").unwrap_or_else(|| "nc_register_web".to_string());
    let area_code = if mode == "phone" {
        normalized_area_code(normalized_query_value(params, "areaCode").or(Some("86".to_string())))
    } else {
        None
    };
    Ok(VerifyCodeRequest {
        mobi,
        area_code,
        verify,
        scene,
    })
}

fn login_request_from_params(
    params: &HashMap<String, String>,
) -> Result<PhoneOrEmailLoginRequest, String> {
    let mode = normalized_query_value(params, "mode").unwrap_or_else(|| "phone".to_string());
    let mobi = normalized_query_value(params, "mobi").ok_or_else(|| "缺少账号信息".to_string())?;
    let code = normalized_query_value(params, "code").ok_or_else(|| "缺少验证码".to_string())?;
    let area_code = if mode == "phone" {
        normalized_area_code(normalized_query_value(params, "areaCode").or(Some("86".to_string())))
    } else {
        None
    };
    Ok(PhoneOrEmailLoginRequest {
        mobi,
        area_code,
        code,
    })
}

fn handle_send_code_request(
    params: &HashMap<String, String>,
    endpoints: &ApiEndpoints,
) -> Result<(), String> {
    let payload = verify_code_request_from_params(params)?;
    let client = create_client(20)?;
    send_verify_code_with_endpoints(endpoints, &client, &payload).map_err(|error| error.to_string())
}

fn handle_phone_or_email_login(
    params: &HashMap<String, String>,
    delivery: &TokenDelivery,
    stop: &AtomicBool,
    endpoints: &ApiEndpoints,
) -> Result<(), String> {
    let payload = login_request_from_params(params)?;
    let client = create_client(20)?;
    let token = signup_by_phone_or_email_with_endpoints(endpoints, &client, &payload)?;
    delivery
        .deliver(token, stop)
        .map_err(|_| "无法保存登录凭据，请重试".to_string())
}

fn wechat_socket_state(state: &str) -> &str {
    state
        .strip_prefix(WECHAT_LOGIN_STATE_PREFIX)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(state)
}

fn login_page_html(state: &str) -> String {
    let wechat_login_url = build_wechat_login_url(state);
    let wechat_ws_state = wechat_socket_state(state);
    format!(
        r##"<!doctype html>
<html lang="zh-CN">
  <head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <title>Send2Boox 登录授权</title>
    <script defer src="https://g.alicdn.com/AWSC/AWSC/awsc.js"></script>
    <script defer src="https://o.alicdn.com/captcha-frontend/aliyunCaptcha/AliyunCaptcha.js"></script>
    <style>
      :root {{
        color-scheme: light;
        --bg: #edf3ef;
        --panel: rgba(255, 255, 255, 0.92);
        --ink: #0f172a;
        --muted: #5b6475;
        --accent: #07c160;
        --accent-soft: rgba(7, 193, 96, 0.12);
        --line: rgba(15, 23, 42, 0.08);
        --shadow: 0 24px 80px rgba(21, 33, 56, 0.12);
      }}
      * {{ box-sizing: border-box; }}
      body {{
        margin: 0;
        font-family: "SF Pro Display", "PingFang SC", "Helvetica Neue", sans-serif;
        background:
          radial-gradient(circle at top left, rgba(7, 193, 96, 0.18) 0%, transparent 30%),
          radial-gradient(circle at right center, rgba(88, 214, 141, 0.14) 0%, transparent 24%),
          linear-gradient(180deg, #f8fbf9 0%, var(--bg) 100%);
        color: var(--ink);
        min-height: 100vh;
        display: grid;
        place-items: center;
        padding: 24px;
      }}
      .panel {{
        width: min(920px, 100%);
        background: var(--panel);
        border: 1px solid var(--line);
        border-radius: 28px;
        box-shadow: var(--shadow);
        padding: 28px;
        backdrop-filter: blur(16px);
      }}
      .eyebrow {{
        margin: 0 0 8px;
        color: var(--accent);
        font-size: 12px;
        font-weight: 700;
        letter-spacing: 0.12em;
        text-transform: uppercase;
      }}
      h1 {{
        margin: 0;
        font-size: 30px;
        line-height: 1.1;
      }}
      .desc {{
        margin: 10px 0 24px;
        color: var(--muted);
        line-height: 1.6;
      }}
      .shell {{
        display: grid;
        gap: 18px;
        padding: 22px;
        border-radius: 22px;
        background: linear-gradient(180deg, #ffffff 0%, #f8fbfa 100%);
        border: 1px solid rgba(7, 193, 96, 0.12);
      }}
      .chooser {{
        display: grid;
        grid-template-columns: repeat(auto-fit, minmax(200px, 1fr));
        gap: 16px;
      }}
      .option-card {{
        border: 1px solid rgba(15, 23, 42, 0.08);
        border-radius: 20px;
        padding: 18px;
        background: #fff;
        text-align: left;
      }}
      .option-card[data-active="true"] {{
        border-color: rgba(7, 193, 96, 0.35);
        box-shadow: 0 18px 44px rgba(7, 193, 96, 0.12);
      }}
      .option-card h2 {{
        margin: 0 0 8px;
        font-size: 20px;
      }}
      .option-card p {{
        margin: 0 0 14px;
        color: var(--muted);
        line-height: 1.6;
      }}
      .actions {{
        display: flex;
        gap: 12px;
        flex-wrap: wrap;
      }}
      .field-actions {{
        display: flex;
        gap: 12px;
        flex-wrap: wrap;
        margin-top: 8px;
      }}
      .btn {{
        appearance: none;
        border: 0;
        border-radius: 999px;
        padding: 12px 18px;
        font-size: 14px;
        font-weight: 600;
        cursor: pointer;
        text-decoration: none;
      }}
      .btn-primary {{
        background: var(--accent);
        color: #fff;
      }}
      .btn-secondary {{
        background: rgba(15, 23, 42, 0.06);
        color: var(--ink);
      }}
      .btn-ghost {{
        background: transparent;
        color: var(--muted);
        border: 1px solid rgba(15, 23, 42, 0.08);
      }}
      .fields {{
        display: grid;
        gap: 14px;
      }}
      .grid-2 {{
        display: grid;
        grid-template-columns: 120px minmax(0, 1fr);
        gap: 12px;
      }}
      .field-label {{
        font-size: 13px;
        color: var(--muted);
      }}
      .input {{
        width: 100%;
        padding: 12px 14px;
        border-radius: 14px;
        border: 1px solid rgba(15, 23, 42, 0.10);
        font-size: 14px;
        color: var(--ink);
        background: #fff;
      }}
      .code-row {{
        display: grid;
        grid-template-columns: minmax(0, 1fr) auto;
        gap: 12px;
      }}
      .flow[hidden] {{
        display: none;
      }}
      .qr-box {{
        min-height: 220px;
        display: grid;
        place-items: center;
        border-radius: 20px;
        border: 1px dashed rgba(15, 23, 42, 0.12);
        background: rgba(248, 251, 250, 0.8);
        padding: 16px;
      }}
      .qr-box svg {{
        width: min(220px, 100%);
        height: auto;
      }}
      .status {{
        padding: 12px 14px;
        border-radius: 14px;
        background: var(--accent-soft);
        color: #0c6b39;
        font-size: 14px;
        width: 100%;
      }}
      .tips {{
        margin: 18px 0 0;
        padding-left: 18px;
        color: var(--muted);
        line-height: 1.8;
      }}
      .footer {{
        margin-top: 18px;
        color: var(--muted);
        font-size: 13px;
      }}
      .captcha-overlay {{
        position: fixed;
        inset: 0;
        display: none;
        align-items: center;
        justify-content: center;
        padding: 24px;
        background: rgba(15, 23, 42, 0.32);
        z-index: 9999;
      }}
      .captcha-overlay[data-open="true"] {{
        display: flex;
      }}
      .captcha-card {{
        width: min(420px, 100%);
        border-radius: 24px;
        background: rgba(255,255,255,0.98);
        box-shadow: var(--shadow);
        padding: 22px;
      }}
      .captcha-title {{
        margin: 0 0 12px;
        font-size: 22px;
      }}
      .captcha-desc {{
        margin: 0 0 16px;
        color: var(--muted);
        line-height: 1.6;
      }}
      .loading {{
        color: var(--muted);
        font-size: 13px;
      }}
      code {{
        background: rgba(17, 24, 39, 0.06);
        border-radius: 8px;
        padding: 2px 6px;
      }}
    </style>
  </head>
  <body>
    <main class="panel">
      <p class="eyebrow">Send2Boox</p>
      <h1>选择登录与授权方式</h1>
      <p class="desc">现在支持微信扫码、BOOX 助手扫码、手机号验证码登录、邮箱验证码登录。所有登录成功后都会把授权状态同步回桌面端，并自动回到仪表盘。</p>

      <section id="chooser" class="shell">
        <div class="chooser">
          <article class="option-card" data-mode="wechat">
            <h2>微信扫码</h2>
            <p>在默认浏览器中打开官网微信二维码页，用微信完成扫码与确认。</p>
            <button id="wechat-start-btn" class="btn btn-primary" type="button">使用微信扫码</button>
          </article>
          <article class="option-card" data-mode="boox">
            <h2>BOOX 助手扫码</h2>
            <p>在当前桥接页直接生成 BOOX 助手二维码，用 App 内扫一扫完成授权。</p>
            <button id="boox-start-btn" class="btn btn-secondary" type="button">使用 BOOX 助手</button>
          </article>
          <article class="option-card" data-mode="phone">
            <h2>手机号登录</h2>
            <p>通过短信验证码登录，获取验证码前会先完成官网同源图形安全校验。</p>
            <button id="phone-start-btn" class="btn btn-secondary" type="button">使用手机号</button>
          </article>
          <article class="option-card" data-mode="email">
            <h2>邮箱登录</h2>
            <p>通过邮箱验证码登录，获取验证码前同样需要完成官网同源图形安全校验。</p>
            <button id="email-start-btn" class="btn btn-secondary" type="button">使用邮箱</button>
          </article>
        </div>
      </section>

      <section id="wechat-flow" class="shell flow" hidden>
        <div class="actions">
          <button id="wechat-open-btn" class="btn btn-primary" type="button">打开微信登录页</button>
          <button data-back class="btn btn-ghost" type="button">返回方式选择</button>
        </div>
        <div id="wechat-status" class="status">等待打开官网微信二维码页...</div>
      </section>

      <section id="boox-flow" class="shell flow" hidden>
        <div class="actions">
          <button id="boox-refresh-btn" class="btn btn-secondary" type="button">刷新 BOOX 助手二维码</button>
          <button data-back class="btn btn-ghost" type="button">返回方式选择</button>
        </div>
        <div id="boox-status" class="status">准备生成 BOOX 助手扫码二维码...</div>
        <div id="boox-qr" class="qr-box">二维码加载中...</div>
      </section>

      <section id="phone-flow" class="shell flow" hidden>
        <div class="actions">
          <button id="phone-send-btn" class="btn btn-secondary" type="button">获取短信验证码</button>
          <button data-back class="btn btn-ghost" type="button">返回方式选择</button>
        </div>
        <div id="phone-status" class="status">请输入手机号并获取验证码。</div>
        <div class="fields">
          <div>
            <div class="field-label">国家/地区区号与手机号</div>
            <div class="grid-2">
              <input id="phone-area-code" class="input" type="text" value="+86" inputmode="numeric" />
              <input id="phone-mobi" class="input" type="text" placeholder="请输入手机号" inputmode="numeric" />
            </div>
          </div>
          <div>
            <div class="field-label">短信验证码</div>
            <div class="code-row">
              <input id="phone-code" class="input" type="text" placeholder="请输入短信验证码" inputmode="numeric" />
              <button id="phone-send-inline-btn" class="btn btn-ghost" type="button">发送验证码</button>
            </div>
          </div>
          <div class="field-actions">
            <button id="phone-login-btn" class="btn btn-primary" type="button">登录并同步桌面端</button>
          </div>
        </div>
      </section>

      <section id="email-flow" class="shell flow" hidden>
        <div class="actions">
          <button id="email-send-btn" class="btn btn-secondary" type="button">获取邮箱验证码</button>
          <button data-back class="btn btn-ghost" type="button">返回方式选择</button>
        </div>
        <div id="email-status" class="status">请输入邮箱并获取验证码。</div>
        <div class="fields">
          <div>
            <div class="field-label">邮箱地址</div>
            <input id="email-mobi" class="input" type="email" placeholder="请输入邮箱地址" />
          </div>
          <div>
            <div class="field-label">邮箱验证码</div>
            <div class="code-row">
              <input id="email-code" class="input" type="text" placeholder="请输入邮箱验证码" inputmode="numeric" />
              <button id="email-send-inline-btn" class="btn btn-ghost" type="button">发送验证码</button>
            </div>
          </div>
          <div class="field-actions">
            <button id="email-login-btn" class="btn btn-primary" type="button">登录并同步桌面端</button>
          </div>
        </div>
      </section>

      <ol class="tips">
        <li>微信扫码会在默认浏览器中打开官网二维码页，BOOX 助手扫码会在当前页直接展示二维码。</li>
        <li>手机号和邮箱登录在发送验证码前，会先执行官方 Aliyun 图形安全校验。</li>
        <li>登录成功后，这个桥接页会自动把状态同步回桌面端；若二维码失效，可以在当前页重新打开或刷新。</li>
      </ol>
      <p class="footer">安全校验字段：<code>{state}</code></p>
    </main>
    <div id="captcha-overlay" class="captcha-overlay" data-open="false" aria-hidden="true">
      <div class="captcha-card">
        <h2 class="captcha-title">安全验证</h2>
        <p class="captcha-desc">发送验证码前需要完成一次官网同源图形安全校验。</p>
        <div id="captcha-element"></div>
        <div id="captcha-button"></div>
        <p id="captcha-loading" class="loading">正在加载安全验证组件...</p>
      </div>
    </div>
    <script>
      const state = {state:?};
      const wechatSocketState = {wechat_ws_state:?};
      const chooserEl = document.getElementById("chooser");
      const wechatFlowEl = document.getElementById("wechat-flow");
      const booxFlowEl = document.getElementById("boox-flow");
      const phoneFlowEl = document.getElementById("phone-flow");
      const emailFlowEl = document.getElementById("email-flow");
      const wechatStatusEl = document.getElementById("wechat-status");
      const booxStatusEl = document.getElementById("boox-status");
      const phoneStatusEl = document.getElementById("phone-status");
      const emailStatusEl = document.getElementById("email-status");
      const booxQrEl = document.getElementById("boox-qr");
      const wechatStartBtn = document.getElementById("wechat-start-btn");
      const wechatOpenBtn = document.getElementById("wechat-open-btn");
      const booxStartBtn = document.getElementById("boox-start-btn");
      const booxRefreshBtn = document.getElementById("boox-refresh-btn");
      const phoneStartBtn = document.getElementById("phone-start-btn");
      const emailStartBtn = document.getElementById("email-start-btn");
      const phoneSendBtn = document.getElementById("phone-send-btn");
      const phoneSendInlineBtn = document.getElementById("phone-send-inline-btn");
      const phoneLoginBtn = document.getElementById("phone-login-btn");
      const emailSendBtn = document.getElementById("email-send-btn");
      const emailSendInlineBtn = document.getElementById("email-send-inline-btn");
      const emailLoginBtn = document.getElementById("email-login-btn");
      const phoneAreaCodeInput = document.getElementById("phone-area-code");
      const phoneMobiInput = document.getElementById("phone-mobi");
      const phoneCodeInput = document.getElementById("phone-code");
      const emailMobiInput = document.getElementById("email-mobi");
      const emailCodeInput = document.getElementById("email-code");
      const captchaOverlayEl = document.getElementById("captcha-overlay");
      const captchaLoadingEl = document.getElementById("captcha-loading");
      const chooserCards = document.querySelectorAll(".option-card[data-mode]");
      const backButtons = document.querySelectorAll("[data-back]");
      let ws = null;
      let booxPollTimer = null;
      let booxQrcodeId = "";
      let captchaInstance = null;
      const countdownTimers = {{}};
      const countdownValues = {{ phone: 0, email: 0 }};

      function setWechatStatus(text) {{
        wechatStatusEl.textContent = text;
      }}

      function setBooxStatus(text) {{
        booxStatusEl.textContent = text;
      }}

      function setPhoneStatus(text) {{
        phoneStatusEl.textContent = text;
      }}

      function setEmailStatus(text) {{
        emailStatusEl.textContent = text;
      }}

      function setActiveCard(mode) {{
        chooserCards.forEach((card) => {{
          card.dataset.active = card.dataset.mode === mode ? "true" : "false";
        }});
      }}

      function stopWechatSocket() {{
        if (ws) {{
          try {{ ws.close(); }} catch (_) {{}}
          ws = null;
        }}
      }}

      function stopBooxPolling() {{
        if (booxPollTimer) {{
          clearInterval(booxPollTimer);
          booxPollTimer = null;
        }}
      }}

      function showChooser() {{
        stopWechatSocket();
        stopBooxPolling();
        chooserEl.hidden = false;
        wechatFlowEl.hidden = true;
        booxFlowEl.hidden = true;
        phoneFlowEl.hidden = true;
        emailFlowEl.hidden = true;
        setActiveCard("");
      }}

      function showWechatFlow() {{
        chooserEl.hidden = true;
        wechatFlowEl.hidden = false;
        booxFlowEl.hidden = true;
        phoneFlowEl.hidden = true;
        emailFlowEl.hidden = true;
        setActiveCard("wechat");
      }}

      function showBooxFlow() {{
        chooserEl.hidden = true;
        wechatFlowEl.hidden = true;
        booxFlowEl.hidden = false;
        phoneFlowEl.hidden = true;
        emailFlowEl.hidden = true;
        setActiveCard("boox");
      }}

      function showPhoneFlow() {{
        chooserEl.hidden = true;
        wechatFlowEl.hidden = true;
        booxFlowEl.hidden = true;
        phoneFlowEl.hidden = false;
        emailFlowEl.hidden = true;
        setActiveCard("phone");
      }}

      function showEmailFlow() {{
        chooserEl.hidden = true;
        wechatFlowEl.hidden = true;
        booxFlowEl.hidden = true;
        phoneFlowEl.hidden = true;
        emailFlowEl.hidden = false;
        setActiveCard("email");
      }}

      async function notifyDesktopToken(token) {{
        const res = await fetch(`/auth/wechat/callback?state=${{encodeURIComponent(state)}}&token=${{encodeURIComponent(token)}}`);
        const data = await res.json();
        if (!data.ok) {{
          throw new Error(data.error || "桌面端回调失败");
        }}
      }}

      function openWechatLoginPage() {{
        window.open({wechat_login_url:?}, "_blank", "noopener,noreferrer");
      }}

      function normalizeAreaCode(value) {{
        const trimmed = String(value || "").trim();
        if (!trimmed) return "+86";
        return trimmed.startsWith("+") ? trimmed : `+${{trimmed}}`;
      }}

      function isPhoneValid() {{
        return /^\d{{5,20}}$/.test(String(phoneMobiInput.value || "").trim());
      }}

      function isEmailValid() {{
        return /^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(String(emailMobiInput.value || "").trim());
      }}

      function isCodeValid(value) {{
        return /^\d{{4,10}}$/.test(String(value || "").trim());
      }}

      function cleanupCaptchaDom() {{
        document.getElementById("aliyunCaptcha-mask")?.remove();
        document.getElementById("aliyunCaptcha-window-popup")?.remove();
      }}

      function closeCaptchaOverlay() {{
        if (captchaInstance && typeof captchaInstance.destroyCaptcha === "function") {{
          try {{ captchaInstance.destroyCaptcha(); }} catch (_) {{}}
        }}
        captchaInstance = null;
        cleanupCaptchaDom();
        captchaOverlayEl.dataset.open = "false";
        captchaOverlayEl.setAttribute("aria-hidden", "true");
      }}

      async function runAliyunCaptcha() {{
        captchaOverlayEl.dataset.open = "true";
        captchaOverlayEl.setAttribute("aria-hidden", "false");
        captchaLoadingEl.textContent = "正在加载安全验证组件...";
        cleanupCaptchaDom();
        return new Promise((resolve, reject) => {{
          const init = window.initAliyunCaptcha;
          if (typeof init !== "function") {{
            closeCaptchaOverlay();
            reject(new Error("安全验证脚本尚未准备完成，请稍后重试"));
            return;
          }}
          try {{
            init({{
              region: navigator.language && navigator.language.toLowerCase().startsWith("zh") ? "cn" : "sgp",
              prefix: "1nxo4o",
              SceneId: "1etq8wxz",
              mode: "embed",
              element: "#captcha-element",
              button: "#captcha-button",
              slideStyle: {{ width: 360, height: 40 }},
              language: navigator.language && navigator.language.toLowerCase().startsWith("zh") ? "cn" : "en",
              immediate: true,
              captchaVerifyCallback: (verifyValue) => {{
                closeCaptchaOverlay();
                resolve(verifyValue);
              }},
              getInstance: (instance) => {{
                captchaInstance = instance;
                captchaLoadingEl.textContent = "请完成安全验证";
              }},
              onError: (error) => {{
                if (error && error.code === "INIT_FAIL") {{
                  closeCaptchaOverlay();
                  reject(new Error("安全验证初始化失败，请稍后重试"));
                }}
              }}
            }});
          }} catch (error) {{
            closeCaptchaOverlay();
            reject(error);
          }}
        }});
      }}

      async function portalGet(path, params) {{
        const query = new URLSearchParams({{ state }});
        Object.entries(params || {{}}).forEach(([key, value]) => {{
          if (value !== undefined && value !== null && String(value).trim() !== "") {{
            query.set(key, String(value));
          }}
        }});
        const res = await fetch(`${{path}}?${{query.toString()}}`);
        const data = await res.json();
        if (!data.ok) {{
          throw new Error(data.error || "请求失败");
        }}
        return data;
      }}

      function setCountdown(mode, seconds) {{
        countdownValues[mode] = seconds;
        if (countdownTimers[mode]) {{
          clearInterval(countdownTimers[mode]);
          countdownTimers[mode] = null;
        }}
        const buttons = mode === "phone"
          ? [phoneSendBtn, phoneSendInlineBtn]
          : [emailSendBtn, emailSendInlineBtn];
        const label = mode === "phone" ? "发送验证码" : "发送验证码";
        const update = () => {{
          const disabled = countdownValues[mode] > 0;
          buttons.forEach((button) => {{
            if (!button) return;
            button.disabled = disabled;
            button.textContent = disabled ? `重新发送（${{countdownValues[mode]}}）` : label;
          }});
        }};
        update();
        if (seconds > 0) {{
          countdownTimers[mode] = setInterval(() => {{
            countdownValues[mode] = Math.max(0, countdownValues[mode] - 1);
            update();
            if (countdownValues[mode] === 0) {{
              clearInterval(countdownTimers[mode]);
              countdownTimers[mode] = null;
            }}
          }}, 1000);
        }}
      }}

      async function sendPhoneCode() {{
        if (!isPhoneValid()) {{
          throw new Error("请输入正确的手机号");
        }}
        setPhoneStatus("正在拉起安全验证...");
        const verify = await runAliyunCaptcha();
        setPhoneStatus("安全验证通过，正在发送短信验证码...");
        await portalGet("/auth/login/send-code", {{
          mode: "phone",
          mobi: phoneMobiInput.value.trim(),
          areaCode: normalizeAreaCode(phoneAreaCodeInput.value),
          verify,
          scene: "nc_register_web"
        }});
        setPhoneStatus("短信验证码已发送，请查看手机短信并完成登录。");
        setCountdown("phone", 60);
      }}

      async function sendEmailCode() {{
        if (!isEmailValid()) {{
          throw new Error("请输入正确的邮箱地址");
        }}
        setEmailStatus("正在拉起安全验证...");
        const verify = await runAliyunCaptcha();
        setEmailStatus("安全验证通过，正在发送邮箱验证码...");
        await portalGet("/auth/login/send-code", {{
          mode: "email",
          mobi: emailMobiInput.value.trim(),
          verify,
          scene: "nc_register_web"
        }});
        setEmailStatus("邮箱验证码已发送，请查看邮箱并完成登录。");
        setCountdown("email", 60);
      }}

      async function submitPhoneLogin() {{
        if (!isPhoneValid()) {{
          throw new Error("请输入正确的手机号");
        }}
        if (!isCodeValid(phoneCodeInput.value)) {{
          throw new Error("请输入正确的短信验证码");
        }}
        setPhoneStatus("正在登录并同步桌面端...");
        await portalGet("/auth/login/submit", {{
          mode: "phone",
          mobi: phoneMobiInput.value.trim(),
          areaCode: normalizeAreaCode(phoneAreaCodeInput.value),
          code: phoneCodeInput.value.trim()
        }});
        window.location.replace(`/auth/callback?state=${{encodeURIComponent(state)}}`);
      }}

      async function submitEmailLogin() {{
        if (!isEmailValid()) {{
          throw new Error("请输入正确的邮箱地址");
        }}
        if (!isCodeValid(emailCodeInput.value)) {{
          throw new Error("请输入正确的邮箱验证码");
        }}
        setEmailStatus("正在登录并同步桌面端...");
        await portalGet("/auth/login/submit", {{
          mode: "email",
          mobi: emailMobiInput.value.trim(),
          code: emailCodeInput.value.trim()
        }});
        window.location.replace(`/auth/callback?state=${{encodeURIComponent(state)}}`);
      }}

      function connectWechatSocket() {{
        stopWechatSocket();
        ws = new WebSocket("{WECHAT_LOGIN_SOCKET_URL}?state=" + encodeURIComponent(wechatSocketState));
        ws.onopen = () => {{
          setWechatStatus("官网微信二维码页已打开，请扫码并在手机上确认登录...");
        }};
        ws.onmessage = async (event) => {{
          try {{
            const payload = JSON.parse(event.data);
            if (payload && payload.action === "pc_login_finish" && payload.data && payload.data.token) {{
              setWechatStatus("登录成功，正在同步到桌面端...");
              if (ws) ws.close();
              await notifyDesktopToken(payload.data.token);
              window.location.replace(`/auth/callback?state=${{encodeURIComponent(state)}}`);
            }}
          }} catch (err) {{
            console.warn("wechat login message error", err);
          }}
        }};
        ws.onerror = () => {{
          setWechatStatus("微信登录桥接连接失败，请返回重新选择或再次打开官网二维码。");
        }};
        ws.onclose = () => {{
          if (!wechatStatusEl.textContent.includes("登录成功")) {{
            setWechatStatus("正在等待微信扫码确认...");
          }}
        }};
      }}

      async function pollBooxQrStatus() {{
        if (!booxQrcodeId) return;
        const res = await fetch(`/auth/qrcode/check?state=${{encodeURIComponent(state)}}&qrcodeId=${{encodeURIComponent(booxQrcodeId)}}`);
        const data = await res.json();
        if (!data.ok) {{
          throw new Error(data.error || "BOOX 助手扫码状态检查失败");
        }}
        if (data.status === 1) {{
          stopBooxPolling();
          setBooxStatus("登录成功，正在同步到桌面端...");
          window.location.replace(`/auth/callback?state=${{encodeURIComponent(state)}}`);
        }} else if (data.status === -1) {{
          stopBooxPolling();
          setBooxStatus("二维码已过期，请刷新 BOOX 助手二维码。");
        }} else {{
          setBooxStatus("请打开 BOOX 助手，在“我的”页顶部使用扫一扫完成登录...");
        }}
      }}

      async function loadBooxQr() {{
        stopBooxPolling();
        showBooxFlow();
        booxQrEl.textContent = "二维码加载中...";
        setBooxStatus("正在生成 BOOX 助手扫码二维码...");
        const res = await fetch(`/auth/qrcode/create?state=${{encodeURIComponent(state)}}`);
        const data = await res.json();
        if (!data.ok) {{
          throw new Error(data.error || "BOOX 助手二维码生成失败");
        }}
        booxQrcodeId = data.qrcodeId || "";
        booxQrEl.innerHTML = data.qrcodeSvg || "";
        setBooxStatus("请打开 BOOX 助手，在“我的”页顶部使用扫一扫完成登录...");
        booxPollTimer = setInterval(async () => {{
          try {{
            await pollBooxQrStatus();
          }} catch (err) {{
            stopBooxPolling();
            setBooxStatus(String(err));
          }}
        }}, 1500);
      }}

      wechatStartBtn?.addEventListener("click", () => {{
        showWechatFlow();
        setWechatStatus("正在打开官网微信二维码页...");
        openWechatLoginPage();
        connectWechatSocket();
      }});

      wechatOpenBtn?.addEventListener("click", () => {{
        openWechatLoginPage();
        setWechatStatus("官网微信二维码页已重新打开，请扫码并在手机上确认登录...");
      }});

      booxStartBtn?.addEventListener("click", async () => {{
        try {{
          await loadBooxQr();
        }} catch (err) {{
          setBooxStatus(String(err));
        }}
      }});

      booxRefreshBtn?.addEventListener("click", async () => {{
        try {{
          await loadBooxQr();
        }} catch (err) {{
          setBooxStatus(String(err));
        }}
      }});

      phoneStartBtn?.addEventListener("click", () => {{
        showPhoneFlow();
        setPhoneStatus("请输入手机号并获取验证码。");
      }});

      emailStartBtn?.addEventListener("click", () => {{
        showEmailFlow();
        setEmailStatus("请输入邮箱并获取验证码。");
      }});

      [phoneSendBtn, phoneSendInlineBtn].forEach((button) => {{
        button?.addEventListener("click", async () => {{
          try {{
            await sendPhoneCode();
          }} catch (err) {{
            setPhoneStatus(String(err));
          }}
        }});
      }});

      [emailSendBtn, emailSendInlineBtn].forEach((button) => {{
        button?.addEventListener("click", async () => {{
          try {{
            await sendEmailCode();
          }} catch (err) {{
            setEmailStatus(String(err));
          }}
        }});
      }});

      phoneLoginBtn?.addEventListener("click", async () => {{
        try {{
          await submitPhoneLogin();
        }} catch (err) {{
          setPhoneStatus(String(err));
        }}
      }});

      emailLoginBtn?.addEventListener("click", async () => {{
        try {{
          await submitEmailLogin();
        }} catch (err) {{
          setEmailStatus(String(err));
        }}
      }});

      backButtons.forEach((button) => {{
        button.addEventListener("click", () => {{
          showChooser();
        }});
      }});

      setCountdown("phone", 0);
      setCountdown("email", 0);
    </script>
  </body>
</html>"##
    )
}

fn build_wechat_login_url(state: &str) -> String {
    let redirect_uri = urlencoding::encode(WECHAT_LOGIN_REDIRECT_URI);
    format!(
        "https://open.weixin.qq.com/connect/qrconnect?appid={WECHAT_LOGIN_APP_ID}&scope={WECHAT_LOGIN_SCOPE}&redirect_uri={redirect_uri}&state={state}&login_type=jssdk&style=black&self_redirect=true"
    )
}

fn callback_page_html() -> String {
    r#"<!doctype html>
<html lang="zh-CN">
  <head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <title>Send2Boox 登录完成</title>
    <style>
      body {
        margin: 0;
        min-height: 100vh;
        display: grid;
        place-items: center;
        font-family: "SF Pro Display", "PingFang SC", sans-serif;
        background: linear-gradient(180deg, #f7fbff 0%, #eef6ff 100%);
        color: #0f172a;
      }
      .card {
        width: min(420px, 92vw);
        padding: 28px;
        border-radius: 24px;
        background: rgba(255,255,255,0.92);
        border: 1px solid rgba(15,23,42,0.08);
        box-shadow: 0 24px 70px rgba(15,23,42,0.10);
        text-align: center;
      }
      h1 { margin: 0 0 12px; font-size: 28px; }
      p { margin: 0; line-height: 1.6; color: #475569; }
    </style>
  </head>
  <body>
    <main class="card">
      <h1>登录完成</h1>
      <p>桌面端已接收到登录状态，你现在可以关闭这个浏览器标签页了。</p>
    </main>
    <script>
      setTimeout(() => {
        try {
          window.open("", "_self");
          window.close();
        } catch (_) {}
      }, 300);
      setTimeout(() => {
        try {
          window.location.replace("about:blank");
          window.close();
        } catch (_) {}
      }, 900);
    </script>
  </body>
</html>"#
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_target_separates_path_and_query() {
        assert_eq!(
            split_target("/auth/start?state=abc&from=desktop"),
            ("/auth/start", "state=abc&from=desktop")
        );
        assert_eq!(split_target("/auth/callback"), ("/auth/callback", ""));
    }

    #[test]
    fn parse_query_decodes_values() {
        let params = parse_query("state=abc123&note=hello%20world");
        assert_eq!(params.get("state").map(String::as_str), Some("abc123"));
        assert_eq!(params.get("note").map(String::as_str), Some("hello world"));
    }

    #[test]
    fn strips_wechat_login_prefix_for_socket_state() {
        assert_eq!(wechat_socket_state("bind_login_123456"), "123456");
        assert_eq!(wechat_socket_state("plain-state"), "plain-state");
        assert_eq!(wechat_socket_state("bind_login_"), "bind_login_");
    }

    #[test]
    fn request_state_validation_allows_favicon_and_matching_state() {
        let params = parse_query("state=match");
        assert!(request_has_valid_state("/auth/start", &params, "match"));
        assert!(request_has_valid_state(
            "/favicon.ico",
            &HashMap::new(),
            "match"
        ));
        assert!(!request_has_valid_state("/auth/start", &params, "other"));
    }

    #[test]
    fn builds_phone_verify_code_request_from_query_params() {
        let params = parse_query(
            "mode=phone&mobi=13800138000&areaCode=86&verify=token123&scene=nc_register_web",
        );
        let payload = verify_code_request_from_params(&params).expect("payload");
        assert_eq!(payload.mobi, "13800138000");
        assert_eq!(payload.area_code.as_deref(), Some("+86"));
        assert_eq!(payload.verify, "token123");
        assert_eq!(payload.scene, "nc_register_web");
    }

    #[test]
    fn builds_email_login_request_without_area_code() {
        let params = parse_query("mode=email&mobi=demo%40example.com&code=123456");
        let payload = login_request_from_params(&params).expect("payload");
        assert_eq!(payload.mobi, "demo@example.com");
        assert_eq!(payload.area_code, None);
        assert_eq!(payload.code, "123456");
    }

    #[test]
    fn extracts_non_empty_qr_login_token_only() {
        let ok = QrCheckResponse {
            status: 1,
            user_info: Some(crate::models::QrLoginUserInfo {
                token: Some(" token-value ".to_string()),
            }),
        };
        let blank = QrCheckResponse {
            status: 1,
            user_info: Some(crate::models::QrLoginUserInfo {
                token: Some("   ".to_string()),
            }),
        };
        let missing = QrCheckResponse {
            status: 1,
            user_info: None,
        };

        assert_eq!(extract_qr_login_token(&ok).as_deref(), Some("token-value"));
        assert_eq!(extract_qr_login_token(&blank), None);
        assert_eq!(extract_qr_login_token(&missing), None);
    }

    #[test]
    fn renders_qr_svg_markup() {
        let svg = render_qr_svg("https://send2boox.com").expect("qr svg");
        assert!(svg.contains("<svg"));
        assert!(svg.contains("</svg>"));
    }

    #[test]
    fn qr_svg_path_has_expected_offset() {
        let qr = QrCode::encode_text("hello", QrCodeEcc::Medium).expect("qr");
        let path = qrcode_path_data(&qr, 4);
        assert!(path.contains('M'));
        assert!(!path.contains("M0,0"));
        assert_eq!(qrcode_svg_viewbox(&qr, 4), qr.size() + 8);
    }

    #[test]
    fn login_page_contains_login_mode_choices() {
        let html = login_page_html("test-state");
        assert!(html.contains("open.weixin.qq.com/connect/qrconnect"));
        assert!(html.contains("使用微信扫码"));
        assert!(html.contains("使用 BOOX 助手"));
        assert!(html.contains("使用手机号"));
        assert!(html.contains("使用邮箱"));
        assert!(html.contains("/auth/qrcode/create"));
        assert!(html.contains("/auth/qrcode/check"));
        assert!(html.contains("/auth/login/send-code"));
        assert!(html.contains("/auth/login/submit"));
        assert!(html.contains(WECHAT_LOGIN_APP_ID));
        assert!(html.contains("/auth/wechat/callback"));
        assert!(html.contains(WECHAT_LOGIN_SOCKET_URL));
        assert!(html.contains("AliyunCaptcha.js"));
    }

    #[test]
    fn login_page_uses_prefixed_state_for_qr_and_raw_state_for_socket() {
        let html = login_page_html("bind_login_demo123");
        assert!(html.contains("bind_login_demo123"));
        assert!(html.contains("const wechatSocketState = \"demo123\";"));
    }

    #[test]
    fn login_page_renders_unescaped_validation_regexes() {
        let html = login_page_html("test-state");
        assert!(html.contains(r"/^\d{5,20}$/"));
        assert!(html.contains(r"/^[^\s@]+@[^\s@]+\.[^\s@]+$/"));
        assert!(html.contains(r"/^\d{4,10}$/"));
    }
}
