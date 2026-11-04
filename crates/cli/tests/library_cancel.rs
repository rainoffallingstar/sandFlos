//! SIGINT while a Zotero WebDAV download is in flight.
//!
//! The fixture accepts the request and withholds the response far beyond the
//! child watchdog. The CLI uses a long `--timeout`, so a prompt exit proves
//! cancellation interrupted the download instead of waiting for that timeout.

#![cfg(unix)]

mod common;

use common::{command_with_api, json_stdout, stderr, stdout, TestDir, TEST_OSS_ENDPOINT_ENV};
use std::{
    fs,
    io::{BufRead, BufReader, Write},
    net::TcpListener,
    path::Path,
    process::{Child, Command, Output, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

const CHILD_DEADLINE: Duration = Duration::from_secs(8);
const PROMPT_EXIT: Duration = Duration::from_secs(3);
const SERVER_STALL: Duration = Duration::from_secs(15);
const SOCKET_DEADLINE: Duration = Duration::from_secs(5);

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
                Ok(Some(status)) => {
                    let mut child = self.child.take().expect("running child");
                    // try_wait already observed the exit; wait is immediate and
                    // keeps the fixture's child-reaping behavior explicit.
                    let _ = child.wait();
                    let mut out = Vec::new();
                    let mut err = Vec::new();
                    if let Some(mut stream) = child.stdout.take() {
                        use std::io::Read;
                        stream.read_to_end(&mut out).expect("read stdout");
                    }
                    if let Some(mut stream) = child.stderr.take() {
                        use std::io::Read;
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
                Ok(None) => thread::sleep(Duration::from_millis(10)),
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Hold {
    Headers,
    Body,
}

struct HeldWebDav {
    endpoint: String,
    received: Arc<AtomicBool>,
    release: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl HeldWebDav {
    fn start(hold: Hold) -> Self {
        let listener =
            TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).expect("bind WebDAV fixture");
        listener
            .set_nonblocking(true)
            .expect("nonblocking WebDAV fixture");
        let endpoint = format!("http://{}/zotero", listener.local_addr().unwrap());
        let received = Arc::new(AtomicBool::new(false));
        let release = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));
        let seen = Arc::clone(&received);
        let releasing = Arc::clone(&release);
        let stopping = Arc::clone(&stop);
        let handle = thread::spawn(move || {
            let deadline = Instant::now() + SOCKET_DEADLINE;
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error)
                        if error.kind() == std::io::ErrorKind::WouldBlock
                            && Instant::now() < deadline =>
                    {
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(_) => return,
                }
            };
            stream
                .set_nonblocking(false)
                .expect("blocking WebDAV connection");
            stream
                .set_read_timeout(Some(SOCKET_DEADLINE))
                .expect("WebDAV read timeout");
            stream
                .set_write_timeout(Some(SOCKET_DEADLINE))
                .expect("WebDAV write timeout");
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 {
                    return;
                }
                if line == "\r\n" {
                    break;
                }
            }
            seen.store(true, Ordering::SeqCst);
            if hold == Hold::Body {
                let _ = stream.write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\nConnection: close\r\n\r\n",
                );
                let _ = stream.flush();
            }
            let deadline = Instant::now() + SERVER_STALL;
            while !releasing.load(Ordering::SeqCst) && !stopping.load(Ordering::SeqCst) {
                if Instant::now() >= deadline {
                    return;
                }
                thread::sleep(Duration::from_millis(5));
            }
            if hold == Hold::Headers && !stopping.load(Ordering::SeqCst) {
                let _ = stream.write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 1\r\nConnection: close\r\n\r\n",
                );
            }
        });
        Self {
            endpoint,
            received,
            release,
            stop,
            handle: Some(handle),
        }
    }

    fn endpoint(&self) -> &str {
        &self.endpoint
    }

    fn wait_until_received(&self) {
        let deadline = Instant::now() + SOCKET_DEADLINE;
        while !self.received.load(Ordering::SeqCst) {
            assert!(
                Instant::now() < deadline,
                "WebDAV fixture received no request"
            );
            thread::sleep(Duration::from_millis(5));
        }
    }

    #[allow(dead_code)]
    fn release(&self) {
        self.release.store(true, Ordering::SeqCst);
    }
}

impl Drop for HeldWebDav {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        self.release.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn configure_zotero_without_local_file(dir: &TestDir) -> std::path::PathBuf {
    let library = dir.path().join("Zotero Data");
    fs::create_dir_all(&library).unwrap();
    let db = rusqlite::Connection::open(library.join("zotero.sqlite")).unwrap();
    db.execute_batch(
        "CREATE TABLE items(itemID INTEGER, key TEXT);
        CREATE TABLE itemAttachments(itemID INTEGER, path TEXT, linkMode INTEGER);
        INSERT INTO items VALUES(4,'ABCDEFGH');
        INSERT INTO itemAttachments VALUES(4,'storage:book.pdf',0);",
    )
    .unwrap();
    drop(db);
    let output = common::command(
        &[
            "--json",
            "zotero",
            "configure",
            "--data-dir",
            library.to_str().unwrap(),
        ],
        &dir.config_dir(),
    )
    .output()
    .expect("configure Zotero");
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    library
}

fn configure_webdav(dir: &TestDir, endpoint: &str) {
    let output = common::run_with_stdin(
        &[
            "--json",
            "zotero",
            "webdav",
            "configure",
            "--url",
            endpoint,
            "--username",
            "user",
            "--password-stdin",
        ],
        &dir.config_dir(),
        "test-password\n",
    );
    assert_eq!(output.status.code(), Some(0), "{output:?}");
}

fn spawn_zotero_push(dir: &TestDir, endpoint: &str, temp: &Path) -> ChildGuard {
    let mut command: Command = command_with_api(
        &[
            "--json",
            "--timeout",
            "300",
            "zotero",
            "push",
            "--attachment-id",
            "4",
        ],
        &dir.config_dir(),
        endpoint,
    );
    command
        .env(TEST_OSS_ENDPOINT_ENV, endpoint)
        .env("BOOXCTL_TOKEN", "fixture-token")
        .env("TMPDIR", temp)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    ChildGuard::new(command.spawn().expect("spawn booxctl"))
}

fn attachment_temp_dirs(temp: &Path) -> Vec<String> {
    fs::read_dir(temp)
        .expect("read temporary directory")
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with("booxctl-attachment-"))
        .collect()
}

fn cancelled_webdav_download_case(hold: Hold) {
    let dir = TestDir::new();
    let temp = dir.path().join("tmp");
    fs::create_dir_all(&temp).unwrap();
    let library = configure_zotero_without_local_file(&dir);
    let original_db = fs::read(library.join("zotero.sqlite")).unwrap();
    let webdav = HeldWebDav::start(hold);
    configure_webdav(&dir, webdav.endpoint());
    let api = common::ApiServer::start();

    let child = spawn_zotero_push(&dir, api.endpoint(), &temp);
    webdav.wait_until_received();
    let interrupted = Instant::now();
    send_sigint(child.pid());
    let output = child.wait_with_deadline(CHILD_DEADLINE);
    let elapsed = interrupted.elapsed();
    let records = api.requests();

    assert!(
        elapsed < PROMPT_EXIT,
        "SIGINT must stop the held WebDAV download promptly, took {elapsed:?}; stdout: {} stderr: {}",
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
    assert_eq!(value["ok"], false);
    assert_eq!(value["error"]["code"], "cancelled");
    assert!(
        records.is_empty(),
        "cancelled download started an upload: {records:?}"
    );
    assert_eq!(
        fs::read(library.join("zotero.sqlite")).unwrap(),
        original_db
    );
    assert_eq!(
        attachment_temp_dirs(&temp),
        Vec::<String>::new(),
        "cancelled download left temporary files"
    );
}

#[test]
fn sigint_while_webdav_response_headers_are_held_exits_without_uploading() {
    cancelled_webdav_download_case(Hold::Headers);
}

#[test]
fn sigint_while_webdav_response_body_is_held_exits_without_uploading() {
    cancelled_webdav_download_case(Hold::Body);
}
