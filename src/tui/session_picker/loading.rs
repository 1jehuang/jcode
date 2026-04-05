use crate::id::{extract_session_name, session_icon};
use crate::message::Role;
use crate::registry::{self, ServerInfo};
use crate::session::{self, CrashedSessionsInfo, Session, SessionStatus};
use crate::storage;
use anyhow::Result;
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};

use super::{
    DEFAULT_SESSION_SCAN_LIMIT, MAX_SESSION_SCAN_LIMIT, MIN_SESSION_SCAN_LIMIT, PreviewMessage,
    SEARCH_CONTENT_BUDGET_BYTES, ServerGroup, SessionInfo,
};

use super::{ResumeTarget, SessionSource};

fn session_scan_limit() -> usize {
    std::env::var("JCODE_SESSION_PICKER_MAX_SESSIONS")
        .ok()
        .and_then(|raw| raw.trim().parse::<usize>().ok())
        .map(|n| n.clamp(MIN_SESSION_SCAN_LIMIT, MAX_SESSION_SCAN_LIMIT))
        .unwrap_or(DEFAULT_SESSION_SCAN_LIMIT)
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

fn build_search_index_from_summary(
    id: &str,
    short_name: &str,
    title: &str,
    working_dir: Option<&str>,
    save_label: Option<&str>,
    messages: &[SessionMessageSummary],
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
    for msg in messages {
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

fn session_sort_key(stem: &str) -> u64 {
    for part in stem.split('_') {
        if part.len() == 13 && part.as_bytes().iter().all(|b| b.is_ascii_digit()) {
            if let Ok(ts) = part.parse::<u64>() {
                return ts;
            }
        }
    }

    stem.split('_')
        .rev()
        .find_map(|part| part.parse::<u64>().ok())
        .unwrap_or(0)
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
                if let Some(text) = map.get("text").and_then(|v| v.as_str()) {
                    if !text.trim().is_empty() {
                        out.push(text.trim().to_string());
                    }
                }
                if let Some(text) = map.get("title").and_then(|v| v.as_str()) {
                    if !text.trim().is_empty() {
                        out.push(text.trim().to_string());
                    }
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
}

pub(super) fn collect_recent_session_stems(
    sessions_dir: &Path,
    scan_limit: usize,
) -> Result<Vec<String>> {
    let mut candidates: Vec<(u64, String)> = Vec::new();

    for entry in std::fs::read_dir(sessions_dir)? {
        let entry = entry?;
        let path = entry.path();
        if !path.extension().map(|e| e == "json").unwrap_or(false) {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        candidates.push((session_sort_key(stem), stem.to_string()));
    }

    candidates.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.cmp(&a.1)));

    let mut recent = Vec::with_capacity(scan_limit);
    for (_, stem) in candidates {
        let path = sessions_dir.join(format!("{stem}.json"));
        if is_empty_session_file(&path) {
            continue;
        }
        recent.push(stem);
        if recent.len() >= scan_limit {
            break;
        }
    }

    Ok(recent)
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
    #[serde(default, deserialize_with = "deserialize_content_text")]
    content: String,
    #[serde(default)]
    token_usage: Option<SessionTokenUsageSummary>,
}

fn deserialize_content_text<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de;

    struct ContentVisitor;

    impl<'de> de::Visitor<'de> for ContentVisitor {
        type Value = String;

        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("a string or array of content blocks")
        }

        fn visit_str<E: de::Error>(self, v: &str) -> Result<String, E> {
            Ok(v.to_string())
        }

        fn visit_string<E: de::Error>(self, v: String) -> Result<String, E> {
            Ok(v)
        }

        fn visit_seq<A: de::SeqAccess<'de>>(self, mut seq: A) -> Result<String, A::Error> {
            let mut text = String::new();
            while let Some(block) = seq.next_element::<serde_json::Value>()? {
                if let Some(t) = block.get("text").and_then(|v| v.as_str()) {
                    let block_type = block.get("type").and_then(|v| v.as_str()).unwrap_or("");
                    if block_type == "text" || block_type == "" {
                        if !text.is_empty() {
                            text.push(' ');
                        }
                        text.push_str(t);
                    }
                }
            }
            Ok(text)
        }

        fn visit_unit<E: de::Error>(self) -> Result<String, E> {
            Ok(String::new())
        }

        fn visit_none<E: de::Error>(self) -> Result<String, E> {
            Ok(String::new())
        }
    }

    deserializer.deserialize_any(ContentVisitor)
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
    let mut sessions: Vec<SessionInfo> = Vec::new();

    let candidates = if sessions_dir.exists() {
        collect_recent_session_stems(&sessions_dir, scan_limit)?
    } else {
        Vec::new()
    };

    for stem in candidates {
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

            for msg in &session.messages {
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

            if session.messages.is_empty() {
                continue;
            }

            let title = session.title.unwrap_or_else(|| "Untitled".to_string());
            let messages_preview: Vec<PreviewMessage> = Vec::new();
            let search_index = build_search_index_from_summary(
                &stem,
                &short_name,
                &title,
                session.working_dir.as_deref(),
                session.save_label.as_deref(),
                &session.messages,
            );

            sessions.push(SessionInfo {
                id: stem.to_string(),
                parent_id: session.parent_id,
                short_name,
                icon: icon.to_string(),
                title,
                message_count: session.messages.len(),
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
            });
        }
    }

    sessions.extend(load_external_claude_code_sessions(scan_limit));
    sessions.extend(load_external_codex_sessions(scan_limit));
    sessions.extend(load_external_pi_sessions(scan_limit));
    sessions.extend(load_external_opencode_sessions(scan_limit));

    sessions.sort_by(|a, b| b.last_message_time.cmp(&a.last_message_time));

    Ok(sessions)
}

fn load_external_claude_code_sessions(scan_limit: usize) -> Vec<SessionInfo> {
    let Ok(sessions) = crate::import::list_claude_code_sessions() else {
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
                resume_target: ResumeTarget::ClaudeCodeSession { session_id },
            }
        })
        .collect()
}

pub(super) fn load_claude_code_preview(session_id: &str) -> Option<Vec<PreviewMessage>> {
    let session = crate::import::list_claude_code_sessions()
        .ok()?
        .into_iter()
        .find(|session| session.session_id == session_id)?;
    let file = File::open(session.full_path).ok()?;
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

fn load_external_codex_sessions(scan_limit: usize) -> Vec<SessionInfo> {
    let Ok(root) = crate::storage::user_home_path(".codex/sessions") else {
        return Vec::new();
    };
    if !root.exists() {
        return Vec::new();
    }

    collect_files_recursive(&root, "jsonl")
        .into_iter()
        .take(scan_limit)
        .filter_map(|path| load_codex_session_info(&path).ok().flatten())
        .collect()
}

fn load_codex_session_info(path: &Path) -> Result<Option<SessionInfo>> {
    let file = File::open(path)?;
    let reader = BufReader::new(file);
    let mut lines = reader.lines();
    let Some(first_line) = lines.next() else {
        return Ok(None);
    };
    let header: serde_json::Value = serde_json::from_str(&first_line?)?;
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

    let mut title: Option<String> = None;
    let mut working_dir: Option<String> = None;
    let mut model: Option<String> = None;
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
        if value.get("type").and_then(|v| v.as_str()) != Some("message") {
            continue;
        }
        let Some(role) = value.get("role").and_then(|v| v.as_str()) else {
            continue;
        };
        let text =
            extract_text_from_value(value.get("content").unwrap_or(&serde_json::Value::Null));
        if title.is_none() && role == "user" {
            let cleaned = text.replace("<environment_context>", "");
            let cleaned = cleaned.trim();
            if !cleaned.is_empty() {
                title = Some(truncate_title_text(cleaned, 72));
            }
        }
        if working_dir.is_none() {
            if let Some(content) = value.get("content") {
                let content_text = extract_text_from_value(content);
                if let Some(cwd_line) = content_text.lines().find(|line| line.contains("<cwd>")) {
                    let cwd = cwd_line
                        .replace("<cwd>", "")
                        .replace("</cwd>", "")
                        .trim()
                        .to_string();
                    if !cwd.is_empty() {
                        working_dir = Some(cwd);
                    }
                }
            }
        }
        if model.is_none() {
            model = value
                .get("model")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
        }
        if let Some(ts) = value
            .get("timestamp")
            .and_then(|v| v.as_str())
            .and_then(|ts| chrono::DateTime::parse_from_rfc3339(ts).ok())
            .map(|dt| dt.with_timezone(&chrono::Utc))
        {
            last_message_time = ts;
        }
        message_count += 1;
        match role {
            "user" => user_message_count += 1,
            "assistant" => assistant_message_count += 1,
            _ => {}
        }
        push_preview_message(&mut preview, role, text);
    }

    if message_count == 0 {
        return Ok(None);
    }

    let short_name = format!("codex {}", &session_id[..session_id.len().min(8)]);
    let title = title
        .unwrap_or_else(|| format!("Codex session {}", &session_id[..session_id.len().min(8)]));
    let search_index = build_search_index(
        &format!("codex:{session_id}"),
        &short_name,
        &title,
        working_dir.as_deref(),
        None,
        &preview,
    );

    Ok(Some(SessionInfo {
        id: format!("codex:{session_id}"),
        parent_id: None,
        short_name,
        icon: "🧠".to_string(),
        title,
        message_count,
        user_message_count,
        assistant_message_count,
        created_at,
        last_message_time,
        last_active_at: Some(last_message_time),
        working_dir,
        model,
        provider_key: Some("openai-codex".to_string()),
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
        source: SessionSource::Codex,
        resume_target: ResumeTarget::CodexSession { session_id },
    }))
}

fn load_external_pi_sessions(scan_limit: usize) -> Vec<SessionInfo> {
    let Ok(root) = crate::storage::user_home_path(".pi/agent/sessions") else {
        return Vec::new();
    };
    if !root.exists() {
        return Vec::new();
    }

    collect_files_recursive(&root, "jsonl")
        .into_iter()
        .take(scan_limit)
        .filter_map(|path| load_pi_session_info(&path).ok().flatten())
        .collect()
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
    }))
}

fn load_external_opencode_sessions(scan_limit: usize) -> Vec<SessionInfo> {
    let Ok(root) = crate::storage::user_home_path(".local/share/opencode/storage/session") else {
        return Vec::new();
    };
    if !root.exists() {
        return Vec::new();
    }

    collect_files_recursive(&root, "json")
        .into_iter()
        .take(scan_limit)
        .filter_map(|path| load_opencode_session_info(&path).ok().flatten())
        .collect()
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

    let messages_root = crate::storage::user_home_path(&format!(
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
        resume_target: ResumeTarget::OpenCodeSession { session_id },
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
mod tests {
    use super::*;

    struct EnvVarGuard {
        key: &'static str,
        prev: Option<std::ffi::OsString>,
    }

    impl EnvVarGuard {
        fn set_path(key: &'static str, value: &std::path::Path) -> Self {
            let prev = std::env::var_os(key);
            crate::env::set_var(key, value);
            Self { key, prev }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            if let Some(prev) = self.prev.take() {
                crate::env::set_var(self.key, prev);
            } else {
                crate::env::remove_var(self.key);
            }
        }
    }

    #[test]
    fn load_sessions_includes_claude_code_sessions_from_external_home() {
        let _env_lock = crate::storage::lock_test_env();
        let temp = tempfile::tempdir().expect("temp dir");
        let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());

        let project_dir = temp.path().join("external/.claude/projects/demo-project");
        std::fs::create_dir_all(&project_dir).expect("create project dir");

        let transcript_path = project_dir.join("claude-session-123.jsonl");
        std::fs::write(
            &transcript_path,
            concat!(
                "{\"type\":\"user\",\"uuid\":\"u1\",\"message\":{\"role\":\"user\",\"content\":\"Investigate the login bug\"}}\n",
                "{\"type\":\"assistant\",\"uuid\":\"a1\",\"parentUuid\":\"u1\",\"message\":{\"role\":\"assistant\",\"content\":\"I can help with that.\"}}\n"
            ),
        )
        .expect("write transcript");

        std::fs::write(
            project_dir.join("sessions-index.json"),
            format!(
                concat!(
                    "{{\"version\":1,\"entries\":[",
                    "{{\"sessionId\":\"claude-session-123\",",
                    "\"fullPath\":\"{}\",",
                    "\"firstPrompt\":\"Investigate the login bug\",",
                    "\"summary\":\"Investigate the login bug\",",
                    "\"messageCount\":2,",
                    "\"created\":\"2026-04-04T12:00:00Z\",",
                    "\"modified\":\"2026-04-04T12:05:00Z\",",
                    "\"projectPath\":\"/tmp/demo-project\"",
                    "}}]}}"
                ),
                transcript_path.display()
            ),
        )
        .expect("write index");

        let sessions = load_sessions().expect("load sessions");
        let session = sessions
            .iter()
            .find(|session| {
                matches!(
                    session.resume_target,
                    ResumeTarget::ClaudeCodeSession { .. }
                )
            })
            .expect("claude session present");

        assert_eq!(session.source, SessionSource::ClaudeCode);
        assert_eq!(session.id, "claude:claude-session-123");
        assert_eq!(session.short_name, "demo-project");
        assert_eq!(session.title, "Investigate the login bug");
        assert_eq!(session.message_count, 2);
        assert_eq!(session.working_dir.as_deref(), Some("/tmp/demo-project"));
    }

    #[test]
    fn load_claude_code_preview_reads_transcript_messages() {
        let _env_lock = crate::storage::lock_test_env();
        let temp = tempfile::tempdir().expect("temp dir");
        let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());

        let project_dir = temp.path().join("external/.claude/projects/demo-project");
        std::fs::create_dir_all(&project_dir).expect("create project dir");

        let transcript_path = project_dir.join("claude-session-456.jsonl");
        std::fs::write(
            &transcript_path,
            concat!(
                "{\"type\":\"user\",\"uuid\":\"u1\",\"message\":{\"role\":\"user\",\"content\":[{\"type\":\"text\",\"text\":\"Fix the flaky test\"}]}}\n",
                "{\"type\":\"assistant\",\"uuid\":\"a1\",\"parentUuid\":\"u1\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"I found the race condition\"}]}}\n"
            ),
        )
        .expect("write transcript");

        std::fs::write(
            project_dir.join("sessions-index.json"),
            format!(
                concat!(
                    "{{\"version\":1,\"entries\":[",
                    "{{\"sessionId\":\"claude-session-456\",",
                    "\"fullPath\":\"{}\",",
                    "\"firstPrompt\":\"Fix the flaky test\",",
                    "\"messageCount\":2,",
                    "\"created\":\"2026-04-04T12:00:00Z\",",
                    "\"modified\":\"2026-04-04T12:05:00Z\"",
                    "}}]}}"
                ),
                transcript_path.display()
            ),
        )
        .expect("write index");

        let preview = load_claude_code_preview("claude-session-456").expect("preview");
        assert_eq!(preview.len(), 2);
        assert_eq!(preview[0].role, "user");
        assert!(preview[0].content.contains("Fix the flaky test"));
        assert_eq!(preview[1].role, "assistant");
        assert!(preview[1].content.contains("I found the race condition"));
    }
}
