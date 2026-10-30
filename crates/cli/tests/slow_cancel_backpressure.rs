//! SIGINT while the upload socket is backpressured.
//!
//! The fixture accepts the OSS PUT and reads only a small prefix of the body,
//! leaving the client's socket send buffer full. The upload request future then
//! waits for writability and cannot make progress from the body reader alone.

#![cfg(unix)]

mod common;

use common::{json_stdout, stderr, stdout, TestDir, TEST_API_ENDPOINT_ENV, TEST_OSS_ENDPOINT_ENV};
use std::{
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
const SERVER_DEADLINE: Duration = Duration::from_secs(20);
const CHILD_DEADLINE: Duration = Duration::from_secs(10);
const PROMPT_EXIT: Duration = Duration::from_secs(3);

struct SlowUploadFixture {
    endpoint: String,
    requests: Arc<Mutex<Vec<String>>>,
    upload_started: Arc<AtomicBool>,
    upload_bytes: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl SlowUploadFixture {
    fn start() -> Self {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .expect("bind slow upload fixture");
        listener.set_nonblocking(true).expect("nonblocking fixture");
        let endpoint = format!("http://{}", listener.local_addr().expect("fixture address"));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let upload_started = Arc::new(AtomicBool::new(false));
        let upload_bytes = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));

        let observed = Arc::clone(&requests);
        let started = Arc::clone(&upload_started);
        let body_bytes = Arc::clone(&upload_bytes);
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
                    .set_read_timeout(Some(Duration::from_secs(1)))
                    .expect("fixture read timeout");
                stream
                    .set_write_timeout(Some(Duration::from_secs(1)))
                    .expect("fixture write timeout");
                let mut reader = BufReader::new(stream.try_clone().expect("clone fixture"));
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
                observed
                    .lock()
                    .expect("fixture requests")
                    .push(request_line.trim_end().to_string());

                let method = request_line.split_whitespace().next().unwrap_or("GET");
                let path = request_line.split_whitespace().nth(1).unwrap_or("/");
                let path_without_query = path.split('?').next().unwrap_or(path);

                if method == "PUT" && path_without_query.starts_with("/uid-fixture/push/") {
                    started.store(true, Ordering::SeqCst);
                    // Reading one prefix and then stalling fills the socket
                    // and HTTP buffers, so the upload future stays pending.
                    stall_after_upload_prefix(
                        &mut reader,
                        content_length,
                        &body_bytes,
                        &stopping,
                        SERVER_DEADLINE,
                    );
                    continue;
                }

                let mut body = vec![0u8; content_length];
                if content_length > 0 && reader.read_exact(&mut body).is_err() {
                    continue;
                }
                let (status, response) = fixture_response(method, path_without_query);
                write_response(&mut stream, status, &response);
            }
        });

        Self {
            endpoint,
            requests,
            upload_started,
            upload_bytes,
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

    fn upload_bytes(&self) -> usize {
        self.upload_bytes.load(Ordering::SeqCst)
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

impl Drop for SlowUploadFixture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let Some(handle) = self.handle.take() else {
            return;
        };
        let deadline = Instant::now() + Duration::from_secs(2);
        while !handle.is_finished() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(2));
        }
        if handle.is_finished() {
            let result = handle.join();
            if !std::thread::panicking() {
                result.expect("slow upload fixture panicked");
            }
        } else if !std::thread::panicking() {
            panic!("slow upload fixture did not stop within two seconds");
        }
    }
}

fn stall_after_upload_prefix(
    reader: &mut BufReader<TcpStream>,
    content_length: usize,
    body_bytes: &AtomicUsize,
    stop: &AtomicBool,
    limit: Duration,
) {
    let deadline = Instant::now() + limit;
    let mut first = [0u8; 8192];
    let prefix_len = content_length.min(first.len());
    if reader.read_exact(&mut first[..prefix_len]).is_err() {
        return;
    }
    body_bytes.fetch_add(prefix_len, Ordering::SeqCst);
    // Leave the remaining body unread so the client's HTTP and kernel send
    // buffers fill. The explicit content length identifies the request even
    // though this fixture deliberately never consumes the complete body.
    while !stop.load(Ordering::SeqCst) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
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
    spawn_push_with_timeout(dir, endpoint, files, "300")
}

fn spawn_push_with_timeout(
    dir: &TestDir,
    endpoint: &str,
    files: &[&Path],
    timeout_secs: &str,
) -> ChildGuard {
    let mut command = Command::new(env!("CARGO_BIN_EXE_sandflos"));
    command.args(["--json", "push"]);
    for file in files {
        command.arg(file);
    }
    command
        .args(["--timeout", timeout_secs])
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

fn sparse_file(dir: &TestDir, name: &str, bytes: u64) -> std::path::PathBuf {
    let path = dir.path().join(name);
    let file = File::create(&path).expect("create sparse file");
    file.set_len(bytes).expect("size sparse file");
    path
}

#[test]
fn sigint_stops_a_backpressured_upload_without_waiting_for_http_timeout() {
    let dir = TestDir::new();
    let server = SlowUploadFixture::start();
    let first = sparse_file(&dir, "large.pdf", LARGE_FILE_BYTES);
    let second = dir.path().join("second.pdf");
    std::fs::write(&second, b"second").expect("write second fixture");

    let child = spawn_push(&dir, server.endpoint(), &[&first, &second]);
    server.wait_until_uploading();
    // Wait until the client has written enough for the socket buffers to fill
    // and the sender to block. The server has drained only a small prefix.
    let fill_deadline = Instant::now() + Duration::from_secs(5);
    while server.upload_bytes() < 4096 && Instant::now() < fill_deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(
        server.upload_bytes() >= 4096,
        "fixture never received a prefix"
    );
    std::thread::sleep(Duration::from_millis(1500));

    let interrupted = Instant::now();
    send_sigint(child.pid());
    let output = child.wait_with_deadline(CHILD_DEADLINE);
    let elapsed = interrupted.elapsed();

    assert!(
        elapsed < PROMPT_EXIT,
        "SIGINT must stop a backpressured upload promptly, took {elapsed:?}; server received {} of {} bytes; stdout: {} stderr: {}",
        server.upload_bytes(),
        LARGE_FILE_BYTES,
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
    assert_eq!(value["data"]["files"][0]["stage"], "upload_oss");
    assert_eq!(value["data"]["files"][0]["error"]["kind"], "cancelled");
    assert_eq!(value["data"]["files"][1]["stage"], "prepare");
    assert_eq!(
        value["data"]["files"][1]["path"],
        second.to_string_lossy().as_ref()
    );
    assert_eq!(value["data"]["files"][1]["error"]["kind"], "cancelled");
    assert_eq!(server.count_requests("PUT /uid-fixture/push/"), 1);
    assert_eq!(server.count_requests("PUT /neocloud/"), 0);
    assert_eq!(server.count_requests("POST /api/1/push/saveAndPush"), 0);
}

#[test]
fn configured_timeout_still_ends_a_backpressured_upload_without_sigint() {
    let dir = TestDir::new();
    let server = SlowUploadFixture::start();
    let first = sparse_file(&dir, "large.pdf", LARGE_FILE_BYTES);

    let child = spawn_push_with_timeout(&dir, server.endpoint(), &[&first], "1");
    server.wait_until_uploading();
    let started = Instant::now();
    let output = child.wait_with_deadline(CHILD_DEADLINE);
    let elapsed = started.elapsed();

    assert!(
        elapsed < PROMPT_EXIT,
        "the configured upload timeout must still bound a stalled request, took {elapsed:?}"
    );
    assert_eq!(
        output.status.code(),
        Some(4),
        "stdout: {} stderr: {}",
        stdout(&output),
        stderr(&output)
    );
    let value = json_stdout(&output);
    assert_eq!(value["error"]["code"], "push_incomplete");
    assert_eq!(value["data"]["files"][0]["stage"], "upload_oss");
    assert_eq!(value["data"]["files"][0]["error"]["kind"], "network");
    assert_eq!(server.count_requests("PUT /uid-fixture/push/"), 1);
    assert_eq!(server.count_requests("PUT /neocloud/"), 0);
    assert_eq!(server.count_requests("POST /api/1/push/saveAndPush"), 0);
}
