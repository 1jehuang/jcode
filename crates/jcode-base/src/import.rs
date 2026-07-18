//! Import Claude Code sessions into jcode
//!
//! This module handles discovering, parsing, and converting Claude Code sessions
//! so they can be resumed within jcode.

use crate::message::{ContentBlock, Role};
use crate::session::{Session, SessionStatus, StoredMessage};
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
pub use jcode_import_core::repo_ranking;
use jcode_import_core::{
    ClaudeCodeContent, ClaudeCodeContentBlock, ClaudeCodeEntry, ClaudeCodeSessionInfo,
    SessionIndexEntry, SessionsIndex, claude_code_session_info_from_index,
    claude_text_from_content, claude_title_candidate, clean_optional_text, codex_title_candidate,
    collect_files_recursive, collect_recent_files_recursive, extract_external_text_from_json,
    extract_opencode_part_text, extract_text_from_json_value, ordered_claude_code_message_entries,
    parse_rfc3339_json, parse_rfc3339_string, resolve_claude_session_path, truncate_title,
    truncate_title_text,
};
pub use jcode_import_core::{
    cursor_cwd_from_transcript_path, cursor_session_id_from_path,
    extract_external_text_from_json as extract_external_text_from_json_value,
    imported_claude_code_session_id, imported_codex_session_id, imported_cursor_session_id,
    imported_opencode_session_id, imported_pi_session_id, is_cursor_subagent_transcript,
};
use std::collections::HashSet;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::path::PathBuf;

// Keep enough recent context for a useful continuation while preventing an
// imported multi-hundred-message transcript from dominating first paint and
// every subsequent TUI frame. Roughly 256 KiB is also a comfortable provider
// context tail once the system prompt and tool schemas are added.
const IMPORT_HISTORY_MAX_MESSAGES: usize = 160;
const IMPORT_HISTORY_MAX_TEXT_BYTES: usize = 256 * 1024;
const IMPORT_BLOCK_MAX_TEXT_BYTES: usize = 32 * 1024;

fn json_line_has_message_role(line: &str) -> bool {
    fn has_value(bytes: &[u8], value: &[u8]) -> bool {
        let needle = b"\"role\"";
        let mut offset = 0usize;
        while let Some(relative) = bytes[offset..]
            .windows(needle.len())
            .position(|window| window == needle)
        {
            let mut index = offset + relative + needle.len();
            while bytes.get(index).is_some_and(u8::is_ascii_whitespace) {
                index += 1;
            }
            if bytes.get(index) != Some(&b':') {
                offset = index;
                continue;
            }
            index += 1;
            while bytes.get(index).is_some_and(u8::is_ascii_whitespace) {
                index += 1;
            }
            if bytes.get(index) == Some(&b'\"')
                && bytes.get(index + 1..index + 1 + value.len()) == Some(value)
                && bytes.get(index + 1 + value.len()) == Some(&b'\"')
            {
                return true;
            }
            offset = index;
        }
        false
    }

    let bytes = line.as_bytes();
    has_value(bytes, b"user") || has_value(bytes, b"assistant")
}

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
    let file = File::open(path)
        .with_context(|| format!("Failed to read session file: {}", path.display()))?;
    let mut entries = Vec::new();
    for line in BufReader::new(file).lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        // Claude transcripts contain progress, attachment, queue, and other
        // records that can dwarf the actual conversation. Avoid fully parsing
        // those records when all we need here are user/assistant messages.
        if !json_line_has_message_role(&line) {
            continue;
        }
        match serde_json::from_str::<ClaudeCodeEntry>(&line) {
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
                    .and_then(|text| claude_title_candidate(&text))
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
                jcode_core::util::truncate_str(&session_id, 8)
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

fn truncate_import_text(text: String, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text;
    }
    let suffix = format!(
        "\n[... {} bytes omitted from imported history]",
        text.len() - max_bytes
    );
    let prefix_budget = max_bytes.saturating_sub(suffix.len());
    format!(
        "{}{}",
        jcode_core::util::truncate_str(&text, prefix_budget),
        suffix
    )
}

fn imported_message_text(blocks: &[ContentBlock], truncate_blocks: bool) -> (String, bool) {
    let mut parts = Vec::new();
    let mut changed = blocks.len() != 1;

    for block in blocks {
        let text = match block {
            ContentBlock::Text {
                text,
                cache_control,
            } => {
                changed |= cache_control.is_some();
                text.clone()
            }
            // Provider-native reasoning cannot safely be replayed under a
            // different provider and is not required to continue the visible
            // conversation. Dropping it also avoids importing large hidden
            // traces that would slow initial rendering and request building.
            ContentBlock::Reasoning { .. }
            | ContentBlock::ReasoningTrace { .. }
            | ContentBlock::AnthropicThinking { .. }
            | ContentBlock::OpenAIReasoning { .. }
            | ContentBlock::OpenAICompaction { .. } => {
                changed = true;
                continue;
            }
            ContentBlock::ToolUse { name, input, .. } => {
                changed = true;
                let arguments = serde_json::to_string(input).unwrap_or_else(|_| "{}".to_string());
                format!("[Imported tool call: {name}]\n{arguments}")
            }
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => {
                changed = true;
                let outcome = if *is_error == Some(true) {
                    "error"
                } else {
                    "result"
                };
                format!("[Imported tool {outcome}: {tool_use_id}]\n{content}")
            }
            ContentBlock::Image { media_type, .. } => {
                changed = true;
                format!("[Imported image: {media_type}]")
            }
        };

        let text = if truncate_blocks {
            let original_len = text.len();
            let truncated = truncate_import_text(text, IMPORT_BLOCK_MAX_TEXT_BYTES);
            changed |= truncated.len() < original_len;
            truncated
        } else {
            text
        };
        if !text.trim().is_empty() {
            parts.push(text);
        }
    }

    let combined = parts.join("\n\n");
    if truncate_blocks {
        let original_len = combined.len();
        let combined = truncate_import_text(combined, IMPORT_BLOCK_MAX_TEXT_BYTES);
        changed |= combined.len() < original_len;
        (combined, changed)
    } else {
        (combined, changed)
    }
}

fn normalize_imported_history(session: &mut Session, apply_limits: bool) -> bool {
    let original_count = session.messages.len();
    let mut changed = false;
    let mut normalized = Vec::with_capacity(original_count);

    for mut message in std::mem::take(&mut session.messages) {
        let (text, message_changed) = imported_message_text(&message.content, apply_limits);
        changed |= message_changed;
        if text.trim().is_empty() {
            changed = true;
            continue;
        }
        message.content = vec![ContentBlock::Text {
            text,
            cache_control: None,
        }];
        normalized.push(message);
    }

    if apply_limits {
        let normalized_count = normalized.len();
        let mut kept_reversed = Vec::new();
        let mut kept_bytes = 0usize;
        for message in normalized.into_iter().rev() {
            let message_bytes = message
                .content
                .iter()
                .map(|block| match block {
                    ContentBlock::Text { text, .. } => text.len(),
                    _ => 0,
                })
                .sum::<usize>();
            if kept_reversed.len() >= IMPORT_HISTORY_MAX_MESSAGES
                || (!kept_reversed.is_empty()
                    && kept_bytes.saturating_add(message_bytes) > IMPORT_HISTORY_MAX_TEXT_BYTES)
            {
                break;
            }
            kept_bytes = kept_bytes.saturating_add(message_bytes);
            kept_reversed.push(message);
        }
        kept_reversed.reverse();
        let omitted = normalized_count.saturating_sub(kept_reversed.len());
        if omitted > 0 {
            changed = true;
            kept_reversed.insert(
                0,
                StoredMessage {
                    id: crate::id::new_id("message"),
                    role: Role::User,
                    content: vec![ContentBlock::Text {
                        text: format!(
                            "[Imported session: {omitted} older messages were omitted for a fast, provider-safe resume. Use session_search to inspect earlier external history.]"
                        ),
                        cache_control: None,
                    }],
                    display_role: None,
                    timestamp: None,
                    tool_duration_ms: None,
                    token_usage: None,
                },
            );
        }
        session.replace_messages(kept_reversed);
    } else {
        session.replace_messages(normalized);
    }

    changed || session.messages.len() != original_count
}

fn reuse_existing_imported_session(session_id: &str) -> bool {
    // An imported snapshot becomes a normal jcode continuation as soon as the
    // user resumes it. Never rewrite that snapshot merely because the external
    // transcript changed or because an older import contains structured blocks:
    // doing so can discard jcode-only turns and journal state. New imports are
    // normalized before their first save, while existing imports remain the
    // durable source of truth for subsequent resumes.
    Session::load(session_id).is_ok()
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
        jcode_session_types::ResumeTarget::CursorSession { session_id, .. } => {
            Some(imported_cursor_session_id(session_id))
        }
    }
}

pub fn resolve_resume_target_to_jcode(
    target: &jcode_session_types::ResumeTarget,
) -> Result<jcode_session_types::ResumeTarget> {
    use jcode_session_types::ResumeTarget;

    let prepare_start = std::time::Instant::now();
    let cache_hit;
    let source_label;
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
            source_label = "claude-code";
            let imported_id = imported_claude_code_session_id(session_id);
            cache_hit = reuse_existing_imported_session(&imported_id);
            if !cache_hit {
                import_session_from_file(Path::new(session_path), session_id)?;
            }
            imported_id
        }
        ResumeTarget::CodexSession {
            session_id,
            session_path,
        } => {
            source_label = "codex";
            let imported_id = imported_codex_session_id(session_id);
            cache_hit = reuse_existing_imported_session(&imported_id);
            if !cache_hit {
                import_codex_session_from_path(Path::new(session_path), Some(session_id))?;
            }
            imported_id
        }
        ResumeTarget::PiSession { session_path } => {
            source_label = "pi";
            let imported_id = imported_pi_session_id(session_path);
            cache_hit = reuse_existing_imported_session(&imported_id);
            if !cache_hit {
                import_pi_session(session_path)?;
            }
            imported_id
        }
        ResumeTarget::OpenCodeSession {
            session_id,
            session_path,
        } => {
            source_label = "opencode";
            let imported_id = imported_opencode_session_id(session_id);
            cache_hit = reuse_existing_imported_session(&imported_id);
            if !cache_hit {
                import_opencode_session_from_path(Path::new(session_path), Some(session_id))?;
            }
            imported_id
        }
        ResumeTarget::CursorSession {
            session_id,
            session_path,
        } => {
            source_label = "cursor";
            let imported_id = imported_cursor_session_id(session_id);
            cache_hit = reuse_existing_imported_session(&imported_id);
            if !cache_hit {
                import_cursor_session_from_path(Path::new(session_path), Some(session_id))?;
            }
            imported_id
        }
    };

    crate::logging::info(&format!(
        "[TIMING] external_resume_prepare: source={source_label} cache_hit={cache_hit} elapsed_ms={}",
        prepare_start.elapsed().as_millis()
    ));

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

    if let Ok(path) = find_cursor_session_file(resume_id) {
        let session = import_cursor_session_from_path(&path, Some(resume_id))?;
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
    import_session_from_file_with_target(
        path,
        session_id,
        imported_claude_code_session_id(session_id),
        false,
    )
}

fn import_session_from_file_with_target(
    path: &Path,
    session_id: &str,
    jcode_session_id: String,
    require_source_identity: bool,
) -> Result<Session> {
    let entries = load_claude_code_entries(path)?;
    if require_source_identity {
        let mut saw_expected_session = false;
        for entry in &entries {
            let Some(source_session_id) = entry.session_id.as_deref() else {
                continue;
            };
            if source_session_id != session_id {
                anyhow::bail!(
                    "Claude transcript {} belongs to session {}, not live session {}",
                    path.display(),
                    source_session_id,
                    session_id
                );
            }
            saw_expected_session = true;
        }
        if !saw_expected_session {
            anyhow::bail!(
                "Claude transcript {} does not identify live session {}",
                path.display(),
                session_id
            );
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

    // Get title from first real user message (skipping Claude Code's synthetic
    // slash-command / command-output / caveat wrapper messages) or the index.
    let title = ordered_entries
        .iter()
        .find_map(|entry| {
            (entry.entry_type == "user")
                .then_some(entry.message.as_ref())
                .flatten()
                .and_then(|message| claude_text_from_content(&message.content))
                .and_then(|text| claude_title_candidate(&text))
        })
        .or_else(|| {
            // Try to get from index
            list_claude_code_sessions()
                .ok()?
                .into_iter()
                .find(|s| s.session_id == session_id)
                .and_then(|s| s.summary.or(Some(s.first_prompt)))
        });

    // Convert messages from the external transcript.
    let mut imported_messages: Vec<StoredMessage> = Vec::new();
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

            imported_messages.push(StoredMessage {
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

    let mut session = Session::create_with_id(jcode_session_id, None, title);
    session.provider_session_id = Some(session_id.to_string());
    session.provider_key = Some("claude-code".to_string());
    session.working_dir = working_dir;
    session.model = model;
    session.created_at = created_at;

    for message in imported_messages {
        session.append_stored_message(message);
    }

    finalize_imported_session(session, created_at, None)
}

fn remove_prepared_takeover_session(session_id: &str) {
    let Ok(snapshot) = crate::session::session_path(session_id) else {
        return;
    };
    let journal = crate::session::session_journal_path_from_snapshot(&snapshot);
    let backup = snapshot.with_extension("bak");
    for path in [snapshot, journal, backup] {
        let _ = std::fs::remove_file(path);
    }
    crate::session_list_cache::invalidate();
}

/// Explicitly hand a currently-running Claude Code session over to Jcode.
///
/// This is deliberately separate from normal resume. It first imports the
/// current transcript into a fresh durable Jcode session, then gracefully stops
/// the exact PID guarded by Claude's process-start token. After Claude exits we
/// refresh the prepared snapshot once to capture any final transcript flush.
/// A stop failure rolls back the staged Jcode session and leaves ordinary
/// resume behavior unchanged.
pub fn take_over_live_claude_session(
    target: &jcode_session_types::ResumeTarget,
) -> Result<jcode_session_types::ResumeTarget> {
    take_over_live_claude_session_with_timeout(target, std::time::Duration::from_secs(10))
}

fn take_over_live_claude_session_with_timeout(
    target: &jcode_session_types::ResumeTarget,
    stop_timeout: std::time::Duration,
) -> Result<jcode_session_types::ResumeTarget> {
    use jcode_session_types::ResumeTarget;

    let ResumeTarget::ClaudeCodeSession {
        session_id,
        session_path,
    } = target
    else {
        anyhow::bail!("live takeover is only available for Claude Code sessions");
    };

    let live = crate::claude_live::find_live_claude_session(session_id)?
        .ok_or_else(|| anyhow::anyhow!("Claude Code session {session_id} is no longer live"))?;
    // Use a normal memorable Jcode session ID so the handed-off conversation
    // remains visible and resumable later. Imported-prefixed IDs are hidden from
    // the native session list because their external source row represents them.
    let takeover_id = Session::create(None, None).id;
    let path = Path::new(session_path);

    let prepared =
        import_session_from_file_with_target(path, session_id, takeover_id.clone(), true)
            .with_context(|| format!("failed to prepare Claude Code session {session_id}"))?;
    if prepared.visible_conversation_message_count() == 0 {
        remove_prepared_takeover_session(&takeover_id);
        anyhow::bail!(
            "Claude Code session {session_id} has no complete conversation messages to take over"
        );
    }

    let stop_outcome = match crate::claude_live::stop_live_claude_session(&live, stop_timeout) {
        Ok(outcome) => outcome,
        Err(err) => {
            remove_prepared_takeover_session(&takeover_id);
            return Err(err).with_context(|| {
                format!("prepared Claude Code session {session_id}, but did not take it over")
            });
        }
    };

    if stop_outcome == crate::claude_live::StopLiveClaudeOutcome::ExitUnconfirmed {
        crate::session_list_cache::invalidate();
        anyhow::bail!(
            "Claude Code process {} was asked to exit, but its exit was not confirmed; prepared Jcode session {} was preserved and can be resumed",
            live.pid,
            takeover_id
        );
    }

    // Claude may flush one last complete message while handling SIGTERM. The
    // staged snapshot remains durable if the final refresh repeatedly fails,
    // but do not report a fully successful handoff with an incomplete tail.
    let mut final_refresh_error = None;
    for attempt in 0..5 {
        match import_session_from_file_with_target(path, session_id, takeover_id.clone(), true) {
            Ok(_) => {
                final_refresh_error = None;
                break;
            }
            Err(err) => {
                final_refresh_error = Some(err);
                if attempt < 4 {
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
            }
        }
    }
    if let Some(err) = final_refresh_error {
        crate::session_list_cache::invalidate();
        return Err(err).with_context(|| {
            format!(
                "Claude Code exited, but its final transcript could not be refreshed; prepared Jcode session {takeover_id} was preserved"
            )
        });
    }

    crate::logging::info(&format!(
        "Claude takeover complete: source_session={session_id} source_pid={} jcode_session={takeover_id}",
        live.pid
    ));
    crate::session_list_cache::invalidate();
    Ok(ResumeTarget::JcodeSession {
        session_id: takeover_id,
    })
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
    // Never overwrite a jcode-side continuation with a shorter external
    // transcript. This protection applies to every importer, not only Claude.
    if crate::session::session_exists(&session.id)
        && let Ok(mut existing) = Session::load(&session.id)
        && existing.messages.len() > session.messages.len()
    {
        if normalize_imported_history(&mut existing, false) {
            existing.save()?;
        }
        return Ok(existing);
    }

    let original_messages = session.messages.len();
    session.created_at = created_at;
    session.updated_at = updated_at.unwrap_or(created_at);
    session.last_active_at = updated_at.or(Some(created_at));
    session.status = SessionStatus::Closed;
    normalize_imported_history(&mut session, true);
    session.save()?;
    crate::logging::info(&format!(
        "Imported session prepared: source_messages={original_messages} kept_messages={}",
        session.messages.len()
    ));
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
        // Codex rollouts are dominated by reasoning, world-state, tool-output,
        // and telemetry records. Only message records carry a user/assistant
        // role, so reject the rest before serde allocates their often-large JSON.
        if !json_line_has_message_role(trimmed) {
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
    let parts_base = crate::storage::user_home_path(".local/share/opencode/storage/part")?;
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
            // Modern OpenCode (Go storage) stores message body text in
            // storage/part/<messageID>/*.json; fall back to legacy inline
            // content/summary for older stores.
            let text = msg_value
                .get("id")
                .and_then(|v| v.as_str())
                .map(|id| extract_opencode_part_text(&parts_base, id, true))
                .filter(|text| !text.trim().is_empty())
                .or_else(|| {
                    msg_value
                        .get("content")
                        .map(extract_text_from_json_value)
                        .filter(|text| !text.trim().is_empty())
                })
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

/// Locate a Cursor agent transcript file for the given session id.
///
/// Cursor stores transcripts at
/// `~/.cursor/projects/<project>/agent-transcripts/<session-id>/<session-id>.jsonl`,
/// so the session id is the file stem. We scan the project tree for a matching
/// stem rather than guessing the project dir.
fn find_cursor_session_file(session_id: &str) -> Result<PathBuf> {
    let root = crate::storage::user_home_path(".cursor/projects")?;
    for path in collect_files_recursive(&root, "jsonl") {
        if cursor_session_id_from_path(&path) == session_id {
            return Ok(path);
        }
    }
    anyhow::bail!("Cursor session {} not found", session_id)
}

pub fn import_cursor_session(session_id: &str) -> Result<Session> {
    let path = find_cursor_session_file(session_id)?;
    import_cursor_session_from_path(&path, Some(session_id))
}

pub fn import_cursor_session_from_path(
    session_path: &Path,
    session_id_hint: Option<&str>,
) -> Result<Session> {
    let session_id = session_id_hint
        .map(|id| id.to_string())
        .unwrap_or_else(|| cursor_session_id_from_path(session_path));
    let created_at =
        jcode_import_core::file_modified_datetime(session_path).unwrap_or_else(Utc::now);

    let mut session = Session::create_with_id(imported_cursor_session_id(&session_id), None, None);
    session.provider_session_id = Some(session_id.clone());
    session.provider_key = Some("cursor".to_string());
    session.working_dir = cursor_cwd_from_transcript_path(session_path);

    let file = File::open(session_path)?;
    let reader = BufReader::new(file);
    let mut title: Option<String> = None;
    for line in reader.lines() {
        let line = line?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) else {
            continue;
        };
        let role = match value
            .get("role")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
        {
            "user" | "human" => Role::User,
            "assistant" | "model" => Role::Assistant,
            _ => continue,
        };
        let content = value
            .get("message")
            .and_then(|message| message.get("content"))
            .or_else(|| value.get("content"))
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        let text = extract_external_text_from_json(&content, true);
        if text.trim().is_empty() {
            continue;
        }
        if title.is_none() && role == Role::User {
            title = Some(truncate_title_text(&text, 72));
        }
        append_text_message(&mut session, role, text, None);
    }

    session.title = title.or_else(|| Some(format!("Cursor session {}", session_id)));
    finalize_imported_session(session, created_at, Some(created_at))
}

#[cfg(test)]
#[path = "import_tests.rs"]
mod tests;
