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
    let second = command_with_auth(&args, &config, server.endpoint())
        .output()
        .unwrap();
    assert_eq!(second.status.code(), Some(2));
    assert!(!stdout(&result).contains("fixture-token"));
}
