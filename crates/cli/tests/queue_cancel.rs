//! SIGINT while a queue request is in flight.
//!
//! Queue fixtures receive the request and withhold the relevant response far
//! beyond the child watchdog. The CLI runs with `--timeout 300`, so a prompt
//! exit proves cancellation interrupted the request instead of letting the
//! configured request timeout expire.

#![cfg(unix)]

mod common;

use boox_core::services::{
    HttpQueueMutationBackend, HttpQueueReadBackend, QueueService, ServiceErrorKind,
};
use common::{json_stdout, stderr, stdout, TestDir, TEST_API_ENDPOINT_ENV, TEST_OSS_ENDPOINT_ENV};
use std::{
    env,
    io::{BufReader, Read, Write},
    net::{TcpListener, TcpStream},
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
const SOCKET_DEADLINE: Duration = Duration::from_secs(5);
const START_DEADLINE: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HoldPhase {
    Auth,
    SyncToken,
    Changes,
    ChangesBody,
    ResendPut,
    ResendPutBody,
    DeleteDoc,
    DeleteDocBody,
    DeleteDelete,
    BatchDelete,
    ParentConfirm,
    ParentFallbackSyncToken,
}

struct QueueFixture {
    endpoint: String,
    held_started: Arc<AtomicBool>,
    requests: Arc<Mutex<Vec<String>>>,
    release_held: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    workers: Arc<Mutex<Vec<JoinHandle<()>>>>,
    handle: Option<JoinHandle<()>>,
}

impl QueueFixture {
    fn start(hold: HoldPhase) -> Self {
        let listener =
            TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).expect("bind queue fixture");
        listener
            .set_nonblocking(true)
            .expect("nonblocking queue fixture");
        let endpoint = format!("http://{}", listener.local_addr().expect("fixture address"));
        let held_started = Arc::new(AtomicBool::new(false));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let release_held = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));
        let workers = Arc::new(Mutex::new(Vec::new()));

        let held = Arc::clone(&held_started);
        let observed = Arc::clone(&requests);
        let release = Arc::clone(&release_held);
        let stopping = Arc::clone(&stop);
        let worker_handles = Arc::clone(&workers);
        let handle = std::thread::spawn(move || loop {
            if stopping.load(Ordering::SeqCst) {
                // Consume every connection already queued before shutdown so
                // a follow-up request cannot hide behind an unaccepted socket.
                while let Ok((stream, _)) = listener.accept() {
                    spawn_worker(
                        stream,
                        hold,
                        Arc::clone(&held),
                        Arc::clone(&observed),
                        Arc::clone(&release),
                        Arc::clone(&stopping),
                        &worker_handles,
                    );
                }
                break;
            }
            match listener.accept() {
                Ok((stream, _)) => spawn_worker(
                    stream,
                    hold,
                    Arc::clone(&held),
                    Arc::clone(&observed),
                    Arc::clone(&release),
                    Arc::clone(&stopping),
                    &worker_handles,
                ),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(2));
                }
                Err(_) => break,
            }
        });

        Self {
            endpoint,
            held_started,
            requests,
            release_held,
            stop,
            workers,
            handle: Some(handle),
        }
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
            "the CLI never reached the held queue response: {:?}",
            self.requests()
        );
    }

    fn requests(&self) -> Vec<String> {
        self.requests.lock().expect("fixture requests").clone()
    }

    /// Stop accepting new work, release held responses, drain connections
    /// already queued in the listener, and join the listener as well as every
    /// worker. Returns the complete request heads observed by the fixture.
    fn finish(&mut self) -> Vec<String> {
        self.stop.store(true, Ordering::SeqCst);
        self.release_held.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            handle.join().expect("join queue fixture listener");
        }
        let workers = std::mem::take(&mut *self.workers.lock().expect("fixture workers"));
        for worker in workers {
            worker.join().expect("join queue fixture worker");
        }
        self.requests()
    }
}

impl Drop for QueueFixture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        self.release_held.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
        let workers = std::mem::take(&mut *self.workers.lock().expect("fixture workers"));
        for worker in workers {
            let _ = worker.join();
        }
    }
}

fn spawn_worker(
    stream: TcpStream,
    hold: HoldPhase,
    held: Arc<AtomicBool>,
    observed: Arc<Mutex<Vec<String>>>,
    release: Arc<AtomicBool>,
    stopping: Arc<AtomicBool>,
    workers: &Arc<Mutex<Vec<JoinHandle<()>>>>,
) {
    let worker = std::thread::spawn(move || {
        handle_connection(stream, hold, held, observed, release, stopping);
    });
    workers.lock().expect("fixture workers").push(worker);
}

fn holds(hold: HoldPhase, method: &str, path_without_query: &str) -> bool {
    match hold {
        HoldPhase::Auth => method == "GET" && path_without_query == "/api/1/users/me",
        HoldPhase::SyncToken => method == "GET" && path_without_query == "/api/1/users/syncToken",
        HoldPhase::Changes | HoldPhase::ChangesBody => {
            method == "GET" && path_without_query == "/neocloud/_changes"
        }
        HoldPhase::ResendPut | HoldPhase::ResendPutBody => {
            method == "PUT" && path_without_query.starts_with("/neocloud/")
        }
        HoldPhase::DeleteDoc | HoldPhase::DeleteDocBody => {
            method == "GET"
                && path_without_query.starts_with("/neocloud/")
                && path_without_query != "/neocloud/_changes"
        }
        HoldPhase::DeleteDelete => {
            method == "DELETE" && path_without_query.starts_with("/neocloud/")
        }
        HoldPhase::BatchDelete => {
            method == "POST" && path_without_query == "/api/1/push/message/batchDelete"
        }
        HoldPhase::ParentConfirm | HoldPhase::ParentFallbackSyncToken => false,
    }
}

fn body_stall(hold: HoldPhase) -> bool {
    matches!(
        hold,
        HoldPhase::ChangesBody | HoldPhase::ResendPutBody | HoldPhase::DeleteDocBody
    )
}

fn handle_connection(
    mut stream: TcpStream,
    hold: HoldPhase,
    held: Arc<AtomicBool>,
    observed: Arc<Mutex<Vec<String>>>,
    release: Arc<AtomicBool>,
    stopping: Arc<AtomicBool>,
) {
    stream.set_nonblocking(false).expect("blocking fixture");
    stream
        .set_write_timeout(Some(SOCKET_DEADLINE))
        .expect("fixture write timeout");

    let deadline = Instant::now() + SOCKET_DEADLINE;
    let mut reader = BufReader::new(stream.try_clone().expect("clone fixture"));
    let Some((head, method, path, content_length)) = read_request_head(&mut reader, deadline)
    else {
        return;
    };
    observed.lock().expect("fixture requests").push(head);
    let path_without_query = path.split('?').next().unwrap_or(&path);
    let _ = read_body(&mut reader, content_length, deadline);

    let ordinal = observed
        .lock()
        .expect("fixture requests")
        .iter()
        .filter(|line| line.starts_with(&format!("{method} {path_without_query}")))
        .count();
    let held_phase = holds(hold, &method, path_without_query)
        || (hold == HoldPhase::ParentConfirm
            && method == "GET"
            && path_without_query == "/neocloud/doc-fixture"
            && ordinal == 2)
        || (hold == HoldPhase::ParentFallbackSyncToken
            && method == "GET"
            && path_without_query == "/api/1/users/syncToken"
            && ordinal == 3);
    if held_phase {
        let (status, body) = fixture_response(hold, &method, path_without_query);
        if body_stall(hold) {
            let split = body.len() / 2;
            write_response_prefix(&mut stream, status, &body, split);
            held.store(true, Ordering::SeqCst);
            if wait_until_released(&release, &stopping) {
                let _ = stream.write_all(&body.as_bytes()[split..]);
            }
        } else if hold_until_released(&held, &release, &stopping) {
            write_response(&mut stream, status, &body);
        }
        return;
    }

    let (status, body) = fixture_response(hold, &method, path_without_query);
    write_response(&mut stream, status, &body);
}

fn wait_until_released(release: &AtomicBool, stop: &AtomicBool) -> bool {
    let deadline = Instant::now() + SERVER_STALL;
    while !stop.load(Ordering::SeqCst)
        && !release.load(Ordering::SeqCst)
        && Instant::now() < deadline
    {
        std::thread::sleep(Duration::from_millis(5));
    }
    !stop.load(Ordering::SeqCst)
}

fn hold_until_released(held: &AtomicBool, release: &AtomicBool, stop: &AtomicBool) -> bool {
    held.store(true, Ordering::SeqCst);
    wait_until_released(release, stop)
}

fn remaining(deadline: Instant) -> Option<Duration> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    (!remaining.is_zero()).then_some(remaining)
}

fn read_line_before(reader: &mut BufReader<TcpStream>, deadline: Instant) -> Option<String> {
    let mut line = Vec::new();
    loop {
        reader
            .get_ref()
            .set_read_timeout(Some(remaining(deadline)?))
            .ok()?;
        let mut byte = [0];
        if reader.read(&mut byte).ok()? == 0 {
            return None;
        }
        line.push(byte[0]);
        if byte[0] == b'\n' {
            return String::from_utf8(line).ok();
        }
        if line.len() > 16 * 1024 {
            return None;
        }
    }
}

fn read_request_head(
    reader: &mut BufReader<TcpStream>,
    deadline: Instant,
) -> Option<(String, String, String, usize)> {
    let request_line = read_line_before(reader, deadline)?;
    let method = request_line.split_whitespace().next()?.to_string();
    let path = request_line.split_whitespace().nth(1)?.to_string();
    let mut raw = request_line;
    let mut content_length = 0usize;
    loop {
        let line = read_line_before(reader, deadline)?;
        raw.push_str(&line);
        if line == "\r\n" {
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
    Some((raw, method, path, content_length))
}

fn read_body(
    reader: &mut BufReader<TcpStream>,
    content_length: usize,
    deadline: Instant,
) -> Option<Vec<u8>> {
    if content_length == 0 {
        return Some(Vec::new());
    }
    let mut body = vec![0u8; content_length];
    let mut offset = 0;
    while offset < body.len() {
        reader
            .get_ref()
            .set_read_timeout(Some(remaining(deadline)?))
            .ok()?;
        let read = reader.read(&mut body[offset..]).ok()?;
        if read == 0 {
            return None;
        }
        offset += read;
    }
    Some(body)
}

fn write_response(stream: &mut TcpStream, status: &str, body: &str) {
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes());
}

fn write_response_prefix(stream: &mut TcpStream, status: &str, body: &str, prefix: usize) {
    let headers = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(headers.as_bytes());
    let _ = stream.write_all(&body.as_bytes()[..prefix]);
    let _ = stream.flush();
}

fn api_envelope(data: &str) -> String {
    format!(r#"{{"result_code":0,"data":{data}}}"#)
}

fn queue_doc() -> String {
    r#"{"_id":"doc-fixture","_rev":"1-fixture","msgType":2,"contentType":"digital_content","name":"book.pdf","size":42,"storageKey":"uid-fixture/push/book.pdf","content":"{\"formats\":[\"pdf\"],\"storage\":{\"pdf\":{\"oss\":{\"key\":\"uid-fixture/push/book.pdf\",\"size\":42}}}}"}"#.to_string()
}

fn parent_queue_doc(rev: &str) -> String {
    format!(
        r#"{{"_id":"doc-fixture","_rev":"{rev}","parent":"parent-fixture","msgType":2,"contentType":"digital_content","name":"book.pdf","size":42,"storageKey":"uid-fixture/push/book.pdf","content":"{{\"formats\":[\"pdf\"],\"storage\":{{\"pdf\":{{\"oss\":{{\"key\":\"uid-fixture/push/book.pdf\",\"size\":42}}}}}}}}"}}"#
    )
}

fn fixture_response(
    hold: HoldPhase,
    method: &str,
    path_without_query: &str,
) -> (&'static str, String) {
    match (method, path_without_query) {
        ("GET", "/api/1/users/me") => (
            "200 OK",
            api_envelope(r#"{"uid":"uid-fixture","nickname":"reader"}"#),
        ),
        ("GET", "/api/1/users/syncToken") => (
            "200 OK",
            api_envelope(r#"{"cookie_name":"SyncGatewaySession","session_id":"fixture-session"}"#),
        ),
        ("GET", "/neocloud/_changes") => (
            "200 OK",
            format!(r#"{{"results":[{{"doc":{}}}]}}"#, queue_doc()),
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
        ("GET", "/neocloud/doc-fixture") if hold == HoldPhase::ParentConfirm => {
            ("200 OK", parent_queue_doc("2-fixture"))
        }
        ("GET", "/neocloud/doc-fixture") if hold == HoldPhase::ParentFallbackSyncToken => {
            ("200 OK", parent_queue_doc("1-fixture"))
        }
        ("GET", "/neocloud/doc-fixture") => ("200 OK", queue_doc()),
        ("PUT", "/neocloud/doc-fixture") if hold == HoldPhase::ParentFallbackSyncToken => (
            "404 Not Found",
            r#"{"error":"missing parent record"}"#.to_string(),
        ),
        ("PUT", "/neocloud/doc-fixture") => (
            "201 Created",
            r#"{"id":"doc-fixture","rev":"2-fixture"}"#.to_string(),
        ),
        ("DELETE", "/neocloud/doc-fixture") => ("200 OK", "{}".to_string()),
        ("POST", "/api/1/push/message/batchDelete") => ("200 OK", api_envelope("null")),
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

fn spawn_queue(dir: &TestDir, endpoint: &str, args: &[&str]) -> ChildGuard {
    let mut command = Command::new(env!("CARGO_BIN_EXE_sandflos"));
    command.args(args);
    command
        .args(["--timeout", "300"])
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

fn interrupted_output(child: ChildGuard) -> (Output, Duration) {
    let interrupted = Instant::now();
    send_sigint(child.pid());
    let output = child.wait_with_deadline(CHILD_DEADLINE);
    (output, interrupted.elapsed())
}

fn assert_prompt_cancel(output: &Output, elapsed: Duration) {
    assert!(
        elapsed < PROMPT_EXIT,
        "SIGINT must stop the held queue request promptly, took {elapsed:?}; stdout: {} stderr: {}",
        stdout(output),
        stderr(output)
    );
    assert_eq!(
        output.status.code(),
        Some(130),
        "stdout: {} stderr: {}",
        stdout(output),
        stderr(output)
    );
    let value = json_stdout(output);
    assert_eq!(value["ok"], false);
    assert!(!stdout(output).contains("fixture-token"));
    assert!(!stdout(output).contains("fixture-session"));
}

fn assert_cancelled_error(output: &Output, elapsed: Duration) {
    assert_prompt_cancel(output, elapsed);
    assert_eq!(json_stdout(output)["error"]["code"], "cancelled");
}

fn count(records: &[String], prefix: &str) -> usize {
    records
        .iter()
        .filter(|line| line.starts_with(prefix))
        .count()
}

fn run_held_phase(hold: HoldPhase, args: &[&str]) -> (QueueFixture, TestDir, ChildGuard) {
    let dir = TestDir::new();
    let server = QueueFixture::start(hold);
    let child = spawn_queue(&dir, server.endpoint(), args);
    server.wait_until_held();
    (server, dir, child)
}

#[test]
fn sigint_while_changes_response_is_slow_exits_promptly_without_results() {
    let (mut server, _dir, child) =
        run_held_phase(HoldPhase::Changes, &["--json", "queue", "list"]);
    let (output, elapsed) = interrupted_output(child);
    assert_cancelled_error(&output, elapsed);
    let records = server.finish();
    assert_eq!(count(&records, "GET /api/1/users/me"), 1);
    assert_eq!(count(&records, "GET /api/1/users/syncToken"), 1);
    assert_eq!(count(&records, "GET /neocloud/_changes"), 1);
}

fn run_cli_phase(hold: HoldPhase, args: &[&str]) -> (QueueFixture, TestDir, ChildGuard) {
    run_held_phase(hold, args)
}

#[test]
fn sigint_while_queue_auth_is_slow_exits_promptly_before_sync_token() {
    let (mut server, _dir, child) = run_cli_phase(HoldPhase::Auth, &["--json", "queue", "list"]);
    let (output, elapsed) = interrupted_output(child);
    assert_cancelled_error(&output, elapsed);
    let records = server.finish();
    assert_eq!(count(&records, "GET /api/1/users/me"), 1);
    assert_eq!(count(&records, "GET /api/1/users/syncToken"), 0);
    assert_eq!(count(&records, "GET /neocloud/_changes"), 0);
}

#[test]
fn sigint_while_queue_sync_token_is_slow_exits_promptly_before_changes() {
    let (mut server, _dir, child) =
        run_cli_phase(HoldPhase::SyncToken, &["--json", "queue", "list"]);
    let (output, elapsed) = interrupted_output(child);
    assert_cancelled_error(&output, elapsed);
    let records = server.finish();
    assert_eq!(count(&records, "GET /api/1/users/me"), 1);
    assert_eq!(count(&records, "GET /api/1/users/syncToken"), 1);
    assert_eq!(count(&records, "GET /neocloud/_changes"), 0);
}

#[test]
fn sigint_while_changes_body_stalls_exits_promptly_without_results() {
    let (mut server, _dir, child) =
        run_cli_phase(HoldPhase::ChangesBody, &["--json", "queue", "list"]);
    let (output, elapsed) = interrupted_output(child);
    assert_cancelled_error(&output, elapsed);
    let records = server.finish();
    assert_eq!(count(&records, "GET /neocloud/_changes"), 1);
}

#[test]
fn sigint_while_resend_sync_token_is_slow_sends_no_write() {
    let (mut server, _dir, child) = run_cli_phase(
        HoldPhase::SyncToken,
        &["--json", "queue", "resend", "doc-fixture"],
    );
    let (output, elapsed) = interrupted_output(child);
    assert_cancelled_error(&output, elapsed);
    let records = server.finish();
    assert_eq!(count(&records, "GET /api/1/users/syncToken"), 1);
    assert_eq!(count(&records, "GET /neocloud/doc-fixture"), 0);
    assert_eq!(count(&records, "PUT /neocloud/"), 0);
}

#[test]
fn sigint_while_resend_put_response_is_slow_reports_unknown_without_retry() {
    let (mut server, _dir, child) = run_cli_phase(
        HoldPhase::ResendPut,
        &["--json", "queue", "resend", "doc-fixture"],
    );
    let (output, elapsed) = interrupted_output(child);
    assert_prompt_cancel(&output, elapsed);
    let records = server.finish();
    assert_eq!(count(&records, "PUT /neocloud/doc-fixture"), 1);
    let value = json_stdout(&output);
    assert_eq!(value["error"]["code"], "resend_unknown");
    assert_eq!(
        value["error"]["message"],
        "重推提交结果未知，请先查询队列再决定是否重试"
    );
}

#[test]
fn sigint_while_delete_document_body_stalls_does_not_start_delete() {
    let (mut server, _dir, child) = run_cli_phase(
        HoldPhase::DeleteDocBody,
        &["--json", "queue", "delete", "doc-fixture", "--yes"],
    );
    let (output, elapsed) = interrupted_output(child);
    assert_prompt_cancel(&output, elapsed);
    let records = server.finish();
    assert_eq!(count(&records, "GET /neocloud/doc-fixture"), 1);
    assert_eq!(count(&records, "DELETE /neocloud/"), 0);
    assert_eq!(count(&records, "POST /api/1/push/message/batchDelete"), 0);
}

#[test]
fn sigint_while_delete_response_is_slow_reports_unknown_and_skips_message_delete() {
    let (mut server, _dir, child) = run_cli_phase(
        HoldPhase::DeleteDelete,
        &["--json", "queue", "delete", "doc-fixture", "--yes"],
    );
    let (output, elapsed) = interrupted_output(child);
    assert_prompt_cancel(&output, elapsed);
    let records = server.finish();
    assert_eq!(count(&records, "GET /neocloud/doc-fixture"), 1);
    assert_eq!(count(&records, "DELETE /neocloud/doc-fixture"), 1);
    assert_eq!(count(&records, "POST /api/1/push/message/batchDelete"), 0);
    let value = json_stdout(&output);
    assert_eq!(value["error"]["code"], "queue_delete_incomplete");
    assert_eq!(value["data"]["complete"], false);
    assert_eq!(value["data"]["unknown"], true);
    assert_eq!(value["data"]["remote_delete_attempted"], true);
    assert_eq!(value["data"]["message_delete_attempted"], false);
}

#[test]
fn sigint_while_message_delete_is_slow_reports_partial_unknown() {
    let (mut server, _dir, child) = run_cli_phase(
        HoldPhase::BatchDelete,
        &["--json", "queue", "delete", "doc-fixture", "--yes"],
    );
    let (output, elapsed) = interrupted_output(child);
    assert_prompt_cancel(&output, elapsed);
    let records = server.finish();
    assert_eq!(count(&records, "DELETE /neocloud/doc-fixture"), 1);
    assert_eq!(count(&records, "POST /api/1/push/message/batchDelete"), 1);
    let value = json_stdout(&output);
    assert_eq!(value["error"]["code"], "queue_delete_incomplete");
    assert_eq!(value["data"]["complete"], false);
    assert_eq!(value["data"]["unknown"], true);
    assert_eq!(value["data"]["remote_delete_attempted"], true);
    assert_eq!(value["data"]["message_delete_attempted"], true);
}

#[test]
fn cancelled_queue_read_sends_no_request() {
    static CANCEL: AtomicBool = AtomicBool::new(true);
    let mut server = QueueFixture::start(HoldPhase::Changes);
    let mut service = QueueService::new(
        HttpQueueReadBackend::with_test_endpoints(
            Duration::from_secs(300),
            "fixture-token",
            server.endpoint(),
            server.endpoint(),
        )
        .expect("read backend")
        .with_cancel_flag(&CANCEL),
    );
    let error = service.list(None).expect_err("pre-cancelled list");
    assert_eq!(error.kind, ServiceErrorKind::Cancelled);
    assert!(server.finish().is_empty());
}

#[test]
fn completed_changes_response_does_not_become_a_queue_result_after_cancellation() {
    static CANCEL: AtomicBool = AtomicBool::new(false);
    CANCEL.store(false, Ordering::SeqCst);
    let mut server = QueueFixture::start(HoldPhase::Changes);
    let mut service = QueueService::new(
        HttpQueueReadBackend::with_test_endpoints(
            Duration::from_secs(300),
            "fixture-token",
            server.endpoint(),
            server.endpoint(),
        )
        .expect("read backend")
        .with_cancel_flag(&CANCEL),
    );
    let controller = release_after_cancel(&server, &CANCEL);
    let error = service.list(None).expect_err("cancelled queue list");
    controller.join().expect("join controller");
    assert_eq!(error.kind, ServiceErrorKind::Cancelled);
    let records = server.finish();
    assert_eq!(count(&records, "GET /neocloud/_changes"), 1);
}

fn release_after_cancel(server: &QueueFixture, flag: &'static AtomicBool) -> JoinHandle<()> {
    let held = Arc::clone(&server.held_started);
    let release = Arc::clone(&server.release_held);
    std::thread::spawn(move || {
        let deadline = Instant::now() + START_DEADLINE;
        while !held.load(Ordering::SeqCst) {
            assert!(Instant::now() < deadline, "held response was not received");
            std::thread::sleep(Duration::from_millis(2));
        }
        // Publish cancellation before the successful response can complete.
        flag.store(true, Ordering::SeqCst);
        release.store(true, Ordering::SeqCst);
    })
}

#[test]
fn confirmed_resend_response_wins_over_late_cancellation() {
    static CANCEL: AtomicBool = AtomicBool::new(false);
    CANCEL.store(false, Ordering::SeqCst);
    let mut server = QueueFixture::start(HoldPhase::ResendPut);
    let mut service = QueueService::new(
        HttpQueueMutationBackend::with_test_endpoints(
            Duration::from_secs(300),
            "fixture-token",
            server.endpoint(),
            server.endpoint(),
        )
        .expect("mutation backend")
        .with_cancel_flag(&CANCEL),
    );
    let controller = release_after_cancel(&server, &CANCEL);
    let result = service.resend("doc-fixture");
    controller.join().expect("join controller");
    assert!(
        result.is_ok(),
        "confirmed submission must stay success: {result:?}"
    );
    let records = server.finish();
    assert_eq!(count(&records, "PUT /neocloud/doc-fixture"), 1);
}

#[test]
fn confirmed_delete_response_is_not_relabelled_unknown_before_message_delete() {
    static CANCEL: AtomicBool = AtomicBool::new(false);
    CANCEL.store(false, Ordering::SeqCst);
    let mut server = QueueFixture::start(HoldPhase::DeleteDelete);
    let mut service = QueueService::new(
        HttpQueueMutationBackend::with_test_endpoints(
            Duration::from_secs(300),
            "fixture-token",
            server.endpoint(),
            server.endpoint(),
        )
        .expect("mutation backend")
        .with_cancel_flag(&CANCEL),
    );
    let controller = release_after_cancel(&server, &CANCEL);
    let outcome = service.delete("doc-fixture", true).expect("delete outcome");
    controller.join().expect("join controller");
    assert!(outcome.remote_delete_attempted);
    assert!(!outcome.complete);
    assert!(!outcome.unknown, "a confirmed first step is not ambiguous");
    assert!(!outcome.message_delete_attempted);
    assert_eq!(
        outcome.error.as_ref().map(|error| error.kind),
        Some(ServiceErrorKind::Cancelled)
    );
    let records = server.finish();
    assert_eq!(count(&records, "DELETE /neocloud/doc-fixture"), 1);
    assert_eq!(count(&records, "POST /api/1/push/message/batchDelete"), 0);
}

#[test]
fn sigint_while_resend_put_body_stalls_reports_unknown_without_retry() {
    let (mut server, _dir, child) = run_cli_phase(
        HoldPhase::ResendPutBody,
        &["--json", "queue", "resend", "doc-fixture"],
    );
    let (output, elapsed) = interrupted_output(child);
    assert_prompt_cancel(&output, elapsed);
    let records = server.finish();
    assert_eq!(count(&records, "PUT /neocloud/doc-fixture"), 1);
    let value = json_stdout(&output);
    assert_eq!(value["error"]["code"], "resend_unknown");
}

#[test]
fn sigint_while_delete_sync_token_is_slow_sends_no_mutation() {
    let (mut server, _dir, child) = run_cli_phase(
        HoldPhase::SyncToken,
        &["--json", "queue", "delete", "doc-fixture", "--yes"],
    );
    let (output, elapsed) = interrupted_output(child);
    assert_cancelled_error(&output, elapsed);
    let records = server.finish();
    assert_eq!(count(&records, "GET /api/1/users/syncToken"), 1);
    assert_eq!(count(&records, "DELETE /neocloud/"), 0);
    assert_eq!(count(&records, "POST /api/1/push/message/batchDelete"), 0);
}

#[test]
fn sigint_while_delete_document_response_headers_stall_sends_no_mutation() {
    let (mut server, _dir, child) = run_cli_phase(
        HoldPhase::DeleteDoc,
        &["--json", "queue", "delete", "doc-fixture", "--yes"],
    );
    let (output, elapsed) = interrupted_output(child);
    assert_cancelled_error(&output, elapsed);
    let records = server.finish();
    assert_eq!(count(&records, "GET /neocloud/doc-fixture"), 1);
    assert_eq!(count(&records, "DELETE /neocloud/"), 0);
    assert_eq!(count(&records, "POST /api/1/push/message/batchDelete"), 0);
}

#[test]
fn confirmed_message_delete_response_wins_over_late_cancellation() {
    static CANCEL: AtomicBool = AtomicBool::new(false);
    CANCEL.store(false, Ordering::SeqCst);
    let mut server = QueueFixture::start(HoldPhase::BatchDelete);
    let mut service = QueueService::new(
        HttpQueueMutationBackend::with_test_endpoints(
            Duration::from_secs(300),
            "fixture-token",
            server.endpoint(),
            server.endpoint(),
        )
        .expect("mutation backend")
        .with_cancel_flag(&CANCEL),
    );
    let controller = release_after_cancel(&server, &CANCEL);
    let outcome = service.delete("doc-fixture", true).expect("delete outcome");
    controller.join().expect("join controller");
    assert!(
        outcome.complete,
        "a confirmed second step is complete: {outcome:?}"
    );
    assert!(!outcome.unknown);
    assert!(outcome.remote_delete_attempted);
    assert!(outcome.message_delete_attempted);
    let records = server.finish();
    assert_eq!(count(&records, "DELETE /neocloud/doc-fixture"), 1);
    assert_eq!(count(&records, "POST /api/1/push/message/batchDelete"), 1);
}

#[test]
fn cancellation_while_resend_sync_token_is_in_flight_does_not_start_document_read_or_put() {
    static CANCEL: AtomicBool = AtomicBool::new(false);
    CANCEL.store(false, Ordering::SeqCst);
    let mut server = QueueFixture::start(HoldPhase::SyncToken);
    let mut service = QueueService::new(
        HttpQueueMutationBackend::with_test_endpoints(
            Duration::from_secs(300),
            "fixture-token",
            server.endpoint(),
            server.endpoint(),
        )
        .expect("mutation backend")
        .with_cancel_flag(&CANCEL),
    );
    let controller = release_after_cancel(&server, &CANCEL);
    let error = service
        .resend("doc-fixture")
        .expect_err("cancelled resend preparation");
    controller.join().expect("join controller");
    assert_eq!(error.kind, ServiceErrorKind::Cancelled);
    let records = server.finish();
    assert_eq!(count(&records, "GET /api/1/users/syncToken"), 1);
    assert_eq!(count(&records, "GET /neocloud/doc-fixture"), 0);
    assert_eq!(count(&records, "PUT /neocloud/"), 0);
}

#[test]
fn cancellation_while_delete_sync_token_is_in_flight_does_not_start_any_mutation() {
    static CANCEL: AtomicBool = AtomicBool::new(false);
    CANCEL.store(false, Ordering::SeqCst);
    let mut server = QueueFixture::start(HoldPhase::SyncToken);
    let mut service = QueueService::new(
        HttpQueueMutationBackend::with_test_endpoints(
            Duration::from_secs(300),
            "fixture-token",
            server.endpoint(),
            server.endpoint(),
        )
        .expect("mutation backend")
        .with_cancel_flag(&CANCEL),
    );
    let controller = release_after_cancel(&server, &CANCEL);
    let error = service
        .delete("doc-fixture", true)
        .expect_err("cancelled delete preparation");
    controller.join().expect("join controller");
    assert_eq!(error.kind, ServiceErrorKind::Cancelled);
    let records = server.finish();
    assert_eq!(count(&records, "GET /api/1/users/syncToken"), 1);
    assert_eq!(count(&records, "GET /neocloud/doc-fixture"), 0);
    assert_eq!(count(&records, "DELETE /neocloud/"), 0);
    assert_eq!(count(&records, "POST /api/1/push/message/batchDelete"), 0);
}

#[test]
fn pre_cancelled_resend_sends_no_request() {
    static CANCEL: AtomicBool = AtomicBool::new(true);
    let mut server = QueueFixture::start(HoldPhase::ResendPut);
    let mut service = QueueService::new(
        HttpQueueMutationBackend::with_test_endpoints(
            Duration::from_secs(300),
            "fixture-token",
            server.endpoint(),
            server.endpoint(),
        )
        .expect("mutation backend")
        .with_cancel_flag(&CANCEL),
    );
    let error = service
        .resend("doc-fixture")
        .expect_err("pre-cancelled resend");
    assert_eq!(error.kind, ServiceErrorKind::Cancelled);
    assert!(server.finish().is_empty());
}

#[test]
fn pre_cancelled_delete_sends_no_request() {
    static CANCEL: AtomicBool = AtomicBool::new(true);
    let mut server = QueueFixture::start(HoldPhase::DeleteDelete);
    let mut service = QueueService::new(
        HttpQueueMutationBackend::with_test_endpoints(
            Duration::from_secs(300),
            "fixture-token",
            server.endpoint(),
            server.endpoint(),
        )
        .expect("mutation backend")
        .with_cancel_flag(&CANCEL),
    );
    let error = service
        .delete("doc-fixture", true)
        .expect_err("pre-cancelled delete");
    assert_eq!(error.kind, ServiceErrorKind::Cancelled);
    assert!(server.finish().is_empty());
}

#[test]
fn sigint_while_parent_sync_confirmation_is_slow_is_unknown_without_fallback_write() {
    let (mut server, _dir, child) = run_cli_phase(
        HoldPhase::ParentConfirm,
        &["--json", "queue", "resend", "doc-fixture"],
    );
    let (output, elapsed) = interrupted_output(child);
    assert_prompt_cancel(&output, elapsed);
    let records = server.finish();
    assert_eq!(count(&records, "PUT /neocloud/doc-fixture"), 1);
    assert_eq!(count(&records, "GET /neocloud/doc-fixture"), 2);
    let value = json_stdout(&output);
    assert_eq!(value["error"]["code"], "resend_unknown");
}

#[test]
fn explicit_parent_rejection_then_cancelled_fallback_sync_token_does_not_fabricate_unknown() {
    let (mut server, _dir, child) = run_cli_phase(
        HoldPhase::ParentFallbackSyncToken,
        &["--json", "queue", "resend", "doc-fixture"],
    );
    let (output, elapsed) = interrupted_output(child);
    assert_cancelled_error(&output, elapsed);
    let records = server.finish();
    assert_eq!(count(&records, "PUT /neocloud/doc-fixture"), 1);
    assert_eq!(count(&records, "GET /api/1/users/syncToken"), 3);
    assert_eq!(
        count(&records, "PUT /uid-fixture/push/"),
        0,
        "fallback message PUT must not start after cancellation"
    );
    assert_eq!(
        count(&records, "PUT /neocloud/"),
        1,
        "only the explicit 404 parent PUT may have been sent"
    );
}
