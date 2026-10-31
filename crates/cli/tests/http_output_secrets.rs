mod common;

use common::{command_with_api, json_stdout, stderr, stdout, ApiFault, ApiServer, TestDir};

#[test]
fn upload_progress_is_bounded_and_requires_an_oss_acknowledgment() {
    use boox_core::services::{HttpPushBackend, PushProgress, PushService};
    use std::time::Duration;
    for (fault, expected_upload_events) in [(ApiFault::UploadResponseLost, 0), (ApiFault::None, 1)]
    {
        let dir = TestDir::new();
        let server = ApiServer::start_with_fault(fault);
        let file = dir.path().join("progress.pdf");
        std::fs::write(&file, b"fixture-book").unwrap();
        let backend = HttpPushBackend::with_test_endpoints(
            Duration::from_secs(2),
            "fixture-token",
            server.endpoint(),
            server.endpoint(),
        )
        .unwrap();
        let mut uploaded = Vec::new();
        let mut read = Vec::new();
        let report = PushService::new(backend).push_files(&[file], |event| match event {
            PushProgress::Read {
                bytes, total_bytes, ..
            } => read.push((bytes, total_bytes)),
            PushProgress::Uploaded {
                bytes, total_bytes, ..
            } => uploaded.push((bytes, total_bytes)),
            _ => {}
        });
        assert_eq!(uploaded.len(), expected_upload_events);
        for (bytes, total) in uploaded {
            assert_eq!(total, 12);
            assert_eq!(bytes, 12);
            assert!(bytes <= total);
        }
        assert!(!read.is_empty(), "local file reads must be observable");
        assert!(read.windows(2).all(|pair| pair[0].0 < pair[1].0));
        for (bytes, total) in read {
            assert_eq!(total, 12);
            assert!(bytes <= total);
        }
        assert_eq!(report.accepted, expected_upload_events);
        let requests = server.requests();
        let uploads: Vec<_> = requests
            .iter()
            .filter(|r| r.method == "PUT" && r.path.starts_with("/uid-fixture/push/"))
            .collect();
        assert_eq!(uploads.len(), 1);
        assert_eq!(uploads[0].body, "fixture-book");
    }
}

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
