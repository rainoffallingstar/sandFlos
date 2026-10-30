use crate::api::{
    auth_source_text, create_client, fetch_auth_context_and_user, fetch_day_read, fetch_devices,
    fetch_push_queue_for_dashboard, fetch_read_time_info, fetch_reading_info, fetch_storage,
};
use crate::device::{build_dashboard_devices, fetch_share_devices};
use crate::models::{
    DashboardAuth, DashboardCalendarMetrics, DashboardProfile, DashboardSnapshot, DashboardStorage,
    DashboardUploadState,
};
use crate::state::{get_dashboard_cache, get_upload_runtime_state, set_dashboard_cache};
use crate::util::{
    parse_u64_field, reading_today_count, reading_total_count, reading_week_total_ms,
    short_duration_text, today_ymd, unix_ms_now,
};

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

pub fn build_dashboard_snapshot() -> DashboardSnapshot {
    let client = match create_client(30) {
        Ok(client) => client,
        Err(err) => {
            return unauthorized_dashboard_snapshot(format!("创建网络客户端失败: {err}"));
        }
    };

    let (auth, user) = match fetch_auth_context_and_user(&client) {
        Ok(value) => value,
        Err(err) => return unauthorized_dashboard_snapshot(err.to_string()),
    };

    let storage_value = fetch_storage(&client, &auth.bearer).unwrap_or(serde_json::Value::Null);
    let storage_used = parse_u64_field(&storage_value, "usedSize")
        .or_else(|| parse_u64_field(&storage_value, "used"))
        .or(auth.storage_used);
    let storage_limit = parse_u64_field(&storage_value, "totalSize")
        .or_else(|| parse_u64_field(&storage_value, "storageLimit"))
        .or(auth.storage_limit);
    let storage_percent = match (storage_used, storage_limit) {
        (Some(used), Some(limit)) if limit > 0 => Some(used as f64 * 100.0 / limit as f64),
        _ => None,
    };

    let reading_info = fetch_reading_info(&client, &auth.bearer).unwrap_or(serde_json::Value::Null);
    let today = today_ymd();
    let read_time_week = fetch_read_time_info(&client, &auth.bearer, &today, "week")
        .unwrap_or(serde_json::Value::Null);
    let day_read_today =
        fetch_day_read(&client, &auth.bearer, &today, &today).unwrap_or(serde_json::Value::Null);

    let raw_devices = fetch_devices(&client, &auth.bearer).unwrap_or(serde_json::Value::Null);
    let share_devices = fetch_share_devices(&auth.uid);
    let devices = build_dashboard_devices(raw_devices, share_devices);

    let push_queue = fetch_push_queue_for_dashboard(&client, &auth).unwrap_or_default();

    DashboardSnapshot {
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
        upload: current_upload_snapshot(),
        fetched_at_ms: unix_ms_now(),
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
