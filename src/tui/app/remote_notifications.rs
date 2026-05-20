use crate::protocol::NotificationType;
use crate::tui::ui::capitalize;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SwarmNotificationPresentation {
    pub title: String,
    pub message: String,
    pub status_notice: String,
}

fn compact_swarm_session_label(session: &str) -> String {
    crate::id::extract_session_name(session)
        .unwrap_or(session)
        .to_string()
}

fn compact_swarm_summary(summary: &str) -> String {
    summary.replace(", ", " · ")
}

fn strip_message_prefix<'a>(message: &'a str, prefix: &str) -> Option<&'a str> {
    message.strip_prefix(prefix).map(str::trim)
}

fn compact_direct_message_body(message: &str) -> String {
    if let Some((_, body)) = message
        .strip_prefix("DM from ")
        .and_then(|rest| rest.split_once(": "))
    {
        return body.trim().to_string();
    }
    message.to_string()
}

fn compact_channel_message_body(message: &str) -> String {
    if let Some((_, body)) = message
        .strip_prefix('#')
        .and_then(|rest| rest.split_once(": "))
    {
        return body.trim().to_string();
    }
    message.to_string()
}

fn compact_broadcast_message_body(message: &str) -> String {
    if let Some((_, body)) = message
        .strip_prefix("broadcast from ")
        .and_then(|rest| rest.split_once(": "))
    {
        return body.trim().to_string();
    }
    message.to_string()
}

fn compact_plan_message_body(message: &str) -> String {
    if message.starts_with("Plan updated by ")
        && message.ends_with(')')
        && let Some(summary) = message.rsplit_once(" (").map(|(_, summary)| summary)
    {
        return compact_swarm_summary(summary.trim_end_matches(')'));
    }

    if let Some(rest) = strip_message_prefix(message, "Plan updated: task '")
        && let Some((task_id, assignee)) = rest.split_once("' assigned to ")
    {
        return format!(
            "Assigned {} → {}",
            task_id.trim(),
            compact_swarm_session_label(assignee.trim_end_matches('.').trim())
        );
    }

    if let Some(rest) = strip_message_prefix(message, "Plan approved by coordinator: ")
        && let Some((count, proposer)) = rest.split_once(" items added from ")
    {
        return format!(
            "Approved {} items from {}",
            count.trim(),
            compact_swarm_session_label(proposer.trim_end_matches('.').trim())
        );
    }

    if let Some(summary) = message
        .strip_prefix("Plan attached to this session (")
        .and_then(|rest| rest.strip_suffix(")."))
    {
        return format!("Attached · {}", compact_swarm_summary(summary));
    }

    message.to_string()
}

fn compact_swarm_path(path: &str) -> String {
    let trimmed = path.trim();
    let parts: Vec<&str> = trimmed
        .split(['/', '\\'])
        .filter(|part| !part.is_empty())
        .collect();

    if parts.len() <= 4 {
        trimmed.to_string()
    } else {
        format!("…/{}", parts[parts.len() - 4..].join("/"))
    }
}

fn sanitize_code_fence_content(text: &str) -> String {
    text.replace("```", "``\u{200b}`")
}

fn file_activity_summary_line(operation: &str, summary: Option<&str>) -> String {
    summary
        .map(str::trim)
        .filter(|summary| !summary.is_empty())
        .map(capitalize)
        .unwrap_or_else(|| capitalize(operation))
}

fn format_file_activity_message(
    path: &str,
    operation: &str,
    intent: Option<&str>,
    summary: Option<&str>,
    detail: Option<&str>,
) -> String {
    let mut message = format!(
        "`{}`\n\n{}",
        compact_swarm_path(path),
        file_activity_summary_line(operation, summary)
    );

    if let Some(intent) = intent.map(str::trim).filter(|intent| !intent.is_empty()) {
        message.push_str("\n\nIntent: ");
        message.push_str(intent);
    }

    if let Some(detail) = detail.map(str::trim).filter(|detail| !detail.is_empty()) {
        message.push_str("\n\n```text\n");
        message.push_str(&sanitize_code_fence_content(detail));
        message.push_str("\n```");
    }

    message
}

pub(super) fn present_swarm_notification(
    sender: &str,
    notification_type: &NotificationType,
    message: &str,
) -> SwarmNotificationPresentation {
    let trimmed = message.trim();
    match notification_type {
        NotificationType::Message { scope, channel } => match scope.as_deref() {
            Some("dm") => {
                if let Some(task_body) =
                    strip_message_prefix(trimmed, "Task assigned to you by coordinator: ")
                {
                    SwarmNotificationPresentation {
                        title: format!("Task · {}", sender),
                        message: task_body.to_string(),
                        status_notice: format!("Task assigned by {}", sender),
                    }
                } else {
                    SwarmNotificationPresentation {
                        title: format!("DM from {}", sender),
                        message: compact_direct_message_body(trimmed),
                        status_notice: format!("DM from {}", sender),
                    }
                }
            }
            Some("channel") => SwarmNotificationPresentation {
                title: format!("#{} · {}", channel.as_deref().unwrap_or("channel"), sender),
                message: compact_channel_message_body(trimmed),
                status_notice: format!(
                    "Channel message · #{}",
                    channel.as_deref().unwrap_or("channel")
                ),
            },
            Some("broadcast") => SwarmNotificationPresentation {
                title: format!("Broadcast · {}", sender),
                message: compact_broadcast_message_body(trimmed),
                status_notice: format!("Broadcast from {}", sender),
            },
            Some("plan") => SwarmNotificationPresentation {
                title: format!("Plan · {}", sender),
                message: compact_plan_message_body(trimmed),
                status_notice: "Swarm plan updated".to_string(),
            },
            Some("swarm") => SwarmNotificationPresentation {
                title: format!("Swarm · {}", sender),
                message: trimmed.to_string(),
                status_notice: "Swarm update".to_string(),
            },
            Some("background_task") => SwarmNotificationPresentation {
                title: if trimmed.starts_with("**Background task progress**") {
                    "Background task progress".to_string()
                } else {
                    "Background task".to_string()
                },
                message: trimmed.to_string(),
                status_notice: if let Some(progress) =
                    crate::message::parse_background_task_progress_notification_markdown(trimmed)
                {
                    format!(
                        "Background task · {} · {}",
                        crate::message::background_task_display_label(
                            &progress.tool_name,
                            progress.display_name.as_deref()
                        ),
                        progress.summary
                    )
                } else if trimmed.starts_with("**Background task progress**") {
                    "Background task progress".to_string()
                } else {
                    "Background task update".to_string()
                },
            },
            Some(other) => SwarmNotificationPresentation {
                title: format!("{} · {}", capitalize(other), sender),
                message: trimmed.to_string(),
                status_notice: format!("{} update", capitalize(other)),
            },
            None => SwarmNotificationPresentation {
                title: format!("Swarm · {}", sender),
                message: trimmed.to_string(),
                status_notice: "Swarm update".to_string(),
            },
        },
        NotificationType::SharedContext { key, value } => SwarmNotificationPresentation {
            title: format!("Shared context · {}", sender),
            message: format!("{} = {}", key, value).trim().to_string(),
            status_notice: format!("Shared context: {}", key),
        },
        NotificationType::FileConflict {
            path,
            operation,
            intent,
            summary,
            detail,
        } => SwarmNotificationPresentation {
            title: format!("File activity · {}", sender),
            message: format_file_activity_message(
                path,
                operation,
                intent.as_deref(),
                summary.as_deref(),
                detail.as_deref(),
            ),
            status_notice: format!("File activity · {}", compact_swarm_path(path)),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::{compact_plan_message_body, present_swarm_notification};
    use crate::protocol::NotificationType;

    #[test]
    fn compact_plan_message_body_drops_redundant_plan_prefix() {
        assert_eq!(
            compact_plan_message_body("Plan updated by sheep (4 items, v1)"),
            "4 items · v1"
        );
        assert_eq!(
            compact_plan_message_body(
                "Plan updated: task 'issue41-memory-headed' assigned to session_mouse_1774660180567.",
            ),
            "Assigned issue41-memory-headed → mouse"
        );
    }

    #[test]
    fn present_swarm_notification_formats_task_assignments_as_tasks() {
        let presentation = present_swarm_notification(
            "sheep",
            &NotificationType::Message {
                scope: Some("dm".to_string()),
                channel: None,
            },
            "Task assigned to you by coordinator: Implement compaction asymptotic fixes — You own the compaction task.",
        );

        assert_eq!(presentation.title, "Task · sheep");
        assert_eq!(
            presentation.message,
            "Implement compaction asymptotic fixes — You own the compaction task."
        );
        assert_eq!(presentation.status_notice, "Task assigned by sheep");
    }

    #[test]
    fn present_swarm_notification_formats_background_task_scope_cleanly() {
        let presentation = present_swarm_notification(
            "background task",
            &NotificationType::Message {
                scope: Some("background_task".to_string()),
                channel: None,
            },
            "Background task failed · selfdev-build · exit 101",
        );

        assert_eq!(presentation.title, "Background task");
        assert_eq!(
            presentation.message,
            "Background task failed · selfdev-build · exit 101"
        );
        assert_eq!(presentation.status_notice, "Background task update");
    }

    #[test]
    fn present_swarm_notification_formats_background_task_progress_notice() {
        let presentation = present_swarm_notification(
            "background task",
            &NotificationType::Message {
                scope: Some("background_task".to_string()),
                channel: None,
            },
            "**Background task progress** `bg123` · `bash`\n\n[#####-------] 42% · Running tests (reported)",
        );

        assert_eq!(presentation.title, "Background task progress");
        assert_eq!(
            presentation.status_notice,
            "Background task · bash · 42% · Running tests"
        );
    }

    #[test]
    fn present_swarm_notification_strips_redundant_dm_prefix() {
        let presentation = present_swarm_notification(
            "sheep",
            &NotificationType::Message {
                scope: Some("dm".to_string()),
                channel: None,
            },
            "DM from sheep: I can see your worktree diff.",
        );

        assert_eq!(presentation.title, "DM from sheep");
        assert_eq!(presentation.message, "I can see your worktree diff.");
        assert_eq!(presentation.status_notice, "DM from sheep");
    }

    #[test]
    fn present_swarm_notification_compacts_plan_titles_and_bodies() {
        let presentation = present_swarm_notification(
            "sheep",
            &NotificationType::Message {
                scope: Some("plan".to_string()),
                channel: None,
            },
            "Plan updated by sheep (4 items, v1)",
        );

        assert_eq!(presentation.title, "Plan · sheep");
        assert_eq!(presentation.message, "4 items · v1");
        assert_eq!(presentation.status_notice, "Swarm plan updated");
    }

    #[test]
    fn present_swarm_notification_formats_file_activity_with_compact_path_and_preview() {
        let presentation = present_swarm_notification(
            "moss",
            &NotificationType::FileConflict {
                path: "/home/jeremy/jcode/src/tool/communicate.rs".to_string(),
                operation: "edited".to_string(),
                intent: Some("wire swarm intent display".to_string()),
                summary: Some("edited lines 323-348 (1 occurrence)".to_string()),
                detail: Some("323- old line\n323+ new line".to_string()),
            },
            "⚠ File activity: /home/jeremy/jcode/src/tool/communicate.rs — moss just edited this file you previously worked with: edited lines 323-348 (1 occurrence)",
        );

        assert_eq!(presentation.title, "File activity · moss");
        assert!(
            presentation
                .message
                .contains("`…/jcode/src/tool/communicate.rs`")
        );
        assert!(
            presentation
                .message
                .contains("Edited lines 323-348 (1 occurrence)")
        );
        assert!(
            presentation
                .message
                .contains("Intent: wire swarm intent display")
        );
        assert!(
            presentation
                .message
                .contains("```text\n323- old line\n323+ new line\n```")
        );
        assert_eq!(
            presentation.status_notice,
            "File activity · …/jcode/src/tool/communicate.rs"
        );
    }
}
