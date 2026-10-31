use crate::api::{
    auth_source_text, create_client, fetch_auth_context_for_token_with_endpoints,
    fetch_day_read_with_endpoints, fetch_devices_with_endpoints,
    fetch_push_queue_for_dashboard_with_endpoints, fetch_read_time_info_with_endpoints,
    fetch_reading_info_with_endpoints, ApiEndpoints, ApiError, ApiErrorKind,
};
use crate::device::{build_dashboard_devices, build_dashboard_devices_pure, fetch_share_devices};
use crate::models::{
    DashboardAuth, DashboardCalendarMetrics, DashboardProfile, DashboardSnapshot, DashboardStorage,
    DashboardUploadState, ShareTransferDevice,
};
use crate::state::{
    get_auth_state, get_dashboard_cache, get_upload_runtime_state, set_dashboard_cache,
};
use crate::util::{
    normalize_optional, reading_today_count, reading_total_count, reading_week_total_ms,
    short_duration_text, today_ymd, unix_ms_now,
};
use serde::Serialize;
use std::fmt;

pub const DASHBOARD_CACHE_MAX_AGE_MS: u128 = 5_000;

#[derive(serde::Serialize)]
pub struct AppStatus {
    pub app: String,
    pub version: String,
    pub unix_ms: u128,
}

pub fn app_status() -> AppStatus {
    AppStatus {
        app: "sandFlos".to_string(),
        version: "0.1.0".to_string(),
        unix_ms: unix_ms_now(),
    }
}

/// Failure category for an explicit desktop dashboard refresh.
///
/// The message and numeric fields come from the shared API boundary and never
/// include response bodies or credentials.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DashboardErrorKind {
    Input,
    Authentication,
    Network,
    Business,
    Protocol,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DashboardError {
    pub kind: DashboardErrorKind,
    pub stage: &'static str,
    pub status: Option<u16>,
    pub result_code: Option<i64>,
    pub message: String,
}

impl DashboardError {
    pub fn authentication(stage: &'static str, message: impl Into<String>) -> Self {
        Self {
            kind: DashboardErrorKind::Authentication,
            stage,
            status: None,
            result_code: None,
            message: message.into(),
        }
    }

    pub fn input(stage: &'static str, message: impl Into<String>) -> Self {
        Self {
            kind: DashboardErrorKind::Input,
            stage,
            status: None,
            result_code: None,
            message: message.into(),
        }
    }

    fn from_api(stage: &'static str, error: ApiError) -> Self {
        let kind = match error.kind {
            ApiErrorKind::Input => DashboardErrorKind::Input,
            ApiErrorKind::Authentication => DashboardErrorKind::Authentication,
            ApiErrorKind::Timeout | ApiErrorKind::Network => DashboardErrorKind::Network,
            ApiErrorKind::HttpStatus => match error.status {
                Some(401 | 403) => DashboardErrorKind::Authentication,
                _ => DashboardErrorKind::Network,
            },
            ApiErrorKind::Business => DashboardErrorKind::Business,
            ApiErrorKind::Protocol => DashboardErrorKind::Protocol,
        };
        Self {
            kind,
            stage,
            status: error.status,
            result_code: error.result_code,
            message: error.message,
        }
    }
}

impl fmt::Display for DashboardError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for DashboardError {}

/// Explicit inputs for a desktop dashboard refresh.
///
/// The caller owns the credential and upload projection. The refresh does not
/// read or write the process-global auth, upload, or dashboard cache state.
/// `share_devices` is the caller's transfer-device projection. The
/// endpoint-injected entry point requires it and never performs a production
/// WebSocket lookup or LAN probe. The production entry point consumes the
/// supplied projection when present and otherwise performs the production share
/// lookup. `today` is the caller's local date, used for both reading-statistics
/// requests.
pub struct DashboardRefreshInput {
    pub token: String,
    pub upload: DashboardUploadState,
    pub share_devices: Option<Vec<ShareTransferDevice>>,
    pub today: String,
}

impl DashboardRefreshInput {
    pub fn new(
        token: impl Into<String>,
        upload: DashboardUploadState,
        today: impl Into<String>,
    ) -> Self {
        Self {
            token: token.into(),
            upload,
            share_devices: None,
            today: today.into(),
        }
    }

    pub fn with_share_devices(mut self, share_devices: Vec<ShareTransferDevice>) -> Self {
        self.share_devices = Some(share_devices);
        self
    }
}

impl fmt::Debug for DashboardRefreshInput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DashboardRefreshInput")
            .field("token", &"<redacted>")
            .field("upload", &self.upload)
            .field("share_devices", &self.share_devices)
            .field("today", &self.today)
            .finish()
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FetchPolicy {
    Strict,
    Legacy,
}

enum ShareProjection {
    Explicit(Vec<ShareTransferDevice>),
    Production,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DeviceEnvironmentSource {
    Pure,
    System,
}

pub fn current_upload_snapshot() -> DashboardUploadState {
    let state = get_upload_runtime_state();
    DashboardUploadState {
        in_progress: state.in_progress,
        status_text: state.status_text,
        last_error: state.last_error,
        current_file: state.current_file,
        bytes_sent: state.bytes_sent,
        bytes_total: state.bytes_total,
        progress_percent: state.progress_percent,
        speed_bps: state.speed_bps,
        eta_seconds: state.eta_seconds,
        updated_ms: state.updated_ms,
    }
}

fn unauthorized_dashboard_snapshot(reason: String) -> DashboardSnapshot {
    DashboardSnapshot {
        auth: DashboardAuth {
            authorized: false,
            source: "none".to_string(),
            message: reason,
        },
        profile: None,
        storage: DashboardStorage::default(),
        devices: Vec::new(),
        push_queue: Vec::new(),
        calendar_metrics: DashboardCalendarMetrics {
            reading_info: serde_json::Value::Null,
            read_time_week: serde_json::Value::Null,
            day_read_today: serde_json::Value::Null,
        },
        upload: current_upload_snapshot(),
        fetched_at_ms: unix_ms_now(),
    }
}

/// Fetches a desktop dashboard using an explicit endpoint context and explicit
/// credential, upload, and transfer-device projections.
///
/// This entry point is side-effect free beyond the HTTP requests: it never
/// reads global auth/upload/cache state, performs a production share WebSocket
/// lookup, or probes the local LAN. `share_devices` is required.
pub fn fetch_dashboard_snapshot_with_endpoints(
    endpoints: &ApiEndpoints,
    client: &reqwest::blocking::Client,
    input: &DashboardRefreshInput,
) -> Result<DashboardSnapshot, DashboardError> {
    let share_devices = input.share_devices.clone().ok_or_else(|| {
        DashboardError::input(
            "devices",
            "endpoint-injected dashboard refresh requires a share_devices projection",
        )
    })?;
    assemble_dashboard(
        endpoints,
        client,
        input,
        FetchPolicy::Strict,
        ShareProjection::Explicit(share_devices),
        DeviceEnvironmentSource::Pure,
    )
}

/// Fetches a desktop dashboard against the production endpoints.
///
/// The caller provides the credential, upload projection, and local date. The
/// refresh does not consult the global auth, upload, or dashboard cache state.
/// When no transfer-device projection is supplied, the production share lookup
/// and LAN discovery run as in the legacy desktop path.
pub fn refresh_dashboard_snapshot(
    input: &DashboardRefreshInput,
) -> Result<DashboardSnapshot, DashboardError> {
    assemble_production_dashboard(input, FetchPolicy::Strict)
}

fn assemble_production_dashboard(
    input: &DashboardRefreshInput,
    policy: FetchPolicy,
) -> Result<DashboardSnapshot, DashboardError> {
    let client = create_client(30).map_err(|error| DashboardError::from_api("client", error))?;
    assemble_dashboard(
        &ApiEndpoints::production(),
        &client,
        input,
        policy,
        match &input.share_devices {
            Some(devices) => ShareProjection::Explicit(devices.clone()),
            None => ShareProjection::Production,
        },
        DeviceEnvironmentSource::System,
    )
}

fn optional_statistic(
    policy: FetchPolicy,
    stage: &'static str,
    result: Result<serde_json::Value, ApiError>,
) -> Result<serde_json::Value, DashboardError> {
    match (policy, result) {
        (_, Ok(value)) => Ok(value),
        (FetchPolicy::Legacy, Err(_)) => Ok(serde_json::Value::Null),
        (FetchPolicy::Strict, Err(error)) => Err(DashboardError::from_api(stage, error)),
    }
}

fn assemble_dashboard(
    endpoints: &ApiEndpoints,
    client: &reqwest::blocking::Client,
    input: &DashboardRefreshInput,
    policy: FetchPolicy,
    share_projection: ShareProjection,
    environment_source: DeviceEnvironmentSource,
) -> Result<DashboardSnapshot, DashboardError> {
    let token = normalize_optional(Some(input.token.clone()))
        .ok_or_else(|| DashboardError::authentication("auth", "当前未登录"))?;

    let (auth, user) = fetch_auth_context_for_token_with_endpoints(endpoints, client, &token)
        .map_err(|error| DashboardError::from_api("auth", error))?;

    let storage_used = auth.storage_used;
    let storage_limit = auth.storage_limit;
    let storage_percent = match (storage_used, storage_limit) {
        (Some(used), Some(limit)) if limit > 0 => Some(used as f64 * 100.0 / limit as f64),
        _ => None,
    };

    let reading_info = optional_statistic(
        policy,
        "reading_info",
        fetch_reading_info_with_endpoints(endpoints, client, &auth.bearer),
    )?;
    let read_time_week = optional_statistic(
        policy,
        "read_time_week",
        fetch_read_time_info_with_endpoints(endpoints, client, &auth.bearer, &input.today, "week"),
    )?;
    let day_read_today = optional_statistic(
        policy,
        "day_read_today",
        fetch_day_read_with_endpoints(endpoints, client, &auth.bearer, &input.today, &input.today),
    )?;

    let raw_devices = optional_statistic(
        policy,
        "devices",
        fetch_devices_with_endpoints(endpoints, client, &auth.bearer),
    )?;
    let share_devices = match share_projection {
        ShareProjection::Explicit(devices) => devices,
        ShareProjection::Production => fetch_share_devices(&auth.uid),
    };
    let devices = match environment_source {
        DeviceEnvironmentSource::Pure => build_dashboard_devices_pure(raw_devices, share_devices),
        DeviceEnvironmentSource::System => build_dashboard_devices(raw_devices, share_devices),
    };

    let queue_result = fetch_push_queue_for_dashboard_with_endpoints(endpoints, client, &auth);
    let push_queue = match (policy, queue_result) {
        (_, Ok(items)) => items,
        (FetchPolicy::Legacy, Err(_)) => Vec::new(),
        (FetchPolicy::Strict, Err(error)) => {
            return Err(DashboardError::from_api("queue", error));
        }
    };

    Ok(DashboardSnapshot {
        auth: DashboardAuth {
            authorized: true,
            source: auth_source_text(&auth),
            message: "已登录".to_string(),
        },
        profile: Some(DashboardProfile {
            uid: auth.uid,
            nickname: crate::util::json_field_to_string(&user, "nickname")
                .or_else(|| crate::util::json_field_to_string(&user, "name")),
            avatar_url: crate::util::json_field_to_string(&user, "avatar")
                .or_else(|| crate::util::json_field_to_string(&user, "avatarUrl")),
        }),
        storage: DashboardStorage {
            used: storage_used,
            limit: storage_limit,
            percent: storage_percent,
        },
        devices,
        push_queue,
        calendar_metrics: DashboardCalendarMetrics {
            reading_info,
            read_time_week,
            day_read_today,
        },
        upload: input.upload.clone(),
        fetched_at_ms: unix_ms_now(),
    })
}

/// Compatibility entry point for existing global-state desktop callers.
///
/// It still reads the legacy credential and upload projection and keeps the
/// historical soft-failure behavior: statistics and device failures become
/// `null`/empty device projections and a failed queue request becomes an empty
/// list. The production share lookup and LAN probing remain enabled. New
/// desktop refresh code uses [`refresh_dashboard_snapshot`] instead.
pub fn build_dashboard_snapshot() -> DashboardSnapshot {
    let token = match get_auth_state().token {
        Some(token) => token,
        None => return unauthorized_dashboard_snapshot("当前未登录".to_string()),
    };
    let input = DashboardRefreshInput::new(token, current_upload_snapshot(), today_ymd());
    match assemble_production_dashboard(&input, FetchPolicy::Legacy) {
        Ok(snapshot) => snapshot,
        Err(error) => unauthorized_dashboard_snapshot(error.to_string()),
    }
}

pub fn get_or_fetch_dashboard_snapshot(use_cache: bool) -> DashboardSnapshot {
    if use_cache {
        if let Some(cached) = get_dashboard_cache(DASHBOARD_CACHE_MAX_AGE_MS) {
            return cached;
        }
    }
    let snapshot = build_dashboard_snapshot();
    set_dashboard_cache(snapshot.clone());
    snapshot
}

pub fn build_reading_metrics_label(snapshot: &DashboardSnapshot) -> String {
    if !snapshot.auth.authorized {
        return "阅读统计指标: 未授权".to_string();
    }
    let today = reading_today_count(&snapshot.calendar_metrics.day_read_today);
    let week_ms = reading_week_total_ms(&snapshot.calendar_metrics.read_time_week);
    let total = reading_total_count(&snapshot.calendar_metrics.reading_info);
    format!(
        "阅读统计指标: 今日{} 本周{} 累计{}",
        today,
        short_duration_text(week_ms),
        total
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{
        DashboardAuth, DashboardCalendarMetrics, DashboardSnapshot, DashboardStorage,
        DashboardUploadState,
    };
    use serde_json::json;

    fn build_snapshot(authorized: bool) -> DashboardSnapshot {
        DashboardSnapshot {
            auth: DashboardAuth {
                authorized,
                source: "token".to_string(),
                message: "ok".to_string(),
            },
            profile: None,
            storage: DashboardStorage::default(),
            devices: Vec::new(),
            push_queue: Vec::new(),
            calendar_metrics: DashboardCalendarMetrics {
                reading_info: json!({ "read": 42 }),
                read_time_week: json!({ "now": { "totalTime": 5_400_000 } }),
                day_read_today: json!([{ "id": 1 }, { "id": 2 }]),
            },
            upload: DashboardUploadState {
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
            },
            fetched_at_ms: 0,
        }
    }

    #[test]
    fn reading_metrics_label_uses_aggregated_values() {
        let snapshot = build_snapshot(true);
        assert_eq!(
            build_reading_metrics_label(&snapshot),
            "阅读统计指标: 今日2 本周1h30m 累计42"
        );
    }

    #[test]
    fn reading_metrics_label_shows_unauthorized_state() {
        let snapshot = build_snapshot(false);
        assert_eq!(
            build_reading_metrics_label(&snapshot),
            "阅读统计指标: 未授权"
        );
    }
}
