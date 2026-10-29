//! Process-level failure contracts: every failing command must keep stdout to
//! one JSON envelope, map to the documented exit code, and never echo a token
//! or a raw server body.

mod common;

use common::{
    command, command_with_api, command_with_api_and_oss, json_stdout, stderr, stdout, ApiFault,
    ApiServer, TestDir,
};

fn assert_single_error_envelope(output: &std::process::Output) -> serde_json::Value {
    assert_eq!(stderr(output), "", "stderr must stay empty in --json mode");
    let value = json_stdout(output);
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["ok"], false);
    assert!(value["error"]["code"].is_string(), "value: {value}");
    value
}

#[test]
fn rejected_credential_is_exit_3_without_leaking_the_server_body() {
    let dir = TestDir::new();
    let server = ApiServer::start_with_fault(ApiFault::CredentialRejected);

    let output = command_with_api(
        &["--json", "devices", "list"],
        &dir.config_dir(),
        server.endpoint(),
    )
    .env("SANDFLOS_TOKEN", "expired-token")
    .output()
    .expect("run sandflos");

    assert_eq!(output.status.code(), Some(3), "stdout: {}", stdout(&output));
    let value = assert_single_error_envelope(&output);
    assert_eq!(value["error"]["code"], "credential_rejected");
    let printed = stdout(&output);
    assert!(!printed.contains("expired-token"));
    assert!(!printed.contains("server body must not be echoed"));
}

#[test]
fn server_error_is_exit_4_and_does_not_echo_the_upstream_body() {
    let dir = TestDir::new();
    let server = ApiServer::start_with_fault(ApiFault::ServerError);

    let output = command_with_api(
        &["--json", "storage", "status"],
        &dir.config_dir(),
        server.endpoint(),
    )
    .env("SANDFLOS_TOKEN", "fixture-token")
    .output()
    .expect("run sandflos");

    assert_eq!(output.status.code(), Some(4), "stdout: {}", stdout(&output));
    let value = assert_single_error_envelope(&output);
    assert_eq!(value["error"]["code"], "network_error");
    assert!(!stdout(&output).contains("fixture internal failure"));
}

#[test]
fn malformed_upstream_payload_is_a_protocol_error_exit_4() {
    let dir = TestDir::new();
    let server = ApiServer::start_with_fault(ApiFault::MalformedUser);

    let output = command_with_api(
        &["--json", "reading", "stats", "--date", "2026-10-29"],
        &dir.config_dir(),
        server.endpoint(),
    )
    .env("SANDFLOS_TOKEN", "fixture-token")
    .output()
    .expect("run sandflos");

    assert_eq!(output.status.code(), Some(4), "stdout: {}", stdout(&output));
    let value = assert_single_error_envelope(&output);
    assert_eq!(value["error"]["code"], "protocol_error");
    assert!(!stdout(&output).contains("not json"));
}

#[test]
fn corrupted_credential_file_is_local_io_not_missing_credentials() {
    let dir = TestDir::new();
    let config = dir.config_dir();
    std::fs::create_dir_all(&config).expect("create config");
    std::fs::write(config.join("auth.json"), "{not json").expect("write corrupt credential");

    let output = command(&["--json", "queue", "list"], &config)
        .output()
        .expect("run sandflos");

    assert_eq!(output.status.code(), Some(5), "stdout: {}", stdout(&output));
    let value = assert_single_error_envelope(&output);
    assert_eq!(value["error"]["code"], "credential_resolution_failed");
    assert!(value["error"]["message"]
        .as_str()
        .is_some_and(|message| !message.contains("尚未登录")));
}

#[test]
fn only_the_api_test_endpoint_is_honoured_without_falling_back_to_production() {
    let dir = TestDir::new();
    let server = ApiServer::start();

    // No OSS endpoint is set. Falling back to production here would either
    // hang on the network or return a different fixture-independent failure.
    let output = command_with_api(
        &["--json", "devices", "list"],
        &dir.config_dir(),
        server.endpoint(),
    )
    .env("SANDFLOS_TOKEN", "fixture-token")
    .output()
    .expect("run sandflos");

    assert_eq!(output.status.code(), Some(0), "stdout: {}", stdout(&output));
    let value = json_stdout(&output);
    assert_eq!(value["data"]["devices"][0]["id"], "device-1");
    assert_eq!(
        server.requests()[0].path,
        "/api/1/users/me",
        "the loopback fixture must be the only endpoint contacted"
    );
}

#[test]
fn a_rejected_credential_during_push_keeps_the_per_file_report() {
    let dir = TestDir::new();
    let server = ApiServer::start_with_fault(ApiFault::CredentialRejected);
    let file = dir.path().join("book.pdf");
    std::fs::write(&file, b"%PDF-1.4 fixture").expect("write fixture file");

    let output = command_with_api_and_oss(
        &["--json", "push", file.to_str().expect("utf-8 path")],
        &dir.config_dir(),
        server.endpoint(),
        server.endpoint(),
    )
    .env("SANDFLOS_TOKEN", "expired-token")
    .output()
    .expect("run sandflos");

    assert_eq!(output.status.code(), Some(3), "stdout: {}", stdout(&output));
    let value = assert_single_error_envelope(&output);
    assert_eq!(value["error"]["code"], "push_incomplete");
    assert_eq!(value["data"]["accepted"], 0);
    assert_eq!(value["data"]["failed"], 1);
    assert_eq!(value["data"]["files"][0]["outcome"], "failed");
    assert_eq!(value["data"]["files"][0]["stage"], "validate");
    assert_eq!(value["data"]["files"][0]["error"]["kind"], "authentication");
    assert!(!stdout(&output).contains("expired-token"));
}

#[test]
fn failed_save_and_push_is_exit_6_unknown_with_the_message_id_kept() {
    let dir = TestDir::new();
    let server = ApiServer::start_with_fault(ApiFault::SaveAndPushFails);
    let file = dir.path().join("book.pdf");
    std::fs::write(&file, b"%PDF-1.4 fixture").expect("write fixture file");

    let output = command_with_api_and_oss(
        &["--json", "push", file.to_str().expect("utf-8 path")],
        &dir.config_dir(),
        server.endpoint(),
        server.endpoint(),
    )
    .env("SANDFLOS_TOKEN", "fixture-token")
    .output()
    .expect("run sandflos");

    assert_eq!(output.status.code(), Some(6), "stdout: {}", stdout(&output));
    let value = assert_single_error_envelope(&output);
    assert_eq!(value["error"]["code"], "push_incomplete");
    assert_eq!(value["data"]["accepted"], 0);
    assert_eq!(value["data"]["unknown"], 1);
    assert_eq!(value["data"]["files"][0]["outcome"], "unknown");
    assert_eq!(value["data"]["files"][0]["stage"], "save_and_push");
    assert!(
        value["data"]["files"][0]["message_id"].is_string(),
        "the accepted message id must survive for a safe retry decision: {value}"
    );
}

#[test]
fn half_completed_queue_delete_is_exit_6_and_reports_the_partial_state() {
    let dir = TestDir::new();
    let server = ApiServer::start_with_fault(ApiFault::DeleteMessageFails);

    let output = command_with_api(
        &["--json", "queue", "delete", "doc-fixture", "--yes"],
        &dir.config_dir(),
        server.endpoint(),
    )
    .env("SANDFLOS_TOKEN", "fixture-token")
    .output()
    .expect("run sandflos");

    assert_eq!(output.status.code(), Some(6), "stdout: {}", stdout(&output));
    let value = assert_single_error_envelope(&output);
    assert_eq!(value["error"]["code"], "queue_delete_incomplete");
    assert_eq!(value["data"]["complete"], false);
    assert_eq!(value["data"]["unknown"], true);
    assert_eq!(value["data"]["remote_delete_attempted"], true);
}
