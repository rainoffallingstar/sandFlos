//! SIGINT while the post-upload API waits for a response.
//!
//! The fixture deliberately receives a request and then withholds its response
//! far beyond the child watchdog. The CLI is started with `--timeout 300`, so a
//! passing test means cancellation interrupted the in-flight request itself;
//! it cannot mean that a shorter network timeout eventually expired.

#![cfg(unix)]

mod common;

use common::{json_stdout, stderr, stdout, TestDir, TEST_API_ENDPOINT_ENV, TEST_OSS_ENDPOINT_ENV};
use serde_json::Value;
use std::{
    env,
    io::{BufRead, BufReader, Read, Write},
    net::{TcpListener, TcpStream},
    path::Path,
    process::{Child, Command, Output, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

const CHILD_DEADLINE: Duration = Duration::from_secs(8);
const PROMPT_EXIT: Duration = Duration::from_secs(3);
const SERVER_STALL: Duration = Duration::from_secs(15);
const START_DEADLINE: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HoldPhase {
    Message,
    SyncToken,
    Confirm,
    ConfirmMalformedOnce,
    Save,
}

struct PostWriteFixture {
    endpoint: String,
    held_started: Arc<AtomicBool>,
    requests: Arc<Mutex<Vec<String>>>,
    message_id: Arc<Mutex<Option<String>>>,
    submission_id: Arc<Mutex<Option<String>>>,
    release_held: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl PostWriteFixture {
    fn start(hold: HoldPhase) -> Self {
        let listener =
            TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).expect("bind post-write fixture");
        listener
            .set_nonblocking(true)
            .expect("nonblocking post-write fixture");
        let endpoint = format!("http://{}", listener.local_addr().expect("fixture address"));
        let held_started = Arc::new(AtomicBool::new(false));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let message_id = Arc::new(Mutex::new(None));
        let submission_id = Arc::new(Mutex::new(None));
        let release_held = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));

        let held = Arc::clone(&held_started);
        let observed = Arc::clone(&requests);
        let message_id_for_server = Arc::clone(&message_id);
        let submission_id_for_server = Arc::clone(&submission_id);
        let release = Arc::clone(&release_held);
        let stopping = Arc::clone(&stop);
        let handle = std::thread::spawn(move || {
            let mut confirmation_gets = 0usize;
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
                let Some((method, path, content_length)) = read_request_head(&mut reader) else {
                    continue;
                };
                observed
                    .lock()
                    .expect("fixture requests")
                    .push(format!("{method} {path}"));

                let path_without_query = path.split('?').next().unwrap_or(&path);
                let body = read_body(&mut reader, content_length);

                if method == "GET"
                    && path_without_query == "/api/1/users/syncToken"
                    && hold == HoldPhase::SyncToken
                {
                    if hold_until_released(&held, &release, &stopping) {
                        write_response(
                            &mut stream,
                            "200 OK",
                            &api_envelope(
                                r#"{"cookie_name":"SyncGatewaySession","session_id":"fixture-session"}"#,
                            ),
                        );
                    }
                    continue;
                }

                if method == "PUT" && path_without_query.starts_with("/neocloud/") {
                    let id = path_without_query
                        .trim_start_matches("/neocloud/")
                        .to_string();
                    *message_id_for_server.lock().expect("message id") = Some(id.clone());
                    if hold == HoldPhase::Message {
                        hold_response(&held, &stopping);
                        continue;
                    }
                    write_response(
                        &mut stream,
                        "201 Created",
                        &format!(r#"{{"id":"{id}","rev":"1-fixture"}}"#),
                    );
                    continue;
                }

                if method == "GET" && path_without_query.starts_with("/neocloud/") {
                    confirmation_gets += 1;
                    if hold == HoldPhase::Confirm {
                        hold_response(&held, &stopping);
                        continue;
                    }
                    if hold == HoldPhase::ConfirmMalformedOnce && confirmation_gets == 1 {
                        write_response(
                            &mut stream,
                            "200 OK",
                            r#"{"unexpected":"malformed confirmation"}"#,
                        );
                        continue;
                    }
                    let id = path_without_query.trim_start_matches("/neocloud/");
                    let content = r#"{"formats":["pdf"],"storage":{"pdf":{"oss":{"key":"uid-fixture/push/fixture.pdf","size":42}}}}"#;
                    let body = serde_json::json!({
                        "_id": id,
                        "_rev": "1-fixture",
                        "msgType": 2,
                        "contentType": "digital_content",
                        "name": "book.pdf",
                        "size": 42,
                        "content": content,
                    })
                    .to_string();
                    write_response(&mut stream, "200 OK", &body);
                    continue;
                }

                if method == "POST" && path_without_query == "/api/1/push/saveAndPush" {
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
                    *submission_id_for_server.lock().expect("submission id") = id;
                    if hold == HoldPhase::Save {
                        hold_response(&held, &stopping);
                        continue;
                    }
                    write_response(&mut stream, "200 OK", &api_envelope(r#"{"accepted":true}"#));
                    continue;
                }

                let (status, body) = fixture_response(&method, path_without_query);
                write_response(&mut stream, status, &body);
            }
        });

        Self {
            endpoint,
            held_started,
            requests,
            message_id,
            submission_id,
            release_held,
            stop,
            handle: Some(handle),
        }
    }

    fn release_held_response(&self) {
        self.release_held.store(true, Ordering::SeqCst);
    }

    fn endpoint(&self) -> &str {
        &self.endpoint
    }

    fn wait_until_held(&self) {
        let deadline = Instant::now() + START_DEADLINE;
        while Instant::now() < deadline {
            if self.held_started.load(Ordering::SeqCst) {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!(
            "the CLI never reached the held response phase: {:?}",
            self.requests()
        );
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

    fn message_id(&self) -> Option<String> {
        self.message_id.lock().expect("message id").clone()
    }

    fn submission_id(&self) -> Option<String> {
        self.submission_id.lock().expect("submission id").clone()
    }
}

impl Drop for PostWriteFixture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        join_with_deadline(self.handle.take(), Duration::from_secs(2));
    }
}

fn hold_response(held: &AtomicBool, stop: &AtomicBool) {
    hold_until_released(held, stop, stop);
}

fn hold_until_released(held: &AtomicBool, release: &AtomicBool, stop: &AtomicBool) -> bool {
    held.store(true, Ordering::SeqCst);
    let deadline = Instant::now() + SERVER_STALL;
    while !stop.load(Ordering::SeqCst)
        && !release.load(Ordering::SeqCst)
        && Instant::now() < deadline
    {
        std::thread::sleep(Duration::from_millis(5));
    }
    !stop.load(Ordering::SeqCst)
}

fn read_request_head(reader: &mut BufReader<TcpStream>) -> Option<(String, String, usize)> {
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).is_err() {
        return None;
    }
    let method = request_line.split_whitespace().next()?.to_string();
    let path = request_line.split_whitespace().nth(1)?.to_string();
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
    Some((method, path, content_length))
}

fn read_body(reader: &mut BufReader<TcpStream>, content_length: usize) -> Option<Vec<u8>> {
    if content_length == 0 {
        return Some(Vec::new());
    }
    let mut body = vec![0u8; content_length];
    reader.read_exact(&mut body).ok().map(|()| body)
}

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
        ("PUT", path) if path.starts_with("/uid-fixture/push/") => ("200 OK", String::new()),
        ("GET", path) if path.starts_with("/uid-fixture/push/") => {
            ("206 Partial Content", String::new())
        }
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
                    return child.wait_with_output().expect("reap booxctl");
                }
                Ok(None) if Instant::now() >= deadline => {
                    let mut child = self.child.take().expect("running child");
                    let _ = child.kill();
                    let output = child.wait_with_output().expect("reap killed booxctl");
                    panic!(
                        "booxctl did not exit within {timeout:?}; stdout: {} stderr: {}",
                        stdout(&output),
                        stderr(&output)
                    );
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(10)),
                Err(error) => panic!("cannot poll booxctl: {error}"),
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
    let mut command = Command::new(env!("CARGO_BIN_EXE_booxctl"));
    command.args(["--json", "push"]);
    for file in files {
        command.arg(file);
    }
    command
        .args(["--timeout", "300"])
        .env("BOOXCTL_CONFIG_DIR", dir.config_dir())
        .env("BOOXCTL_CREDENTIAL_STORE", "file")
        .env("BOOXCTL_TOKEN", "fixture-token")
        .env(TEST_API_ENDPOINT_ENV, endpoint)
        .env(TEST_OSS_ENDPOINT_ENV, endpoint)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    ChildGuard::new(command.spawn().expect("spawn booxctl"))
}

fn assert_second_file_was_not_started(value: &Value, second: &Path) {
    assert_eq!(value["data"]["files"][1]["stage"], "prepare");
    assert_eq!(value["data"]["files"][1]["outcome"], "failed");
    assert_eq!(
        value["data"]["files"][1]["path"],
        second.to_string_lossy().as_ref()
    );
    assert_eq!(value["data"]["files"][1]["display_name"], "second.pdf");
    assert_eq!(value["data"]["files"][1]["error"]["kind"], "cancelled");
}

#[test]
fn malformed_confirmation_document_is_retried_before_the_batch_succeeds() {
    let dir = TestDir::new();
    let server = PostWriteFixture::start(HoldPhase::ConfirmMalformedOnce);
    let file = dir.path().join("book.pdf");
    std::fs::write(&file, b"book").expect("write fixture file");

    let child = spawn_push(&dir, server.endpoint(), &[&file]);
    let output = child.wait_with_deadline(CHILD_DEADLINE);

    assert_eq!(
        output.status.code(),
        Some(0),
        "stdout: {} stderr: {}",
        stdout(&output),
        stderr(&output)
    );
    let value = json_stdout(&output);
    assert_eq!(value["ok"], true);
    assert_eq!(value["data"]["accepted"], 1);
    assert_eq!(value["data"]["files"][0]["outcome"], "accepted");
    assert_eq!(
        server.count_requests("GET /neocloud/"),
        2,
        "a malformed confirmation document must consume one attempt and the next valid response must succeed: {:?}",
        server.requests()
    );
    assert_eq!(server.count_requests("PUT /neocloud/"), 1);
    assert_eq!(server.count_requests("POST /api/1/push/saveAndPush"), 1);
}

#[test]
fn sigint_while_sync_token_is_pending_does_not_start_the_message_put() {
    let dir = TestDir::new();
    let server = PostWriteFixture::start(HoldPhase::SyncToken);
    let first = dir.path().join("first.pdf");
    let second = dir.path().join("second.pdf");
    std::fs::write(&first, b"first").expect("write first fixture");
    std::fs::write(&second, b"second").expect("write second fixture");

    let child = spawn_push(&dir, server.endpoint(), &[&first, &second]);
    server.wait_until_held();
    let interrupted = Instant::now();
    send_sigint(child.pid());
    // Let the signal handler observe cancellation, then complete the token
    // request that was already in flight. Its response must not unlock a new
    // message PUT.
    std::thread::sleep(Duration::from_millis(5));
    server.release_held_response();
    let output = child.wait_with_deadline(CHILD_DEADLINE);
    let elapsed = interrupted.elapsed();

    assert!(
        elapsed < PROMPT_EXIT,
        "SIGINT must stop a pending sync-token request promptly, took {elapsed:?}; stdout: {} stderr: {}",
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
    assert_eq!(value["data"]["files"][0]["stage"], "write_message");
    assert_eq!(value["data"]["files"][0]["outcome"], "unknown");
    assert_eq!(value["data"]["files"][0]["error"]["kind"], "cancelled");
    assert!(
        value["data"]["files"][0]["message_id"].is_null(),
        "no message identity exists before the PUT: {value}"
    );
    assert!(value["data"]["files"][0]["rev"].is_null());
    assert_second_file_was_not_started(&value, &second);
    assert_eq!(
        server.count_requests("PUT /neocloud/"),
        0,
        "cancellation before the PUT must not be reported or retried as a remote write: {:?}",
        server.requests()
    );
    assert_eq!(server.count_requests("POST /api/1/push/saveAndPush"), 0);
    assert_eq!(server.count_requests("PUT /uid-fixture/push/"), 1);
}

#[test]
fn sigint_while_message_write_response_is_slow_exits_130_without_retry() {
    let dir = TestDir::new();
    let server = PostWriteFixture::start(HoldPhase::Message);
    let first = dir.path().join("first.pdf");
    let second = dir.path().join("second.pdf");
    std::fs::write(&first, b"first").expect("write first fixture");
    std::fs::write(&second, b"second").expect("write second fixture");

    let child = spawn_push(&dir, server.endpoint(), &[&first, &second]);
    server.wait_until_held();
    let interrupted = Instant::now();
    send_sigint(child.pid());
    let output = child.wait_with_deadline(CHILD_DEADLINE);
    let elapsed = interrupted.elapsed();

    assert!(
        elapsed < PROMPT_EXIT,
        "SIGINT must interrupt the held message response promptly, took {elapsed:?}; stdout: {} stderr: {}",
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
    assert_eq!(value["data"]["files"][0]["stage"], "write_message");
    assert_eq!(value["data"]["files"][0]["outcome"], "unknown");
    assert_eq!(value["data"]["files"][0]["error"]["kind"], "cancelled");
    assert_second_file_was_not_started(&value, &second);
    assert_eq!(
        server.count_requests("PUT /neocloud/"),
        1,
        "an unconfirmed message write must not be retried"
    );
    assert_eq!(
        server.count_requests("GET /neocloud/"),
        0,
        "cancellation must not start sync confirmation"
    );
    assert_eq!(server.count_requests("POST /api/1/push/saveAndPush"), 0);
    assert_eq!(
        server.count_requests("PUT /uid-fixture/push/"),
        1,
        "the second file must not reach OSS"
    );
}

#[test]
fn sigint_during_confirm_sync_polling_keeps_identity_and_stops_promptly() {
    let dir = TestDir::new();
    let server = PostWriteFixture::start(HoldPhase::Confirm);
    let first = dir.path().join("first.pdf");
    let second = dir.path().join("second.pdf");
    std::fs::write(&first, b"first").expect("write first fixture");
    std::fs::write(&second, b"second").expect("write second fixture");

    let child = spawn_push(&dir, server.endpoint(), &[&first, &second]);
    server.wait_until_held();
    let interrupted = Instant::now();
    send_sigint(child.pid());
    let output = child.wait_with_deadline(CHILD_DEADLINE);
    let elapsed = interrupted.elapsed();

    assert!(
        elapsed < PROMPT_EXIT,
        "SIGINT must interrupt confirm_sync polling promptly, took {elapsed:?}; stdout: {} stderr: {}",
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
    assert_eq!(value["data"]["files"][0]["stage"], "confirm_sync");
    assert_eq!(value["data"]["files"][0]["outcome"], "unknown");
    assert_eq!(value["data"]["files"][0]["error"]["kind"], "cancelled");
    let known_id = server
        .message_id()
        .expect("fixture observed the message id");
    assert_eq!(value["data"]["files"][0]["message_id"], known_id);
    assert_eq!(value["data"]["files"][0]["rev"], "1-fixture");
    assert_second_file_was_not_started(&value, &second);
    assert_eq!(server.count_requests("PUT /neocloud/"), 1);
    assert_eq!(server.count_requests("GET /neocloud/"), 1);
    assert_eq!(
        server.count_requests("POST /api/1/push/saveAndPush"),
        0,
        "sync confirmation must not advance to submission"
    );
    assert_eq!(server.count_requests("PUT /uid-fixture/push/"), 1);
}

#[test]
fn sigint_while_save_and_push_response_is_slow_keeps_identity_and_does_not_retry() {
    let dir = TestDir::new();
    let server = PostWriteFixture::start(HoldPhase::Save);
    let first = dir.path().join("first.pdf");
    let second = dir.path().join("second.pdf");
    std::fs::write(&first, b"first").expect("write first fixture");
    std::fs::write(&second, b"second").expect("write second fixture");

    let child = spawn_push(&dir, server.endpoint(), &[&first, &second]);
    server.wait_until_held();
    let interrupted = Instant::now();
    send_sigint(child.pid());
    let output = child.wait_with_deadline(CHILD_DEADLINE);
    let elapsed = interrupted.elapsed();

    assert!(
        elapsed < PROMPT_EXIT,
        "SIGINT must interrupt the held saveAndPush response promptly, took {elapsed:?}; stdout: {} stderr: {}",
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
    let known_id = server
        .submission_id()
        .expect("fixture observed the submitted message id");
    assert_eq!(value["data"]["files"][0]["message_id"], known_id);
    assert_eq!(value["data"]["files"][0]["rev"], "1-fixture");
    assert_second_file_was_not_started(&value, &second);
    assert_eq!(server.count_requests("PUT /neocloud/"), 1);
    assert_eq!(server.count_requests("GET /neocloud/"), 1);
    assert_eq!(
        server.count_requests("POST /api/1/push/saveAndPush"),
        1,
        "the submission must not be retried"
    );
    assert_eq!(server.count_requests("PUT /uid-fixture/push/"), 1);
}
