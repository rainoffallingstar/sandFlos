use crate::models::{
    CalibreBookSummary, CalibreConnectionState, CalibreConnectionSummary, CalibreFormatSummary,
    CalibreSaveInput, DashboardSnapshot,
};
use crate::push::UploadItem;
use crate::push::{clear_upload_transfer_metrics, set_upload_progress_label};
use crate::state::{finish_upload_task, try_begin_upload_task, update_upload_runtime_state};
use crate::util::{normalize_optional, unix_ms_now};
use rusqlite::{params, Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};
use uuid::Uuid;

const CALIBRE_CONFIG_FILE: &str = "calibre_config.json";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CalibreConfigFile {
    #[serde(default)]
    library_dirs: Vec<String>,
    library_dir: Option<String>,
    database_path: Option<String>,
    detected_at_ms: Option<u128>,
    validated_at_ms: Option<u128>,
    last_error: Option<String>,
}

#[derive(Debug, Clone)]
struct CalibreFormatRecord {
    title: String,
    format: String,
    local_path: Option<PathBuf>,
    local_exists: bool,
}

fn calibre_config_path() -> Option<PathBuf> {
    let dir = crate::state::app_data_dir();
    Some(dir.join(CALIBRE_CONFIG_FILE))
}

pub fn load_calibre_config() -> CalibreConfigFile {
    let Some(path) = calibre_config_path() else {
        return CalibreConfigFile::default();
    };
    let Ok(raw) = fs::read_to_string(path) else {
        return CalibreConfigFile::default();
    };
    serde_json::from_str::<CalibreConfigFile>(&raw).unwrap_or_default()
}

pub fn save_calibre_config(config: &CalibreConfigFile) -> Result<(), String> {
    let path = calibre_config_path().ok_or_else(|| "无法定位 Calibre 配置目录".to_string())?;
    let text = serde_json::to_string(config).map_err(|err| err.to_string())?;
    fs::write(path, text).map_err(|err| err.to_string())
}

fn expand_home_path(value: &str) -> PathBuf {
    if let Some(rest) = value.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    PathBuf::from(value)
}

fn normalize_library_dir(value: Option<String>) -> Option<String> {
    normalize_optional(value).map(|raw| expand_home_path(&raw).to_string_lossy().to_string())
}

fn normalize_library_dirs(values: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for value in values {
        let normalized = normalize_library_dir(Some(value.clone()));
        if let Some(path) = normalized {
            if !out.contains(&path) {
                out.push(path);
            }
        }
    }
    out
}

fn configured_library_dirs(config: &CalibreConfigFile) -> Vec<String> {
    let mut values = normalize_library_dirs(&config.library_dirs);
    if values.is_empty() {
        if let Some(single) = normalize_library_dir(config.library_dir.clone()) {
            values.push(single);
        }
    }
    values
}

fn ready_library_dirs(config: &CalibreConfigFile) -> Vec<String> {
    configured_library_dirs(config)
        .into_iter()
        .filter(|dir| Path::new(&build_database_path(dir)).is_file())
        .collect()
}

fn default_library_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    let candidate = PathBuf::from(home).join("Calibre Library");
    if candidate.join("metadata.db").is_file() {
        Some(candidate)
    } else {
        None
    }
}

fn default_library_dirs() -> Vec<PathBuf> {
    default_library_dir().into_iter().collect()
}

fn build_database_path(library_dir: &str) -> String {
    PathBuf::from(library_dir)
        .join("metadata.db")
        .to_string_lossy()
        .to_string()
}

fn build_database_paths(library_dirs: &[String]) -> Vec<String> {
    library_dirs
        .iter()
        .map(|dir| {
            PathBuf::from(dir)
                .join("metadata.db")
                .to_string_lossy()
                .to_string()
        })
        .collect()
}

fn connection_summary_from_config(config: &CalibreConfigFile) -> CalibreConnectionSummary {
    let library_dirs = configured_library_dirs(config);
    let database_paths = build_database_paths(&library_dirs);
    let ready_library_dirs = library_dirs
        .iter()
        .filter(|dir| Path::new(&build_database_path(dir)).is_file())
        .cloned()
        .collect::<Vec<_>>();
    let ready_library_count = ready_library_dirs.len();
    CalibreConnectionSummary {
        total_library_count: library_dirs.len(),
        ready_library_count,
        library_dirs,
        database_paths,
        ready_library_dirs,
    }
}

fn derive_missing_fields(summary: &CalibreConnectionSummary) -> Vec<String> {
    let mut fields = Vec::new();
    if summary.library_dirs.is_empty() {
        fields.push("library_dirs".to_string());
    }
    if summary.ready_library_count == 0 {
        fields.push("database_path".to_string());
    }
    fields
}

fn build_connection_state(config: &CalibreConfigFile) -> CalibreConnectionState {
    let summary = connection_summary_from_config(config);
    let missing_fields = derive_missing_fields(&summary);
    let state = if summary.total_library_count > 0
        && summary.ready_library_count == summary.total_library_count
    {
        "connected"
    } else if summary.ready_library_count > 0 {
        "pending"
    } else if config.last_error.is_some() {
        "failed"
    } else {
        "pending"
    };
    CalibreConnectionState {
        state: state.to_string(),
        missing_fields,
        summary,
        detected_at_ms: config.detected_at_ms,
        validated_at_ms: config.validated_at_ms,
        last_error: config.last_error.clone(),
    }
}

fn validate_library_dir(path: &Path) -> Result<(), String> {
    if !path.is_dir() {
        return Err("所选路径不是目录".to_string());
    }
    if !path.join("metadata.db").is_file() {
        return Err("所选目录中未找到 metadata.db".to_string());
    }
    Ok(())
}

pub fn set_library_dirs_config(
    library_dirs: Vec<String>,
    detected_at_ms: Option<u128>,
) -> Result<CalibreConnectionState, String> {
    let normalized_dirs = normalize_library_dirs(&library_dirs);
    if normalized_dirs.is_empty() {
        let mut config = load_calibre_config();
        config.library_dirs.clear();
        config.library_dir = None;
        config.database_path = None;
        config.detected_at_ms = detected_at_ms.or(config.detected_at_ms);
        config.validated_at_ms = Some(unix_ms_now());
        config.last_error = None;
        save_calibre_config(&config)?;
        return Ok(build_connection_state(&config));
    }
    let mut errors = Vec::new();
    for dir in &normalized_dirs {
        let path = PathBuf::from(dir);
        if let Err(err) = validate_library_dir(&path) {
            errors.push(format!("{dir}: {err}"));
        }
    }
    if !errors.is_empty() {
        return Err(errors.join("\n"));
    }
    let mut config = load_calibre_config();
    config.library_dirs = normalized_dirs.clone();
    config.library_dir = normalized_dirs.first().cloned();
    config.database_path = config.library_dir.as_deref().map(build_database_path);
    config.detected_at_ms = detected_at_ms.or(config.detected_at_ms);
    config.validated_at_ms = Some(unix_ms_now());
    config.last_error = None;
    save_calibre_config(&config)?;
    Ok(build_connection_state(&config))
}

pub fn detect_library_inner() -> Result<CalibreConnectionState, String> {
    let detected = default_library_dirs();
    if detected.is_empty() {
        let mut config = load_calibre_config();
        config.last_error =
            Some("未在默认位置找到 Calibre Library，请手动选择书库目录。".to_string());
        save_calibre_config(&config)?;
        return Err("未在默认位置找到 Calibre Library，请手动选择书库目录。".to_string());
    }
    set_library_dirs_config(
        detected
            .iter()
            .map(|path| path.to_string_lossy().to_string())
            .collect(),
        Some(unix_ms_now()),
    )
}

pub fn refresh_libraries_inner() -> Result<CalibreConnectionState, String> {
    let mut config = load_calibre_config();
    let summary = connection_summary_from_config(&config);
    config.validated_at_ms = Some(unix_ms_now());
    config.last_error = if summary.total_library_count == 0
        || summary.ready_library_count == summary.total_library_count
    {
        None
    } else {
        let ready = summary.ready_library_count;
        let total = summary.total_library_count;
        Some(format!(
            "书库检查完成: {ready}/{total} 个可用，其余目录需要修复或重新挂载。"
        ))
    };
    save_calibre_config(&config)?;
    Ok(build_connection_state(&config))
}

fn create_temp_dir(prefix: &str) -> Result<PathBuf, String> {
    let dir = std::env::temp_dir().join(format!("{prefix}-{}", Uuid::new_v4()));
    fs::create_dir_all(&dir).map_err(|err| err.to_string())?;
    Ok(dir)
}

fn open_copied_database(library_dir: &str) -> Result<(PathBuf, Connection), String> {
    let source = PathBuf::from(library_dir).join("metadata.db");
    if !source.is_file() {
        return Err("metadata.db 不存在".to_string());
    }
    let temp_dir = create_temp_dir("send2boox-calibre-db")?;
    let copied = temp_dir.join("metadata.db");
    fs::copy(&source, &copied).map_err(|err| err.to_string())?;
    let conn = Connection::open_with_flags(copied, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|err| err.to_string())?;
    Ok((temp_dir, conn))
}

fn extract_year(value: Option<String>) -> Option<String> {
    let raw = value?;
    let year = raw
        .chars()
        .skip_while(|ch| !ch.is_ascii_digit())
        .take(4)
        .collect::<String>();
    if year.len() == 4 {
        Some(year)
    } else {
        None
    }
}

fn author_summary(value: Option<String>) -> Option<String> {
    normalize_optional(value).map(|text| text.replace(" & ", "、"))
}

fn format_candidate_paths(base_dir: &Path, name: Option<&str>, format: &str) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    let extension = format.to_ascii_lowercase();
    if let Some(name) = name.and_then(|value| normalize_optional(Some(value.to_string()))) {
        let raw = base_dir.join(&name);
        candidates.push(raw.clone());
        if raw.extension().is_none() {
            candidates.push(base_dir.join(format!("{name}.{extension}")));
        }
    }
    candidates
}

fn resolve_format_local_path(
    library_dir: &str,
    relative_book_path: &str,
    name: Option<&str>,
    format: &str,
) -> Option<PathBuf> {
    let base_dir = PathBuf::from(library_dir).join(relative_book_path);
    for candidate in format_candidate_paths(&base_dir, name, format) {
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    let expected_ext = format.to_ascii_lowercase();
    let entries = fs::read_dir(&base_dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        let matches_ext = path
            .extension()
            .and_then(|value| value.to_str())
            .map(|value| value.eq_ignore_ascii_case(&expected_ext))
            .unwrap_or(false);
        if path.is_file() && matches_ext {
            return Some(path);
        }
    }
    None
}

fn library_label(library_dir: &str) -> String {
    Path::new(library_dir)
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(library_dir)
        .to_string()
}

fn build_format_summary(
    library_dir: &str,
    data_id: i64,
    format: String,
    local_path: Option<PathBuf>,
    size_hint: Option<u64>,
) -> CalibreFormatSummary {
    let local_exists = local_path
        .as_ref()
        .map(|path| path.is_file())
        .unwrap_or(false);
    let file_size = local_path
        .as_ref()
        .and_then(|path| fs::metadata(path).ok().map(|meta| meta.len()))
        .or(size_hint);
    CalibreFormatSummary {
        data_id,
        library_dir: library_dir.to_string(),
        format: format.clone(),
        file_name: local_path
            .as_ref()
            .and_then(|path| path.file_name())
            .and_then(|value| value.to_str())
            .map(|value| value.to_string()),
        file_path: local_path
            .as_ref()
            .map(|path| path.to_string_lossy().to_string()),
        file_size,
        local_exists,
        can_push_directly: local_exists,
        status_label: if local_exists {
            "本地书籍可直接推送".to_string()
        } else {
            "本地文件缺失".to_string()
        },
    }
}

fn fetch_format_records(
    conn: &Connection,
    library_dir: &str,
    book_id: i64,
    relative_book_path: &str,
) -> Result<Vec<CalibreFormatSummary>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT id, format, name, uncompressed_size
             FROM data
             WHERE book = ?1
             ORDER BY format ASC",
        )
        .map_err(|err| err.to_string())?;
    let rows = stmt
        .query_map(params![book_id], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<u64>>(3).ok().flatten(),
            ))
        })
        .map_err(|err| err.to_string())?;
    let mut formats = Vec::new();
    for row in rows {
        let (data_id, format, name, size_hint) = row.map_err(|err| err.to_string())?;
        let local_path =
            resolve_format_local_path(library_dir, relative_book_path, name.as_deref(), &format);
        formats.push(build_format_summary(
            library_dir,
            data_id,
            format,
            local_path,
            size_hint,
        ));
    }
    Ok(formats)
}

type RecentBookRow = (
    i64,
    Option<String>,
    Option<String>,
    String,
    Option<String>,
    Option<String>,
);

fn map_recent_book_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RecentBookRow> {
    Ok((
        row.get::<_, i64>(0)?,
        row.get::<_, Option<String>>(1)?,
        row.get::<_, Option<String>>(2)?,
        row.get::<_, String>(3)?,
        row.get::<_, Option<String>>(4)?,
        row.get::<_, Option<String>>(5)?,
    ))
}

fn calibre_book_matches_query(book: &CalibreBookSummary, query: &str) -> bool {
    let mut haystack = vec![
        book.title.clone(),
        book.author_summary.clone().unwrap_or_default(),
        book.published_year.clone().unwrap_or_default(),
        book.library_label.clone(),
        book.library_dir.clone(),
        book.date_modified.clone(),
    ];
    for format in &book.formats {
        haystack.push(format.format.clone());
        haystack.push(format.file_name.clone().unwrap_or_default());
        haystack.push(format.status_label.clone());
    }
    haystack.join(" ").to_lowercase().contains(query)
}

pub(crate) fn list_books_at(
    library_dir: &str,
    limit: Option<usize>,
) -> Result<Vec<CalibreBookSummary>, String> {
    let conn = Connection::open_with_flags(
        Path::new(library_dir).join("metadata.db"),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .map_err(|_| "cannot open Calibre database")?;
    // Calibre may hold an exclusive write lock while updating its metadata.
    // Read-only commands should report the conflict promptly instead of
    // blocking the CLI behind SQLite's multi-second default busy timeout.
    conn.busy_timeout(std::time::Duration::from_millis(500))
        .map_err(|_| "cannot configure Calibre database timeout")?;
    let rows = if let Some(limit) = limit.filter(|value| *value > 0) {
        let mut stmt = conn
            .prepare(
                "SELECT id, title, author_sort, path, last_modified, pubdate
                 FROM books
                 ORDER BY last_modified DESC
                 LIMIT ?1",
            )
            .map_err(|err| err.to_string())?;
        let mapped = stmt
            .query_map(params![limit as i64], map_recent_book_row)
            .map_err(|err| err.to_string())?;
        let mut rows = Vec::new();
        for row in mapped {
            rows.push(row.map_err(|err| err.to_string())?);
        }
        rows
    } else {
        let mut stmt = conn
            .prepare(
                "SELECT id, title, author_sort, path, last_modified, pubdate
                 FROM books
                 ORDER BY last_modified DESC",
            )
            .map_err(|err| err.to_string())?;
        let mapped = stmt
            .query_map([], map_recent_book_row)
            .map_err(|err| err.to_string())?;
        let mut rows = Vec::new();
        for row in mapped {
            rows.push(row.map_err(|err| err.to_string())?);
        }
        rows
    };
    let mut books = Vec::new();
    for row in rows {
        let (book_id, title, author_sort, relative_book_path, last_modified, pubdate) = row;
        let formats = fetch_format_records(&conn, library_dir, book_id, &relative_book_path)?;
        if formats.is_empty() {
            continue;
        }
        books.push(CalibreBookSummary {
            book_id,
            library_dir: library_dir.to_string(),
            library_label: library_label(library_dir),
            title: normalize_optional(title).unwrap_or_else(|| "未命名书籍".to_string()),
            author_summary: author_summary(author_sort),
            published_year: extract_year(pubdate),
            date_modified: last_modified.unwrap_or_default(),
            formats,
        });
    }
    Ok(books)
}

fn list_recent_books_from_library(
    library_dir: &str,
    limit: Option<usize>,
) -> Result<Vec<CalibreBookSummary>, String> {
    list_books_at(library_dir, limit)
}

pub fn list_recent_books_inner(
    limit: Option<usize>,
    offset: Option<usize>,
    search: Option<String>,
) -> Result<Vec<CalibreBookSummary>, String> {
    let config = load_calibre_config();
    let library_dirs = ready_library_dirs(&config);
    if library_dirs.is_empty() {
        return Err("请先补全 Calibre 书库目录".to_string());
    }
    let mut books = Vec::new();
    let offset = offset.unwrap_or(0);
    let search = normalize_optional(search).map(|value| value.to_lowercase());
    let fetch_limit = if search.is_some() {
        None
    } else {
        limit
            .filter(|value| *value > 0)
            .map(|value| offset.saturating_add(value).max(10))
    };
    let per_library_limit = fetch_limit;
    for library_dir in library_dirs {
        books.extend(list_recent_books_from_library(
            &library_dir,
            per_library_limit,
        )?);
    }
    books.sort_by(|a, b| b.date_modified.cmp(&a.date_modified));
    if let Some(query) = search.as_deref() {
        books.retain(|book| calibre_book_matches_query(book, query));
    }
    if offset > 0 {
        books = books.into_iter().skip(offset).collect();
    }
    if let Some(limit) = limit.filter(|value| *value > 0) {
        books.truncate(limit);
    }
    Ok(books)
}

fn sanitize_file_stem(value: &str) -> String {
    let mapped = value
        .chars()
        .map(|ch| match ch {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => ' ',
            _ => ch,
        })
        .collect::<String>();
    let collapsed = mapped.split_whitespace().collect::<Vec<_>>().join(" ");
    collapsed.trim_matches('.').trim().to_string()
}

fn build_display_name(title: &str, local_path: &Path, format: &str) -> String {
    let ext = local_path
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| value.to_ascii_lowercase())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| format.to_ascii_lowercase());
    let stem = sanitize_file_stem(title);
    if stem.is_empty() {
        return local_path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("book")
            .to_string();
    }
    format!("{stem}.{ext}")
}

fn fetch_format_record_by_id(
    library_dir: &str,
    data_id: i64,
) -> Result<CalibreFormatRecord, String> {
    let (temp_dir, conn) = open_copied_database(library_dir)?;
    let record = conn
        .query_row(
            "SELECT d.id, b.title, b.path, d.format, d.name
             FROM data d
             JOIN books b ON b.id = d.book
             WHERE d.id = ?1",
            params![data_id],
            |row| {
                let title = normalize_optional(row.get::<_, Option<String>>(1)?)
                    .unwrap_or_else(|| "未命名书籍".to_string());
                let relative_book_path = row.get::<_, String>(2)?;
                let format = row.get::<_, String>(3)?;
                let name = row.get::<_, Option<String>>(4)?;
                let local_path = resolve_format_local_path(
                    library_dir,
                    &relative_book_path,
                    name.as_deref(),
                    &format,
                );
                Ok(CalibreFormatRecord {
                    title,
                    format,
                    local_exists: local_path
                        .as_ref()
                        .map(|path| path.is_file())
                        .unwrap_or(false),
                    local_path,
                })
            },
        )
        .map_err(|err| err.to_string())?;
    let _ = fs::remove_dir_all(temp_dir);
    Ok(record)
}

fn resolve_upload_item_for_format(record: &CalibreFormatRecord) -> Result<UploadItem, String> {
    let Some(local_path) = record.local_path.clone() else {
        return Err("Calibre 书籍文件不存在".to_string());
    };
    if !record.local_exists {
        return Err("Calibre 书籍文件不存在".to_string());
    }
    Ok(UploadItem {
        display_name: Some(build_display_name(
            &record.title,
            &local_path,
            &record.format,
        )),
        path: local_path,
    })
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

pub fn calibre_status() -> Result<CalibreConnectionState, String> {
    Ok(build_connection_state(&load_calibre_config()))
}

pub fn calibre_save_library_dir(input: CalibreSaveInput) -> Result<CalibreConnectionState, String> {
    let mut dirs = input.library_dirs.unwrap_or_default();
    if let Some(single) = input.library_dir {
        dirs.push(single);
    }
    let normalized = normalize_library_dirs(&dirs);
    set_library_dirs_config(normalized, None)
}

pub fn calibre_push_format(library_dir: String, data_id: i64) -> Result<DashboardSnapshot, String> {
    if !try_begin_upload_task() {
        return Err("已有上传任务在执行，请稍后重试。".to_string());
    }

    set_upload_progress_label("上传进度: 正在检查 Calibre 书籍文件...");
    let upload_item = match fetch_format_record_by_id(&library_dir, data_id)
        .and_then(|record| resolve_upload_item_for_format(&record))
    {
        Ok(value) => value,
        Err(err) => {
            record_upload_failure(&err);
            return Err(err);
        }
    };

    crate::push::upload_items_blocking_with_active_task(vec![upload_item])
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    #[test]
    fn sanitize_file_stem_replaces_invalid_chars() {
        assert_eq!(
            sanitize_file_stem("三体: 黑暗森林/全集"),
            "三体 黑暗森林 全集"
        );
    }

    #[test]
    fn build_display_name_prefers_title_with_original_extension() {
        let name = build_display_name("活着", Path::new("/tmp/huozhe.epub"), "EPUB");
        assert_eq!(name, "活着.epub");
    }

    #[test]
    fn sqlite_query_returns_recent_books() {
        let temp_dir = create_temp_dir("send2boox-calibre-db-test").unwrap();
        let library_dir = temp_dir.join("Calibre Library");
        fs::create_dir_all(library_dir.join("liu-ci-xin").join("san-ti")).unwrap();
        fs::write(
            library_dir
                .join("liu-ci-xin")
                .join("san-ti")
                .join("san-ti.epub"),
            b"demo",
        )
        .unwrap();
        let db_path = library_dir.join("metadata.db");
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "
            CREATE TABLE books (
                id INTEGER PRIMARY KEY,
                title TEXT,
                sort TEXT,
                timestamp TEXT,
                pubdate TEXT,
                series_index REAL,
                author_sort TEXT,
                isbn TEXT,
                lccn TEXT,
                path TEXT,
                flags INTEGER,
                uuid TEXT,
                has_cover BOOL,
                last_modified TEXT
            );
            CREATE TABLE data (
                id INTEGER PRIMARY KEY,
                book INTEGER,
                format TEXT,
                uncompressed_size INTEGER,
                name TEXT
            );
            ",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO books (id, title, author_sort, path, last_modified, pubdate) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![1_i64, "三体", "刘慈欣", "liu-ci-xin/san-ti", "2025-01-01 00:00:00+00:00", "2008-01-01"],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO data (id, book, format, uncompressed_size, name) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![7_i64, 1_i64, "EPUB", 4_i64, "san-ti"],
        )
        .unwrap();

        let books =
            list_recent_books_from_library(&library_dir.to_string_lossy(), Some(10)).unwrap();
        assert_eq!(books.len(), 1);
        assert_eq!(books[0].title, "三体");
        assert_eq!(books[0].formats[0].data_id, 7);
        assert!(books[0].formats[0].local_exists);
    }

    #[test]
    fn configured_library_dirs_supports_legacy_and_multi_values() {
        let config = CalibreConfigFile {
            library_dirs: vec![
                "/Volumes/A/Calibre Library".to_string(),
                "/Volumes/B/Calibre Library".to_string(),
            ],
            library_dir: Some("/Volumes/Legacy/Calibre Library".to_string()),
            ..CalibreConfigFile::default()
        };
        let dirs = configured_library_dirs(&config);
        assert_eq!(dirs.len(), 2);
        assert_eq!(dirs[0], "/Volumes/A/Calibre Library");
        assert_eq!(dirs[1], "/Volumes/B/Calibre Library");
    }
}
