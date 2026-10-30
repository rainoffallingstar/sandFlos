mod common;

use common::{json_stdout, run, run_with_stdin_with_auth, stdout, AuthServer, TestDir};
use std::fs::{self, OpenOptions};

#[test]
fn failed_replacement_during_busy_store_preserves_the_usable_old_credential() {
    let dir = TestDir::new();
    let config = dir.config_dir();
    let server = AuthServer::accepting();
    let args = ["--json", "auth", "import", "--token-stdin"];
    let first = run_with_stdin_with_auth(&args, &config, server.endpoint(), "old-fixture-token");
    assert_eq!(first.status.code(), Some(0), "{}", stdout(&first));
    let before = fs::read(config.join("auth.json")).unwrap();

    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .open(config.join("auth.lock"))
        .unwrap();
    lock.try_lock().unwrap();
    let rejected = run_with_stdin_with_auth(&args, &config, server.endpoint(), "new-fixture-token");
    assert_eq!(rejected.status.code(), Some(5), "{}", stdout(&rejected));
    assert_eq!(
        json_stdout(&rejected)["error"]["code"],
        "credential_store_busy"
    );
    assert!(!stdout(&rejected).contains("fixture-token"));
    assert_eq!(fs::read(config.join("auth.json")).unwrap(), before);
    drop(lock);

    let verified = common::command_with_auth(
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
        .unwrap()
        .contains("Bearer old-fixture-token"));
}

#[test]
fn logout_reports_a_delete_failure_and_can_be_retried() {
    let dir = TestDir::new();
    let config = dir.config_dir();
    let blocked = config.join("auth.json");
    fs::create_dir_all(&blocked).unwrap();
    fs::write(blocked.join("keep"), b"sentinel").unwrap();
    let rejected = run(&["--json", "auth", "logout"], &config);
    assert_eq!(rejected.status.code(), Some(5), "{}", stdout(&rejected));
    assert_eq!(
        json_stdout(&rejected)["error"]["code"],
        "credential_delete_failed"
    );
    assert_eq!(fs::read(blocked.join("keep")).unwrap(), b"sentinel");

    fs::remove_dir_all(blocked).unwrap();
    let retried = run(&["--json", "auth", "logout"], &config);
    assert_eq!(retried.status.code(), Some(0), "{}", stdout(&retried));
    assert_eq!(json_stdout(&retried)["data"]["cleared"], false);
}

#[cfg(windows)]
#[test]
fn atomic_replace_failure_on_windows_preserves_old_credentials_and_can_retry() {
    use std::os::windows::fs::OpenOptionsExt;
    let dir = TestDir::new();
    let config = dir.config_dir();
    let server = AuthServer::accepting();
    let args = ["--json", "auth", "import", "--token-stdin"];
    let first = run_with_stdin_with_auth(&args, &config, server.endpoint(), "old-fixture-token");
    assert_eq!(first.status.code(), Some(0), "{}", stdout(&first));
    let before = fs::read(config.join("auth.json")).unwrap();
    // A real OS handle prevents MoveFileExW from replacing the destination.
    let handle = OpenOptions::new()
        .read(true)
        .share_mode(1) // FILE_SHARE_READ, deliberately no FILE_SHARE_DELETE
        .open(config.join("auth.json"))
        .unwrap();
    let failed = run_with_stdin_with_auth(&args, &config, server.endpoint(), "new-fixture-token");
    assert_eq!(failed.status.code(), Some(5), "{}", stdout(&failed));
    assert_eq!(
        json_stdout(&failed)["error"]["code"],
        "credential_write_failed"
    );
    assert!(!stdout(&failed).contains("fixture-token"));
    assert_eq!(fs::read(config.join("auth.json")).unwrap(), before);
    drop(handle);
    let verified = common::command_with_auth(
        &["--json", "auth", "status", "--verify"],
        &config,
        server.endpoint(),
    )
    .output()
    .unwrap();
    assert_eq!(verified.status.code(), Some(0), "{}", stdout(&verified));
    assert!(server
        .requests()
        .last()
        .unwrap()
        .contains("Bearer old-fixture-token"));
    let retry = run_with_stdin_with_auth(&args, &config, server.endpoint(), "new-fixture-token");
    assert_eq!(retry.status.code(), Some(0), "{}", stdout(&retry));
}
