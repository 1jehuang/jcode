//! Import Claude Code sessions into jcode
//!
//! This module handles discovering, parsing, and converting Claude Code sessions
//! so they can be resumed within jcode.

use crate::message::{ContentBlock, Role};
use crate::session::{Session, SessionStatus, StoredMessage};
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use jcode_import_core::{
    ClaudeCodeContent, ClaudeCodeContentBlock, ClaudeCodeEntry, ClaudeCodeSessionInfo,
    SessionIndexEntry, SessionsIndex, claude_code_session_info_from_index,
    claude_text_from_content, clean_optional_text, codex_title_candidate, collect_files_recursive,
    collect_recent_files_recursive, extract_text_from_json_value,
    ordered_claude_code_message_entries, parse_rfc3339_json, parse_rfc3339_string,
    resolve_claude_session_path, truncate_title,
};
pub use jcode_import_core::{
    imported_claude_code_session_id, imported_codex_session_id, imported_opencode_session_id,
    imported_pi_session_id,
};
use std::collections::HashSet;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::path::PathBuf;

/// Discover all Claude Code project directories under ~/.claude/projects.
fn discover_project_dirs() -> Result<Vec<PathBuf>> {
    let claude_dir = crate::storage::user_home_path(".claude/projects")
        .context("Could not find Claude projects directory")?;

    if !claude_dir.exists() {
        return Ok(Vec::new());
    }

    let mut project_dirs = Vec::new();
    for entry in std::fs::read_dir(&claude_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            project_dirs.push(path);
        }
    }

    project_dirs.sort();
    Ok(project_dirs)
}

/// Discover all Claude Code projects and their sessions-index.json files.
#[cfg(test)]
fn discover_projects() -> Result<Vec<PathBuf>> {
    Ok(discover_project_dirs()?
        .into_iter()
        .map(|dir| dir.join("sessions-index.json"))
        .filter(|path| path.exists())
        .collect())
}

fn load_claude_code_entries(path: &Path) -> Result<Vec<ClaudeCodeEntry>> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read session file: {}", path.display()))?;

    let mut entries = Vec::new();
    for line in content.lines() {
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<ClaudeCodeEntry>(line) {
            Ok(entry) => entries.push(entry),
            Err(e) => {
                crate::logging::debug(&format!(
                    "Skipping malformed Claude Code entry in {}: {}",
                    path.display(),
                    e
                ));
            }
        }
    }
    Ok(entries)
}

fn claude_code_session_info_from_file(
    path: &Path,
    indexed: Option<&SessionIndexEntry>,
) -> Result<ClaudeCodeSessionInfo> {
    let entries = load_claude_code_entries(path)?;
    let ordered_entries = ordered_claude_code_message_entries(&entries);
    let first_entry = ordered_entries.first().copied();
    let last_entry = ordered_entries.last().copied();

    let session_id = indexed
        .map(|entry| entry.session_id.clone())
        .or_else(|| {
            entries
                .iter()
                .find_map(|entry| entry.session_id.clone())
                .or_else(|| {
                    path.file_stem()
                        .and_then(|stem| stem.to_str())
                        .map(|s| s.to_string())
                })
        })
        .unwrap_or_else(|| path.to_string_lossy().to_string());

    let first_prompt = indexed
        .and_then(|entry| clean_optional_text(entry.first_prompt.clone()))
        .or_else(|| {
            ordered_entries.iter().find_map(|entry| {
                (entry.entry_type == "user")
                    .then_some(entry.message.as_ref())
                    .flatten()
                    .and_then(|message| claude_text_from_content(&message.content))
            })
        })
        .or_else(|| indexed.and_then(|entry| clean_optional_text(entry.summary.clone())))
        .unwrap_or_else(|| "No prompt".to_string());

    let summary = indexed.and_then(|entry| clean_optional_text(entry.summary.clone()));
    let message_count = indexed
        .and_then(|entry| entry.message_count)
        .filter(|count| *count > 0)
        .unwrap_or(ordered_entries.len() as u32);
    let created = indexed
        .and_then(|entry| parse_rfc3339_string(entry.created.as_deref()))
        .or_else(|| first_entry.and_then(|entry| parse_rfc3339_string(entry.timestamp.as_deref())));
    let modified = indexed
        .and_then(|entry| parse_rfc3339_string(entry.modified.as_deref()))
        .or_else(|| last_entry.and_then(|entry| parse_rfc3339_string(entry.timestamp.as_deref())));
    let project_path = indexed
        .and_then(|entry| clean_optional_text(entry.project_path.clone()))
        .or_else(|| first_entry.and_then(|entry| entry.cwd.clone()));

    Ok(ClaudeCodeSessionInfo {
        session_id,
        first_prompt,
        summary,
        message_count,
        created,
        modified,
        project_path,
        full_path: path.to_string_lossy().to_string(),
    })
}

/// List all available Claude Code sessions
pub fn list_claude_code_sessions() -> Result<Vec<ClaudeCodeSessionInfo>> {
    let mut all_sessions = Vec::new();
    let mut seen_session_ids = HashSet::new();

    for project_dir in discover_project_dirs()? {
        let index_path = project_dir.join("sessions-index.json");
        if index_path.exists() {
            let content = std::fs::read_to_string(&index_path)
                .with_context(|| format!("Failed to read {}", index_path.display()))?;

            let index: SessionsIndex = serde_json::from_str(&content)
                .with_context(|| format!("Failed to parse {}", index_path.display()))?;

            for entry in index.entries {
                if entry.is_sidechain.unwrap_or(false) {
                    continue;
                }

                let Some(path) = resolve_claude_session_path(&project_dir, &entry) else {
                    continue;
                };

                let session =
                    if let Some(session) = claude_code_session_info_from_index(&path, &entry) {
                        session
                    } else {
                        let session = claude_code_session_info_from_file(&path, Some(&entry))?;
                        if session.message_count == 0
                            || (session.summary.is_none() && session.first_prompt == "No prompt")
                        {
                            continue;
                        }
                        session
                    };
                seen_session_ids.insert(session.session_id.clone());
                all_sessions.push(session);
            }
        }

        for path in collect_files_recursive(&project_dir, "jsonl") {
            let Some(session_id) = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .map(|stem| stem.to_string())
            else {
                continue;
            };
            if seen_session_ids.contains(&session_id) {
                continue;
            }
            let session = claude_code_session_info_from_file(&path, None)?;
            if session.message_count == 0
                || (session.summary.is_none() && session.first_prompt == "No prompt")
            {
                continue;
            }
            seen_session_ids.insert(session.session_id.clone());
            all_sessions.push(session);
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

pub fn list_claude_code_sessions_lazy(scan_limit: usize) -> Result<Vec<ClaudeCodeSessionInfo>> {
    let mut all_sessions = Vec::new();
    let mut seen_session_ids = HashSet::new();

    for project_dir in discover_project_dirs()? {
        let index_path = project_dir.join("sessions-index.json");
        if index_path.exists() {
            let content = std::fs::read_to_string(&index_path)
                .with_context(|| format!("Failed to read {}", index_path.display()))?;
            let index: SessionsIndex = serde_json::from_str(&content)
                .with_context(|| format!("Failed to parse {}", index_path.display()))?;

            for entry in index.entries {
                if entry.is_sidechain.unwrap_or(false) {
                    continue;
                }

                let Some(path) = resolve_claude_session_path(&project_dir, &entry) else {
                    continue;
                };

                if let Some(session) = claude_code_session_info_from_index(&path, &entry) {
                    seen_session_ids.insert(session.session_id.clone());
                    all_sessions.push(session);
                }
            }
        }

        for path in collect_recent_files_recursive(&project_dir, "jsonl", scan_limit) {
            let Some(session_id) = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .map(|stem| stem.to_string())
            else {
                continue;
            };
            if seen_session_ids.contains(&session_id) {
                continue;
            }

            let modified = path
                .metadata()
                .and_then(|meta| meta.modified())
                .ok()
                .map(DateTime::<Utc>::from);
            let project_path = project_dir
                .file_name()
                .and_then(|name| name.to_str())
                .map(|name| name.replace('-', "/"));
            let label = format!(
                "Claude Code session {}",
                &session_id[..session_id.len().min(8)]
            );
            all_sessions.push(ClaudeCodeSessionInfo {
                session_id: session_id.clone(),
                first_prompt: label.clone(),
                summary: Some(label),
                message_count: 0,
                created: modified,
                modified,
                project_path,
                full_path: path.to_string_lossy().to_string(),
            });
            seen_session_ids.insert(session_id);
        }
    }

    all_sessions.sort_by(|a, b| {
        let a_date = a.modified.or(a.created);
        let b_date = b.modified.or(b.created);
        b_date.cmp(&a_date)
    });
    all_sessions.truncate(scan_limit);
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
        ClaudeCodeContent::Blocks(blocks) => blocks
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
                        thought_signature: None,
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
            .collect(),
    }
}

/// Import a Claude Code session by ID
pub fn import_session(session_id: &str) -> Result<Session> {
    let session_file = find_session_file(session_id)?;
    import_session_from_file(&session_file, session_id)
}

pub fn imported_session_id_for_target(
    target: &jcode_session_types::ResumeTarget,
) -> Option<String> {
    match target {
        jcode_session_types::ResumeTarget::JcodeSession { session_id } => Some(session_id.clone()),
        jcode_session_types::ResumeTarget::ClaudeCodeSession { session_id, .. } => {
            Some(imported_claude_code_session_id(session_id))
        }
        jcode_session_types::ResumeTarget::CodexSession { session_id, .. } => {
            Some(imported_codex_session_id(session_id))
        }
        jcode_session_types::ResumeTarget::PiSession { session_path } => {
            Some(imported_pi_session_id(session_path))
        }
        jcode_session_types::ResumeTarget::OpenCodeSession { session_id, .. } => {
            Some(imported_opencode_session_id(session_id))
        }
    }
}

pub fn resolve_resume_target_to_jcode(
    target: &jcode_session_types::ResumeTarget,
) -> Result<jcode_session_types::ResumeTarget> {
    use jcode_session_types::ResumeTarget;

    let session_id = match target {
        ResumeTarget::JcodeSession { session_id } => {
            return Ok(ResumeTarget::JcodeSession {
                session_id: session_id.clone(),
            });
        }
        ResumeTarget::ClaudeCodeSession {
            session_id,
            session_path,
        } => {
            import_session_from_file(Path::new(session_path), session_id)?;
            imported_claude_code_session_id(session_id)
        }
        ResumeTarget::CodexSession {
            session_id,
            session_path,
        } => {
            import_codex_session_from_path(Path::new(session_path), Some(session_id))?;
            imported_codex_session_id(session_id)
        }
        ResumeTarget::PiSession { session_path } => {
            import_pi_session(session_path)?;
            imported_pi_session_id(session_path)
        }
        ResumeTarget::OpenCodeSession {
            session_id,
            session_path,
        } => {
            import_opencode_session_from_path(Path::new(session_path), Some(session_id))?;
            imported_opencode_session_id(session_id)
        }
    };

    Ok(ResumeTarget::JcodeSession { session_id })
}

pub fn import_external_resume_id(resume_id: &str) -> Result<Option<String>> {
    if let Ok(path) = find_codex_session_file(resume_id) {
        let session = import_codex_session_from_path(&path, Some(resume_id))?;
        return Ok(Some(session.id));
    }

    if let Ok(path) = find_session_file(resume_id) {
        let session = import_session_from_file(&path, resume_id)?;
        return Ok(Some(session.id));
    }

    if let Ok(path) = find_opencode_session_file(resume_id) {
        let session = import_opencode_session_from_path(&path, Some(resume_id))?;
        return Ok(Some(session.id));
    }

    let pi_path = Path::new(resume_id);
    if pi_path.exists() {
        let session = import_pi_session(resume_id)?;
        return Ok(Some(session.id));
    }

    Ok(None)
}

/// Import a Claude Code session from a file path
pub fn import_session_from_file(path: &Path, session_id: &str) -> Result<Session> {
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

    let ordered_entries = ordered_claude_code_message_entries(&entries);

    // Extract metadata from entries
    let first_entry = ordered_entries.first().copied();
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
            list_claude_code_sessions()
                .ok()?
                .into_iter()
                .find(|s| s.session_id == session_id)
                .and_then(|s| s.summary.or(Some(s.first_prompt)))
        });

    // Create jcode session
    let jcode_session_id = imported_claude_code_session_id(session_id);
    let mut session = Session::create_with_id(jcode_session_id, None, title);
    session.provider_session_id = Some(session_id.to_string());
    session.provider_key = Some("claude-code".to_string());
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
            let msg_id = entry
                .uuid
                .clone()
                .unwrap_or_else(|| crate::id::new_id("msg"));

            session.append_stored_message(StoredMessage {
                id: msg_id,
                role,
                content: content_blocks,
                display_role: None,
                timestamp: None,
                tool_duration_ms: None,
                token_usage: None,
            });
        }
    }

    // Save the session
    session.save()?;

    Ok(session)
}

fn append_text_message(
    session: &mut Session,
    role: Role,
    text: String,
    timestamp: Option<DateTime<Utc>>,
) {
    let text = text.trim();
    if text.is_empty() {
        return;
    }
    session.append_stored_message(StoredMessage {
        id: crate::id::new_id("msg"),
        role,
        content: vec![ContentBlock::Text {
            text: text.to_string(),
            cache_control: None,
        }],
        display_role: None,
        timestamp,
        tool_duration_ms: None,
        token_usage: None,
    });
}

fn finalize_imported_session(
    mut session: Session,
    created_at: DateTime<Utc>,
    updated_at: Option<DateTime<Utc>>,
) -> Result<Session> {
    session.created_at = created_at;
    session.updated_at = updated_at.unwrap_or(created_at);
    session.last_active_at = updated_at.or(Some(created_at));
    session.status = SessionStatus::Closed;
    session.save()?;
    Ok(session)
}

fn find_codex_session_file(session_id: &str) -> Result<PathBuf> {
    let root = crate::storage::user_home_path(".codex/sessions")?;
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
            return Ok(path);
        }
    }
    anyhow::bail!("Codex session {} not found", session_id)
}

pub fn import_codex_session(session_id: &str) -> Result<Session> {
    let path = find_codex_session_file(session_id)?;
    import_codex_session_from_path(&path, Some(session_id))
}

pub fn import_codex_session_from_path(
    path: &Path,
    session_id_hint: Option<&str>,
) -> Result<Session> {
    let file = File::open(path)?;
    let reader = BufReader::new(file);
    let mut lines = reader.lines();
    let Some(first_line) = lines.next() else {
        anyhow::bail!("Codex session file is empty: {}", path.display())
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
        .filter(|id| !id.is_empty())
        .or(session_id_hint)
        .ok_or_else(|| anyhow::anyhow!("Codex session id missing in {}", path.display()))?;

    let created_at = parse_rfc3339_json(meta.get("timestamp"))
        .or_else(|| parse_rfc3339_json(header.get("timestamp")))
        .unwrap_or_else(Utc::now);
    let mut updated_at = Some(created_at);
    let mut title: Option<String> = None;
    let mut working_dir = meta
        .get("cwd")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let mut model: Option<String> = None;
    let mut session = Session::create_with_id(imported_codex_session_id(session_id), None, None);
    session.provider_session_id = Some(session_id.to_string());
    session.provider_key = Some("openai-codex".to_string());

    for line in lines {
        let line = line?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) else {
            continue;
        };
        let line_type = value
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        let (role, content_value, timestamp_value, model_value) = if line_type == "message" {
            let Some(role) = value.get("role").and_then(|v| v.as_str()) else {
                continue;
            };
            (
                role,
                value.get("content").unwrap_or(&serde_json::Value::Null),
                value.get("timestamp"),
                value.get("model"),
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
                value.get("timestamp").or_else(|| payload.get("timestamp")),
                payload.get("model"),
            )
        } else {
            continue;
        };

        let role = match role {
            "user" => Role::User,
            "assistant" => Role::Assistant,
            _ => continue,
        };
        let text = extract_text_from_json_value(content_value);
        if title.is_none() && role == Role::User {
            title = codex_title_candidate(&text);
        }
        if working_dir.is_none() {
            let cwd_text = extract_text_from_json_value(content_value);
            if let Some(cwd_line) = cwd_text.lines().find(|line| line.contains("<cwd>")) {
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
        if model.is_none() {
            model = model_value.and_then(|v| v.as_str()).map(|s| s.to_string());
        }
        let timestamp = parse_rfc3339_json(timestamp_value);
        if timestamp.is_some() {
            updated_at = timestamp;
        }
        append_text_message(&mut session, role, text, timestamp);
    }

    session.title = title.or_else(|| Some(format!("Codex session {}", session_id)));
    session.working_dir = working_dir;
    session.model = model;
    finalize_imported_session(session, created_at, updated_at)
}

pub fn import_pi_session(session_path: &str) -> Result<Session> {
    let path = PathBuf::from(session_path);
    let file = File::open(&path)?;
    let reader = BufReader::new(file);
    let mut lines = reader.lines();
    let Some(first_line) = lines.next() else {
        anyhow::bail!("Pi session file is empty: {}", path.display())
    };
    let header: serde_json::Value = serde_json::from_str(&first_line?)?;
    if header.get("type").and_then(|v| v.as_str()) != Some("session") {
        anyhow::bail!("Invalid Pi session header in {}", path.display())
    }

    let provider_session_id = header
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let created_at = parse_rfc3339_json(header.get("timestamp")).unwrap_or_else(Utc::now);
    let mut updated_at = Some(created_at);
    let mut title: Option<String> = None;
    let mut model: Option<String> = None;
    let mut provider_key: Option<String> = Some("pi".to_string());
    let mut session = Session::create_with_id(imported_pi_session_id(session_path), None, None);
    session.provider_session_id = if provider_session_id.is_empty() {
        None
    } else {
        Some(provider_session_id)
    };
    session.working_dir = header
        .get("cwd")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    for line in lines {
        let line = line?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) else {
            continue;
        };
        let timestamp = parse_rfc3339_json(value.get("timestamp"));
        if timestamp.is_some() {
            updated_at = timestamp;
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
                let role = match message.get("role").and_then(|v| v.as_str()) {
                    Some("user") => Role::User,
                    Some("assistant") => Role::Assistant,
                    _ => continue,
                };
                let text = extract_text_from_json_value(
                    message.get("content").unwrap_or(&serde_json::Value::Null),
                );
                if title.is_none() && role == Role::User && !text.trim().is_empty() {
                    title = Some(truncate_title(&text));
                }
                if model.is_none() {
                    model = message
                        .get("model")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                }
                append_text_message(&mut session, role, text, timestamp);
            }
            _ => {}
        }
    }

    session.title = title.or_else(|| {
        path.file_stem()
            .and_then(|s| s.to_str())
            .map(|stem| format!("Pi session {}", stem))
    });
    session.provider_key = provider_key;
    session.model = model;
    finalize_imported_session(session, created_at, updated_at)
}

fn find_opencode_session_file(session_id: &str) -> Result<PathBuf> {
    let root = crate::storage::user_home_path(".local/share/opencode/storage/session")?;
    for path in collect_files_recursive(&root, "json") {
        let Ok(value) = serde_json::from_reader::<_, serde_json::Value>(File::open(&path)?) else {
            continue;
        };
        if value.get("id").and_then(|v| v.as_str()) == Some(session_id) {
            return Ok(path);
        }
    }
    anyhow::bail!("OpenCode session {} not found", session_id)
}

pub fn import_opencode_session(session_id: &str) -> Result<Session> {
    let session_path = find_opencode_session_file(session_id)?;
    import_opencode_session_from_path(&session_path, Some(session_id))
}

pub fn import_opencode_session_from_path(
    session_path: &Path,
    session_id_hint: Option<&str>,
) -> Result<Session> {
    let value: serde_json::Value = serde_json::from_reader(File::open(session_path)?)?;
    let session_id = value
        .get("id")
        .and_then(|v| v.as_str())
        .filter(|id| !id.is_empty())
        .or(session_id_hint)
        .ok_or_else(|| {
            anyhow::anyhow!("OpenCode session id missing in {}", session_path.display())
        })?;
    let created_at = value
        .get("time")
        .and_then(|time| time.get("created"))
        .and_then(|v| v.as_i64())
        .and_then(DateTime::<Utc>::from_timestamp_millis)
        .unwrap_or_else(Utc::now);
    let mut updated_at = value
        .get("time")
        .and_then(|time| time.get("updated"))
        .and_then(|v| v.as_i64())
        .and_then(DateTime::<Utc>::from_timestamp_millis)
        .or(Some(created_at));
    let mut session = Session::create_with_id(imported_opencode_session_id(session_id), None, None);
    session.provider_session_id = Some(session_id.to_string());
    session.provider_key = Some("opencode".to_string());
    session.working_dir = value
        .get("directory")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    session.title = value
        .get("title")
        .and_then(|v| v.as_str())
        .map(truncate_title);

    let messages_root = crate::storage::user_home_path(format!(
        ".local/share/opencode/storage/message/{}",
        session_id
    ))?;
    let mut messages: Vec<(Option<DateTime<Utc>>, Role, String)> = Vec::new();
    let mut model: Option<String> = None;
    let mut provider_key = session.provider_key.clone();

    if messages_root.exists() {
        for msg_path in collect_files_recursive(&messages_root, "json") {
            let Ok(msg_value) =
                serde_json::from_reader::<_, serde_json::Value>(File::open(&msg_path)?)
            else {
                continue;
            };
            let role = match msg_value.get("role").and_then(|v| v.as_str()) {
                Some("user") => Role::User,
                Some("assistant") => Role::Assistant,
                _ => continue,
            };
            let text = msg_value
                .get("content")
                .map(extract_text_from_json_value)
                .filter(|text| !text.trim().is_empty())
                .or_else(|| msg_value.get("summary").map(extract_text_from_json_value))
                .unwrap_or_default();
            if model.is_none() {
                model = msg_value
                    .get("modelID")
                    .or_else(|| msg_value.get("model").and_then(|m| m.get("modelID")))
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
            }
            if provider_key.as_deref() == Some("opencode") {
                provider_key = msg_value
                    .get("providerID")
                    .or_else(|| msg_value.get("model").and_then(|m| m.get("providerID")))
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .or(provider_key);
            }
            let timestamp = msg_value
                .get("time")
                .and_then(|time| time.get("created"))
                .and_then(|v| v.as_i64())
                .and_then(DateTime::<Utc>::from_timestamp_millis);
            if timestamp.is_some() {
                updated_at = timestamp;
            }
            messages.push((timestamp, role, text));
        }
    }

    messages.sort_by_key(|(timestamp, _, _)| *timestamp);
    for (timestamp, role, text) in messages {
        append_text_message(&mut session, role, text, timestamp);
    }

    if session.title.is_none() {
        session.title = Some(format!("OpenCode session {}", session_id));
    }
    session.provider_key = provider_key;
    session.model = model;
    finalize_imported_session(session, created_at, updated_at)
}

#[cfg(test)]
#[path = "import_tests.rs"]
mod tests;
