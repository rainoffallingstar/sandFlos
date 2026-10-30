mod common;
use common::*;
#[test]
fn migration_validates_credentials_preserves_source_and_refuses_overwrite() {
    let dir = TestDir::new();
    let config = dir.config_dir();
    let source = dir.path().join("auth_cache.json");
    let bytes = br#"{"token":"fixture-token","updated_ms":1}"#;
    std::fs::write(&source, bytes).unwrap();
    let server = AuthServer::accepting();
    let args = [
        "--json",
        "config",
        "migrate",
        "--from",
        source.to_str().unwrap(),
    ];
    let result = command_with_auth(&args, &config, server.endpoint())
        .output()
        .unwrap();
    assert_eq!(result.status.code(), Some(0), "{}", stdout(&result));
    assert_eq!(json_stdout(&result)["data"]["migrated"], true);
    assert_eq!(std::fs::read(&source).unwrap(), bytes);
    assert!(!stdout(&result).contains("fixture-token"));

    // The imported credential must remain usable by a later CLI process.
    let verified = command_with_auth(
        &["--json", "auth", "status", "--verify"],
        &config,
        server.endpoint(),
    )
    .output()
    .unwrap();
    assert_eq!(verified.status.code(), Some(0), "{}", stdout(&verified));
    assert_eq!(json_stdout(&verified)["data"]["authorized"], true);
    let requests = server.requests();
    assert!(requests
        .last()
        .is_some_and(|request| request.contains("Bearer fixture-token")));

    let second = command_with_auth(&args, &config, server.endpoint())
        .output()
        .unwrap();
    assert_eq!(second.status.code(), Some(2));
}

#[test]
fn corrupted_migration_source_fails_without_creating_target_and_can_be_retried() {
    let dir = TestDir::new();
    let config = dir.config_dir();
    let source = dir.path().join("auth_cache.json");
    std::fs::write(&source, br#"{"token":"fixture-token","updated_ms":1,"#).unwrap();
    let server = AuthServer::accepting();
    let args = [
        "--json",
        "config",
        "migrate",
        "--from",
        source.to_str().unwrap(),
    ];

    let rejected = command_with_auth(&args, &config, server.endpoint())
        .output()
        .unwrap();
    assert_eq!(rejected.status.code(), Some(5), "{}", stdout(&rejected));
    let value = json_stdout(&rejected);
    assert_eq!(value["error"]["code"], "invalid_migration_source");
    assert!(!stdout(&rejected).contains("fixture-token"));
    assert!(!config.join("auth.json").exists());
    assert!(server.requests().is_empty());

    std::fs::write(&source, br#"{"token":"fixture-token","updated_ms":2}"#).unwrap();
    let retried = command_with_auth(&args, &config, server.endpoint())
        .output()
        .unwrap();
    assert_eq!(retried.status.code(), Some(0), "{}", stdout(&retried));
    assert_eq!(json_stdout(&retried)["data"]["migrated"], true);
    assert_eq!(
        std::fs::read(&source).unwrap(),
        br#"{"token":"fixture-token","updated_ms":2}"#
    );

    // A separate process must be able to use the credential written by the
    // retry; the legacy source stays byte-for-byte unchanged.
    let verified = command_with_auth(
        &["--json", "auth", "status", "--verify"],
        &config,
        server.endpoint(),
    )
    .output()
    .unwrap();
    assert_eq!(verified.status.code(), Some(0), "{}", stdout(&verified));
    assert_eq!(json_stdout(&verified)["data"]["authorized"], true);
    assert!(server
        .requests()
        .last()
        .is_some_and(|request| request.contains("Bearer fixture-token")));
    assert_eq!(
        std::fs::read(&source).unwrap(),
        br#"{"token":"fixture-token","updated_ms":2}"#
    );
}

#[test]
fn migration_source_without_a_token_is_rejected_before_persisting() {
    let dir = TestDir::new();
    let config = dir.config_dir();
    let source = dir.path().join("auth_cache.json");
    let source_json = serde_json::json!({"updated_ms": 3});
    std::fs::write(&source, source_json.to_string()).unwrap();
    let server = AuthServer::accepting();

    let output = command_with_auth(
        &[
            "--json",
            "config",
            "migrate",
            "--from",
            source.to_str().unwrap(),
        ],
        &config,
        server.endpoint(),
    )
    .output()
    .unwrap();

    assert_eq!(output.status.code(), Some(2), "{}", stdout(&output));
    let value = json_stdout(&output);
    assert_eq!(value["error"]["code"], "migration_source_missing_token");
    assert!(!config.join("auth.json").exists());
    assert!(server.requests().is_empty());
}
