use super::*;

pub(super) fn append_system_notice(sess: &mut session::Session, text: &str) {
    append_arrival(sess, &[], "", text);
}

/// Tool calls in the final assistant message that have no result yet. When
/// the agent itself runs `/cloud` mid-turn, its own bash call is one of these.
pub(super) fn dangling_tool_use_ids(sess: &session::Session) -> Vec<String> {
    use crate::message::{ContentBlock, Role};
    let Some(last_assistant) = sess
        .messages
        .iter()
        .rposition(|m| m.role == Role::Assistant)
    else {
        return Vec::new();
    };
    let answered: std::collections::HashSet<&str> = sess.messages[last_assistant + 1..]
        .iter()
        .flat_map(|m| m.content.iter())
        .filter_map(|b| match b {
            ContentBlock::ToolResult { tool_use_id, .. } => Some(tool_use_id.as_str()),
            _ => None,
        })
        .collect();
    sess.messages[last_assistant]
        .content
        .iter()
        .filter_map(|b| match b {
            ContentBlock::ToolUse { id, .. } if !answered.contains(id.as_str()) => Some(id.clone()),
            _ => None,
        })
        .collect()
}

pub(super) fn append_arrival(
    sess: &mut session::Session,
    dangling: &[String],
    host: &str,
    text: &str,
) {
    let mut blocks: Vec<crate::message::ContentBlock> = dangling
        .iter()
        .map(|id| crate::message::ContentBlock::ToolResult {
            tool_use_id: id.clone(),
            content: format!(
                "jcode cloud move completed. This session now runs on cloud host `{host}`. This tool call was interrupted by the move; rerun it here if its result still matters."
            ),
            is_error: None,
        })
        .collect();
    blocks.push(crate::message::ContentBlock::Text {
        text: format!("<system-reminder>\n{text}\n</system-reminder>"),
        cache_control: None,
    });
    // Tool results must stay provider-visible and paired with their calls.
    let role = dangling
        .is_empty()
        .then_some(session::StoredDisplayRole::System);
    sess.add_message_with_display_role(crate::message::Role::User, blocks, role);
}

pub(super) fn arrival_context(manifest: &MoveManifest) -> Vec<String> {
    let mut lines = vec![format!(
        "This session was moved from `{}` to cloud host `{}` at {} (migration epoch {}). You are now running on the cloud host. The conversation, todos and project memory came along.",
        manifest.source_host, manifest.target_host, manifest.created_at, manifest.epoch
    )];
    if let Some(repo) = &manifest.repo {
        lines.push(format!(
            "The repository is restored at the same path `{}` on branch `{}` at {}{}. Paths in earlier messages are still valid.",
            repo.root,
            repo.branch.as_deref().unwrap_or("(detached)"),
            &repo.head[..repo.head.len().min(12)],
            if repo.dirty {
                ", with the same uncommitted and untracked changes"
            } else {
                ""
            }
        ));
        if !repo.ignored_files.is_empty() {
            lines.push(format!(
                "Ignored files copied: {}.",
                repo.ignored_files.join(", ")
            ));
        }
        lines.push("Build caches (target/, node_modules/, etc.) did not come along. The first build here will be cold. Install dependencies if a command fails for that reason.".to_string());
    }
    if !manifest.toolchain_hints.is_empty() {
        lines.push(format!(
            "Toolchain hints from the repo: {}. Check these tools exist on this host before relying on them.",
            manifest.toolchain_hints.join(", ")
        ));
    }
    if !manifest.not_transferred.is_empty() {
        lines.push(format!(
            "These were running locally and did NOT move: {}. Restart them here if you still need them.",
            manifest.not_transferred.join("; ")
        ));
    }
    if !manifest.mcp_servers.is_empty() {
        lines.push(format!(
            "Local MCP servers ({}) are not connected here unless this host configures them too.",
            manifest.mcp_servers.join(", ")
        ));
    }
    lines
}

pub(super) fn arrival_notice(manifest: &MoveManifest) -> String {
    let mut lines = arrival_context(manifest);
    lines.push("There is no local display, desktop, browser or ssh-agent on this host. Keep working on the task exactly where you left off. The user can bring the session back with /local (`jcode cloud return`). Git will merge your work with any local edits.".to_string());
    lines.push(crate::cloud_environment::migration_guidance(
        &manifest.session_id,
    ));
    lines.join("\n")
}

pub(super) fn describe_local_only_state(session_id: &str) -> Vec<String> {
    let mut items = Vec::new();
    let dir = std::env::temp_dir().join("jcode-bg-tasks");
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_none_or(|e| e != "json") {
                continue;
            }
            let Ok(raw) = std::fs::read(&path) else {
                continue;
            };
            let Ok(value) = serde_json::from_slice::<serde_json::Value>(&raw) else {
                continue;
            };
            if value.get("session_id").and_then(|v| v.as_str()) != Some(session_id) {
                continue;
            }
            if value.get("status").and_then(|v| v.as_str()) != Some("running") {
                continue;
            }
            let name = value
                .get("display_name")
                .and_then(|v| v.as_str())
                .or_else(|| value.get("tool_name").and_then(|v| v.as_str()))
                .unwrap_or("task");
            let id = value.get("task_id").and_then(|v| v.as_str()).unwrap_or("?");
            items.push(format!("background task {id} ({name})"));
        }
    }
    items
}
