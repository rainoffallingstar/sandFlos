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
        .stdin(Stdio::null());
    command
}

pub fn command_with_auth(args: &[&str], config_dir: &Path, endpoint: &str) -> Command {
    let mut command = command(args, config_dir);
    command.env(TEST_AUTH_ENDPOINT_ENV, endpoint);
    command
}

pub fn run(args: &[&str], config_dir: &Path) -> Output {
    command(args, config_dir).output().expect("run sandflos")
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
