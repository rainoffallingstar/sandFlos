use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AuthState {
    pub token: Option<String>,
    pub updated_ms: Option<u128>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PersistedAuthState {
    pub token: Option<String>,
    pub updated_ms: Option<u128>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BucketConfig {
    #[serde(rename = "aliEndpoint")]
    pub ali_endpoint: Option<String>,
    pub bucket: Option<String>,
    #[serde(default)]
    pub region: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OssSts {
    #[serde(rename = "AccessKeyId")]
    pub access_key_id: String,
    #[serde(rename = "AccessKeySecret")]
    pub access_key_secret: String,
    #[serde(rename = "SecurityToken")]
    pub security_token: String,
    #[serde(rename = "Expiration")]
    pub expiration: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncToken {
    #[serde(rename = "cookie_name")]
    pub cookie_name: Option<String>,
    #[serde(rename = "session_id")]
    pub session_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct UploadAuthContext {
    pub bearer: String,
    pub uid: String,
    pub storage_limit: Option<u64>,
    pub storage_used: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DashboardAuth {
    pub authorized: bool,
    pub source: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct DashboardProfile {
    pub uid: String,
    pub nickname: Option<String>,
    pub avatar_url: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct DashboardStorage {
    pub used: Option<u64>,
    pub limit: Option<u64>,
    pub percent: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DashboardDevice {
    pub id: Option<String>,
    pub model: Option<String>,
    pub mac_address: Option<String>,
    pub ip_address: Option<String>,
    pub login_status: Option<String>,
    pub latest_login_time: Option<String>,
    pub latest_logout_time: Option<String>,
    pub locked: Option<bool>,
    pub same_lan: bool,
    pub lan_ip: Option<String>,
    pub transfer_host: Option<String>,
    pub same_lan_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DashboardPushItem {
    pub id: String,
    pub rev: Option<String>,
    pub name: String,
    pub size: Option<u64>,
    pub updated_at: Option<i64>,
    pub format: Option<String>,
    pub resource_key: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DashboardCalendarMetrics {
    pub reading_info: Value,
    pub read_time_week: Value,
    pub day_read_today: Value,
}

#[derive(Debug, Clone, Serialize)]
pub struct DashboardUploadState {
    pub in_progress: bool,
    pub status_text: String,
    pub last_error: Option<String>,
    pub current_file: Option<String>,
    pub bytes_sent: Option<u64>,
    pub bytes_total: Option<u64>,
    pub progress_percent: Option<f64>,
    pub speed_bps: Option<f64>,
    pub eta_seconds: Option<f64>,
    pub updated_ms: u128,
}

#[derive(Debug, Clone, Serialize)]
pub struct DashboardSnapshot {
    pub auth: DashboardAuth,
    pub profile: Option<DashboardProfile>,
    pub storage: DashboardStorage,
    pub devices: Vec<DashboardDevice>,
    pub push_queue: Vec<DashboardPushItem>,
    pub calendar_metrics: DashboardCalendarMetrics,
    pub upload: DashboardUploadState,
    pub fetched_at_ms: u128,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ZoteroConnectionSummary {
    pub profile_dir: Option<String>,
    pub data_dir: Option<String>,
    pub database_path: Option<String>,
    pub database_exists: bool,
    pub webdav_url: Option<String>,
    pub webdav_username: Option<String>,
    pub protocol: Option<String>,
    pub protocol_is_webdav: bool,
    pub webdav_verified: bool,
    pub password_saved: bool,
    pub download_mode_personal: Option<String>,
    pub download_mode_groups: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ZoteroDetectionResult {
    pub profile_dir: Option<String>,
    pub profile_source: Option<String>,
    pub data_dir: Option<String>,
    pub data_dir_source: Option<String>,
    pub database_path: Option<String>,
    pub database_exists: bool,
    pub webdav_url: Option<String>,
    pub webdav_url_source: Option<String>,
    pub webdav_username: Option<String>,
    pub webdav_username_source: Option<String>,
    pub protocol: Option<String>,
    pub protocol_source: Option<String>,
    pub protocol_is_webdav: bool,
    pub webdav_verified: bool,
    pub download_mode_personal: Option<String>,
    pub download_mode_groups: Option<String>,
    pub has_saved_password: bool,
    pub detected_at_ms: Option<u128>,
    pub issues: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ZoteroConnectionState {
    pub state: String,
    pub missing_fields: Vec<String>,
    pub summary: ZoteroConnectionSummary,
    pub detected_at_ms: Option<u128>,
    pub validated_at_ms: Option<u128>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ZoteroAttachmentSummary {
    pub attachment_item_id: i64,
    pub attachment_key: String,
    pub file_name: Option<String>,
    pub content_type: Option<String>,
    pub link_mode: i64,
    pub local_exists: bool,
    pub local_path: Option<String>,
    pub can_push_directly: bool,
    pub can_download_from_webdav: bool,
    pub status_label: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ZoteroItemSummary {
    pub item_id: i64,
    pub item_key: String,
    pub title: String,
    pub author_summary: Option<String>,
    pub year: Option<String>,
    pub date_modified: String,
    pub attachments: Vec<ZoteroAttachmentSummary>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ZoteroSaveInput {
    pub profile_dir: Option<String>,
    pub data_dir: Option<String>,
    pub webdav_url: Option<String>,
    pub webdav_username: Option<String>,
    pub webdav_password: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CalibreConnectionSummary {
    pub library_dirs: Vec<String>,
    pub database_paths: Vec<String>,
    pub ready_library_dirs: Vec<String>,
    pub ready_library_count: usize,
    pub total_library_count: usize,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CalibreConnectionState {
    pub state: String,
    pub missing_fields: Vec<String>,
    pub summary: CalibreConnectionSummary,
    pub detected_at_ms: Option<u128>,
    pub validated_at_ms: Option<u128>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CalibreFormatSummary {
    pub data_id: i64,
    pub library_dir: String,
    pub format: String,
    pub file_name: Option<String>,
    pub file_path: Option<String>,
    pub file_size: Option<u64>,
    pub local_exists: bool,
    pub can_push_directly: bool,
    pub status_label: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CalibreBookSummary {
    pub book_id: i64,
    pub library_dir: String,
    pub library_label: String,
    pub title: String,
    pub author_summary: Option<String>,
    pub published_year: Option<String>,
    pub date_modified: String,
    pub formats: Vec<CalibreFormatSummary>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct CalibreSaveInput {
    pub library_dir: Option<String>,
    pub library_dirs: Option<Vec<String>>,
}

#[derive(Debug, Clone)]
pub struct ShareTransferDevice {
    pub model: Option<String>,
    pub mac_address: Option<String>,
    pub host: Option<String>,
    pub status: Option<String>,
}

#[derive(Debug, Clone)]
pub struct PushDocDetail {
    pub rev: String,
    pub resource_key: String,
}

#[derive(Debug, Clone)]
pub struct UploadRuntimeState {
    pub in_progress: bool,
    pub status_text: String,
    pub last_error: Option<String>,
    pub current_file: Option<String>,
    pub bytes_sent: Option<u64>,
    pub bytes_total: Option<u64>,
    pub progress_percent: Option<f64>,
    pub speed_bps: Option<f64>,
    pub eta_seconds: Option<f64>,
    pub updated_ms: u128,
}

impl Default for UploadRuntimeState {
    fn default() -> Self {
        Self {
            in_progress: false,
            status_text: "上传进度: 空闲".to_string(),
            last_error: None,
            current_file: None,
            bytes_sent: None,
            bytes_total: None,
            progress_percent: None,
            speed_bps: None,
            eta_seconds: None,
            updated_ms: 0,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct ApiEnvelope<T> {
    #[serde(default)]
    pub result_code: i64,
    #[serde(default)]
    pub message: Option<String>,
    pub data: T,
}

#[derive(Debug, Clone)]
pub struct QrCreateResponse {
    pub qrcode_id: String,
    pub qrcode_data: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerifyCodeRequest {
    pub mobi: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub area_code: Option<String>,
    pub verify: String,
    pub scene: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PhoneOrEmailLoginRequest {
    pub mobi: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub area_code: Option<String>,
    pub code: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct QrCheckResponse {
    #[serde(default)]
    pub status: i64,
    #[serde(rename = "userInfo", default)]
    pub user_info: Option<QrLoginUserInfo>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct QrLoginUserInfo {
    pub token: Option<String>,
}
