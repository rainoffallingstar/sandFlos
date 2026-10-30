//! HTTP transfer contracts at the public backend boundary (S3).
use boox_core::{
    models::{BucketConfig, OssSts},
    services::{HttpPushBackend, PushBackend, ServiceErrorKind},
};
use std::{fs, net::TcpListener, time::Duration};

#[test]
fn expired_storage_credentials_fail_before_opening_an_upload_connection() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let path = std::env::temp_dir().join(format!("expired-sts-{}.pdf", uuid::Uuid::new_v4()));
    fs::write(&path, b"fixture-book").unwrap();
    let mut backend = HttpPushBackend::with_test_endpoints(
        Duration::from_secs(1),
        "fixture-token",
        &endpoint,
        &endpoint,
    )
    .unwrap();
    let bucket = BucketConfig {
        bucket: Some("fixture".into()),
        ali_endpoint: Some("https://oss.invalid".into()),
        ..Default::default()
    };
    let sts = OssSts {
        access_key_id: "fixture-key".into(),
        access_key_secret: "fixture-secret".into(),
        security_token: "fixture-sts-token".into(),
        expiration: "2000-01-01T00:00:00Z".into(),
    };
    let result = backend.upload(&bucket, &sts, "fixture/book.pdf", &path, "application/pdf");
    fs::remove_file(path).unwrap();
    let error = result.unwrap_err();
    assert_eq!(error.kind, ServiceErrorKind::Protocol);
    assert_eq!(error.code, "sts_expired");
    assert!(!error.message.contains("fixture-secret"));
    assert!(
        matches!(listener.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock)
    );
}
