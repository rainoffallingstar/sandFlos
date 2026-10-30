//! Streaming upload behaviour at the OSS protocol boundary (S3).
//!
//! These tests exercise the real HTTP adapter against a loopback OSS fixture.
//! They cover the remaining T7 boundaries: a connection dropped while the body
//! is still being sent must not be reported as an accepted upload, and the
//! a single PUT must carry the expected file bytes and length.

use boox_core::{
    models::{BucketConfig, OssSts},
    services::{HttpPushBackend, PushBackend, ServiceErrorKind},
};
use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    net::TcpListener,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    thread::JoinHandle,
    time::Duration,
};

struct OssFixture {
    endpoint: String,
    bytes_seen: Arc<AtomicUsize>,
    uploads: Arc<AtomicUsize>,
    payload: Arc<Mutex<Vec<u8>>>,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl OssFixture {
    /// Some(limit) closes after at most that many bytes; None acknowledges
    /// the complete declared body.
    fn start(drop_after: Option<usize>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind OSS fixture");
        listener.set_nonblocking(true).expect("nonblocking OSS");
        let endpoint = format!("http://{}", listener.local_addr().expect("address"));
        let bytes_seen = Arc::new(AtomicUsize::new(0));
        let uploads = Arc::new(AtomicUsize::new(0));
        let payload = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let seen = Arc::clone(&bytes_seen);
        let counted = Arc::clone(&uploads);
        let received = Arc::clone(&payload);
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
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .expect("OSS read timeout");
                stream
                    .set_write_timeout(Some(Duration::from_secs(5)))
                    .expect("OSS write timeout");
                let mut reader = BufReader::new(stream.try_clone().expect("clone stream"));
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
                    let trimmed = line.trim_end();
                    if let Some(value) = trimmed
                        .to_ascii_lowercase()
                        .strip_prefix("content-length:")
                        .and_then(|value| value.trim().parse::<usize>().ok())
                    {
                        content_length = value;
                    }
                }
                counted.fetch_add(1, Ordering::SeqCst);
                match drop_after {
                    Some(limit) => {
                        let to_read = content_length.min(limit);
                        let mut body = vec![0u8; to_read];
                        let read = reader.read(&mut body).unwrap_or(0);
                        seen.fetch_add(read, Ordering::SeqCst);
                        // Close without any HTTP response: the upload was cut
                        // off mid-body.
                        drop(stream);
                    }
                    None => {
                        let mut body = vec![0u8; content_length];
                        if reader.read_exact(&mut body).is_err() {
                            continue;
                        }
                        seen.fetch_add(body.len(), Ordering::SeqCst);
                        *received.lock().unwrap() = body;
                        let response =
                            "HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                        let _ = stream.write_all(response.as_bytes());
                    }
                }
            }
        });
        Self {
            endpoint,
            bytes_seen,
            uploads,
            payload,
            stop,
            handle: Some(handle),
        }
    }

    fn bytes_seen(&self) -> usize {
        self.bytes_seen.load(Ordering::SeqCst)
    }

    fn uploads(&self) -> usize {
        self.uploads.load(Ordering::SeqCst)
    }
}

impl Drop for OssFixture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn bucket() -> BucketConfig {
    BucketConfig {
        bucket: Some("fixture".into()),
        ali_endpoint: Some("https://oss.invalid".into()),
        ..Default::default()
    }
}

fn sts() -> OssSts {
    OssSts {
        access_key_id: "fixture-id".into(),
        access_key_secret: "fixture-secret".into(),
        security_token: "fixture-token".into(),
        expiration: "2099-01-01T00:00:00Z".into(),
    }
}

fn temp_file(bytes: &[u8]) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("oss-stream-{}.pdf", uuid::Uuid::new_v4()));
    fs::write(&path, bytes).expect("write fixture");
    path
}

#[test]
fn dropped_upload_connection_is_not_reported_as_accepted() {
    let server = OssFixture::start(Some(8));
    let path = temp_file(&[b'x'; 64 * 1024]);
    let mut backend = HttpPushBackend::with_test_endpoints(
        Duration::from_secs(2),
        "credential",
        &server.endpoint,
        &server.endpoint,
    )
    .expect("loopback backend");

    let result = backend.upload(
        &bucket(),
        &sts(),
        "fixture/book.pdf",
        &path,
        "application/pdf",
    );
    fs::remove_file(&path).unwrap();

    assert_eq!(server.uploads(), 1, "the upload must have started");
    assert!(
        server.bytes_seen() > 0,
        "the server must receive part of the body before disconnecting"
    );
    assert!(
        server.bytes_seen() < 64 * 1024,
        "the connection must be cut before the whole body arrives"
    );
    let error = result.expect_err("a dropped connection cannot be an accepted upload");
    assert_eq!(error.kind, ServiceErrorKind::Network, "{error:?}");
}

#[test]
fn one_upload_carries_the_expected_file_bytes_and_length() {
    let server = OssFixture::start(None);
    let payload = vec![b'z'; 64 * 1024];
    let path = temp_file(&payload);
    let mut backend = HttpPushBackend::with_test_endpoints(
        Duration::from_secs(2),
        "credential",
        &server.endpoint,
        &server.endpoint,
    )
    .expect("loopback backend");

    backend
        .upload(
            &bucket(),
            &sts(),
            "fixture/book.pdf",
            &path,
            "application/pdf",
        )
        .expect("a complete acknowledged upload succeeds");
    fs::remove_file(&path).unwrap();

    assert_eq!(server.uploads(), 1, "a single PUT sends the body once");
    assert_eq!(*server.payload.lock().unwrap(), payload);
    assert_eq!(
        server.bytes_seen(),
        payload.len(),
        "the streamed body must match the file size exactly"
    );
}
