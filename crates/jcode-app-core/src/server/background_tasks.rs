use super::live_turn::{LiveTurnSwarmContext, run_live_turn_if_idle};
use super::state::SwarmEvent;
use super::{
    SessionAgents, SessionInterruptQueues, SwarmMember, fanout_session_event,
    queue_soft_interrupt_for_session,
};
use crate::message::{
    format_background_task_notification_markdown, format_background_task_progress_markdown,
};
use crate::protocol::{NotificationType, ServerEvent};
use jcode_agent_runtime::SoftInterruptSource;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use tokio::sync::{RwLock, broadcast};

async fn emit_external_wake(
    session_id: &str,
    reason: &str,
    notification: &str,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
) -> bool {
    if crate::config::config().server.wake_mode != crate::config::WakeMode::External {
        return false;
    }
    let _ = fanout_session_event(
        swarm_members,
        session_id,
        ServerEvent::WakeRequested {
            session_id: session_id.to_string(),
            reason: reason.to_string(),
            notification: notification.to_string(),
        },
    )
    .await;
    true
}

#[expect(
    clippy::too_many_arguments,
    reason = "background task completion needs session, interrupt, and swarm status state"
)]
pub(super) async fn dispatch_background_task_completion(
    task: &crate::bus::BackgroundTaskCompleted,
    sessions: &SessionAgents,
    soft_interrupt_queues: &SessionInterruptQueues,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: &Arc<RwLock<HashMap<String, HashSet<String>>>>,
    event_history: &Arc<RwLock<VecDeque<SwarmEvent>>>,
    event_counter: &Arc<AtomicU64>,
    swarm_event_tx: &broadcast::Sender<SwarmEvent>,
) {
    let notification = format_background_task_notification_markdown(task);

    if task.notify
        && fanout_session_event(
            swarm_members,
            &task.session_id,
            ServerEvent::Notification {
                from_session: "background_task".to_string(),
                from_name: Some("background task".to_string()),
                notification_type: NotificationType::Message {
                    scope: Some("background_task".to_string()),
                    channel: None,
                    tldr: None,
                },
                message: notification.clone(),
            },
        )
        .await
            == 0
    {
        crate::logging::warn(&format!(
            "Failed to notify attached clients for background task completion on session {}",
            task.session_id
        ));
    }

    if task.wake
        && !emit_external_wake(
            &task.session_id,
            "background_task_completed",
            &notification,
            swarm_members,
        )
        .await
        && !run_live_turn_if_idle(
            &task.session_id,
            &notification,
            Some(
                "A background task for this session just finished. Review the completion message and continue if useful."
                    .to_string(),
            ),
            sessions,
            LiveTurnSwarmContext::new(
                swarm_members,
                swarms_by_id,
                event_history,
                event_counter,
                swarm_event_tx,
            ),
        )
        .await
        && !queue_soft_interrupt_for_session(
            &task.session_id,
            notification.clone(),
            false,
            SoftInterruptSource::BackgroundTask,
            soft_interrupt_queues,
            sessions,
        )
        .await
    {
        crate::logging::warn(&format!(
            "Failed to deliver background task completion to session {}",
            task.session_id
        ));
    }
}

/// Deliver a stall-watchdog wake for a background task that has gone quiet.
///
/// Mirrors completion delivery: optionally notify attached clients, then wake
/// an idle agent or queue a soft interrupt for a busy one. The task is still
/// running; the message tells the agent to inspect and decide.
#[expect(
    clippy::too_many_arguments,
    reason = "background task stall delivery needs session, interrupt, and swarm status state"
)]
pub(super) async fn dispatch_background_task_stalled(
    task: &crate::bus::BackgroundTaskStalled,
    sessions: &SessionAgents,
    soft_interrupt_queues: &SessionInterruptQueues,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: &Arc<RwLock<HashMap<String, HashSet<String>>>>,
    event_history: &Arc<RwLock<VecDeque<SwarmEvent>>>,
    event_counter: &Arc<AtomicU64>,
    swarm_event_tx: &broadcast::Sender<SwarmEvent>,
) {
    let notification = crate::message::format_background_task_stalled_markdown(task);

    if task.notify
        && fanout_session_event(
            swarm_members,
            &task.session_id,
            ServerEvent::Notification {
                from_session: "background_task".to_string(),
                from_name: Some("background task".to_string()),
                notification_type: NotificationType::Message {
                    scope: Some("background_task".to_string()),
                    channel: None,
                    tldr: None,
                },
                message: notification.clone(),
            },
        )
        .await
            == 0
    {
        crate::logging::warn(&format!(
            "Failed to notify attached clients for background task stall on session {}",
            task.session_id
        ));
    }

    if task.wake
        && !emit_external_wake(
            &task.session_id,
            "background_task_stalled",
            &notification,
            swarm_members,
        )
        .await
        && !run_live_turn_if_idle(
            &task.session_id,
            &notification,
            Some(
                "A background task for this session has produced no output or progress for its stall window. Inspect it and decide whether to keep waiting, fix it, or cancel it."
                    .to_string(),
            ),
            sessions,
            LiveTurnSwarmContext::new(
                swarm_members,
                swarms_by_id,
                event_history,
                event_counter,
                swarm_event_tx,
            ),
        )
        .await
        && !queue_soft_interrupt_for_session(
            &task.session_id,
            notification.clone(),
            false,
            SoftInterruptSource::BackgroundTask,
            soft_interrupt_queues,
            sessions,
        )
        .await
    {
        crate::logging::warn(&format!(
            "Failed to deliver background task stall to session {}",
            task.session_id
        ));
    }
}

/// Deliver the result of a backgrounded `swarm await_members` watcher to the
/// requesting session. Mirrors background-task completion delivery: optionally
/// notify attached clients, then wake an idle agent or queue a soft interrupt
/// for a busy one.
#[expect(
    clippy::too_many_arguments,
    reason = "swarm await completion needs session, interrupt, and swarm status state"
)]
pub(super) async fn dispatch_swarm_await_completion(
    event: &crate::bus::SwarmAwaitCompleted,
    sessions: &SessionAgents,
    soft_interrupt_queues: &SessionInterruptQueues,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: &Arc<RwLock<HashMap<String, HashSet<String>>>>,
    event_history: &Arc<RwLock<VecDeque<SwarmEvent>>>,
    event_counter: &Arc<AtomicU64>,
    swarm_event_tx: &broadcast::Sender<SwarmEvent>,
) {
    if event.notify
        && fanout_session_event(
            swarm_members,
            &event.session_id,
            ServerEvent::Notification {
                from_session: "swarm".to_string(),
                from_name: Some("swarm await".to_string()),
                notification_type: NotificationType::Message {
                    scope: Some("swarm_await".to_string()),
                    channel: None,
                    tldr: None,
                },
                message: event.notification.clone(),
            },
        )
        .await
            == 0
    {
        crate::logging::warn(&format!(
            "Failed to notify attached clients for swarm await completion on session {}",
            event.session_id
        ));
    }

    if !event.wake {
        return;
    }

    if emit_external_wake(
        &event.session_id,
        "swarm_await_completed",
        &event.notification,
        swarm_members,
    )
    .await
    {
        return;
    }

    if !run_live_turn_if_idle(
        &event.session_id,
        &event.notification,
        Some(
            "A swarm await you started just resolved. Review the result and continue if useful."
                .to_string(),
        ),
        sessions,
        LiveTurnSwarmContext::new(
            swarm_members,
            swarms_by_id,
            event_history,
            event_counter,
            swarm_event_tx,
        ),
    )
    .await
        && !queue_soft_interrupt_for_session(
            &event.session_id,
            event.notification.clone(),
            false,
            SoftInterruptSource::BackgroundTask,
            soft_interrupt_queues,
            sessions,
        )
        .await
    {
        crate::logging::warn(&format!(
            "Failed to deliver swarm await completion to session {}",
            event.session_id
        ));
    }
}

pub(super) async fn dispatch_background_task_progress(
    task: &crate::bus::BackgroundTaskProgressEvent,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
) {
    let notification = format_background_task_progress_markdown(task);
    if fanout_session_event(
        swarm_members,
        &task.session_id,
        ServerEvent::Notification {
            from_session: "background_task".to_string(),
            from_name: Some("background task".to_string()),
            notification_type: NotificationType::Message {
                scope: Some("background_task".to_string()),
                channel: None,
                tldr: None,
            },
            message: notification,
        },
    )
    .await
        == 0
    {
        crate::logging::warn(&format!(
            "Failed to notify attached clients for background task progress on session {}",
            task.session_id
        ));
    }
}

/// Update a swarm worker's cached output tail and rebroadcast swarm status so
/// the coordinator's inline gallery can render the live viewport. The tail is
/// already capped by the producer; we only store and fan it out.
pub(super) async fn dispatch_swarm_output_tail(
    tail: &crate::bus::SwarmOutputTail,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: &Arc<RwLock<HashMap<String, HashSet<String>>>>,
) {
    let swarm_id = {
        let mut members = swarm_members.write().await;
        let Some(member) = members.get_mut(&tail.session_id) else {
            return;
        };
        member.output_tail = Some(tail.tail.clone());
        member.swarm_id.clone()
    };
    if let Some(swarm_id) = swarm_id {
        super::swarm::broadcast_swarm_status(&swarm_id, swarm_members, swarms_by_id).await;
    }
}

/// Update a swarm member's aggregate todo progress (completed/total) and a
/// compact snapshot of the items themselves from a `TodoUpdated` bus event,
/// then rebroadcast swarm status so coordinators see the counter move and the
/// focused inline panel can list what the agent is working through. Only the
/// counts and capped display essentials cross the swarm boundary.
pub(super) async fn dispatch_swarm_todo_progress(
    event: &crate::bus::TodoEvent,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: &Arc<RwLock<HashMap<String, HashSet<String>>>>,
) {
    let total = event.todos.len() as u32;
    let completed = event
        .todos
        .iter()
        .filter(|t| t.status == "completed")
        .count() as u32;
    let progress = if total == 0 {
        None
    } else {
        Some((completed, total))
    };
    let mut items = compact_todo_items(&event.todos);

    let swarm_id = {
        let mut members = swarm_members.write().await;
        let Some(member) = members.get_mut(&event.session_id) else {
            return;
        };
        // Keep tool activity attached while the same todo remains active. A
        // transition to a different active item starts a fresh intent history.
        let old_active = member
            .todo_items
            .iter()
            .find(|item| item.status == "in_progress");
        let new_active = items.iter_mut().find(|item| item.status == "in_progress");
        if let (Some(old), Some(new)) = (old_active, new_active)
            && old.content == new.content
        {
            new.tool_intents = old.tool_intents.clone();
        }
        if member.todo_progress == progress && member.todo_items == items {
            return; // no change, skip the broadcast
        }
        member.todo_progress = progress;
        member.todo_items = items;
        member.swarm_id.clone()
    };
    if let Some(swarm_id) = swarm_id {
        super::swarm::broadcast_swarm_status(&swarm_id, swarm_members, swarms_by_id).await;
    }
}

/// Mirror a worker's three most recent agent-provided tool intents beneath its
/// active todo. Running/completed/error events update the same correlated row.
pub(super) async fn dispatch_swarm_tool_activity(
    event: &crate::bus::ToolEvent,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: &Arc<RwLock<HashMap<String, HashSet<String>>>>,
) {
    let swarm_id = {
        let mut members = swarm_members.write().await;
        let Some(member) = members.get_mut(&event.session_id) else {
            return;
        };
        if !update_active_todo_tool(&mut member.todo_items, event) {
            return;
        }
        member.swarm_id.clone()
    };

    if let Some(swarm_id) = swarm_id {
        super::swarm::broadcast_swarm_status(&swarm_id, swarm_members, swarms_by_id).await;
    }
}

pub(super) async fn dispatch_swarm_runtime_status(
    event: &crate::bus::SubagentStatus,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: &Arc<RwLock<HashMap<String, HashSet<String>>>>,
) {
    let Some(model) = event
        .model
        .as_ref()
        .filter(|model| !model.trim().is_empty())
    else {
        return;
    };
    let swarm_id = {
        let mut members = swarm_members.write().await;
        let Some(member) = members.get_mut(&event.session_id) else {
            return;
        };
        if member.runtime.model.as_ref() == Some(model) {
            return;
        }
        member.runtime.model = Some(model.clone());
        member.swarm_id.clone()
    };
    if let Some(swarm_id) = swarm_id {
        super::swarm::broadcast_swarm_status(&swarm_id, swarm_members, swarms_by_id).await;
    }
}

pub(super) async fn dispatch_swarm_batch_progress(
    progress: &crate::bus::BatchProgress,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: &Arc<RwLock<HashMap<String, HashSet<String>>>>,
) {
    if progress.total == 0 {
        return;
    }
    let swarm_id = {
        let mut members = swarm_members.write().await;
        let Some(member) = members.get_mut(&progress.session_id) else {
            return;
        };
        if !update_active_todo_batch_progress(&mut member.todo_items, progress) {
            return;
        }
        member.swarm_id.clone()
    };
    if let Some(swarm_id) = swarm_id {
        super::swarm::broadcast_swarm_status(&swarm_id, swarm_members, swarms_by_id).await;
    }
}

fn update_active_todo_batch_progress(
    todo_items: &mut [crate::protocol::SwarmTodoItem],
    progress: &crate::bus::BatchProgress,
) -> bool {
    let Some(tool) = todo_items
        .iter_mut()
        .find(|todo| todo.status == "in_progress")
        .and_then(|todo| {
            todo.tool_intents
                .iter_mut()
                .find(|tool| tool.tool_call_id == progress.tool_call_id)
        })
    else {
        return false;
    };
    let next = crate::protocol::SwarmToolProgress {
        current: progress.completed as u64,
        total: progress.total as u64,
        unit: Some("tools".to_string()),
    };
    if tool.progress.as_ref() == Some(&next) {
        return false;
    }
    tool.progress = Some(next);
    true
}

fn update_active_todo_tool(
    todo_items: &mut [crate::protocol::SwarmTodoItem],
    event: &crate::bus::ToolEvent,
) -> bool {
    let Some(intent) = event
        .intent
        .as_deref()
        .map(str::trim)
        .filter(|intent| !intent.is_empty())
    else {
        return false;
    };
    let Some(active) = todo_items
        .iter_mut()
        .find(|item| item.status == "in_progress")
    else {
        return false;
    };

    let status = event.status.as_str().to_string();
    if let Some(existing) = active
        .tool_intents
        .iter_mut()
        .find(|tool| tool.tool_call_id == event.tool_call_id)
    {
        existing.tool_name = event.tool_name.clone();
        existing.intent = cap_chars(intent, SWARM_TOOL_INTENT_CAP);
        existing.status = status;
    } else {
        active.tool_intents.push(crate::protocol::SwarmToolIntent {
            tool_call_id: event.tool_call_id.clone(),
            tool_name: event.tool_name.clone(),
            intent: cap_chars(intent, SWARM_TOOL_INTENT_CAP),
            status,
            progress: None,
        });
        if active.tool_intents.len() > SWARM_TOOL_INTENTS_CAP {
            active
                .tool_intents
                .drain(..active.tool_intents.len() - SWARM_TOOL_INTENTS_CAP);
        }
    }
    true
}

/// Max todo entries mirrored across the swarm status boundary per member.
const SWARM_TODO_ITEMS_CAP: usize = 12;
/// Max characters per mirrored todo entry.
const SWARM_TODO_CONTENT_CAP: usize = 120;
const SWARM_TOOL_INTENTS_CAP: usize = 3;
const SWARM_TOOL_INTENT_CAP: usize = 120;

/// Build the capped, display-only todo snapshot that crosses the swarm
/// boundary. Prefers showing the active window: everything from the first
/// non-completed item onward, then backfills with the most recent completed
/// items if there is room left in the cap.
fn compact_todo_items(todos: &[crate::todo::TodoItem]) -> Vec<crate::protocol::SwarmTodoItem> {
    let first_open = todos
        .iter()
        .position(|t| t.status != "completed")
        .unwrap_or_else(|| todos.len().saturating_sub(SWARM_TODO_ITEMS_CAP));
    // Show a little completed context above the active window when possible.
    let start = first_open.saturating_sub(2);
    todos
        .iter()
        .skip(start)
        .take(SWARM_TODO_ITEMS_CAP)
        .map(|t| crate::protocol::SwarmTodoItem {
            content: cap_chars(&t.content, SWARM_TODO_CONTENT_CAP),
            status: t.status.clone(),
            tool_intents: Vec::new(),
        })
        .collect()
}

fn cap_chars(s: &str, cap: usize) -> String {
    if s.chars().count() <= cap {
        return s.to_string();
    }
    let mut out: String = s.chars().take(cap.saturating_sub(1)).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::{BatchProgress, ToolEvent, ToolStatus};

    fn tool(id: &str, intent: &str, status: ToolStatus) -> ToolEvent {
        ToolEvent {
            session_id: "worker".into(),
            message_id: "message".into(),
            tool_call_id: id.into(),
            tool_name: "bash".into(),
            status,
            intent: Some(intent.into()),
            title: None,
        }
    }

    #[test]
    fn active_todo_keeps_last_three_tool_intents_and_updates_status_in_place() {
        let mut items = vec![crate::protocol::SwarmTodoItem {
            content: "test token refresh flow".into(),
            status: "in_progress".into(),
            tool_intents: Vec::new(),
        }];

        for id in ["one", "two", "three", "four"] {
            assert!(update_active_todo_tool(
                &mut items,
                &tool(id, &format!("intent {id}"), ToolStatus::Running),
            ));
        }
        let intents = &items[0].tool_intents;
        assert_eq!(intents.len(), 3);
        assert_eq!(intents[0].tool_call_id, "two");
        assert_eq!(intents[2].tool_call_id, "four");

        assert!(update_active_todo_tool(
            &mut items,
            &tool("four", "intent four", ToolStatus::Completed),
        ));
        assert_eq!(items[0].tool_intents.len(), 3);
        assert_eq!(items[0].tool_intents[2].status, "completed");
    }

    #[test]
    fn tool_intent_is_ignored_without_an_active_todo() {
        let mut items = vec![crate::protocol::SwarmTodoItem {
            content: "done".into(),
            status: "completed".into(),
            tool_intents: Vec::new(),
        }];
        assert!(!update_active_todo_tool(
            &mut items,
            &tool("one", "irrelevant", ToolStatus::Running),
        ));
        assert!(items[0].tool_intents.is_empty());
    }

    #[test]
    fn batch_progress_updates_the_correlated_active_tool() {
        let mut items = vec![crate::protocol::SwarmTodoItem {
            content: "run targeted tests".into(),
            status: "in_progress".into(),
            tool_intents: vec![crate::protocol::SwarmToolIntent {
                tool_call_id: "batch-1".into(),
                tool_name: "batch".into(),
                intent: "Run targeted authentication tests".into(),
                status: "running".into(),
                progress: None,
            }],
        }];
        let progress = BatchProgress {
            session_id: "worker".into(),
            tool_call_id: "batch-1".into(),
            completed: 27,
            total: 43,
            last_completed: Some("read".into()),
            running: Vec::new(),
            subcalls: Vec::new(),
        };

        assert!(update_active_todo_batch_progress(&mut items, &progress));
        let captured = items[0].tool_intents[0]
            .progress
            .as_ref()
            .expect("progress captured");
        assert_eq!((captured.current, captured.total), (27, 43));
        assert!(!update_active_todo_batch_progress(&mut items, &progress));
    }
}

pub(super) async fn dispatch_ui_activity(
    activity: &crate::bus::UiActivity,
    swarm_members: &Arc<RwLock<HashMap<String, SwarmMember>>>,
) {
    if activity.message.trim().is_empty() {
        return;
    }
    let Some(session_id) = activity.session_id.as_deref() else {
        return;
    };

    if fanout_session_event(
        swarm_members,
        session_id,
        ServerEvent::Notification {
            from_session: "jcode".to_string(),
            from_name: Some("Jcode".to_string()),
            notification_type: NotificationType::Message {
                scope: Some(activity.kind.scope().to_string()),
                channel: None,
                tldr: None,
            },
            message: activity.message.clone(),
        },
    )
    .await
        == 0
    {
        crate::logging::warn(&format!(
            "Failed to notify attached clients for UI activity on session {}",
            session_id
        ));
    }
}
