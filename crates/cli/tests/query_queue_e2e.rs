mod common;

use common::{command_with_api, json_stdout, stderr, ApiServer, TestDir, TEST_OSS_ENDPOINT_ENV};
use std::process::Output;

#[test]
fn devices_storage_and_reading_use_the_loopback_api_contract() {
    let dir = TestDir::new();
    let server = ApiServer::start();

    let devices = run(&["--json", "devices", "list"], &dir, server.endpoint());
    assert_eq!(
        devices.status.code(),
        Some(0),
        "stdout: {} stderr: {}",
        common::stdout(&devices),
        stderr(&devices)
    );
    let value = json_stdout(&devices);
    assert_eq!(value["ok"], true);
    assert_eq!(value["data"]["devices"][0]["id"], "device-1");
    assert_eq!(value["data"]["devices"][0]["model"], "BOOX Note");
    assert_eq!(value["data"]["devices"][0]["login_status"], "online");

    let storage = run(&["--json", "storage", "status"], &dir, server.endpoint());
    assert_eq!(
        storage.status.code(),
        Some(0),
        "stdout: {} stderr: {}",
        common::stdout(&storage),
        stderr(&storage)
    );
    let value = json_stdout(&storage);
    assert_eq!(value["data"]["used"], 1000);
    assert_eq!(value["data"]["limit"], 4000);
    assert_eq!(value["data"]["percent"], 25.0);

    let reading = run(
        &["--json", "reading", "stats", "--date", "2026-10-29"],
        &dir,
        server.endpoint(),
    );
    assert_eq!(
        reading.status.code(),
        Some(0),
        "stdout: {} stderr: {}",
        common::stdout(&reading),
        stderr(&reading)
    );
    let value = json_stdout(&reading);
    assert_eq!(value["data"]["date"], "2026-10-29");
    assert_eq!(value["data"]["reading_info"]["todayReadSeconds"], 125);
    assert_eq!(value["data"]["week"]["seconds"], 700);
    assert_eq!(value["data"]["day"][0]["title"], "chapter-one");

    let requests = server.requests();
    let paths = requests
        .iter()
        .map(|request| format!("{} {}", request.method, request.path))
        .collect::<Vec<_>>();
    assert_eq!(
        paths,
        vec![
            "GET /api/1/users/me".to_string(),
            "GET /api/1/users/getDevice".to_string(),
            "GET /api/1/users/me".to_string(),
            "GET /api/1/statistics/v2/user/storage".to_string(),
            "GET /api/1/users/me".to_string(),
            "GET /api/1/statistics/readingInfo".to_string(),
            "GET /api/1/statistics/readTimeInfo?date=2026-10-29&range=week".to_string(),
            "GET /api/1/statistics/dayRead?startTime=2026-10-29&endTime=2026-10-29".to_string(),
        ]
    );
    assert!(requests.iter().all(|request| {
        request
            .header("authorization")
            .is_some_and(|value| value == "Bearer fixture-token")
    }));
}

#[test]
fn queue_list_and_delete_use_the_loopback_contract() {
    let dir = TestDir::new();
    let server = ApiServer::start();

    let listing = run(
        &["--json", "queue", "list", "--limit", "1"],
        &dir,
        server.endpoint(),
    );
    assert_eq!(
        listing.status.code(),
        Some(0),
        "stdout: {} stderr: {}",
        common::stdout(&listing),
        stderr(&listing)
    );
    let value = json_stdout(&listing);
    assert_eq!(value["ok"], true);
    assert_eq!(value["data"]["items"][0]["id"], "doc-fixture");
    assert_eq!(value["data"]["items"][0]["name"], "fixture.pdf");
    assert_eq!(value["data"]["items"][0]["format"], "pdf");
    assert_eq!(value["data"]["limit"], 1);

    let deleted = run(
        &["--json", "queue", "delete", "doc-fixture", "--yes"],
        &dir,
        server.endpoint(),
    );
    assert_eq!(
        deleted.status.code(),
        Some(0),
        "stdout: {} stderr: {}",
        common::stdout(&deleted),
        stderr(&deleted)
    );
    let value = json_stdout(&deleted);
    assert_eq!(value["data"]["complete"], true);
    assert_eq!(value["data"]["id_checked"], true);

    let requests = server.requests();
    let paths = requests
        .iter()
        .map(|request| format!("{} {}", request.method, request.path))
        .collect::<Vec<_>>();
    assert_eq!(
        paths,
        vec![
            "GET /api/1/users/me".to_string(),
            "GET /api/1/users/syncToken".to_string(),
            "GET /neocloud/_changes?filter=sync_gateway/bychannel&channels=uid-fixture-MESSAGE&include_docs=true&descending=true&limit=200".to_string(),
            "GET /api/1/users/me".to_string(),
            "GET /api/1/users/syncToken".to_string(),
            "GET /neocloud/doc-fixture".to_string(),
            "GET /api/1/users/syncToken".to_string(),
            "DELETE /neocloud/doc-fixture?rev=1-fixture".to_string(),
            "POST /api/1/push/message/batchDelete".to_string(),
        ]
    );
    let deletion = requests
        .iter()
        .find(|request| request.method == "DELETE")
        .expect("delete request");
    assert_eq!(
        deletion.header("cookie"),
        Some("SyncGatewaySession=fixture-session")
    );
    let batch = requests
        .iter()
        .find(|request| request.method == "POST")
        .expect("batch delete request");
    assert_eq!(batch.body, r#"{"ids":["doc-fixture"]}"#);
}

fn run(args: &[&str], dir: &TestDir, endpoint: &str) -> Output {
    command_with_api(args, &dir.config_dir(), endpoint)
        .env(TEST_OSS_ENDPOINT_ENV, endpoint)
        .env("BOOXCTL_TOKEN", "fixture-token")
        .output()
        .expect("run booxctl")
}

#[test]
fn queue_resend_uses_the_loopback_contract() {
    let dir = TestDir::new();
    let server = ApiServer::start();

    let output = run(
        &["--json", "queue", "resend", "doc-fixture"],
        &dir,
        server.endpoint(),
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "stdout: {} stderr: {}",
        common::stdout(&output),
        stderr(&output)
    );
    let value = json_stdout(&output);
    assert_eq!(value["data"]["resent"], true);

    let requests = server.requests();
    let paths = requests
        .iter()
        .map(|request| format!("{} {}", request.method, request.path))
        .collect::<Vec<_>>();
    assert!(paths.contains(&"GET /api/1/config/buckets".to_string()));
    assert!(paths.contains(&"GET /api/1/config/stss".to_string()));
    assert!(paths.contains(&"GET /neocloud/doc-fixture".to_string()));
    assert!(paths.contains(&"PUT /neocloud/doc-fixture".to_string()));
}

#[test]
fn push_uploads_and_reports_accepted_files() {
    use common::command_with_api_and_oss;
    let dir = TestDir::new();
    let server = ApiServer::start();
    let file = dir.path().join("book.pdf");
    std::fs::write(&file, b"%PDF-1.4 fixture").expect("write fixture file");

    let output = command_with_api_and_oss(
        &["--json", "push", file.to_str().expect("utf-8 path")],
        &dir.config_dir(),
        server.endpoint(),
        server.endpoint(),
    )
    .env("BOOXCTL_TOKEN", "fixture-token")
    .output()
    .expect("run booxctl");

    assert_eq!(
        output.status.code(),
        Some(0),
        "stdout: {} stderr: {}",
        common::stdout(&output),
        stderr(&output)
    );
    let value = json_stdout(&output);
    assert_eq!(value["ok"], true);
    assert_eq!(value["data"]["accepted"], 1);
    assert_eq!(value["data"]["failed"], 0);
    assert_eq!(value["data"]["files"][0]["outcome"], "accepted");
    assert!(value["data"]["files"][0]["message_id"].is_string());

    let requests = server.requests();
    let paths = requests
        .iter()
        .map(|request| format!("{} {}", request.method, request.path))
        .collect::<Vec<_>>();
    assert!(paths
        .iter()
        .any(|path| path == "POST /api/1/push/saveAndPush"));
    assert!(paths
        .iter()
        .any(|path| path.starts_with("PUT /uid-fixture/push/")));
    assert!(paths
        .iter()
        .any(|path| path.starts_with("GET /uid-fixture/push/")));
    assert!(paths
        .iter()
        .any(|path| path.starts_with("PUT /neocloud/") && path != "PUT /neocloud/doc-fixture"));
}
