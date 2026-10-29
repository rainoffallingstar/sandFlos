mod common;

use common::{
    command, json_stdout, run, run_with_stdin, run_with_stdin_with_auth, stderr, stdout,
    AuthServer, TestDir, TEST_AUTH_ENDPOINT_ENV,
};
use std::{fs, io::Write, process::Stdio};

#[test]
fn import_then_status_in_new_process_is_authorized_without_leaking_token() {
    let dir = TestDir::new();
    let config = dir.config_dir();
    let token = format!("fixture-{}", "credential-value");
    let server = AuthServer::accepting();

    let imported = run_with_stdin_with_auth(
        &["--json", "auth", "import", "--token-stdin"],
        &config,
        server.endpoint(),
        &token,
    );
    assert_eq!(
        imported.status.code(),
        Some(0),
        "stdout: {} stderr: {}",
        stdout(&imported),
        stderr(&imported)
    );
    let imported_json = json_stdout(&imported);
    assert_eq!(imported_json["ok"], true);
    assert_eq!(imported_json["data"]["authorized"], true);
    assert_eq!(imported_json["data"]["source"], "file");
    assert!(!stdout(&imported).contains(&token));

    let status = run(&["--json", "auth", "status"], &config);
    assert_eq!(status.status.code(), Some(0), "stderr: {}", stderr(&status));
    let status_json = json_stdout(&status);
    assert_eq!(status_json["data"]["authorized"], true);
    assert_eq!(status_json["data"]["source"], "file");
    assert!(!stdout(&status).contains(&token));

    let credential = fs::read_to_string(config.join("auth.json")).expect("credential file");
    assert!(credential.contains(&token));
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].starts_with("GET /api/1/users/me "));
    assert!(requests[0]
        .to_ascii_lowercase()
        .contains(&format!("authorization: bearer {token}")));

    let logout = run(&["--json", "auth", "logout"], &config);
    assert_eq!(logout.status.code(), Some(0), "stderr: {}", stderr(&logout));
    let logout_json = json_stdout(&logout);
    assert_eq!(logout_json["data"]["cleared"], true);
    assert_eq!(logout_json["data"]["environment_token_active"], false);

    let after = run(&["--json", "auth", "status"], &config);
    assert_eq!(after.status.code(), Some(0), "stderr: {}", stderr(&after));
    assert_eq!(json_stdout(&after)["data"]["authorized"], false);
}

#[test]
fn verify_without_credentials_requires_login_without_network() {
    let dir = TestDir::new();
    let output = run(&["--json", "auth", "status", "--verify"], &dir.config_dir());

    assert_eq!(output.status.code(), Some(3), "stderr: {}", stderr(&output));
    assert_eq!(stderr(&output), "");
    let value = json_stdout(&output);
    assert_eq!(value["ok"], false);
    assert_eq!(value["error"]["code"], "missing_credentials");
}

#[test]
fn config_directories_do_not_share_tokens() {
    let first = TestDir::new();
    let second = TestDir::new();
    let server = AuthServer::accepting();

    let imported = run_with_stdin_with_auth(
        &["auth", "import", "--token-stdin"],
        &first.config_dir(),
        server.endpoint(),
        "token-one",
    );
    assert_eq!(
        imported.status.code(),
        Some(0),
        "stderr: {}",
        stderr(&imported)
    );

    let first_status = run(&["--json", "auth", "status"], &first.config_dir());
    assert_eq!(json_stdout(&first_status)["data"]["authorized"], true);

    let second_status = run(&["--json", "auth", "status"], &second.config_dir());
    assert_eq!(second_status.status.code(), Some(0));
    assert_eq!(json_stdout(&second_status)["data"]["authorized"], false);
    assert!(!second.config_dir().join("auth.json").exists());
}

#[test]
fn empty_token_is_an_input_error_and_is_not_saved() {
    let dir = TestDir::new();
    let config = dir.config_dir();
    let output = run_with_stdin(
        &["--json", "auth", "import", "--token-stdin"],
        &config,
        "\n",
    );

    assert_eq!(output.status.code(), Some(2), "stderr: {}", stderr(&output));
    assert_eq!(stderr(&output), "");
    let value = json_stdout(&output);
    assert_eq!(value["ok"], false);
    assert_eq!(value["error"]["code"], "invalid_token");
    assert!(!config.join("auth.json").exists());
}

#[test]
fn logout_is_idempotent() {
    let dir = TestDir::new();
    let config = dir.config_dir();

    for expected in [false, false] {
        let output = run(&["--json", "auth", "logout"], &config);
        assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
        assert_eq!(json_stdout(&output)["data"]["cleared"], expected);
    }
}

#[test]
fn environment_token_takes_precedence_and_blocks_import() {
    let dir = TestDir::new();
    let mut status = command(&["--json", "auth", "status"], &dir.config_dir());
    status.env("SANDFLOS_TOKEN", "environment-token");
    let output = status.output().expect("run sandflos");
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    let value = json_stdout(&output);
    assert_eq!(value["data"]["authorized"], true);
    assert_eq!(value["data"]["source"], "environment");

    let mut import = command(
        &["--json", "auth", "import", "--token-stdin"],
        &dir.config_dir(),
    );
    import
        .env("SANDFLOS_TOKEN", "environment-token")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = import.spawn().expect("spawn import");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(b"file-token")
        .expect("write token");
    let output = child.wait_with_output().expect("wait for import");
    assert_eq!(output.status.code(), Some(2), "stderr: {}", stderr(&output));
    let value = json_stdout(&output);
    assert_eq!(value["error"]["code"], "environment_token_active");
    assert!(!dir.config_dir().join("auth.json").exists());
}

#[test]
fn qr_login_requires_a_terminal() {
    let dir = TestDir::new();
    let output = run(&["--json", "auth", "login", "--qr"], &dir.config_dir());

    assert_eq!(output.status.code(), Some(2), "stderr: {}", stderr(&output));
    assert_eq!(stderr(&output), "");
    let value = json_stdout(&output);
    assert_eq!(value["ok"], false);
    assert_eq!(value["error"]["code"], "interactive_terminal_required");
    assert!(!dir.config_dir().join("auth.json").exists());
}

#[test]
fn auth_requires_a_subcommand() {
    let dir = TestDir::new();
    let output = run(&["--json", "auth"], &dir.config_dir());

    assert_eq!(output.status.code(), Some(2));
    assert_eq!(json_stdout(&output)["error"]["code"], "usage");
}

#[test]
fn explicit_config_dir_takes_precedence_over_environment() {
    let environment = TestDir::new();
    let explicit = TestDir::new();
    let server = AuthServer::accepting();
    let imported = run_with_stdin_with_auth(
        &[
            "auth",
            "import",
            "--token-stdin",
            "--config-dir",
            explicit.config_dir().to_str().expect("utf8 path"),
        ],
        &environment.config_dir(),
        server.endpoint(),
        "explicit-token",
    );
    assert_eq!(
        imported.status.code(),
        Some(0),
        "stderr: {}",
        stderr(&imported)
    );

    let environment_status = run(&["--json", "auth", "status"], &environment.config_dir());
    assert_eq!(environment_status.status.code(), Some(0));
    assert_eq!(
        json_stdout(&environment_status)["data"]["authorized"],
        false
    );
    assert!(!environment.config_dir().join("auth.json").exists());
}

#[test]
fn corrupt_credentials_are_a_storage_error_and_are_not_leaked() {
    let dir = TestDir::new();
    let config = dir.config_dir();
    fs::create_dir_all(&config).expect("create config");
    fs::write(config.join("auth.json"), "{not json").expect("write corrupt credentials");

    let output = run(&["--json", "auth", "status"], &config);
    assert_eq!(output.status.code(), Some(5), "stderr: {}", stderr(&output));
    assert_eq!(stderr(&output), "");
    let value = json_stdout(&output);
    assert_eq!(value["error"]["code"], "invalid_credentials_file");
    assert!(!stdout(&output).contains("not json"));
}

#[test]
fn credential_write_failure_is_reported_without_creating_credentials() {
    let dir = TestDir::new();
    let config = dir.config_dir();
    fs::write(&config, "not a directory").expect("blocking config path");
    let server = AuthServer::accepting();

    let output = run_with_stdin_with_auth(
        &["--json", "auth", "import", "--token-stdin"],
        &config,
        server.endpoint(),
        "new-token",
    );
    assert_eq!(output.status.code(), Some(5), "stderr: {}", stderr(&output));
    assert_eq!(stderr(&output), "");
    let value = json_stdout(&output);
    assert_eq!(value["ok"], false);
    assert_eq!(value["error"]["code"], "credential_write_failed");
    assert!(!config.join("auth.json").exists());
}

#[test]
fn rejected_import_is_an_auth_error_and_is_not_saved() {
    let dir = TestDir::new();
    let config = dir.config_dir();
    let server = AuthServer::rejecting();

    let output = run_with_stdin_with_auth(
        &["--json", "auth", "import", "--token-stdin"],
        &config,
        server.endpoint(),
        "rejected-token",
    );

    assert_eq!(output.status.code(), Some(3), "stderr: {}", stderr(&output));
    assert_eq!(stderr(&output), "");
    let value = json_stdout(&output);
    assert_eq!(value["ok"], false);
    assert_eq!(value["error"]["code"], "credential_rejected");
    assert!(!stdout(&output).contains("server body must not be echoed"));
    assert!(!config.join("auth.json").exists());
}

#[test]
fn non_loopback_test_auth_endpoint_is_rejected() {
    let dir = TestDir::new();
    let mut command = command(&["--json", "auth", "status"], &dir.config_dir());
    command.env(TEST_AUTH_ENDPOINT_ENV, "https://example.com");
    let output = command.output().expect("run sandflos");

    assert_eq!(output.status.code(), Some(2), "stderr: {}", stderr(&output));
    assert_eq!(stderr(&output), "");
    let value = json_stdout(&output);
    assert_eq!(value["error"]["code"], "invalid_test_auth_endpoint");
    assert!(!dir.config_dir().exists());
}
