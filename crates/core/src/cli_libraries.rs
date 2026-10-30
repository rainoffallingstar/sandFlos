//! Library configuration isolated from the desktop application's global paths.
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};
#[derive(Debug)]
pub struct LibraryError {
    pub kind: crate::services::ServiceErrorKind,
    pub message: String,
}
impl LibraryError {
    fn network(message: impl Into<String>) -> Self {
        Self {
            kind: crate::services::ServiceErrorKind::Network,
            message: message.into(),
        }
    }
    fn input(message: impl Into<String>) -> Self {
        Self {
            kind: crate::services::ServiceErrorKind::Input,
            message: message.into(),
        }
    }
}
impl From<&str> for LibraryError {
    fn from(message: &str) -> Self {
        Self {
            kind: crate::services::ServiceErrorKind::LocalIo,
            message: message.into(),
        }
    }
}
impl From<String> for LibraryError {
    fn from(message: String) -> Self {
        Self {
            kind: crate::services::ServiceErrorKind::LocalIo,
            message,
        }
    }
}
impl std::fmt::Display for LibraryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for LibraryError {}
#[derive(Default, Serialize, Deserialize)]
struct LibraryConfig {
    #[serde(default)]
    calibre: Vec<PathBuf>,
    #[serde(default)]
    zotero: Option<PathBuf>,
    #[serde(default)]
    webdav: Option<WebDavConfig>,
}
// Hold an OS file lock across each read/modify/write transaction.
fn configuration_lock(dir: &Path) -> Result<fs::File, LibraryError> {
    fs::create_dir_all(dir).map_err(|_| "cannot create configuration directory")?;
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(dir.join("libraries.lock"))
        .map_err(|_| "cannot open library configuration lock")?;
    lock.try_lock().map_err(|_| "library configuration busy")?;
    Ok(lock)
}
fn load(dir: &Path) -> Result<LibraryConfig, LibraryError> {
    match fs::read(dir.join("libraries.json")) {
        Ok(bytes) => {
            serde_json::from_slice(&bytes).map_err(|_| "invalid library configuration".into())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(LibraryConfig::default()),
        Err(_) => Err("cannot read library configuration".into()),
    }
}
fn save(dir: &Path, config: &LibraryConfig) -> Result<(), LibraryError> {
    fs::create_dir_all(dir).map_err(|_| "cannot create configuration directory")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))
            .map_err(|_| "cannot secure config directory")?;
    }
    let destination = dir.join("libraries.json");
    let temporary = dir.join(format!("libraries-{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|_| "cannot create library configuration")?;
        f.write_all(&serde_json::to_vec(config).map_err(|_| "cannot serialize configuration")?)
            .map_err(|_| "cannot write configuration")?;
        f.sync_all().map_err(|_| "cannot sync configuration")?;
        crate::persistence::atomic_replace(&temporary, &destination)
            .map_err(|_| "cannot replace configuration")?;
        Ok(())
    })();
    let _ = fs::remove_file(temporary);
    result
}
pub fn configure_calibre(config_dir: &Path, libraries: Vec<PathBuf>) -> Result<(), LibraryError> {
    let _lock = configuration_lock(config_dir)?;
    if libraries.is_empty() {
        return Err("at least one library is required".into());
    }
    let mut normalized = vec![];
    for library in libraries {
        let library = fs::canonicalize(library).map_err(|_| "library does not exist")?;
        rusqlite::Connection::open_with_flags(
            library.join("metadata.db"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .map_err(|_| "cannot open Calibre database")?
        .prepare(
            "SELECT b.id,b.title,b.author_sort,b.path,b.last_modified,b.pubdate,
                    d.id,d.book,d.format,d.name,d.uncompressed_size
             FROM books b LEFT JOIN data d ON d.book=b.id LIMIT 0",
        )
        .map_err(|_| "not a Calibre database")?;
        if !normalized.contains(&library) {
            normalized.push(library);
        }
    }
    let mut config = load(config_dir)?;
    config.calibre = normalized;
    save(config_dir, &config)
}
pub fn calibre_directories(config_dir: &Path) -> Result<Vec<PathBuf>, LibraryError> {
    Ok(load(config_dir)?.calibre)
}
pub fn calibre_list(
    config_dir: &Path,
    limit: Option<usize>,
    offset: Option<usize>,
    search: Option<String>,
) -> Result<Vec<crate::models::CalibreBookSummary>, LibraryError> {
    let libraries = calibre_directories(config_dir)?;
    if libraries.is_empty() {
        return Err("configure a Calibre library first".into());
    }
    let mut books = Vec::new();
    for library in libraries {
        books.extend(
            crate::calibre::list_books_at(library.to_str().ok_or("invalid library path")?, None)
                .map_err(|_| "cannot query Calibre database")?,
        );
    }
    books.sort_by(|a, b| {
        b.date_modified
            .cmp(&a.date_modified)
            .then(a.library_dir.cmp(&b.library_dir))
            .then(a.book_id.cmp(&b.book_id))
    });
    if let Some(search) = search {
        let search = search.to_lowercase();
        books.retain(|b| {
            b.title.to_lowercase().contains(&search)
                || b.author_summary
                    .as_deref()
                    .unwrap_or("")
                    .to_lowercase()
                    .contains(&search)
        });
    }
    Ok(books
        .into_iter()
        .skip(offset.unwrap_or(0))
        .take(limit.unwrap_or(usize::MAX))
        .collect())
}
pub fn calibre_file(library: &Path, data_id: i64) -> Result<PathBuf, LibraryError> {
    let conn = rusqlite::Connection::open_with_flags(
        library.join("metadata.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .map_err(|_| "cannot open Calibre database")?;
    let (relative, name, format): (String, String, String) = conn
        .query_row(
            "SELECT b.path,d.name,d.format FROM data d JOIN books b ON b.id=d.book WHERE d.id=?1",
            [data_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .map_err(|_| "Calibre format not found")?;
    let file = library
        .join(relative)
        .join(format!("{}.{}", name, format.to_lowercase()));
    let root = fs::canonicalize(library).map_err(|_| "library does not exist")?;
    let file = fs::canonicalize(file).map_err(|_| "book file does not exist")?;
    if !file.starts_with(root) || !file.is_file() {
        return Err("book path outside library or not a file".into());
    }
    Ok(file)
}
pub fn configure_zotero(config_dir: &Path, data_dir: &Path) -> Result<(), LibraryError> {
    let _lock = configuration_lock(config_dir)?;
    let data = fs::canonicalize(data_dir).map_err(|_| "Zotero directory does not exist")?;
    rusqlite::Connection::open_with_flags(
        data.join("zotero.sqlite"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .map_err(|_| "cannot open Zotero database")?
    .prepare("SELECT itemID FROM itemAttachments LIMIT 0")
    .map_err(|_| "not a Zotero database")?;
    let mut config = load(config_dir)?;
    config.zotero = Some(data);
    save(config_dir, &config)
}
pub fn zotero_directory(config_dir: &Path) -> Result<Option<PathBuf>, LibraryError> {
    Ok(load(config_dir)?.zotero)
}
pub fn zotero_list(
    config_dir: &Path,
    limit: Option<usize>,
    offset: Option<usize>,
    search: Option<String>,
) -> Result<Vec<crate::models::ZoteroItemSummary>, LibraryError> {
    let data = zotero_directory(config_dir)?.ok_or("configure Zotero first")?;
    if limit == Some(0) {
        return Ok(vec![]);
    }
    crate::zotero::list_items_at(
        data.to_str().ok_or("invalid data directory")?,
        limit,
        offset,
        search,
        false,
    )
    .map_err(|_| "cannot query Zotero database".into())
}
pub struct ResolvedAttachment {
    pub path: PathBuf,
    cleanup: Option<PathBuf>,
}
impl Drop for ResolvedAttachment {
    fn drop(&mut self) {
        if let Some(dir) = &self.cleanup {
            let _ = fs::remove_dir_all(dir);
        }
    }
}
pub fn zotero_attachment(
    config_dir: &Path,
    id: i64,
    _timeout: std::time::Duration,
) -> Result<ResolvedAttachment, LibraryError> {
    let data = zotero_directory(config_dir)?.ok_or("configure Zotero first")?;
    let db = rusqlite::Connection::open_with_flags(
        data.join("zotero.sqlite"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .map_err(|_| "cannot open Zotero database")?;
    let (key,path,mode):(String,String,i64)=db.query_row("SELECT i.key,a.path,a.linkMode FROM itemAttachments a JOIN items i ON i.itemID=a.itemID WHERE a.itemID=?1",[id],|r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))).map_err(|_| "attachment not found")?;
    if mode != 0 {
        return Err("only stored attachments are supported".into());
    }
    let relative = path
        .strip_prefix("storage:")
        .ok_or("invalid stored attachment path")?;
    if key.is_empty() || !key.bytes().all(|b| b.is_ascii_alphanumeric()) || !safe_relative(relative)
    {
        return Err("unsafe attachment path".into());
    }
    let path = data.join("storage").join(&key).join(relative);
    if !path.exists() {
        return download_attachment(config_dir, &key, relative, _timeout);
    }
    let path = fs::canonicalize(path).map_err(|_| "cannot resolve local attachment")?;
    if !path.starts_with(fs::canonicalize(&data).map_err(|_| "invalid Zotero directory")?)
        || !path.is_file()
    {
        return Err("unsafe attachment path".into());
    }
    Ok(ResolvedAttachment {
        path,
        cleanup: None,
    })
}
fn safe_relative(value: &str) -> bool {
    !value.is_empty()
        && !value.contains('\\')
        && Path::new(value)
            .components()
            .all(|p| matches!(p, std::path::Component::Normal(_)))
}

pub fn configure_webdav(
    config_dir: &Path,
    url: &str,
    username: &str,
    password: &str,
) -> Result<(), LibraryError> {
    let kind = std::env::var("SANDFLOS_CREDENTIAL_STORE")
        .ok()
        .map(|v| crate::auth_service::CredentialStoreKind::parse(&v))
        .transpose()
        .map_err(|_| "invalid credential store")?
        .unwrap_or_else(crate::auth_service::CredentialStoreKind::platform_default);
    configure_webdav_with_store(config_dir, url, username, password, kind)
}
#[derive(Serialize, Deserialize)]
struct WebDavConfig {
    url: String,
    username: String,
    file_store: bool,
    #[serde(default)]
    password_base64: bool,
}
pub fn configure_webdav_with_store(
    config_dir: &Path,
    url: &str,
    username: &str,
    password: &str,
    kind: crate::auth_service::CredentialStoreKind,
) -> Result<(), LibraryError> {
    let _lock = configuration_lock(config_dir)?;
    let mut url = url::Url::parse(url).map_err(|_| LibraryError::input("invalid WebDAV URL"))?;
    if !matches!(url.scheme(), "https" | "http")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.host_str().is_none()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(LibraryError::input("invalid WebDAV URL"));
    }
    if url.scheme() == "http"
        && !matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"))
    {
        return Err(LibraryError::input("WebDAV requires HTTPS"));
    }
    if username.trim().is_empty() || password.is_empty() {
        return Err(LibraryError::input("WebDAV credentials are required"));
    }
    let path = url.path().trim_end_matches('/').to_string();
    if !path.ends_with("/zotero") {
        url.set_path(&format!("{path}/zotero"));
    }
    let mut config = load(config_dir)?;
    crate::auth_service::credential_store(
        kind,
        &crate::auth_service::AppPaths::new(config_dir.join("webdav")),
    )
    .save(&BASE64.encode(password.as_bytes()))
    .map_err(|_| "cannot persist WebDAV password with selected credential store")?;
    config.webdav = Some(WebDavConfig {
        url: url.to_string().trim_end_matches('/').into(),
        username: username.into(),
        file_store: matches!(kind, crate::auth_service::CredentialStoreKind::File),
        password_base64: true,
    });
    save(config_dir, &config)
}
fn download_attachment(
    config_dir: &Path,
    key: &str,
    preferred: &str,
    timeout: std::time::Duration,
) -> Result<ResolvedAttachment, LibraryError> {
    use std::io::Read;
    const MAX: u64 = 200 * 1024 * 1024;
    let config = load(config_dir)?;
    let config = config
        .webdav
        .ok_or("local attachment missing; configure WebDAV")?;
    let password = crate::auth_service::credential_store(
        if config.file_store {
            crate::auth_service::CredentialStoreKind::File
        } else {
            crate::auth_service::CredentialStoreKind::platform_default()
        },
        &crate::auth_service::AppPaths::new(config_dir.join("webdav")),
    )
    .load()
    .map_err(|_| "WebDAV credentials unavailable")?
    .ok_or("WebDAV password missing")?;
    // CredentialStore validates BOOX token syntax; encode opaque WebDAV
    // passwords so whitespace and Unicode survive its persistence boundary.
    // Older configurations stored the password directly and remain readable.
    let password = if config.password_base64 {
        String::from_utf8(
            BASE64
                .decode(password.token)
                .map_err(|_| "invalid WebDAV credential")?,
        )
        .map_err(|_| "invalid WebDAV credential")?
    } else {
        password.token
    };
    let client = reqwest::blocking::Client::builder()
        .timeout(timeout)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| "cannot build WebDAV client")?;
    let response = client
        .get(format!("{}/{key}.zip", config.url))
        .basic_auth(&config.username, Some(&password))
        .send()
        .map_err(|_| LibraryError::network("WebDAV network failure"))?;
    if !response.status().is_success() {
        let status = response.status().as_u16();
        return Err(LibraryError {
            kind: match status {
                401 | 403 => crate::services::ServiceErrorKind::Authentication,
                404 => crate::services::ServiceErrorKind::NotFound,
                _ => crate::services::ServiceErrorKind::Network,
            },
            message: if status == 404 {
                "WebDAV attachment not found".to_string()
            } else {
                format!("WebDAV HTTP {status}")
            },
        });
    }
    if response.content_length().is_some_and(|len| len > MAX) {
        return Err("WebDAV archive too large".into());
    }
    let mut bytes = Vec::new();
    response
        .take(MAX + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| LibraryError::network("cannot read WebDAV response"))?;
    if bytes.len() as u64 > MAX {
        return Err("WebDAV archive too large".into());
    }
    let mut archive =
        zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|_| "invalid WebDAV ZIP")?;
    let mut selected = None;
    let mut total = 0u64;
    for i in 0..archive.len() {
        let file = archive.by_index(i).map_err(|_| "invalid ZIP entry")?;
        if file.enclosed_name().is_none()
            || file.unix_mode().is_some_and(|m| m & 0o170000 == 0o120000)
        {
            return Err("unsafe ZIP path".into());
        }
        total = total.saturating_add(file.size());
        if total > MAX {
            return Err("expanded ZIP too large".into());
        }
        if !file.is_dir()
            && !file.name().split('/').any(|p| p.starts_with('.'))
            && file.name() == preferred
        {
            selected = Some(i);
        }
    }
    let index = selected.ok_or("ZIP does not contain the requested attachment")?;
    let directory =
        std::env::temp_dir().join(format!("sandflos-attachment-{}", uuid::Uuid::new_v4()));
    fs::create_dir(&directory).map_err(|_| "cannot create temporary directory")?;
    let mut resolved = ResolvedAttachment {
        path: directory.join("attachment"),
        cleanup: Some(directory.clone()),
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))
            .map_err(|_| "cannot secure temporary directory")?;
    }
    let mut entry = archive.by_index(index).map_err(|_| "invalid ZIP entry")?;
    let name = Path::new(entry.name())
        .file_name()
        .ok_or("invalid attachment filename")?;
    resolved.path = directory.join(name);
    let mut output = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&resolved.path)
        .map_err(|_| "cannot create attachment")?;
    let copied = std::io::copy(&mut entry.by_ref().take(MAX + 1), &mut output)
        .map_err(|_| "cannot extract attachment")?;
    if copied > MAX {
        return Err("attachment too large".into());
    }
    Ok(resolved)
}
