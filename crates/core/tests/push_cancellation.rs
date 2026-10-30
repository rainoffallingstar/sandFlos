use boox_core::models::{BucketConfig, OssSts, UploadAuthContext};
use boox_core::services::{
    HttpPushBackend, PushBackend, PushOutcome, PushService, ServiceError, ServiceErrorKind,
    ServiceResult,
};
use std::{
    cell::Cell,
    path::Path,
    path::PathBuf,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

static CANCELLED: AtomicBool = AtomicBool::new(true);
static CANCEL_ON_UPLOAD: AtomicBool = AtomicBool::new(false);
static CANCEL_ON_SAVE: AtomicBool = AtomicBool::new(false);

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

struct CancellingPush {
    flag: &'static AtomicBool,
    cancel_on_upload: bool,
    cancel_on_save: bool,
    cancel_after_save_success: bool,
    uploads: Cell<usize>,
    writes: Cell<usize>,
    saves: Cell<usize>,
}

impl CancellingPush {
    fn new(flag: &'static AtomicBool) -> Self {
        flag.store(false, Ordering::SeqCst);
        Self {
            flag,
            cancel_on_upload: false,
            cancel_on_save: false,
            cancel_after_save_success: false,
            uploads: Cell::new(0),
            writes: Cell::new(0),
            saves: Cell::new(0),
        }
    }
}

impl PushBackend for CancellingPush {
    fn cancellation_requested(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
    }

    fn auth_context(&mut self) -> ServiceResult<UploadAuthContext> {
        Ok(UploadAuthContext {
            bearer: "token".into(),
            uid: "user".into(),
            storage_limit: None,
            storage_used: None,
        })
    }

    fn default_bucket(&mut self) -> ServiceResult<(String, BucketConfig)> {
        Ok(("onyx-cloud".into(), BucketConfig::default()))
    }

    fn sts(&mut self, _: &UploadAuthContext) -> ServiceResult<OssSts> {
        Ok(OssSts {
            access_key_id: "id".into(),
            access_key_secret: "secret".into(),
            security_token: "sts".into(),
            expiration: "2099-01-01T00:00:00Z".into(),
        })
    }

    fn upload(
        &mut self,
        _: &BucketConfig,
        _: &OssSts,
        _: &str,
        _: &Path,
        _: &str,
    ) -> ServiceResult<()> {
        self.uploads.set(self.uploads.get() + 1);
        if self.cancel_on_upload {
            self.flag.store(true, Ordering::SeqCst);
            return Err(ServiceError::cancelled("upload"));
        }
        Ok(())
    }

    fn signed_url(
        &mut self,
        _: &UploadAuthContext,
        _: &BucketConfig,
        _: &str,
    ) -> ServiceResult<String> {
        Ok("https://signed.invalid/file".into())
    }

    fn write_message(
        &mut self,
        _: &UploadAuthContext,
        _: &Path,
        _: &str,
        _: u64,
        _: &str,
        _: &str,
        _: &str,
    ) -> ServiceResult<(String, String)> {
        self.writes.set(self.writes.get() + 1);
        Ok(("doc-1".into(), "1-rev".into()))
    }

    fn confirm_sync(&mut self, _: &UploadAuthContext, _: &str, _: &str) -> ServiceResult<()> {
        Ok(())
    }

    fn save_and_push(
        &mut self,
        _: &UploadAuthContext,
        _: &str,
        _: &str,
        _: &str,
        _: &str,
        _: &str,
        _: &str,
    ) -> ServiceResult<()> {
        self.saves.set(self.saves.get() + 1);
        if self.cancel_on_save {
            self.flag.store(true, Ordering::SeqCst);
            return Err(ServiceError::cancelled("save_and_push"));
        }
        if self.cancel_after_save_success {
            self.flag.store(true, Ordering::SeqCst);
        }
        Ok(())
    }
}

fn temp_file(name: &str, bytes: &[u8]) -> PathBuf {
    let root = std::env::temp_dir().join(format!("push-cancel-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join(name);
    std::fs::write(&path, bytes).unwrap();
    path
}

#[test]
fn cancelled_upload_stops_the_batch_before_any_message_write() {
    let first = temp_file("first.pdf", b"first");
    let second = temp_file("second.pdf", b"second");
    let mut backend = CancellingPush::new(&CANCEL_ON_UPLOAD);
    backend.cancel_on_upload = true;
    let mut service = PushService::new(backend);

    let report = service.push_files(&[first, second], |_| {});

    assert_eq!(report.exit_code(), 130);
    assert_eq!(report.accepted, 0);
    assert_eq!(
        report.files[0].stage,
        boox_core::services::PushStage::UploadOss
    );
    assert_eq!(report.files[0].outcome, PushOutcome::Failed);
    assert_eq!(
        report.files[0].error.as_ref().map(|error| error.kind),
        Some(ServiceErrorKind::Cancelled)
    );
    assert_eq!(
        report.files[1].error.as_ref().map(|error| error.kind),
        Some(ServiceErrorKind::Cancelled),
        "the remaining file must be cancelled without contacting the upload backend"
    );
    assert_eq!(service.backend().uploads.get(), 1);
    assert_eq!(service.backend().writes.get(), 0);
    assert_eq!(service.backend().saves.get(), 0);
}

#[test]
fn cancellation_after_message_creation_is_unknown_with_the_message_id() {
    let file = temp_file("book.pdf", b"book");
    let mut backend = CancellingPush::new(&CANCEL_ON_SAVE);
    backend.cancel_on_save = true;
    let mut service = PushService::new(backend);

    let report = service.push_files(std::slice::from_ref(&file), |_| {});

    assert_eq!(report.exit_code(), 130);
    assert_eq!(report.accepted, 0);
    assert_eq!(report.unknown, 1);
    assert_eq!(report.files[0].outcome, PushOutcome::Unknown);
    assert_eq!(
        report.files[0].error.as_ref().map(|error| error.kind),
        Some(ServiceErrorKind::Cancelled)
    );
    assert_eq!(report.files[0].message_id.as_deref(), Some("doc-1"));
    assert_eq!(service.backend().saves.get(), 1);
}

#[test]
fn cancellation_after_the_first_file_keeps_that_acceptance() {
    let first = temp_file("first.pdf", b"first");
    let second = temp_file("second.pdf", b"second");
    let mut backend = CancellingPush::new(&CANCEL_ON_SAVE);
    backend.cancel_after_save_success = true;
    let mut service = PushService::new(backend);

    let report = service.push_files(&[first, second], |_| {});

    assert_eq!(report.exit_code(), 130);
    assert_eq!(report.accepted, 1);
    assert_eq!(report.files[0].outcome, PushOutcome::Accepted);
    assert_eq!(report.files[0].message_id.as_deref(), Some("doc-1"));
    assert_eq!(report.files[1].outcome, PushOutcome::Failed);
    assert_eq!(
        report.files[1].error.as_ref().map(|error| error.kind),
        Some(ServiceErrorKind::Cancelled)
    );
    assert_eq!(service.backend().uploads.get(), 1);
    assert_eq!(service.backend().writes.get(), 1);
    assert_eq!(service.backend().saves.get(), 1);
}
