//! SIGINT must stop an in-flight upload before the message write/submit.
//!
//! The loopback OSS endpoint deliberately consumes the request body slowly.
//! That keeps the upload in progress long enough to deliver a real SIGINT to
//! the CLI process and observes whether the reader stops or continues into
//! neocloud message creation.

#![cfg(unix)]

mod common;

use common::{json_stdout, stderr, stdout, TestDir, TEST_API_ENDPOINT_ENV, TEST_OSS_ENDPOINT_ENV};
use std::{
    env,
    io::{BufRead, BufReader, Read, Write},
    net::TcpListener,
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

struct SlowOss {
    endpoint: String,
    requests: Arc<Mutex<Vec<String>>>,
    upload_started: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl SlowOss {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind slow OSS");
        listener.set_nonblocking(true).expect("nonblocking OSS");
        let endpoint = format!("http://{}", listener.local_addr().expect("address"));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let upload_started = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));
        let observed = Arc::clone(&requests);
        let started = Arc::clone(&upload_started);
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
                stream.set_nonblocking(false).expect("blocking OSS");
                stream
                    .set_read_timeout(Some(Duration::from_secs(10)))
                    .expect("OSS read timeout");
                stream
                    .set_write_timeout(Some(Duration::from_secs(10)))
                    .expect("OSS write timeout");
                let mut reader = BufReader::new(stream.try_clone().expect("clone OSS"));
                let mut request_line = String::new();
                if reader.read_line(&mut request_line).is_err() {
                    continue;
                }
                let mut headers = Vec::new();
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
                    headers.push(line.trim_end().to_string());
                }
                observed
                    .lock()
                    .expect("OSS requests")
                    .push(request_line.clone());
                let method = request_line.split_whitespace().next().unwrap_or("GET");
                let path = request_line.split_whitespace().nth(1).unwrap_or("/");
                if method == "PUT" && path.starts_with("/uid-fixture/push/") {
                    started.store(true, Ordering::SeqCst);
                    // Consume slowly so SIGINT arrives while the body is still
                    // streaming. Stop reading as soon as the client aborts.
                    let mut remaining = content_length;
                    let mut chunk = [0u8; 2048];
                    while remaining > 0 {
                        let want = remaining.min(chunk.len());
                        match reader.read(&mut chunk[..want]) {
                            Ok(0) | Err(_) => break,
                            Ok(read) => {
                                remaining -= read;
                                std::thread::sleep(Duration::from_millis(25));
                            }
                        }
                    }
                    let _ = stream.write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    );
                    continue;
                }
                let (status, body) = fixture_response(method, path);
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
            upload_started,
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
            self.requests.lock().expect("OSS requests")
        );
    }

    fn requests(&self) -> Vec<String> {
        self.requests.lock().expect("OSS requests").clone()
    }
}

impl Drop for SlowOss {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn fixture_response(method: &str, path: &str) -> (&'static str, String) {
    match (method, path) {
        ("GET", "/api/1/users/me") => (
            "200 OK",
            r#"{"result_code":0,"data":{"uid":"uid-fixture","nickname":"reader"}}"#.to_string(),
        ),
        ("GET", "/api/1/statistics/v2/user/storage") => (
            "200 OK",
            r#"{"result_code":0,"data":{"usedSize":0,"totalSize":1000000000}}"#.to_string(),
        ),
        ("GET", "/api/1/config/buckets") => (
            "200 OK",
            r#"{"result_code":0,"data":{"onyx-cloud":{"bucket":"onyx-cloud","aliEndpoint":"https://oss-fixture.invalid"}}}"#.to_string(),
        ),
        ("GET", "/api/1/config/stss") => (
            "200 OK",
            r#"{"result_code":0,"data":{"AccessKeyId":"id","AccessKeySecret":"secret","SecurityToken":"sts","Expiration":"2099-01-01T00:00:00Z"}}"#.to_string(),
        ),
        _ => ("404 Not Found", r#"{"error":"unexpected request"}"#.to_string()),
    }
}

extern "C" {
    fn kill(pid: i32, sig: i32) -> i32;
}

#[test]
fn sigint_during_upload_exits_130_without_creating_a_message() {
    let dir = TestDir::new();
    let server = SlowOss::start();
    let file = dir.path().join("big.pdf");
    std::fs::write(&file, vec![b'x'; 512 * 1024]).expect("write fixture");

    let child = Command::new(env!("CARGO_BIN_EXE_sandflos"))
        .args([
            "--json",
            "push",
            file.to_str().expect("utf-8 path"),
            "--timeout",
            "30",
        ])
        .env("SANDFLOS_CONFIG_DIR", dir.config_dir())
        .env("SANDFLOS_CREDENTIAL_STORE", "file")
        .env("SANDFLOS_TOKEN", "fixture-token")
        .env(TEST_API_ENDPOINT_ENV, server.endpoint())
        .env(TEST_OSS_ENDPOINT_ENV, server.endpoint())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn sandflos");

    server.wait_until_uploading();
    let rc = unsafe { kill(child.id() as i32, 2) };
    assert_eq!(rc, 0, "SIGINT must be delivered");
    let output = child.wait_with_output().expect("wait for sandflos");

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
    let requests = server.requests();
    assert!(
        requests
            .iter()
            .all(|line| !line.starts_with("PUT /neocloud/")
                && !line.starts_with("POST /api/1/push/saveAndPush")),
        "cancellation must stop before message creation: {requests:#?}"
    );
}
