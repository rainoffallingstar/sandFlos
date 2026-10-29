use crate::api::create_client;
use crate::models::{
    DashboardSnapshot, ZoteroAttachmentSummary, ZoteroConnectionState, ZoteroConnectionSummary,
    ZoteroDetectionResult, ZoteroItemSummary, ZoteroSaveInput,
};
use crate::push::{clear_upload_transfer_metrics, set_upload_progress_label};
use crate::state::{finish_upload_task, try_begin_upload_task, update_upload_runtime_state};
use crate::util::{normalize_optional, unix_ms_now};
use keyring::{Entry, Error as KeyringError};
use reqwest::{blocking::Client, Method};
use rusqlite::{params, Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{Cursor, Read, Write},
    path::{Path, PathBuf},
};
use url::Url;
use uuid::Uuid;
use zip::ZipArchive;

const ZOTERO_CONFIG_FILE: &str = "zotero_config.json";
const KEYRING_SERVICE: &str = "com.fallingstar.send2boox.zotero.webdav";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ZoteroConfigFile {
    profile_dir: Option<String>,
    data_dir: Option<String>,
    database_path: Option<String>,
    webdav_url: Option<String>,
    webdav_username: Option<String>,
    protocol: Option<String>,
    webdav_verified: bool,
    download_mode_personal: Option<String>,
    download_mode_groups: Option<String>,
    detected_at_ms: Option<u128>,
    validated_at_ms: Option<u128>,
    last_error: Option<String>,
}

#[derive(Debug, Clone)]
struct AttachmentResolution {
    local_path: PathBuf,
    cleanup_dir: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct AttachmentRecord {
    attachment_item_id: i64,
    attachment_key: String,
    file_name: Option<String>,
    content_type: Option<String>,
    link_mode: i64,
    local_path: Option<PathBuf>,
    local_exists: bool,
}

fn zotero_config_path() -> Option<PathBuf> {
    let dir = crate::state::app_data_dir();
    Some(dir.join(ZOTERO_CONFIG_FILE))
}

pub fn load_zotero_config() -> ZoteroConfigFile {
    let Some(path) = zotero_config_path() else {
        return ZoteroConfigFile::default();
    };
    let Ok(raw) = fs::read_to_string(path) else {
        return ZoteroConfigFile::default();
    };
    serde_json::from_str::<ZoteroConfigFile>(&raw).unwrap_or_default()
}

pub fn save_zotero_config(config: &ZoteroConfigFile) -> Result<(), String> {
    let path = zotero_config_path().ok_or_else(|| "无法定位 Zotero 配置目录".to_string())?;
    let text = serde_json::to_string(config).map_err(|err| err.to_string())?;
    fs::write(path, text).map_err(|err| err.to_string())
}

fn normalize_profile_dir(value: Option<String>) -> Option<String> {
    normalize_optional(value).map(|raw| expand_home_path(&raw).to_string_lossy().to_string())
}

fn normalize_data_dir(value: Option<String>) -> Option<String> {
    normalize_optional(value).map(|raw| expand_home_path(&raw).to_string_lossy().to_string())
}

fn normalize_webdav_url(value: Option<String>) -> Option<String> {
    let raw = normalize_optional(value)?;
    let with_scheme = if raw.contains("://") {
        raw
    } else {
        format!("https://{raw}")
    };
    let mut parsed = Url::parse(&with_scheme).ok()?;
    let mut segments = parsed
        .path_segments()
        .map(|parts| {
            parts
                .filter(|part| !part.is_empty())
                .map(|part| part.to_string())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if segments.last().map(|value| value.as_str()) != Some("zotero") {
        segments.push("zotero".to_string());
    }
    let normalized_path = if segments.is_empty() {
        "/zotero".to_string()
    } else {
        format!("/{}", segments.join("/"))
    };
    parsed.set_path(&normalized_path);
    parsed.set_query(None);
    parsed.set_fragment(None);
    let mut normalized = parsed.to_string();
    while normalized.ends_with('/') {
        normalized.pop();
    }
    Some(normalized)
}

fn webdav_url_aliases(url: &str) -> Vec<String> {
    let Some(mut parsed) = Url::parse(url).ok() else {
        return vec![url.to_string()];
    };
    let segments = parsed
        .path_segments()
        .map(|parts| {
            parts
                .filter(|part| !part.is_empty())
                .map(|part| part.to_string())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let mut aliases = vec![url.to_string()];
    if segments.last().map(|value| value.as_str()) == Some("zotero") {
        let parent_segments = &segments[..segments.len().saturating_sub(1)];
        let parent_path = if parent_segments.is_empty() {
            "/".to_string()
        } else {
            format!("/{}", parent_segments.join("/"))
        };
        parsed.set_path(&parent_path);
        parsed.set_query(None);
        parsed.set_fragment(None);
        aliases.push(parsed.to_string().trim_end_matches('/').to_string());
    }
    aliases.sort();
    aliases.dedup();
    aliases
}

fn webdav_download_base_urls(url: &str) -> Vec<String> {
    let mut bases = Vec::new();
    if let Some(canonical) = normalize_webdav_url(Some(url.to_string())) {
        bases.push(canonical);
    }
    for alias in webdav_url_aliases(url) {
        let candidate = alias.trim_end_matches('/').to_string();
        if !candidate.is_empty() && !bases.contains(&candidate) {
            bases.push(candidate);
        }
    }
    bases
}

fn expand_home_path(value: &str) -> PathBuf {
    if let Some(rest) = value.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    PathBuf::from(value)
}

fn build_database_path(data_dir: Option<&str>) -> Option<String> {
    data_dir.map(|dir| {
        PathBuf::from(dir)
            .join("zotero.sqlite")
            .to_string_lossy()
            .to_string()
    })
}

#[cfg(target_os = "macos")]
fn default_zotero_root() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(
        PathBuf::from(home)
            .join("Library")
            .join("Application Support")
            .join("Zotero"),
    )
}

fn parse_profiles_ini_default_profile(root: &Path) -> Result<PathBuf, String> {
    let path = root.join("profiles.ini");
    let content =
        fs::read_to_string(&path).map_err(|err| format!("读取 profiles.ini 失败: {err}"))?;
    let mut current_path = None::<String>;
    let mut is_relative = true;
    let mut is_default = false;
    let mut fallback = None::<PathBuf>;

    for raw_line in content.lines() {
        let line = raw_line.trim();
        if line.starts_with('[') && line.ends_with(']') {
            if let Some(profile_path) = current_path.take() {
                let resolved = resolve_profile_path(root, &profile_path, is_relative);
                if fallback.is_none() {
                    fallback = Some(resolved.clone());
                }
                if is_default {
                    return Ok(resolved);
                }
            }
            is_relative = true;
            is_default = false;
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            let value = value.trim();
            match key.trim() {
                "Path" => current_path = Some(value.to_string()),
                "IsRelative" => is_relative = value != "0",
                "Default" => is_default = value == "1",
                _ => {}
            }
        }
    }

    if let Some(profile_path) = current_path {
        let resolved = resolve_profile_path(root, &profile_path, is_relative);
        if is_default {
            return Ok(resolved);
        }
        if fallback.is_none() {
            fallback = Some(resolved);
        }
    }

    fallback.ok_or_else(|| "profiles.ini 中未找到可用 profile".to_string())
}

fn resolve_profile_path(root: &Path, profile_path: &str, is_relative: bool) -> PathBuf {
    if is_relative {
        root.join(profile_path)
    } else {
        PathBuf::from(profile_path)
    }
}

fn parse_pref_value(content: &str, key: &str) -> Option<String> {
    let needle = format!("user_pref(\"{key}\",");
    for raw_line in content.lines() {
        let line = raw_line.trim();
        if !line.starts_with(&needle) {
            continue;
        }
        let suffix = line
            .strip_prefix(&needle)?
            .trim()
            .trim_end_matches(");")
            .trim();
        if suffix.starts_with('"') && suffix.ends_with('"') && suffix.len() >= 2 {
            return Some(
                suffix[1..suffix.len() - 1]
                    .replace("\\\"", "\"")
                    .replace("\\\\", "\\"),
            );
        }
        return Some(suffix.to_string());
    }
    None
}

fn parse_pref_bool(content: &str, key: &str) -> Option<bool> {
    parse_pref_value(content, key).and_then(|raw| match raw.trim() {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    })
}

fn detect_from_profile_dir(profile_dir: &Path) -> ZoteroDetectionResult {
    let prefs_path = profile_dir.join("prefs.js");
    let mut result = ZoteroDetectionResult {
        profile_dir: Some(profile_dir.to_string_lossy().to_string()),
        profile_source: Some("profile_dir".to_string()),
        detected_at_ms: Some(unix_ms_now()),
        ..ZoteroDetectionResult::default()
    };
    let prefs_content = match fs::read_to_string(&prefs_path) {
        Ok(value) => value,
        Err(err) => {
            result.issues.push(format!("读取 prefs.js 失败: {err}"));
            return result;
        }
    };

    let use_data_dir =
        parse_pref_bool(&prefs_content, "extensions.zotero.useDataDir").unwrap_or(false);
    let data_dir = if use_data_dir {
        normalize_data_dir(parse_pref_value(
            &prefs_content,
            "extensions.zotero.dataDir",
        ))
    } else {
        None
    }
    .or_else(|| {
        let fallback = profile_dir.join("zotero.sqlite");
        fallback
            .exists()
            .then(|| profile_dir.to_string_lossy().to_string())
    });

    let database_path = build_database_path(data_dir.as_deref());
    let database_exists = database_path
        .as_ref()
        .map(|path| Path::new(path).is_file())
        .unwrap_or(false);

    result.data_dir = data_dir.clone();
    result.data_dir_source = if use_data_dir && data_dir.is_some() {
        Some("prefs.js:dataDir".to_string())
    } else if data_dir.is_some() {
        Some("profile_dir:fallback".to_string())
    } else {
        None
    };
    result.database_path = database_path;
    result.database_exists = database_exists;
    result.protocol = normalize_optional(parse_pref_value(
        &prefs_content,
        "extensions.zotero.sync.storage.protocol",
    ));
    result.protocol_source = result.protocol.as_ref().map(|_| "prefs.js".to_string());
    result.protocol_is_webdav = result.protocol.as_deref() == Some("webdav");
    result.webdav_url = normalize_webdav_url(parse_pref_value(
        &prefs_content,
        "extensions.zotero.sync.storage.url",
    ));
    result.webdav_url_source = result.webdav_url.as_ref().map(|_| "prefs.js".to_string());
    result.webdav_username = normalize_optional(parse_pref_value(
        &prefs_content,
        "extensions.zotero.sync.storage.username",
    ));
    result.webdav_username_source = result
        .webdav_username
        .as_ref()
        .map(|_| "prefs.js".to_string());
    result.webdav_verified =
        parse_pref_bool(&prefs_content, "extensions.zotero.sync.storage.verified").unwrap_or(false);
    result.download_mode_personal = normalize_optional(parse_pref_value(
        &prefs_content,
        "extensions.zotero.sync.storage.downloadMode.personal",
    ));
    result.download_mode_groups = normalize_optional(parse_pref_value(
        &prefs_content,
        "extensions.zotero.sync.storage.downloadMode.groups",
    ));

    if result.data_dir.is_none() {
        result
            .issues
            .push("未在 prefs.js 中找到可用 dataDir".to_string());
    }
    if !result.database_exists {
        result.issues.push("zotero.sqlite 不存在".to_string());
    }
    if !result.protocol_is_webdav {
        result
            .issues
            .push("当前附件同步协议不是 WebDAV".to_string());
    }

    result
}

fn keyring_account(url: &str, username: &str) -> String {
    format!("{url}|{username}")
}

fn webdav_entry(url: &str, username: &str) -> Result<Entry, String> {
    Entry::new(KEYRING_SERVICE, &keyring_account(url, username)).map_err(|err| err.to_string())
}

fn password_saved_for(url: Option<&str>, username: Option<&str>) -> bool {
    match (url, username) {
        (Some(url), Some(username)) => webdav_url_aliases(url).into_iter().any(|candidate| {
            webdav_entry(&candidate, username)
                .map(|entry| !matches!(entry.get_password(), Err(KeyringError::NoEntry)))
                .unwrap_or(false)
        }),
        _ => false,
    }
}

fn load_saved_password(url: &str, username: &str) -> Result<Option<String>, String> {
    for candidate in webdav_url_aliases(url) {
        let entry = webdav_entry(&candidate, username)?;
        match entry.get_password() {
            Ok(password) => return Ok(Some(password)),
            Err(KeyringError::NoEntry) => continue,
            Err(err) => return Err(err.to_string()),
        }
    }
    Ok(None)
}

fn save_password(url: &str, username: &str, password: &str) -> Result<(), String> {
    let entry = webdav_entry(url, username)?;
    entry.set_password(password).map_err(|err| err.to_string())
}

fn delete_password(url: &str, username: &str) -> Result<(), String> {
    for candidate in webdav_url_aliases(url) {
        let entry = webdav_entry(&candidate, username)?;
        match entry.delete_credential() {
            Ok(()) | Err(KeyringError::NoEntry) => {}
            Err(err) => return Err(err.to_string()),
        }
    }
    Ok(())
}

pub fn detect_config_inner() -> Result<ZoteroDetectionResult, String> {
    #[cfg(not(target_os = "macos"))]
    {
        Err("当前版本只支持 macOS 自动检测 Zotero 配置".to_string())
    }
    #[cfg(target_os = "macos")]
    {
        let root = default_zotero_root().ok_or_else(|| "无法定位用户主目录".to_string())?;
        let profile_dir = parse_profiles_ini_default_profile(&root)?;
        let mut detection = detect_from_profile_dir(&profile_dir);
        detection.has_saved_password = password_saved_for(
            detection.webdav_url.as_deref(),
            detection.webdav_username.as_deref(),
        );
        let mut config = load_zotero_config();
        merge_detection_into_config(&mut config, &detection);
        config.last_error = None;
        save_zotero_config(&config)?;
        Ok(detection)
    }
}

fn merge_detection_into_config(config: &mut ZoteroConfigFile, detection: &ZoteroDetectionResult) {
    config.profile_dir = detection.profile_dir.clone();
    config.data_dir = detection.data_dir.clone();
    config.database_path = detection.database_path.clone();
    config.webdav_url = detection.webdav_url.clone();
    config.webdav_username = detection.webdav_username.clone();
    config.protocol = detection.protocol.clone();
    config.webdav_verified = detection.webdav_verified;
    config.download_mode_personal = detection.download_mode_personal.clone();
    config.download_mode_groups = detection.download_mode_groups.clone();
    config.detected_at_ms = detection.detected_at_ms;
}

fn connection_summary_from_config(config: &ZoteroConfigFile) -> ZoteroConnectionSummary {
    let normalized_webdav_url = normalize_webdav_url(config.webdav_url.clone());
    let password_saved = password_saved_for(
        normalized_webdav_url
            .as_deref()
            .or(config.webdav_url.as_deref()),
        config.webdav_username.as_deref(),
    );
    ZoteroConnectionSummary {
        profile_dir: config.profile_dir.clone(),
        data_dir: config.data_dir.clone(),
        database_path: config.database_path.clone(),
        database_exists: config
            .database_path
            .as_ref()
            .map(|path| Path::new(path).is_file())
            .unwrap_or(false),
        webdav_url: normalized_webdav_url,
        webdav_username: config.webdav_username.clone(),
        protocol: config.protocol.clone(),
        protocol_is_webdav: config.protocol.as_deref() == Some("webdav"),
        webdav_verified: config.webdav_verified,
        password_saved,
        download_mode_personal: config.download_mode_personal.clone(),
        download_mode_groups: config.download_mode_groups.clone(),
    }
}

fn derive_missing_fields(summary: &ZoteroConnectionSummary) -> Vec<String> {
    let mut missing = Vec::new();
    if summary.profile_dir.is_none() {
        missing.push("profile_dir".to_string());
        return missing;
    }
    if summary.data_dir.is_none() || !summary.database_exists {
        missing.push("data_dir".to_string());
        return missing;
    }
    if !summary.protocol_is_webdav {
        missing.push("webdav_protocol".to_string());
        return missing;
    }
    if summary.webdav_url.is_none() {
        missing.push("webdav_url".to_string());
    }
    if summary.webdav_username.is_none() {
        missing.push("webdav_username".to_string());
    }
    if !summary.password_saved {
        missing.push("webdav_password".to_string());
    }
    missing
}

fn build_connection_state(config: &ZoteroConfigFile) -> ZoteroConnectionState {
    let summary = connection_summary_from_config(config);
    let missing_fields = derive_missing_fields(&summary);
    let state = if config.detected_at_ms.is_none()
        && summary.profile_dir.is_none()
        && summary.data_dir.is_none()
        && summary.webdav_url.is_none()
    {
        "undetected".to_string()
    } else if !missing_fields.is_empty() {
        "pending".to_string()
    } else if config.last_error.is_some() {
        "failed".to_string()
    } else if config.validated_at_ms.is_some() {
        "connected".to_string()
    } else {
        "pending".to_string()
    };
    ZoteroConnectionState {
        state,
        missing_fields,
        summary,
        detected_at_ms: config.detected_at_ms,
        validated_at_ms: config.validated_at_ms,
        last_error: config.last_error.clone(),
    }
}

fn validate_webdav(url: &str, username: &str, password: &str) -> Result<(), String> {
    let client = create_client(20)?;
    let response = client
        .request(Method::OPTIONS, format!("{url}/"))
        .basic_auth(username, Some(password))
        .send()
        .map_err(|err| err.to_string())?;
    let status = response.status();
    if status.is_success() || status.is_redirection() || status.as_u16() == 405 {
        Ok(())
    } else {
        Err(format!("WebDAV 验证失败: HTTP {status}"))
    }
}

pub fn save_and_validate_inner(input: ZoteroSaveInput) -> Result<ZoteroConnectionState, String> {
    let mut config = load_zotero_config();
    let previous_url = config.webdav_url.clone();
    let previous_username = config.webdav_username.clone();

    if let Some(profile_dir) = normalize_profile_dir(input.profile_dir) {
        config.profile_dir = Some(profile_dir);
    }
    if let Some(data_dir) = normalize_data_dir(input.data_dir) {
        config.data_dir = Some(data_dir.clone());
        config.database_path = build_database_path(Some(&data_dir));
    }
    if let Some(webdav_url) = normalize_webdav_url(input.webdav_url) {
        config.webdav_url = Some(webdav_url);
    }
    if let Some(username) = normalize_optional(input.webdav_username) {
        config.webdav_username = Some(username);
    }
    if config.database_path.is_none() {
        config.database_path = build_database_path(config.data_dir.as_deref());
    }
    if config.protocol.is_none() {
        config.protocol = Some("webdav".to_string());
    }

    let summary_before_validation = connection_summary_from_config(&config);
    let missing_before_validation = derive_missing_fields(&summary_before_validation)
        .into_iter()
        .filter(|field| field != "webdav_password")
        .collect::<Vec<_>>();
    if !missing_before_validation.is_empty() {
        config.last_error = Some("配置尚未补全".to_string());
        save_zotero_config(&config)?;
        return Ok(build_connection_state(&config));
    }

    let url = config
        .webdav_url
        .clone()
        .ok_or_else(|| "WebDAV 地址不能为空".to_string())?;
    let username = config
        .webdav_username
        .clone()
        .ok_or_else(|| "WebDAV 用户名不能为空".to_string())?;
    let password_input = normalize_optional(input.webdav_password);
    if password_input.is_none() && !password_saved_for(Some(&url), Some(&username)) {
        config.last_error = None;
        save_zotero_config(&config)?;
        return Ok(build_connection_state(&config));
    }
    let password = if let Some(password) = password_input.as_deref() {
        password.to_string()
    } else {
        load_saved_password(&url, &username)?.ok_or_else(|| "请先补全 WebDAV 密码".to_string())?
    };

    if let Err(err) = validate_webdav(&url, &username, &password) {
        config.last_error = Some(err.clone());
        save_zotero_config(&config)?;
        return Err(err);
    }
    save_password(&url, &username, &password)?;

    if let (Some(old_url), Some(old_username)) = (previous_url, previous_username) {
        if (old_url != url || old_username != username)
            && !(old_url == url && old_username == username)
        {
            let _ = delete_password(&old_url, &old_username);
        }
    }

    config.webdav_verified = true;
    config.validated_at_ms = Some(unix_ms_now());
    config.last_error = None;
    save_zotero_config(&config)?;
    Ok(build_connection_state(&config))
}

pub fn detect_from_selected_path(selected_path: &Path) -> Result<ZoteroDetectionResult, String> {
    let profile_dir = if selected_path.join("prefs.js").is_file() {
        selected_path.to_path_buf()
    } else if selected_path.join("profiles.ini").is_file() {
        parse_profiles_ini_default_profile(selected_path)?
    } else {
        return Err("所选目录不是 Zotero profile 或 Zotero 根目录".to_string());
    };
    let mut detection = detect_from_profile_dir(&profile_dir);
    detection.has_saved_password = password_saved_for(
        detection.webdav_url.as_deref(),
        detection.webdav_username.as_deref(),
    );
    let mut config = load_zotero_config();
    merge_detection_into_config(&mut config, &detection);
    config.last_error = None;
    save_zotero_config(&config)?;
    Ok(detection)
}

pub fn detect_from_selected_data_dir(
    selected_path: &Path,
) -> Result<ZoteroDetectionResult, String> {
    if !selected_path.join("zotero.sqlite").is_file() {
        return Err("所选目录中未找到 zotero.sqlite".to_string());
    }
    let mut config = load_zotero_config();
    config.data_dir = Some(selected_path.to_string_lossy().to_string());
    config.database_path = Some(
        selected_path
            .join("zotero.sqlite")
            .to_string_lossy()
            .to_string(),
    );
    config.detected_at_ms = Some(unix_ms_now());
    save_zotero_config(&config)?;
    Ok(ZoteroDetectionResult {
        profile_dir: config.profile_dir.clone(),
        profile_source: config.profile_dir.as_ref().map(|_| "manual".to_string()),
        data_dir: config.data_dir.clone(),
        data_dir_source: Some("manual".to_string()),
        database_path: config.database_path.clone(),
        database_exists: true,
        webdav_url: config.webdav_url.clone(),
        webdav_url_source: config.webdav_url.as_ref().map(|_| "saved".to_string()),
        webdav_username: config.webdav_username.clone(),
        webdav_username_source: config.webdav_username.as_ref().map(|_| "saved".to_string()),
        protocol: config.protocol.clone(),
        protocol_source: config.protocol.as_ref().map(|_| "saved".to_string()),
        protocol_is_webdav: config.protocol.as_deref() == Some("webdav"),
        webdav_verified: config.webdav_verified,
        download_mode_personal: config.download_mode_personal.clone(),
        download_mode_groups: config.download_mode_groups.clone(),
        has_saved_password: password_saved_for(
            config.webdav_url.as_deref(),
            config.webdav_username.as_deref(),
        ),
        detected_at_ms: config.detected_at_ms,
        issues: Vec::new(),
    })
}

fn create_temp_dir(prefix: &str) -> Result<PathBuf, String> {
    let dir = std::env::temp_dir().join(format!("{prefix}-{}", Uuid::new_v4()));
    fs::create_dir_all(&dir).map_err(|err| err.to_string())?;
    Ok(dir)
}

fn open_copied_database(data_dir: &str) -> Result<(PathBuf, Connection), String> {
    let source = PathBuf::from(data_dir).join("zotero.sqlite");
    if !source.is_file() {
        return Err("zotero.sqlite 不存在".to_string());
    }
    let temp_dir = create_temp_dir("send2boox-zotero-db")?;
    let copied = temp_dir.join("zotero.sqlite");
    fs::copy(&source, &copied).map_err(|err| err.to_string())?;
    let conn = Connection::open_with_flags(copied, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|err| err.to_string())?;
    Ok((temp_dir, conn))
}

fn fetch_field_value(conn: &Connection, item_id: i64, field_name: &str) -> Option<String> {
    conn.query_row(
        "SELECT v.value
         FROM itemData d
         JOIN fields f ON f.fieldID = d.fieldID
         JOIN itemDataValues v ON v.valueID = d.valueID
         WHERE d.itemID = ?1 AND f.fieldName = ?2
         LIMIT 1",
        params![item_id, field_name],
        |row| row.get::<_, String>(0),
    )
    .ok()
    .and_then(|value| normalize_optional(Some(value)))
}

fn fetch_author_summary(conn: &Connection, item_id: i64) -> Result<Option<String>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT c.firstName, c.lastName, c.fieldMode
             FROM itemCreators ic
             JOIN creators c ON c.creatorID = ic.creatorID
             WHERE ic.itemID = ?1
             ORDER BY ic.orderIndex ASC",
        )
        .map_err(|err| err.to_string())?;
    let rows = stmt
        .query_map(params![item_id], |row| {
            let first_name = row.get::<_, Option<String>>(0)?;
            let last_name = row.get::<_, Option<String>>(1)?;
            let field_mode = row.get::<_, i64>(2)?;
            Ok((first_name, last_name, field_mode))
        })
        .map_err(|err| err.to_string())?;
    let mut authors = Vec::new();
    for row in rows {
        let (first_name, last_name, field_mode) = row.map_err(|err| err.to_string())?;
        let name = if field_mode == 1 {
            normalize_optional(last_name)
        } else {
            let first = normalize_optional(first_name).unwrap_or_default();
            let last = normalize_optional(last_name).unwrap_or_default();
            normalize_optional(Some(format!("{last}{first}")))
        };
        if let Some(name) = name {
            authors.push(name);
        }
    }
    if authors.is_empty() {
        Ok(None)
    } else if authors.len() <= 2 {
        Ok(Some(authors.join("、")))
    } else {
        Ok(Some(format!(
            "{} 等 {} 位",
            authors[..2].join("、"),
            authors.len()
        )))
    }
}

fn extract_year(value: Option<String>) -> Option<String> {
    let raw = value?;
    let chars = raw
        .chars()
        .skip_while(|ch| !ch.is_ascii_digit())
        .take(4)
        .collect::<String>();
    if chars.len() == 4 {
        Some(chars)
    } else {
        None
    }
}

fn resolve_attachment_local_path(
    data_dir: &str,
    attachment_key: &str,
    attachment_path: Option<&str>,
) -> Option<PathBuf> {
    let attachment_path = attachment_path?;
    let relative = attachment_path.strip_prefix("storage:")?;
    Some(
        PathBuf::from(data_dir)
            .join("storage")
            .join(attachment_key)
            .join(relative),
    )
}

fn attachment_file_name(attachment_path: Option<&str>) -> Option<String> {
    attachment_path
        .and_then(|path| path.strip_prefix("storage:"))
        .and_then(|value| Path::new(value).file_name())
        .and_then(|value| value.to_str())
        .map(|value| value.to_string())
}

fn fetch_attachment_records(
    conn: &Connection,
    data_dir: &str,
    parent_item_id: i64,
) -> Result<Vec<AttachmentRecord>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT ia.itemID, child.key, ia.path, ia.contentType, ia.linkMode
             FROM itemAttachments ia
             JOIN items child ON child.itemID = ia.itemID
             WHERE ia.parentItemID = ?1
             ORDER BY child.dateModified DESC",
        )
        .map_err(|err| err.to_string())?;
    let rows = stmt
        .query_map(params![parent_item_id], |row| {
            let attachment_item_id = row.get::<_, i64>(0)?;
            let attachment_key = row.get::<_, String>(1)?;
            let attachment_path = row.get::<_, Option<String>>(2)?;
            let content_type = row.get::<_, Option<String>>(3)?;
            let link_mode = row.get::<_, i64>(4)?;
            Ok((
                attachment_item_id,
                attachment_key,
                attachment_path,
                content_type,
                link_mode,
            ))
        })
        .map_err(|err| err.to_string())?;
    let mut out = Vec::new();
    for row in rows {
        let (attachment_item_id, attachment_key, attachment_path, content_type, link_mode) =
            row.map_err(|err| err.to_string())?;
        let local_path =
            resolve_attachment_local_path(data_dir, &attachment_key, attachment_path.as_deref());
        let local_exists = local_path
            .as_ref()
            .map(|path| path.is_file())
            .unwrap_or(false);
        out.push(AttachmentRecord {
            attachment_item_id,
            attachment_key,
            file_name: attachment_file_name(attachment_path.as_deref()),
            content_type,
            link_mode,
            local_path,
            local_exists,
        });
    }
    Ok(out)
}

fn build_attachment_summary(
    record: AttachmentRecord,
    can_download_from_webdav: bool,
) -> ZoteroAttachmentSummary {
    let status_label = if record.link_mode != 0 {
        "当前附件不是 stored attachment".to_string()
    } else if record.local_exists {
        "本地附件可直接推送".to_string()
    } else if can_download_from_webdav {
        "本地缺失，可从 WebDAV 拉取".to_string()
    } else {
        "本地附件缺失".to_string()
    };
    ZoteroAttachmentSummary {
        attachment_item_id: record.attachment_item_id,
        attachment_key: record.attachment_key,
        file_name: record.file_name,
        content_type: record.content_type,
        link_mode: record.link_mode,
        local_exists: record.local_exists,
        local_path: record
            .local_path
            .as_ref()
            .map(|path| path.to_string_lossy().to_string()),
        can_push_directly: record.link_mode == 0 && record.local_exists,
        can_download_from_webdav: record.link_mode == 0
            && !record.local_exists
            && can_download_from_webdav,
        status_label,
    }
}

fn map_recent_item_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<(i64, String, String)> {
    Ok((
        row.get::<_, i64>(0)?,
        row.get::<_, String>(1)?,
        row.get::<_, String>(2)?,
    ))
}

fn build_zotero_item_summary(
    conn: &Connection,
    data_dir: &str,
    item_id: i64,
    item_key: String,
    date_modified: String,
    can_download_from_webdav: bool,
) -> Result<Option<ZoteroItemSummary>, String> {
    let title =
        fetch_field_value(conn, item_id, "title").unwrap_or_else(|| "未命名条目".to_string());
    let author_summary = fetch_author_summary(conn, item_id)?;
    let year = extract_year(fetch_field_value(conn, item_id, "date"));
    let attachments = fetch_attachment_records(conn, data_dir, item_id)?
        .into_iter()
        .map(|record| build_attachment_summary(record, can_download_from_webdav))
        .collect::<Vec<_>>();
    if attachments.is_empty() {
        return Ok(None);
    }
    Ok(Some(ZoteroItemSummary {
        item_id,
        item_key,
        title,
        author_summary,
        year,
        date_modified,
        attachments,
    }))
}

fn zotero_item_matches_query(item: &ZoteroItemSummary, query: &str) -> bool {
    let mut haystack = vec![
        item.title.clone(),
        item.author_summary.clone().unwrap_or_default(),
        item.year.clone().unwrap_or_default(),
        item.date_modified.clone(),
    ];
    for attachment in &item.attachments {
        haystack.push(attachment.file_name.clone().unwrap_or_default());
        haystack.push(attachment.attachment_key.clone());
        haystack.push(attachment.content_type.clone().unwrap_or_default());
        haystack.push(attachment.status_label.clone());
    }
    haystack.join(" ").to_lowercase().contains(query)
}

pub fn list_recent_items_inner(
    limit: Option<usize>,
    offset: Option<usize>,
    search: Option<String>,
) -> Result<Vec<ZoteroItemSummary>, String> {
    let config = load_zotero_config();
    let data_dir = config
        .data_dir
        .clone()
        .ok_or_else(|| "请先补全 Zotero 数据目录".to_string())?;
    let has_password = password_saved_for(
        config.webdav_url.as_deref(),
        config.webdav_username.as_deref(),
    );
    let can_download_from_webdav = config.protocol.as_deref() == Some("webdav")
        && config.webdav_url.is_some()
        && config.webdav_username.is_some()
        && has_password;
    list_items_at(&data_dir, limit, offset, search, can_download_from_webdav)
}

pub fn fetch_attachment_record_by_id(
    attachment_item_id: i64,
) -> Result<(ZoteroConfigFile, AttachmentRecord), String> {
    let config = load_zotero_config();
    let data_dir = config
        .data_dir
        .clone()
        .ok_or_else(|| "请先补全 Zotero 数据目录".to_string())?;
    let (temp_dir, conn) = open_copied_database(&data_dir)?;
    let record = conn
        .query_row(
            "SELECT ia.itemID, child.key, ia.path, ia.contentType, ia.linkMode
             FROM itemAttachments ia
             JOIN items child ON child.itemID = ia.itemID
             WHERE ia.itemID = ?1",
            params![attachment_item_id],
            |row| {
                let attachment_key = row.get::<_, String>(1)?;
                let attachment_path = row.get::<_, Option<String>>(2)?;
                Ok(AttachmentRecord {
                    attachment_item_id: row.get::<_, i64>(0)?,
                    attachment_key: attachment_key.clone(),
                    file_name: attachment_file_name(attachment_path.as_deref()),
                    content_type: row.get::<_, Option<String>>(3)?,
                    link_mode: row.get::<_, i64>(4)?,
                    local_path: resolve_attachment_local_path(
                        &data_dir,
                        &attachment_key,
                        attachment_path.as_deref(),
                    ),
                    local_exists: resolve_attachment_local_path(
                        &data_dir,
                        &attachment_key,
                        attachment_path.as_deref(),
                    )
                    .map(|path| path.is_file())
                    .unwrap_or(false),
                })
            },
        )
        .map_err(|err| err.to_string())?;
    let _ = fs::remove_dir_all(temp_dir);
    Ok((config, record))
}

fn download_remote_attachment(
    client: &Client,
    url: &str,
    username: &str,
    password: &str,
    attachment_key: &str,
    preferred_name: Option<&str>,
) -> Result<AttachmentResolution, String> {
    let candidate_urls = webdav_download_base_urls(url)
        .into_iter()
        .map(|base| format!("{base}/{attachment_key}.zip"))
        .collect::<Vec<_>>();
    let mut last_error = None::<String>;
    let bytes = {
        let mut found = None;
        for zip_url in candidate_urls {
            let response = client
                .get(&zip_url)
                .basic_auth(username, Some(password))
                .send()
                .map_err(|err| err.to_string())?;
            let status = response.status();
            if status.is_success() {
                found = Some(response.bytes().map_err(|err| err.to_string())?);
                break;
            }
            last_error = Some(format!("WebDAV 下载失败: HTTP {status}，{zip_url}"));
        }
        found.ok_or_else(|| {
            last_error.unwrap_or_else(|| "WebDAV 下载失败: 未命中可用的远端附件地址".to_string())
        })?
    };
    let temp_dir = create_temp_dir("send2boox-zotero-attachment")?;
    let mut archive =
        ZipArchive::new(Cursor::new(bytes.to_vec())).map_err(|err| err.to_string())?;
    for index in 0..archive.len() {
        let mut file = archive.by_index(index).map_err(|err| err.to_string())?;
        if file.is_dir() {
            continue;
        }
        let entry_name = file
            .enclosed_name()
            .and_then(|path| path.file_name().map(|name| name.to_owned()))
            .and_then(|value| value.to_str().map(|text| text.to_string()))
            .or_else(|| preferred_name.map(|value| value.to_string()))
            .unwrap_or_else(|| format!("{attachment_key}.bin"));
        if entry_name.starts_with('.') {
            continue;
        }
        let out_path = temp_dir.join(entry_name);
        let mut out_file = fs::File::create(&out_path).map_err(|err| err.to_string())?;
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer)
            .map_err(|err| err.to_string())?;
        out_file.write_all(&buffer).map_err(|err| err.to_string())?;
        return Ok(AttachmentResolution {
            local_path: out_path,
            cleanup_dir: Some(temp_dir),
        });
    }
    Err("WebDAV 附件压缩包中未找到可用文件".to_string())
}

fn resolve_attachment_for_push(
    config: &ZoteroConfigFile,
    record: &AttachmentRecord,
) -> Result<AttachmentResolution, String> {
    if record.link_mode != 0 {
        return Err("当前附件不是 stored attachment，暂不支持直接推送".to_string());
    }
    if record.local_exists {
        let local_path = record
            .local_path
            .clone()
            .ok_or_else(|| "未找到本地附件路径".to_string())?;
        return Ok(AttachmentResolution {
            local_path,
            cleanup_dir: None,
        });
    }
    let url = config
        .webdav_url
        .clone()
        .ok_or_else(|| "缺少 WebDAV 地址".to_string())?;
    let username = config
        .webdav_username
        .clone()
        .ok_or_else(|| "缺少 WebDAV 用户名".to_string())?;
    let password = load_saved_password(&url, &username)?
        .ok_or_else(|| "缺少已保存的 WebDAV 密码".to_string())?;
    let client = create_client(90)?;
    download_remote_attachment(
        &client,
        &url,
        &username,
        &password,
        &record.attachment_key,
        record.file_name.as_deref(),
    )
}

fn cleanup_resolution(resolution: &AttachmentResolution) {
    if let Some(dir) = &resolution.cleanup_dir {
        let _ = fs::remove_dir_all(dir);
    }
}

fn record_upload_failure(message: &str) {
    finish_upload_task();
    set_upload_progress_label("上传进度: 失败");
    clear_upload_transfer_metrics();
    let message = message.to_string();
    update_upload_runtime_state(move |state| {
        state.last_error = Some(message.clone());
    });
}

pub fn zotero_status() -> Result<ZoteroConnectionState, String> {
    Ok(build_connection_state(&load_zotero_config()))
}

pub fn zotero_push_attachment(attachment_item_id: i64) -> Result<DashboardSnapshot, String> {
    if !try_begin_upload_task() {
        return Err("已有上传任务在执行，请稍后重试。".to_string());
    }

    set_upload_progress_label("上传进度: 正在检查 Zotero 附件...");
    let prepared =
        match fetch_attachment_record_by_id(attachment_item_id).and_then(|(config, record)| {
            if !record.local_exists {
                set_upload_progress_label("上传进度: 正在从 WebDAV 拉取 Zotero 附件...");
            }
            resolve_attachment_for_push(&config, &record)
        }) {
            Ok(value) => value,
            Err(err) => {
                record_upload_failure(&err);
                return Err(err);
            }
        };

    let upload_result =
        crate::push::upload_items_blocking_with_active_task(vec![crate::push::UploadItem {
            path: prepared.local_path.clone(),
            display_name: None,
        }]);
    cleanup_resolution(&prepared);
    upload_result
}

pub(crate) fn list_items_at(
    data_dir: &str,
    limit: Option<usize>,
    offset: Option<usize>,
    search: Option<String>,
    can_download_from_webdav: bool,
) -> Result<Vec<ZoteroItemSummary>, String> {
    let conn = Connection::open_with_flags(
        Path::new(data_dir).join("zotero.sqlite"),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .map_err(|_| "cannot open Zotero database")?;
    let offset = offset.unwrap_or(0);
    let search = normalize_optional(search).map(|value| value.to_lowercase());
    let rows = if search.is_none() && limit.filter(|value| *value > 0).is_some() {
        let limit = limit.unwrap_or_default();
        let mut stmt = conn
            .prepare(
                "SELECT i.itemID, i.key, i.dateModified
                 FROM items i
                 JOIN itemTypes it ON it.itemTypeID = i.itemTypeID
                 JOIN libraries l ON l.libraryID = i.libraryID
                 WHERE l.type = 'user'
                   AND it.typeName NOT IN ('attachment', 'note', 'annotation')
                 ORDER BY i.dateModified DESC
                 LIMIT ?1 OFFSET ?2",
            )
            .map_err(|err| err.to_string())?;
        let mapped = stmt
            .query_map(params![limit as i64, offset as i64], map_recent_item_row)
            .map_err(|err| err.to_string())?;
        let mut rows = Vec::new();
        for row in mapped {
            rows.push(row.map_err(|err| err.to_string())?);
        }
        rows
    } else if search.is_none() && offset > 0 {
        let mut stmt = conn
            .prepare(
                "SELECT i.itemID, i.key, i.dateModified
                 FROM items i
                 JOIN itemTypes it ON it.itemTypeID = i.itemTypeID
                 JOIN libraries l ON l.libraryID = i.libraryID
                 WHERE l.type = 'user'
                   AND it.typeName NOT IN ('attachment', 'note', 'annotation')
                 ORDER BY i.dateModified DESC
                 LIMIT -1 OFFSET ?1",
            )
            .map_err(|err| err.to_string())?;
        let mapped = stmt
            .query_map(params![offset as i64], map_recent_item_row)
            .map_err(|err| err.to_string())?;
        let mut rows = Vec::new();
        for row in mapped {
            rows.push(row.map_err(|err| err.to_string())?);
        }
        rows
    } else {
        let mut stmt = conn
            .prepare(
                "SELECT i.itemID, i.key, i.dateModified
                 FROM items i
                 JOIN itemTypes it ON it.itemTypeID = i.itemTypeID
                 JOIN libraries l ON l.libraryID = i.libraryID
                 WHERE l.type = 'user'
                   AND it.typeName NOT IN ('attachment', 'note', 'annotation')
                 ORDER BY i.dateModified DESC",
            )
            .map_err(|err| err.to_string())?;
        let mapped = stmt
            .query_map([], map_recent_item_row)
            .map_err(|err| err.to_string())?;
        let mut rows = Vec::new();
        for row in mapped {
            rows.push(row.map_err(|err| err.to_string())?);
        }
        rows
    };
    let mut items = Vec::new();
    let page_limit = limit.filter(|value| *value > 0);
    let mut matched = 0_usize;
    for row in rows {
        let (item_id, item_key, date_modified) = row;
        let Some(item) = build_zotero_item_summary(
            &conn,
            data_dir,
            item_id,
            item_key,
            date_modified,
            can_download_from_webdav,
        )?
        else {
            continue;
        };
        if let Some(query) = search.as_deref() {
            if !zotero_item_matches_query(&item, query) {
                continue;
            }
            if matched < offset {
                matched += 1;
                continue;
            }
        }
        items.push(item);
        if let Some(limit) = page_limit {
            if items.len() >= limit {
                break;
            }
        }
    }
    Ok(items)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    #[test]
    fn parse_profiles_ini_prefers_default_profile() {
        let temp_dir = create_temp_dir("send2boox-zotero-test").unwrap();
        let root = temp_dir.join("Zotero");
        fs::create_dir_all(root.join("Profiles/a.default")).unwrap();
        fs::create_dir_all(root.join("Profiles/b.default")).unwrap();
        fs::write(
            root.join("profiles.ini"),
            "[Profile0]\nName=a\nIsRelative=1\nPath=Profiles/a.default\n\n[Profile1]\nName=b\nIsRelative=1\nPath=Profiles/b.default\nDefault=1\n",
        )
        .unwrap();
        let resolved = parse_profiles_ini_default_profile(&root).unwrap();
        assert!(resolved.ends_with("Profiles/b.default"));
        let _ = fs::remove_dir_all(temp_dir);
    }

    #[test]
    fn parse_prefs_extracts_webdav_fields() {
        let prefs = r#"
user_pref("extensions.zotero.useDataDir", true);
user_pref("extensions.zotero.dataDir", "/tmp/zotero");
user_pref("extensions.zotero.sync.storage.protocol", "webdav");
user_pref("extensions.zotero.sync.storage.url", "example.com/webdav/");
user_pref("extensions.zotero.sync.storage.username", "demo");
"#;
        assert_eq!(
            parse_pref_value(prefs, "extensions.zotero.dataDir").as_deref(),
            Some("/tmp/zotero")
        );
        assert_eq!(
            normalize_webdav_url(parse_pref_value(
                prefs,
                "extensions.zotero.sync.storage.url",
            ))
            .as_deref(),
            Some("https://example.com/webdav/zotero")
        );
        assert_eq!(
            parse_pref_bool(prefs, "extensions.zotero.useDataDir"),
            Some(true)
        );
    }

    #[test]
    fn normalize_webdav_url_keeps_or_appends_zotero_root() {
        assert_eq!(
            normalize_webdav_url(Some("https://example.com/webdav".to_string())).as_deref(),
            Some("https://example.com/webdav/zotero")
        );
        assert_eq!(
            normalize_webdav_url(Some("https://example.com/webdav/zotero/".to_string())).as_deref(),
            Some("https://example.com/webdav/zotero")
        );
    }

    #[test]
    fn webdav_download_base_urls_try_canonical_then_legacy_root() {
        assert_eq!(
            webdav_download_base_urls("https://example.com/webdav"),
            vec![
                "https://example.com/webdav/zotero".to_string(),
                "https://example.com/webdav".to_string()
            ]
        );
        assert_eq!(
            webdav_download_base_urls("https://example.com/webdav/zotero"),
            vec![
                "https://example.com/webdav/zotero".to_string(),
                "https://example.com/webdav".to_string()
            ]
        );
    }

    #[test]
    fn derive_missing_fields_uses_expected_priority() {
        let mut summary = ZoteroConnectionSummary::default();
        assert_eq!(derive_missing_fields(&summary), vec!["profile_dir"]);
        summary.profile_dir = Some("/tmp/profile".to_string());
        assert_eq!(derive_missing_fields(&summary), vec!["data_dir"]);
        summary.data_dir = Some("/tmp/data".to_string());
        summary.database_exists = true;
        assert_eq!(derive_missing_fields(&summary), vec!["webdav_protocol"]);
        summary.protocol = Some("webdav".to_string());
        summary.protocol_is_webdav = true;
        assert_eq!(
            derive_missing_fields(&summary),
            vec!["webdav_url", "webdav_username", "webdav_password"]
        );
    }

    #[test]
    fn resolve_attachment_local_path_maps_storage_prefix() {
        let path =
            resolve_attachment_local_path("/tmp/zotero", "ABC123", Some("storage:demo/file.pdf"))
                .unwrap();
        assert_eq!(
            path,
            PathBuf::from("/tmp/zotero/storage/ABC123/demo/file.pdf")
        );
    }

    #[test]
    fn sqlite_query_returns_recent_items() {
        let temp_dir = create_temp_dir("send2boox-zotero-db-test").unwrap();
        let db_path = temp_dir.join("zotero.sqlite");
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "
            CREATE TABLE libraries (libraryID INTEGER PRIMARY KEY, type TEXT, editable INT, filesEditable INT, version INT, storageVersion INT, lastSync INT, archived INT);
            CREATE TABLE itemTypes (itemTypeID INTEGER PRIMARY KEY, typeName TEXT);
            CREATE TABLE items (itemID INTEGER PRIMARY KEY, itemTypeID INT, dateAdded TEXT, dateModified TEXT, clientDateModified TEXT, libraryID INT, key TEXT, version INT, synced INT);
            CREATE TABLE fields (fieldID INTEGER PRIMARY KEY, fieldName TEXT, fieldFormatID INT);
            CREATE TABLE itemData (itemID INT, fieldID INT, valueID INT);
            CREATE TABLE itemDataValues (valueID INTEGER PRIMARY KEY, value TEXT);
            CREATE TABLE creators (creatorID INTEGER PRIMARY KEY, firstName TEXT, lastName TEXT, fieldMode INT);
            CREATE TABLE itemCreators (itemID INT, creatorID INT, creatorTypeID INT, orderIndex INT);
            CREATE TABLE itemAttachments (itemID INTEGER PRIMARY KEY, parentItemID INT, linkMode INT, contentType TEXT, charsetID INT, path TEXT, syncState INT, storageModTime INT, storageHash TEXT, lastProcessedModificationTime INT);
            ",
        )
        .unwrap();
        conn.execute("INSERT INTO libraries (libraryID, type, editable, filesEditable, version, storageVersion, lastSync, archived) VALUES (1, 'user', 1, 1, 0, 0, 0, 0)", []).unwrap();
        conn.execute(
            "INSERT INTO itemTypes (itemTypeID, typeName) VALUES (3, 'attachment'), (7, 'book')",
            [],
        )
        .unwrap();
        conn.execute("INSERT INTO items (itemID, itemTypeID, dateAdded, dateModified, clientDateModified, libraryID, key, version, synced) VALUES (1, 7, '2024-01-01', '2024-01-02', '2024-01-02', 1, 'BOOK1', 0, 0), (2, 3, '2024-01-01', '2024-01-02', '2024-01-02', 1, 'ATT1', 0, 0)", []).unwrap();
        conn.execute("INSERT INTO fields (fieldID, fieldName, fieldFormatID) VALUES (1, 'title', 0), (2, 'date', 0)", []).unwrap();
        conn.execute("INSERT INTO itemDataValues (valueID, value) VALUES (1, 'Test Book'), (2, '2024-03-01')", []).unwrap();
        conn.execute(
            "INSERT INTO itemData (itemID, fieldID, valueID) VALUES (1, 1, 1), (1, 2, 2)",
            [],
        )
        .unwrap();
        conn.execute("INSERT INTO creators (creatorID, firstName, lastName, fieldMode) VALUES (1, 'Ada', 'Lovelace', 0)", []).unwrap();
        conn.execute("INSERT INTO itemCreators (itemID, creatorID, creatorTypeID, orderIndex) VALUES (1, 1, 1, 0)", []).unwrap();
        conn.execute("INSERT INTO itemAttachments (itemID, parentItemID, linkMode, contentType, charsetID, path, syncState, storageModTime, storageHash, lastProcessedModificationTime) VALUES (2, 1, 0, 'application/pdf', 0, 'storage:test.pdf', 0, 0, '', 0)", []).unwrap();
        drop(conn);

        let cfg = ZoteroConfigFile {
            data_dir: Some(temp_dir.to_string_lossy().to_string()),
            protocol: Some("webdav".to_string()),
            webdav_url: Some("https://example.com/webdav".to_string()),
            webdav_username: Some("demo".to_string()),
            ..ZoteroConfigFile::default()
        };
        let (copy_dir, conn) = open_copied_database(temp_dir.to_str().unwrap()).unwrap();
        let title = fetch_field_value(&conn, 1, "title");
        let author = fetch_author_summary(&conn, 1).unwrap();
        let attachments = fetch_attachment_records(&conn, temp_dir.to_str().unwrap(), 1).unwrap();
        assert_eq!(title.as_deref(), Some("Test Book"));
        assert_eq!(author.as_deref(), Some("LovelaceAda"));
        assert_eq!(attachments.len(), 1);
        let _ = fs::remove_dir_all(copy_dir);
        let _ = fs::remove_dir_all(temp_dir);
        let _ = cfg;
    }
}
