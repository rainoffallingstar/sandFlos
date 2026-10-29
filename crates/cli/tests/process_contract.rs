mod common;

use common::{command, json_stdout, run, stderr, stdout, TestDir};

#[test]
fn help_exits_without_creating_config_or_network() {
    let dir = TestDir::new();
    let output = run(&["--help"], &dir.config_dir());

    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    let text = stdout(&output);
    assert!(text.contains("Usage:"), "stdout: {text}");
    assert!(text.contains("sandflos"), "stdout: {text}");
    assert!(
        !dir.config_dir().exists(),
        "help created configuration at {}",
        dir.config_dir().display()
    );
}

#[test]
fn version_is_available_without_creating_config() {
    let dir = TestDir::new();
    let output = run(&["--version"], &dir.config_dir());

    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    let text = stdout(&output);
    assert!(text.contains("sandflos"), "stdout: {text}");
    assert!(
        !dir.config_dir().exists(),
        "version created configuration at {}",
        dir.config_dir().display()
    );
}

#[test]
fn missing_command_is_a_usage_error() {
    let dir = TestDir::new();
    let output = run(&[], &dir.config_dir());

    assert_eq!(output.status.code(), Some(2));
    assert!(
        stderr(&output).contains("Usage:"),
        "stderr: {}",
        stderr(&output)
    );
}

#[test]
fn json_parse_error_is_one_stdout_envelope() {
    let dir = TestDir::new();
    let output = run(&["--json", "no-such-command"], &dir.config_dir());

    assert_eq!(output.status.code(), Some(2));
    assert_eq!(stderr(&output), "");
    let text = stdout(&output);
    assert_eq!(text.lines().count(), 1, "stdout: {text}");
    let value: serde_json::Value = serde_json::from_str(&text).expect("valid JSON envelope");
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["ok"], false);
    assert_eq!(value["error"]["code"], "usage");
    assert_eq!(value["data"], serde_json::Value::Null);
}

#[test]
fn diagnostics_reports_redacted_config_without_credentials() {
    let dir = TestDir::new();
    let home = dir.path().join("home");
    std::fs::create_dir_all(&home).expect("create fixture home");
    let config = home.join("config");
    let mut diagnostics = command(&["--json", "diagnostics"], &config);
    diagnostics
        .env("HOME", &home)
        .env("SANDFLOS_TOKEN", "secret-environment-token");
    let output = diagnostics.output().expect("run diagnostics");

    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    assert!(!stdout(&output).contains("secret-environment-token"));
    let value = json_stdout(&output);
    assert_eq!(value["ok"], true);
    assert_eq!(value["data"]["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(value["data"]["auth"]["environment_token_active"], true);
    assert_eq!(value["data"]["credential_store"], "file");
    // The child process runs with the fixture HOME, so diagnostics must
    // report the path relative to that home rather than exposing the full
    // temporary directory.
    assert_eq!(
        value["data"]["config_dir"]["path"],
        format!("~{}config", std::path::MAIN_SEPARATOR)
    );
}

#[test]
fn diagnostics_rejects_arguments_and_does_not_need_a_tty() {
    let dir = TestDir::new();
    let output = run(&["--json", "diagnostics", "extra"], &dir.config_dir());

    assert_eq!(output.status.code(), Some(2));
    assert_eq!(json_stdout(&output)["error"]["code"], "usage");
}

#[test]
fn verbose_errors_add_a_stderr_diagnostic_line() {
    let dir = TestDir::new();
    let output = run(&["--json", "--verbose", "push"], &dir.config_dir());

    assert_eq!(output.status.code(), Some(2));
    let value = json_stdout(&output);
    assert_eq!(value["error"]["code"], "usage");
    let diagnostic = stderr(&output);
    assert!(diagnostic.contains("exit=2"), "stderr: {diagnostic}");
}
