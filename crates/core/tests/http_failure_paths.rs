//! Protocol-level failure paths for push submission and queue mutation.
//!
//! A timeout, redirect, gateway error, or unparseable success response cannot
//! prove that the server rejected `saveAndPush`. Without an idempotency
//! guarantee, repeating it could repeat its effects. Submit once and let the
//! caller report an unknown outcome for queue reconciliation.
//!
//! Queue deletion has two steps. A missing remote document must still allow
//! the message record to be removed, while a failure after a successful
//! remote delete must be reported as partial and unknown.

use boox_core::api::{save_and_push_with_endpoints, ApiEndpoints};
use boox_core::models::UploadAuthContext;
use boox_core::services::{
    HttpQueueMutationBackend, HttpQueueReadBackend, QueueService, ServiceErrorKind,
};
use serde_json::Value;
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::TcpListener,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread::JoinHandle,
    time::Duration,
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
    body: String,
}

impl Recorded {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find_map(|line| line.strip_prefix(&format!("{name}:")))
            .map(str::trim)
    }
}

#[derive(Clone, Debug)]
struct Response {
    status: &'static str,
    extra_headers: Vec<&'static str>,
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

impl Loopback {
    fn gateway_error() -> Self {
        Self::with_routes(|_, _| Response::json("502 Bad Gateway", "{}"))
    }

    fn redirect() -> Self {
        Self::with_routes(|_, _| Response {
            status: "302 Found",
            extra_headers: vec!["Location: /api/1/push/saveAndPush-retry"],
            body: String::new(),
        })
    }

    fn malformed_success() -> Self {
        Self::with_routes(|_, _| Response::json("200 OK", "<html>not an api envelope</html>"))
    }

    fn with_routes(routes: impl Fn(&str, &str) -> Response + Send + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback fixture");
        listener
            .set_nonblocking(true)
            .expect("nonblocking loopback fixture");
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
                let mut content_length = 0_usize;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).is_err() || line == "\r\n" || line.is_empty() {
                        break;
                    }
                    let trimmed = line.trim_end();
                    if let Some(value) = trimmed
                        .strip_prefix("content-length:")
                        .or_else(|| trimmed.strip_prefix("Content-Length:"))
                    {
                        content_length = value.trim().parse().unwrap_or(0);
                    }
                    headers.push(trimmed.to_string());
                }
                let mut body = vec![0_u8; content_length];
                let _ = reader.read_exact(&mut body);
                observed.lock().expect("observed requests").push(Recorded {
                    request_line,
                    headers,
                    body: String::from_utf8_lossy(&body).into_owned(),
                });
                let Response {
                    status,
                    extra_headers,
                    body,
                } = {
                    let recorded = observed.lock().expect("observed requests");
                    let last = recorded.last().expect("recorded request");
                    let path = last.request_line.split_whitespace().nth(1).unwrap_or("/");
                    let method = last.request_line.split_whitespace().next().unwrap_or("GET");
                    routes(method, path)
                };
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

    fn requests(&self) -> Vec<Recorded> {
        self.requests.lock().expect("observed requests").clone()
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

fn auth() -> UploadAuthContext {
    UploadAuthContext {
        bearer: "fixture-token".into(),
        uid: "uid-fixture".into(),
        storage_limit: None,
        storage_used: None,
    }
}

#[test]
fn save_and_push_submits_once_on_an_ambiguous_gateway_error() {
    let server = Loopback::gateway_error();
    let endpoints =
        ApiEndpoints::loopback(&server.endpoint, &server.endpoint).expect("loopback endpoints");

    let client = boox_core::api::create_client(10).expect("client");
    let result = save_and_push_with_endpoints(
        &endpoints,
        &client,
        &auth(),
        "onyx-cloud",
        "uid-fixture/push/book.pdf",
        "book.pdf",
        "pdf",
        "doc-fixture",
        "1-fixture",
    );

    assert!(result.is_err(), "gateway failure must not look accepted");
    let requests = server.requests();
    assert_eq!(
        requests.len(),
        1,
        "saveAndPush must not retry an ambiguous submit: {requests:#?}"
    );
    let request = &requests[0];
    assert_eq!(
        request.request_line.trim_end(),
        "POST /api/1/push/saveAndPush HTTP/1.1"
    );
    assert_eq!(
        request.header("authorization"),
        Some("Bearer fixture-token")
    );
    let body: Value = serde_json::from_str(&request.body).expect("json submit body");
    assert_eq!(body["data"]["bucket"], "onyx-cloud");
    assert_eq!(body["data"]["resourceKey"], "uid-fixture/push/book.pdf");
    assert_eq!(body["data"]["resourceType"], "pdf");
    assert_eq!(body["cbMsg"]["id"], "doc-fixture");
    assert_eq!(body["cbMsg"]["rev"], "1-fixture");
}

#[test]
fn sts_and_queue_write_use_the_injected_origin() {
    let server = Loopback::with_routes(|method, path| {
        match (method, path) {
        ("GET", "/api/1/config/stss") => Response::json(
            "200 OK",
            r#"{"result_code":0,"data":{"AccessKeyId":"ak","AccessKeySecret":"sk","SecurityToken":"sts-token","Expiration":"2099-01-01T00:00:00Z"}}"#,
        ),
        ("GET", "/api/1/users/syncToken") => Response {
            status: "200 OK",
            extra_headers: vec!["Set-Cookie: RouteSession=route-cookie; Path=/neocloud"],
            body: r#"{"result_code":0,"data":{"cookie_name":"SyncGatewaySession","session_id":"fixture-session"}}"#.to_string(),
        },
        ("PUT", _) if path.starts_with("/neocloud/") => {
            Response::json("201 Created", r#"{"ok":true,"id":"doc-fixture","rev":"1-fixture"}"#)
        }
        _ => Response::json("404 Not Found", r#"{"error":"unexpected route"}"#),
    }
    });
    let endpoints =
        ApiEndpoints::loopback(&server.endpoint, &server.endpoint).expect("loopback endpoints");
    let client = boox_core::api::create_client(10).expect("client");

    let sts = boox_core::api::fetch_sts_for_auth_with_endpoints(&endpoints, &client, &auth())
        .expect("sts");
    assert_eq!(sts.access_key_id, "ak");
    assert_eq!(sts.security_token, "sts-token");

    let (id, rev) = boox_core::api::put_push_message_doc_with_endpoints(
        &endpoints,
        &client,
        &auth(),
        "book.pdf",
        42,
        "pdf",
        "uid-fixture/push/book.pdf",
        "https://signed.example/book.pdf",
    )
    .expect("queue write");
    assert_eq!(id, "doc-fixture");
    assert_eq!(rev, "1-fixture");

    let requests = server.requests();
    let paths = requests
        .iter()
        .map(|request| {
            let mut parts = request.request_line.split_whitespace();
            format!(
                "{} {}",
                parts.next().unwrap_or_default(),
                parts.next().unwrap_or_default()
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        paths[..2],
        ["GET /api/1/config/stss", "GET /api/1/users/syncToken"]
    );
    let write_path = paths[2]
        .strip_prefix("PUT /neocloud/")
        .expect("queue write path");
    assert_eq!(write_path.len(), 36, "queue write uses a UUID message id");
    for request in &requests[..2] {
        assert_eq!(
            request.header("authorization"),
            Some("Bearer fixture-token"),
            "API calls carry the bearer token"
        );
    }
    let write = &requests[2];
    let cookie = write.header("cookie").unwrap_or_default();
    assert!(
        cookie.contains("SyncGatewaySession=fixture-session"),
        "neocloud write authenticates with the sync cookie, got {cookie:?}"
    );
    assert!(
        cookie.contains("RouteSession=route-cookie"),
        "path-scoped cookie missing: {cookie:?}"
    );
    let doc: Value = serde_json::from_str(&write.body).expect("queue doc body");
    assert_eq!(doc["dbId"], "uid-fixture-MESSAGE");
    assert_eq!(doc["name"], "book.pdf");
    assert_eq!(doc["size"], 42);
    assert_eq!(doc["_id"], write_path);
    let content: Value = serde_json::from_str(doc["content"].as_str().expect("content string"))
        .expect("content json");
    assert_eq!(
        content["storage"]["pdf"]["oss"]["key"],
        "uid-fixture/push/book.pdf"
    );
    assert_eq!(content["storage"]["pdf"]["oss"]["size"], 42);
}

#[test]
fn save_and_push_does_not_follow_a_redirect() {
    let server = Loopback::redirect();
    let endpoints =
        ApiEndpoints::loopback(&server.endpoint, &server.endpoint).expect("loopback endpoints");
    let client = boox_core::api::create_client(10).expect("client");

    let result = save_and_push_with_endpoints(
        &endpoints,
        &client,
        &auth(),
        "onyx-cloud",
        "uid-fixture/push/book.pdf",
        "book.pdf",
        "pdf",
        "doc-fixture",
        "1-fixture",
    );

    assert!(result.is_err(), "a redirect must not look accepted");
    let requests = server.requests();
    assert_eq!(
        requests.len(),
        1,
        "the client must not follow or retry a redirected submit: {requests:#?}"
    );
    assert_eq!(
        requests[0].request_line.trim_end(),
        "POST /api/1/push/saveAndPush HTTP/1.1"
    );
}

#[test]
fn save_and_push_rejects_an_unparseable_success_response_without_retrying() {
    let server = Loopback::malformed_success();
    let endpoints =
        ApiEndpoints::loopback(&server.endpoint, &server.endpoint).expect("loopback endpoints");
    let client = boox_core::api::create_client(10).expect("client");

    let result = save_and_push_with_endpoints(
        &endpoints,
        &client,
        &auth(),
        "onyx-cloud",
        "uid-fixture/push/book.pdf",
        "book.pdf",
        "pdf",
        "doc-fixture",
        "1-fixture",
    );

    assert!(
        result.is_err(),
        "HTTP 200 with an invalid envelope must not look accepted"
    );
    let requests = server.requests();
    assert_eq!(
        requests.len(),
        1,
        "an unparseable success is ambiguous and must not be retried: {requests:#?}"
    );
}

fn queue_auth_routes(method: &str, path: &str) -> Option<Response> {
    match (method, path) {
        ("GET", "/api/1/users/me") => Some(Response::json(
            "200 OK",
            r#"{"result_code":0,"data":{"uid":"uid-fixture","nickname":"reader"}}"#,
        )),
        ("GET", "/api/1/statistics/v2/user/storage") => Some(Response::json(
            "200 OK",
            r#"{"result_code":0,"data":{"usedSize":1000,"totalSize":4000}}"#,
        )),
        ("GET", "/api/1/users/syncToken") => Some(Response {
            status: "200 OK",
            extra_headers: vec!["Set-Cookie: SyncGatewaySession=fixture-session; Path=/"],
            body: r#"{"result_code":0,"data":{"cookie_name":"SyncGatewaySession","session_id":"fixture-session"}}"#.to_string(),
        }),
        _ => None,
    }
}

fn queue_mutation_service(endpoint: &str) -> QueueService<HttpQueueMutationBackend> {
    let backend = HttpQueueMutationBackend::with_test_endpoints(
        Duration::from_secs(5),
        "fixture-token",
        endpoint,
        endpoint,
    )
    .expect("queue backend");
    QueueService::new(backend)
}

#[test]
fn queue_delete_treats_a_missing_document_as_idempotent_and_removes_the_record() {
    let server = Loopback::with_routes(|method, path| {
        if let Some(response) = queue_auth_routes(method, path) {
            return response;
        }
        match (method, path) {
            ("GET", "/neocloud/missing-doc") => {
                Response::json("404 Not Found", r#"{"error":"missing"}"#)
            }
            ("POST", "/api/1/push/message/batchDelete") => {
                Response::json("200 OK", r#"{"result_code":0,"message":null,"data":null}"#)
            }
            _ => Response::json("404 Not Found", r#"{"error":"unexpected route"}"#),
        }
    });
    let mut service = queue_mutation_service(&server.endpoint);

    let outcome = service.delete("missing-doc", true).expect("delete outcome");

    assert!(outcome.complete);
    assert!(!outcome.unknown);
    assert!(!outcome.remote_delete_attempted);
    assert!(outcome.message_delete_attempted);
    assert!(outcome.error.is_none());

    let requests = server.requests();
    let paths = requests
        .iter()
        .map(|request| {
            let mut parts = request.request_line.split_whitespace();
            format!(
                "{} {}",
                parts.next().unwrap_or_default(),
                parts.next().unwrap_or_default()
            )
        })
        .collect::<Vec<_>>();
    assert!(
        !paths.iter().any(|path| path.starts_with("DELETE ")),
        "a missing remote document needs no DELETE: {paths:#?}"
    );
    let batch = requests
        .iter()
        .find(|request| {
            request
                .request_line
                .starts_with("POST /api/1/push/message/batchDelete")
        })
        .expect("message record delete");
    assert_eq!(batch.body, r#"{"ids":["missing-doc"]}"#);
}

#[test]
fn queue_delete_reports_unknown_when_the_second_step_fails() {
    let document = r#"{"_id":"doc-fixture","_rev":"1-fixture","msgType":2,"contentType":"digital_content","name":"book.pdf","size":42,"storageKey":"uid-fixture/push/book.pdf","content":"{\"formats\":[\"pdf\"],\"storage\":{\"pdf\":{\"oss\":{\"key\":\"uid-fixture/push/book.pdf\",\"size\":42}}}}"}"#;
    let server = Loopback::with_routes(move |method, path| {
        if let Some(response) = queue_auth_routes(method, path) {
            return response;
        }
        match (method, path) {
            ("GET", "/neocloud/doc-fixture") => Response::json("200 OK", document),
            ("DELETE", path) if path.starts_with("/neocloud/doc-fixture?rev=") => {
                Response::json("200 OK", "{}")
            }
            ("POST", "/api/1/push/message/batchDelete") => Response::json(
                "503 Service Unavailable",
                r#"{"error":"message store unavailable"}"#,
            ),
            _ => Response::json("404 Not Found", r#"{"error":"unexpected route"}"#),
        }
    });
    let mut service = queue_mutation_service(&server.endpoint);

    let outcome = service.delete("doc-fixture", true).expect("delete outcome");

    assert!(!outcome.complete);
    assert!(outcome.unknown);
    assert!(outcome.remote_delete_attempted);
    assert!(outcome.message_delete_attempted);
    let error = outcome.error.expect("partial delete error");
    assert_eq!(error.kind, ServiceErrorKind::Network);
    assert_eq!(error.stage, "queue.delete");
    assert!(
        !error.message.contains("message store unavailable"),
        "the server body must not be surfaced: {}",
        error.message
    );

    let requests = server.requests();
    let paths = requests
        .iter()
        .map(|request| {
            let mut parts = request.request_line.split_whitespace();
            format!(
                "{} {}",
                parts.next().unwrap_or_default(),
                parts.next().unwrap_or_default()
            )
        })
        .collect::<Vec<_>>();
    assert!(paths.contains(&"DELETE /neocloud/doc-fixture?rev=1-fixture".to_string()));
    assert!(paths.contains(&"POST /api/1/push/message/batchDelete".to_string()));
}

#[test]
fn queue_delete_stops_before_mutating_when_the_document_lookup_fails() {
    let server = Loopback::with_routes(|method, path| {
        if let Some(response) = queue_auth_routes(method, path) {
            return response;
        }
        match (method, path) {
            ("GET", "/neocloud/doc-fixture") => Response::json(
                "503 Service Unavailable",
                r#"{"error":"neocloud unavailable"}"#,
            ),
            _ => Response::json("404 Not Found", r#"{"error":"unexpected route"}"#),
        }
    });
    let mut service = queue_mutation_service(&server.endpoint);

    let error = service
        .delete("doc-fixture", true)
        .expect_err("lookup failure must abort the delete");

    assert_eq!(error.kind, ServiceErrorKind::Network);
    assert_eq!(error.stage, "queue.delete");
    assert!(
        !error.message.contains("neocloud unavailable"),
        "the server body must not be surfaced: {}",
        error.message
    );
    let requests = server.requests();
    assert!(
        requests.iter().all(|request| {
            !request.request_line.starts_with("DELETE ")
                && !request
                    .request_line
                    .starts_with("POST /api/1/push/message/batchDelete")
        }),
        "no mutation may run when looking up the document failed: {requests:#?}"
    );
}

#[test]
fn queue_list_classifies_authentication_failures_without_leaking_the_body() {
    let server = Loopback::with_routes(|method, path| match (method, path) {
        ("GET", "/api/1/users/me") => Response::json(
            "401 Unauthorized",
            r#"{"error":"token fixture-secret-value expired"}"#,
        ),
        _ => Response::json("404 Not Found", r#"{"error":"unexpected route"}"#),
    });
    let backend = HttpQueueReadBackend::with_test_endpoints(
        Duration::from_secs(5),
        "fixture-token",
        &server.endpoint,
        &server.endpoint,
    )
    .expect("queue backend");
    let error = QueueService::new(backend)
        .list(None)
        .expect_err("authentication must fail");

    assert_eq!(error.kind, ServiceErrorKind::Authentication);
    assert_eq!(error.code, "credential_rejected");
    assert!(
        !error.message.contains("fixture-secret-value"),
        "the server body must not be surfaced: {}",
        error.message
    );
    assert_eq!(
        server.requests().len(),
        1,
        "authentication rejection must stop before queue access"
    );
}

#[test]
fn queue_list_rejects_a_malformed_sync_response_as_a_protocol_error() {
    let server = Loopback::with_routes(|method, path| {
        if let Some(response) = queue_auth_routes(method, path) {
            return response;
        }
        match (method, path) {
            (_, path) if path.starts_with("/neocloud/_changes") => {
                Response::json("200 OK", "<html>proxy login page</html>")
            }
            _ => Response::json("404 Not Found", r#"{"error":"unexpected route"}"#),
        }
    });
    let backend = HttpQueueReadBackend::with_test_endpoints(
        Duration::from_secs(5),
        "fixture-token",
        &server.endpoint,
        &server.endpoint,
    )
    .expect("queue backend");
    let error = QueueService::new(backend)
        .list(None)
        .expect_err("malformed queue response must fail");

    assert_eq!(error.kind, ServiceErrorKind::Protocol);
    assert_eq!(error.code, "protocol_error");
    assert!(
        !error.message.contains("proxy login page"),
        "the malformed body must not be surfaced: {}",
        error.message
    );
}

fn push_fixture_routes(method: &str, path: &str) -> Response {
    if let Some(response) = queue_auth_routes(method, path) {
        return response;
    }
    match (method, path) {
        ("GET", "/api/1/config/buckets") => Response::json(
            "200 OK",
            r#"{"result_code":0,"data":{"onyx-cloud":{"bucket":"onyx-cloud","aliEndpoint":"https://oss-fixture.invalid"}}}"#,
        ),
        ("GET", "/api/1/config/stss") => Response::json(
            "200 OK",
            r#"{"result_code":0,"data":{"AccessKeyId":"fixture-key","AccessKeySecret":"fixture-secret","SecurityToken":"fixture-security","Expiration":"2099-01-01T00:00:00Z"}}"#,
        ),
        ("GET", "/neocloud/doc-fixture") => Response::json(
            "200 OK",
            r#"{"_id":"doc-fixture","_rev":"1-fixture","name":"book.pdf","size":42,"storageKey":"uid-fixture/push/book.pdf","content":"{\"formats\":[\"pdf\"]}"}"#,
        ),
        ("GET", path) if path.starts_with("/uid-fixture/push/") => {
            Response::json("206 Partial Content", "")
        }
        ("PUT", path) if path.starts_with("/uid-fixture/push/") => Response::json("200 OK", ""),
        ("PUT", path) if path.starts_with("/neocloud/") => {
            Response::json("502 Bad Gateway", "do not echo this body")
        }
        _ => Response::json("404 Not Found", "unexpected fixture route"),
    }
}

#[test]
fn ambiguous_resend_submits_once_and_reports_unknown() {
    let server = Loopback::with_routes(push_fixture_routes);
    let error = queue_mutation_service(&server.endpoint)
        .resend("doc-fixture")
        .expect_err("a gateway error cannot confirm a resend");
    let writes = server
        .requests()
        .iter()
        .filter(|request| request.request_line.starts_with("PUT /neocloud/"))
        .count();
    assert_eq!(
        (writes, error.kind),
        (1, ServiceErrorKind::Unknown),
        "{error:?}"
    );
    assert_eq!(error.kind.exit_code(), 6);
}

#[test]
fn unavailable_quota_stops_before_oss_upload() {
    let server = Loopback::with_routes(|method, path| {
        if path == "/api/1/statistics/v2/user/storage" {
            return Response::json("500 Internal Server Error", "private quota error");
        }
        push_fixture_routes(method, path)
    });
    let path = std::env::temp_dir().join(format!("quota-{}.pdf", uuid::Uuid::new_v4()));
    std::fs::write(&path, b"fixture PDF").unwrap();
    let backend = boox_core::services::HttpPushBackend::with_test_endpoints(
        Duration::from_secs(2),
        "fixture-token",
        &server.endpoint,
        &server.endpoint,
    )
    .unwrap();
    let report = boox_core::services::PushService::new(backend)
        .push_files(std::slice::from_ref(&path), |_| {});
    std::fs::remove_file(path).unwrap();
    assert_eq!(report.exit_code(), 4);
    assert_eq!(report.accepted, 0);
    let requests = server.requests();
    assert!(
        requests.iter().all(|request| {
            request.request_line.starts_with("GET /api/1/users/me ")
                || request
                    .request_line
                    .starts_with("GET /api/1/statistics/v2/user/storage ")
        }),
        "quota failure must stop before bucket/STS/OSS requests: {requests:#?}"
    );
}

#[test]
fn unconfirmed_message_write_is_unknown_and_keeps_the_resource_key() {
    let server = Loopback::with_routes(push_fixture_routes);
    let path = std::env::temp_dir().join(format!("write-{}.pdf", uuid::Uuid::new_v4()));
    std::fs::write(&path, b"fixture PDF").unwrap();
    let backend = boox_core::services::HttpPushBackend::with_test_endpoints(
        Duration::from_secs(2),
        "fixture-token",
        &server.endpoint,
        &server.endpoint,
    )
    .unwrap();
    let report = boox_core::services::PushService::new(backend)
        .push_files(std::slice::from_ref(&path), |_| {});
    std::fs::remove_file(path).unwrap();
    assert_eq!(
        (report.exit_code(), report.unknown, report.accepted),
        (6, 1, 0)
    );
    assert_eq!(
        report.files[0].stage,
        boox_core::services::PushStage::WriteMessage
    );
    assert!(report.files[0].resource_key.is_some());
    let writes = server
        .requests()
        .iter()
        .filter(|request| request.request_line.starts_with("PUT /neocloud/"))
        .count();
    assert_eq!(writes, 1);
}

#[test]
fn device_queries_do_not_depend_on_storage_statistics() {
    let server = Loopback::with_routes(|method, path| {
        if (method, path) == ("GET", "/api/1/users/getDevice") {
            return Response::json("200 OK", r#"{"result_code":0,"data":[]}"#);
        }
        if path == "/api/1/statistics/v2/user/storage" {
            return Response::json("500 Internal Server Error", "unavailable");
        }
        push_fixture_routes(method, path)
    });
    let backend = boox_core::services::HttpQueryBackend::with_test_endpoints(
        Duration::from_secs(2),
        "fixture-token",
        &server.endpoint,
        &server.endpoint,
    )
    .unwrap();
    let devices = boox_core::services::QueryService::new(backend)
        .devices()
        .unwrap();
    assert!(devices.is_empty());
    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    assert!(requests
        .iter()
        .all(|request| !request.request_line.contains("/storage")));
}

#[test]
fn resend_parent_fallback_preserves_the_original_format() {
    let server = Loopback::with_routes(|method, path| {
        if (method, path) == ("GET", "/neocloud/doc-fixture") {
            return Response::json(
                "200 OK",
                r#"{"_id":"doc-fixture","_rev":"1-fixture","parent":"parent-fixture","name":"book.epub","size":42,"storageKey":"uid-fixture/push/book.epub","content":"{\"formats\":[\"epub\"]}"}"#,
            );
        }
        if (method, path) == ("PUT", "/neocloud/doc-fixture") {
            return Response::json("404 Not Found", "missing");
        }
        if method == "PUT" && path.starts_with("/neocloud/") {
            return Response::json("201 Created", r#"{"id":"new-doc","rev":"1-new"}"#);
        }
        push_fixture_routes(method, path)
    });
    queue_mutation_service(&server.endpoint)
        .resend("doc-fixture")
        .unwrap();
    let requests = server.requests();
    let recreated = requests
        .iter()
        .find(|request| {
            request.request_line.starts_with("PUT /neocloud/")
                && !request
                    .request_line
                    .starts_with("PUT /neocloud/doc-fixture ")
        })
        .expect("fallback recreates a message");
    let doc: Value = serde_json::from_str(&recreated.body).unwrap();
    let content: Value = serde_json::from_str(doc["content"].as_str().unwrap()).unwrap();
    assert_eq!(content["formats"][0], "epub");
    assert!(content["storage"]["epub"].is_object());
}

#[test]
fn queue_http_requests_honour_the_configured_timeout() {
    let server = Loopback::with_routes(|method, path| {
        if path.starts_with("/neocloud/_changes?") {
            std::thread::sleep(Duration::from_secs(2));
            return Response::json("200 OK", r#"{"results":[]}"#);
        }
        push_fixture_routes(method, path)
    });
    let backend = HttpQueueReadBackend::with_test_endpoints(
        Duration::from_secs(1),
        "fixture-token",
        &server.endpoint,
        &server.endpoint,
    )
    .unwrap();
    let result = QueueService::new(backend).list(None);
    assert!(
        result.is_err(),
        "a request exceeding the configured timeout must fail"
    );
    assert_eq!(result.unwrap_err().kind.exit_code(), 4);
}
