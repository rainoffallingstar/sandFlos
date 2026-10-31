//! Public desktop dashboard refresh contract over literal loopback HTTP.
//!
//! The refresh receives an explicit credential, upload projection, local date,
//! and optional share-device projection. It does not read or write the core
//! process-global auth, upload, or dashboard cache state.

use boox_core::api::ApiEndpoints;
use boox_core::dashboard::{
    fetch_dashboard_snapshot_with_endpoints, DashboardErrorKind, DashboardRefreshInput,
};
use boox_core::models::{DashboardUploadState, ShareTransferDevice};
use std::{
    io::{BufReader, Read, Write},
    net::{TcpListener, TcpStream},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

struct Loopback {
    endpoint: String,
    requests: Arc<Mutex<Vec<Recorded>>>,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

#[derive(Clone, Debug)]
struct Recorded {
    request_line: String,
    headers: Vec<String>,
}

impl Recorded {
    fn path(&self) -> &str {
        self.request_line.split_whitespace().nth(1).unwrap_or("/")
    }

    fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find_map(|line| {
            line.split_once(':')
                .and_then(|(key, value)| key.eq_ignore_ascii_case(name).then_some(value.trim()))
        })
    }
}

struct Response {
    status: &'static str,
    extra_headers: Vec<String>,
    body: String,
}

impl Response {
    fn json(status: &'static str, body: impl Into<String>) -> Self {
        Self {
            status,
            extra_headers: Vec::new(),
            body: body.into(),
        }
    }
}

const DEADLINE: Duration = Duration::from_secs(5);

impl Loopback {
    fn with_routes(routes: impl Fn(&str, &str) -> Response + Send + 'static) -> Self {
        let listener =
            TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).expect("bind dashboard fixture");
        listener
            .set_nonblocking(true)
            .expect("nonblocking dashboard fixture");
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
                stream
                    .set_nonblocking(false)
                    .expect("blocking fixture connection");
                stream
                    .set_write_timeout(Some(Duration::from_secs(2)))
                    .expect("fixture write timeout");
                let deadline = Instant::now() + DEADLINE;
                let mut reader = match stream.try_clone() {
                    Ok(stream) => BufReader::new(stream),
                    Err(_) => continue,
                };
                let Some(request_line) = read_line_before(&mut reader, deadline) else {
                    continue;
                };
                let mut headers = Vec::new();
                let mut content_length = 0_usize;
                while let Some(line) = read_line_before(&mut reader, deadline) {
                    if line == "\r\n" || line.is_empty() {
                        break;
                    }
                    let trimmed = line.trim_end().to_string();
                    if let Some(value) = trimmed
                        .split_once(':')
                        .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                        .map(|(_, value)| value)
                    {
                        content_length = value.trim().parse().unwrap_or(0);
                    }
                    headers.push(trimmed);
                }
                let mut body = vec![0_u8; content_length];
                if content_length > 0 {
                    let mut read = 0;
                    while read < content_length {
                        let Some(remaining) = deadline.checked_duration_since(Instant::now())
                        else {
                            break;
                        };
                        let _ =
                            stream.set_read_timeout(Some(remaining.max(Duration::from_millis(1))));
                        match reader.read(&mut body[read..]) {
                            Ok(0) | Err(_) => break,
                            Ok(count) => read += count,
                        }
                    }
                    if read != content_length {
                        continue;
                    }
                }
                observed.lock().expect("observed requests").push(Recorded {
                    request_line: request_line.clone(),
                    headers,
                });
                let path = request_line.split_whitespace().nth(1).unwrap_or("/");
                let method = request_line.split_whitespace().next().unwrap_or("GET");
                let Response {
                    status,
                    extra_headers,
                    body,
                } = routes(method, path);
                let mut response = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n",
                    body.len()
                );
                for header in extra_headers {
                    response.push_str(&header);
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

    fn requests(&self) -> Vec<Recorded> {
        self.requests.lock().expect("observed requests").clone()
    }
}

fn remaining(deadline: Instant) -> Option<Duration> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    (!remaining.is_zero()).then_some(remaining)
}

fn read_line_before(reader: &mut BufReader<TcpStream>, deadline: Instant) -> Option<String> {
    let mut line = Vec::new();
    loop {
        reader
            .get_ref()
            .set_read_timeout(Some(remaining(deadline)?))
            .ok()?;
        let mut byte = [0_u8];
        if reader.read(&mut byte).ok()? == 0 {
            return None;
        }
        line.push(byte[0]);
        if byte[0] == b'\n' {
            return String::from_utf8(line).ok();
        }
        if line.len() > 16 * 1024 {
            return None;
        }
    }
}

impl Drop for Loopback {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn upload() -> DashboardUploadState {
    DashboardUploadState {
        in_progress: false,
        status_text: "上传进度: 空闲".into(),
        last_error: None,
        current_file: None,
        bytes_sent: None,
        bytes_total: None,
        progress_percent: None,
        speed_bps: None,
        eta_seconds: None,
        updated_ms: 7,
    }
}

fn input(token: &str) -> DashboardRefreshInput {
    DashboardRefreshInput::new(token, upload(), "2026-10-31")
        .with_share_devices(Vec::<ShareTransferDevice>::new())
}

fn auth_routes(method: &str, path: &str) -> Option<Response> {
    if method != "GET" {
        return None;
    }
    match path {
        "/api/1/users/me" => Some(Response::json(
            "200 OK",
            r#"{"result_code":0,"data":{"uid":"uid-fixture","nickname":"Reader","avatar":"https://avatar.invalid/a.png","storage_limit":1000,"storage_used":100}}"#,
        )),
        "/api/1/statistics/v2/user/storage" => Some(Response::json(
            "200 OK",
            r#"{"result_code":0,"data":{"usedSize":250,"totalSize":1000}}"#,
        )),
        "/api/1/statistics/readingInfo" => {
            Some(Response::json("200 OK", r#"{"result_code":0,"data":{"read":42}}"#))
        }
        "/api/1/statistics/readTimeInfo?date=2026-10-31&range=week" => Some(Response::json(
            "200 OK",
            r#"{"result_code":0,"data":{"now":{"totalTime":5400000}}}"#,
        )),
        "/api/1/statistics/dayRead?startTime=2026-10-31&endTime=2026-10-31" => Some(Response::json(
            "200 OK",
            r#"{"result_code":0,"data":[{"id":1},{"id":2}]}"#,
        )),
        "/api/1/users/getDevice" => Some(Response::json(
            "200 OK",
            r#"{"result_code":0,"data":[]}"#,
        )),
        "/api/1/users/syncToken" => Some(Response {
            status: "200 OK",
            extra_headers: vec![
                "Set-Cookie: RouteSession=route-cookie; Path=/neocloud".into(),
            ],
            body: r#"{"result_code":0,"data":{"cookie_name":"SyncGatewaySession","session_id":"fixture-session"}}"#.into(),
        }),
        _ => None,
    }
}

#[test]
fn queue_failure_is_not_reported_as_a_successful_empty_queue() {
    let server = Loopback::with_routes(|method, path| {
        if let Some(response) = auth_routes(method, path) {
            return response;
        }
        if path.starts_with("/neocloud/_changes") {
            return Response::json("503 Service Unavailable", "upstream-secret");
        }
        Response::json("404 Not Found", r#"{"error":"unexpected route"}"#)
    });
    let endpoints =
        ApiEndpoints::loopback(&server.endpoint, &server.endpoint).expect("loopback endpoints");
    let client = boox_core::api::create_client(5).expect("client");

    let error =
        fetch_dashboard_snapshot_with_endpoints(&endpoints, &client, &input("explicit-token"))
            .expect_err("a queue request failure must not look like an empty queue");

    assert_eq!(error.kind, DashboardErrorKind::Network);
    assert_eq!(error.stage, "queue");
    assert_eq!(error.status, Some(503));
    assert!(
        !error.message.contains("upstream-secret"),
        "server body must not be surfaced: {}",
        error.message
    );
    assert!(
        server
            .requests()
            .iter()
            .any(|request| request.path().starts_with("/neocloud/_changes")),
        "the queue request must have been attempted"
    );
}

#[test]
fn invalid_queue_payload_is_a_protocol_error() {
    let server = Loopback::with_routes(|method, path| {
        if let Some(response) = auth_routes(method, path) {
            return response;
        }
        if path.starts_with("/neocloud/_changes") {
            return Response::json("200 OK", r#"{"results":null,"secret":"not-a-queue"}"#);
        }
        Response::json("404 Not Found", r#"{"error":"unexpected route"}"#)
    });
    let endpoints = ApiEndpoints::loopback(&server.endpoint, &server.endpoint).unwrap();
    let client = boox_core::api::create_client(5).unwrap();

    let error =
        fetch_dashboard_snapshot_with_endpoints(&endpoints, &client, &input("explicit-token"))
            .expect_err("invalid queue JSON must not mean an empty queue");
    assert_eq!(error.kind, DashboardErrorKind::Protocol);
    assert_eq!(error.stage, "queue");
    assert!(!error.message.contains("not-a-queue"));
}

#[test]
fn identity_rejection_is_authentication_and_does_not_leak_the_body() {
    let server = Loopback::with_routes(|method, path| {
        if (method, path) == ("GET", "/api/1/users/me") {
            return Response::json(
                "401 Unauthorized",
                r#"{"error":"credential fixture-secret-value expired"}"#,
            );
        }
        Response::json("404 Not Found", r#"{"error":"unexpected route"}"#)
    });
    let endpoints = ApiEndpoints::loopback(&server.endpoint, &server.endpoint).unwrap();
    let client = boox_core::api::create_client(5).unwrap();

    let error =
        fetch_dashboard_snapshot_with_endpoints(&endpoints, &client, &input("rejected-token"))
            .expect_err("a rejected credential must fail the refresh");
    assert_eq!(error.kind, DashboardErrorKind::Authentication);
    assert_eq!(error.stage, "auth");
    assert_eq!(error.status, Some(401));
    assert!(!error.message.contains("fixture-secret-value"));
    assert_eq!(
        server.requests().len(),
        1,
        "no statistics may follow rejected identity"
    );
}

#[test]
fn network_failure_is_not_misreported_as_unauthenticated() {
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    drop(listener);
    let endpoints = ApiEndpoints::loopback(&endpoint, &endpoint).unwrap();
    let client = boox_core::api::create_client(2).unwrap();

    let error =
        fetch_dashboard_snapshot_with_endpoints(&endpoints, &client, &input("explicit-token"))
            .expect_err("a refused connection must fail");
    assert_eq!(error.kind, DashboardErrorKind::Network);
    assert_eq!(error.stage, "auth");
}

#[test]
fn successful_refresh_projects_all_dashboard_fields_with_the_supplied_date() {
    let server = Loopback::with_routes(|method, path| {
        if (method, path) == ("GET", "/api/1/users/getDevice") {
            return Response::json(
                "200 OK",
                r#"{"result_code":0,"data":[{"id":"device-1","model":"Boox Note","macAddress":"AA:BB:CC:DD:EE:FF","loginStatus":"online"}]}"#,
            );
        }
        if let Some(response) = auth_routes(method, path) {
            return response;
        }
        if path.starts_with("/neocloud/_changes") {
            return Response::json(
                "200 OK",
                r#"{"results":[{"doc":{"_id":"doc-1","_rev":"1-rev","msgType":2,"contentType":"digital_content","name":"book.pdf","size":12,"updatedAt":90,"content":{"formats":["pdf"],"storage":{"pdf":{"oss":{"key":"uid-fixture/push/book.pdf","size":12}}}}}}]}"#,
            );
        }
        Response::json("404 Not Found", r#"{"error":"unexpected route"}"#)
    });
    let client = boox_core::api::create_client(5).unwrap();
    let mut refresh = input("explicit-token");
    refresh.share_devices = Some(vec![
        ShareTransferDevice {
            model: Some("Boox Note".into()),
            mac_address: Some("AA:BB:CC:DD:EE:FF".into()),
            host: Some("http://192.168.1.50:8080/".into()),
            status: Some("online".into()),
        },
        ShareTransferDevice {
            model: Some("Unmatched".into()),
            mac_address: Some("11:22:33:44:55:66".into()),
            host: None,
            status: None,
        },
    ]);
    let endpoints = ApiEndpoints::loopback(&server.endpoint, &server.endpoint).unwrap();

    let snapshot = fetch_dashboard_snapshot_with_endpoints(&endpoints, &client, &refresh)
        .expect("dashboard refresh");
    assert!(snapshot.auth.authorized);
    assert_eq!(snapshot.auth.source, "token");
    assert_eq!(snapshot.profile.as_ref().unwrap().uid, "uid-fixture");
    assert_eq!(
        snapshot.profile.as_ref().unwrap().nickname.as_deref(),
        Some("Reader")
    );
    assert_eq!(snapshot.storage.used, Some(250));
    assert_eq!(snapshot.storage.limit, Some(1000));
    assert_eq!(snapshot.storage.percent, Some(25.0));
    assert_eq!(snapshot.devices.len(), 1);
    assert_eq!(snapshot.devices[0].model.as_deref(), Some("Boox Note"));
    assert!(snapshot.devices[0].same_lan);
    assert_eq!(
        snapshot.devices[0].transfer_host.as_deref(),
        Some("http://192.168.1.50:8080/")
    );
    assert_eq!(
        snapshot.devices[0].same_lan_reason.as_deref(),
        Some("share_socket_mac")
    );
    assert_eq!(snapshot.push_queue.len(), 1);
    assert_eq!(snapshot.push_queue[0].id, "doc-1");
    assert_eq!(snapshot.push_queue[0].name, "book.pdf");
    assert_eq!(snapshot.calendar_metrics.reading_info["read"], 42);
    assert_eq!(
        snapshot.calendar_metrics.read_time_week["now"]["totalTime"],
        5_400_000
    );
    assert_eq!(
        snapshot
            .calendar_metrics
            .day_read_today
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(snapshot.upload.updated_ms, 7);
    assert_eq!(snapshot.upload.status_text, "上传进度: 空闲");

    let requests = server.requests();
    assert!(requests
        .iter()
        .any(|r| r.path() == "/api/1/statistics/readTimeInfo?date=2026-10-31&range=week"));
    assert!(requests
        .iter()
        .any(|r| r.path() == "/api/1/statistics/dayRead?startTime=2026-10-31&endTime=2026-10-31"));
    let authenticated = requests
        .iter()
        .filter(|r| !r.path().starts_with("/neocloud/"))
        .all(|r| r.header("authorization") == Some("Bearer explicit-token"));
    assert!(
        authenticated,
        "API requests must use only the explicit token"
    );
    assert!(requests
        .iter()
        .filter(|r| r.path().starts_with("/neocloud/"))
        .all(|r| r.header("authorization").is_none() && r.header("cookie").is_some()));
}

#[test]
fn successful_empty_queue_stays_distinct_from_failure() {
    let server = Loopback::with_routes(|method, path| {
        if let Some(response) = auth_routes(method, path) {
            return response;
        }
        if path.starts_with("/neocloud/_changes") {
            return Response::json("200 OK", r#"{"results":[]}"#);
        }
        Response::json("404 Not Found", r#"{"error":"unexpected route"}"#)
    });
    let endpoints = ApiEndpoints::loopback(&server.endpoint, &server.endpoint).unwrap();
    let client = boox_core::api::create_client(5).unwrap();

    let snapshot =
        fetch_dashboard_snapshot_with_endpoints(&endpoints, &client, &input("explicit-token"))
            .expect("a successful empty queue is a valid snapshot");
    assert!(snapshot.auth.authorized);
    assert!(snapshot.push_queue.is_empty());
}

#[test]
fn explicit_refresh_ignores_global_auth_upload_and_dashboard_cache() {
    let server = Loopback::with_routes(|method, path| {
        if let Some(response) = auth_routes(method, path) {
            return response;
        }
        if path.starts_with("/neocloud/_changes") {
            return Response::json("200 OK", r#"{"results":[]}"#);
        }
        Response::json("404 Not Found", r#"{"error":"unexpected route"}"#)
    });
    let endpoints = ApiEndpoints::loopback(&server.endpoint, &server.endpoint).unwrap();
    let client = boox_core::api::create_client(5).unwrap();

    let runtime = boox_core::state::global_state();
    let previous_token = {
        let mut auth = runtime.auth_state.lock().expect("global auth");
        let previous = auth.token.clone();
        auth.token = Some("global-secret-token".into());
        previous
    };
    let previous_upload = {
        let mut upload = runtime.upload_runtime_state.lock().expect("global upload");
        let previous = upload.clone();
        upload.status_text = "global upload".into();
        upload.updated_ms = 999;
        previous
    };
    let previous_cache = runtime.dashboard_cache.lock().expect("global cache").take();

    let snapshot =
        fetch_dashboard_snapshot_with_endpoints(&endpoints, &client, &input("explicit-token"))
            .expect("explicit refresh");

    runtime.auth_state.lock().expect("global auth").token = previous_token;
    *runtime.upload_runtime_state.lock().expect("global upload") = previous_upload;
    let cache_after_refresh = std::mem::replace(
        &mut *runtime.dashboard_cache.lock().expect("global cache"),
        previous_cache,
    );

    assert_eq!(snapshot.upload.status_text, "上传进度: 空闲");
    assert_eq!(snapshot.upload.updated_ms, 7);
    assert!(
        cache_after_refresh.is_none(),
        "an explicit refresh must not populate the core dashboard cache"
    );
    let api_requests: Vec<_> = server
        .requests()
        .into_iter()
        .filter(|request| !request.path().starts_with("/neocloud/"))
        .collect();
    assert!(!api_requests.is_empty());
    assert!(api_requests
        .iter()
        .all(|request| request.header("authorization") == Some("Bearer explicit-token")));
    assert!(api_requests.iter().all(|request| !request
        .header("authorization")
        .unwrap_or_default()
        .contains("global-secret-token")));
}

#[test]
fn endpoint_refresh_rejects_a_missing_share_projection_without_network() {
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    listener.set_nonblocking(true).unwrap();
    let endpoints = ApiEndpoints::loopback(&endpoint, &endpoint).unwrap();
    let client = boox_core::api::create_client(2).unwrap();

    let error = fetch_dashboard_snapshot_with_endpoints(
        &endpoints,
        &client,
        &DashboardRefreshInput::new("explicit-token", upload(), "2026-10-31"),
    )
    .expect_err("endpoint refresh must require an explicit share projection");
    assert_eq!(error.kind, DashboardErrorKind::Input);
    assert_eq!(error.stage, "devices");
    assert!(
        listener.accept().is_err(),
        "a missing projection must fail before any network request"
    );
}

#[test]
fn strict_refresh_retains_statistics_http_failure() {
    let server = Loopback::with_routes(|method, path| {
        if let Some(response) = auth_routes(method, path) {
            if path.starts_with("/api/1/statistics/readingInfo") {
                return Response::json("503 Service Unavailable", "statistics unavailable");
            }
            return response;
        }
        Response::json("404 Not Found", r#"{"error":"unexpected route"}"#)
    });
    let endpoints = ApiEndpoints::loopback(&server.endpoint, &server.endpoint).unwrap();
    let client = boox_core::api::create_client(5).unwrap();

    let error =
        fetch_dashboard_snapshot_with_endpoints(&endpoints, &client, &input("explicit-token"))
            .expect_err("strict refresh must retain a reading statistics failure");
    assert_eq!(error.kind, DashboardErrorKind::Network);
    assert_eq!(error.stage, "reading_info");
    assert_eq!(error.status, Some(503));
    assert!(!error.message.contains("statistics unavailable"));
}

#[test]
fn strict_refresh_retains_device_http_failure() {
    let server = Loopback::with_routes(|method, path| {
        if let Some(response) = auth_routes(method, path) {
            if path == "/api/1/users/getDevice" {
                return Response::json("502 Bad Gateway", "device upstream secret");
            }
            return response;
        }
        Response::json("404 Not Found", r#"{"error":"unexpected route"}"#)
    });
    let endpoints = ApiEndpoints::loopback(&server.endpoint, &server.endpoint).unwrap();
    let client = boox_core::api::create_client(5).unwrap();

    let error =
        fetch_dashboard_snapshot_with_endpoints(&endpoints, &client, &input("explicit-token"))
            .expect_err("strict refresh must retain a device failure");
    assert_eq!(error.kind, DashboardErrorKind::Network);
    assert_eq!(error.stage, "devices");
    assert_eq!(error.status, Some(502));
    assert!(!error.message.contains("device upstream secret"));
}

#[test]
fn strict_refresh_retains_each_reading_and_device_failure_stage() {
    for (route, stage) in [
        ("/api/1/statistics/readingInfo", "reading_info"),
        ("/api/1/statistics/readTimeInfo", "read_time_week"),
        ("/api/1/statistics/dayRead", "day_read_today"),
        ("/api/1/users/getDevice", "devices"),
    ] {
        let server = Loopback::with_routes(move |method, path| {
            if let Some(response) = auth_routes(method, path) {
                if path.starts_with(route) {
                    return Response::json("503 Service Unavailable", r#"{"secret":"stage-body"}"#);
                }
                return response;
            }
            Response::json("404 Not Found", r#"{"error":"unexpected route"}"#)
        });
        let endpoints = ApiEndpoints::loopback(&server.endpoint, &server.endpoint).unwrap();
        let client = boox_core::api::create_client(5).unwrap();

        let error =
            fetch_dashboard_snapshot_with_endpoints(&endpoints, &client, &input("explicit-token"))
                .expect_err("a strict refresh must retain this stage failure");
        assert_eq!(error.kind, DashboardErrorKind::Network, "stage {stage}");
        assert_eq!(error.stage, stage);
        assert_eq!(error.status, Some(503));
        assert!(!error.message.contains("stage-body"));
    }
}

#[test]
fn http_200_null_statistics_remain_valid_missing_values() {
    let server = Loopback::with_routes(|method, path| {
        if let Some(response) = auth_routes(method, path) {
            if path.starts_with("/api/1/statistics/readingInfo")
                || path.starts_with("/api/1/statistics/readTimeInfo")
                || path.starts_with("/api/1/statistics/dayRead")
            {
                return Response::json("200 OK", r#"{"result_code":0,"data":null}"#);
            }
            if path == "/api/1/users/getDevice" {
                return Response::json("200 OK", r#"{"result_code":0,"data":null}"#);
            }
            return response;
        }
        if path.starts_with("/neocloud/_changes") {
            return Response::json("200 OK", r#"{"results":[]}"#);
        }
        Response::json("404 Not Found", r#"{"error":"unexpected route"}"#)
    });
    let endpoints = ApiEndpoints::loopback(&server.endpoint, &server.endpoint).unwrap();
    let client = boox_core::api::create_client(5).unwrap();

    let snapshot =
        fetch_dashboard_snapshot_with_endpoints(&endpoints, &client, &input("explicit-token"))
            .expect("HTTP 200 with a null payload is a valid missing value");
    assert_eq!(
        snapshot.calendar_metrics.reading_info,
        serde_json::Value::Null
    );
    assert_eq!(
        snapshot.calendar_metrics.read_time_week,
        serde_json::Value::Null
    );
    assert_eq!(
        snapshot.calendar_metrics.day_read_today,
        serde_json::Value::Null
    );
    assert!(snapshot.devices.is_empty());
}
