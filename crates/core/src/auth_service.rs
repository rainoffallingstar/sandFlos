use crate::api::{
    check_qr_login_at, create_client, create_client_with_base_url, create_qr_login_at,
    fetch_user_me_at,
};
use crate::util::{json_field_to_string, normalize_optional, unix_ms_now};
use serde::{Deserialize, Serialize};
use std::{
    fmt,
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

const CREDENTIAL_FILE: &str = "auth.json";
const CREDENTIAL_SCHEMA_VERSION: u32 = 1;
const MAX_TOKEN_LEN: usize = 16 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthErrorKind {
    Input,
    Authentication,
    Storage,
    Network,
    Timeout,
    Cancelled,
    Unsupported,
}

impl AuthErrorKind {
    pub fn exit_code(self) -> i32 {
        match self {
            Self::Input => 2,
            Self::Authentication => 3,
            Self::Storage => 5,
            Self::Network | Self::Timeout => 4,
            Self::Cancelled => 130,
            Self::Unsupported => 5,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AuthError {
    pub kind: AuthErrorKind,
    pub code: &'static str,
    pub message: String,
}

impl AuthError {
    pub fn new(kind: AuthErrorKind, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            kind,
            code,
            message: message.into(),
        }
    }

    pub fn input(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(AuthErrorKind::Input, code, message)
    }

    pub fn authentication(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(AuthErrorKind::Authentication, code, message)
    }

    pub fn storage(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(AuthErrorKind::Storage, code, message)
    }

    pub fn network(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(AuthErrorKind::Network, code, message)
    }

    pub fn timeout(message: impl Into<String>) -> Self {
        Self::new(AuthErrorKind::Timeout, "timeout", message)
    }

    pub fn cancelled() -> Self {
        Self::new(AuthErrorKind::Cancelled, "cancelled", "操作已取消")
    }
}

impl fmt::Display for AuthError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for AuthError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthSource {
    None,
    Environment,
    File,
    Keyring,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialStoreKind {
    File,
    Keyring,
    Unavailable,
}

impl CredentialStoreKind {
    pub fn parse(value: &str) -> Result<Self, AuthError> {
        match value.trim().to_ascii_lowercase().as_str() {
            "file" => Ok(Self::File),
            "keyring" | "platform" => Ok(Self::Keyring),
            _ => Err(AuthError::input(
                "invalid_credential_store",
                "凭据存储必须是 file 或 keyring",
            )),
        }
    }

    pub fn platform_default() -> Self {
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        {
            Self::Keyring
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            Self::Unavailable
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AuthStatus {
    pub authorized: bool,
    pub source: AuthSource,
    pub updated_ms: Option<u128>,
}

impl AuthStatus {
    fn missing() -> Self {
        Self {
            authorized: false,
            source: AuthSource::None,
            updated_ms: None,
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct ResolvedToken {
    token: String,
    pub source: AuthSource,
    pub updated_ms: Option<u128>,
}

impl ResolvedToken {
    pub fn expose(&self) -> &str {
        &self.token
    }

    fn status(&self) -> AuthStatus {
        AuthStatus {
            authorized: true,
            source: self.source,
            updated_ms: self.updated_ms,
        }
    }
}

impl fmt::Debug for ResolvedToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ResolvedToken")
            .field("token", &"<redacted>")
            .field("source", &self.source)
            .field("updated_ms", &self.updated_ms)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LogoutOutcome {
    pub cleared: bool,
    pub environment_token_active: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AuthIdentity {
    pub uid: String,
    pub nickname: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct QrLoginChallenge {
    pub qrcode_id: String,
    pub qrcode_data: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QrLoginPoll {
    Pending,
    Confirmed { token: String },
    Expired,
}

pub trait AuthClient: Send + Sync {
    fn verify_token(&self, token: &str) -> Result<AuthIdentity, AuthError>;
    fn create_qr_login(&self) -> Result<QrLoginChallenge, AuthError>;
    fn check_qr_login(&self, qrcode_id: &str) -> Result<QrLoginPoll, AuthError>;
}

#[derive(Debug, Clone)]
pub struct HttpAuthClient {
    timeout: Duration,
    base_url: String,
}

const DEFAULT_BASE_URL: &str = "https://send2boox.com";

impl Default for HttpAuthClient {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(20),
            base_url: DEFAULT_BASE_URL.to_string(),
        }
    }
}

impl HttpAuthClient {
    pub fn new(timeout: Duration) -> Self {
        Self {
            timeout,
            base_url: DEFAULT_BASE_URL.to_string(),
        }
    }

    pub fn with_base_url(
        timeout: Duration,
        base_url: impl Into<String>,
    ) -> Result<Self, AuthError> {
        let base_url = base_url.into();
        create_client_with_base_url(timeout.as_secs().max(1), &base_url)
            .map_err(|_| AuthError::input("invalid_endpoint", "认证服务地址无效"))?;
        Ok(Self { timeout, base_url })
    }
}

fn api_error(error: String) -> AuthError {
    if let Some(rest) = error.strip_prefix("HTTP ") {
        let status = rest
            .split_whitespace()
            .next()
            .and_then(|value| value.parse::<u16>().ok());
        return match status {
            Some(401) | Some(403) => {
                AuthError::authentication("credential_rejected", "认证凭据无效或已过期")
            }
            Some(code) => AuthError::network("http_error", format!("认证服务返回 HTTP {code}")),
            None => AuthError::network("protocol_error", "认证服务返回了无法识别的响应"),
        };
    }
    let lower = error.to_ascii_lowercase();
    if lower.contains("timed out") || lower.contains("timeout") {
        return AuthError::timeout("认证服务请求超时");
    }
    AuthError::network("network_error", "无法完成认证服务请求")
}

impl AuthClient for HttpAuthClient {
    fn verify_token(&self, token: &str) -> Result<AuthIdentity, AuthError> {
        let client = create_client(self.timeout.as_secs().max(1))
            .map_err(|_| AuthError::network("client_error", "无法创建认证 HTTP 客户端"))?;
        let user = fetch_user_me_at(&client, &self.base_url, token).map_err(api_error)?;
        let uid = json_field_to_string(&user, "uid")
            .ok_or_else(|| AuthError::network("protocol_error", "用户信息缺少 uid"))?;
        let nickname =
            json_field_to_string(&user, "nickname").or_else(|| json_field_to_string(&user, "name"));
        Ok(AuthIdentity { uid, nickname })
    }

    fn create_qr_login(&self) -> Result<QrLoginChallenge, AuthError> {
        let client = create_client(self.timeout.as_secs().max(1))
            .map_err(|_| AuthError::network("client_error", "无法创建认证 HTTP 客户端"))?;
        let response = create_qr_login_at(&client, &self.base_url).map_err(api_error)?;
        if response.qrcode_id.trim().is_empty() || response.qrcode_data.trim().is_empty() {
            return Err(AuthError::network(
                "protocol_error",
                "二维码接口未返回可用二维码",
            ));
        }
        Ok(QrLoginChallenge {
            qrcode_id: response.qrcode_id,
            qrcode_data: response.qrcode_data,
        })
    }

    fn check_qr_login(&self, qrcode_id: &str) -> Result<QrLoginPoll, AuthError> {
        let client = create_client(self.timeout.as_secs().max(1))
            .map_err(|_| AuthError::network("client_error", "无法创建认证 HTTP 客户端"))?;
        let response = check_qr_login_at(&client, &self.base_url, qrcode_id).map_err(api_error)?;
        match response.status {
            1 => {
                let token = normalize_optional(response.user_info.and_then(|info| info.token))
                    .ok_or_else(|| {
                        AuthError::authentication(
                            "qr_token_missing",
                            "扫码登录成功，但服务端未返回 token",
                        )
                    })?;
                validate_token(&token)?;
                Ok(QrLoginPoll::Confirmed { token })
            }
            -1 => Ok(QrLoginPoll::Expired),
            _ => Ok(QrLoginPoll::Pending),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QrLoginOptions {
    pub poll_interval: Duration,
    pub timeout: Duration,
}

impl Default for QrLoginOptions {
    fn default() -> Self {
        Self {
            poll_interval: Duration::from_millis(1500),
            timeout: Duration::from_secs(5 * 60),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppPaths {
    config_dir: PathBuf,
}

impl AppPaths {
    pub fn new(config_dir: impl Into<PathBuf>) -> Self {
        Self {
            config_dir: config_dir.into(),
        }
    }

    pub fn resolve(
        explicit_dir: Option<PathBuf>,
        environment_dir: Option<PathBuf>,
    ) -> Result<Self, AuthError> {
        if let Some(dir) = explicit_dir {
            return Ok(Self::new(dir));
        }
        if let Some(dir) = environment_dir {
            return Ok(Self::new(dir));
        }
        default_config_dir().map(Self::new).ok_or_else(|| {
            AuthError::storage("config_dir_unavailable", "无法定位默认 CLI 配置目录")
        })
    }

    pub fn from_process_env(explicit_dir: Option<PathBuf>) -> Result<Self, AuthError> {
        let environment_dir = std::env::var_os("SANDFLOS_CONFIG_DIR")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from);
        Self::resolve(explicit_dir, environment_dir)
    }

    pub fn config_dir(&self) -> &Path {
        &self.config_dir
    }

    pub fn credential_path(&self) -> PathBuf {
        self.config_dir.join(CREDENTIAL_FILE)
    }
}

pub fn default_config_dir() -> Option<PathBuf> {
    dirs::data_dir().map(|base| base.join("sandflos-cli"))
}

fn validate_token(raw: &str) -> Result<String, AuthError> {
    let token = raw.trim();
    if token.is_empty() {
        return Err(AuthError::input("invalid_token", "token 不能为空"));
    }
    if token.len() > MAX_TOKEN_LEN {
        return Err(AuthError::input("invalid_token", "token 长度超出限制"));
    }
    if token.chars().any(char::is_whitespace) || token.chars().any(char::is_control) {
        return Err(AuthError::input(
            "invalid_token",
            "token 不能包含空白或控制字符",
        ));
    }
    Ok(token.to_string())
}

#[derive(Debug, Serialize, Deserialize)]
struct PersistedCredential {
    schema_version: u32,
    token: String,
    updated_ms: u128,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredCredential {
    pub token: String,
    pub updated_ms: u128,
}

pub trait CredentialStore: Send + Sync {
    fn load(&self) -> Result<Option<StoredCredential>, AuthError>;
    fn save(&self, token: &str) -> Result<u128, AuthError>;
    fn clear(&self) -> Result<bool, AuthError>;
    fn can_persist(&self) -> bool;

    fn source(&self) -> AuthSource;
}

pub struct FileCredentialStore {
    path: PathBuf,
}

impl FileCredentialStore {
    pub fn new(paths: &AppPaths) -> Self {
        Self {
            path: paths.credential_path(),
        }
    }
}

impl CredentialStore for FileCredentialStore {
    fn load(&self) -> Result<Option<StoredCredential>, AuthError> {
        let mut file = match File::open(&self.path) {
            Ok(file) => file,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(err) => {
                return Err(AuthError::storage(
                    "credential_read_failed",
                    format!("无法读取认证凭据: {err}"),
                ))
            }
        };
        let mut text = String::new();
        file.read_to_string(&mut text).map_err(|err| {
            AuthError::storage("credential_read_failed", format!("无法读取认证凭据: {err}"))
        })?;
        let credential: PersistedCredential = serde_json::from_str(&text)
            .map_err(|_| AuthError::storage("invalid_credentials_file", "认证凭据文件格式无效"))?;
        if credential.schema_version != CREDENTIAL_SCHEMA_VERSION {
            return Err(AuthError::storage(
                "unsupported_credentials_version",
                "认证凭据文件版本不受支持",
            ));
        }
        let token = validate_token(&credential.token).map_err(|_| {
            AuthError::storage("invalid_credentials_file", "认证凭据文件中的 token 无效")
        })?;
        Ok(Some(StoredCredential {
            token,
            updated_ms: credential.updated_ms,
        }))
    }

    fn save(&self, token: &str) -> Result<u128, AuthError> {
        let config_dir = self
            .path
            .parent()
            .ok_or_else(|| AuthError::storage("credential_write_failed", "凭据路径缺少配置目录"))?;
        if let Err(err) = fs::create_dir_all(config_dir) {
            return Err(AuthError::storage(
                "credential_write_failed",
                format!("无法创建配置目录: {err}"),
            ));
        }
        set_private_dir_permissions(config_dir)?;
        let _lock = credential_lock(config_dir)?;

        let updated_ms = unix_ms_now();
        let payload = PersistedCredential {
            schema_version: CREDENTIAL_SCHEMA_VERSION,
            token: token.to_string(),
            updated_ms,
        };
        let text = serde_json::to_vec(&payload)
            .map_err(|_| AuthError::storage("credential_write_failed", "无法序列化认证凭据"))?;
        let temp_path = config_dir.join(format!(
            ".{CREDENTIAL_FILE}.tmp-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));

        let write_result = write_private_temp_file(&temp_path, &text)
            .and_then(|()| replace_file(&temp_path, &self.path));
        if let Err(err) = write_result {
            let _ = fs::remove_file(&temp_path);
            return Err(AuthError::storage(
                "credential_write_failed",
                format!("无法保存认证凭据: {err}"),
            ));
        }
        Ok(updated_ms)
    }

    fn clear(&self) -> Result<bool, AuthError> {
        let parent = self.path.parent().ok_or_else(|| {
            AuthError::storage("credential_delete_failed", "invalid credential path")
        })?;
        if !parent.exists() {
            return Ok(false);
        }
        let _lock = credential_lock(parent)?;
        match fs::remove_file(&self.path) {
            Ok(()) => Ok(true),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(err) => Err(AuthError::storage(
                "credential_delete_failed",
                format!("无法删除认证凭据: {err}"),
            )),
        }
    }

    fn can_persist(&self) -> bool {
        true
    }

    fn source(&self) -> AuthSource {
        AuthSource::File
    }
}

fn credential_lock(dir: &Path) -> Result<File, AuthError> {
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(dir.join("auth.lock"))
        .map_err(|_| AuthError::storage("credential_lock_failed", "cannot open credential lock"))?;
    file.try_lock()
        .map_err(|_| AuthError::storage("credential_store_busy", "credential store is busy"))?;
    Ok(file)
}

const KEYRING_SERVICE: &str = "com.fallingstar.sandflos.cli";

pub struct KeyringCredentialStore {
    #[cfg_attr(not(any(target_os = "macos", target_os = "windows")), allow(dead_code))]
    service: String,
}

impl KeyringCredentialStore {
    pub fn for_paths(paths: &AppPaths) -> Self {
        use sha1::{Digest, Sha1};
        let absolute = std::path::absolute(paths.config_dir())
            .unwrap_or_else(|_| paths.config_dir().to_path_buf());
        let identity = absolute.to_string_lossy();
        Self {
            service: format!("{KEYRING_SERVICE}.{:x}", Sha1::digest(identity.as_bytes())),
        }
    }
    pub fn new() -> Self {
        Self {
            service: KEYRING_SERVICE.to_string(),
        }
    }
}

impl Default for KeyringCredentialStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn keyring_entry(service: &str) -> Result<keyring::Entry, AuthError> {
    keyring::Entry::new(service, "token")
        .map_err(|_| AuthError::storage("credential_store_unavailable", "平台凭据存储不可用"))
}

impl CredentialStore for KeyringCredentialStore {
    fn load(&self) -> Result<Option<StoredCredential>, AuthError> {
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        {
            use keyring::Error as KeyringError;
            let entry = keyring_entry(&self.service)?;
            match entry.get_password() {
                Ok(text) => {
                    let credential: PersistedCredential =
                        serde_json::from_str(&text).map_err(|_| {
                            AuthError::storage("invalid_credentials_file", "平台凭据内容格式无效")
                        })?;
                    if credential.schema_version != CREDENTIAL_SCHEMA_VERSION {
                        return Err(AuthError::storage(
                            "unsupported_credentials_version",
                            "平台凭据版本不受支持",
                        ));
                    }
                    let token = validate_token(&credential.token).map_err(|_| {
                        AuthError::storage("invalid_credentials_file", "平台凭据中的 token 无效")
                    })?;
                    Ok(Some(StoredCredential {
                        token,
                        updated_ms: credential.updated_ms,
                    }))
                }
                Err(KeyringError::NoEntry) => Ok(None),
                Err(_) => Err(AuthError::storage(
                    "credential_read_failed",
                    "无法读取平台凭据存储",
                )),
            }
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            Err(AuthError::new(
                AuthErrorKind::Unsupported,
                "credential_store_unavailable",
                "当前平台没有可用且持久的系统凭据存储",
            ))
        }
    }

    fn save(&self, token: &str) -> Result<u128, AuthError> {
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        {
            let updated_ms = unix_ms_now();
            let payload = PersistedCredential {
                schema_version: CREDENTIAL_SCHEMA_VERSION,
                token: token.to_string(),
                updated_ms,
            };
            let text = serde_json::to_string(&payload)
                .map_err(|_| AuthError::storage("credential_write_failed", "无法序列化认证凭据"))?;
            keyring_entry(&self.service)?
                .set_password(&text)
                .map_err(|_| {
                    AuthError::storage("credential_write_failed", "无法保存到平台凭据存储")
                })?;
            Ok(updated_ms)
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            let _ = token;
            Err(AuthError::new(
                AuthErrorKind::Unsupported,
                "credential_store_unavailable",
                "当前平台没有可用且持久的系统凭据存储；请设置 SANDFLOS_TOKEN 或显式选择 SANDFLOS_CREDENTIAL_STORE=file",
            ))
        }
    }

    fn clear(&self) -> Result<bool, AuthError> {
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        {
            use keyring::Error as KeyringError;
            match keyring_entry(&self.service)?.delete_credential() {
                Ok(()) => Ok(true),
                Err(KeyringError::NoEntry) => Ok(false),
                Err(_) => Err(AuthError::storage(
                    "credential_delete_failed",
                    "无法删除平台凭据",
                )),
            }
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            Ok(false)
        }
    }

    fn can_persist(&self) -> bool {
        cfg!(any(target_os = "macos", target_os = "windows"))
    }

    fn source(&self) -> AuthSource {
        AuthSource::Keyring
    }
}

pub struct UnavailableCredentialStore;

impl CredentialStore for UnavailableCredentialStore {
    fn load(&self) -> Result<Option<StoredCredential>, AuthError> {
        Ok(None)
    }

    fn save(&self, _token: &str) -> Result<u128, AuthError> {
        Err(AuthError::new(
            AuthErrorKind::Unsupported,
            "credential_store_unavailable",
            "当前平台没有可用且持久的系统凭据存储；请设置 SANDFLOS_TOKEN 或显式选择 SANDFLOS_CREDENTIAL_STORE=file",
        ))
    }

    fn clear(&self) -> Result<bool, AuthError> {
        Ok(false)
    }

    fn can_persist(&self) -> bool {
        false
    }

    fn source(&self) -> AuthSource {
        AuthSource::None
    }
}

pub fn credential_store(kind: CredentialStoreKind, paths: &AppPaths) -> Box<dyn CredentialStore> {
    match kind {
        CredentialStoreKind::File => Box::new(FileCredentialStore::new(paths)),
        CredentialStoreKind::Keyring => Box::new(KeyringCredentialStore::for_paths(paths)),
        CredentialStoreKind::Unavailable => Box::new(UnavailableCredentialStore),
    }
}

pub struct AuthService {
    paths: AppPaths,
    client: Box<dyn AuthClient>,
    store: Box<dyn CredentialStore>,
    environment_token: Option<String>,
}

impl AuthService {
    pub fn new(
        paths: AppPaths,
        client: Box<dyn AuthClient>,
        store: Box<dyn CredentialStore>,
        environment_token: Option<String>,
    ) -> Self {
        Self {
            paths,
            client,
            store,
            environment_token: environment_token.and_then(|value| {
                let trimmed = value.trim();
                (!trimmed.is_empty()).then(|| trimmed.to_string())
            }),
        }
    }

    pub fn with_http_client(
        paths: AppPaths,
        store: Box<dyn CredentialStore>,
        environment_token: Option<String>,
    ) -> Self {
        Self::new(
            paths,
            Box::new(HttpAuthClient::default()),
            store,
            environment_token,
        )
    }

    pub fn paths(&self) -> &AppPaths {
        &self.paths
    }

    pub fn environment_token_active(&self) -> bool {
        self.environment_token.is_some()
    }

    pub fn resolve_token(&self) -> Result<Option<ResolvedToken>, AuthError> {
        if let Some(token) = &self.environment_token {
            return Ok(Some(ResolvedToken {
                token: token.clone(),
                source: AuthSource::Environment,
                updated_ms: None,
            }));
        }
        let Some(credential) = self.store.load()? else {
            return Ok(None);
        };
        Ok(Some(ResolvedToken {
            token: credential.token,
            source: self.store.source(),
            updated_ms: Some(credential.updated_ms),
        }))
    }

    pub fn status(&self, verify: bool) -> Result<AuthStatus, AuthError> {
        let Some(resolved) = self.resolve_token()? else {
            if verify {
                return Err(AuthError::authentication(
                    "missing_credentials",
                    "尚未登录，请先导入 token 或完成扫码登录",
                ));
            }
            return Ok(AuthStatus::missing());
        };
        if verify {
            self.client.verify_token(resolved.expose())?;
        }
        Ok(resolved.status())
    }

    pub fn import_token(&self, raw: &str) -> Result<AuthStatus, AuthError> {
        self.ensure_environment_token_absent()?;
        let token = validate_token(raw)?;
        if !self.store.can_persist() {
            return Err(AuthError::new(
                AuthErrorKind::Unsupported,
                "credential_store_unavailable",
                "当前平台没有可用且持久的凭证存储；请设置 SANDFLOS_TOKEN 或显式选择 file 存储",
            ));
        }
        self.client.verify_token(&token)?;
        let updated_ms = self.store.save(&token)?;
        Ok(AuthStatus {
            authorized: true,
            source: self.store.source(),
            updated_ms: Some(updated_ms),
        })
    }

    pub fn logout(&self) -> Result<LogoutOutcome, AuthError> {
        let cleared = self.store.clear()?;
        Ok(LogoutOutcome {
            cleared,
            environment_token_active: self.environment_token.is_some(),
        })
    }

    pub fn login_with_qr<F, C>(
        &self,
        options: QrLoginOptions,
        on_challenge: F,
        is_cancelled: C,
    ) -> Result<AuthStatus, AuthError>
    where
        F: FnOnce(&QrLoginChallenge) -> Result<(), AuthError>,
        C: Fn() -> bool,
    {
        self.ensure_environment_token_absent()?;
        if !self.store.can_persist() {
            return Err(AuthError::new(
                AuthErrorKind::Unsupported,
                "credential_store_unavailable",
                "当前平台没有可用且持久的凭证存储；请设置 SANDFLOS_TOKEN 或显式选择 SANDFLOS_CREDENTIAL_STORE=file",
            ));
        }
        let challenge = self.client.create_qr_login()?;
        on_challenge(&challenge)?;
        let deadline = Instant::now() + options.timeout;
        loop {
            if is_cancelled() {
                return Err(AuthError::cancelled());
            }
            if Instant::now() >= deadline {
                return Err(AuthError::timeout("等待扫码登录超时"));
            }
            match self.client.check_qr_login(&challenge.qrcode_id)? {
                QrLoginPoll::Pending => {
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    let wait = options.poll_interval.min(remaining);
                    if !wait.is_zero() {
                        std::thread::sleep(wait);
                    }
                }
                QrLoginPoll::Expired => {
                    return Err(AuthError::authentication(
                        "qr_expired",
                        "二维码已过期，请重新生成",
                    ))
                }
                QrLoginPoll::Confirmed { token } => {
                    let token = validate_token(&token)?;
                    let updated_ms = self.store.save(&token)?;
                    return Ok(AuthStatus {
                        authorized: true,
                        source: self.store.source(),
                        updated_ms: Some(updated_ms),
                    });
                }
            }
        }
    }

    fn ensure_environment_token_absent(&self) -> Result<(), AuthError> {
        if self.environment_token.is_some() {
            return Err(AuthError::input(
                "environment_token_active",
                "SANDFLOS_TOKEN 已生效，新凭据不会覆盖或落盘；请清除该环境变量后重试",
            ));
        }
        Ok(())
    }
}

fn write_private_temp_file(path: &Path, data: &[u8]) -> io::Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(data)?;
    file.sync_all()?;
    drop(file);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

fn replace_file(temp_path: &Path, destination: &Path) -> io::Result<()> {
    crate::persistence::atomic_replace(temp_path, destination)
}

fn set_private_dir_permissions(path: &Path) -> Result<(), AuthError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(|err| {
            AuthError::storage(
                "credential_write_failed",
                format!("无法设置配置目录权限: {err}"),
            )
        })?;
    }
    #[cfg(windows)]
    {
        // Pass the path as data, never interpolate it into PowerShell code.
        let script = r#"$ErrorActionPreference='Stop'; $p=$env:SANDFLOS_PRIVATE_DIRECTORY; $sid=[System.Security.Principal.WindowsIdentity]::GetCurrent().User; $acl=New-Object System.Security.AccessControl.DirectorySecurity; $acl.SetOwner($sid); $acl.SetAccessRuleProtection($true,$false); $rule=New-Object System.Security.AccessControl.FileSystemAccessRule($sid,'FullControl','ContainerInherit,ObjectInherit','None','Allow'); $acl.AddAccessRule($rule); Set-Acl -LiteralPath $p -AclObject $acl"#;
        let status = std::process::Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", script])
            .env("SANDFLOS_PRIVATE_DIRECTORY", path)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map_err(|_| {
                AuthError::storage(
                    "credential_write_failed",
                    "cannot restrict credential directory ACL",
                )
            })?;
        if !status.success() {
            return Err(AuthError::storage(
                "credential_write_failed",
                "cannot restrict credential directory ACL",
            ));
        }
    }
    #[cfg(not(any(unix, windows)))]
    let _ = path;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    };

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "sandflos-auth-service-{}-{}",
                std::process::id(),
                uuid::Uuid::new_v4()
            ));
            fs::create_dir_all(&path).expect("temp dir");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    struct FakeClient {
        verified_tokens: Mutex<Vec<String>>,
        verify_result: Mutex<Result<AuthIdentity, AuthError>>,
        qr_polls: Mutex<Vec<QrLoginPoll>>,
        qr_checks: AtomicUsize,
        created_qr: Mutex<QrLoginChallenge>,
    }

    impl FakeClient {
        fn accepted() -> Self {
            Self {
                verified_tokens: Mutex::new(Vec::new()),
                verify_result: Mutex::new(Ok(AuthIdentity {
                    uid: "uid-1".to_string(),
                    nickname: Some("reader".to_string()),
                })),
                qr_polls: Mutex::new(Vec::new()),
                qr_checks: AtomicUsize::new(0),
                created_qr: Mutex::new(QrLoginChallenge {
                    qrcode_id: "qr-1".to_string(),
                    qrcode_data: "https://example.test/qr-1".to_string(),
                }),
            }
        }

        fn with_qr_polls(polls: Vec<QrLoginPoll>) -> Self {
            let fake = Self::accepted();
            *fake.qr_polls.lock().expect("polls") = polls;
            fake
        }
    }

    impl AuthClient for FakeClient {
        fn verify_token(&self, token: &str) -> Result<AuthIdentity, AuthError> {
            self.verified_tokens
                .lock()
                .expect("verified tokens")
                .push(token.to_string());
            self.verify_result.lock().expect("verify result").clone()
        }

        fn create_qr_login(&self) -> Result<QrLoginChallenge, AuthError> {
            Ok(self.created_qr.lock().expect("created qr").clone())
        }

        fn check_qr_login(&self, qrcode_id: &str) -> Result<QrLoginPoll, AuthError> {
            assert_eq!(qrcode_id, "qr-1");
            self.qr_checks.fetch_add(1, Ordering::SeqCst);
            let mut polls = self.qr_polls.lock().expect("polls");
            if polls.is_empty() {
                Ok(QrLoginPoll::Pending)
            } else {
                Ok(polls.remove(0))
            }
        }
    }

    fn service(
        dir: &TempDir,
        client: Arc<FakeClient>,
        environment_token: Option<&str>,
    ) -> AuthService {
        AuthService::new(
            AppPaths::new(dir.path().join("config")),
            Box::new(SharedClient(client)),
            Box::new(FileCredentialStore::new(&AppPaths::new(
                dir.path().join("config"),
            ))),
            environment_token.map(ToString::to_string),
        )
    }

    fn service_with_token(
        dir: &TempDir,
        client: Arc<FakeClient>,
        environment_token: Option<&str>,
    ) -> AuthService {
        AuthService::new(
            AppPaths::new(dir.path().join("config")),
            Box::new(SharedClient(client)),
            Box::new(FileCredentialStore::new(&AppPaths::new(
                dir.path().join("config"),
            ))),
            environment_token.map(ToString::to_string),
        )
    }

    struct SharedClient(Arc<FakeClient>);

    impl AuthClient for SharedClient {
        fn verify_token(&self, token: &str) -> Result<AuthIdentity, AuthError> {
            self.0.verify_token(token)
        }

        fn create_qr_login(&self) -> Result<QrLoginChallenge, AuthError> {
            self.0.create_qr_login()
        }

        fn check_qr_login(&self, qrcode_id: &str) -> Result<QrLoginPoll, AuthError> {
            self.0.check_qr_login(qrcode_id)
        }
    }

    fn spawn_auth_server(
        responses: Vec<(&'static str, &'static str, &'static str)>,
    ) -> (String, Arc<Mutex<Vec<String>>>, std::thread::JoinHandle<()>) {
        use std::io::{BufRead, BufReader, Write};
        use std::net::TcpListener;

        let listener = TcpListener::bind("localhost:0").expect("bind auth fixture");
        let address = listener.local_addr().expect("fixture address");
        let requests = Arc::new(Mutex::new(Vec::new()));
        let observed = Arc::clone(&requests);
        let handle = std::thread::spawn(move || {
            for (status, content_type, body) in responses {
                let (mut stream, _) = listener.accept().expect("accept request");
                let mut reader = BufReader::new(stream.try_clone().expect("clone stream"));
                let mut request_line = String::new();
                reader.read_line(&mut request_line).expect("request line");
                let mut headers = Vec::new();
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).expect("header");
                    if line == "\r\n" || line.is_empty() {
                        break;
                    }
                    headers.push(line.trim_end().to_string());
                }
                observed
                    .lock()
                    .expect("requests")
                    .push(format!("{request_line}{}", headers.join("\n")));
                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream
                    .write_all(response.as_bytes())
                    .expect("write response");
            }
        });
        (format!("http://{address}"), requests, handle)
    }

    #[test]
    fn http_adapter_verifies_token_and_classifies_rejection() {
        let (base_url, requests, handle) = spawn_auth_server(vec![
            (
                "200 OK",
                "application/json",
                r#"{"result_code":0,"data":{"uid":"uid-http","nickname":"reader"}}"#,
            ),
            (
                "401 Unauthorized",
                "application/json",
                r#"{"error":"secret echo"}"#,
            ),
        ]);
        let client = HttpAuthClient::with_base_url(Duration::from_secs(2), base_url)
            .expect("http auth client");

        let identity = client.verify_token("fixture-token").expect("verified");
        assert_eq!(identity.uid, "uid-http");
        assert_eq!(identity.nickname.as_deref(), Some("reader"));

        let error = client.verify_token("fixture-token").expect_err("rejected");
        assert_eq!(error.kind, AuthErrorKind::Authentication);
        assert_eq!(error.code, "credential_rejected");
        assert!(!error.message.contains("secret echo"));
        handle.join().expect("fixture server");

        let requests = requests.lock().expect("requests");
        assert_eq!(requests.len(), 2);
        assert!(requests[0].starts_with("GET /api/1/users/me "));
        assert!(requests[0]
            .to_ascii_lowercase()
            .contains("authorization: bearer fixture-token"));
    }

    #[test]
    fn http_adapter_parses_qr_lifecycle() {
        let (base_url, requests, handle) = spawn_auth_server(vec![
            (
                "200 OK",
                "application/json",
                r#"{"result_code":0,"data":{"qrcodeId":"qr-http","data":"qr-payload"}}"#,
            ),
            (
                "200 OK",
                "application/json",
                r#"{"result_code":0,"data":{"status":0}}"#,
            ),
            (
                "200 OK",
                "application/json",
                r#"{"result_code":0,"data":{"status":1,"userInfo":{"token":"qr-token"}}}"#,
            ),
        ]);
        let client = HttpAuthClient::with_base_url(Duration::from_secs(2), base_url)
            .expect("http auth client");

        let challenge = client.create_qr_login().expect("challenge");
        assert_eq!(challenge.qrcode_id, "qr-http");
        assert_eq!(challenge.qrcode_data, "qr-payload");
        assert_eq!(
            client
                .check_qr_login(&challenge.qrcode_id)
                .expect("pending"),
            QrLoginPoll::Pending
        );
        assert_eq!(
            client
                .check_qr_login(&challenge.qrcode_id)
                .expect("confirmed"),
            QrLoginPoll::Confirmed {
                token: "qr-token".to_string()
            }
        );
        handle.join().expect("fixture server");

        let requests = requests.lock().expect("requests");
        assert!(requests[0].starts_with("POST /api/1/auth/qrcode/create "));
        assert!(requests[1].starts_with("GET /api/1/auth/qrcode/check?"));
        assert!(requests[1].contains("clientType=web"));
        assert!(requests[1].contains("qrcodeId=qr-http"));
    }

    #[test]
    fn invalid_http_endpoint_is_an_input_error() {
        let error = HttpAuthClient::with_base_url(Duration::from_secs(1), "not a valid endpoint")
            .expect_err("invalid endpoint");
        assert_eq!(error.kind, AuthErrorKind::Input);
        assert_eq!(error.code, "invalid_endpoint");
    }

    #[test]
    fn two_config_dirs_do_not_share_credentials() {
        let first = TempDir::new();
        let second = TempDir::new();
        let client = Arc::new(FakeClient::accepted());

        service(&first, Arc::clone(&client), None)
            .import_token("token-one")
            .expect("import first token");
        let second_status = service(&second, Arc::clone(&client), None)
            .status(false)
            .expect("second status");

        assert!(!second_status.authorized);
        assert_eq!(second_status.source, AuthSource::None);
        assert!(!second.path().join("config").exists());
    }

    #[test]
    fn import_then_verify_in_new_process_is_authorized() {
        let dir = TempDir::new();
        let client = Arc::new(FakeClient::accepted());
        service(&dir, Arc::clone(&client), None)
            .import_token("token-one")
            .expect("import");
        let restarted = service(&dir, Arc::clone(&client), None);

        let status = restarted.status(true).expect("verify status");
        assert!(status.authorized);
        assert_eq!(status.source, AuthSource::File);
        assert!(status.updated_ms.is_some());
        assert_eq!(
            client.verified_tokens.lock().expect("tokens").as_slice(),
            &["token-one".to_string(), "token-one".to_string()]
        );
    }

    #[test]
    fn environment_token_takes_precedence_and_is_never_persisted() {
        let dir = TempDir::new();
        let client = Arc::new(FakeClient::accepted());
        let stored = service(&dir, Arc::clone(&client), None);
        stored.import_token("file-token").expect("import");
        let service_with_env = service(&dir, Arc::clone(&client), Some("env-token"));

        let resolved = service_with_env
            .resolve_token()
            .expect("resolve")
            .expect("token");
        assert_eq!(resolved.expose(), "env-token");
        assert_eq!(resolved.source, AuthSource::Environment);
        assert_eq!(
            format!("{resolved:?}"),
            "ResolvedToken { token: \"<redacted>\", source: Environment, updated_ms: None }"
        );

        let status = service_with_env.status(true).expect("verify");
        assert_eq!(status.source, AuthSource::Environment);
        let persisted = fs::read_to_string(dir.path().join("config/auth.json")).expect("file");
        assert!(!persisted.contains("env-token"));
    }

    #[test]
    fn environment_token_blocks_import_and_qr_login() {
        let dir = TempDir::new();
        let client = Arc::new(FakeClient::accepted());
        let service = service(&dir, Arc::clone(&client), Some("env-token"));

        let import_error = service
            .import_token("file-token")
            .expect_err("import rejected");
        assert_eq!(import_error.kind, AuthErrorKind::Input);
        assert_eq!(import_error.code, "environment_token_active");

        let qr_error = service
            .login_with_qr(QrLoginOptions::default(), |_| Ok(()), || false)
            .expect_err("qr rejected");
        assert_eq!(qr_error.code, "environment_token_active");
        assert_eq!(client.qr_checks.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn credential_write_failure_is_storage_error_and_saves_nothing() {
        let dir = TempDir::new();
        let config = dir.path().join("config");
        fs::write(&config, "not a directory").expect("blocking config path");
        let client = Arc::new(FakeClient::accepted());

        let error = service(&dir, Arc::clone(&client), None)
            .import_token("new-token")
            .expect_err("write failure");
        assert_eq!(error.kind, AuthErrorKind::Storage);
        assert_eq!(error.code, "credential_write_failed");
        assert!(!config.join("auth.json").exists());
    }

    #[test]
    fn invalid_tokens_are_rejected_without_writing_credentials() {
        let dir = TempDir::new();
        let client = Arc::new(FakeClient::accepted());
        let service = service(&dir, Arc::clone(&client), None);

        for token in ["", "   ", "two words", "line\nbreak"] {
            let error = service.import_token(token).expect_err("invalid token");
            assert_eq!(error.kind, AuthErrorKind::Input);
            assert_eq!(error.code, "invalid_token");
        }
        assert!(!dir.path().join("config/auth.json").exists());
    }

    #[test]
    fn status_without_credentials_is_not_authorized_but_is_not_an_error() {
        let dir = TempDir::new();
        let client = Arc::new(FakeClient::accepted());
        let status = service(&dir, Arc::clone(&client), None)
            .status(false)
            .expect("status");
        assert!(!status.authorized);
        assert_eq!(status.source, AuthSource::None);
    }

    #[test]
    fn verify_without_credentials_requires_login() {
        let dir = TempDir::new();
        let client = Arc::new(FakeClient::accepted());
        let error = service(&dir, Arc::clone(&client), None)
            .status(true)
            .expect_err("missing credentials");
        assert_eq!(error.kind, AuthErrorKind::Authentication);
        assert_eq!(error.code, "missing_credentials");
        assert_eq!(error.kind.exit_code(), 3);
    }

    #[test]
    fn rejected_token_is_reported_as_authentication_error() {
        let dir = TempDir::new();
        let client = Arc::new(FakeClient::accepted());
        let service = service(&dir, Arc::clone(&client), None);
        service.import_token("token-one").expect("import");
        *client.verify_result.lock().expect("verify result") = Err(AuthError::authentication(
            "credential_rejected",
            "认证凭据无效或已过期",
        ));

        let error = service.status(true).expect_err("rejected");
        assert_eq!(error.kind, AuthErrorKind::Authentication);
        assert_eq!(error.kind.exit_code(), 3);
    }

    #[test]
    fn import_verifies_token_before_persisting() {
        let dir = TempDir::new();
        let client = Arc::new(FakeClient::accepted());
        let status = service(&dir, Arc::clone(&client), None)
            .import_token("token-one")
            .expect("import verified token");

        assert!(status.authorized);
        assert_eq!(
            client.verified_tokens.lock().expect("tokens").as_slice(),
            &["token-one".to_string()]
        );
        assert!(dir.path().join("config/auth.json").exists());
    }

    #[test]
    fn rejected_import_does_not_persist_a_credential() {
        let dir = TempDir::new();
        let client = Arc::new(FakeClient::accepted());
        *client.verify_result.lock().expect("verify result") = Err(AuthError::authentication(
            "credential_rejected",
            "认证凭据无效或已过期",
        ));

        let error = service(&dir, Arc::clone(&client), None)
            .import_token("token-one")
            .expect_err("rejected import");
        assert_eq!(error.kind, AuthErrorKind::Authentication);
        assert_eq!(error.code, "credential_rejected");
        assert!(!dir.path().join("config/auth.json").exists());
    }

    #[test]
    fn logout_is_idempotent_and_reports_environment_shadowing() {
        let dir = TempDir::new();
        let client = Arc::new(FakeClient::accepted());
        let service = service(&dir, Arc::clone(&client), None);
        service.import_token("token-one").expect("import");

        let outcome = service.logout().expect("logout");
        assert!(outcome.cleared);
        assert!(!outcome.environment_token_active);
        let second = service.logout().expect("idempotent logout");
        assert!(!second.cleared);
        assert!(!service.status(false).expect("status").authorized);

        let env_service = service_with_token(&dir, Arc::clone(&client), Some("env-token"));
        let outcome = env_service.logout().expect("env logout");
        assert!(!outcome.cleared);
        assert!(outcome.environment_token_active);
        assert!(env_service.status(false).expect("env status").authorized);
    }

    #[test]
    fn qr_pending_then_confirmed_persists_session() {
        let dir = TempDir::new();
        let client = Arc::new(FakeClient::with_qr_polls(vec![
            QrLoginPoll::Pending,
            QrLoginPoll::Confirmed {
                token: "qr-token".to_string(),
            },
        ]));
        let service = service(&dir, Arc::clone(&client), None);
        let mut displayed = None;
        let status = service
            .login_with_qr(
                QrLoginOptions {
                    poll_interval: Duration::ZERO,
                    timeout: Duration::from_secs(2),
                },
                |challenge| {
                    displayed = Some(challenge.qrcode_data.clone());
                    Ok(())
                },
                || false,
            )
            .expect("qr login");

        assert_eq!(displayed.as_deref(), Some("https://example.test/qr-1"));
        assert!(status.authorized);
        assert_eq!(client.qr_checks.load(Ordering::SeqCst), 2);
        let resolved = service.resolve_token().expect("resolve").expect("token");
        assert_eq!(resolved.expose(), "qr-token");
    }

    #[test]
    fn expired_qr_is_rejected_without_persisting() {
        let dir = TempDir::new();
        let client = Arc::new(FakeClient::with_qr_polls(vec![QrLoginPoll::Expired]));
        let service = service(&dir, Arc::clone(&client), None);

        let error = service
            .login_with_qr(QrLoginOptions::default(), |_| Ok(()), || false)
            .expect_err("expired");
        assert_eq!(error.kind, AuthErrorKind::Authentication);
        assert_eq!(error.code, "qr_expired");
        assert!(!dir.path().join("config/auth.json").exists());
    }

    #[test]
    fn cancelled_qr_wait_stops_polling() {
        let dir = TempDir::new();
        let client = Arc::new(FakeClient::with_qr_polls(vec![QrLoginPoll::Pending]));
        let service = service(&dir, Arc::clone(&client), None);
        let checks = Arc::new(AtomicUsize::new(0));
        let checks_for_closure = Arc::clone(&checks);

        let error = service
            .login_with_qr(
                QrLoginOptions {
                    poll_interval: Duration::from_millis(1),
                    timeout: Duration::from_secs(2),
                },
                |_| Ok(()),
                move || checks_for_closure.fetch_add(1, Ordering::SeqCst) >= 1,
            )
            .expect_err("cancelled");
        assert_eq!(error.kind, AuthErrorKind::Cancelled);
        assert_eq!(error.kind.exit_code(), 130);
    }

    #[test]
    fn app_paths_prefers_explicit_then_environment() {
        let explicit = AppPaths::resolve(
            Some(PathBuf::from("/explicit")),
            Some(PathBuf::from("/environment")),
        )
        .expect("explicit");
        assert_eq!(explicit.config_dir(), Path::new("/explicit"));

        let environment =
            AppPaths::resolve(None, Some(PathBuf::from("/environment"))).expect("environment");
        assert_eq!(environment.config_dir(), Path::new("/environment"));
    }

    #[test]
    fn unavailable_store_refuses_persistence_instead_of_falling_back_to_file() {
        let dir = TempDir::new();
        let client = Arc::new(FakeClient::accepted());
        let paths = AppPaths::new(dir.path().join("config"));
        let service = AuthService::new(
            paths,
            Box::new(SharedClient(Arc::clone(&client))),
            credential_store(
                CredentialStoreKind::Unavailable,
                &AppPaths::new(dir.path().join("config")),
            ),
            None,
        );

        let error = service.import_token("token-one").expect_err("unavailable");
        assert_eq!(error.kind, AuthErrorKind::Unsupported);
        assert_eq!(error.code, "credential_store_unavailable");
        assert_eq!(error.kind.exit_code(), 5);
        assert!(!dir.path().join("config/auth.json").exists());

        let error = service
            .login_with_qr(QrLoginOptions::default(), |_| Ok(()), || false)
            .expect_err("qr unavailable");
        assert_eq!(error.code, "credential_store_unavailable");
        assert_eq!(client.qr_checks.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn credential_store_kind_validation_is_explicit() {
        assert_eq!(
            CredentialStoreKind::parse("file").unwrap(),
            CredentialStoreKind::File
        );
        assert_eq!(
            CredentialStoreKind::parse("keyring").unwrap(),
            CredentialStoreKind::Keyring
        );
        assert_eq!(
            CredentialStoreKind::parse("platform").unwrap(),
            CredentialStoreKind::Keyring
        );
        let error = CredentialStoreKind::parse("plaintext-anywhere").unwrap_err();
        assert_eq!(error.code, "invalid_credential_store");
    }

    #[test]
    fn corrupted_credentials_return_storage_error() {
        let dir = TempDir::new();
        let config = dir.path().join("config");
        fs::create_dir_all(&config).expect("config");
        fs::write(config.join("auth.json"), "{not json").expect("corrupt file");
        let client = Arc::new(FakeClient::accepted());

        let error = service(&dir, Arc::clone(&client), None)
            .status(false)
            .expect_err("corrupt");
        assert_eq!(error.kind, AuthErrorKind::Storage);
        assert_eq!(error.code, "invalid_credentials_file");
    }

    #[cfg(unix)]
    #[test]
    fn credentials_and_config_directory_are_private() {
        use std::os::unix::fs::PermissionsExt;

        let dir = TempDir::new();
        let client = Arc::new(FakeClient::accepted());
        let service = service(&dir, Arc::clone(&client), None);
        service.import_token("token-one").expect("import");

        let config = dir.path().join("config");
        let file = config.join("auth.json");
        assert_eq!(
            fs::metadata(&config)
                .expect("dir metadata")
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(&file)
                .expect("file metadata")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
}
