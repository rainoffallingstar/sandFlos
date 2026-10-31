//! Continuous file-read progress is distinct from OSS acknowledgment.
//!
//! The loopback server consumes the complete PUT body but withholds its HTTP
//! response. A non-JSON CLI invocation must report bytes read while it waits,
//! and must not claim the upload was acknowledged until the response arrives.

mod common;

use common::{stderr, stdout, TestDir, TEST_API_ENDPOINT_ENV, TEST_OSS_ENDPOINT_ENV};
use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    net::{TcpListener, TcpStream},
    path::Path,
    process::{Child, Command, Output, Stdio},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError},
        Arc, Mutex,
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

const FILE_BYTES: usize = 1024 * 1024;
const SERVER_DEADLINE: Duration = Duration::from_secs(15);
const CHILD_DEADLINE: Duration = Duration::from_secs(10);
const PROGRESS_DEADLINE: Duration = Duration::from_secs(3);

struct ProgressFixture {
    endpoint: String,
    upload_complete: Arc<AtomicBool>,
    upload_bytes: Arc<AtomicUsize>,
    uploads: Arc<AtomicUsize>,
    release_response: Arc<AtomicBool>,
    requests: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl ProgressFixture {
    fn start() -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind progress fixture");
        listener.set_nonblocking(true).expect("nonblocking fixture");
        let endpoint = format!("http://{}", listener.local_addr().expect("fixture address"));
        let upload_complete = Arc::new(AtomicBool::new(false));
        let upload_bytes = Arc::new(AtomicUsize::new(0));
        let uploads = Arc::new(AtomicUsize::new(0));
        let release_response = Arc::new(AtomicBool::new(false));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));

        let complete = Arc::clone(&upload_complete);
        let bytes = Arc::clone(&upload_bytes);
        let counted = Arc::clone(&uploads);
        let release = Arc::clone(&release_response);
        let observed = Arc::clone(&requests);
        let stopping = Arc::clone(&stop);
        let handle = std::thread::spawn(move || {
            while !stopping.load(Ordering::SeqCst) {
                let (mut stream, _) = match listener.accept() {
                    Ok(value) => value,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
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
                    counted.fetch_add(1, Ordering::SeqCst);
                    let mut body = vec![0u8; content_length];
                    if reader.read_exact(&mut body).is_err() {
                        continue;
                    }
                    bytes.store(body.len(), Ordering::SeqCst);
                    complete.store(true, Ordering::SeqCst);
                    let deadline = Instant::now() + SERVER_DEADLINE;
                    while !release.load(Ordering::SeqCst)
                        && !stopping.load(Ordering::SeqCst)
                        && Instant::now() < deadline
                    {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    // The body has been consumed, but the client has not been
                    // told that OSS accepted it until this response is sent.
                    let _ = stream.write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
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
            upload_complete,
            upload_bytes,
            uploads,
            release_response,
            requests,
            stop,
            handle: Some(handle),
        }
    }

    fn endpoint(&self) -> &str {
        &self.endpoint
    }

    fn wait_until_uploaded(&self) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if self.upload_complete.load(Ordering::SeqCst) {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!(
            "the CLI never delivered the complete upload body: {:?}",
            self.requests()
        );
    }

    fn release(&self) {
        self.release_response.store(true, Ordering::SeqCst);
    }

    fn upload_bytes(&self) -> usize {
        self.upload_bytes.load(Ordering::SeqCst)
    }

    fn uploads(&self) -> usize {
        self.uploads.load(Ordering::SeqCst)
    }

    fn requests(&self) -> Vec<String> {
        self.requests.lock().expect("fixture requests").clone()
    }
}

impl Drop for ProgressFixture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        self.release_response.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            let deadline = Instant::now() + Duration::from_secs(2);
            while !handle.is_finished() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(5));
            }
            if handle.is_finished() {
                let _ = handle.join();
            }
        }
    }
}

fn fixture_response(method: &str, path: &str) -> (&'static str, String) {
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
        ("POST", "/api/1/push/saveAndPush") => ("200 OK", envelope(r#"{"accepted":true}"#)),
        _ => (
            "404 Not Found",
            r#"{"error":"unexpected fixture request"}"#.to_string(),
        ),
    }
}

fn envelope(data: &str) -> String {
    format!(r#"{{"result_code":0,"data":{data}}}"#)
}

fn write_response(stream: &mut TcpStream, status: &str, body: &str) {
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes());
}

struct ChildGuard {
    child: Option<Child>,
}

impl ChildGuard {
    fn new(child: Child) -> Self {
        Self { child: Some(child) }
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

fn spawn_push(dir: &TestDir, endpoint: &str, file: &Path) -> (ChildGuard, Receiver<String>) {
    let mut command = Command::new(env!("CARGO_BIN_EXE_sandflos"));
    command
        .args([
            "push",
            file.to_str().expect("utf-8 path"),
            "--timeout",
            "300",
        ])
        .env("SANDFLOS_CONFIG_DIR", dir.config_dir())
        .env("SANDFLOS_CREDENTIAL_STORE", "file")
        .env("SANDFLOS_TOKEN", "fixture-token")
        .env(TEST_API_ENDPOINT_ENV, endpoint)
        .env(TEST_OSS_ENDPOINT_ENV, endpoint)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().expect("spawn sandflos");
    let stderr_pipe = child.stderr.take().expect("child stderr");
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stderr_pipe).lines() {
            let Ok(line) = line else {
                break;
            };
            if sender.send(line).is_err() {
                break;
            }
        }
    });
    (ChildGuard::new(child), receiver)
}

fn parse_read_progress(line: &str) -> Option<(u64, u64)> {
    let (_, counts) = line.rsplit_once(": read ")?;
    let counts = counts.strip_suffix(" bytes")?;
    let (bytes, total) = counts.split_once('/')?;
    Some((bytes.parse().ok()?, total.parse().ok()?))
}

#[test]
fn slow_oss_response_reports_read_bytes_before_acknowledgment() {
    let dir = TestDir::new();
    let fixture = ProgressFixture::start();
    let file = dir.path().join("book.pdf");
    fs::write(&file, vec![b'r'; FILE_BYTES]).expect("write fixture");

    let (child, lines) = spawn_push(&dir, fixture.endpoint(), &file);
    fixture.wait_until_uploaded();
    assert_eq!(
        fixture.upload_bytes(),
        FILE_BYTES,
        "the fixture must have consumed the complete body before withholding the response"
    );

    let started = Instant::now();
    let mut observed = Vec::new();
    let mut reads = Vec::new();
    loop {
        match lines.recv_timeout(PROGRESS_DEADLINE.saturating_sub(started.elapsed())) {
            Ok(line) => {
                observed.push(line.clone());
                if let Some(sample) = parse_read_progress(&line) {
                    reads.push(sample);
                    if sample == (FILE_BYTES as u64, FILE_BYTES as u64) {
                        break;
                    }
                } else if line.contains("uploaded ") || line.contains("accepted") {
                    panic!(
                        "completing the file read before the OSS acknowledgment must not report success: {observed:?}"
                    );
                }
            }
            Err(RecvTimeoutError::Timeout) => panic!(
                "the final read count was not reported while the OSS response was withheld; stderr so far: {observed:?}"
            ),
            Err(RecvTimeoutError::Disconnected) => {
                panic!("sandflos stopped before reporting the final read count: {observed:?}")
            }
        }
    }

    fixture.release();
    let output = child.wait_with_deadline(CHILD_DEADLINE);
    while let Ok(line) = lines.try_recv() {
        if let Some(sample) = parse_read_progress(&line) {
            reads.push(sample);
        }
        observed.push(line);
    }
    assert!(
        !reads.is_empty(),
        "the upload must report continuous local reads: {observed:?}"
    );
    assert!(
        reads.windows(2).all(|pair| pair[0].0 < pair[1].0),
        "read progress must be strictly monotonic: {reads:?}"
    );
    assert!(
        reads
            .iter()
            .all(|(bytes, total)| *total == FILE_BYTES as u64 && bytes <= total),
        "read progress must stay bounded by the file size: {reads:?}"
    );
    assert_eq!(
        reads.last().copied(),
        Some((FILE_BYTES as u64, FILE_BYTES as u64)),
        "the final read sample must equal the complete file size: {reads:?}"
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "stdout: {} stderr: {}",
        stdout(&output),
        stderr(&output)
    );
    assert!(
        observed
            .iter()
            .any(|line| line.contains("uploaded 1048576 bytes")),
        "the acknowledged upload must still be reported after the response: {observed:?}"
    );
    assert!(stdout(&output).contains("accepted\t"));
    assert_eq!(fixture.uploads(), 1, "a single PUT must carry the body");
    assert_eq!(
        fixture
            .requests()
            .iter()
            .filter(|line| line.starts_with("POST /api/1/push/saveAndPush"))
            .count(),
        1
    );
}
