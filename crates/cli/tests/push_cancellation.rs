//! Real SIGINT cancellation at the CLI boundary.
//!
//! These tests use a loopback OSS/API fixture. The first upload is either made
//! deliberately slow or the submission response is held until the test has
//! delivered SIGINT. A sparse multi-megabyte input keeps the upload out of the
//! kernel's handoff buffers long enough to exercise cancellation during body
//! streaming rather than after the local read has already finished.

#![cfg(unix)]

mod common;

use common::{json_stdout, stderr, stdout, TestDir, TEST_API_ENDPOINT_ENV, TEST_OSS_ENDPOINT_ENV};
use serde_json::Value;
use std::{
    env,
    fs::File,
    io::{BufRead, BufReader, Read, Write},
    net::{TcpListener, TcpStream},
    path::Path,
    process::{Child, Command, Output, Stdio},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

const LARGE_FILE_BYTES: u64 = 8 * 1024 * 1024;
const SERVER_DEADLINE: Duration = Duration::from_secs(12);
const CHILD_DEADLINE: Duration = Duration::from_secs(10);
const PROMPT_EXIT: Duration = Duration::from_secs(5);

struct PushFixture {
    endpoint: String,
    requests: Arc<Mutex<Vec<String>>>,
    uploads: Arc<AtomicUsize>,
    upload_started: Arc<AtomicBool>,
    upload_bytes: Arc<AtomicUsize>,
    submission_started: Arc<AtomicBool>,
    submission_release: Arc<AtomicBool>,
    submitted_id: Arc<Mutex<Option<String>>>,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl PushFixture {
    fn start(slow_upload: bool, hold_submission: bool) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind push fixture");
        listener
            .set_nonblocking(true)
            .expect("nonblocking push fixture");
        let endpoint = format!("http://{}", listener.local_addr().expect("fixture address"));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let uploads = Arc::new(AtomicUsize::new(0));
        let upload_started = Arc::new(AtomicBool::new(false));
        let upload_bytes = Arc::new(AtomicUsize::new(0));
        let submission_started = Arc::new(AtomicBool::new(false));
        let submission_release = Arc::new(AtomicBool::new(false));
        let submitted_id = Arc::new(Mutex::new(None));
        let stop = Arc::new(AtomicBool::new(false));

        let observed = Arc::clone(&requests);
        let counted_uploads = Arc::clone(&uploads);
        let started = Arc::clone(&upload_started);
        let body_bytes = Arc::clone(&upload_bytes);
        let submitted = Arc::clone(&submission_started);
        let release = Arc::clone(&submission_release);
        let submitted_id_for_server = Arc::clone(&submitted_id);
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
                stream.set_nonblocking(false).expect("blocking fixture");
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .expect("fixture read timeout");
                stream
                    .set_write_timeout(Some(Duration::from_secs(5)))
                    .expect("fixture write timeout");
                let mut reader = BufReader::new(stream.try_clone().expect("clone fixture"));
                let mut request_line = String::new();
                if reader.read_line(&mut request_line).is_err() {
                    continue;
                }
                let mut content_length = 0usize;
                let mut header_lines = Vec::new();
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
                    header_lines.push(line.trim_end().to_string());
                }
                observed
                    .lock()
                    .expect("fixture requests")
                    .push(request_line.trim_end().to_string());

                let method = request_line.split_whitespace().next().unwrap_or("GET");
                let path = request_line.split_whitespace().nth(1).unwrap_or("/");
                let path_without_query = path.split('?').next().unwrap_or(path);

                if method == "PUT" && path_without_query.starts_with("/uid-fixture/push/") {
                    counted_uploads.fetch_add(1, Ordering::SeqCst);
                    started.store(true, Ordering::SeqCst);
                    let completed = if slow_upload {
                        read_body_slowly(
                            &mut reader,
                            content_length,
                            &body_bytes,
                            &stopping,
                            SERVER_DEADLINE,
                        )
                    } else {
                        read_body_with_timeout(&mut reader, content_length, &body_bytes).is_some()
                    };
                    if completed {
                        let _ = stream.write_all(
                            b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                        );
                    }
                    continue;
                }

                let body = read_body_with_timeout(&mut reader, content_length, &body_bytes);

                if method == "POST"
                    && path_without_query == "/api/1/push/saveAndPush"
                    && hold_submission
                {
                    let id = body
                        .as_deref()
                        .and_then(|bytes| serde_json::from_slice::<Value>(bytes).ok())
                        .and_then(|value| {
                            value
                                .get("cbMsg")
                                .and_then(|msg| msg.get("id"))
                                .and_then(Value::as_str)
                                .map(ToString::to_string)
                        });
                    *submitted_id_for_server.lock().expect("submission id") = id;
                    submitted.store(true, Ordering::SeqCst);
                    let deadline = Instant::now() + SERVER_DEADLINE;
                    while !release.load(Ordering::SeqCst)
                        && !stopping.load(Ordering::SeqCst)
                        && Instant::now() < deadline
                    {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    // Drop the socket without a response. The submission
                    // may already have committed, so the client can only
                    // report an unknown outcome.
                    continue;
                }

                let (status, body) = fixture_response(method, path_without_query);
                write_response(&mut stream, status, &body);
            }
        });

        Self {
            endpoint,
            requests,
            uploads,
            upload_started,
            upload_bytes,
            submission_started,
            submission_release,
            submitted_id,
            stop,
            handle: Some(handle),
        }
    }

    fn endpoint(&self) -> &str {
        &self.endpoint
    }

    fn wait_until_uploading(&self) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if self.upload_started.load(Ordering::SeqCst) {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!(
            "the CLI never started the OSS upload: {:?}",
            self.requests()
        );
    }

    fn wait_until_submission(&self) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if self.submission_started.load(Ordering::SeqCst) {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("the CLI never submitted saveAndPush: {:?}", self.requests());
    }

    fn release_submission(&self) {
        self.submission_release.store(true, Ordering::SeqCst);
    }

    fn uploads(&self) -> usize {
        self.uploads.load(Ordering::SeqCst)
    }

    fn upload_bytes(&self) -> usize {
        self.upload_bytes.load(Ordering::SeqCst)
    }

    fn submitted_id(&self) -> Option<String> {
        self.submitted_id.lock().expect("submission id").clone()
    }

    fn requests(&self) -> Vec<String> {
        self.requests.lock().expect("fixture requests").clone()
    }

    fn count_requests(&self, prefix: &str) -> usize {
        self.requests()
            .iter()
            .filter(|line| line.starts_with(prefix))
            .count()
    }
}

impl Drop for PushFixture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        self.submission_release.store(true, Ordering::SeqCst);
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

fn read_body_with_timeout(
    reader: &mut BufReader<TcpStream>,
    content_length: usize,
    body_bytes: &AtomicUsize,
) -> Option<Vec<u8>> {
    if content_length == 0 {
        return Some(Vec::new());
    }
    let mut body = vec![0u8; content_length];
    match reader.read_exact(&mut body) {
        Ok(()) => {
            body_bytes.fetch_add(body.len(), Ordering::SeqCst);
            Some(body)
        }
        Err(_) => None,
    }
}

fn read_body_slowly(
    reader: &mut BufReader<TcpStream>,
    content_length: usize,
    body_bytes: &AtomicUsize,
    stop: &AtomicBool,
    limit: Duration,
) -> bool {
    let deadline = Instant::now() + limit;
    let mut remaining = content_length;
    let mut chunk = [0u8; 64 * 1024];
    while remaining > 0 && Instant::now() < deadline && !stop.load(Ordering::SeqCst) {
        let want = remaining.min(chunk.len());
        match reader.read(&mut chunk[..want]) {
            Ok(0) | Err(_) => return false,
            Ok(read) => {
                remaining -= read;
                body_bytes.fetch_add(read, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(25));
            }
        }
    }
    remaining == 0
}

fn write_response(stream: &mut TcpStream, status: &str, body: &str) {
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes());
}

fn api_envelope(data: &str) -> String {
    format!(r#"{{"result_code":0,"data":{data}}}"#)
}

fn fixture_response(method: &str, path: &str) -> (&'static str, String) {
    match (method, path) {
        ("GET", "/api/1/users/me") => (
            "200 OK",
            api_envelope(r#"{"uid":"uid-fixture","nickname":"reader"}"#),
        ),
        ("GET", "/api/1/statistics/v2/user/storage") => (
            "200 OK",
            api_envelope(r#"{"usedSize":0,"totalSize":1000000000}"#),
        ),
        ("GET", "/api/1/config/buckets") => (
            "200 OK",
            api_envelope(
                r#"{"onyx-cloud":{"bucket":"onyx-cloud","aliEndpoint":"https://oss-fixture.invalid"}}"#,
            ),
        ),
        ("GET", "/api/1/config/stss") => (
            "200 OK",
            api_envelope(
                r#"{"AccessKeyId":"id","AccessKeySecret":"secret","SecurityToken":"sts","Expiration":"2099-01-01T00:00:00Z"}"#,
            ),
        ),
        ("GET", "/api/1/users/syncToken") => (
            "200 OK",
            api_envelope(r#"{"cookie_name":"SyncGatewaySession","session_id":"fixture-session"}"#),
        ),
        ("GET", path) if path.starts_with("/uid-fixture/push/") => {
            ("206 Partial Content", String::new())
        }
        ("PUT", path) if path.starts_with("/neocloud/") => {
            let id = path.trim_start_matches("/neocloud/");
            (
                "201 Created",
                format!(r#"{{"id":"{id}","rev":"1-fixture"}}"#),
            )
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
        ("POST", "/api/1/push/saveAndPush") => ("200 OK", api_envelope(r#"{"accepted":true}"#)),
        _ => (
            "404 Not Found",
            r#"{"error":"unexpected fixture request"}"#.to_string(),
        ),
    }
}

extern "C" {
    fn kill(pid: i32, sig: i32) -> i32;
}

fn send_sigint(pid: u32) {
    let rc = unsafe { kill(pid as i32, 2) };
    assert_eq!(rc, 0, "SIGINT must be delivered");
}

struct ChildGuard {
    child: Option<Child>,
}

impl ChildGuard {
    fn new(child: Child) -> Self {
        Self { child: Some(child) }
    }

    fn pid(&self) -> u32 {
        self.child.as_ref().expect("running child").id()
    }

    fn wait_with_deadline(mut self, timeout: Duration) -> Output {
        let deadline = Instant::now() + timeout;
        loop {
            match self.child.as_mut().expect("running child").try_wait() {
                Ok(Some(_)) => {
                    let child = self.child.take().expect("running child");
                    return child.wait_with_output().expect("reap sandflos");
                }
                Ok(None) if Instant::now() >= deadline => {
                    let mut child = self.child.take().expect("running child");
                    let _ = child.kill();
                    let output = child.wait_with_output().expect("reap killed sandflos");
                    panic!(
                        "sandflos did not exit within {timeout:?}; stdout: {} stderr: {}",
                        stdout(&output),
                        stderr(&output)
                    );
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(10)),
                Err(error) => panic!("cannot poll sandflos: {error}"),
            }
        }
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn spawn_push(dir: &TestDir, endpoint: &str, files: &[&Path]) -> ChildGuard {
    let mut command = Command::new(env!("CARGO_BIN_EXE_sandflos"));
    command.args(["--json", "push"]);
    for file in files {
        command.arg(file);
    }
    command
        .args(["--timeout", "30"])
        .env("SANDFLOS_CONFIG_DIR", dir.config_dir())
        .env("SANDFLOS_CREDENTIAL_STORE", "file")
        .env("SANDFLOS_TOKEN", "fixture-token")
        .env(TEST_API_ENDPOINT_ENV, endpoint)
        .env(TEST_OSS_ENDPOINT_ENV, endpoint)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    ChildGuard::new(command.spawn().expect("spawn sandflos"))
}

#[cfg(target_os = "linux")]
fn process_read_bytes(pid: u32) -> Option<u64> {
    let text = std::fs::read_to_string(format!("/proc/{pid}/io")).ok()?;
    text.lines()
        .find_map(|line| line.strip_prefix("rchar:")?.trim().parse::<u64>().ok())
}

fn sparse_file(dir: &TestDir, name: &str, bytes: u64) -> std::path::PathBuf {
    let path = dir.path().join(name);
    let file = File::create(&path).expect("create sparse file");
    file.set_len(bytes).expect("size sparse file");
    path
}

#[test]
fn sigint_during_large_upload_stops_two_file_batch_before_message_creation() {
    let dir = TestDir::new();
    let server = PushFixture::start(true, false);
    let first = sparse_file(&dir, "large.pdf", LARGE_FILE_BYTES);
    let second = dir.path().join("second.pdf");
    std::fs::write(&second, b"second").expect("write second fixture");

    let child = spawn_push(&dir, server.endpoint(), &[&first, &second]);
    server.wait_until_uploading();
    std::thread::sleep(Duration::from_millis(100));
    // On Linux, `rchar` is direct evidence that the process has not read the
    // entire sparse file into memory before cancellation. Keep this check
    // conditional because /proc is not available on every Unix target.
    #[cfg(target_os = "linux")]
    {
        let read_bytes = process_read_bytes(child.pid()).expect("read /proc/<pid>/io");
        assert!(
            read_bytes < LARGE_FILE_BYTES,
            "the process read {read_bytes} bytes before SIGINT; it must not consume the whole {LARGE_FILE_BYTES}-byte file"
        );
    }
    let interrupted = Instant::now();
    send_sigint(child.pid());
    let output = child.wait_with_deadline(CHILD_DEADLINE);
    let elapsed = interrupted.elapsed();

    assert!(
        elapsed < PROMPT_EXIT,
        "SIGINT must stop the body promptly, took {elapsed:?}; stdout: {} stderr: {}",
        stdout(&output),
        stderr(&output)
    );
    assert_eq!(
        output.status.code(),
        Some(130),
        "stdout: {} stderr: {}",
        stdout(&output),
        stderr(&output)
    );
    let value = json_stdout(&output);
    assert_eq!(value["error"]["code"], "push_incomplete");
    assert_eq!(value["data"]["files"].as_array().map(Vec::len), Some(2));
    assert_eq!(value["data"]["files"][0]["stage"], "upload_oss");
    assert_eq!(value["data"]["files"][0]["error"]["kind"], "cancelled");
    assert_eq!(value["data"]["files"][1]["stage"], "prepare");
    assert_eq!(
        value["data"]["files"][1]["path"],
        second.to_string_lossy().as_ref()
    );
    assert_eq!(value["data"]["files"][1]["display_name"], "second.pdf");
    assert_eq!(value["data"]["files"][1]["error"]["kind"], "cancelled");
    assert_eq!(
        server.uploads(),
        1,
        "the second file must not start an upload"
    );
    assert!(
        (server.upload_bytes() as u64) < LARGE_FILE_BYTES,
        "cancellation must happen before the server receives the whole file"
    );
    assert_eq!(
        server.count_requests("PUT /uid-fixture/push/"),
        1,
        "only the first file may reach OSS"
    );
    assert_eq!(server.count_requests("PUT /neocloud/"), 0);
    assert_eq!(server.count_requests("POST /api/1/push/saveAndPush"), 0);
    let requests = server.requests();
    assert!(
        requests
            .iter()
            .all(|line| !line.starts_with("PUT /neocloud/")
                && !line.starts_with("POST /api/1/push/saveAndPush")),
        "cancellation must stop before message creation: {requests:#?}"
    );
}

#[test]
fn sigint_during_save_and_push_reports_unknown_and_stops_the_second_file() {
    let dir = TestDir::new();
    let server = PushFixture::start(false, true);
    let first = dir.path().join("first.pdf");
    let second = dir.path().join("second.pdf");
    std::fs::write(&first, b"first").expect("write first fixture");
    std::fs::write(&second, b"second").expect("write second fixture");

    let child = spawn_push(&dir, server.endpoint(), &[&first, &second]);
    server.wait_until_submission();
    let interrupted = Instant::now();
    send_sigint(child.pid());
    // Give the child's signal handler a moment to record the flag before the
    // held submission socket is dropped. The response remains absent, so the
    // message outcome cannot be treated as confirmed.
    std::thread::sleep(Duration::from_millis(250));
    server.release_submission();
    let output = child.wait_with_deadline(CHILD_DEADLINE);
    let elapsed = interrupted.elapsed();

    assert!(
        elapsed < PROMPT_EXIT,
        "a held submission must be released promptly after SIGINT, took {elapsed:?}; stdout: {} stderr: {}",
        stdout(&output),
        stderr(&output)
    );
    assert_eq!(
        output.status.code(),
        Some(130),
        "stdout: {} stderr: {}",
        stdout(&output),
        stderr(&output)
    );
    let value = json_stdout(&output);
    assert_eq!(value["error"]["code"], "push_incomplete");
    assert_eq!(value["data"]["files"].as_array().map(Vec::len), Some(2));
    assert_eq!(value["data"]["files"][0]["stage"], "save_and_push");
    assert_eq!(value["data"]["files"][0]["outcome"], "unknown");
    assert_eq!(value["data"]["files"][0]["error"]["kind"], "cancelled");
    let message_id = value["data"]["files"][0]["message_id"]
        .as_str()
        .expect("the first file must retain the known message id");
    assert_eq!(server.submitted_id().as_deref(), Some(message_id));
    assert_eq!(value["data"]["files"][0]["rev"], "1-fixture");
    assert_eq!(value["data"]["files"][1]["stage"], "prepare");
    assert_eq!(value["data"]["files"][1]["outcome"], "failed");
    assert_eq!(
        value["data"]["files"][1]["path"],
        second.to_string_lossy().as_ref()
    );
    assert_eq!(value["data"]["files"][1]["display_name"], "second.pdf");
    assert_eq!(value["data"]["files"][1]["error"]["kind"], "cancelled");
    assert_eq!(server.uploads(), 1, "the second file must not reach OSS");
    assert_eq!(server.count_requests("PUT /neocloud/"), 1);
    assert_eq!(
        server.count_requests("POST /api/1/push/saveAndPush"),
        1,
        "the submission must not be retried"
    );
}
