mod common;

use common::{command_with_api, json_stdout, stderr, stdout, ApiFault, ApiServer, TestDir};

#[test]
fn body_read_complete_without_an_oss_acknowledgment_is_not_accepted() {
    let dir = TestDir::new();
    let server = ApiServer::start_with_fault(ApiFault::UploadResponseLost);
    let file = dir.path().join("book.pdf");
    std::fs::write(&file, b"fixture-book").unwrap();
    let output = command_with_api(
        &["--json", "push", file.to_str().unwrap()],
        &dir.config_dir(),
        server.endpoint(),
    )
    .env("SANDFLOS_TOKEN", "fixture-token")
    .output()
    .unwrap();
    assert_eq!(output.status.code(), Some(4), "{}", stdout(&output));
    let value = json_stdout(&output);
    assert_eq!(value["data"]["accepted"], 0);
    assert_eq!(value["data"]["files"][0]["stage"], "upload_oss");
    assert_eq!(value["data"]["files"][0]["outcome"], "failed");
    let requests = server.requests();
    assert!(requests.iter().any(|r| r.method == "PUT"
        && r.path.starts_with("/uid-fixture/push/")
        && r.body == "fixture-book"));
    assert!(!requests.iter().any(|r| r.path.starts_with("/neocloud/")));
    assert!(!requests.iter().any(|r| r.path == "/api/1/push/saveAndPush"));
}

#[test]
fn synchronization_errors_never_echo_remote_revision_into_json_or_logs() {
    let dir = TestDir::new();
    let server = ApiServer::start_with_fault(ApiFault::SyncRevisionSecret);
    let file = dir.path().join("book.pdf");
    std::fs::write(&file, b"fixture-book").unwrap();
    let output = command_with_api(
        &["--json", "--verbose", "push", file.to_str().unwrap()],
        &dir.config_dir(),
        server.endpoint(),
    )
    .env("SANDFLOS_TOKEN", "fixture-token")
    .output()
    .unwrap();
    assert_eq!(output.status.code(), Some(6), "{}", stdout(&output));
    let value = json_stdout(&output);
    assert_eq!(value["data"]["files"][0]["outcome"], "unknown");
    assert!(value["data"]["files"][0]["message_id"].is_string());
    assert!(
        !stdout(&output).contains("upstream-secret"),
        "{}",
        stdout(&output)
    );
    assert!(
        !stderr(&output).contains("upstream-secret"),
        "{}",
        stderr(&output)
    );
    assert!(!server
        .requests()
        .iter()
        .any(|r| r.path == "/api/1/push/saveAndPush"));
}
