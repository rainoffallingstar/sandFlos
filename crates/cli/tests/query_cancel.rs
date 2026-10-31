//! SIGINT while a read-only query request is in flight.
//!
//! Query fixtures receive the request and withhold its response far beyond
//! the child watchdog. The CLI runs with `--timeout 300`, so a prompt exit
//! cannot come from the configured request timeout expiring.

#![cfg(unix)]

mod common;

use boox_core::services::{HttpQueryBackend, QueryService, ServiceErrorKind};
use common::{json_stdout, stderr, stdout, TestDir, TEST_API_ENDPOINT_ENV, TEST_OSS_ENDPOINT_ENV};
use std::{
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
    AuthBody,
    Devices,
    Storage,
    StorageBody,
    ReadingInfo,
    Week,
    Day,
}

struct QueryFixture {
    endpoint: String,
    held_started: Arc<AtomicBool>,
    requests: Arc<Mutex<Vec<String>>>,
    release_held: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    workers: Arc<Mutex<Vec<JoinHandle<()>>>>,
    handle: Option<JoinHandle<()>>,
}

impl QueryFixture {
    fn start(hold: HoldPhase) -> Self {
        let listener =
            TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).expect("bind query fixture");
        listener
            .set_nonblocking(true)
            .expect("nonblocking query fixture");
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
            "the CLI never reached the held query response: {:?}",
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
            handle.join().expect("join query fixture listener");
        }
        let workers = std::mem::take(&mut *self.workers.lock().expect("fixture workers"));
        for worker in workers {
            worker.join().expect("join query fixture worker");
        }
        self.requests()
    }
}

impl Drop for QueryFixture {
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

    let held_phase = matches!(
        (hold, method.as_str(), path_without_query),
        (HoldPhase::Auth, "GET", "/api/1/users/me")
            | (HoldPhase::AuthBody, "GET", "/api/1/users/me")
            | (HoldPhase::Devices, "GET", "/api/1/users/getDevice")
            | (
                HoldPhase::Storage,
                "GET",
                "/api/1/statistics/v2/user/storage"
            )
            | (
                HoldPhase::StorageBody,
                "GET",
                "/api/1/statistics/v2/user/storage"
            )
            | (
                HoldPhase::ReadingInfo,
                "GET",
                "/api/1/statistics/readingInfo"
            )
            | (HoldPhase::Week, "GET", "/api/1/statistics/readTimeInfo")
            | (HoldPhase::Day, "GET", "/api/1/statistics/dayRead")
    );
    if held_phase {
        if matches!(hold, HoldPhase::AuthBody | HoldPhase::StorageBody) {
            let (status, body) = fixture_response(&method, path_without_query);
            let split = body.len() / 2;
            write_response_prefix(&mut stream, status, &body, split);
            held.store(true, Ordering::SeqCst);
            if wait_until_released(&release, &stopping) {
                let _ = stream.write_all(&body.as_bytes()[split..]);
            }
            return;
        }
        if hold_until_released(&held, &release, &stopping) {
            let (status, body) = fixture_response(&method, path_without_query);
            write_response(&mut stream, status, &body);
        }
        return;
    }

    let (status, body) = fixture_response(&method, path_without_query);
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

fn fixture_response(method: &str, path: &str) -> (&'static str, String) {
    match (method, path) {
        ("GET", "/api/1/users/me") => (
            "200 OK",
            api_envelope(r#"{"uid":"uid-fixture","nickname":"reader"}"#),
        ),
        ("GET", "/api/1/users/getDevice") => (
            "200 OK",
            api_envelope(r#"[{"id":"device-1","model":"BOOX Note","loginStatus":"online"}]"#),
        ),
        ("GET", "/api/1/statistics/v2/user/storage") => (
            "200 OK",
            api_envelope(r#"{"usedSize":1000,"totalSize":4000}"#),
        ),
        ("GET", "/api/1/statistics/readingInfo") => (
            "200 OK",
            api_envelope(r#"{"todayReadSeconds":125,"totalReadSeconds":9000}"#),
        ),
        ("GET", "/api/1/statistics/readTimeInfo") => ("200 OK", api_envelope(r#"{"seconds":700}"#)),
        ("GET", "/api/1/statistics/dayRead") => {
            ("200 OK", api_envelope(r#"[{"title":"chapter-one"}]"#))
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

fn spawn_query(dir: &TestDir, endpoint: &str, args: &[&str]) -> ChildGuard {
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
        "SIGINT must stop the held query request promptly, took {elapsed:?}; stdout: {} stderr: {}",
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
    assert_eq!(value["error"]["code"], "cancelled");
    assert!(!stdout(output).contains("fixture-token"));
    assert!(!stdout(output).contains("device-1"));
}

fn count(records: &[String], prefix: &str) -> usize {
    records
        .iter()
        .filter(|line| line.starts_with(prefix))
        .count()
}

fn run_held_phase(hold: HoldPhase, args: &[&str]) -> (QueryFixture, TestDir, ChildGuard) {
    let dir = TestDir::new();
    let server = QueryFixture::start(hold);
    let child = spawn_query(&dir, server.endpoint(), args);
    server.wait_until_held();
    (server, dir, child)
}

#[test]
fn sigint_while_devices_response_is_slow_exits_promptly_without_a_result() {
    let (mut server, _dir, child) =
        run_held_phase(HoldPhase::Devices, &["--json", "devices", "list"]);
    let (output, elapsed) = interrupted_output(child);
    assert_prompt_cancel(&output, elapsed);
    let records = server.finish();
    assert_eq!(count(&records, "GET /api/1/users/me"), 1);
    assert_eq!(count(&records, "GET /api/1/users/getDevice"), 1);
}

#[test]
fn sigint_while_storage_response_is_slow_exits_promptly_without_a_result() {
    let (mut server, _dir, child) =
        run_held_phase(HoldPhase::Storage, &["--json", "storage", "status"]);
    let (output, elapsed) = interrupted_output(child);
    assert_prompt_cancel(&output, elapsed);
    let records = server.finish();
    assert_eq!(count(&records, "GET /api/1/users/me"), 1);
    assert_eq!(count(&records, "GET /api/1/statistics/v2/user/storage"), 1);
}

#[test]
fn sigint_while_account_response_is_slow_exits_promptly_before_devices() {
    let (mut server, _dir, child) = run_held_phase(HoldPhase::Auth, &["--json", "devices", "list"]);
    let (output, elapsed) = interrupted_output(child);
    assert_prompt_cancel(&output, elapsed);
    let records = server.finish();
    assert_eq!(count(&records, "GET /api/1/users/me"), 1);
    assert_eq!(count(&records, "GET /api/1/users/getDevice"), 0);
}

#[test]
fn sigint_while_reading_info_is_slow_stops_before_week_and_day_requests() {
    let (mut server, _dir, child) = run_held_phase(
        HoldPhase::ReadingInfo,
        &["--json", "reading", "stats", "--date", "2026-10-29"],
    );
    let (output, elapsed) = interrupted_output(child);
    assert_prompt_cancel(&output, elapsed);
    let records = server.finish();
    assert_eq!(count(&records, "GET /api/1/statistics/readingInfo"), 1);
    assert_eq!(count(&records, "GET /api/1/statistics/readTimeInfo"), 0);
    assert_eq!(count(&records, "GET /api/1/statistics/dayRead"), 0);
}

#[test]
fn sigint_while_week_read_time_is_slow_stops_before_day_read() {
    let (mut server, _dir, child) = run_held_phase(
        HoldPhase::Week,
        &["--json", "reading", "stats", "--date", "2026-10-29"],
    );
    let (output, elapsed) = interrupted_output(child);
    assert_prompt_cancel(&output, elapsed);
    let records = server.finish();
    assert_eq!(count(&records, "GET /api/1/statistics/readingInfo"), 1);
    assert_eq!(count(&records, "GET /api/1/statistics/readTimeInfo"), 1);
    assert_eq!(count(&records, "GET /api/1/statistics/dayRead"), 0);
}

#[test]
fn sigint_while_day_read_is_slow_exits_promptly_without_a_result() {
    let (mut server, _dir, child) = run_held_phase(
        HoldPhase::Day,
        &["--json", "reading", "stats", "--date", "2026-10-29"],
    );
    let (output, elapsed) = interrupted_output(child);
    assert_prompt_cancel(&output, elapsed);
    let records = server.finish();
    assert_eq!(count(&records, "GET /api/1/statistics/readingInfo"), 1);
    assert_eq!(count(&records, "GET /api/1/statistics/readTimeInfo"), 1);
    assert_eq!(count(&records, "GET /api/1/statistics/dayRead"), 1);
}

#[test]
fn sigint_while_api_envelope_body_stalls_exits_promptly_without_a_result() {
    let (mut server, _dir, child) =
        run_held_phase(HoldPhase::AuthBody, &["--json", "devices", "list"]);
    let (output, elapsed) = interrupted_output(child);
    assert_prompt_cancel(&output, elapsed);
    let records = server.finish();
    assert_eq!(count(&records, "GET /api/1/users/me"), 1);
    assert_eq!(count(&records, "GET /api/1/users/getDevice"), 0);
}

#[test]
fn sigint_while_storage_envelope_body_stalls_exits_promptly_without_a_result() {
    let (mut server, _dir, child) =
        run_held_phase(HoldPhase::StorageBody, &["--json", "storage", "status"]);
    let (output, elapsed) = interrupted_output(child);
    assert_prompt_cancel(&output, elapsed);
    let records = server.finish();
    assert_eq!(count(&records, "GET /api/1/users/me"), 1);
    assert_eq!(count(&records, "GET /api/1/statistics/v2/user/storage"), 1);
    assert_eq!(records.len(), 2);
}

#[test]
fn cancelled_before_query_request_sends_nothing() {
    static CANCEL: AtomicBool = AtomicBool::new(false);
    CANCEL.store(true, Ordering::SeqCst);
    let mut server = QueryFixture::start(HoldPhase::Devices);
    let mut service = QueryService::new(
        HttpQueryBackend::with_test_endpoints(
            Duration::from_secs(300),
            "fixture-token",
            server.endpoint(),
            server.endpoint(),
        )
        .expect("create backend")
        .with_cancel_flag(&CANCEL),
    );
    let error = service.devices().expect_err("pre-cancelled query");
    assert_eq!(error.kind, ServiceErrorKind::Cancelled);
    assert_eq!(error.code, "cancelled");
    assert!(server.finish().is_empty());
}

fn release_after_cancel(
    server: &QueryFixture,
    flag: &'static AtomicBool,
) -> std::thread::JoinHandle<()> {
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
fn completed_identity_response_does_not_start_devices_after_cancellation() {
    static CANCEL: AtomicBool = AtomicBool::new(false);
    CANCEL.store(false, Ordering::SeqCst);
    let mut server = QueryFixture::start(HoldPhase::Auth);
    let mut service = QueryService::new(
        HttpQueryBackend::with_test_endpoints(
            Duration::from_secs(300),
            "fixture-token",
            server.endpoint(),
            server.endpoint(),
        )
        .expect("create backend")
        .with_cancel_flag(&CANCEL),
    );
    let controller = release_after_cancel(&server, &CANCEL);
    let error = service.devices().expect_err("cancelled identity");
    controller.join().expect("join cancellation controller");
    assert_eq!(error.kind, ServiceErrorKind::Cancelled);
    let records = server.finish();
    assert_eq!(count(&records, "GET /api/1/users/me"), 1);
    assert_eq!(count(&records, "GET /api/1/users/getDevice"), 0);
}

#[test]
fn completed_reading_info_response_does_not_start_week_after_cancellation() {
    static CANCEL: AtomicBool = AtomicBool::new(false);
    CANCEL.store(false, Ordering::SeqCst);
    let mut server = QueryFixture::start(HoldPhase::ReadingInfo);
    let mut service = QueryService::new(
        HttpQueryBackend::with_test_endpoints(
            Duration::from_secs(300),
            "fixture-token",
            server.endpoint(),
            server.endpoint(),
        )
        .expect("create backend")
        .with_cancel_flag(&CANCEL),
    );
    let controller = release_after_cancel(&server, &CANCEL);
    let error = service
        .reading("2026-10-29")
        .expect_err("cancelled reading report");
    controller.join().expect("join cancellation controller");
    assert_eq!(error.kind, ServiceErrorKind::Cancelled);
    let records = server.finish();
    assert_eq!(count(&records, "GET /api/1/statistics/readingInfo"), 1);
    assert_eq!(count(&records, "GET /api/1/statistics/readTimeInfo"), 0);
    assert_eq!(count(&records, "GET /api/1/statistics/dayRead"), 0);
}
