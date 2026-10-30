mod common;

use common::{command_with_api, json_stdout, stdout, ApiServer, TestDir};
use serde_json::Value;

#[test]
fn missing_reading_data_stays_null_instead_of_becoming_zero() {
    let dir = TestDir::new();
    let server = ApiServer::start_with_fault(common::ApiFault::MissingReadingData);
    let output = command_with_api(
        &["--json", "reading", "stats", "--date", "2026-10-30"],
        &dir.config_dir(),
        server.endpoint(),
    )
    .env("SANDFLOS_TOKEN", "fixture-token")
    .output()
    .unwrap();
    assert_eq!(output.status.code(), Some(0), "{}", stdout(&output));
    let value = json_stdout(&output);
    for field in ["reading_info", "week", "day", "timezone"] {
        assert_eq!(value["data"].get(field), Some(&Value::Null), "{field}");
    }
}

#[test]
fn requested_calendar_date_survives_year_boundary_and_host_timezone() {
    for (date, timezone) in [
        ("2026-12-31", "Pacific/Kiritimati"),
        ("2027-01-01", "America/Adak"),
    ] {
        let dir = TestDir::new();
        let server = ApiServer::start();
        let output = command_with_api(
            &["--json", "reading", "stats", "--date", date],
            &dir.config_dir(),
            server.endpoint(),
        )
        .env("SANDFLOS_TOKEN", "fixture-token")
        .env("TZ", timezone)
        .output()
        .unwrap();
        assert_eq!(output.status.code(), Some(0), "{}", stdout(&output));
        let value = json_stdout(&output);
        assert_eq!(value["data"]["date"], date);
        // Unknown server timezone must be explicit, not presented as local/UTC.
        assert_eq!(value["data"].get("timezone"), Some(&Value::Null));
        let paths: Vec<_> = server.requests().into_iter().map(|r| r.path).collect();
        assert!(paths.contains(&format!(
            "/api/1/statistics/readTimeInfo?date={date}&range=week"
        )));
        assert!(paths.contains(&format!(
            "/api/1/statistics/dayRead?startTime={date}&endTime={date}"
        )));
    }
}
