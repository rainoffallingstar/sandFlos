//! Streaming upload behaviour at the OSS protocol boundary (S3).
//!
//! These tests exercise the real HTTP adapter against a loopback OSS fixture.
//! They cover the remaining T7 boundaries: a connection dropped while the body
//! is still being sent must not be reported as an accepted upload, and the
//! a single PUT must carry the expected file bytes and length.

use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine};
use boox_core::{
    models::{BucketConfig, OssSts},
    services::{HttpPushBackend, PushBackend, ServiceErrorKind},
};
use hmac::{Hmac, Mac};
use sha1::Sha1;
use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    net::TcpListener,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc, Arc, Mutex,
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

struct OssFixture {
    endpoint: String,
    bytes_seen: Arc<AtomicUsize>,
    uploads: Arc<AtomicUsize>,
    payload: Arc<Mutex<Vec<u8>>>,
    headers: Arc<Mutex<Vec<(String, String)>>>,
    release_response: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl OssFixture {
    /// Some(limit) closes after at most that many bytes; None acknowledges
    /// the complete declared body.
    fn start(drop_after: Option<usize>) -> Self {
        Self::start_with_options(drop_after, false, None)
    }

    /// Hold the response after the complete body arrives so callers can observe
    /// local read progress separately from the eventual OSS acknowledgment.
    fn start_with_hold(drop_after: Option<usize>, hold_response: bool) -> Self {
        Self::start_with_options(drop_after, hold_response, None)
    }

    fn start_with_options(
        drop_after: Option<usize>,
        hold_response: bool,
        slow_read: Option<(usize, Duration)>,
    ) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind OSS fixture");
        listener.set_nonblocking(true).expect("nonblocking OSS");
        let endpoint = format!("http://{}", listener.local_addr().expect("address"));
        let bytes_seen = Arc::new(AtomicUsize::new(0));
        let uploads = Arc::new(AtomicUsize::new(0));
        let payload = Arc::new(Mutex::new(Vec::new()));
        let headers = Arc::new(Mutex::new(Vec::new()));
        let release_response = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));
        let seen = Arc::clone(&bytes_seen);
        let counted = Arc::clone(&uploads);
        let received = Arc::clone(&payload);
        let recorded_headers = Arc::clone(&headers);
        let release = Arc::clone(&release_response);
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
                    if let Some((name, value)) = trimmed.split_once(':') {
                        recorded_headers
                            .lock()
                            .unwrap()
                            .push((name.to_ascii_lowercase(), value.trim().to_string()));
                        if name.eq_ignore_ascii_case("content-length") {
                            content_length = value.trim().parse().unwrap_or(0);
                        }
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
                        let (chunk_size, delay) =
                            slow_read.unwrap_or((content_length.max(1), Duration::ZERO));
                        let mut body = Vec::with_capacity(content_length);
                        let mut buffer = vec![0u8; chunk_size.max(1).min(content_length.max(1))];
                        let mut complete = true;
                        while body.len() < content_length {
                            let wanted = (content_length - body.len()).min(buffer.len());
                            match reader.read(&mut buffer[..wanted]) {
                                Ok(0) => {
                                    complete = false;
                                    break;
                                }
                                Ok(read) => {
                                    body.extend_from_slice(&buffer[..read]);
                                    seen.fetch_add(read, Ordering::SeqCst);
                                    if !delay.is_zero() {
                                        std::thread::sleep(delay);
                                    }
                                }
                                Err(_) => {
                                    complete = false;
                                    break;
                                }
                            }
                        }
                        if !complete {
                            continue;
                        }
                        *received.lock().unwrap() = body;
                        if hold_response {
                            let deadline = Instant::now() + Duration::from_secs(5);
                            while !release.load(Ordering::SeqCst)
                                && !stopping.load(Ordering::SeqCst)
                                && Instant::now() < deadline
                            {
                                std::thread::sleep(Duration::from_millis(5));
                            }
                        }
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
            headers,
            release_response,
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

    fn release(&self) {
        self.release_response.store(true, Ordering::SeqCst);
    }

    fn header(&self, name: &str) -> Option<String> {
        self.headers
            .lock()
            .unwrap()
            .iter()
            .find(|(header, _)| header.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.clone())
    }
}

impl Drop for OssFixture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        self.release_response.store(true, Ordering::SeqCst);
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

#[test]
fn upload_reports_monotonic_local_reads_before_the_oss_response() {
    let server = OssFixture::start_with_hold(None, true);
    let payload = vec![b'p'; 256 * 1024];
    let path = temp_file(&payload);
    let mut backend = HttpPushBackend::with_test_endpoints(
        Duration::from_secs(2),
        "credential",
        &server.endpoint,
        &server.endpoint,
    )
    .expect("loopback backend");

    let (sender, receiver) = mpsc::channel();
    let uploader = std::thread::spawn(move || {
        let result = backend.upload_with_progress(
            &bucket(),
            &sts(),
            "fixture/book.pdf",
            &path,
            "application/pdf",
            &mut |bytes| sender.send(bytes).expect("progress receiver"),
        );
        fs::remove_file(&path).unwrap();
        result
    });
    let first = receiver
        .recv_timeout(Duration::from_secs(3))
        .expect("progress is observed while the OSS response is withheld");
    assert!(
        first > 0 && first <= payload.len() as u64,
        "the first sample must be bounded by the file size: {first}"
    );
    server.release();
    uploader
        .join()
        .expect("upload thread")
        .expect("the delayed acknowledgment still completes the upload");

    let observations = std::iter::once(first)
        .chain(receiver.try_iter())
        .collect::<Vec<_>>();
    assert!(!observations.is_empty(), "the body read must be observable");
    assert!(
        observations.windows(2).all(|pair| pair[0] < pair[1]),
        "progress must be monotonic: {observations:?}"
    );
    assert_eq!(
        observations.last().copied(),
        Some(payload.len() as u64),
        "read progress must stop at the exact file size"
    );
    assert_eq!(server.uploads(), 1, "one acknowledged PUT");
    assert_eq!(server.bytes_seen(), payload.len());
}

static TRANSFER_CANCELLED: AtomicBool = AtomicBool::new(false);

#[test]
fn upload_authorization_signs_the_canonical_resource_independently() {
    let server = OssFixture::start(None);
    let payload = vec![b'a'; 32 * 1024];
    let path = temp_file(&payload);
    let sts = OssSts {
        access_key_id: "fixture-ak".into(),
        access_key_secret: "fixture-secret".into(),
        security_token: "fixture-sts-token".into(),
        expiration: "2099-01-01T00:00:00Z".into(),
    };
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
            &sts,
            "fixture/book.pdf",
            &path,
            "application/pdf",
        )
        .expect("a complete acknowledged upload succeeds");
    fs::remove_file(&path).unwrap();

    let date = server.header("date").expect("PUT carries a Date header");
    let content_type = server
        .header("content-type")
        .expect("PUT carries a Content-Type header");
    let token = server
        .header("x-oss-security-token")
        .expect("PUT carries the STS token");
    let authorization = server
        .header("authorization")
        .expect("PUT carries an Authorization header");
    assert_eq!(content_type, "application/pdf");
    assert_eq!(token, "fixture-sts-token");

    // Independent OSS v1 signing input. This deliberately does not call the
    // production signing helper or reuse its string construction.
    let string_to_sign = format!(
        "PUT\n\n{content_type}\n{date}\nx-oss-security-token:{token}\n/fixture/fixture/book.pdf"
    );
    let mut mac = Hmac::<Sha1>::new_from_slice(b"fixture-secret").expect("HMAC key");
    mac.update(string_to_sign.as_bytes());
    let expected = BASE64_STANDARD.encode(mac.finalize().into_bytes());
    assert_eq!(authorization, format!("OSS fixture-ak:{expected}"));
}

#[test]
fn slow_reader_produces_multiple_distinct_read_samples_before_completion() {
    let server =
        OssFixture::start_with_options(None, false, Some((64 * 1024, Duration::from_millis(10))));
    let payload = vec![b's'; 16 * 1024 * 1024];
    let path = temp_file(&payload);
    let mut backend = HttpPushBackend::with_test_endpoints(
        Duration::from_secs(10),
        "credential",
        &server.endpoint,
        &server.endpoint,
    )
    .expect("loopback backend");

    let (sender, receiver) = mpsc::channel();
    backend
        .upload_with_progress(
            &bucket(),
            &sts(),
            "fixture/book.pdf",
            &path,
            "application/pdf",
            &mut |bytes| sender.send(bytes).expect("progress receiver"),
        )
        .expect("the slow upload completes");
    fs::remove_file(&path).unwrap();

    let observations = receiver.try_iter().collect::<Vec<_>>();
    assert!(
        observations.len() >= 2,
        "a slow 16 MiB body must expose continuous progress, got {observations:?}"
    );
    assert!(
        observations.windows(2).all(|pair| pair[0] < pair[1]),
        "progress must be monotonic: {observations:?}"
    );
    assert!(
        observations
            .iter()
            .all(|bytes| *bytes <= payload.len() as u64),
        "no sample may exceed the file size: {observations:?}"
    );
    assert_eq!(
        observations.last().copied(),
        Some(payload.len() as u64),
        "the final sample must be the exact file size"
    );
    assert_eq!(server.uploads(), 1, "one PUT");
    assert_eq!(server.bytes_seen(), payload.len());
}

#[test]
fn cancellation_suppresses_the_final_read_sample() {
    TRANSFER_CANCELLED.store(false, Ordering::SeqCst);
    let server =
        OssFixture::start_with_options(None, true, Some((64 * 1024, Duration::from_millis(5))));
    let payload = vec![b'c'; 8 * 1024 * 1024];
    let path = temp_file(&payload);
    let mut backend = HttpPushBackend::with_test_endpoints(
        Duration::from_secs(10),
        "credential",
        &server.endpoint,
        &server.endpoint,
    )
    .expect("loopback backend")
    .with_cancel_flag(&TRANSFER_CANCELLED);

    // The callback holds each invocation long enough for the test to set the
    // cancellation flag after the first sample. Any later invocation is
    // therefore unambiguously delivered after cancellation began.
    let (sender, receiver) = mpsc::channel();
    let samples = Arc::new(Mutex::new(Vec::new()));
    let observed = Arc::clone(&samples);
    let uploader = std::thread::spawn(move || {
        let result = backend.upload_with_progress(
            &bucket(),
            &sts(),
            "fixture/book.pdf",
            &path,
            "application/pdf",
            &mut |bytes| {
                observed
                    .lock()
                    .unwrap()
                    .push((bytes, TRANSFER_CANCELLED.load(Ordering::SeqCst)));
                let _ = sender.send(bytes);
                std::thread::sleep(Duration::from_millis(50));
            },
        );
        fs::remove_file(&path).unwrap();
        result
    });

    receiver
        .recv_timeout(Duration::from_secs(5))
        .expect("progress starts while the response is withheld");
    TRANSFER_CANCELLED.store(true, Ordering::SeqCst);
    let result = uploader.join().expect("upload thread");
    let error = result.expect_err("cancellation must not report an accepted upload");
    assert_eq!(error.kind, ServiceErrorKind::Cancelled, "{error:?}");

    let samples = samples.lock().unwrap();
    assert!(
        !samples.is_empty(),
        "some progress was observed before cancellation"
    );
    assert!(
        samples.iter().all(|(_, cancelled)| !cancelled),
        "no progress callback may run after cancellation is observed: {samples:?}"
    );
    assert!(
        samples
            .iter()
            .all(|(bytes, _)| *bytes < payload.len() as u64),
        "cancellation must not emit a final completion sample: {samples:?}"
    );
    assert_eq!(server.uploads(), 1, "cancellation must not retry the PUT");
}
