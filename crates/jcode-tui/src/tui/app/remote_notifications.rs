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

/// Single-line file-activity body for compact notifications mode: keeps the
/// compacted path and the summary line, dropping the intent and diff preview.
fn format_file_activity_message_compact(
    path: &str,
    operation: &str,
    summary: Option<&str>,
) -> String {
    format!(
        "`{}` · {}",
        compact_swarm_path(path),
        file_activity_summary_line(operation, summary)
    )
}

pub(super) fn present_swarm_notification(
    sender: &str,
    notification_type: &NotificationType,
    message: &str,
    compact: bool,
) -> SwarmNotificationPresentation {
    let mut presentation =
        present_swarm_notification_inner(sender, notification_type, message, compact);
    // Sender-provided tldr: store the full body but render it collapsed to the
    // tldr line with an expand control. The status line shows the tldr too.
    if let NotificationType::Message {
        tldr: Some(tldr), ..
    } = notification_type
    {
        let tldr = jcode_tui_messages::swarm_preview_line(
            tldr,
            jcode_tui_messages::SWARM_INLINE_BODY_MAX_CHARS,
        );
        if !tldr.is_empty() && !presentation.message.trim().is_empty() {
            presentation.status_notice = format!("{} · {}", presentation.status_notice, tldr);
            presentation.message =
                jcode_tui_messages::encode_collapsible_swarm_content(&tldr, &presentation.message);
        }
    }
    // Never render long agent prose inline: anything beyond one short row is
    // collapsed behind a derived one-row preview, expandable on demand.
    if collapses_to_one_row(notification_type) {
        presentation.message =
            jcode_tui_messages::collapse_long_swarm_content(&presentation.message);
    }
    presentation
}

/// Notification kinds whose transcript body is agent-authored prose that
/// should never render as more than one row by default. Plan, await and
/// background-task notices have their own compact renderers.
fn collapses_to_one_row(notification_type: &NotificationType) -> bool {
    match notification_type {
        NotificationType::Message { scope, .. } => !matches!(
            scope.as_deref(),
            Some("plan" | "background_task" | "swarm_await")
        ),
        NotificationType::SharedContext { .. } => true,
        NotificationType::FileConflict { .. } => false,
    }
}

/// If `content` is the model-facing text of a swarm message delivered to this
/// session (`DM from X: ...`, `#chan from X: ...`, `broadcast from X: ...`),
/// return a one-row swarm display message for it instead of the raw text.
pub(super) fn swarm_delivery_display_message(content: &str) -> Option<crate::tui::DisplayMessage> {
    let trimmed = content.trim();
    let (title, body) = if let Some(rest) = trimmed.strip_prefix("Cross-swarm DM from ") {
        let (sender, body) = rest.split_once(": ")?;
        (format!("DM from {}", sender.trim()), body)
    } else if let Some(rest) = trimmed.strip_prefix("DM from ") {
        let (sender, body) = rest.split_once(": ")?;
        if sender.contains(char::is_whitespace) {
            return None;
        }
        (format!("DM from {}", sender), body)
    } else if let Some(rest) = trimmed.strip_prefix("broadcast from ") {
        let (sender, body) = rest.split_once(": ")?;
        if sender.contains(char::is_whitespace) {
            return None;
        }
        (format!("Broadcast · {}", sender), body)
    } else {
        let rest = trimmed.strip_prefix('#')?;
        let (head, body) = rest.split_once(": ")?;
        let (channel, sender) = head.split_once(" from ")?;
        if channel.contains(char::is_whitespace) {
            return None;
        }
        (format!("#{} · {}", channel, sender.trim()), body)
    };
    if title.contains('\n') {
        return None;
    }
    Some(crate::tui::DisplayMessage::swarm(
        title,
        jcode_tui_messages::collapse_long_swarm_content(body),
    ))
}

/// Full underlying body of swarm display content, collapsed or not.
pub(super) fn swarm_display_full_body(content: &str) -> &str {
    match jcode_tui_messages::parse_collapsible_swarm_content(content) {
        Some(parsed) if !parsed.body.trim().is_empty() => parsed.body.trim(),
        Some(parsed) => parsed.tldr.trim(),
        None => content.trim(),
    }
}

/// Rewrite user/system transcript rows that are really delivered swarm
/// messages into one-row swarm rows. Used for restored history, where the
/// original notification card is not available.
pub(super) fn normalize_swarm_delivery_messages(messages: &mut [crate::tui::DisplayMessage]) {
    for message in messages.iter_mut() {
        if !matches!(message.role.as_str(), "user" | "system") {
            continue;
        }
        if let Some(swarm) = swarm_delivery_display_message(&message.content) {
            *message = swarm;
        }
    }
}

fn present_swarm_notification_inner(
    sender: &str,
    notification_type: &NotificationType,
    message: &str,
    _compact: bool,
) -> SwarmNotificationPresentation {
    let trimmed = message.trim();
    match notification_type {
        NotificationType::Message { scope, channel, .. } => match scope.as_deref() {
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
            Some("swarm_await") => SwarmNotificationPresentation {
                title: "🐝 Swarm await".to_string(),
                message: crate::tui::ui::compact_swarm_await_summary(trimmed),
                status_notice: "🐝 Swarm await finished".to_string(),
            },
            Some(other) => SwarmNotificationPresentation {
                title: format!("Swarm · {}", sender),
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
        } => {
            let summary_line =
                format_file_activity_message_compact(path, operation, summary.as_deref())
                    .replace('`', "");
            let mut detail_parts = Vec::new();
            if let Some(intent) = intent
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                detail_parts.push(format!("Intent: {intent}"));
            }
            if let Some(detail) = detail
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                detail_parts.push(format!(
                    "```text\n{}\n```",
                    sanitize_code_fence_content(detail)
                ));
            }
            let detail_body = detail_parts.join("\n\n");
            let has_details = intent
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty())
                || detail
                    .as_deref()
                    .is_some_and(|value| !value.trim().is_empty());
            let conflict = operation.to_ascii_lowercase().contains("conflict")
                || summary.as_deref().is_some_and(|value| {
                    let value = value.to_ascii_lowercase();
                    value.contains("conflict") || value.contains("concurrent")
                });
            SwarmNotificationPresentation {
                title: format!(
                    "{} · {}",
                    if conflict {
                        "File conflict"
                    } else {
                        "File activity"
                    },
                    sender
                ),
                message: if has_details {
                    jcode_tui_messages::encode_collapsible_swarm_content(
                        &summary_line,
                        &detail_body,
                    )
                } else {
                    summary_line
                },
                status_notice: format!("File activity · {}", compact_swarm_path(path)),
            }
        }
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
    fn present_swarm_notification_with_tldr_encodes_collapsed_content() {
        let presentation = present_swarm_notification(
            "sheep",
            &NotificationType::Message {
                scope: Some("dm".to_string()),
                channel: None,
                tldr: Some("fixed the flaky test".to_string()),
            },
            "DM from sheep: The flaky test was caused by a race in the setup helper. I rewrote it to use a barrier and verified 200 consecutive runs pass.",
            false,
        );

        let parsed = jcode_tui_messages::parse_collapsible_swarm_content(&presentation.message)
            .expect("tldr message should encode collapsible content");
        assert!(!parsed.expanded);
        assert_eq!(parsed.tldr, "fixed the flaky test");
        assert!(parsed.body.contains("race in the setup helper"));
        assert!(
            presentation.status_notice.contains("fixed the flaky test"),
            "{}",
            presentation.status_notice
        );
    }

    #[test]
    fn present_swarm_notification_without_tldr_collapses_long_body() {
        let body = "Investigated the parser. ".repeat(20);
        let presentation = present_swarm_notification(
            "sheep",
            &NotificationType::Message {
                scope: Some("dm".to_string()),
                channel: None,
                tldr: None,
            },
            &format!("DM from sheep: {body}\n\nMore details here."),
            false,
        );
        let parsed = jcode_tui_messages::parse_collapsible_swarm_content(&presentation.message)
            .expect("long body must collapse");
        assert!(!parsed.expanded);
        assert!(!parsed.tldr.contains('\n'));
        assert!(parsed.tldr.chars().count() <= jcode_tui_messages::SWARM_INLINE_BODY_MAX_CHARS);
        assert!(parsed.body.contains("More details here."));
    }

    #[test]
    fn swarm_delivery_display_message_collapses_injected_dm() {
        let long = "x".repeat(400);
        let msg = super::swarm_delivery_display_message(&format!("DM from fox: {long}"))
            .expect("dm delivery");
        assert_eq!(msg.role, "swarm");
        assert_eq!(msg.title.as_deref(), Some("DM from fox"));
        let parsed =
            jcode_tui_messages::parse_collapsible_swarm_content(&msg.content).expect("collapsed");
        assert_eq!(parsed.body, long);

        let chan = super::swarm_delivery_display_message("#parser from owl: hi").expect("channel");
        assert_eq!(chan.title.as_deref(), Some("#parser · owl"));
        assert_eq!(chan.content, "hi");

        assert!(super::swarm_delivery_display_message("DM from my boss: please review").is_none());
        assert!(super::swarm_delivery_display_message("hello there").is_none());
    }

    #[test]
    fn present_swarm_notification_without_tldr_keeps_plain_content() {
        let presentation = present_swarm_notification(
            "sheep",
            &NotificationType::Message {
                scope: Some("dm".to_string()),
                channel: None,
                tldr: None,
            },
            "DM from sheep: short note",
            false,
        );
        assert!(
            jcode_tui_messages::parse_collapsible_swarm_content(&presentation.message).is_none()
        );
        assert_eq!(presentation.message, "short note");
    }

    #[test]
    fn present_swarm_notification_formats_task_assignments_as_tasks() {
        let presentation = present_swarm_notification(
            "sheep",
            &NotificationType::Message {
                scope: Some("dm".to_string()),
                channel: None,
                tldr: None,
            },
            "Task assigned to you by coordinator: Implement compaction asymptotic fixes - You own the compaction task.",
            false,
        );

        assert_eq!(presentation.title, "Task · sheep");
        assert_eq!(
            presentation.message,
            "Implement compaction asymptotic fixes - You own the compaction task."
        );
        assert_eq!(presentation.status_notice, "Task assigned by sheep");
    }

    #[test]
    fn present_swarm_notification_compacts_swarm_await_without_report_prose() {
        let presentation = present_swarm_notification(
            "swarm await",
            &NotificationType::Message {
                scope: Some("swarm_await".to_string()),
                channel: None,
                tldr: None,
            },
            "🐝 **Swarm await finished**\n\nAll members done. All 2 members are done: fox, wolf\n\nMember statuses:\n  ✓ fox (completed)\n  ✓ wolf (completed)\n\nCompletion reports:\n\n--- fox (completed) ---\nParser tests pass.",
            false,
        );

        assert_eq!(presentation.title, "🐝 Swarm await");
        assert_eq!(presentation.message, "✓ 2/2");
        assert!(!presentation.message.contains("fox"));
        assert!(!presentation.message.contains("Parser tests"));
        assert_eq!(presentation.status_notice, "🐝 Swarm await finished");
    }

    #[test]
    fn present_swarm_notification_formats_background_task_scope_cleanly() {
        let presentation = present_swarm_notification(
            "background task",
            &NotificationType::Message {
                scope: Some("background_task".to_string()),
                channel: None,
                tldr: None,
            },
            "Background task failed · selfdev-build · exit 101",
            false,
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
                tldr: None,
            },
            "**Background task progress** `bg123` · `bash`\n\n[#####-------] 42% · Running tests (reported)",
            false,
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
                tldr: None,
            },
            "DM from sheep: I can see your worktree diff.",
            false,
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
                tldr: None,
            },
            "Plan updated by sheep (4 items, v1)",
            false,
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
            "⚠ File activity: /home/jeremy/jcode/src/tool/communicate.rs - moss just edited this file you previously worked with: edited lines 323-348 (1 occurrence)",
            false,
        );

        assert_eq!(presentation.title, "File activity · moss");
        let parsed = jcode_tui_messages::parse_collapsible_swarm_content(&presentation.message)
            .expect("file activity with details should be collapsible");
        assert_eq!(
            parsed.tldr,
            "…/jcode/src/tool/communicate.rs · Edited lines 323-348 (1 occurrence)"
        );
        assert!(parsed.body.contains("Intent: wire swarm intent display"));
        assert!(
            parsed
                .body
                .contains("```text\n323- old line\n323+ new line\n```")
        );
        assert_eq!(
            presentation.status_notice,
            "File activity · …/jcode/src/tool/communicate.rs"
        );
    }

    #[test]
    fn present_swarm_notification_compact_mode_collapses_file_activity_to_single_line() {
        let presentation = present_swarm_notification(
            "moss",
            &NotificationType::FileConflict {
                path: "/home/jeremy/jcode/src/tool/communicate.rs".to_string(),
                operation: "edited".to_string(),
                intent: Some("wire swarm intent display".to_string()),
                summary: Some("edited lines 323-348 (1 occurrence)".to_string()),
                detail: Some("323- old line\n323+ new line".to_string()),
            },
            "⚠ File activity: /home/jeremy/jcode/src/tool/communicate.rs - moss just edited this file you previously worked with: edited lines 323-348 (1 occurrence)",
            true,
        );

        assert_eq!(presentation.title, "File activity · moss");
        let parsed = jcode_tui_messages::parse_collapsible_swarm_content(&presentation.message)
            .expect("compact mode should retain collapsible details");
        assert_eq!(
            parsed.tldr,
            "…/jcode/src/tool/communicate.rs · Edited lines 323-348 (1 occurrence)"
        );
        assert!(parsed.body.contains("Intent: wire swarm intent display"));
        assert!(parsed.body.contains("323- old line"));
        assert_eq!(
            presentation.status_notice,
            "File activity · …/jcode/src/tool/communicate.rs"
        );
    }
}
