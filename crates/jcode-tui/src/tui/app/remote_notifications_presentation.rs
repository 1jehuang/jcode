//! Swarm notification presentation.

use super::*;

pub(super) fn present_swarm_notification_inner(
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
        NotificationType::FileConflict { path, .. } => {
            let Some(edit) = FileActivityEdit::from_notification(sender, notification_type) else {
                return SwarmNotificationPresentation {
                    title: format!("File activity · {}", sender),
                    message: compact_swarm_path(path),
                    status_notice: format!("File activity · {}", compact_swarm_path(path)),
                };
            };
            let message = file_activity_stack_message(std::slice::from_ref(&edit), false);
            SwarmNotificationPresentation {
                title: message.title.unwrap_or_else(|| "File activity".to_string()),
                message: message.content,
                status_notice: format!("File activity · {}", compact_swarm_path(path)),
            }
        }
    }
}

/// The single transcript row for a stack of peer edits.
///
/// One edit shows the agent, its line counts and intent. Several edits show
/// only each agent's emoji and summed line counts. Paths and diffs stay
/// behind the expand control.
pub(in super::super) fn file_activity_stack_message(
    stack: &[FileActivityEdit],
    expanded: bool,
) -> crate::tui::DisplayMessage {
    let (title, tldr) = match stack {
        [edit] => (
            format!(
                "{} · {}",
                if edit.is_conflict() {
                    "File conflict"
                } else {
                    "File activity"
                },
                edit.sender
            ),
            format!(
                "{}{} · {}",
                edit.change_label(),
                if edit.edits > 1 {
                    format!(" · {} edits", edit.edits)
                } else {
                    String::new()
                },
                edit.intent
                    .clone()
                    .unwrap_or_else(|| compact_swarm_path(&edit.path))
            ),
        ),
        stack => {
            let mut senders: Vec<&str> = Vec::new();
            for edit in stack {
                if !senders.contains(&edit.sender.as_str()) {
                    senders.push(&edit.sender);
                }
            }
            let parts = senders
                .iter()
                .map(|sender| {
                    let own = stack.iter().filter(|edit| edit.sender == *sender);
                    let edits: u32 = own.clone().map(|edit| edit.edits).sum();
                    let delta = own.clone().try_fold((0u64, 0u64), |(a, r), edit| {
                        edit.delta.map(|(added, removed)| (a + added, r + removed))
                    });
                    let mut part = crate::id::session_icon(sender).to_string();
                    if let Some((added, removed)) = delta {
                        part.push_str(&format!(" +{added} -{removed}"));
                    }
                    if edits > 1 || delta.is_none() {
                        part.push_str(&format!(
                            " · {edits} edit{}",
                            if edits == 1 { "" } else { "s" }
                        ));
                    }
                    part
                })
                .collect::<Vec<_>>();
            ("File activity".to_string(), parts.join("  "))
        }
    };
    let body = stack
        .iter()
        .map(|edit| {
            let mut part = format!(
                "{} {} · {} · {}",
                crate::id::session_icon(&edit.sender),
                edit.sender,
                compact_swarm_path(&edit.path),
                edit.change_label()
            );
            if let Some(intent) = &edit.intent {
                part.push_str(&format!("\n\nIntent: {intent}"));
            }
            if let Some(detail) = &edit.detail {
                part.push_str(&format!(
                    "\n\n```text\n{}\n```",
                    sanitize_code_fence_content(detail)
                ));
            }
            part
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    let tldr = tldr
        .replace('`', "")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let content = jcode_tui_messages::encode_collapsible_swarm_content(&tldr, &body);
    let content = if expanded {
        jcode_tui_messages::toggle_collapsible_swarm_content(&content).unwrap_or(content)
    } else {
        content
    };
    crate::tui::DisplayMessage::swarm(title, content)
}

/// If `content` is the model-facing text of a swarm message delivered to this
/// session (`DM from X: ...`, `#chan from X: ...`, `broadcast from X: ...`),
/// return a one-row swarm display message for it instead of the raw text.
pub(in super::super) fn swarm_delivery_display_message(
    content: &str,
) -> Option<crate::tui::DisplayMessage> {
    let trimmed = content.trim();
    let (title, body) = if trimmed.starts_with("Agent ") {
        // Completion wakes use plain prose rather than the DM wire prefix.
        let notice = parse_completion_notice(trimmed)?;
        return Some(crate::tui::DisplayMessage::swarm(
            format!("Swarm · {}", notice.sender),
            completion_notice_content(trimmed, None)
                .unwrap_or_else(|| jcode_tui_messages::collapse_long_swarm_content(trimmed)),
        ));
    } else if let Some(rest) = trimmed.strip_prefix("Cross-swarm DM from ") {
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
