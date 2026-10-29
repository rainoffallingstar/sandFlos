mod common;

use common::{json_stdout, run, stderr, TestDir};

#[test]
fn push_requires_a_file_before_any_network_work() {
    let dir = TestDir::new();
    let output = run(&["--json", "push"], &dir.config_dir());

    assert_eq!(output.status.code(), Some(2), "stderr: {}", stderr(&output));
    assert_eq!(stderr(&output), "");
    let value = json_stdout(&output);
    assert_eq!(value["ok"], false);
    assert_eq!(value["error"]["code"], "usage");
}

#[test]
fn push_rejects_unknown_options_before_upload() {
    let dir = TestDir::new();
    let output = run(
        &["--json", "push", "--watch", "book.pdf"],
        &dir.config_dir(),
    );

    assert_eq!(output.status.code(), Some(2), "stderr: {}", stderr(&output));
    assert_eq!(json_stdout(&output)["error"]["code"], "usage");
}

#[test]
fn queue_delete_without_yes_is_rejected_before_network() {
    let dir = TestDir::new();
    let output = run(&["--json", "queue", "delete", "doc-123"], &dir.config_dir());

    assert_eq!(output.status.code(), Some(2), "stderr: {}", stderr(&output));
    let value = json_stdout(&output);
    assert_eq!(value["ok"], false);
    assert_eq!(value["error"]["code"], "confirmation_required");
}

#[test]
fn queue_resend_requires_exactly_one_id() {
    let dir = TestDir::new();
    let output = run(&["--json", "queue", "resend"], &dir.config_dir());

    assert_eq!(output.status.code(), Some(2));
    assert_eq!(json_stdout(&output)["error"]["code"], "usage");
}

#[test]
fn queue_list_rejects_zero_limit_without_network() {
    let dir = TestDir::new();
    let output = run(
        &["--json", "queue", "list", "--limit", "0"],
        &dir.config_dir(),
    );

    assert_eq!(output.status.code(), Some(2));
    assert_eq!(json_stdout(&output)["error"]["code"], "usage");
}

#[test]
fn global_help_after_command_still_prints_help() {
    let dir = TestDir::new();
    let output = run(&["push", "--help"], &dir.config_dir());

    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("Usage:"), "stdout: {text}");
    assert!(text.contains("push"), "stdout: {text}");
}

#[test]
fn files_after_double_dash_are_not_parsed_as_options() {
    let dir = TestDir::new();
    let output = run(
        &["--json", "push", "--", "--not-an-option.pdf"],
        &dir.config_dir(),
    );

    // The path is accepted by the parser. It then fails local validation because
    // the file does not exist; it must not fail as an unknown option.
    assert_eq!(output.status.code(), Some(5), "stderr: {}", stderr(&output));
    let value = json_stdout(&output);
    assert_eq!(value["ok"], false);
    assert_eq!(value["data"]["failed"], 1);
    assert_eq!(value["error"]["code"], "push_incomplete");
}

#[test]
fn queue_requires_a_subcommand() {
    let dir = TestDir::new();
    let output = run(&["--json", "queue"], &dir.config_dir());

    assert_eq!(output.status.code(), Some(2));
    assert_eq!(json_stdout(&output)["error"]["code"], "usage");
}
