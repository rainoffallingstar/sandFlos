//! SIGINT cancellation at the authentication persistence boundary.
//!
//! Synthetic loopback HTTP only. These tests assert that a cancelled login
//! never writes credentials, and that a cancelled `auth import` does not wait
//! forever for stdin EOF.

#![cfg(unix)]

mod common;

use common::{json_stdout, stderr, stdout, TestDir};
use std::{
    io::{Read, Write},
    net::{Ipv4Addr, TcpListener},
    process::{Child, Command, Output, Stdio},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

const CHILD_DEADLINE: Duration = Duration::from_secs(5);
const START_DEADLINE: Duration = Duration::from_secs(5);

extern "C" {
    fn kill(pid: i32, sig: i32) -> i32;
}

fn send_sigint(pid: u32) {
    let rc = unsafe { kill(pid as i32, 2) };
    assert_eq!(rc, 0, "SIGINT must be delivered");
}

/// Wait until the child has installed its SIGINT handler. A fixed startup
/// sleep races with dynamic linking and parallel test execution.
#[cfg(target_os = "linux")]
fn wait_for_sigint_handler(pid: u32) {
    let status_path = format!("/proc/{pid}/status");
    let deadline = Instant::now() + START_DEADLINE;
    loop {
        if let Ok(status) = std::fs::read_to_string(&status_path) {
            if let Some(mask) = status
                .lines()
                .find_map(|line| line.strip_prefix("SigCgt:\t"))
                .and_then(|value| u64::from_str_radix(value.trim(), 16).ok())
            {
                // SIGINT is signal 2, represented by bit 1.
                if mask & (1 << 1) != 0 {
                    return;
                }
            }
        }
        assert!(
            Instant::now() < deadline,
            "booxctl did not install its SIGINT handler"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[cfg(all(unix, not(target_os = "linux")))]
fn wait_for_sigint_handler(_pid: u32) {
    // No portable way to inspect another process's signal dispositions.
    // Leave enough time for process startup before delivering SIGINT.
    std::thread::sleep(Duration::from_millis(750));
}

struct HeldAuthServer {
    endpoint: String,
    requests_started: Arc<AtomicUsize>,
    release: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl HeldAuthServer {
    fn start() -> Self {
        Self::start_expecting(1)
    }

    fn start_expecting(expected: usize) -> Self {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind held auth fixture");
        listener
            .set_nonblocking(true)
            .expect("nonblocking held auth fixture");
        let endpoint = format!("http://{}", listener.local_addr().expect("fixture address"));
        let requests_started = Arc::new(AtomicUsize::new(0));
        let release = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));
        let started = Arc::clone(&requests_started);
        let released = Arc::clone(&release);
        let stopping = Arc::clone(&stop);
        let handle = std::thread::spawn(move || {
            let deadline = Instant::now() + START_DEADLINE;
            let mut handlers = Vec::new();
            // Accept every expected verification connection before serving any
            // of them. The migration test can then hold both processes at the
            // verification boundary and exercise the persistence race.
            while handlers.len() < expected {
                if stopping.load(Ordering::SeqCst) || Instant::now() >= deadline {
                    break;
                }
                match listener.accept() {
                    Ok((stream, _)) => {
                        let started = Arc::clone(&started);
                        let released = Arc::clone(&released);
                        let stopping = Arc::clone(&stopping);
                        handlers.push(std::thread::spawn(move || {
                            serve_held_auth(stream, started, released, stopping)
                        }));
                    }
                    Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(2));
                    }
                    Err(_) => break,
                }
            }
            for handler in handlers {
                let _ = handler.join();
            }
        });
        Self {
            endpoint,
            requests_started,
            release,
            stop,
            handle: Some(handle),
        }
    }

    fn endpoint(&self) -> &str {
        &self.endpoint
    }

    fn wait_until_started(&self, expected: usize) {
        let deadline = Instant::now() + START_DEADLINE;
        while Instant::now() < deadline {
            if self.requests_started.load(Ordering::SeqCst) >= expected {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!(
            "expected {expected} auth verification requests, observed {}",
            self.requests_started.load(Ordering::SeqCst)
        );
    }

    fn release(&self) {
        self.release.store(true, Ordering::SeqCst);
    }
}

fn serve_held_auth(
    mut stream: std::net::TcpStream,
    requests_started: Arc<AtomicUsize>,
    release: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
) {
    stream
        .set_read_timeout(Some(Duration::from_millis(50)))
        .expect("fixture read timeout");
    stream
        .set_write_timeout(Some(Duration::from_secs(2)))
        .expect("fixture write timeout");
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut request = Vec::new();
    let mut buffer = [0_u8; 1024];
    while Instant::now() < deadline && !stop.load(Ordering::SeqCst) {
        match stream.read(&mut buffer) {
            Ok(0) => break,
            Ok(count) => {
                request.extend_from_slice(&buffer[..count]);
                if request.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }
            Err(err)
                if matches!(
                    err.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                continue;
            }
            Err(err) if err.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => return,
        }
    }
    if !request.windows(4).any(|window| window == b"\r\n\r\n") {
        return;
    }
    requests_started.fetch_add(1, Ordering::SeqCst);
    let release_deadline = Instant::now() + Duration::from_secs(5);
    while !release.load(Ordering::SeqCst)
        && !stop.load(Ordering::SeqCst)
        && Instant::now() < release_deadline
    {
        std::thread::sleep(Duration::from_millis(5));
    }
    let body = r#"{"result_code":0,"data":{"uid":"synthetic-user"}}"#;
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes());
}

impl Drop for HeldAuthServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        self.release.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
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
                Ok(Some(status)) => {
                    let mut child = self.child.take().expect("running child");
                    let _ = child.wait();
                    let mut out = Vec::new();
                    let mut err = Vec::new();
                    if let Some(mut stream) = child.stdout.take() {
                        stream.read_to_end(&mut out).expect("read stdout");
                    }
                    if let Some(mut stream) = child.stderr.take() {
                        stream.read_to_end(&mut err).expect("read stderr");
                    }
                    return Output {
                        status,
                        stdout: out,
                        stderr: err,
                    };
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

fn import_command(dir: &TestDir, endpoint: &str) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_booxctl"));
    command
        .args(["--json", "auth", "import", "--token-stdin"])
        .env("BOOXCTL_CONFIG_DIR", dir.config_dir())
        .env("BOOXCTL_CREDENTIAL_STORE", "file")
        .env("BOOXCTL_TEST_AUTH_BASE_URL", endpoint)
        .env_remove("BOOXCTL_TOKEN")
        .env("NO_PROXY", Ipv4Addr::LOCALHOST.to_string())
        .env("no_proxy", Ipv4Addr::LOCALHOST.to_string())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}

fn status_authorized(dir: &TestDir) -> bool {
    let output = Command::new(env!("CARGO_BIN_EXE_booxctl"))
        .args(["--json", "auth", "status"])
        .env("BOOXCTL_CONFIG_DIR", dir.config_dir())
        .env("BOOXCTL_CREDENTIAL_STORE", "file")
        .env_remove("BOOXCTL_TOKEN")
        .stdin(Stdio::null())
        .output()
        .expect("run auth status");
    assert_eq!(
        output.status.code(),
        Some(0),
        "status stderr: {}",
        stderr(&output)
    );
    json_stdout(&output)["data"]["authorized"]
        .as_bool()
        .expect("authorized bool")
}

#[test]
fn sigint_after_verification_response_does_not_persist_credential() {
    let dir = TestDir::new();
    let server = HeldAuthServer::start();
    let mut command = import_command(&dir, server.endpoint());
    let mut child = command.spawn().expect("spawn auth import");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(b"synthetic-cancel-token\n")
        .expect("write token");
    let guard = ChildGuard::new(child);

    server.wait_until_started(1);
    wait_for_sigint_handler(guard.pid());
    send_sigint(guard.pid());
    std::thread::sleep(Duration::from_millis(100));
    server.release();
    let output = guard.wait_with_deadline(CHILD_DEADLINE);

    assert_eq!(
        output.status.code(),
        Some(130),
        "stdout: {} stderr: {}",
        stdout(&output),
        stderr(&output)
    );
    let value = json_stdout(&output);
    assert_eq!(
        value["ok"], false,
        "cancelled import reported success: {value}"
    );
    assert_eq!(value["error"]["code"], "cancelled");
    assert!(
        !dir.config_dir().join("auth.json").exists(),
        "cancelled import must not write credentials"
    );
    assert!(
        !status_authorized(&dir),
        "cancelled import left credentials usable"
    );
}

#[test]
fn sigint_while_stdin_is_still_open_exits_without_waiting_for_eof() {
    let dir = TestDir::new();
    let mut command = import_command(&dir, &format!("http://{}:9", Ipv4Addr::LOCALHOST));
    let mut child = command.spawn().expect("spawn auth import");
    let mut stdin = child.stdin.take().expect("stdin");
    stdin
        .write_all(b"synthetic-token-without-eof")
        .expect("write token");
    stdin.flush().expect("flush token");
    let guard = ChildGuard::new(child);

    wait_for_sigint_handler(guard.pid());
    send_sigint(guard.pid());
    // Keep `stdin` open while waiting. A correct implementation polls the
    // cancellation flag and exits; the buggy one stays parked until EOF.
    let output = guard.wait_with_deadline(Duration::from_secs(3));
    drop(stdin);

    assert_eq!(
        output.status.code(),
        Some(130),
        "stdout: {} stderr: {}",
        stdout(&output),
        stderr(&output)
    );
    let value = json_stdout(&output);
    assert_eq!(
        value["ok"], false,
        "cancelled import reported success: {value}"
    );
    assert_eq!(value["error"]["code"], "cancelled");
    assert!(!dir.config_dir().join("auth.json").exists());
    assert!(!status_authorized(&dir));
}

fn migration_command(dir: &TestDir, source: &std::path::Path, endpoint: &str) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_booxctl"));
    command
        .args([
            "--json",
            "config",
            "migrate",
            "--from",
            source.to_str().expect("utf8 source"),
        ])
        .env("BOOXCTL_CONFIG_DIR", dir.config_dir())
        .env("BOOXCTL_CREDENTIAL_STORE", "file")
        .env("BOOXCTL_TEST_AUTH_BASE_URL", endpoint)
        .env_remove("BOOXCTL_TOKEN")
        .env("NO_PROXY", Ipv4Addr::LOCALHOST.to_string())
        .env("no_proxy", Ipv4Addr::LOCALHOST.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}

#[test]
fn concurrent_migrations_cannot_replace_each_others_target() {
    let dir = TestDir::new();
    let server = HeldAuthServer::start_expecting(2);
    let first_source = dir.path().join("legacy-first.json");
    let second_source = dir.path().join("legacy-second.json");
    std::fs::write(
        &first_source,
        r#"{"token":"synthetic-first","updated_ms":1}"#,
    )
    .unwrap();
    std::fs::write(
        &second_source,
        r#"{"token":"synthetic-second","updated_ms":2}"#,
    )
    .unwrap();

    let first = migration_command(&dir, &first_source, server.endpoint())
        .spawn()
        .expect("spawn first migration");
    let second = migration_command(&dir, &second_source, server.endpoint())
        .spawn()
        .expect("spawn second migration");
    let first = ChildGuard::new(first);
    let second = ChildGuard::new(second);

    // Both processes must reach verification before either target write can
    // happen. Waiting for two accepted requests rules out a sequential run
    // where one migration finished before the other started verification.
    server.wait_until_started(2);
    server.release();
    let first = first.wait_with_deadline(CHILD_DEADLINE);
    let second = second.wait_with_deadline(CHILD_DEADLINE);

    let mut statuses = [first.status.code(), second.status.code()];
    statuses.sort();
    assert!(
        statuses == [Some(0), Some(2)] || statuses == [Some(0), Some(5)],
        "exactly one migration may create the target; first: {} second: {}",
        stdout(&first),
        stdout(&second)
    );
    let rejected = if first.status.code() == Some(0) {
        &second
    } else {
        &first
    };
    // Both outcomes mean "the target was not available for this migration".
    // The second process may lose the short final lock before it can inspect
    // the newly created target. Neither may report `migrated`.
    let rejected_value = json_stdout(rejected);
    let code = rejected_value["error"]["code"]
        .as_str()
        .expect("rejection code");
    assert!(
        matches!(code, "credentials_already_exist" | "credential_store_busy"),
        "unexpected rejection: {code}"
    );
    let saved: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.config_dir().join("auth.json")).unwrap())
            .expect("saved credential");
    let saved_token = saved["token"].as_str().expect("saved token");
    assert!(
        saved_token == "synthetic-first" || saved_token == "synthetic-second",
        "unexpected target: {saved_token}"
    );
    assert!(status_authorized(&dir));
}
