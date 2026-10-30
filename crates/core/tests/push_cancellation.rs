use boox_core::models::{BucketConfig, OssSts, UploadAuthContext};
use boox_core::services::{
    HttpPushBackend, PushBackend, PushOutcome, PushService, ServiceError, ServiceErrorKind,
    ServiceResult,
};
use std::{
    cell::Cell,
    fs,
    io::{BufRead, BufReader, Read, Write},
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

static CANCELLED: AtomicBool = AtomicBool::new(true);
static CANCEL_ON_UPLOAD: AtomicBool = AtomicBool::new(false);
static CANCEL_ON_SAVE: AtomicBool = AtomicBool::new(false);
static CANCEL_AFTER_SAVE_SUCCESS: AtomicBool = AtomicBool::new(false);
static CANCEL_AFTER_MESSAGE_WRITE: AtomicBool = AtomicBool::new(false);

/// Temp directory that removes itself even when an assertion panics.
struct TempTree {
    root: PathBuf,
}

impl TempTree {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("push-cancel-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).expect("create temp tree");
        Self { root }
    }

    fn file(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let path = self.root.join(name);
        fs::write(&path, bytes).expect("write temp file");
        path
    }
}

impl Drop for TempTree {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn cancellation_stops_before_authentication_or_upload() {
    let temp = TempTree::new();
    let path = temp.file("cancel.pdf", b"pdf");
    let backend = HttpPushBackend::new(Duration::from_secs(1), "fake-token")
        .unwrap()
        .with_cancel_flag(&CANCELLED);
    let report = PushService::new(backend).push_files(std::slice::from_ref(&path), |_| {});
    assert_eq!(report.exit_code(), 130);
    assert_eq!(report.accepted, 0);
}

#[test]
fn preexisting_cancellation_takes_priority_over_local_preflight_errors() {
    let temp = TempTree::new();
    let paths = [
        temp.root.join("missing.pdf"),
        temp.root.join("also-missing.pdf"),
    ];
    let backend = HttpPushBackend::new(Duration::from_secs(1), "fake-token")
        .unwrap()
        .with_cancel_flag(&CANCELLED);
    let report = PushService::new(backend).push_files(&paths, |_| {});
    assert_eq!(report.exit_code(), 130);
    assert_eq!(report.files.len(), 2);
    for (file, path) in report.files.iter().zip(paths) {
        assert_eq!(file.path, path.to_string_lossy());
        assert_eq!(file.stage, boox_core::services::PushStage::Prepare);
        assert_eq!(
            file.error.as_ref().unwrap().kind,
            ServiceErrorKind::Cancelled
        );
    }
}

struct CancellingPush {
    flag: &'static AtomicBool,
    cancel_on_upload: bool,
    cancel_on_save: bool,
    cancel_after_save_success: bool,
    uploads: Cell<usize>,
    writes: Cell<usize>,
    saves: Cell<usize>,
}

impl CancellingPush {
    fn new(flag: &'static AtomicBool) -> Self {
        flag.store(false, Ordering::SeqCst);
        Self {
            flag,
            cancel_on_upload: false,
            cancel_on_save: false,
            cancel_after_save_success: false,
            uploads: Cell::new(0),
            writes: Cell::new(0),
            saves: Cell::new(0),
        }
    }
}

impl PushBackend for CancellingPush {
    fn cancellation_requested(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
    }

    fn auth_context(&mut self) -> ServiceResult<UploadAuthContext> {
        Ok(UploadAuthContext {
            bearer: "token".into(),
            uid: "user".into(),
            storage_limit: None,
            storage_used: None,
        })
    }

    fn default_bucket(&mut self) -> ServiceResult<(String, BucketConfig)> {
        Ok(("onyx-cloud".into(), BucketConfig::default()))
    }

    fn sts(&mut self, _: &UploadAuthContext) -> ServiceResult<OssSts> {
        Ok(OssSts {
            access_key_id: "id".into(),
            access_key_secret: "secret".into(),
            security_token: "sts".into(),
            expiration: "2099-01-01T00:00:00Z".into(),
        })
    }

    fn upload(
        &mut self,
        _: &BucketConfig,
        _: &OssSts,
        _: &str,
        _: &Path,
        _: &str,
    ) -> ServiceResult<()> {
        self.uploads.set(self.uploads.get() + 1);
        if self.cancel_on_upload {
            self.flag.store(true, Ordering::SeqCst);
            return Err(ServiceError::cancelled("upload"));
        }
        Ok(())
    }

    fn signed_url(
        &mut self,
        _: &UploadAuthContext,
        _: &BucketConfig,
        _: &str,
    ) -> ServiceResult<String> {
        Ok("https://signed.invalid/file".into())
    }

    fn write_message(
        &mut self,
        _: &UploadAuthContext,
        _: &Path,
        _: &str,
        _: u64,
        _: &str,
        _: &str,
        _: &str,
    ) -> ServiceResult<(String, String)> {
        self.writes.set(self.writes.get() + 1);
        Ok(("doc-1".into(), "1-rev".into()))
    }

    fn confirm_sync(&mut self, _: &UploadAuthContext, _: &str, _: &str) -> ServiceResult<()> {
        Ok(())
    }

    fn save_and_push(
        &mut self,
        _: &UploadAuthContext,
        _: &str,
        _: &str,
        _: &str,
        _: &str,
        _: &str,
        _: &str,
    ) -> ServiceResult<()> {
        self.saves.set(self.saves.get() + 1);
        if self.cancel_on_save {
            self.flag.store(true, Ordering::SeqCst);
            return Err(ServiceError::cancelled("save_and_push"));
        }
        if self.cancel_after_save_success {
            self.flag.store(true, Ordering::SeqCst);
        }
        Ok(())
    }
}

#[test]
fn cancelled_upload_stops_the_batch_before_any_message_write() {
    let temp = TempTree::new();
    let first = temp.file("first.pdf", b"first");
    let second = temp.file("second.pdf", b"second");
    let mut backend = CancellingPush::new(&CANCEL_ON_UPLOAD);
    backend.cancel_on_upload = true;
    let mut service = PushService::new(backend);

    let report = service.push_files(&[first, second], |_| {});

    assert_eq!(report.exit_code(), 130);
    assert_eq!(report.accepted, 0);
    assert_eq!(
        report.files[0].stage,
        boox_core::services::PushStage::UploadOss
    );
    assert_eq!(report.files[0].outcome, PushOutcome::Failed);
    assert_eq!(
        report.files[0].error.as_ref().map(|error| error.kind),
        Some(ServiceErrorKind::Cancelled)
    );
    assert_eq!(
        report.files[1].error.as_ref().map(|error| error.kind),
        Some(ServiceErrorKind::Cancelled),
        "the remaining file must be cancelled without contacting the upload backend"
    );
    assert_eq!(service.backend().uploads.get(), 1);
    assert_eq!(service.backend().writes.get(), 0);
    assert_eq!(service.backend().saves.get(), 0);
}

#[test]
fn cancellation_after_message_creation_is_unknown_with_the_message_id() {
    let temp = TempTree::new();
    let file = temp.file("book.pdf", b"book");
    let mut backend = CancellingPush::new(&CANCEL_ON_SAVE);
    backend.cancel_on_save = true;
    let mut service = PushService::new(backend);

    let report = service.push_files(std::slice::from_ref(&file), |_| {});

    assert_eq!(report.exit_code(), 130);
    assert_eq!(report.accepted, 0);
    assert_eq!(report.unknown, 1);
    assert_eq!(report.files[0].outcome, PushOutcome::Unknown);
    assert_eq!(
        report.files[0].error.as_ref().map(|error| error.kind),
        Some(ServiceErrorKind::Cancelled)
    );
    assert_eq!(report.files[0].message_id.as_deref(), Some("doc-1"));
    assert_eq!(service.backend().saves.get(), 1);
}

#[test]
fn cancellation_after_the_first_file_keeps_that_acceptance() {
    let temp = TempTree::new();
    let first = temp.file("first.pdf", b"first");
    let second = temp.file("second.pdf", b"second");
    let mut backend = CancellingPush::new(&CANCEL_AFTER_SAVE_SUCCESS);
    backend.cancel_after_save_success = true;
    let mut service = PushService::new(backend);

    let report = service.push_files(&[first, second], |_| {});

    assert_eq!(report.exit_code(), 130);
    assert_eq!(report.accepted, 1);
    assert_eq!(report.files[0].outcome, PushOutcome::Accepted);
    assert_eq!(report.files[0].message_id.as_deref(), Some("doc-1"));
    assert_eq!(report.files[1].outcome, PushOutcome::Failed);
    assert_eq!(
        report.files[1].error.as_ref().map(|error| error.kind),
        Some(ServiceErrorKind::Cancelled)
    );
    assert_eq!(service.backend().uploads.get(), 1);
    assert_eq!(service.backend().writes.get(), 1);
    assert_eq!(service.backend().saves.get(), 1);
}

/// Real HTTP adapter regression: the message PUT response arrives, then the
/// server observes cancellation before responding. The known `(id, rev)` must
/// survive so the service can record an unknown outcome and stop before
/// confirming sync or submitting the file.
struct MessageWriteFixture {
    endpoint: String,
    message_puts: Arc<AtomicUsize>,
    confirmations: Arc<AtomicUsize>,
    submissions: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl MessageWriteFixture {
    fn start(cancel_on_write: &'static AtomicBool) -> Self {
        cancel_on_write.store(false, Ordering::SeqCst);
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind message fixture");
        listener
            .set_nonblocking(true)
            .expect("nonblocking message fixture");
        let endpoint = format!("http://{}", listener.local_addr().expect("fixture address"));
        let message_puts = Arc::new(AtomicUsize::new(0));
        let confirmations = Arc::new(AtomicUsize::new(0));
        let submissions = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));

        let puts = Arc::clone(&message_puts);
        let confirms = Arc::clone(&confirmations);
        let posts = Arc::clone(&submissions);
        let stopping = Arc::clone(&stop);
        let handle = std::thread::spawn(move || {
            while !stopping.load(Ordering::SeqCst) {
                let (mut stream, _) = match listener.accept() {
                    Ok(value) => value,
                    Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(2));
                        continue;
                    }
                    Err(_) => break,
                };
                stream
                    .set_nonblocking(false)
                    .expect("blocking message fixture");
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .expect("message read timeout");
                stream
                    .set_write_timeout(Some(Duration::from_secs(5)))
                    .expect("message write timeout");
                let mut reader = BufReader::new(stream.try_clone().expect("clone message fixture"));
                let mut request_line = String::new();
                if reader.read_line(&mut request_line).is_err() {
                    continue;
                }
                let mut content_length = 0usize;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).is_err() || line == "\r\n" || line.is_empty() {
                        break;
                    }
                    if let Some(value) = line
                        .to_ascii_lowercase()
                        .strip_prefix("content-length:")
                        .and_then(|value| value.trim().parse::<usize>().ok())
                    {
                        content_length = value;
                    }
                }
                let method = request_line.split_whitespace().next().unwrap_or("GET");
                let path = request_line.split_whitespace().nth(1).unwrap_or("/");
                let path_without_query = path.split('?').next().unwrap_or(path);

                if method == "PUT" && path_without_query.starts_with("/neocloud/") {
                    puts.fetch_add(1, Ordering::SeqCst);
                    let id = path_without_query
                        .trim_start_matches("/neocloud/")
                        .to_string();
                    if content_length > 0 {
                        let mut sink = vec![0u8; content_length];
                        let _ = reader.read_exact(&mut sink);
                    }
                    // Simulate cancellation arriving while the response is in
                    // flight: the backend has the identity but must not discard
                    // it merely because the flag flipped before `send` returned.
                    cancel_on_write.store(true, Ordering::SeqCst);
                    let (status, body) = (
                        "201 Created",
                        format!(r#"{{"id":"{id}","rev":"1-fixture"}}"#),
                    );
                    write_message_response(&mut stream, status, &body);
                    continue;
                }

                if method == "GET" && path_without_query.starts_with("/neocloud/") {
                    confirms.fetch_add(1, Ordering::SeqCst);
                }
                if method == "POST" && path_without_query == "/api/1/push/saveAndPush" {
                    posts.fetch_add(1, Ordering::SeqCst);
                }

                if content_length > 0 {
                    let mut sink = vec![0u8; content_length];
                    let _ = reader.read_exact(&mut sink);
                }
                let (status, body) = message_fixture_response(method, path_without_query);
                write_message_response(&mut stream, status, &body);
            }
        });

        Self {
            endpoint,
            message_puts,
            confirmations,
            submissions,
            stop,
            handle: Some(handle),
        }
    }

    fn message_puts(&self) -> usize {
        self.message_puts.load(Ordering::SeqCst)
    }

    fn confirmations(&self) -> usize {
        self.confirmations.load(Ordering::SeqCst)
    }

    fn submissions(&self) -> usize {
        self.submissions.load(Ordering::SeqCst)
    }
}

impl Drop for MessageWriteFixture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        join_with_deadline(self.handle.take(), Duration::from_secs(2));
    }
}

/// Do not let a blocked fixture socket keep a failed test process alive.
fn join_with_deadline(handle: Option<JoinHandle<()>>, limit: Duration) {
    let Some(handle) = handle else {
        return;
    };
    let deadline = Instant::now() + limit;
    while !handle.is_finished() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(2));
    }
    if handle.is_finished() {
        let _ = handle.join();
    }
}

fn write_message_response(stream: &mut TcpStream, status: &str, body: &str) {
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes());
}

fn envelope(data: &str) -> String {
    format!(r#"{{"result_code":0,"data":{data}}}"#)
}

fn message_fixture_response(method: &str, path: &str) -> (&'static str, String) {
    match (method, path) {
        ("GET", "/api/1/users/me") => (
            "200 OK",
            envelope(r#"{"uid":"uid-fixture","nickname":"reader"}"#),
        ),
        ("GET", "/api/1/statistics/v2/user/storage") => (
            "200 OK",
            envelope(r#"{"usedSize":0,"totalSize":1000000000}"#),
        ),
        ("GET", "/api/1/config/buckets") => (
            "200 OK",
            envelope(
                r#"{"onyx-cloud":{"bucket":"onyx-cloud","aliEndpoint":"https://oss-fixture.invalid"}}"#,
            ),
        ),
        ("GET", "/api/1/config/stss") => (
            "200 OK",
            envelope(
                r#"{"AccessKeyId":"id","AccessKeySecret":"secret","SecurityToken":"sts","Expiration":"2099-01-01T00:00:00Z"}"#,
            ),
        ),
        ("GET", "/api/1/users/syncToken") => (
            "200 OK",
            envelope(r#"{"cookie_name":"SyncGatewaySession","session_id":"fixture-session"}"#),
        ),
        ("PUT", path) if path.starts_with("/uid-fixture/push/") => ("200 OK", String::new()),
        ("GET", path) if path.starts_with("/uid-fixture/push/") => {
            ("206 Partial Content", String::new())
        }
        ("GET", path) if path.starts_with("/neocloud/") => {
            let id = path.trim_start_matches("/neocloud/");
            let content = r#"{"formats":["pdf"],"storage":{"pdf":{"oss":{"key":"uid-fixture/push/fixture.pdf","size":42}}}}"#;
            (
                "200 OK",
                serde_json::json!({
                    "_id": id,
                    "_rev": "1-fixture",
                    "msgType": 2,
                    "contentType": "digital_content",
                    "name": "book.pdf",
                    "size": 42,
                    "content": content,
                })
                .to_string(),
            )
        }
        ("POST", "/api/1/push/saveAndPush") => ("200 OK", envelope(r#"{"accepted":true}"#)),
        _ => (
            "404 Not Found",
            r#"{"error":"unexpected fixture request"}"#.to_string(),
        ),
    }
}

#[test]
fn cancellation_after_write_response_keeps_known_message_id_and_rev() {
    let temp = TempTree::new();
    let file = temp.file("book.pdf", b"book");
    let fixture = MessageWriteFixture::start(&CANCEL_AFTER_MESSAGE_WRITE);
    let backend = HttpPushBackend::with_test_endpoints(
        Duration::from_secs(5),
        "fixture-token",
        &fixture.endpoint,
        &fixture.endpoint,
    )
    .expect("loopback backend")
    .with_cancel_flag(&CANCEL_AFTER_MESSAGE_WRITE);
    let mut service = PushService::new(backend);

    let report = service.push_files(std::slice::from_ref(&file), |_| {});

    assert_eq!(
        fixture.message_puts(),
        1,
        "the message write must have happened"
    );
    assert_eq!(report.exit_code(), 130);
    assert_eq!(report.accepted, 0);
    assert_eq!(report.unknown, 1);
    assert_eq!(
        report.files[0].stage,
        boox_core::services::PushStage::WriteMessage
    );
    assert_eq!(report.files[0].outcome, PushOutcome::Unknown);
    assert_eq!(
        report.files[0].error.as_ref().map(|error| error.kind),
        Some(ServiceErrorKind::Cancelled)
    );
    assert!(
        report.files[0].message_id.is_some(),
        "the known message id must survive cancellation after the write response"
    );
    assert_eq!(report.files[0].rev.as_deref(), Some("1-fixture"));
    assert_eq!(
        fixture.confirmations(),
        0,
        "cancellation must stop before confirm_sync"
    );
    assert_eq!(
        fixture.submissions(),
        0,
        "cancellation must stop before saveAndPush"
    );
}
