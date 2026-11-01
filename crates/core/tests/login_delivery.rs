//! Public browser-login delivery contract over a real loopback portal.
//!
//! These tests never contact the production login service. They use the
//! portal's public startup API and the callback route that accepts a token
//! supplied by the browser page.

use boox_core::{api::ApiEndpoints, auth::LoginPortal};
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

fn http_request(port: u16, target: &str) -> std::io::Result<(u16, String)> {
    let deadline = Instant::now() + Duration::from_secs(3);
    let address = SocketAddr::from(([127, 0, 0, 1], port));
    let mut stream = TcpStream::connect_timeout(&address, Duration::from_secs(2))?;
    stream.set_write_timeout(Some(Duration::from_millis(500)))?;
    write!(
        stream,
        "GET {target} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
    )?;
    let mut raw = Vec::new();
    let mut buffer = [0_u8; 4096];
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "loopback response exceeded absolute deadline",
            ));
        }
        stream.set_read_timeout(Some(remaining.min(Duration::from_millis(500))))?;
        match stream.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => raw.extend_from_slice(&buffer[..read]),
            Err(err)
                if matches!(
                    err.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                continue
            }
            Err(err) => return Err(err),
        }
    }
    let response = String::from_utf8(raw)
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?;
    let status = response
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(0);
    let body = response
        .split_once("\r\n\r\n")
        .map(|(_, body)| body.to_string())
        .unwrap_or_default();
    Ok((status, body))
}

fn state_from_bridge_url(url: &str) -> String {
    url::Url::parse(url)
        .expect("parse bridge URL")
        .query_pairs()
        .find(|(name, _)| name == "state")
        .map(|(_, value)| value.into_owned())
        .expect("bridge URL contains state")
}

#[test]
fn delivery_is_explicit_retryable_once_only_and_portal_shutdown_is_bounded() {
    let bridge_url = Arc::new(Mutex::new(None::<String>));
    let captured_url = bridge_url.clone();
    let attempts = Arc::new(AtomicUsize::new(0));
    let successes = Arc::new(AtomicUsize::new(0));
    let received = Arc::new(Mutex::new(Vec::<String>::new()));
    let (attempt_counter, success_counter, received_tokens) =
        (attempts.clone(), successes.clone(), received.clone());

    let portal = LoginPortal::start(
        move |url| {
            *captured_url.lock().expect("capture bridge URL") = Some(url.to_string());
            Ok(())
        },
        move |token: String| {
            let attempt = attempt_counter.fetch_add(1, Ordering::SeqCst);
            received_tokens
                .lock()
                .expect("record token")
                .push(token.clone());
            if attempt == 0 {
                Err("synthetic persistence failure with callback-secret".to_string())
            } else {
                success_counter.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        },
    )
    .expect("start portal without opening a real browser");

    let url = bridge_url
        .lock()
        .expect("read bridge URL")
        .clone()
        .expect("bridge URL delivered to opener");
    let state = state_from_bridge_url(&url);
    let port = portal.port();

    let (status, _body) = http_request(
        port,
        "/auth/wechat/callback?state=wrong-state&token=synthetic-login-token",
    )
    .expect("invalid state request");
    assert_eq!(status, 400);
    assert_eq!(attempts.load(Ordering::SeqCst), 0);
    assert!(received.lock().expect("read tokens").is_empty());

    let (status, body) = http_request(port, &format!("/auth/wechat/callback?state={state}&token="))
        .expect("missing token request");
    assert_eq!(status, 200);
    assert!(body.contains("\"ok\":false"), "body: {body}");
    assert_eq!(attempts.load(Ordering::SeqCst), 0);

    let callback = format!("/auth/wechat/callback?state={state}&token=synthetic-login-token");
    let (status, body) = http_request(port, &callback).expect("failed delivery request");
    assert_eq!(status, 200);
    assert!(body.contains("\"ok\":false"), "body: {body}");
    assert!(!body.contains("synthetic-login-token"), "body: {body}");
    assert!(!body.contains("callback-secret"), "body: {body}");
    assert!(!body.contains("persistence failure"), "body: {body}");
    assert_eq!(attempts.load(Ordering::SeqCst), 1);
    assert_eq!(successes.load(Ordering::SeqCst), 0);
    assert!(boox_core::state::get_auth_state().token.is_none());

    let (status, body) = http_request(port, &callback).expect("retry delivery request");
    assert_eq!(status, 200);
    assert!(body.contains("\"ok\":true"), "body: {body}");
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
    assert_eq!(successes.load(Ordering::SeqCst), 1);

    let (status, body) = http_request(port, &callback).expect("duplicate success request");
    assert_eq!(status, 200);
    assert!(body.contains("\"ok\":true"), "body: {body}");
    assert_eq!(
        attempts.load(Ordering::SeqCst),
        2,
        "a successful callback must not be delivered again"
    );
    assert_eq!(successes.load(Ordering::SeqCst), 1);
    assert_eq!(
        received.lock().expect("read tokens").as_slice(),
        [
            "synthetic-login-token".to_string(),
            "synthetic-login-token".to_string()
        ]
    );
    assert!(boox_core::state::get_auth_state().token.is_none());

    let shutdown_started = Instant::now();
    portal.shutdown();
    assert!(
        shutdown_started.elapsed() < Duration::from_secs(1),
        "portal shutdown must be bounded"
    );
    let address = SocketAddr::from(([127, 0, 0, 1], port));
    assert!(
        TcpStream::connect_timeout(&address, Duration::from_millis(200)).is_err(),
        "stopped portal must stop accepting connections"
    );
}

#[test]
fn opener_failure_returns_an_error_and_stops_the_portal() {
    let captured = Arc::new(Mutex::new(None::<String>));
    let opener_url = captured.clone();
    let error = match LoginPortal::start(
        move |url| {
            *opener_url.lock().expect("capture bridge URL") = Some(url.to_string());
            Err("synthetic browser failure".to_string())
        },
        |_| Ok(()),
    ) {
        Ok(_) => panic!("opener failure must fail startup"),
        Err(error) => error,
    };
    assert_eq!(error, "synthetic browser failure");

    let url = captured
        .lock()
        .expect("read bridge URL")
        .clone()
        .expect("bridge URL delivered before opener failure");
    let port = url::Url::parse(&url)
        .expect("parse bridge URL")
        .port()
        .expect("bridge URL has port");
    let address = SocketAddr::from(([127, 0, 0, 1], port));
    assert!(
        TcpStream::connect_timeout(&address, Duration::from_millis(200)).is_err(),
        "failed startup must not leave a listener running"
    );
}

#[test]
fn dropping_the_owned_portal_stops_its_listener() {
    let portal = LoginPortal::start(|_| Ok(()), |_| Ok(())).expect("start portal");
    let port = portal.port();
    drop(portal);

    let address = SocketAddr::from(([127, 0, 0, 1], port));
    assert!(
        TcpStream::connect_timeout(&address, Duration::from_millis(200)).is_err(),
        "dropping the portal must stop its listener"
    );
}

fn start_upstream(
    responses: Vec<(&'static str, &'static str)>,
) -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind upstream fixture");
    let endpoint = format!(
        "http://{}",
        listener.local_addr().expect("upstream address")
    );
    let worker = thread::spawn(move || {
        for (path, body) in responses {
            let (mut stream, _) = listener.accept().expect("accept upstream request");
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .expect("upstream read timeout");
            let mut reader = BufReader::new(stream.try_clone().expect("clone upstream stream"));
            let mut request = String::new();
            reader.read_line(&mut request).expect("read request line");
            assert!(request.contains(path), "unexpected request: {request}");
            let mut content_length = 0;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).expect("read request header");
                if line == "\r\n" || line.is_empty() {
                    break;
                }
                if let Some(value) = line
                    .to_ascii_lowercase()
                    .strip_prefix("content-length:")
                    .map(str::trim)
                {
                    content_length = value.parse::<usize>().expect("content length");
                }
            }
            let mut body_bytes = vec![0; content_length];
            reader
                .read_exact(&mut body_bytes)
                .expect("read request body");
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .expect("write upstream response");
        }
    });
    (endpoint, worker)
}

#[test]
fn qr_and_email_completion_use_the_same_callback_path() {
    for (path, response, request) in [
        (
            "/api/1/auth/qrcode/check",
            r#"{"result_code":0,"data":{"status":1,"userInfo":{"token":"synthetic-qr-token"}}}"#,
            "/auth/qrcode/check?state={state}&qrcodeId=fixture-qr",
        ),
        (
            "/api/1/users/signupByPhoneOrEmail",
            r#"{"result_code":0,"data":{"token":"synthetic-email-token"}}"#,
            "/auth/login/submit?state={state}&mode=email&mobi=demo%40example.com&code=123456",
        ),
    ] {
        let (endpoint, upstream) = start_upstream(vec![("/api/1/", response)]);
        let endpoints = ApiEndpoints::loopback(&endpoint, &endpoint).expect("loopback endpoints");
        let received = Arc::new(Mutex::new(Vec::new()));
        let received_by_callback = received.clone();
        let portal = LoginPortal::start_with_endpoints(
            |_| Ok(()),
            move |token: String| {
                received_by_callback
                    .lock()
                    .expect("record delivered token")
                    .push(token);
                Ok(())
            },
            endpoints,
        )
        .expect("start portal with loopback endpoints");
        let state = state_from_bridge_url(&portal.bridge_url());
        let target = request.replace("{state}", &state);
        let (status, body) = http_request(portal.port(), &target).expect("portal request");
        assert_eq!(status, 200);
        assert!(body.contains("\"ok\":true"), "body: {body}");
        upstream.join().expect("upstream fixture");

        let expected = if path.ends_with("check") {
            "synthetic-qr-token"
        } else {
            "synthetic-email-token"
        };
        assert_eq!(
            received.lock().expect("read delivered token").as_slice(),
            [expected.to_string()]
        );
        assert!(boox_core::state::get_auth_state().token.is_none());
    }
}

#[test]
fn dropping_with_partial_request_joins_worker_and_suppresses_late_delivery() {
    let deliveries = Arc::new(AtomicUsize::new(0));
    let delivered = deliveries.clone();
    let portal = LoginPortal::start(
        |_| Ok(()),
        move |_| {
            delivered.fetch_add(1, Ordering::SeqCst);
            Ok(())
        },
    )
    .expect("start portal");
    let state = state_from_bridge_url(&portal.bridge_url());
    let port = portal.port();
    let address = SocketAddr::from(([127, 0, 0, 1], port));
    let mut client =
        TcpStream::connect_timeout(&address, Duration::from_secs(2)).expect("connect to portal");
    client
        .set_write_timeout(Some(Duration::from_secs(2)))
        .expect("client write timeout");
    write!(
        client,
        "GET /auth/wechat/callback?state={state}&token=synthetic-late-token HTTP/1.1\r\nHost: [IP]\r\n"
    )
    .expect("write partial request");
    client.flush().expect("flush partial request");
    thread::sleep(Duration::from_millis(50));

    let shutdown_started = Instant::now();
    drop(portal);
    let shutdown_elapsed = shutdown_started.elapsed();

    // If a detached worker is still reading this connection, finishing the
    // request would deliver a token after drop returned.
    let _ = client.write_all(b"\r\n");
    let _ = client.flush();
    thread::sleep(Duration::from_millis(300));

    let listener_closed = TcpStream::connect_timeout(&address, Duration::from_millis(200)).is_err();
    assert_eq!(
        deliveries.load(Ordering::SeqCst),
        0,
        "no delivery callback may begin after shutdown starts"
    );
    assert!(
        listener_closed,
        "drop must join the worker and close the listener"
    );
    assert!(
        shutdown_elapsed < Duration::from_secs(1),
        "drop with a partial request must finish promptly, took {shutdown_elapsed:?}"
    );
}

#[test]
fn legacy_login_replaces_its_previous_owned_portal() {
    let urls = Arc::new(Mutex::new(Vec::<String>::new()));
    let first_urls = urls.clone();
    boox_core::auth::start_login_flow(move |url| {
        first_urls
            .lock()
            .expect("record first bridge URL")
            .push(url.to_string());
        Ok(())
    })
    .expect("start first legacy portal");
    let second_urls = urls.clone();
    boox_core::auth::start_login_flow(move |url| {
        second_urls
            .lock()
            .expect("record second bridge URL")
            .push(url.to_string());
        Ok(())
    })
    .expect("start replacement legacy portal");

    let captured = urls.lock().expect("read captured URLs").clone();
    assert_eq!(captured.len(), 2, "both startup calls opened a URL");
    let first = url::Url::parse(&captured[0]).expect("parse first bridge URL");
    let second = url::Url::parse(&captured[1]).expect("parse second bridge URL");
    let first_port = first.port().expect("first bridge URL port");
    let second_port = second.port().expect("second bridge URL port");
    assert_ne!(first_port, second_port);

    let first_address = SocketAddr::from(([127, 0, 0, 1], first_port));
    assert!(
        TcpStream::connect_timeout(&first_address, Duration::from_millis(200)).is_err(),
        "starting login again must stop the previously held portal"
    );
    let second_address = SocketAddr::from(([127, 0, 0, 1], second_port));
    TcpStream::connect_timeout(&second_address, Duration::from_secs(2))
        .expect("the replacement portal remains available");
}
