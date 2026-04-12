use crate::message::ContentBlock;
use crate::session::{Session, SessionStatus};
use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};

const CATCHUP_STATE_FILE: &str = "catchup_seen.json";

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct PersistedCatchupState {
    #[serde(default)]
    seen_at_ms_by_session: HashMap<String, i64>,
}

#[derive(Debug, Clone)]
pub struct CatchupBrief {
    pub reason: String,
    pub tags: Vec<String>,
    pub last_user_prompt: Option<String>,
    pub activity_steps: Vec<String>,
    pub files_touched: Vec<String>,
    pub tool_counts: Vec<(String, usize)>,
    pub validation_notes: Vec<String>,
    pub latest_agent_response: Option<String>,
    pub needs_from_user: String,
    pub updated_at: DateTime<Utc>,
}

pub fn needs_catchup(session_id: &str, updated_at: DateTime<Utc>, status: &SessionStatus) -> bool {
    if !is_attention_status(status) {
        return false;
    }
    let seen = load_seen_state()
        .seen_at_ms_by_session
        .get(session_id)
        .copied();
    needs_catchup_with_seen(updated_at.timestamp_millis(), seen, status)
}

pub(crate) fn needs_catchup_with_seen(
    updated_at_ms: i64,
    seen_at_ms: Option<i64>,
    status: &SessionStatus,
) -> bool {
    is_attention_status(status) && seen_at_ms.unwrap_or_default() < updated_at_ms
}

pub fn mark_seen(session_id: &str, updated_at: DateTime<Utc>) -> Result<()> {
    let mut state = load_seen_state();
    state
        .seen_at_ms_by_session
        .insert(session_id.to_string(), updated_at.timestamp_millis());
    save_seen_state(&state)
}

pub fn build_brief(session: &Session) -> CatchupBrief {
    let rendered = crate::session::render_messages(session);
    let last_user_prompt = rendered
        .iter()
        .rev()
        .find(|msg| msg.role == "user" && !msg.content.trim().is_empty())
        .map(|msg| msg.content.trim().to_string());
    let latest_agent_response = rendered
        .iter()
        .rev()
        .find(|msg| msg.role == "assistant" && !msg.content.trim().is_empty())
        .map(|msg| msg.content.trim().to_string());

    let files_touched = collect_touched_files(session);
    let tool_counts = collect_tool_counts(session);
    let validation_notes = collect_validation_notes(&rendered);
    let activity_steps = collect_activity_steps(session);
    let (reason, tags) = reason_and_tags(&session.status);
    let needs_from_user = infer_needs_from_user(&session.status, latest_agent_response.as_deref());

    CatchupBrief {
        reason,
        tags,
        last_user_prompt,
        activity_steps,
        files_touched,
        tool_counts,
        validation_notes,
        latest_agent_response,
        needs_from_user,
        updated_at: session.updated_at,
    }
}

pub fn render_markdown(
    session: &Session,
    source_session_id: Option<&str>,
    queue_position: Option<(usize, usize)>,
    brief: &CatchupBrief,
) -> String {
    let display_name = session.display_name().to_string();
    let icon = crate::id::session_icon(&display_name);
    let status_icon = status_icon(&session.status);
    let status_label = status_label(&session.status);
    let updated_ago = format_time_ago(brief.updated_at);
    let source_label = source_session_id
        .and_then(crate::id::extract_session_name)
        .unwrap_or("previous session");

    let mut out = String::new();
    out.push_str("# Catch Up\n\n");
    out.push_str(&format!(
        "**{} {}** · {} **{}** · updated {}\n\n",
        icon, display_name, status_icon, status_label, updated_ago
    ));

    if let Some((index, total)) = queue_position {
        out.push_str(&format!("- Queue: **{} of {}**\n", index, total));
    }
    if source_session_id.is_some() {
        out.push_str(&format!("- From: **{}**\n", source_label));
    }
    out.push_str(&format!("- Session: `{}`\n\n", session.id));

    if !brief.activity_steps.is_empty() {
        out.push_str("```mermaid\nflowchart TD\n");
        out.push_str(&format!(
            "    A[\"Why<br/>{}\"]:::status --> B[\"Your last prompt\"]:::user\n",
            mermaid_escape(&brief.reason)
        ));
        let mut prev = 'B';
        for (idx, step) in brief.activity_steps.iter().take(4).enumerate() {
            let node = ((b'C' + idx as u8) as char).to_string();
            out.push_str(&format!(
                "    {}[\"{}\"]:::step\n",
                node,
                mermaid_escape(step)
            ));
            out.push_str(&format!("    {} --> {}\n", prev, node));
            prev = node.chars().next().unwrap_or('B');
        }
        out.push_str(&format!(
            "    {} --> Z[\"Need from you<br/>{}\"]:::decision\n",
            prev,
            mermaid_escape(&brief.needs_from_user)
        ));
        out.push_str("    classDef status fill:#18331f,stroke:#4caf50,color:#d6ffd9;\n");
        out.push_str("    classDef user fill:#1f3659,stroke:#7fb3ff,color:#e8f1ff;\n");
        out.push_str("    classDef step fill:#2b2b33,stroke:#9090a0,color:#f0f0f5;\n");
        out.push_str("    classDef decision fill:#43284f,stroke:#d38cff,color:#fdefff;\n");
        out.push_str("```\n\n");
    }

    out.push_str("## Why this needs attention\n\n");
    out.push_str(&format!("> {}\n\n", brief.reason));
    if !brief.tags.is_empty() {
        out.push_str(&format!(
            "{}\n\n",
            brief
                .tags
                .iter()
                .map(|tag| format!("`{}`", tag))
                .collect::<Vec<_>>()
                .join(" ")
        ));
    }

    out.push_str("## Your last prompt\n\n");
    if let Some(prompt) = brief.last_user_prompt.as_deref() {
        out.push_str(&format!("> {}\n\n", markdown_quote(prompt)));
    } else {
        out.push_str("> No user prompt found in the restored transcript.\n\n");
    }

    out.push_str("## What happened\n\n");
    if brief.activity_steps.is_empty() {
        out.push_str("- No tool activity was reconstructed from the stored transcript.\n\n");
    } else {
        for step in &brief.activity_steps {
            out.push_str(&format!("- {}\n", step));
        }
        out.push('\n');
    }

    out.push_str("## What changed\n\n");
    if brief.files_touched.is_empty() {
        out.push_str("- Files: _no explicit file paths captured_\n");
    } else {
        out.push_str(&format!(
            "- Files: {}\n",
            brief
                .files_touched
                .iter()
                .take(5)
                .map(|path| format!("`{}`", path))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if brief.tool_counts.is_empty() {
        out.push_str("- Tools: _none captured_\n");
    } else {
        out.push_str(&format!(
            "- Tools: {}\n",
            brief
                .tool_counts
                .iter()
                .take(6)
                .map(|(name, count)| format!("`{}`×{}", name, count))
                .collect::<Vec<_>>()
                .join(" · ")
        ));
    }
    if brief.validation_notes.is_empty() {
        out.push_str("- Validation: _no test/build validation detected_\n\n");
    } else {
        out.push_str("- Validation:\n");
        for note in &brief.validation_notes {
            out.push_str(&format!("  - {}\n", note));
        }
        out.push('\n');
    }

    out.push_str("## Latest agent response\n\n");
    if let Some(response) = brief.latest_agent_response.as_deref() {
        out.push_str(&format!("> {}\n\n", markdown_quote(response)));
    } else {
        out.push_str("> No final assistant response was found.\n\n");
    }

    out.push_str("## Needs from you\n\n");
    out.push_str(&format!("> {}\n\n", brief.needs_from_user));

    out.push_str("## Actions\n\n");
    out.push_str("- **Enter** — continue in this session\n");
    out.push_str("- **/back** — return to the previous session\n");
    out.push_str("- **/catchup next** — jump to the next unfinished handoff\n");
    out.push_str("- **/resume** — browse all sessions normally\n");

    out
}

fn state_path() -> Result<std::path::PathBuf> {
    Ok(crate::storage::jcode_dir()?.join(CATCHUP_STATE_FILE))
}

fn load_seen_state() -> PersistedCatchupState {
    let Ok(path) = state_path() else {
        return PersistedCatchupState::default();
    };
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn save_seen_state(state: &PersistedCatchupState) -> Result<()> {
    let path = state_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, serde_json::to_string_pretty(state)?)?;
    Ok(())
}

fn is_attention_status(status: &SessionStatus) -> bool {
    matches!(
        status,
        SessionStatus::Closed
            | SessionStatus::Reloaded
            | SessionStatus::Compacted
            | SessionStatus::RateLimited
            | SessionStatus::Crashed { .. }
            | SessionStatus::Error { .. }
    )
}

fn reason_and_tags(status: &SessionStatus) -> (String, Vec<String>) {
    match status {
        SessionStatus::Closed => (
            "Finished and is ready for your next instruction.".to_string(),
            vec!["completed".to_string(), "decision needed".to_string()],
        ),
        SessionStatus::Reloaded => (
            "Resumed after a reload and may need confirmation before continuing.".to_string(),
            vec!["reloaded".to_string(), "review".to_string()],
        ),
        SessionStatus::Compacted => (
            "Compacted older context; review the latest result before continuing.".to_string(),
            vec!["compacted".to_string(), "review".to_string()],
        ),
        SessionStatus::RateLimited => (
            "Paused by rate limiting; decide whether to retry here or move on.".to_string(),
            vec!["waiting".to_string(), "rate limited".to_string()],
        ),
        SessionStatus::Crashed { message } => (
            message
                .as_deref()
                .map(|msg| format!("Failed and needs attention: {}", msg.trim()))
                .unwrap_or_else(|| {
                    "Failed and may need intervention before continuing.".to_string()
                }),
            vec!["failed".to_string(), "intervention".to_string()],
        ),
        SessionStatus::Error { message } => (
            format!(
                "Stopped with an error and needs attention: {}",
                message.trim()
            ),
            vec!["failed".to_string(), "error".to_string()],
        ),
        SessionStatus::Active => ("Still active.".to_string(), vec!["active".to_string()]),
    }
}

fn infer_needs_from_user(status: &SessionStatus, latest_response: Option<&str>) -> String {
    if matches!(
        status,
        SessionStatus::Error { .. } | SessionStatus::Crashed { .. }
    ) {
        return "Inspect the failure, decide whether to retry, and redirect follow-up work if needed."
            .to_string();
    }

    let latest_lower = latest_response.unwrap_or_default().to_lowercase();
    if [
        "decide",
        "choose",
        "approve",
        "which",
        "option",
        "what do you want",
    ]
    .iter()
    .any(|needle| latest_lower.contains(needle))
    {
        return "Review the proposed options and decide the next step for this session."
            .to_string();
    }

    if matches!(status, SessionStatus::RateLimited) {
        return "Decide whether to retry this session now or move on to the next catch-up."
            .to_string();
    }

    "Continue here if you want to direct follow-up work, or jump to the next catch-up.".to_string()
}

fn collect_touched_files(session: &Session) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut files = Vec::new();
    for msg in &session.messages {
        for block in &msg.content {
            if let ContentBlock::ToolUse { input, .. } = block {
                for key in ["file_path", "path"] {
                    let Some(value) = input.get(key).and_then(|value| value.as_str()) else {
                        continue;
                    };
                    let trimmed = value.trim();
                    if trimmed.is_empty() || !seen.insert(trimmed.to_string()) {
                        continue;
                    }
                    files.push(trimmed.to_string());
                    if files.len() >= 12 {
                        return files;
                    }
                }
            }
        }
    }
    files
}

fn collect_tool_counts(session: &Session) -> Vec<(String, usize)> {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for msg in &session.messages {
        for block in &msg.content {
            if let ContentBlock::ToolUse { name, .. } = block {
                *counts.entry(name.clone()).or_default() += 1;
            }
        }
    }
    let mut counts: Vec<(String, usize)> = counts.into_iter().collect();
    counts.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    counts
}

fn collect_activity_steps(session: &Session) -> Vec<String> {
    let mut steps = Vec::new();
    let mut last = String::new();
    for msg in &session.messages {
        for block in &msg.content {
            let Some(step) = tool_use_step(block) else {
                continue;
            };
            if step == last {
                continue;
            }
            last = step.clone();
            steps.push(step);
            if steps.len() >= 6 {
                return steps;
            }
        }
    }
    steps
}

fn tool_use_step(block: &ContentBlock) -> Option<String> {
    let ContentBlock::ToolUse { name, input, .. } = block else {
        return None;
    };
    let obj = input.as_object();
    match name.as_str() {
        "agentgrep" | "grep" | "glob" | "ls" | "codesearch" | "session_search" => {
            Some("Searched code and session context".to_string())
        }
        "read" => Some(
            obj.and_then(|map| map.get("file_path").and_then(|v| v.as_str()))
                .map(|path| format!("Inspected `{}`", path.trim()))
                .unwrap_or_else(|| "Inspected files".to_string()),
        ),
        "edit" | "multiedit" | "write" | "patch" | "apply_patch" => Some(
            obj.and_then(|map| map.get("file_path").and_then(|v| v.as_str()))
                .map(|path| format!("Updated `{}`", path.trim()))
                .unwrap_or_else(|| "Edited files".to_string()),
        ),
        "bash" => {
            let command = obj
                .and_then(|map| map.get("command").and_then(|v| v.as_str()))
                .unwrap_or_default()
                .trim();
            let lower = command.to_lowercase();
            if lower.contains("cargo test")
                || lower.contains("pytest")
                || lower.contains("npm test")
                || lower.contains("pnpm test")
                || lower.contains("go test")
            {
                Some(format!("Ran tests{}", summarize_shell_suffix(command)))
            } else if lower.contains("cargo build")
                || lower.contains("npm run build")
                || lower.contains("pnpm build")
                || lower.contains("go build")
            {
                Some(format!(
                    "Built the project{}",
                    summarize_shell_suffix(command)
                ))
            } else {
                Some(format!(
                    "Ran shell command{}",
                    summarize_shell_suffix(command)
                ))
            }
        }
        "communicate" => Some("Coordinated with other agents".to_string()),
        "subagent" => Some("Spawned a subagent".to_string()),
        "memory" => Some("Queried memory context".to_string()),
        "side_panel" | "todo" | "todoread" | "todowrite" | "goal" => None,
        other => Some(format!("Used `{}`", other)),
    }
}

fn summarize_shell_suffix(command: &str) -> String {
    let trimmed = command.trim();
    if trimmed.is_empty() {
        String::new()
    } else {
        format!(" · `{}`", truncate(trimmed, 56))
    }
}

fn collect_validation_notes(rendered: &[crate::session::RenderedMessage]) -> Vec<String> {
    let mut notes = Vec::new();
    for msg in rendered.iter().rev() {
        if msg.role != "tool" {
            continue;
        }
        let Some(tool) = msg.tool_data.as_ref() else {
            continue;
        };
        if tool.name != "bash" {
            continue;
        }
        let command = tool
            .input
            .get("command")
            .and_then(|value| value.as_str())
            .unwrap_or_default()
            .trim();
        if command.is_empty() {
            continue;
        }
        let lower = command.to_lowercase();
        let label = if lower.contains("test") {
            "tests"
        } else if lower.contains("build") {
            "build"
        } else {
            continue;
        };
        let ok = !looks_like_error(&msg.content);
        notes.push(format!(
            "{} {}",
            if ok { "✓" } else { "✗" },
            format!("{}: `{}`", label, truncate(command, 64))
        ));
        if notes.len() >= 3 {
            break;
        }
    }
    notes.reverse();
    notes
}

fn looks_like_error(text: &str) -> bool {
    let trimmed = text.trim_start().to_lowercase();
    trimmed.starts_with("error:") || trimmed.starts_with("failed:") || trimmed.contains("exit 1")
}

fn status_icon(status: &SessionStatus) -> &'static str {
    match status {
        SessionStatus::Closed => "🟢",
        SessionStatus::Reloaded => "🔄",
        SessionStatus::Compacted => "🟠",
        SessionStatus::RateLimited => "⏳",
        SessionStatus::Crashed { .. } | SessionStatus::Error { .. } => "🔴",
        SessionStatus::Active => "▶",
    }
}

fn status_label(status: &SessionStatus) -> &'static str {
    match status {
        SessionStatus::Closed => "completed",
        SessionStatus::Reloaded => "reloaded",
        SessionStatus::Compacted => "compacted",
        SessionStatus::RateLimited => "waiting",
        SessionStatus::Crashed { .. } | SessionStatus::Error { .. } => "failed",
        SessionStatus::Active => "active",
    }
}

fn format_time_ago(updated_at: DateTime<Utc>) -> String {
    let delta = Utc::now().signed_duration_since(updated_at);
    if delta.num_seconds() < 60 {
        format!("{}s ago", delta.num_seconds().max(0))
    } else if delta.num_minutes() < 60 {
        format!("{}m ago", delta.num_minutes())
    } else if delta.num_hours() < 24 {
        format!("{}h ago", delta.num_hours())
    } else {
        format!("{}d ago", delta.num_days())
    }
}

fn truncate(text: &str, max_chars: usize) -> String {
    let trimmed = text.trim();
    if trimmed.chars().count() <= max_chars {
        return trimmed.to_string();
    }
    let mut out = trimmed
        .chars()
        .take(max_chars.saturating_sub(1))
        .collect::<String>();
    out.push('…');
    out
}

fn markdown_quote(text: &str) -> String {
    truncate(text.replace('\n', " ").trim(), 600)
}

fn mermaid_escape(text: &str) -> String {
    text.replace('"', "'")
        .replace('\n', "<br/>")
        .replace(':', " -")
}

#[cfg(test)]
mod tests {
    use super::{build_brief, needs_catchup_with_seen, render_markdown};
    use crate::message::{ContentBlock, Role};
    use crate::session::{Session, SessionStatus};

    #[test]
    fn needs_catchup_requires_attention_status_and_newer_than_seen() {
        assert!(needs_catchup_with_seen(10, Some(9), &SessionStatus::Closed));
        assert!(!needs_catchup_with_seen(
            10,
            Some(10),
            &SessionStatus::Closed
        ));
        assert!(!needs_catchup_with_seen(10, None, &SessionStatus::Active));
    }

    #[test]
    fn render_markdown_includes_key_sections() {
        let mut session = Session::create(None, Some("catchup".to_string()));
        session.short_name = Some("fox".to_string());
        session.status = SessionStatus::Closed;
        session.add_message(
            Role::User,
            vec![ContentBlock::Text {
                text: "Implement the catch up side panel".to_string(),
                cache_control: None,
            }],
        );
        session.add_message(
            Role::Assistant,
            vec![ContentBlock::Text {
                text: "Searched the relevant session picker code.".to_string(),
                cache_control: None,
            }],
        );
        session.add_message(
            Role::Assistant,
            vec![ContentBlock::ToolUse {
                id: "tool_1".to_string(),
                name: "read".to_string(),
                input: serde_json::json!({"file_path": "src/tui/session_picker.rs"}),
            }],
        );
        session.add_message(
            Role::Assistant,
            vec![ContentBlock::Text {
                text: "Implemented the first pass and left a few follow-up notes.".to_string(),
                cache_control: None,
            }],
        );
        let brief = build_brief(&session);
        let markdown = render_markdown(&session, Some("session_otter"), Some((1, 3)), &brief);
        assert!(markdown.contains("# Catch Up"));
        assert!(markdown.contains("## Your last prompt"));
        assert!(markdown.contains("## What happened"));
        assert!(markdown.contains("## Latest agent response"));
        assert!(markdown.contains("## Needs from you"));
        assert!(markdown.contains("```mermaid"));
    }
}
