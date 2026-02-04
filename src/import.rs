//! Import Claude Code sessions into jcode
//!
//! This module handles discovering, parsing, and converting Claude Code sessions
//! so they can be resumed within jcode.

use crate::message::{ContentBlock, Role};
use crate::session::{Session, SessionStatus, StoredMessage};
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;

/// Entry in the Claude Code sessions-index.json file
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

/// Claude Code sessions-index.json format
#[derive(Debug, Deserialize)]
pub struct SessionsIndex {
    pub version: u32,
    pub entries: Vec<SessionIndexEntry>,
}

/// Info about a Claude Code session for listing
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

/// Entry in a Claude Code JSONL session file
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClaudeCodeEntry {
    #[serde(rename = "type")]
    entry_type: String,
    uuid: Option<String>,
    parent_uuid: Option<String>,
    session_id: Option<String>,
    cwd: Option<String>,
    message: Option<ClaudeCodeMessage>,
    timestamp: Option<String>,
    #[serde(default)]
    is_sidechain: bool,
}

/// Message content in Claude Code format
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClaudeCodeMessage {
    role: String,
    #[serde(default)]
    model: Option<String>,
    // Content can be a string or array
    #[serde(default)]
    content: ClaudeCodeContent,
}

/// Content can be either a plain string or array of blocks
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(untagged)]
enum ClaudeCodeContent {
    #[default]
    Empty,
    Text(String),
    Blocks(Vec<ClaudeCodeContentBlock>),
}

/// Individual content block in Claude Code format
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ClaudeCodeContentBlock {
    Text {
        text: String,
    },
    Thinking {
        thinking: String,
        #[serde(default)]
        signature: Option<String>,
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

/// Discover all Claude Code projects and their sessions-index.json files
fn discover_projects() -> Result<Vec<PathBuf>> {
    let claude_dir = dirs::home_dir()
        .context("Could not find home directory")?
        .join(".claude")
        .join("projects");

    if !claude_dir.exists() {
        return Ok(Vec::new());
    }

    let mut index_files = Vec::new();
    for entry in std::fs::read_dir(&claude_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            let index_path = path.join("sessions-index.json");
            if index_path.exists() {
                index_files.push(index_path);
            }
        }
    }

    Ok(index_files)
}

/// List all available Claude Code sessions
pub fn list_claude_code_sessions() -> Result<Vec<ClaudeCodeSessionInfo>> {
    let mut all_sessions = Vec::new();

    for index_path in discover_projects()? {
        let content = std::fs::read_to_string(&index_path)
            .with_context(|| format!("Failed to read {}", index_path.display()))?;

        let index: SessionsIndex = serde_json::from_str(&content)
            .with_context(|| format!("Failed to parse {}", index_path.display()))?;

        for entry in index.entries {
            // Skip sidechains (branched conversations)
            if entry.is_sidechain.unwrap_or(false) {
                continue;
            }

            let created = entry.created.as_ref().and_then(|s| {
                DateTime::parse_from_rfc3339(s).ok().map(|dt| dt.with_timezone(&Utc))
            });
            let modified = entry.modified.as_ref().and_then(|s| {
                DateTime::parse_from_rfc3339(s).ok().map(|dt| dt.with_timezone(&Utc))
            });

            all_sessions.push(ClaudeCodeSessionInfo {
                session_id: entry.session_id,
                first_prompt: entry.first_prompt.unwrap_or_else(|| "No prompt".to_string()),
                summary: entry.summary,
                message_count: entry.message_count.unwrap_or(0),
                created,
                modified,
                project_path: entry.project_path,
                full_path: entry.full_path,
            });
        }
    }

    // Sort by modified date descending
    all_sessions.sort_by(|a, b| {
        let a_date = a.modified.or(a.created);
        let b_date = b.modified.or(b.created);
        b_date.cmp(&a_date)
    });

    Ok(all_sessions)
}

/// List sessions filtered by project path
pub fn list_sessions_for_project(project_filter: &str) -> Result<Vec<ClaudeCodeSessionInfo>> {
    let sessions = list_claude_code_sessions()?;
    Ok(sessions
        .into_iter()
        .filter(|s| {
            s.project_path
                .as_ref()
                .map(|p| p.contains(project_filter))
                .unwrap_or(false)
        })
        .collect())
}

/// Find a session file by ID
fn find_session_file(session_id: &str) -> Result<PathBuf> {
    let sessions = list_claude_code_sessions()?;

    for session in sessions {
        if session.session_id == session_id {
            let path = PathBuf::from(&session.full_path);
            if path.exists() {
                return Ok(path);
            }
        }
    }

    anyhow::bail!("Session {} not found", session_id);
}

/// Convert Claude Code content blocks to jcode ContentBlocks
fn convert_content_blocks(content: &ClaudeCodeContent) -> Vec<ContentBlock> {
    match content {
        ClaudeCodeContent::Empty => vec![],
        ClaudeCodeContent::Text(text) => {
            if text.is_empty() {
                vec![]
            } else {
                vec![ContentBlock::Text {
                    text: text.clone(),
                    cache_control: None,
                }]
            }
        }
        ClaudeCodeContent::Blocks(blocks) => {
            blocks
                .iter()
                .filter_map(|block| match block {
                    ClaudeCodeContentBlock::Text { text } => Some(ContentBlock::Text {
                        text: text.clone(),
                        cache_control: None,
                    }),
                    ClaudeCodeContentBlock::Thinking { thinking, .. } => {
                        Some(ContentBlock::Reasoning {
                            text: thinking.clone(),
                        })
                    }
                    ClaudeCodeContentBlock::ToolUse { id, name, input } => {
                        Some(ContentBlock::ToolUse {
                            id: id.clone(),
                            name: name.clone(),
                            input: input.clone(),
                        })
                    }
                    ClaudeCodeContentBlock::ToolResult {
                        tool_use_id,
                        content,
                        is_error,
                    } => Some(ContentBlock::ToolResult {
                        tool_use_id: tool_use_id.clone(),
                        content: content.clone(),
                        is_error: *is_error,
                    }),
                    ClaudeCodeContentBlock::Unknown => None,
                })
                .collect()
        }
    }
}

/// Import a Claude Code session by ID
pub fn import_session(session_id: &str) -> Result<Session> {
    let session_file = find_session_file(session_id)?;
    import_session_from_file(&session_file, session_id)
}

/// Import a Claude Code session from a file path
pub fn import_session_from_file(path: &PathBuf, session_id: &str) -> Result<Session> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read session file: {}", path.display()))?;

    // Parse JSONL entries
    let mut entries: Vec<ClaudeCodeEntry> = Vec::new();
    for line in content.lines() {
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<ClaudeCodeEntry>(line) {
            Ok(entry) => entries.push(entry),
            Err(e) => {
                // Log but skip malformed lines
                crate::logging::debug(&format!("Skipping malformed entry: {}", e));
            }
        }
    }

    // Filter to actual messages (user/assistant types, not progress/snapshots)
    let message_entries: Vec<&ClaudeCodeEntry> = entries
        .iter()
        .filter(|e| {
            (e.entry_type == "user" || e.entry_type == "assistant")
                && e.message.is_some()
                && !e.is_sidechain
        })
        .collect();

    // Build a map of uuid -> entry for ordering
    let mut uuid_to_entry: HashMap<String, &ClaudeCodeEntry> = HashMap::new();
    for entry in &message_entries {
        if let Some(ref uuid) = entry.uuid {
            uuid_to_entry.insert(uuid.clone(), entry);
        }
    }

    // Find root entries (no parent or parent not in our message set)
    // Then build the conversation in order by following parent_uuid links
    let mut ordered_entries: Vec<&ClaudeCodeEntry> = Vec::new();
    let mut visited: std::collections::HashSet<String> = std::collections::HashSet::new();

    // Find entry with no parent (or parent is not a message entry)
    let roots: Vec<&ClaudeCodeEntry> = message_entries
        .iter()
        .filter(|e| {
            e.parent_uuid.is_none()
                || !uuid_to_entry.contains_key(e.parent_uuid.as_ref().unwrap())
        })
        .copied()
        .collect();

    // For each root, follow the chain
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

            // Find next entry that has this one as parent
            let next = message_entries.iter().find(|e| {
                e.parent_uuid.as_ref() == current.uuid.as_ref() && e.uuid.as_ref().map(|u| !visited.contains(u)).unwrap_or(true)
            });

            match next {
                Some(n) => current = n,
                None => break,
            }
        }
    }

    // Extract metadata from entries
    let first_entry = ordered_entries.first();
    let working_dir = first_entry.and_then(|e| e.cwd.clone());
    // Get model from first assistant message (user messages don't have model)
    let model = ordered_entries
        .iter()
        .find(|e| e.entry_type == "assistant")
        .and_then(|e| e.message.as_ref()?.model.clone());
    let created_at = first_entry
        .and_then(|e| e.timestamp.as_ref())
        .and_then(|t| DateTime::parse_from_rfc3339(t).ok())
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(Utc::now);

    // Get title from first user message or sessions index
    let title = first_entry
        .and_then(|e| {
            if e.entry_type == "user" {
                match &e.message.as_ref()?.content {
                    ClaudeCodeContent::Text(t) => Some(truncate_title(t)),
                    ClaudeCodeContent::Blocks(blocks) => {
                        for b in blocks {
                            if let ClaudeCodeContentBlock::Text { text } = b {
                                return Some(truncate_title(text));
                            }
                        }
                        None
                    }
                    _ => None,
                }
            } else {
                None
            }
        })
        .or_else(|| {
            // Try to get from index
            list_claude_code_sessions().ok()?.into_iter()
                .find(|s| s.session_id == session_id)
                .and_then(|s| s.summary.or(Some(s.first_prompt)))
        });

    // Create jcode session
    let jcode_session_id = format!("imported_cc_{}", session_id);
    let mut session = Session::create_with_id(jcode_session_id, None, title);
    session.provider_session_id = Some(session_id.to_string());
    session.working_dir = working_dir;
    session.model = model;
    session.created_at = created_at;
    session.status = SessionStatus::Closed;

    // Convert messages
    for entry in ordered_entries {
        if let Some(ref msg) = entry.message {
            let role = match msg.role.as_str() {
                "user" => Role::User,
                "assistant" => Role::Assistant,
                _ => continue,
            };

            let content_blocks = convert_content_blocks(&msg.content);

            // Skip empty messages
            if content_blocks.is_empty() {
                continue;
            }

            // Generate message ID from uuid or create new
            let msg_id = entry.uuid.clone().unwrap_or_else(|| crate::id::new_id("msg"));

            session.messages.push(StoredMessage {
                id: msg_id,
                role,
                content: content_blocks,
            });
        }
    }

    // Save the session
    session.save()?;

    Ok(session)
}

/// Truncate a string to use as a title (first line, max 80 chars)
fn truncate_title(s: &str) -> String {
    let first_line = s.lines().next().unwrap_or(s);
    if first_line.len() > 80 {
        format!("{}...", &first_line[..77])
    } else {
        first_line.to_string()
    }
}

/// Print sessions in a formatted table
pub fn print_sessions_table(sessions: &[ClaudeCodeSessionInfo]) {
    if sessions.is_empty() {
        println!("No Claude Code sessions found.");
        return;
    }

    println!("Claude Code Sessions:\n");
    println!(
        "{:<36}  {:>5}  {:<20}  {}",
        "Session ID", "Msgs", "Modified", "First Prompt"
    );
    println!("{}", "-".repeat(100));

    for session in sessions {
        let modified = session
            .modified
            .map(|d| d.format("%Y-%m-%d %H:%M").to_string())
            .unwrap_or_else(|| "Unknown".to_string());

        let prompt = if session.first_prompt.len() > 40 {
            format!("{}...", &session.first_prompt[..37])
        } else {
            session.first_prompt.clone()
        };

        println!(
            "{:<36}  {:>5}  {:<20}  {}",
            session.session_id, session.message_count, modified, prompt
        );
    }

    println!("\nTotal: {} sessions", sessions.len());
    println!("\nTo import a session:");
    println!("  jcode import session <session-id>");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_truncate_title() {
        assert_eq!(truncate_title("short"), "short");
        assert_eq!(truncate_title("line1\nline2"), "line1");

        let long = "a".repeat(100);
        let truncated = truncate_title(&long);
        assert!(truncated.ends_with("..."));
        assert!(truncated.len() <= 80);
    }

    #[test]
    fn test_convert_text_content() {
        let content = ClaudeCodeContent::Text("hello".to_string());
        let blocks = convert_content_blocks(&content);
        assert_eq!(blocks.len(), 1);
        match &blocks[0] {
            ContentBlock::Text { text, .. } => assert_eq!(text, "hello"),
            _ => panic!("Expected text block"),
        }
    }

    #[test]
    fn test_convert_empty_content() {
        let content = ClaudeCodeContent::Empty;
        let blocks = convert_content_blocks(&content);
        assert!(blocks.is_empty());
    }

    #[test]
    fn test_convert_blocks_content() {
        let content = ClaudeCodeContent::Blocks(vec![
            ClaudeCodeContentBlock::Text { text: "hello".to_string() },
            ClaudeCodeContentBlock::Thinking {
                thinking: "let me think".to_string(),
                signature: None,
            },
            ClaudeCodeContentBlock::ToolUse {
                id: "tool1".to_string(),
                name: "bash".to_string(),
                input: serde_json::json!({"cmd": "ls"}),
            },
        ]);
        let blocks = convert_content_blocks(&content);
        assert_eq!(blocks.len(), 3);

        match &blocks[0] {
            ContentBlock::Text { text, .. } => assert_eq!(text, "hello"),
            _ => panic!("Expected text"),
        }
        match &blocks[1] {
            ContentBlock::Reasoning { text } => assert_eq!(text, "let me think"),
            _ => panic!("Expected reasoning"),
        }
        match &blocks[2] {
            ContentBlock::ToolUse { name, .. } => assert_eq!(name, "bash"),
            _ => panic!("Expected tool use"),
        }
    }
}
