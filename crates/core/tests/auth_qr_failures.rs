//! QR failure acceptance through the public service and real loopback HTTP.
use boox_core::auth_service::{
    AppPaths, AuthErrorKind, AuthService, FileCredentialStore, HttpAuthClient, QrLoginOptions,
};
use std::{
    cell::Cell,
    io::{BufRead, BufReader, Read, Write},
    net::TcpListener,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

struct Fixture {
    root: PathBuf,
    endpoint: String,
    requests: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl Fixture {
    fn new(poll_status: &'static str, poll_body: Option<&'static str>) -> Self {
        Self::with_delay(poll_status, poll_body, Duration::ZERO)
    }

    fn with_delay(
        poll_status: &'static str,
        poll_body: Option<&'static str>,
        delay: Duration,
    ) -> Self {
        let root = std::env::temp_dir().join(format!("sandflos-qr-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let (observed, stopping) = (requests.clone(), stop.clone());
        let worker = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(6);
            while !stopping.load(Ordering::SeqCst) && Instant::now() < deadline {
                let (mut stream, _) = match listener.accept() {
                    Ok(connection) => connection,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
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
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut request = String::new();
                if reader.read_line(&mut request).is_err() {
                    continue;
                }
                let mut line = String::new();
                let mut content_length = 0;
                loop {
                    line.clear();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                        break;
                    }
                    if let Some((name, value)) = line.split_once(':') {
                        if name.eq_ignore_ascii_case("content-length") {
                            content_length = value.trim().parse::<usize>().unwrap();
                        }
                    }
                }
                let mut body = vec![0; content_length];
                let _ = reader.read_exact(&mut body);
                let create = request.starts_with("POST /api/1/auth/qrcode/create ");
                observed.lock().unwrap().push(request);
                let (status, body) = if create {
                    (
                        "200 OK",
                        Some(
                            r#"{"result_code":0,"data":{"qrcodeId":"fixture-qr","data":"fixture-payload"}}"#,
                        ),
                    )
                } else {
                    thread::sleep(delay);
                    (poll_status, poll_body)
                };
                if let Some(body) = body {
                    let _ = write!(stream,
                        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                }
            }
        });
        Self {
            root,
            endpoint,
            requests,
            stop,
            worker: Some(worker),
        }
    }

    fn service(&self) -> AuthService {
        let paths = AppPaths::new(self.root.join("config"));
        AuthService::new(
            paths.clone(),
            Box::new(
                HttpAuthClient::with_base_url(Duration::from_secs(1), &self.endpoint).unwrap(),
            ),
            Box::new(FileCredentialStore::new(&paths)),
            None,
        )
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        self.worker.take().unwrap().join().unwrap();
        std::fs::remove_dir_all(&self.root).unwrap();
    }
}

#[test]
fn qr_missing_token_rejection_and_lost_connection_never_persist_a_session() {
    for (status, body, kind, code) in [
        (
            "200 OK",
            Some(r#"{"result_code":0,"data":{"status":1,"userInfo":{}}}"#),
            AuthErrorKind::Authentication,
            "qr_token_missing",
        ),
        (
            "403 Forbidden",
            Some("upstream-secret-must-not-echo"),
            AuthErrorKind::Authentication,
            "credential_rejected",
        ),
        ("200 OK", None, AuthErrorKind::Network, "network_error"),
    ] {
        let fixture = Fixture::new(status, body);
        let service = fixture.service();
        let error = service
            .login_with_qr(
                QrLoginOptions {
                    poll_interval: Duration::ZERO,
                    timeout: Duration::from_secs(3),
                },
                |_| Ok(()),
                || false,
            )
            .unwrap_err();
        assert_eq!(error.kind, kind);
        assert_eq!(error.code, code);
        assert!(!error.message.contains("upstream-secret"));
        assert!(!service.status(false).unwrap().authorized);
        assert!(!fixture.root.join("config").exists());
    }
}

#[test]
fn qr_wait_deadline_does_not_poll_and_pending_cancellation_stops_the_next_poll() {
    let pending = r#"{"result_code":0,"data":{"status":0}}"#;
    let fixture = Fixture::new("200 OK", Some(pending));
    let service = fixture.service();
    let error = service
        .login_with_qr(
            QrLoginOptions {
                poll_interval: Duration::ZERO,
                timeout: Duration::ZERO,
            },
            |_| Ok(()),
            || false,
        )
        .unwrap_err();
    assert_eq!(error.kind, AuthErrorKind::Timeout);
    assert_eq!(fixture.requests.lock().unwrap().len(), 1);
    assert!(!service.status(false).unwrap().authorized);

    let fixture = Fixture::new("200 OK", Some(pending));
    let service = fixture.service();
    let checks = Cell::new(0);
    let error = service
        .login_with_qr(
            QrLoginOptions {
                poll_interval: Duration::ZERO,
                timeout: Duration::from_secs(3),
            },
            |_| Ok(()),
            || {
                checks.set(checks.get() + 1);
                checks.get() > 1
            },
        )
        .unwrap_err();
    assert_eq!(error.kind, AuthErrorKind::Cancelled);
    assert_eq!(error.kind.exit_code(), 130);
    assert_eq!(fixture.requests.lock().unwrap().len(), 2);
    assert!(!fixture.root.join("config").exists());
}

#[test]
fn qr_poll_http_timeout_is_bounded_and_does_not_persist() {
    let fixture = Fixture::with_delay(
        "200 OK",
        Some(r#"{"result_code":0,"data":{"status":0}}"#),
        Duration::from_millis(1500),
    );
    let service = fixture.service();
    let started = Instant::now();
    let error = service
        .login_with_qr(
            QrLoginOptions {
                poll_interval: Duration::ZERO,
                timeout: Duration::from_secs(3),
            },
            |_| Ok(()),
            || false,
        )
        .unwrap_err();
    assert_eq!(error.kind, AuthErrorKind::Timeout);
    assert_eq!(error.kind.exit_code(), 4);
    assert!(started.elapsed() < Duration::from_secs(5));
    assert!(!service.status(false).unwrap().authorized);
    assert!(!fixture.root.join("config").exists());
}
