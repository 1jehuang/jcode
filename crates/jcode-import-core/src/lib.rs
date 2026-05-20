use chrono::{DateTime, Utc};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, HashSet};
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

pub type ImportCoreResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// Entry in the Claude Code sessions-index.json file.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionIndexEntry {
    pub session_id: String,
    pub full_path: String,
    #[serde(default)]
    pub file_mtime: Option<u64>,
    #[serde(default)]
    pub first_prompt: Option<String>,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub message_count: Option<u32>,
    #[serde(default)]
    pub created: Option<String>,
    #[serde(default)]
    pub modified: Option<String>,
    #[serde(default)]
    pub git_branch: Option<String>,
    #[serde(default)]
    pub project_path: Option<String>,
    #[serde(default)]
    pub is_sidechain: Option<bool>,
}

/// Claude Code sessions-index.json format.
#[derive(Debug, Deserialize)]
pub struct SessionsIndex {
    pub version: u32,
    pub entries: Vec<SessionIndexEntry>,
}

/// Info about a Claude Code session for listing.
#[derive(Debug, Clone)]
pub struct ClaudeCodeSessionInfo {
    pub session_id: String,
    pub first_prompt: String,
    pub summary: Option<String>,
    pub message_count: u32,
    pub created: Option<DateTime<Utc>>,
    pub modified: Option<DateTime<Utc>>,
    pub project_path: Option<String>,
    pub full_path: String,
}

/// Entry in a Claude Code JSONL session file.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeCodeEntry {
    #[serde(rename = "type")]
    pub entry_type: String,
    pub uuid: Option<String>,
    pub parent_uuid: Option<String>,
    #[serde(rename = "sessionId")]
    pub session_id: Option<String>,
    pub cwd: Option<String>,
    pub message: Option<ClaudeCodeMessage>,
    pub timestamp: Option<String>,
    #[serde(default)]
    pub is_sidechain: bool,
}

/// Message content in Claude Code format.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeCodeMessage {
    pub role: String,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub content: ClaudeCodeContent,
}

/// Content can be either a plain string or array of blocks.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(untagged)]
pub enum ClaudeCodeContent {
    #[default]
    Empty,
    Text(String),
    Blocks(Vec<ClaudeCodeContentBlock>),
}

/// Individual content block in Claude Code format.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClaudeCodeContentBlock {
    Text {
        text: String,
    },
    Thinking {
        thinking: String,
        #[serde(default)]
        #[serde(rename = "signature")]
        _signature: Option<String>,
    },
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
        #[serde(default)]
        is_error: Option<bool>,
    },
    #[serde(other)]
    Unknown,
}

pub fn parse_rfc3339_string(value: Option<&str>) -> Option<DateTime<Utc>> {
    value
        .and_then(|ts| DateTime::parse_from_rfc3339(ts).ok())
        .map(|dt| dt.with_timezone(&Utc))
}

pub fn clean_optional_text(value: Option<String>) -> Option<String> {
    value.and_then(|text| {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    })
}

pub fn resolve_claude_session_path(
    project_dir: &Path,
    entry: &SessionIndexEntry,
) -> Option<PathBuf> {
    let indexed_path = PathBuf::from(&entry.full_path);
    let fallback_path = project_dir.join(format!("{}.jsonl", entry.session_id));
    if indexed_path.exists() {
        Some(indexed_path)
    } else if fallback_path.exists() {
        Some(fallback_path)
    } else {
        None
    }
}

pub fn claude_code_session_info_from_index(
    path: &Path,
    entry: &SessionIndexEntry,
) -> Option<ClaudeCodeSessionInfo> {
    let message_count = entry.message_count.filter(|count| *count > 0)?;
    let summary = clean_optional_text(entry.summary.clone());
    let first_prompt =
        clean_optional_text(entry.first_prompt.clone()).or_else(|| summary.clone())?;

    Some(ClaudeCodeSessionInfo {
        session_id: entry.session_id.clone(),
        first_prompt,
        summary,
        message_count,
        created: parse_rfc3339_string(entry.created.as_deref()),
        modified: parse_rfc3339_string(entry.modified.as_deref()),
        project_path: clean_optional_text(entry.project_path.clone()),
        full_path: path.to_string_lossy().to_string(),
    })
}

pub fn claude_text_from_content(content: &ClaudeCodeContent) -> Option<String> {
    match content {
        ClaudeCodeContent::Empty => None,
        ClaudeCodeContent::Text(text) => {
            let text = text.trim();
            if text.is_empty() {
                None
            } else {
                Some(text.to_string())
            }
        }
        ClaudeCodeContent::Blocks(blocks) => {
            let text = blocks
                .iter()
                .filter_map(|block| match block {
                    ClaudeCodeContentBlock::Text { text } => Some(text.trim()),
                    ClaudeCodeContentBlock::Thinking { thinking, .. } => Some(thinking.trim()),
                    ClaudeCodeContentBlock::ToolResult { content, .. } => Some(content.trim()),
                    _ => None,
                })
                .filter(|text| !text.is_empty())
                .collect::<Vec<_>>()
                .join("\n");
            if text.is_empty() { None } else { Some(text) }
        }
    }
}

pub fn ordered_claude_code_message_entries(entries: &[ClaudeCodeEntry]) -> Vec<&ClaudeCodeEntry> {
    let message_entries: Vec<&ClaudeCodeEntry> = entries
        .iter()
        .filter(|e| {
            (e.entry_type == "user" || e.entry_type == "assistant")
                && e.message.is_some()
                && !e.is_sidechain
        })
        .collect();

    let mut uuid_to_entry: HashMap<String, &ClaudeCodeEntry> = HashMap::new();
    for entry in &message_entries {
        if let Some(ref uuid) = entry.uuid {
            uuid_to_entry.insert(uuid.clone(), entry);
        }
    }

    let mut ordered_entries: Vec<&ClaudeCodeEntry> = Vec::new();
    let mut visited: HashSet<String> = HashSet::new();

    let roots: Vec<&ClaudeCodeEntry> = message_entries
        .iter()
        .filter(|e| {
            e.parent_uuid.is_none()
                || !uuid_to_entry.contains_key(e.parent_uuid.as_deref().unwrap_or_default())
        })
        .copied()
        .collect();

    for root in roots {
        let mut current = root;
        loop {
            if let Some(ref uuid) = current.uuid {
                if visited.contains(uuid) {
                    break;
                }
                visited.insert(uuid.clone());
            }
            ordered_entries.push(current);

            let next = message_entries.iter().find(|e| {
                e.parent_uuid.as_ref() == current.uuid.as_ref()
                    && e.uuid
                        .as_ref()
                        .map(|u| !visited.contains(u))
                        .unwrap_or(true)
            });

            match next {
                Some(n) => current = n,
                None => break,
            }
        }
    }

    for entry in message_entries {
        if entry
            .uuid
            .as_ref()
            .map(|uuid| visited.contains(uuid))
            .unwrap_or(false)
        {
            continue;
        }
        ordered_entries.push(entry);
    }

    ordered_entries
}

pub fn collect_files_recursive(root: &Path, extension: &str) -> Vec<PathBuf> {
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
    files.sort();
    files
}

pub fn collect_recent_files_recursive(root: &Path, extension: &str, limit: usize) -> Vec<PathBuf> {
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

pub fn parse_rfc3339_json(value: Option<&serde_json::Value>) -> Option<DateTime<Utc>> {
    value
        .and_then(|v| v.as_str())
        .and_then(|ts| DateTime::parse_from_rfc3339(ts).ok())
        .map(|dt| dt.with_timezone(&Utc))
}

pub fn extract_external_text_from_json(value: &serde_json::Value, include_tools: bool) -> String {
    fn visit(value: &serde_json::Value, include_tools: bool, out: &mut Vec<String>) {
        match value {
            serde_json::Value::String(text) if !text.trim().is_empty() => {
                out.push(text.trim().to_string());
            }
            serde_json::Value::Array(items) => {
                for item in items {
                    visit(item, include_tools, out);
                }
            }
            serde_json::Value::Object(map) => {
                let block_type = map.get("type").and_then(|v| v.as_str()).unwrap_or_default();
                if !include_tools
                    && matches!(block_type, "tool_use" | "tool_result" | "function_call")
                {
                    return;
                }
                if let Some(text) = map.get("text").and_then(|v| v.as_str()) {
                    if !text.trim().is_empty() {
                        out.push(text.trim().to_string());
                    }
                } else if include_tools
                    && let Some(content) = map.get("content").and_then(|v| v.as_str())
                    && !content.trim().is_empty()
                {
                    out.push(content.trim().to_string());
                }
                for (key, nested) in map {
                    if matches!(key.as_str(), "type" | "text" | "content") {
                        continue;
                    }
                    visit(nested, include_tools, out);
                }
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    visit(value, include_tools, &mut out);
    out.join("\n")
}

pub fn file_modified_datetime(path: &Path) -> Option<DateTime<Utc>> {
    std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
        .map(DateTime::<Utc>::from)
}

#[derive(Debug, Clone)]
pub struct ExternalMessageRecord {
    pub role: String,
    pub text: String,
    pub timestamp: Option<DateTime<Utc>>,
    pub id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ExternalSessionRecord {
    pub source: &'static str,
    pub session_id: String,
    pub short_name: Option<String>,
    pub title: Option<String>,
    pub working_dir: Option<String>,
    pub provider_key: Option<String>,
    pub model: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub path: PathBuf,
    pub messages: Vec<ExternalMessageRecord>,
}

pub fn load_claude_external_messages(
    path: &Path,
    include_tools: bool,
) -> Vec<ExternalMessageRecord> {
    let Ok(file) = File::open(path) else {
        return Vec::new();
    };
    BufReader::new(file)
        .lines()
        .map_while(|line| line.ok())
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line.trim()).ok())
        .filter_map(|value| {
            let entry_type = value
                .get("type")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            if entry_type != "user" && entry_type != "assistant" {
                return None;
            }
            let message = value.get("message")?;
            let role = message
                .get("role")
                .and_then(|v| v.as_str())
                .unwrap_or(entry_type)
                .to_string();
            let text = extract_external_text_from_json(
                message.get("content").unwrap_or(&serde_json::Value::Null),
                include_tools,
            );
            if text.trim().is_empty() {
                return None;
            }
            Some(ExternalMessageRecord {
                role,
                text,
                timestamp: parse_rfc3339_json(value.get("timestamp")),
                id: value
                    .get("uuid")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
            })
        })
        .collect()
}

pub fn load_codex_external_session(
    path: &Path,
    include_tools: bool,
) -> ImportCoreResult<Option<ExternalSessionRecord>> {
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
    let session_id = meta.get("id").and_then(|v| v.as_str()).unwrap_or_default();
    if session_id.is_empty() {
        return Ok(None);
    }
    let created_at = parse_rfc3339_json(meta.get("timestamp"))
        .or_else(|| parse_rfc3339_json(header.get("timestamp")))
        .unwrap_or_else(Utc::now);
    let mut updated_at = file_modified_datetime(path).unwrap_or(created_at);
    let working_dir = meta.get("cwd").and_then(|v| v.as_str()).map(str::to_string);
    let mut messages = Vec::new();
    for line in lines.map_while(|line| line.ok()) {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line.trim()) else {
            continue;
        };
        let line_type = value
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        let (role, content_value) = if line_type == "message" {
            let Some(role) = value.get("role").and_then(|v| v.as_str()) else {
                continue;
            };
            (
                role,
                value.get("content").unwrap_or(&serde_json::Value::Null),
            )
        } else if line_type == "response_item" {
            let Some(payload) = value.get("payload") else {
                continue;
            };
            if payload.get("type").and_then(|v| v.as_str()) != Some("message") {
                continue;
            }
            let Some(role) = payload.get("role").and_then(|v| v.as_str()) else {
                continue;
            };
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
        let text = extract_external_text_from_json(content_value, include_tools);
        if text.trim().is_empty() {
            continue;
        }
        let timestamp = parse_rfc3339_json(value.get("timestamp"));
        if let Some(ts) = timestamp {
            updated_at = updated_at.max(ts);
        }
        messages.push(ExternalMessageRecord {
            role: role.to_string(),
            text,
            timestamp,
            id: value.get("id").and_then(|v| v.as_str()).map(str::to_string),
        });
    }
    Ok(Some(ExternalSessionRecord {
        source: "codex",
        session_id: session_id.to_string(),
        short_name: Some(format!("codex {}", &session_id[..session_id.len().min(8)])),
        title: Some(format!(
            "Codex session {}",
            &session_id[..session_id.len().min(8)]
        )),
        working_dir,
        provider_key: Some("openai-codex".to_string()),
        model: None,
        created_at,
        updated_at,
        path: path.to_path_buf(),
        messages,
    }))
}

pub fn load_pi_external_session(
    path: &Path,
    include_tools: bool,
) -> ImportCoreResult<Option<ExternalSessionRecord>> {
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
        .unwrap_or_default();
    if session_id.is_empty() {
        return Ok(None);
    }
    let created_at = parse_rfc3339_json(header.get("timestamp")).unwrap_or_else(Utc::now);
    let mut updated_at = file_modified_datetime(path).unwrap_or(created_at);
    let working_dir = header
        .get("cwd")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let mut provider_key = Some("pi".to_string());
    let mut model = None;
    let mut messages = Vec::new();
    for line in lines.map_while(|line| line.ok()) {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line.trim()) else {
            continue;
        };
        if let Some(ts) = parse_rfc3339_json(value.get("timestamp")) {
            updated_at = updated_at.max(ts);
        }
        match value.get("type").and_then(|v| v.as_str()) {
            Some("model_change") => {
                provider_key = value
                    .get("provider")
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
                    .or(provider_key);
                model = value
                    .get("modelId")
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
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
                if role != "user" && role != "assistant" {
                    continue;
                }
                let text = extract_external_text_from_json(
                    message.get("content").unwrap_or(&serde_json::Value::Null),
                    include_tools,
                );
                if text.trim().is_empty() {
                    continue;
                }
                messages.push(ExternalMessageRecord {
                    role: role.to_string(),
                    text,
                    timestamp: parse_rfc3339_json(value.get("timestamp")),
                    id: value.get("id").and_then(|v| v.as_str()).map(str::to_string),
                });
            }
            _ => {}
        }
    }
    Ok(Some(ExternalSessionRecord {
        source: "pi",
        session_id: session_id.to_string(),
        short_name: Some(format!("pi {}", &session_id[..session_id.len().min(8)])),
        title: Some(format!(
            "Pi session {}",
            &session_id[..session_id.len().min(8)]
        )),
        working_dir,
        provider_key,
        model,
        created_at,
        updated_at,
        path: path.to_path_buf(),
        messages,
    }))
}

pub fn load_opencode_external_session(
    path: &Path,
    messages_base: &Path,
    include_tools: bool,
    max_scan_sessions: usize,
) -> ImportCoreResult<Option<ExternalSessionRecord>> {
    let value: serde_json::Value = serde_json::from_reader(File::open(path)?)?;
    let session_id = value.get("id").and_then(|v| v.as_str()).unwrap_or_default();
    if session_id.is_empty() {
        return Ok(None);
    }
    let created_at = value
        .get("time")
        .and_then(|time| time.get("created"))
        .and_then(|v| v.as_i64())
        .and_then(DateTime::<Utc>::from_timestamp_millis)
        .unwrap_or_else(Utc::now);
    let updated_at = value
        .get("time")
        .and_then(|time| time.get("updated"))
        .and_then(|v| v.as_i64())
        .and_then(DateTime::<Utc>::from_timestamp_millis)
        .or_else(|| file_modified_datetime(path))
        .unwrap_or(created_at);
    let working_dir = value
        .get("directory")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let title = value
        .get("title")
        .and_then(|v| v.as_str())
        .map(|title| truncate_title_text(title, 72))
        .unwrap_or_else(|| {
            format!(
                "OpenCode session {}",
                &session_id[..session_id.len().min(8)]
            )
        });
    let mut provider_key = Some("opencode".to_string());
    let mut model = None;
    let mut messages = Vec::new();
    let messages_root = messages_base.join(session_id);
    if messages_root.exists() {
        for msg_path in collect_recent_files_recursive(&messages_root, "json", max_scan_sessions) {
            let Ok(msg_value) =
                serde_json::from_reader::<_, serde_json::Value>(File::open(&msg_path)?)
            else {
                continue;
            };
            let role = msg_value
                .get("role")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            if role != "user" && role != "assistant" {
                continue;
            }
            if model.is_none() {
                model = msg_value
                    .get("modelID")
                    .or_else(|| msg_value.get("model").and_then(|m| m.get("modelID")))
                    .and_then(|v| v.as_str())
                    .map(str::to_string);
            }
            provider_key = msg_value
                .get("providerID")
                .or_else(|| msg_value.get("model").and_then(|m| m.get("providerID")))
                .and_then(|v| v.as_str())
                .map(str::to_string)
                .or(provider_key);
            let text = msg_value
                .get("summary")
                .or_else(|| msg_value.get("content"))
                .map(|value| extract_external_text_from_json(value, include_tools))
                .unwrap_or_default();
            if text.trim().is_empty() {
                continue;
            }
            messages.push(ExternalMessageRecord {
                role: role.to_string(),
                text,
                timestamp: None,
                id: msg_value
                    .get("id")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
            });
        }
    }
    Ok(Some(ExternalSessionRecord {
        source: "opencode",
        session_id: session_id.to_string(),
        short_name: Some(format!(
            "opencode {}",
            &session_id[..session_id.len().min(8)]
        )),
        title: Some(title),
        working_dir,
        provider_key,
        model,
        created_at,
        updated_at,
        path: path.to_path_buf(),
        messages,
    }))
}

fn truncate_title_text(text: &str, max_chars: usize) -> String {
    let trimmed = text.trim();
    if trimmed.chars().count() <= max_chars {
        trimmed.to_string()
    } else {
        format!(
            "{}…",
            trimmed
                .chars()
                .take(max_chars.saturating_sub(1))
                .collect::<String>()
        )
    }
}

pub fn extract_text_from_json_value(value: &serde_json::Value) -> String {
    fn visit(value: &serde_json::Value, out: &mut Vec<String>) {
        match value {
            serde_json::Value::String(text) if !text.trim().is_empty() => {
                out.push(text.trim().to_string());
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
                    return;
                }
                if let Some(text) = map.get("title").and_then(|v| v.as_str())
                    && !text.trim().is_empty()
                {
                    out.push(text.trim().to_string());
                }
                for (key, nested) in map {
                    if key == "type" || key == "title" {
                        continue;
                    }
                    visit(nested, out);
                }
            }
            _ => {}
        }
    }

    let mut out = Vec::new();
    visit(value, &mut out);
    out.join(" ")
}

pub fn truncate_title(s: &str) -> String {
    let trimmed = s.lines().next().unwrap_or_default().trim();
    const MAX_CHARS: usize = 80;
    if trimmed.chars().count() <= MAX_CHARS {
        trimmed.to_string()
    } else {
        let mut out = trimmed
            .chars()
            .take(MAX_CHARS.saturating_sub(3))
            .collect::<String>();
        out.push_str("...");
        out
    }
}

pub fn codex_title_candidate(text: &str) -> Option<String> {
    let cleaned = text.replace("<environment_context>", "");
    let cleaned = cleaned.trim();
    if cleaned.is_empty() {
        return None;
    }
    if cleaned.starts_with("# AGENTS.md instructions")
        || cleaned.starts_with("<permissions instructions>")
        || cleaned.contains("\n<INSTRUCTIONS>")
    {
        return None;
    }
    Some(truncate_title(cleaned))
}

pub fn imported_claude_code_session_id(session_id: &str) -> String {
    format!("imported_cc_{}", session_id)
}

pub fn imported_codex_session_id(session_id: &str) -> String {
    format!("imported_codex_{}", session_id)
}

pub fn imported_opencode_session_id(session_id: &str) -> String {
    format!("imported_opencode_{}", session_id)
}

pub fn imported_pi_session_id(session_path: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(session_path.as_bytes());
    let digest = hasher.finalize();
    format!("imported_pi_{}", hex::encode(&digest[..8]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_optional_text_trims_and_drops_empty() {
        assert_eq!(
            clean_optional_text(Some("  hello  ".into())),
            Some("hello".into())
        );
        assert_eq!(clean_optional_text(Some("   ".into())), None);
        assert_eq!(clean_optional_text(None), None);
    }

    #[test]
    fn claude_text_from_blocks_joins_textual_content() {
        let content = ClaudeCodeContent::Blocks(vec![
            ClaudeCodeContentBlock::Text {
                text: " hello ".into(),
            },
            ClaudeCodeContentBlock::Thinking {
                thinking: " thought ".into(),
                _signature: None,
            },
            ClaudeCodeContentBlock::ToolResult {
                tool_use_id: "tool".into(),
                content: " result ".into(),
                is_error: None,
            },
            ClaudeCodeContentBlock::Unknown,
        ]);
        assert_eq!(
            claude_text_from_content(&content),
            Some("hello\nthought\nresult".into())
        );
    }

    #[test]
    fn ordered_claude_entries_follow_parent_chain() {
        let jsonl = [
            r#"{"type":"assistant","uuid":"b","parentUuid":"a","message":{"role":"assistant","content":"there"}}"#,
            r#"{"type":"user","uuid":"a","message":{"role":"user","content":"hi"}}"#,
        ];
        let entries = jsonl
            .iter()
            .map(|line| serde_json::from_str::<ClaudeCodeEntry>(line).unwrap())
            .collect::<Vec<_>>();
        let ordered = ordered_claude_code_message_entries(&entries);
        assert_eq!(
            ordered
                .iter()
                .map(|entry| entry.uuid.as_deref())
                .collect::<Vec<_>>(),
            vec![Some("a"), Some("b")]
        );
    }

    #[test]
    fn imported_pi_id_is_stable_and_prefixed() {
        assert_eq!(
            imported_pi_session_id("/tmp/session"),
            imported_pi_session_id("/tmp/session")
        );
        assert!(imported_pi_session_id("/tmp/session").starts_with("imported_pi_"));
    }

    #[test]
    fn collect_recent_files_returns_empty_for_zero_limit() {
        assert!(collect_recent_files_recursive(Path::new("."), "rs", 0).is_empty());
    }

    #[test]
    fn extract_external_text_respects_include_tools() {
        let value = serde_json::json!([
            {"type": "text", "text": " hello "},
            {"type": "tool_result", "content": " tool output "}
        ]);
        assert_eq!(extract_external_text_from_json(&value, false), "hello");
        assert_eq!(
            extract_external_text_from_json(&value, true),
            "hello\ntool output"
        );
    }

    #[test]
    fn extract_text_from_json_collects_nested_text() {
        let value = serde_json::json!({
            "type": "message",
            "content": [
                {"type": "text", "text": " hello "},
                {"title": "ignored title", "other": " world "}
            ]
        });
        assert_eq!(
            extract_text_from_json_value(&value),
            "hello ignored title world"
        );
    }

    #[test]
    fn codex_title_candidate_filters_environment_noise() {
        assert_eq!(
            codex_title_candidate("<environment_context> Build feature"),
            Some("Build feature".into())
        );
        assert_eq!(
            codex_title_candidate(
                "# AGENTS.md instructions
Do x"
            ),
            None
        );
    }
}
