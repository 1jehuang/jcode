use crate::id::{extract_session_name, session_icon};
use crate::message::Role;
use crate::registry::{self, ServerInfo};
use crate::session::{self, CrashedSessionsInfo, Session, SessionStatus, StoredDisplayRole};
use crate::storage;
use anyhow::Result;
use serde::Deserialize;
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, HashSet};
use std::fs::File;
#[cfg(test)]
use std::io::Read;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use super::{
    DEFAULT_SESSION_SCAN_LIMIT, MAX_SESSION_SCAN_LIMIT, MIN_SESSION_SCAN_LIMIT, PreviewMessage,
    SEARCH_CONTENT_BUDGET_BYTES, ServerGroup, SessionInfo,
};

use super::{ResumeTarget, SessionSource};

#[cfg(test)]
const TRANSCRIPT_SEARCH_CHUNK_BYTES: usize = 64 * 1024;

fn session_scan_limit() -> usize {
    std::env::var("JCODE_SESSION_PICKER_MAX_SESSIONS")
        .ok()
        .and_then(|raw| raw.trim().parse::<usize>().ok())
        .map(|n| n.clamp(MIN_SESSION_SCAN_LIMIT, MAX_SESSION_SCAN_LIMIT))
        .unwrap_or(DEFAULT_SESSION_SCAN_LIMIT)
}

fn session_candidate_window(scan_limit: usize) -> usize {
    scan_limit
        .saturating_mul(20)
        .clamp(scan_limit.max(1), 20_000)
}

const SESSION_LIST_CACHE_TTL: Duration = Duration::from_secs(5);

#[derive(Clone)]
struct SessionListCacheEntry {
    loaded_at: Instant,
    sessions_dir: PathBuf,
    scan_limit: usize,
    sessions: Vec<SessionInfo>,
}

fn session_list_cache() -> &'static Mutex<Option<SessionListCacheEntry>> {
    static CACHE: OnceLock<Mutex<Option<SessionListCacheEntry>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(None))
}

fn push_with_byte_budget(dst: &mut String, src: &str, budget: &mut usize) {
    if *budget == 0 || src.is_empty() {
        return;
    }

    let mut end = src.len().min(*budget);
    while end > 0 && !src.is_char_boundary(end) {
        end -= 1;
    }
    if end == 0 {
        return;
    }

    dst.push_str(&src[..end]);
    *budget = budget.saturating_sub(end);
}

pub(super) fn build_search_index(
    id: &str,
    short_name: &str,
    title: &str,
    working_dir: Option<&str>,
    save_label: Option<&str>,
    messages_preview: &[PreviewMessage],
) -> String {
    let mut combined = String::new();
    combined.push_str(title);
    combined.push(' ');
    combined.push_str(short_name);
    combined.push(' ');
    combined.push_str(id);

    if let Some(dir) = working_dir {
        combined.push(' ');
        combined.push_str(dir);
    }

    if let Some(label) = save_label {
        combined.push(' ');
        combined.push_str(label);
    }

    let mut budget = SEARCH_CONTENT_BUDGET_BYTES;
    for msg in messages_preview {
        let content = msg.content.trim();
        if content.is_empty() {
            continue;
        }
        combined.push(' ');
        push_with_byte_budget(&mut combined, content, &mut budget);
        if budget == 0 {
            break;
        }
    }

    combined.to_lowercase()
}

#[cfg(test)]
pub(super) fn session_matches_query(session: &SessionInfo, query: &str) -> bool {
    let normalized = query.trim().to_lowercase();
    if normalized.is_empty() {
        return true;
    }

    if session.search_index.contains(&normalized) {
        return true;
    }

    session_transcript_contains_query(session, &normalized)
}

/// Fast in-memory matcher for interactive picker filtering.
///
/// This intentionally avoids transcript file I/O because it runs on every
/// keystroke while the `/resume` overlay is open. Transcript-backed content can
/// still become searchable after preview load because the picker refreshes the
/// session's cached `search_index` from the loaded preview.
pub(super) fn session_matches_picker_query(session: &SessionInfo, query: &str) -> bool {
    let normalized = query.trim().to_lowercase();
    normalized.is_empty() || session.search_index.contains(&normalized)
}

#[cfg(test)]
fn session_transcript_contains_query(session: &SessionInfo, query_lower: &str) -> bool {
    transcript_paths_for_session(session)
        .into_iter()
        .any(|path| file_contains_case_insensitive_query(&path, query_lower))
}

#[cfg(test)]
fn transcript_paths_for_session(session: &SessionInfo) -> Vec<PathBuf> {
    match &session.resume_target {
        ResumeTarget::JcodeSession { session_id } => {
            let Ok(sessions_dir) = storage::jcode_dir().map(|dir| dir.join("sessions")) else {
                return Vec::new();
            };
            vec![
                sessions_dir.join(format!("{session_id}.json")),
                sessions_dir.join(format!("{session_id}.journal.jsonl")),
            ]
        }
        ResumeTarget::ClaudeCodeSession { session_path, .. }
        | ResumeTarget::CodexSession { session_path, .. }
        | ResumeTarget::PiSession { session_path }
        | ResumeTarget::OpenCodeSession { session_path, .. } => {
            vec![PathBuf::from(session_path)]
        }
    }
}

#[cfg(test)]
fn file_contains_case_insensitive_query(path: &Path, query_lower: &str) -> bool {
    if query_lower.is_empty() {
        return true;
    }
    if !path.exists() {
        return false;
    }

    if query_lower.is_ascii() {
        return file_contains_ascii_case_insensitive(path, query_lower.as_bytes());
    }

    std::fs::read_to_string(path)
        .ok()
        .map(|content| content.to_lowercase().contains(query_lower))
        .unwrap_or(false)
}

#[cfg(test)]
fn file_contains_ascii_case_insensitive(path: &Path, needle_lower: &[u8]) -> bool {
    let Ok(file) = File::open(path) else {
        return false;
    };
    let mut reader = BufReader::new(file);
    let overlap = needle_lower.len().saturating_sub(1);
    let mut carry = Vec::with_capacity(overlap);
    let mut buf = vec![0u8; TRANSCRIPT_SEARCH_CHUNK_BYTES];

    loop {
        let read = match reader.read(&mut buf) {
            Ok(0) => return false,
            Ok(read) => read,
            Err(_) => return false,
        };

        let mut window = Vec::with_capacity(carry.len() + read);
        window.extend_from_slice(&carry);
        window.extend_from_slice(&buf[..read]);

        if contains_ascii_case_insensitive_bytes(&window, needle_lower) {
            return true;
        }

        carry.clear();
        let keep = overlap.min(window.len());
        carry.extend_from_slice(&window[window.len().saturating_sub(keep)..]);
    }
}

#[cfg(test)]
fn contains_ascii_case_insensitive_bytes(haystack: &[u8], needle_lower: &[u8]) -> bool {
    if needle_lower.is_empty() {
        return true;
    }
    if needle_lower.len() > haystack.len() {
        return false;
    }

    haystack.windows(needle_lower.len()).any(|window| {
        window
            .iter()
            .zip(needle_lower.iter())
            .all(|(&hay, &needle)| hay.to_ascii_lowercase() == needle)
    })
}

fn build_search_index_from_summary(
    id: &str,
    short_name: &str,
    title: &str,
    working_dir: Option<&str>,
    save_label: Option<&str>,
) -> String {
    let mut combined = String::new();
    combined.push_str(title);
    combined.push(' ');
    combined.push_str(short_name);
    combined.push(' ');
    combined.push_str(id);

    if let Some(dir) = working_dir {
        combined.push(' ');
        combined.push_str(dir);
    }

    if let Some(label) = save_label {
        combined.push(' ');
        combined.push_str(label);
    }

    combined.to_lowercase()
}

fn session_sort_key(stem: &str) -> u64 {
    for part in stem.split('_') {
        if part.len() == 13
            && part.as_bytes().iter().all(|b| b.is_ascii_digit())
            && let Ok(ts) = part.parse::<u64>()
        {
            return ts;
        }
    }

    stem.split('_')
        .rev()
        .find_map(|part| part.parse::<u64>().ok())
        .unwrap_or(0)
}

fn path_modified_sort_key(path: &Path) -> u128 {
    path.metadata()
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos())
        .unwrap_or(0)
}

fn session_candidate_sort_key(
    sessions_dir: &Path,
    snapshot_path: &Path,
    stem: &str,
) -> (u128, u64, String) {
    let journal_path = sessions_dir.join(format!("{stem}.journal.jsonl"));
    let modified = path_modified_sort_key(snapshot_path).max(path_modified_sort_key(&journal_path));
    (modified, session_sort_key(stem), stem.to_string())
}

fn classify_session_source(
    id: &str,
    provider_key: Option<&str>,
    model: Option<&str>,
) -> SessionSource {
    if id.starts_with("imported_cc_") {
        return SessionSource::ClaudeCode;
    }

    let provider_key = provider_key.unwrap_or_default().to_ascii_lowercase();
    let model = model.unwrap_or_default().to_ascii_lowercase();

    if provider_key == "pi" || provider_key.starts_with("pi-") {
        return SessionSource::Pi;
    }
    if provider_key == "opencode"
        || provider_key == "opencode-go"
        || provider_key.contains("opencode")
    {
        return SessionSource::OpenCode;
    }
    if provider_key.contains("codex") || model.contains("codex") || model.contains("openai-codex") {
        return SessionSource::Codex;
    }

    SessionSource::Jcode
}

fn collect_files_recursive(root: &Path, extension: &str) -> Vec<PathBuf> {
    fn walk(dir: &Path, extension: &str, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, extension, out);
            } else if path
                .extension()
                .and_then(|ext| ext.to_str())
                .map(|ext| ext.eq_ignore_ascii_case(extension))
                .unwrap_or(false)
            {
                out.push(path);
            }
        }
    }

    let mut files = Vec::new();
    walk(root, extension, &mut files);
    files.sort_by(|a, b| {
        let a_time = std::fs::metadata(a).and_then(|meta| meta.modified()).ok();
        let b_time = std::fs::metadata(b).and_then(|meta| meta.modified()).ok();
        b_time.cmp(&a_time).then_with(|| b.cmp(a))
    });
    files
}

fn collect_recent_files_recursive(root: &Path, extension: &str, limit: usize) -> Vec<PathBuf> {
    fn modified_sort_key(path: &Path) -> u64 {
        path.metadata()
            .and_then(|meta| meta.modified())
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|duration| duration.as_secs())
            .unwrap_or(0)
    }

    fn walk(
        dir: &Path,
        extension: &str,
        limit: usize,
        out: &mut BinaryHeap<Reverse<(u64, PathBuf)>>,
    ) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, extension, limit, out);
            } else if path
                .extension()
                .and_then(|ext| ext.to_str())
                .map(|ext| ext.eq_ignore_ascii_case(extension))
                .unwrap_or(false)
            {
                let key = (modified_sort_key(&path), path);
                if out.len() < limit {
                    out.push(Reverse(key));
                } else if out.peek().map(|smallest| key > smallest.0).unwrap_or(true) {
                    out.pop();
                    out.push(Reverse(key));
                }
            }
        }
    }

    if limit == 0 {
        return Vec::new();
    }

    let mut heap: BinaryHeap<Reverse<(u64, PathBuf)>> = BinaryHeap::new();
    walk(root, extension, limit, &mut heap);
    let mut files: Vec<(u64, PathBuf)> = heap.into_iter().map(|entry| entry.0).collect();
    files.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.cmp(&a.1)));
    files.into_iter().map(|(_, path)| path).collect()
}

fn push_preview_message(preview: &mut Vec<PreviewMessage>, role: &str, content: String) {
    let content = content.trim();
    if content.is_empty() {
        return;
    }
    preview.push(PreviewMessage {
        role: role.to_string(),
        content: content.to_string(),
        tool_calls: Vec::new(),
        tool_data: None,
        timestamp: None,
    });
    if preview.len() > 20 {
        let drop_count = preview.len().saturating_sub(20);
        preview.drain(0..drop_count);
    }
}

fn extract_text_from_value(value: &serde_json::Value) -> String {
    fn visit(value: &serde_json::Value, out: &mut Vec<String>) {
        match value {
            serde_json::Value::String(text) => {
                if !text.trim().is_empty() {
                    out.push(text.trim().to_string());
                }
            }
            serde_json::Value::Array(items) => {
                for item in items {
                    visit(item, out);
                }
            }
            serde_json::Value::Object(map) => {
                if let Some(text) = map.get("text").and_then(|v| v.as_str())
                    && !text.trim().is_empty()
                {
                    out.push(text.trim().to_string());
                }
                if let Some(text) = map.get("title").and_then(|v| v.as_str())
                    && !text.trim().is_empty()
                {
                    out.push(text.trim().to_string());
                }
                for value in map.values() {
                    visit(value, out);
                }
            }
            _ => {}
        }
    }

    let mut out = Vec::new();
    visit(value, &mut out);
    out.join(" ")
}

fn extract_block_text_from_value(value: &serde_json::Value) -> String {
    fn extract(value: &serde_json::Value, separator: &str) -> Option<String> {
        match value {
            serde_json::Value::String(text) => {
                let trimmed = text.trim();
                (!trimmed.is_empty()).then(|| trimmed.to_string())
            }
            serde_json::Value::Array(items) => {
                let parts: Vec<String> =
                    items.iter().filter_map(|item| extract(item, " ")).collect();
                (!parts.is_empty()).then(|| parts.join("\n\n"))
            }
            serde_json::Value::Object(map) => {
                if let Some(text) = map.get("text").and_then(|v| v.as_str()) {
                    let trimmed = text.trim();
                    return (!trimmed.is_empty()).then(|| trimmed.to_string());
                }

                let mut parts = Vec::new();
                if let Some(title) = map.get("title").and_then(|v| v.as_str()) {
                    let trimmed = title.trim();
                    if !trimmed.is_empty() {
                        parts.push(trimmed.to_string());
                    }
                }
                for (key, nested) in map {
                    if key == "type" || key == "title" {
                        continue;
                    }
                    if let Some(text) = extract(nested, " ") {
                        parts.push(text);
                    }
                }

                (!parts.is_empty()).then(|| parts.join(separator))
            }
            _ => None,
        }
    }

    extract(value, " ").unwrap_or_default()
}

fn truncate_title_text(text: &str, max_chars: usize) -> String {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return "Untitled".to_string();
    }
    if trimmed.chars().count() <= max_chars {
        return trimmed.to_string();
    }
    let truncated: String = trimmed.chars().take(max_chars.saturating_sub(1)).collect();
    format!("{}…", truncated.trim_end())
}

fn parse_timestamp_value(
    value: Option<&serde_json::Value>,
) -> Option<chrono::DateTime<chrono::Utc>> {
    value
        .and_then(|v| v.as_str())
        .and_then(|ts| chrono::DateTime::parse_from_rfc3339(ts).ok())
        .map(|dt| dt.with_timezone(&chrono::Utc))
}

fn value_first_text(value: &serde_json::Value) -> Option<&str> {
    match value {
        serde_json::Value::String(text) => Some(text.as_str()),
        serde_json::Value::Array(items) => items.iter().find_map(value_first_text),
        serde_json::Value::Object(map) => map.get("text").and_then(|text| text.as_str()),
        _ => None,
    }
}

#[cfg(test)]
fn message_value_is_internal_system_reminder(message: &serde_json::Value) -> bool {
    message
        .get("content")
        .and_then(value_first_text)
        .is_some_and(|text| text.trim_start().starts_with("<system-reminder>"))
}

fn content_value_starts_with_system_reminder(content: &serde_json::Value) -> bool {
    value_first_text(content).is_some_and(|text| text.trim_start().starts_with("<system-reminder>"))
}

#[cfg(test)]
fn message_value_is_visible_conversation(message: &serde_json::Value) -> bool {
    let has_display_role = message
        .get("display_role")
        .is_some_and(|value| !value.is_null());
    !has_display_role && !message_value_is_internal_system_reminder(message)
}

#[cfg(test)]
fn snapshot_has_visible_conversation(path: &Path) -> Option<bool> {
    let content = std::fs::read_to_string(path).ok()?;
    let value = serde_json::from_str::<serde_json::Value>(&content).ok()?;
    let messages = value.get("messages")?.as_array()?;
    Some(messages.iter().any(message_value_is_visible_conversation))
}

#[cfg(test)]
fn journal_has_visible_conversation(path: &Path) -> Option<bool> {
    let file = File::open(path).ok()?;
    let reader = BufReader::new(file);
    let mut saw_parseable_line = false;
    for line in reader.lines().map_while(|line| line.ok()) {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) else {
            continue;
        };
        saw_parseable_line = true;
        let Some(messages) = value.get("append_messages").and_then(|v| v.as_array()) else {
            continue;
        };
        if messages.iter().any(message_value_is_visible_conversation) {
            return Some(true);
        }
    }
    saw_parseable_line.then_some(false)
}

#[cfg(test)]
fn is_empty_session_file(path: &Path) -> bool {
    let Ok(file) = std::fs::File::open(path) else {
        return true;
    };
    let mut buf = [0u8; 300];
    let n = match file.take(300).read(&mut buf) {
        Ok(n) => n,
        Err(_) => return true,
    };
    let head = &buf[..n];
    head.windows(13).any(|w| w == b"\"messages\":[]")
        || head.windows(14).any(|w| w == b"\"messages\": []")
}

#[cfg(test)]
fn session_has_history(sessions_dir: &Path, stem: &str) -> bool {
    let snapshot_path = sessions_dir.join(format!("{stem}.json"));
    let journal_path = sessions_dir.join(format!("{stem}.journal.jsonl"));

    if journal_has_visible_conversation(&journal_path) == Some(true) {
        return true;
    }

    if let Some(has_visible) = snapshot_has_visible_conversation(&snapshot_path) {
        return has_visible;
    }

    if !is_empty_session_file(&snapshot_path) {
        return true;
    }

    journal_path
        .metadata()
        .map(|meta| meta.len() > 0)
        .unwrap_or(false)
}

fn collect_recent_session_candidates(
    sessions_dir: &Path,
    candidate_limit: usize,
) -> Result<Vec<String>> {
    let mut candidates: BinaryHeap<Reverse<(u128, u64, String)>> = BinaryHeap::new();

    for entry in std::fs::read_dir(sessions_dir)? {
        let entry = entry?;
        let path = entry.path();
        if !path.extension().map(|e| e == "json").unwrap_or(false) {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if stem.starts_with("imported_") {
            continue;
        }

        let key = session_candidate_sort_key(sessions_dir, &path, stem);
        if candidates.len() < candidate_limit {
            candidates.push(Reverse(key));
            continue;
        }

        let should_replace = candidates
            .peek()
            .map(|smallest| key > smallest.0)
            .unwrap_or(true);
        if should_replace {
            candidates.pop();
            candidates.push(Reverse(key));
        }
    }

    let mut out: Vec<(u128, u64, String)> = candidates.into_iter().map(|entry| entry.0).collect();
    out.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| b.1.cmp(&a.1))
            .then_with(|| b.2.cmp(&a.2))
    });
    Ok(out.into_iter().map(|(_, _, stem)| stem).collect())
}

#[cfg(test)]
pub(super) fn collect_recent_session_stems(
    sessions_dir: &Path,
    scan_limit: usize,
) -> Result<Vec<String>> {
    let mut candidate_limit = session_candidate_window(scan_limit);

    loop {
        let candidates = collect_recent_session_candidates(sessions_dir, candidate_limit)?;
        let mut recent = Vec::with_capacity(scan_limit);
        for stem in candidates {
            if !session_has_history(sessions_dir, &stem) {
                continue;
            }
            recent.push(stem);
            if recent.len() >= scan_limit {
                break;
            }
        }

        if recent.len() >= scan_limit || candidate_limit >= MAX_SESSION_SCAN_LIMIT {
            return Ok(recent);
        }

        candidate_limit = candidate_limit
            .saturating_mul(2)
            .min(MAX_SESSION_SCAN_LIMIT);
    }
}

#[derive(Deserialize)]
struct SessionSummary {
    #[serde(default)]
    parent_id: Option<String>,
    #[serde(default)]
    title: Option<String>,
    created_at: chrono::DateTime<chrono::Utc>,
    updated_at: chrono::DateTime<chrono::Utc>,
    #[serde(default)]
    last_active_at: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default)]
    messages: Vec<SessionMessageSummary>,
    #[serde(default)]
    working_dir: Option<String>,
    #[serde(default)]
    short_name: Option<String>,
    #[serde(default)]
    provider_key: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    is_canary: bool,
    #[serde(default)]
    is_debug: bool,
    #[serde(default)]
    saved: bool,
    #[serde(default)]
    save_label: Option<String>,
    #[serde(default)]
    status: SessionStatus,
}

#[derive(Deserialize)]
struct SessionMessageSummary {
    role: Role,
    // `/resume` only needs role/display/token metadata for the initial list.
    // Deserializing full message content here makes large sessions expensive to
    // show, and preview/search content is loaded lazily through the transcript
    // paths when needed. We still retain the one content-derived bit needed to
    // exclude internal system-reminder messages from visible counts.
    #[serde(
        default,
        rename = "content",
        deserialize_with = "deserialize_content_starts_with_system_reminder"
    )]
    content_starts_with_system_reminder: bool,
    #[serde(default)]
    display_role: Option<StoredDisplayRole>,
    #[serde(default)]
    token_usage: Option<SessionTokenUsageSummary>,
}

fn summary_message_is_visible_conversation(message: &SessionMessageSummary) -> bool {
    message.display_role.is_none() && !message.content_starts_with_system_reminder
}

fn deserialize_content_starts_with_system_reminder<'de, D>(
    deserializer: D,
) -> std::result::Result<bool, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(content_value_starts_with_system_reminder(&value))
}

#[derive(Deserialize)]
struct SessionTokenUsageSummary {
    input_tokens: u64,
    output_tokens: u64,
    #[serde(default)]
    cache_read_input_tokens: Option<u64>,
    #[serde(default)]
    cache_creation_input_tokens: Option<u64>,
}

impl SessionTokenUsageSummary {
    fn total_tokens(&self) -> u64 {
        self.input_tokens
            + self.output_tokens
            + self.cache_read_input_tokens.unwrap_or(0)
            + self.cache_creation_input_tokens.unwrap_or(0)
    }
}

#[derive(Deserialize)]
struct SessionJournalSummaryMeta {
    #[serde(default)]
    parent_id: Option<String>,
    #[serde(default)]
    title: Option<String>,
    updated_at: chrono::DateTime<chrono::Utc>,
    #[serde(default)]
    working_dir: Option<String>,
    #[serde(default)]
    short_name: Option<String>,
    #[serde(default)]
    provider_key: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    is_canary: bool,
    #[serde(default)]
    is_debug: bool,
    #[serde(default)]
    saved: bool,
    #[serde(default)]
    save_label: Option<String>,
    #[serde(default)]
    status: SessionStatus,
    #[serde(default)]
    last_active_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Deserialize)]
struct SessionJournalSummaryEntry {
    meta: SessionJournalSummaryMeta,
    #[serde(default)]
    append_messages: Vec<SessionMessageSummary>,
}

fn load_session_summary(path: &Path) -> Result<SessionSummary> {
    let file = File::open(path)?;
    let reader = BufReader::new(file);
    let mut summary: SessionSummary = serde_json::from_reader(reader)?;

    let journal_path = session::session_journal_path_from_snapshot(path);
    if journal_path.exists() {
        let file = File::open(&journal_path)?;
        let reader = BufReader::new(file);
        for (line_idx, line) in reader.lines().enumerate() {
            let line = line?;
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }

            match serde_json::from_str::<SessionJournalSummaryEntry>(trimmed) {
                Ok(entry) => {
                    summary.parent_id = entry.meta.parent_id;
                    summary.title = entry.meta.title;
                    summary.updated_at = entry.meta.updated_at;
                    summary.last_active_at = entry.meta.last_active_at;
                    summary.working_dir = entry.meta.working_dir;
                    summary.short_name = entry.meta.short_name;
                    summary.provider_key = entry.meta.provider_key;
                    summary.model = entry.meta.model;
                    summary.is_canary = entry.meta.is_canary;
                    summary.is_debug = entry.meta.is_debug;
                    summary.saved = entry.meta.saved;
                    summary.save_label = entry.meta.save_label;
                    summary.status = entry.meta.status;
                    summary.messages.extend(entry.append_messages);
                }
                Err(err) => {
                    crate::logging::warn(&format!(
                        "Session picker journal parse failed at {} line {}: {}",
                        journal_path.display(),
                        line_idx + 1,
                        err
                    ));
                    break;
                }
            }
        }
    }

    Ok(summary)
}

pub(super) fn build_messages_preview(session: &Session) -> Vec<PreviewMessage> {
    session::render_messages(session)
        .into_iter()
        .rev()
        .take(20)
        .rev()
        .map(|msg| PreviewMessage {
            role: msg.role,
            content: msg.content,
            tool_calls: msg.tool_calls,
            tool_data: msg.tool_data,
            timestamp: None,
        })
        .collect()
}

pub(super) fn crashed_sessions_from_all_sessions(
    sessions: &[SessionInfo],
) -> Option<CrashedSessionsInfo> {
    let recovered_parents: HashSet<&str> = sessions
        .iter()
        .filter(|s| s.id.starts_with("session_recovery_"))
        .filter_map(|s| s.parent_id.as_deref())
        .collect();

    let mut crashed: Vec<&SessionInfo> = sessions
        .iter()
        .filter(|s| matches!(s.status, SessionStatus::Crashed { .. }))
        .filter(|s| !recovered_parents.contains(s.id.as_str()))
        .collect();
    if crashed.is_empty() {
        return None;
    }

    let crash_timestamp =
        |session: &SessionInfo| session.last_active_at.unwrap_or(session.last_message_time);
    let most_recent = crashed
        .iter()
        .map(|session| crash_timestamp(session))
        .max()?;
    let crash_window = chrono::Duration::seconds(60);
    crashed.retain(|s| {
        let delta = most_recent.signed_duration_since(crash_timestamp(s));
        delta >= chrono::Duration::zero() && delta <= crash_window
    });
    if crashed.is_empty() {
        return None;
    }

    crashed.sort_by(|a, b| b.last_message_time.cmp(&a.last_message_time));

    Some(CrashedSessionsInfo {
        session_ids: crashed.iter().map(|s| s.id.clone()).collect(),
        display_names: crashed.iter().map(|s| s.short_name.clone()).collect(),
        most_recent_crash: most_recent,
    })
}

pub fn load_sessions() -> Result<Vec<SessionInfo>> {
    let sessions_dir = storage::jcode_dir()?.join("sessions");
    let scan_limit = session_scan_limit();

    if let Ok(cache) = session_list_cache().lock()
        && let Some(entry) = cache.as_ref()
        && entry.sessions_dir == sessions_dir
        && entry.scan_limit == scan_limit
        && entry.loaded_at.elapsed() <= SESSION_LIST_CACHE_TTL
    {
        return Ok(entry.sessions.clone());
    }

    let mut sessions: Vec<SessionInfo> = Vec::new();

    let candidates = if sessions_dir.exists() {
        // Keep startup responsive by avoiding `session_has_history` here. That helper parses
        // snapshots/journals, and `load_session_summary` below parses the same files again.
        // Instead, gather a recency-ordered candidate window cheaply from metadata and let the
        // single summary pass filter empty sessions while filling up to `scan_limit` entries.
        collect_recent_session_candidates(&sessions_dir, session_candidate_window(scan_limit))?
    } else {
        Vec::new()
    };

    for stem in candidates {
        if sessions.len() >= scan_limit {
            break;
        }
        if stem.starts_with("imported_cc_")
            || stem.starts_with("imported_codex_")
            || stem.starts_with("imported_pi_")
            || stem.starts_with("imported_opencode_")
        {
            continue;
        }
        let path = sessions_dir.join(format!("{stem}.json"));
        if let Ok(session) = load_session_summary(&path) {
            let short_name = session
                .short_name
                .clone()
                .or_else(|| extract_session_name(&stem).map(|s| s.to_string()))
                .unwrap_or_else(|| stem.clone());
            let icon = session_icon(&short_name);

            let mut user_message_count = 0;
            let mut assistant_message_count = 0;
            let mut estimated_tokens: usize = 0;

            let visible_message_count = session
                .messages
                .iter()
                .filter(|msg| summary_message_is_visible_conversation(msg))
                .count();
            if visible_message_count == 0 {
                continue;
            }

            for msg in session
                .messages
                .iter()
                .filter(|msg| summary_message_is_visible_conversation(msg))
            {
                match msg.role {
                    Role::User => user_message_count += 1,
                    Role::Assistant => assistant_message_count += 1,
                }
                if let Some(usage) = &msg.token_usage {
                    estimated_tokens =
                        estimated_tokens.saturating_add(usage.total_tokens() as usize);
                }
            }

            let status = session.status.clone();
            let needs_catchup = crate::catchup::needs_catchup(&stem, session.updated_at, &status);
            let source = classify_session_source(
                &stem,
                session.provider_key.as_deref(),
                session.model.as_deref(),
            );

            let title = session.title.unwrap_or_else(|| "Untitled".to_string());
            let messages_preview: Vec<PreviewMessage> = Vec::new();
            let search_index = build_search_index_from_summary(
                &stem,
                &short_name,
                &title,
                session.working_dir.as_deref(),
                session.save_label.as_deref(),
            );

            sessions.push(SessionInfo {
                id: stem.to_string(),
                parent_id: session.parent_id,
                short_name,
                icon: icon.to_string(),
                title,
                message_count: visible_message_count,
                user_message_count,
                assistant_message_count,
                created_at: session.created_at,
                last_message_time: session.updated_at,
                last_active_at: session.last_active_at,
                working_dir: session.working_dir,
                model: session.model,
                provider_key: session.provider_key,
                is_canary: session.is_canary,
                is_debug: session.is_debug,
                saved: session.saved,
                save_label: session.save_label,
                status,
                needs_catchup,
                estimated_tokens,
                messages_preview,
                search_index,
                server_name: None,
                server_icon: None,
                source,
                resume_target: ResumeTarget::JcodeSession {
                    session_id: stem.to_string(),
                },
                external_path: None,
            });
        }
    }

    sessions.extend(load_external_claude_code_sessions(scan_limit));
    sessions.extend(load_external_codex_sessions(scan_limit));
    sessions.extend(load_external_pi_sessions(scan_limit));
    sessions.extend(load_external_opencode_sessions(scan_limit));

    sessions.sort_by(|a, b| b.last_message_time.cmp(&a.last_message_time));

    if let Ok(mut cache) = session_list_cache().lock() {
        *cache = Some(SessionListCacheEntry {
            loaded_at: Instant::now(),
            sessions_dir,
            scan_limit,
            sessions: sessions.clone(),
        });
    }

    Ok(sessions)
}

fn load_external_claude_code_sessions(scan_limit: usize) -> Vec<SessionInfo> {
    let Ok(sessions) = crate::import::list_claude_code_sessions_lazy(scan_limit) else {
        return Vec::new();
    };

    sessions
        .into_iter()
        .take(scan_limit)
        .map(|session| {
            let session_id = session.session_id;
            let created_at = session.created.unwrap_or_else(chrono::Utc::now);
            let last_message_time = session.modified.or(session.created).unwrap_or(created_at);
            let working_dir = session.project_path;
            let title = session
                .summary
                .filter(|summary| !summary.trim().is_empty())
                .unwrap_or_else(|| truncate_title_text(&session.first_prompt, 72));
            let short_name = working_dir
                .as_deref()
                .and_then(|dir| Path::new(dir).file_name())
                .and_then(|name| name.to_str())
                .map(|name| name.to_string())
                .unwrap_or_else(|| format!("claude {}", &session_id[..session_id.len().min(8)]));
            let search_index = build_search_index(
                &format!("claude:{session_id}"),
                &short_name,
                &title,
                working_dir.as_deref(),
                None,
                &[],
            );

            SessionInfo {
                id: format!("claude:{session_id}"),
                parent_id: None,
                short_name,
                icon: "🧵".to_string(),
                title,
                message_count: session.message_count as usize,
                user_message_count: 0,
                assistant_message_count: 0,
                created_at,
                last_message_time,
                last_active_at: Some(last_message_time),
                working_dir,
                model: None,
                provider_key: Some("claude-code".to_string()),
                is_canary: false,
                is_debug: false,
                saved: false,
                save_label: None,
                status: SessionStatus::Closed,
                needs_catchup: false,
                estimated_tokens: 0,
                messages_preview: Vec::new(),
                search_index,
                server_name: None,
                server_icon: None,
                source: SessionSource::ClaudeCode,
                resume_target: ResumeTarget::ClaudeCodeSession {
                    session_id,
                    session_path: session.full_path.clone(),
                },
                external_path: Some(session.full_path),
            }
        })
        .collect()
}

pub(super) fn load_claude_code_preview_from_path(path: &Path) -> Option<Vec<PreviewMessage>> {
    let file = File::open(path).ok()?;
    let reader = BufReader::new(file);
    let mut preview = Vec::new();

    for line in reader.lines() {
        let line = line.ok()?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let value: serde_json::Value = serde_json::from_str(trimmed).ok()?;
        let entry_type = value
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        if entry_type != "user" && entry_type != "assistant" {
            continue;
        }
        let Some(message) = value.get("message") else {
            continue;
        };
        let role = message
            .get("role")
            .and_then(|v| v.as_str())
            .unwrap_or(entry_type);
        let text =
            extract_text_from_value(message.get("content").unwrap_or(&serde_json::Value::Null));
        push_preview_message(&mut preview, role, text);
    }

    if preview.is_empty() {
        None
    } else {
        Some(preview)
    }
}

pub(super) fn load_claude_code_preview(session_id: &str) -> Option<Vec<PreviewMessage>> {
    let session = crate::import::list_claude_code_sessions()
        .ok()?
        .into_iter()
        .find(|session| session.session_id == session_id)?;
    load_claude_code_preview_from_path(Path::new(&session.full_path))
}

fn load_external_codex_sessions(scan_limit: usize) -> Vec<SessionInfo> {
    let Ok(root) = crate::storage::user_home_path(".codex/sessions") else {
        return Vec::new();
    };
    if !root.exists() {
        return Vec::new();
    }

    collect_recent_files_recursive(&root, "jsonl", scan_limit)
        .into_iter()
        .filter_map(|path| load_codex_session_stub(&path).ok().flatten())
        .collect()
}

fn load_codex_session_stub(path: &Path) -> Result<Option<SessionInfo>> {
    let file = File::open(path)?;
    let mut lines = BufReader::new(file).lines();
    let Some(first_line) = lines.next() else {
        return Ok(None);
    };
    let header: serde_json::Value = serde_json::from_str(&first_line?)?;
    let meta = if header.get("type").and_then(|v| v.as_str()) == Some("session_meta") {
        header.get("payload").unwrap_or(&header)
    } else {
        &header
    };
    let session_id = meta
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    if session_id.is_empty() {
        return Ok(None);
    }

    let created_at = parse_timestamp_value(meta.get("timestamp"))
        .or_else(|| parse_timestamp_value(header.get("timestamp")))
        .unwrap_or_else(chrono::Utc::now);
    let last_message_time = std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .map(chrono::DateTime::<chrono::Utc>::from)
        .unwrap_or(created_at);
    let working_dir = meta
        .get("cwd")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let short_name = format!("codex {}", &session_id[..session_id.len().min(8)]);
    let title = format!("Codex session {}", &session_id[..session_id.len().min(8)]);
    let search_index = build_search_index(
        &format!("codex:{session_id}"),
        &short_name,
        &title,
        working_dir.as_deref(),
        None,
        &[],
    );

    Ok(Some(SessionInfo {
        id: format!("codex:{session_id}"),
        parent_id: None,
        short_name,
        icon: "🧠".to_string(),
        title,
        message_count: 0,
        user_message_count: 0,
        assistant_message_count: 0,
        created_at,
        last_message_time,
        last_active_at: Some(last_message_time),
        working_dir,
        model: None,
        provider_key: Some("openai-codex".to_string()),
        is_canary: false,
        is_debug: false,
        saved: false,
        save_label: None,
        status: SessionStatus::Closed,
        needs_catchup: false,
        estimated_tokens: 0,
        messages_preview: Vec::new(),
        search_index,
        server_name: None,
        server_icon: None,
        source: SessionSource::Codex,
        resume_target: ResumeTarget::CodexSession {
            session_id,
            session_path: path.to_string_lossy().to_string(),
        },
        external_path: Some(path.to_string_lossy().to_string()),
    }))
}

fn find_codex_session_file(session_id: &str) -> Option<PathBuf> {
    let root = crate::storage::user_home_path(".codex/sessions").ok()?;
    if !root.exists() {
        return None;
    }

    for path in collect_files_recursive(&root, "jsonl") {
        let Ok(file) = File::open(&path) else {
            continue;
        };
        let mut lines = BufReader::new(file).lines();
        let Some(Ok(first_line)) = lines.next() else {
            continue;
        };
        let Ok(header) = serde_json::from_str::<serde_json::Value>(&first_line) else {
            continue;
        };
        let meta = if header.get("type").and_then(|v| v.as_str()) == Some("session_meta") {
            header.get("payload").unwrap_or(&header)
        } else {
            &header
        };
        if meta.get("id").and_then(|v| v.as_str()) == Some(session_id) {
            return Some(path);
        }
    }
    None
}

pub(super) fn load_codex_preview_from_path(path: &Path) -> Option<Vec<PreviewMessage>> {
    let file = File::open(path).ok()?;
    let reader = BufReader::new(file);
    let mut preview = Vec::new();

    for line in reader.lines().skip(1) {
        let line = line.ok()?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let value: serde_json::Value = serde_json::from_str(trimmed).ok()?;
        let line_type = value
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        let (role, content_value) = if line_type == "message" {
            let role = value.get("role").and_then(|v| v.as_str())?;
            (
                role,
                value.get("content").unwrap_or(&serde_json::Value::Null),
            )
        } else if line_type == "response_item" {
            let payload = value.get("payload")?;
            if payload.get("type").and_then(|v| v.as_str()) != Some("message") {
                continue;
            }
            let role = payload.get("role").and_then(|v| v.as_str())?;
            (
                role,
                payload.get("content").unwrap_or(&serde_json::Value::Null),
            )
        } else {
            continue;
        };
        if role != "user" && role != "assistant" {
            continue;
        }
        let text = extract_block_text_from_value(content_value);
        push_preview_message(&mut preview, role, text);
    }

    if preview.is_empty() {
        None
    } else {
        Some(preview)
    }
}

pub(super) fn load_codex_preview(session_id: &str) -> Option<Vec<PreviewMessage>> {
    let path = find_codex_session_file(session_id)?;
    load_codex_preview_from_path(&path)
}

pub(super) fn load_pi_preview_from_path(path: &Path) -> Option<Vec<PreviewMessage>> {
    load_pi_session_info(path)
        .ok()
        .flatten()
        .map(|session| session.messages_preview)
}

fn load_external_pi_sessions(scan_limit: usize) -> Vec<SessionInfo> {
    let Ok(root) = crate::storage::user_home_path(".pi/agent/sessions") else {
        return Vec::new();
    };
    if !root.exists() {
        return Vec::new();
    }

    collect_recent_files_recursive(&root, "jsonl", scan_limit)
        .into_iter()
        .filter_map(|path| load_pi_session_stub(&path).ok().flatten())
        .collect()
}

fn load_pi_session_stub(path: &Path) -> Result<Option<SessionInfo>> {
    let file = File::open(path)?;
    let mut lines = BufReader::new(file).lines();
    let Some(first_line) = lines.next() else {
        return Ok(None);
    };
    let header: serde_json::Value = serde_json::from_str(&first_line?)?;
    if header.get("type").and_then(|v| v.as_str()) != Some("session") {
        return Ok(None);
    }

    let session_id = header
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    if session_id.is_empty() {
        return Ok(None);
    }

    let created_at = header
        .get("timestamp")
        .and_then(|v| v.as_str())
        .and_then(|ts| chrono::DateTime::parse_from_rfc3339(ts).ok())
        .map(|dt| dt.with_timezone(&chrono::Utc))
        .unwrap_or_else(chrono::Utc::now);
    let last_message_time = std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .map(chrono::DateTime::<chrono::Utc>::from)
        .unwrap_or(created_at);
    let working_dir = header
        .get("cwd")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let short_name = format!("pi {}", &session_id[..session_id.len().min(8)]);
    let title = format!("Pi session {}", &session_id[..session_id.len().min(8)]);
    let search_index = build_search_index(
        &format!("pi:{session_id}"),
        &short_name,
        &title,
        working_dir.as_deref(),
        None,
        &[],
    );

    Ok(Some(SessionInfo {
        id: format!("pi:{session_id}"),
        parent_id: None,
        short_name,
        icon: "π".to_string(),
        title,
        message_count: 0,
        user_message_count: 0,
        assistant_message_count: 0,
        created_at,
        last_message_time,
        last_active_at: Some(last_message_time),
        working_dir,
        model: None,
        provider_key: Some("pi".to_string()),
        is_canary: false,
        is_debug: false,
        saved: false,
        save_label: None,
        status: SessionStatus::Closed,
        needs_catchup: false,
        estimated_tokens: 0,
        messages_preview: Vec::new(),
        search_index,
        server_name: None,
        server_icon: None,
        source: SessionSource::Pi,
        resume_target: ResumeTarget::PiSession {
            session_path: path.to_string_lossy().to_string(),
        },
        external_path: Some(path.to_string_lossy().to_string()),
    }))
}

fn load_pi_session_info(path: &Path) -> Result<Option<SessionInfo>> {
    let file = File::open(path)?;
    let reader = BufReader::new(file);
    let mut lines = reader.lines();
    let Some(first_line) = lines.next() else {
        return Ok(None);
    };
    let header: serde_json::Value = serde_json::from_str(&first_line?)?;
    if header.get("type").and_then(|v| v.as_str()) != Some("session") {
        return Ok(None);
    }

    let session_id = header
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    if session_id.is_empty() {
        return Ok(None);
    }

    let created_at = header
        .get("timestamp")
        .and_then(|v| v.as_str())
        .and_then(|ts| chrono::DateTime::parse_from_rfc3339(ts).ok())
        .map(|dt| dt.with_timezone(&chrono::Utc))
        .unwrap_or_else(chrono::Utc::now);
    let working_dir = header
        .get("cwd")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let mut title: Option<String> = None;
    let mut model: Option<String> = None;
    let mut provider_key: Option<String> = Some("pi".to_string());
    let mut last_message_time = created_at;
    let mut user_message_count = 0usize;
    let mut assistant_message_count = 0usize;
    let mut message_count = 0usize;
    let mut preview = Vec::new();

    for line in lines {
        let line = line?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) else {
            continue;
        };

        if let Some(ts) = value
            .get("timestamp")
            .and_then(|v| v.as_str())
            .and_then(|ts| chrono::DateTime::parse_from_rfc3339(ts).ok())
            .map(|dt| dt.with_timezone(&chrono::Utc))
        {
            last_message_time = ts;
        }

        match value.get("type").and_then(|v| v.as_str()) {
            Some("model_change") => {
                provider_key = value
                    .get("provider")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .or(provider_key);
                model = value
                    .get("modelId")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .or(model);
            }
            Some("message") => {
                let Some(message) = value.get("message") else {
                    continue;
                };
                let role = message
                    .get("role")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                let text = extract_text_from_value(
                    message.get("content").unwrap_or(&serde_json::Value::Null),
                );
                if title.is_none() && role == "user" && !text.trim().is_empty() {
                    title = Some(truncate_title_text(&text, 72));
                }
                if model.is_none() {
                    model = message
                        .get("model")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                }
                message_count += 1;
                match role {
                    "user" => user_message_count += 1,
                    "assistant" => assistant_message_count += 1,
                    _ => {}
                }
                push_preview_message(&mut preview, role, text);
            }
            _ => {}
        }
    }

    if message_count == 0 {
        return Ok(None);
    }

    let short_name = format!("pi {}", &session_id[..session_id.len().min(8)]);
    let title =
        title.unwrap_or_else(|| format!("Pi session {}", &session_id[..session_id.len().min(8)]));
    let search_index = build_search_index(
        &format!("pi:{session_id}"),
        &short_name,
        &title,
        working_dir.as_deref(),
        None,
        &preview,
    );

    Ok(Some(SessionInfo {
        id: format!("pi:{session_id}"),
        parent_id: None,
        short_name,
        icon: "π".to_string(),
        title,
        message_count,
        user_message_count,
        assistant_message_count,
        created_at,
        last_message_time,
        last_active_at: Some(last_message_time),
        working_dir,
        model,
        provider_key,
        is_canary: false,
        is_debug: false,
        saved: false,
        save_label: None,
        status: SessionStatus::Closed,
        needs_catchup: false,
        estimated_tokens: 0,
        messages_preview: preview,
        search_index,
        server_name: None,
        server_icon: None,
        source: SessionSource::Pi,
        resume_target: ResumeTarget::PiSession {
            session_path: path.to_string_lossy().to_string(),
        },
        external_path: Some(path.to_string_lossy().to_string()),
    }))
}

fn load_external_opencode_sessions(scan_limit: usize) -> Vec<SessionInfo> {
    let Ok(root) = crate::storage::user_home_path(".local/share/opencode/storage/session") else {
        return Vec::new();
    };
    if !root.exists() {
        return Vec::new();
    }

    collect_recent_files_recursive(&root, "json", scan_limit)
        .into_iter()
        .filter_map(|path| load_opencode_session_stub(&path).ok().flatten())
        .collect()
}

pub(super) fn load_opencode_preview_from_path(path: &Path) -> Option<Vec<PreviewMessage>> {
    load_opencode_session_info(path)
        .ok()
        .flatten()
        .map(|session| session.messages_preview)
}

fn load_opencode_session_stub(path: &Path) -> Result<Option<SessionInfo>> {
    let value: serde_json::Value = serde_json::from_reader(File::open(path)?)?;
    let session_id = value
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    if session_id.is_empty() {
        return Ok(None);
    }

    let created_at = value
        .get("time")
        .and_then(|time| time.get("created"))
        .and_then(|v| v.as_i64())
        .and_then(chrono::DateTime::<chrono::Utc>::from_timestamp_millis)
        .unwrap_or_else(chrono::Utc::now);
    let last_message_time = value
        .get("time")
        .and_then(|time| time.get("updated"))
        .and_then(|v| v.as_i64())
        .and_then(chrono::DateTime::<chrono::Utc>::from_timestamp_millis)
        .unwrap_or(created_at);
    let working_dir = value
        .get("directory")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let short_name = format!("opencode {}", &session_id[..session_id.len().min(8)]);
    let title = value
        .get("title")
        .and_then(|v| v.as_str())
        .map(|s| truncate_title_text(s, 72))
        .unwrap_or_else(|| {
            format!(
                "OpenCode session {}",
                &session_id[..session_id.len().min(8)]
            )
        });
    let search_index = build_search_index(
        &format!("opencode:{session_id}"),
        &short_name,
        &title,
        working_dir.as_deref(),
        None,
        &[],
    );

    Ok(Some(SessionInfo {
        id: format!("opencode:{session_id}"),
        parent_id: None,
        short_name,
        icon: "◌".to_string(),
        title,
        message_count: 0,
        user_message_count: 0,
        assistant_message_count: 0,
        created_at,
        last_message_time,
        last_active_at: Some(last_message_time),
        working_dir,
        model: None,
        provider_key: Some("opencode".to_string()),
        is_canary: false,
        is_debug: false,
        saved: false,
        save_label: None,
        status: SessionStatus::Closed,
        needs_catchup: false,
        estimated_tokens: 0,
        messages_preview: Vec::new(),
        search_index,
        server_name: None,
        server_icon: None,
        source: SessionSource::OpenCode,
        resume_target: ResumeTarget::OpenCodeSession {
            session_id,
            session_path: path.to_string_lossy().to_string(),
        },
        external_path: Some(path.to_string_lossy().to_string()),
    }))
}

fn load_opencode_session_info(path: &Path) -> Result<Option<SessionInfo>> {
    let value: serde_json::Value = serde_json::from_reader(File::open(path)?)?;
    let session_id = value
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    if session_id.is_empty() {
        return Ok(None);
    }

    let created_at = value
        .get("time")
        .and_then(|time| time.get("created"))
        .and_then(|v| v.as_i64())
        .and_then(chrono::DateTime::<chrono::Utc>::from_timestamp_millis)
        .unwrap_or_else(chrono::Utc::now);
    let last_message_time = value
        .get("time")
        .and_then(|time| time.get("updated"))
        .and_then(|v| v.as_i64())
        .and_then(chrono::DateTime::<chrono::Utc>::from_timestamp_millis)
        .unwrap_or(created_at);
    let working_dir = value
        .get("directory")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let title = value
        .get("title")
        .and_then(|v| v.as_str())
        .map(|s| truncate_title_text(s, 72))
        .unwrap_or_else(|| {
            format!(
                "OpenCode session {}",
                &session_id[..session_id.len().min(8)]
            )
        });

    let messages_root = crate::storage::user_home_path(format!(
        ".local/share/opencode/storage/message/{}",
        session_id
    ))?;
    let mut preview = Vec::new();
    let mut user_message_count = 0usize;
    let mut assistant_message_count = 0usize;
    let mut provider_key: Option<String> = Some("opencode".to_string());
    let mut model: Option<String> = None;

    if messages_root.exists() {
        for msg_path in collect_files_recursive(&messages_root, "json") {
            let Ok(msg_value) =
                serde_json::from_reader::<_, serde_json::Value>(File::open(&msg_path)?)
            else {
                continue;
            };
            let role = msg_value
                .get("role")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            let text = msg_value
                .get("summary")
                .map(extract_text_from_value)
                .unwrap_or_default();
            match role {
                "user" => user_message_count += 1,
                "assistant" => assistant_message_count += 1,
                _ => {}
            }
            if model.is_none() {
                model = msg_value
                    .get("modelID")
                    .or_else(|| msg_value.get("model").and_then(|m| m.get("modelID")))
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
            }
            if provider_key.is_none() {
                provider_key = msg_value
                    .get("providerID")
                    .or_else(|| msg_value.get("model").and_then(|m| m.get("providerID")))
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
            }
            push_preview_message(&mut preview, role, text);
        }
    }

    let message_count = user_message_count + assistant_message_count;
    if message_count == 0 {
        return Ok(None);
    }

    let short_name = format!("opencode {}", &session_id[..session_id.len().min(8)]);
    let search_index = build_search_index(
        &format!("opencode:{session_id}"),
        &short_name,
        &title,
        working_dir.as_deref(),
        None,
        &preview,
    );

    Ok(Some(SessionInfo {
        id: format!("opencode:{session_id}"),
        parent_id: None,
        short_name,
        icon: "◌".to_string(),
        title,
        message_count,
        user_message_count,
        assistant_message_count,
        created_at,
        last_message_time,
        last_active_at: Some(last_message_time),
        working_dir,
        model,
        provider_key,
        is_canary: false,
        is_debug: false,
        saved: false,
        save_label: None,
        status: SessionStatus::Closed,
        needs_catchup: false,
        estimated_tokens: 0,
        messages_preview: preview,
        search_index,
        server_name: None,
        server_icon: None,
        source: SessionSource::OpenCode,
        resume_target: ResumeTarget::OpenCodeSession {
            session_id,
            session_path: path.to_string_lossy().to_string(),
        },
        external_path: Some(path.to_string_lossy().to_string()),
    }))
}

pub fn load_servers() -> Vec<ServerInfo> {
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        tokio::task::block_in_place(|| {
            handle.block_on(async { registry::list_servers().await.unwrap_or_default() })
        })
    } else {
        tokio::runtime::Runtime::new()
            .map(|rt| rt.block_on(async { registry::list_servers().await.unwrap_or_default() }))
            .unwrap_or_default()
    }
}

pub fn load_sessions_grouped() -> Result<(Vec<ServerGroup>, Vec<SessionInfo>)> {
    let all_sessions = load_sessions()?;
    let servers = load_servers();

    let mut session_to_server: HashMap<String, &ServerInfo> = HashMap::new();
    for server in &servers {
        for session_name in &server.sessions {
            session_to_server.insert(session_name.clone(), server);
        }
    }

    let mut server_sessions: HashMap<String, Vec<SessionInfo>> = HashMap::new();
    let mut orphan_sessions: Vec<SessionInfo> = Vec::new();

    for mut session in all_sessions {
        if let Some(server) = session_to_server.get(&session.short_name) {
            session.server_name = Some(server.name.clone());
            session.server_icon = Some(server.icon.clone());
            server_sessions
                .entry(server.name.clone())
                .or_default()
                .push(session);
        } else {
            orphan_sessions.push(session);
        }
    }

    let mut groups: Vec<ServerGroup> = servers
        .iter()
        .map(|server| {
            let sessions = server_sessions.remove(&server.name).unwrap_or_default();
            ServerGroup {
                name: server.name.clone(),
                icon: server.icon.clone(),
                version: server.version.clone(),
                git_hash: server.git_hash.clone(),
                is_running: true,
                sessions,
            }
        })
        .collect();

    groups.sort_by(|a, b| {
        let a_latest = a.sessions.iter().map(|s| s.last_message_time).max();
        let b_latest = b.sessions.iter().map(|s| s.last_message_time).max();
        b_latest.cmp(&a_latest)
    });

    Ok((groups, orphan_sessions))
}

#[cfg(test)]
#[path = "loading_tests.rs"]
mod tests;
