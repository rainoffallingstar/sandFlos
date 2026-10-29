#![allow(dead_code)]

use serde_json::Value;
use std::{
    env, fs,
    io::{BufRead, BufReader, Write},
    net::TcpListener,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    thread::JoinHandle,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

pub const TEST_AUTH_ENDPOINT_ENV: &str = "SANDFLOS_TEST_AUTH_BASE_URL";
pub const TEST_API_ENDPOINT_ENV: &str = "SANDFLOS_TEST_API_BASE_URL";
pub const TEST_OSS_ENDPOINT_ENV: &str = "SANDFLOS_TEST_OSS_BASE_URL";

static NEXT_ID: AtomicU64 = AtomicU64::new(0);

pub struct TestDir(PathBuf);

impl TestDir {
    pub fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time")
            .as_nanos();
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let path =
            env::temp_dir().join(format!("sandflos-cli-{}-{nonce}-{id}", std::process::id()));
        fs::create_dir_all(&path).expect("create test directory");
        Self(path)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    pub fn config_dir(&self) -> PathBuf {
        self.0.join("config")
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

pub fn command(args: &[&str], config_dir: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_sandflos"));
    command
        .args(args)
        .env("SANDFLOS_CONFIG_DIR", config_dir)
        .env("SANDFLOS_CREDENTIAL_STORE", "file")
        .env_remove("SANDFLOS_TOKEN")
        .env_remove(TEST_AUTH_ENDPOINT_ENV)
        .env_remove(TEST_API_ENDPOINT_ENV)
        .env_remove(TEST_OSS_ENDPOINT_ENV)
        .stdin(Stdio::null());
    command
}

pub fn command_with_auth(args: &[&str], config_dir: &Path, endpoint: &str) -> Command {
    let mut command = command(args, config_dir);
    command.env(TEST_AUTH_ENDPOINT_ENV, endpoint);
    command
}

pub fn command_with_api(args: &[&str], config_dir: &Path, endpoint: &str) -> Command {
    let mut command = command(args, config_dir);
    command.env(TEST_API_ENDPOINT_ENV, endpoint);
    command
}

pub fn command_with_api_and_oss(
    args: &[&str],
    config_dir: &Path,
    endpoint: &str,
    oss_endpoint: &str,
) -> Command {
    let mut command = command_with_api(args, config_dir, endpoint);
    command.env(TEST_OSS_ENDPOINT_ENV, oss_endpoint);
    command
}

pub fn run(args: &[&str], config_dir: &Path) -> Output {
    command(args, config_dir).output().expect("run sandflos")
}

pub fn run_with_api(args: &[&str], config_dir: &Path, endpoint: &str) -> Output {
    command_with_api(args, config_dir, endpoint)
        .output()
        .expect("run sandflos")
}

pub fn run_with_stdin(args: &[&str], config_dir: &Path, input: &str) -> Output {
    run_with_stdin_from(command(args, config_dir), input)
}

pub fn run_with_stdin_with_auth(
    args: &[&str],
    config_dir: &Path,
    endpoint: &str,
    input: &str,
) -> Output {
    run_with_stdin_from(command_with_auth(args, config_dir, endpoint), input)
}

fn run_with_stdin_from(mut command: Command, input: &str) -> Output {
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn sandflos");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(input.as_bytes())
        .expect("write stdin");
    child.wait_with_output().expect("wait for sandflos")
}

pub fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

pub fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

pub fn json_stdout(output: &Output) -> Value {
    let text = stdout(output);
    assert_eq!(text.lines().count(), 1, "stdout: {text}");
    serde_json::from_str(&text).expect("valid JSON envelope")
}

pub struct AuthServer {
    endpoint: String,
    requests: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl AuthServer {
    pub fn accepting() -> Self {
        Self::start(
            "200 OK",
            r#"{"result_code":0,"data":{"uid":"uid-fixture","nickname":"reader"}}"#,
        )
    }

    pub fn rejecting() -> Self {
        Self::start(
            "401 Unauthorized",
            r#"{"error":"server body must not be echoed"}"#,
        )
    }

    fn start(status: &'static str, body: &'static str) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind auth fixture");
        listener
            .set_nonblocking(true)
            .expect("nonblocking auth fixture");
        let endpoint = format!("http://{}", listener.local_addr().expect("fixture address"));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let observed = Arc::clone(&requests);
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
                let mut reader = BufReader::new(stream.try_clone().expect("clone stream"));
                let mut request_line = String::new();
                if reader.read_line(&mut request_line).is_err() {
                    continue;
                }
                let mut headers = Vec::new();
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).is_err() || line == "\r\n" || line.is_empty() {
                        break;
                    }
                    headers.push(line.trim_end().to_string());
                }
                observed
                    .lock()
                    .expect("auth requests")
                    .push(format!("{request_line}{}", headers.join("\n")));
                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes());
            }
        });
        Self {
            endpoint,
            requests,
            stop,
            handle: Some(handle),
        }
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    pub fn requests(&self) -> Vec<String> {
        self.requests.lock().expect("auth requests").clone()
    }
}

impl Drop for AuthServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

#[derive(Clone, Debug)]
pub struct RecordedRequest {
    pub method: String,
    pub path: String,
    pub headers: Vec<String>,
    pub body: String,
}

impl RecordedRequest {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find_map(|line| line.strip_prefix(&format!("{name}:")))
            .map(str::trim)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApiFault {
    None,
    /// Reject the bearer token at `/api/1/users/me`.
    CredentialRejected,
    /// Fail account lookup with a server error.
    ServerError,
    /// Return a non-JSON success body for account lookup.
    MalformedUser,
    /// Delete the push document, then fail the message-record deletion.
    DeleteMessageFails,
    /// Accept the file but fail `saveAndPush`, leaving the outcome unknown.
    SaveAndPushFails,
}

pub struct ApiServer {
    endpoint: String,
    requests: Arc<Mutex<Vec<RecordedRequest>>>,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl ApiServer {
    pub fn start() -> Self {
        Self::start_with_fault(ApiFault::None)
    }

    pub fn start_with_fault(fault: ApiFault) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind API fixture");
        listener
            .set_nonblocking(true)
            .expect("nonblocking API fixture");
        let endpoint = format!("http://{}", listener.local_addr().expect("fixture address"));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let observed = Arc::clone(&requests);
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
                let mut reader = BufReader::new(stream.try_clone().expect("clone stream"));
                let mut request_line = String::new();
                if reader.read_line(&mut request_line).is_err() {
                    continue;
                }
                let mut parts = request_line.split_whitespace();
                let method = parts.next().unwrap_or_default().to_string();
                let path = parts.next().unwrap_or_default().to_string();
                let mut headers = Vec::new();
                let mut content_length = 0usize;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).is_err() || line == "\r\n" || line.is_empty() {
                        break;
                    }
                    let trimmed = line.trim_end().to_string();
                    if let Some(value) = trimmed
                        .to_ascii_lowercase()
                        .strip_prefix("content-length:")
                        .and_then(|value| value.trim().parse::<usize>().ok())
                    {
                        content_length = value;
                    }
                    headers.push(trimmed);
                }
                let mut body = vec![0u8; content_length];
                if content_length > 0 {
                    use std::io::Read as _;
                    if reader.read_exact(&mut body).is_err() {
                        continue;
                    }
                }
                observed
                    .lock()
                    .expect("API requests")
                    .push(RecordedRequest {
                        method,
                        path,
                        headers,
                        body: String::from_utf8_lossy(&body).into_owned(),
                    });

                let (status, extra_headers, body) = api_fixture_response(
                    observed
                        .lock()
                        .expect("API requests")
                        .last()
                        .expect("request"),
                    fault,
                );
                let mut response = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n",
                    body.len()
                );
                for header in extra_headers {
                    response.push_str(header);
                    response.push_str("\r\n");
                }
                response.push_str("\r\n");
                response.push_str(&body);
                let _ = stream.write_all(response.as_bytes());
            }
        });
        Self {
            endpoint,
            requests,
            stop,
            handle: Some(handle),
        }
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    pub fn requests(&self) -> Vec<RecordedRequest> {
        self.requests.lock().expect("API requests").clone()
    }
}

impl Drop for ApiServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn api_fixture_response(
    request: &RecordedRequest,
    fault: ApiFault,
) -> (&'static str, Vec<&'static str>, String) {
    let path = request.path.split('?').next().unwrap_or_default();
    match (fault, request.method.as_str(), path) {
        (ApiFault::CredentialRejected, "GET", "/api/1/users/me") => {
            return (
                "401 Unauthorized",
                Vec::new(),
                r#"{"error":"server body must not be echoed"}"#.to_string(),
            )
        }
        (ApiFault::ServerError, "GET", "/api/1/users/me") => {
            return (
                "500 Internal Server Error",
                Vec::new(),
                r#"{"error":"fixture internal failure"}"#.to_string(),
            )
        }
        (ApiFault::MalformedUser, "GET", "/api/1/users/me") => {
            return ("200 OK", Vec::new(), "<html>not json</html>".to_string())
        }
        (ApiFault::DeleteMessageFails, "POST", "/api/1/push/message/batchDelete") => {
            return (
                "500 Internal Server Error",
                Vec::new(),
                r#"{"error":"fixture batch delete failure"}"#.to_string(),
            )
        }
        (ApiFault::SaveAndPushFails, "POST", "/api/1/push/saveAndPush") => {
            return (
                "500 Internal Server Error",
                Vec::new(),
                r#"{"error":"fixture saveAndPush failure"}"#.to_string(),
            )
        }
        _ => {}
    }
    let data = match (request.method.as_str(), path) {
        ("GET", "/api/1/users/me") => r#"{"uid":"uid-fixture","nickname":"reader"}"#,
        ("GET", "/api/1/statistics/v2/user/storage") => r#"{"usedSize":1000,"totalSize":4000}"#,
        ("GET", "/api/1/users/getDevice") => {
            r#"[{"id":"device-1","model":"BOOX Note","loginStatus":"online"}]"#
        }
        ("GET", "/api/1/statistics/readingInfo") => {
            r#"{"todayReadSeconds":125,"totalReadSeconds":9000}"#
        }
        ("GET", "/api/1/statistics/readTimeInfo") => r#"{"seconds":700}"#,
        ("GET", "/api/1/statistics/dayRead") => r#"[{"title":"chapter-one"}]"#,
        ("GET", "/api/1/users/syncToken") => {
            let body = r#"{"result_code":0,"data":{"cookie_name":"SyncGatewaySession","session_id":"fixture-session"}}"#;
            return (
                "200 OK",
                vec!["Set-Cookie: SyncGatewaySession=fixture-session; Path=/"],
                body.to_string(),
            );
        }
        ("GET", "/neocloud/_changes") => {
            let content = r#"{"formats":["pdf"],"storage":{"pdf":{"oss":{"key":"uid-fixture/push/fixture.pdf","size":42}}}}"#;
            let doc = serde_json::json!({
                "_id": "doc-fixture",
                "_rev": "1-fixture",
                "msgType": 2,
                "contentType": "digital_content",
                "name": "fixture.pdf",
                "size": 42,
                "updatedAt": 1000,
                "storageKey": "uid-fixture/push/fixture.pdf",
                "content": content,
            });
            return (
                "200 OK",
                Vec::new(),
                serde_json::json!({ "results": [{ "doc": doc }] }).to_string(),
            );
        }
        ("GET", "/neocloud/doc-fixture") => {
            let content = r#"{"formats":["pdf"],"storage":{"pdf":{"oss":{"key":"uid-fixture/push/fixture.pdf","size":42}}}}"#;
            return (
                "200 OK",
                Vec::new(),
                serde_json::json!({
                    "_id": "doc-fixture",
                    "_rev": "1-fixture",
                    "msgType": 2,
                    "contentType": "digital_content",
                    "name": "fixture.pdf",
                    "size": 42,
                    "storageKey": "uid-fixture/push/fixture.pdf",
                    "content": content,
                })
                .to_string(),
            );
        }
        ("DELETE", "/neocloud/doc-fixture") => return ("200 OK", Vec::new(), "{}".to_string()),
        ("POST", "/api/1/push/message/batchDelete") => {
            return (
                "200 OK",
                Vec::new(),
                r#"{"result_code":0,"data":null}"#.to_string(),
            )
        }
        ("GET", "/api/1/config/buckets") => {
            r#"{"onyx-cloud":{"bucket":"onyx-cloud","aliEndpoint":"https://oss-fixture.invalid"}}"#
        }
        ("GET", "/api/1/config/stss") => {
            r#"{"AccessKeyId":"fixture-key","AccessKeySecret":"fixture-secret","SecurityToken":"fixture-security","Expiration":"2099-01-01T00:00:00Z"}"#
        }
        ("PUT", path) if path.starts_with("/uid-fixture/push/") => {
            return ("200 OK", Vec::new(), String::new())
        }
        ("GET", path) if path.starts_with("/uid-fixture/push/") => {
            return ("206 Partial Content", Vec::new(), String::new())
        }
        ("PUT", path) if path.starts_with("/neocloud/") => {
            let id = path.trim_start_matches("/neocloud/");
            return (
                "201 Created",
                Vec::new(),
                serde_json::json!({ "id": id, "rev": "1-fixture" }).to_string(),
            );
        }
        ("GET", path) if path.starts_with("/neocloud/") => {
            let id = path.trim_start_matches("/neocloud/");
            if id == "doc-fixture" {
                // Handled above for queue tests.
                unreachable!();
            }
            let content = r#"{"formats":["pdf"],"storage":{"pdf":{"oss":{"key":"uid-fixture/push/fixture.pdf","size":42}}}}"#;
            return (
                "200 OK",
                Vec::new(),
                serde_json::json!({
                    "_id": id,
                    "_rev": "1-fixture",
                    "msgType": 2,
                    "contentType": "digital_content",
                    "name": "book.pdf",
                    "size": 42,
                    "storageKey": "uid-fixture/push/fixture.pdf",
                    "content": content,
                })
                .to_string(),
            );
        }
        ("POST", "/api/1/push/saveAndPush") => {
            return (
                "200 OK",
                Vec::new(),
                r#"{"result_code":0,"data":{"accepted":true}}"#.to_string(),
            )
        }
        _ => {
            return (
                "404 Not Found",
                Vec::new(),
                r#"{"error":"unexpected fixture request"}"#.to_string(),
            )
        }
    };
    (
        "200 OK",
        Vec::new(),
        format!(r#"{{"result_code":0,"message":null,"data":{data}}}"#),
    )
}
