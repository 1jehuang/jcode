use chrono::{DateTime, Utc};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, HashSet};
use std::path::{Path, PathBuf};

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
            serde_json::Value::String(text) => {
                if !text.trim().is_empty() {
                    out.push(text.trim().to_string());
                }
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

pub fn extract_text_from_json_value(value: &serde_json::Value) -> String {
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
    let trimmed = s.trim();
    const MAX_CHARS: usize = 80;
    if trimmed.chars().count() <= MAX_CHARS {
        trimmed.to_string()
    } else {
        let mut out = trimmed.chars().take(MAX_CHARS).collect::<String>();
        out.push('…');
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
