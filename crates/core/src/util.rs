use chrono::Local;
use serde_json::Value;
use std::time::{SystemTime, UNIX_EPOCH};

pub fn unix_ms_now() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

pub fn today_ymd() -> String {
    Local::now().format("%Y-%m-%d").to_string()
}

pub fn normalize_optional(value: Option<String>) -> Option<String> {
    value.and_then(|item| {
        let trimmed = item.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    })
}

pub fn parse_u64_field(value: &Value, key: &str) -> Option<u64> {
    let field = value.get(key)?;
    if let Some(number) = field.as_u64() {
        return Some(number);
    }
    if let Some(number) = field.as_i64() {
        return u64::try_from(number).ok();
    }
    field.as_str().and_then(|raw| raw.parse::<u64>().ok())
}

/// Read the account storage totals from the `/statistics/v2/user/storage` payload.
///
/// The live endpoint does not return `usedSize`/`totalSize`; it returns one
/// entry per category (`{"doc":{"data":5880440},...,"left":{"data":6882379303}}`).
/// The used figure is the sum of every category except `left`, and `left` is
/// the remaining space, so the quota is `used + left`. Older payloads that do
/// carry `usedSize`/`totalSize` keep working through the first branch.
pub fn storage_totals(value: &Value) -> Option<(u64, u64)> {
    let direct_used = parse_u64_field(value, "usedSize").or_else(|| parse_u64_field(value, "used"));
    let direct_limit =
        parse_u64_field(value, "totalSize").or_else(|| parse_u64_field(value, "storageLimit"));
    if direct_used.is_some() || direct_limit.is_some() {
        let used = direct_used?;
        let limit = direct_limit.or_else(|| parse_u64_field(value, "left").map(|l| used + l))?;
        return Some((used, limit));
    }

    let object = value.as_object()?;
    let mut used = 0_u64;
    let mut seen_category = false;
    let mut leftover = None;
    for (key, entry) in object {
        let Some(amount) = entry.get("data").and_then(Value::as_u64) else {
            continue;
        };
        if key == "left" {
            leftover = Some(amount);
            continue;
        }
        seen_category = true;
        used = used.saturating_add(amount);
    }
    if !seen_category {
        return None;
    }
    Some((used, used.saturating_add(leftover.unwrap_or(0))))
}

pub fn parse_bool_field(value: &Value, key: &str) -> Option<bool> {
    let field = value.get(key)?;
    if let Some(raw) = field.as_bool() {
        return Some(raw);
    }
    if let Some(raw) = field.as_i64() {
        return Some(raw != 0);
    }
    if let Some(raw) = field.as_u64() {
        return Some(raw != 0);
    }
    field.as_str().map(|raw| {
        matches!(
            raw.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on" | "online"
        )
    })
}

pub fn json_field_to_string(value: &Value, key: &str) -> Option<String> {
    let field = value.get(key)?;
    if let Some(text) = field.as_str() {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return None;
        }
        return Some(trimmed.to_string());
    }
    if let Some(num) = field.as_i64() {
        return Some(num.to_string());
    }
    if let Some(num) = field.as_u64() {
        return Some(num.to_string());
    }
    None
}

pub fn value_to_array(value: Value) -> Vec<Value> {
    if let Some(items) = value.as_array() {
        return items.clone();
    }
    for key in ["rows", "list", "devices", "results"] {
        if let Some(items) = value.get(key).and_then(Value::as_array) {
            return items.clone();
        }
    }
    Vec::new()
}

pub fn value_to_i64(value: &Value) -> Option<i64> {
    if let Some(raw) = value.as_i64() {
        return Some(raw);
    }
    if let Some(raw) = value.as_u64() {
        return i64::try_from(raw).ok();
    }
    if let Some(raw) = value.as_f64() {
        if raw.is_finite() {
            return Some(raw.round() as i64);
        }
    }
    value.as_str().and_then(|raw| raw.parse::<i64>().ok())
}

pub fn object_field_i64(value: &Value, key: &str) -> Option<i64> {
    value.get(key).and_then(value_to_i64)
}

pub fn reading_today_count(day_read_today: &Value) -> i64 {
    if let Some(items) = day_read_today.as_array() {
        return items.len() as i64;
    }
    if let Some(items) = day_read_today.get("list").and_then(Value::as_array) {
        return items.len() as i64;
    }
    if let Some(items) = day_read_today.get("rows").and_then(Value::as_array) {
        return items.len() as i64;
    }
    object_field_i64(day_read_today, "read")
        .or_else(|| object_field_i64(day_read_today, "count"))
        .or_else(|| object_field_i64(day_read_today, "total"))
        .unwrap_or(0)
}

pub fn reading_week_total_ms(read_time_week: &Value) -> i64 {
    if let Some(now) = read_time_week.get("now") {
        if let Some(value) = object_field_i64(now, "totalTime") {
            return value.max(0);
        }
    }
    object_field_i64(read_time_week, "totalTime")
        .or_else(|| object_field_i64(read_time_week, "weekTotalTime"))
        .unwrap_or(0)
        .max(0)
}

pub fn reading_total_count(reading_info: &Value) -> i64 {
    object_field_i64(reading_info, "read").unwrap_or(0).max(0)
}

pub fn short_duration_text(ms: i64) -> String {
    if ms <= 0 {
        return "0m".to_string();
    }
    let seconds = ms / 1000;
    let hours = seconds / 3600;
    let minutes = (seconds % 3600) / 60;
    if hours > 0 {
        format!("{hours}h{minutes}m")
    } else {
        format!("{minutes}m")
    }
}

pub fn truncate_menu_title(value: &str) -> String {
    let mut out = String::new();
    for (idx, ch) in value.chars().enumerate() {
        if idx >= 30 {
            out.push('…');
            break;
        }
        out.push(ch);
    }
    out
}

pub fn bytes_to_text(value: u64) -> String {
    if value == 0 {
        return "0 B".to_string();
    }
    let units = ["B", "KB", "MB", "GB", "TB"];
    let mut idx = 0;
    let mut cur = value as f64;
    while cur >= 1024.0 && idx < units.len() - 1 {
        cur /= 1024.0;
        idx += 1;
    }
    if idx > 1 {
        format!("{:.2} {}", cur, units[idx])
    } else {
        format!("{:.0} {}", cur, units[idx])
    }
}

pub fn duration_text(ms: i64) -> String {
    if ms <= 0 {
        return "-".to_string();
    }
    let seconds = ms / 1000;
    let hours = seconds / 3600;
    let minutes = (seconds % 3600) / 60;
    if hours > 0 {
        format!("{hours}h {minutes}m")
    } else {
        format!("{minutes}m")
    }
}

pub fn time_ago_text(ts: u128) -> String {
    if ts == 0 {
        return "刚刚".to_string();
    }
    let now = unix_ms_now();
    let diff = now.saturating_sub(ts) / 1000;
    if diff < 5 {
        "刚刚".to_string()
    } else if diff < 60 {
        format!("{diff}s 前")
    } else if diff < 3600 {
        format!("{}m 前", diff / 60)
    } else if diff < 86400 {
        format!("{}h 前", diff / 3600)
    } else {
        format!("{}d 前", diff / 86400)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn storage_totals_sums_categories_and_derives_the_quota_from_leftover() {
        // Shape captured from the live endpoint on 2026-11-08.
        let payload = json!({
            "doc": {"name": "doc", "data": 5880440_u64},
            "appData": {"name": "appDate", "data": 0_u64},
            "push": {"name": "push", "data": 3778898256_u64},
            "reader": {"name": "reader", "data": 70260241_u64},
            "share": {"name": "share", "data": 0_u64},
            "other": {"name": "other", "data": 0_u64},
            "left": {"name": "left", "data": 6882379303_u64}
        });
        let (used, limit) = storage_totals(&payload).expect("category payload");
        assert_eq!(used, 3855038937);
        assert_eq!(limit, 10737418240);
    }

    #[test]
    fn storage_totals_still_reads_legacy_flat_fields() {
        let payload = json!({"usedSize": 1000_u64, "totalSize": 4000_u64});
        assert_eq!(storage_totals(&payload), Some((1000, 4000)));
    }

    #[test]
    fn storage_totals_rejects_payloads_without_any_category() {
        assert_eq!(storage_totals(&json!({"left": {"data": 5_u64}})), None);
        assert_eq!(storage_totals(&json!(null)), None);
    }
}
