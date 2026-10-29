mod common;

use common::{command, json_stdout, TestDir};

#[test]
fn invalid_reading_dates_are_usage_errors_before_credentials() {
    let dir = TestDir::new();
    for date in ["2026-02-29", "2026-13-01", "2026-1-01", "not-a-date"] {
        let output = command(
            &["--json", "reading", "stats", "--date", date],
            &dir.config_dir(),
        )
        .output()
        .expect("run sandflos");
        assert_eq!(output.status.code(), Some(2), "date={date}: {output:?}");
        assert_eq!(json_stdout(&output)["error"]["code"], "invalid_date");
        assert!(!dir.config_dir().exists());
    }
}

#[test]
fn invalid_auth_origins_are_rejected_without_configuration_changes() {
    let dir = TestDir::new();
    for endpoint in [
        "http://127.0.0.1:1/path",
        "http://user:password@127.0.0.1:1",
        "http://127.0.0.1:1/?redirect=1",
    ] {
        let output = command(&["--json", "auth", "status"], &dir.config_dir())
            .env(common::TEST_AUTH_ENDPOINT_ENV, endpoint)
            .output()
            .expect("run sandflos");
        assert_eq!(output.status.code(), Some(2), "{output:?}");
        assert!(!dir.config_dir().exists());
    }
}

#[test]
fn invalid_api_or_orphaned_oss_endpoints_never_fall_back_to_production() {
    let dir = TestDir::new();
    for args in [
        vec!["--json", "devices", "list"],
        vec!["--json", "queue", "list"],
    ] {
        for (api, oss) in [
            (Some("https://example.invalid"), None),
            (Some("http://127.0.0.1:1/path"), None),
            (Some("http://user:password@127.0.0.1:1"), None),
            (None, Some("http://127.0.0.1:1")),
        ] {
            let mut child = command(&args, &dir.config_dir());
            child.env("SANDFLOS_TOKEN", "fixture-token");
            if let Some(api) = api {
                child.env(common::TEST_API_ENDPOINT_ENV, api);
            }
            if let Some(oss) = oss {
                child.env(common::TEST_OSS_ENDPOINT_ENV, oss);
            }
            let output = child.output().expect("run sandflos");
            assert_eq!(output.status.code(), Some(2), "{output:?}");
            assert_eq!(
                json_stdout(&output)["error"]["code"],
                "invalid_test_endpoint"
            );
            assert!(!dir.config_dir().exists());
        }
    }
}

#[cfg(unix)]
#[test]
fn non_unicode_test_endpoints_are_usage_errors() {
    use std::{ffi::OsString, os::unix::ffi::OsStringExt};
    let dir = TestDir::new();
    for (name, args) in [
        (
            common::TEST_AUTH_ENDPOINT_ENV,
            vec!["--json", "auth", "status"],
        ),
        (
            common::TEST_API_ENDPOINT_ENV,
            vec!["--json", "devices", "list"],
        ),
    ] {
        let output = command(&args, &dir.config_dir())
            .env(name, OsString::from_vec(vec![0xff]))
            .env("SANDFLOS_TOKEN", "fixture-token")
            .output()
            .expect("run sandflos");
        assert_eq!(output.status.code(), Some(2), "{output:?}");
    }
}
