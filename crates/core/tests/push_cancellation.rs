use boox_core::services::{HttpPushBackend, PushService};
use std::{sync::atomic::AtomicBool, time::Duration};
static CANCELLED: AtomicBool = AtomicBool::new(true);
#[test]
fn cancellation_stops_before_authentication_or_upload() {
    let path = std::env::temp_dir().join(format!("cancel-{}.pdf", uuid::Uuid::new_v4()));
    std::fs::write(&path, b"pdf").unwrap();
    let backend = HttpPushBackend::new(Duration::from_secs(1), "fake-token")
        .unwrap()
        .with_cancel_flag(&CANCELLED);
    let report = PushService::new(backend).push_files(std::slice::from_ref(&path), |_| {});
    assert_eq!(report.exit_code(), 130);
    assert_eq!(report.accepted, 0);
    std::fs::remove_file(path).unwrap();
}
