//! Public WeChat login flow over loopback HTTP and WebSocket fixtures.
//!
//! The fixture follows the official browser page's sequence (QR image, WeChat
//! long poll, BOOX callback, BOOX socket) without contacting any real service.

use boox_core::auth_service::{AppPaths, AuthErrorKind, AuthService, FileCredentialStore};
use boox_core::wechat_login::{login_with_wechat, WechatLoginEndpoints, WechatQr};
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
use tungstenite::{accept_hdr, Message};
use uuid::Uuid;

const QR_JPEG: &[u8] = include_bytes!("fixtures/wechat_qr.jpg");

struct FlowFixture {
    root: PathBuf,
    http: String,
    ws: String,
    events: Arc<Mutex<Vec<String>>>,
    http_worker: Option<JoinHandle<()>>,
    ws_worker: Option<JoinHandle<()>>,
    stop: Arc<AtomicBool>,
}

impl FlowFixture {
    fn start(qr_shown: Arc<AtomicBool>, socket_token: &'static str) -> Self {
        Self::scenario(qr_shown, socket_token, "ok")
    }

    fn scenario(
        qr_shown: Arc<AtomicBool>,
        socket_token: &'static str,
        scenario: &'static str,
    ) -> Self {
        let root = std::env::temp_dir().join(format!("booxctl-wechat-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let events = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));

        let http_listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
        http_listener.set_nonblocking(true).unwrap();
        let http = format!("http://{}", http_listener.local_addr().unwrap());
        let http_events = events.clone();
        let http_stop = stop.clone();
        let shown = qr_shown.clone();
        let http_worker = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(8);
            let mut polls = 0;
            while !http_stop.load(Ordering::SeqCst) && Instant::now() < deadline {
                let (mut stream, _) = match http_listener.accept() {
                    Ok(v) => v,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(2));
                        continue;
                    }
                    Err(_) => break,
                };
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                stream
                    .set_write_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let read_deadline = Instant::now() + Duration::from_secs(2);
                let mut request = Vec::new();
                while !request.ends_with(b"\r\n\r\n") && request.len() < 65536 {
                    let left = read_deadline.saturating_duration_since(Instant::now());
                    if left.is_zero() {
                        break;
                    }
                    stream.set_read_timeout(Some(left)).unwrap();
                    let mut byte = [0];
                    if stream.read(&mut byte).unwrap_or(0) == 0 {
                        break;
                    }
                    request.push(byte[0]);
                }
                if request.is_empty() {
                    continue;
                }
                let request = String::from_utf8(request).unwrap();
                let path = request.split_whitespace().nth(1).unwrap_or("/").to_string();
                http_events.lock().unwrap().push(path.clone());
                let (content_type, response): (&str, Vec<u8>) = if path
                    .contains("/connect/qrconnect")
                {
                    assert!(path.contains("f=xml"), "QR session request: {path}");
                    (
                        "text/xml",
                        br#"<xml><uuid><![CDATA[fixture-uuid]]></uuid><usenewdomain>0</usenewdomain></xml>"#.to_vec(),
                    )
                } else if path.contains("/connect/qrcode/") {
                    if scenario == "png" {
                        (
                            "image/png",
                            include_bytes!("fixtures/wechat_qr.png").to_vec(),
                        )
                    } else {
                        ("image/jpeg", QR_JPEG.to_vec())
                    }
                } else if path.starts_with("/connect/l/qrconnect?") {
                    assert!(shown.load(Ordering::SeqCst), "long poll before callback");
                    polls += 1;
                    let script = if scenario == "denied" {
                        "window.wx_errcode=403;window.wx_code='';"
                    } else if scenario == "expired" {
                        "window.wx_errcode=402;window.wx_code='';"
                    } else if scenario == "invalid_poll" {
                        "window.wx_errcode=405;window.wx_code='';"
                    } else if polls == 1 {
                        "window.wx_errcode=408;window.wx_code='';"
                    } else {
                        "window.wx_errcode=405;window.wx_code='synthetic-wx-code';"
                    };
                    ("application/javascript", script.as_bytes().to_vec())
                } else if path.contains("/api/1/passport/wechat/pc/login") {
                    assert!(path.contains("state=bind_login_"));
                    ("text/html", b"ok".to_vec())
                } else if path.starts_with("/api/1/users/me") {
                    (
                        "application/json",
                        br#"{"result_code":0,"data":{"uid":"synthetic-user"}}"#.to_vec(),
                    )
                } else {
                    ("text/plain", b"not found".to_vec())
                };
                let status = if scenario == "callback_failure" && path.contains("/passport/wechat/")
                {
                    "413 Rejected"
                } else {
                    "200 OK"
                };
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    response.len()
                );
                let _ = stream.write_all(&response);
            }
        });

        let ws_listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
        ws_listener.set_nonblocking(true).unwrap();
        let ws = format!("ws://{}", ws_listener.local_addr().unwrap());
        let ws_events = events.clone();
        let ws_stop = stop.clone();
        let ws_worker = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(8);
            while !ws_stop.load(Ordering::SeqCst) && Instant::now() < deadline {
                let (stream, _) = match ws_listener.accept() {
                    Ok(v) => v,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(2));
                        continue;
                    }
                    Err(_) => break,
                };
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                stream
                    .set_write_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut seen_target = String::new();
                // Error type is fixed by tungstenite::accept_hdr.
                #[allow(clippy::result_large_err)]
                let callback =
                    |request: &tungstenite::handshake::server::Request,
                     response: tungstenite::handshake::server::Response| {
                        seen_target = request.uri().to_string();
                        Ok(response)
                    };
                let mut socket = match accept_hdr(stream, callback) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                ws_events.lock().unwrap().push(format!("ws:{seen_target}"));
                let nonce = seen_target.split("state=").nth(1).unwrap();
                socket
                    .send(Message::Text(format!(
                        r#"{{"action":"message","message":"{nonce} joined!","room":"{nonce}"}}"#
                    )))
                    .unwrap();
                let payload = format!(
                    r#"{{"action":"pc_login_finish","data":{{"token":"{socket_token}"}}}}"#
                );
                let _ = socket.send(Message::Text(payload));
                let _ = socket.flush();
                thread::sleep(Duration::from_millis(50));
            }
        });

        Self {
            root,
            http,
            ws,
            events,
            http_worker: Some(http_worker),
            ws_worker: Some(ws_worker),
            stop,
        }
    }

    fn service(&self) -> AuthService {
        let paths = AppPaths::new(self.root.join("config"));
        AuthService::new(
            paths.clone(),
            Box::new(
                boox_core::auth_service::HttpAuthClient::with_base_url(
                    Duration::from_secs(1),
                    &self.http,
                )
                .unwrap(),
            ),
            Box::new(FileCredentialStore::new(&paths)),
            None,
        )
    }
}

impl Drop for FlowFixture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(self.http.trim_start_matches("http://"));
        if let Some(worker) = self.http_worker.take() {
            worker.join().expect("fixture worker");
        }
        if let Some(worker) = self.ws_worker.take() {
            worker.join().expect("fixture worker");
        }
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn wechat_flow_shows_decoded_qr_polls_callback_and_persists_socket_token() {
    let shown = Arc::new(AtomicBool::new(false));
    let fixture = FlowFixture::start(shown.clone(), "synthetic-wechat-token");
    let endpoints =
        WechatLoginEndpoints::loopback(&fixture.http, &fixture.ws).expect("loopback endpoints");
    let service = fixture.service();
    let callback_count = Arc::new(AtomicUsize::new(0));
    let count = callback_count.clone();
    let shown_by_callback = shown.clone();
    let status = login_with_wechat(
        &service,
        endpoints,
        Duration::from_secs(5),
        move |qr: &WechatQr| {
            count.fetch_add(1, Ordering::SeqCst);
            assert_eq!(qr.modules.len(), 47, "quiet zone plus 41 modules");
            assert_eq!(qr.modules[0].len(), 47);
            assert!(qr.modules[0].iter().all(|dark| !dark));
            assert!(qr.modules[3][3]);
            shown_by_callback.store(true, Ordering::SeqCst);
            Ok(())
        },
        || false,
    )
    .expect("wechat login");

    assert!(status.authorized);
    assert_eq!(callback_count.load(Ordering::SeqCst), 1);
    assert_eq!(
        service
            .resolve_token()
            .expect("resolve stored token")
            .expect("stored")
            .expose(),
        "synthetic-wechat-token"
    );
    let events = fixture.events.lock().unwrap().clone();
    assert_eq!(
        events
            .iter()
            .filter(|e| e.as_str() == "/connect/qrcode/fixture-uuid")
            .count(),
        1,
        "events: {events:?}"
    );
    assert!(events
        .iter()
        .any(|e| e.starts_with("/connect/l/qrconnect?")));
    assert!(events
        .iter()
        .any(|e| e.contains("/api/1/passport/wechat/pc/login")));
    assert!(events.iter().any(|e| e.contains("ws:/ws/socketio/?state=")));
    assert!(events.iter().any(|e| e.contains("state=bind_login_")));
}

#[test]
fn cancelled_wechat_login_never_requests_or_persists() {
    let shown = Arc::new(AtomicBool::new(false));
    let fixture = FlowFixture::start(shown.clone(), "must-not-persist");
    let endpoints =
        WechatLoginEndpoints::loopback(&fixture.http, &fixture.ws).expect("loopback endpoints");
    let service = fixture.service();
    let error = login_with_wechat(
        &service,
        endpoints,
        Duration::from_secs(5),
        |_| panic!("QR callback must not run after cancellation"),
        || true,
    )
    .unwrap_err();
    assert_eq!(error.kind, AuthErrorKind::Cancelled);
    assert!(fixture.events.lock().unwrap().is_empty());
    assert!(!service.status(false).unwrap().authorized);
}

#[test]
fn non_loopback_injected_endpoint_is_rejected() {
    let error =
        WechatLoginEndpoints::loopback("https://send2boox.com", "wss://send2boox.com/ws/socketio/")
            .unwrap_err();
    assert_eq!(error.kind, AuthErrorKind::Input);
    assert_eq!(error.code, "invalid_endpoint");
}

#[test]
fn environment_token_blocks_wechat_before_network() {
    let fixture = FlowFixture::start(Arc::new(AtomicBool::new(false)), "unused");
    let paths = AppPaths::new(fixture.root.join("config"));
    let service = AuthService::new(
        paths.clone(),
        Box::new(boox_core::auth_service::HttpAuthClient::new(
            Duration::from_secs(1),
        )),
        Box::new(FileCredentialStore::new(&paths)),
        Some("synthetic-env".into()),
    );
    let error = login_with_wechat(
        &service,
        WechatLoginEndpoints::loopback(&fixture.http, &fixture.ws).unwrap(),
        Duration::from_secs(1),
        |_| panic!("must reject before showing QR"),
        || false,
    )
    .unwrap_err();
    assert_eq!(error.code, "environment_token_active");
    assert!(fixture.events.lock().unwrap().is_empty());
}

#[test]
fn rejected_expired_invalid_callback_and_missing_token_never_persist() {
    for (scenario, token, expected) in [
        ("denied", "synthetic", "wechat_login_cancelled"),
        ("expired", "synthetic", "wechat_login_expired"),
        ("invalid_poll", "synthetic", "protocol_error"),
        ("callback_failure", "synthetic", "wechat_callback_rejected"),
        ("ok", "", "protocol_error"),
    ] {
        let shown = Arc::new(AtomicBool::new(false));
        let fixture = FlowFixture::scenario(shown.clone(), token, scenario);
        let service = fixture.service();
        let error = login_with_wechat(
            &service,
            WechatLoginEndpoints::loopback(&fixture.http, &fixture.ws).unwrap(),
            Duration::from_secs(5),
            |_| {
                shown.store(true, Ordering::SeqCst);
                Ok(())
            },
            || false,
        )
        .unwrap_err();
        assert_eq!(error.code, expected, "scenario: {scenario}");
        assert!(!service.status(false).unwrap().authorized);
        assert!(!fixture
            .events
            .lock()
            .unwrap()
            .iter()
            .any(|p| p.starts_with("/api/1/users/me")));
    }
}

#[test]
fn cancellation_after_display_stops_before_poll_and_save() {
    let shown = Arc::new(AtomicBool::new(false));
    let fixture = FlowFixture::start(shown.clone(), "synthetic");
    let service = fixture.service();
    let error = login_with_wechat(
        &service,
        WechatLoginEndpoints::loopback(&fixture.http, &fixture.ws).unwrap(),
        Duration::from_secs(5),
        |_| {
            shown.store(true, Ordering::SeqCst);
            Ok(())
        },
        || shown.load(Ordering::SeqCst),
    )
    .unwrap_err();
    assert_eq!(error.kind, AuthErrorKind::Cancelled);
    assert!(!service.status(false).unwrap().authorized);
    assert!(!fixture
        .events
        .lock()
        .unwrap()
        .iter()
        .any(|p| p.starts_with("/connect/l/")));
}

#[test]
fn png_qr_also_completes_login() {
    let shown = Arc::new(AtomicBool::new(false));
    let fixture = FlowFixture::scenario(shown.clone(), "synthetic-png-token", "png");
    let service = fixture.service();
    let status = login_with_wechat(
        &service,
        WechatLoginEndpoints::loopback(&fixture.http, &fixture.ws).unwrap(),
        Duration::from_secs(5),
        |qr| {
            assert_eq!(qr.width(), 47);
            assert_eq!(qr.height(), 47);
            assert!(qr.modules[3][3]);
            shown.store(true, Ordering::SeqCst);
            Ok(())
        },
        || false,
    )
    .unwrap();
    assert!(status.authorized);
}
