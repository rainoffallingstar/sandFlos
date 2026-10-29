use boox_core::auth_service::{AppPaths, CredentialStore, FileCredentialStore};
use std::fs;

#[test]
fn concurrent_writer_cannot_replace_or_clear_credentials() {
    let dir = std::env::temp_dir().join(format!("sandflos-lock-{}", uuid::Uuid::new_v4()));
    let store = FileCredentialStore::new(&AppPaths::new(&dir));
    store.save("old-credential").unwrap();
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(dir.join("auth.lock"))
        .unwrap();
    lock.lock().unwrap();
    assert_eq!(
        store.save("new-credential").unwrap_err().code,
        "credential_store_busy"
    );
    assert_eq!(store.clear().unwrap_err().code, "credential_store_busy");
    assert_eq!(store.load().unwrap().unwrap().token, "old-credential");
    drop(lock);
    store.save("new-credential").unwrap();
    assert_eq!(store.load().unwrap().unwrap().token, "new-credential");
    store.clear().unwrap();
    fs::remove_dir_all(dir).unwrap();
}
