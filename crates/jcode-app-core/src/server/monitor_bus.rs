//! Swarm bus monitoring: file-touch conflicts, plan and member events.
//!
//! `Server::monitor_bus` is one long event loop with many sinks (file state,
//! swarm state, sessions, queues, event history). It lives here so `server.rs`
//! keeps only the wiring that spawns it.

use super::background_tasks::{
    dispatch_background_task_completion, dispatch_background_task_progress,
    dispatch_background_task_stalled, dispatch_swarm_await_completion,
    dispatch_swarm_batch_progress, dispatch_swarm_output_tail, dispatch_swarm_runtime_status,
    dispatch_swarm_todo_progress, dispatch_swarm_tool_activity, dispatch_ui_activity,
};
use super::file_activity::file_activity_scope_label;
use super::state::latest_peer_touches;
use super::*;
use crate::bus::BusEvent;
use crate::protocol::NotificationType;
use jcode_agent_runtime::SoftInterruptSource;

impl Server {
    /// Monitor the global Bus for FileTouch events and detect conflicts
    #[expect(
        clippy::too_many_arguments,
        reason = "bus monitor needs file state, swarm state, sessions, queues, and event history sinks"
    )]
    pub(super) async fn monitor_bus(
        mut receiver: tokio::sync::mpsc::UnboundedReceiver<crate::bus::RuntimeBusDelivery>,
        file_touch: FileTouchService,
        swarm_members: Arc<RwLock<HashMap<String, SwarmMember>>>,
        swarms_by_id: Arc<RwLock<HashMap<String, HashSet<String>>>>,
        swarm_plans: Arc<RwLock<HashMap<String, VersionedPlan>>>,
        swarm_coordinators: Arc<RwLock<HashMap<String, String>>>,
        _shared_context: Arc<RwLock<HashMap<String, HashMap<String, SharedContext>>>>,
        sessions: SessionAgents,
        soft_interrupt_queues: SessionInterruptQueues,
        event_history: Arc<RwLock<std::collections::VecDeque<SwarmEvent>>>,
        event_counter: Arc<std::sync::atomic::AtomicU64>,
        swarm_event_tx: broadcast::Sender<SwarmEvent>,
    ) {
        let mut last_cleanup = Instant::now();
        const TOUCH_EXPIRY: Duration = Duration::from_secs(30 * 60); // 30 min
        const CLEANUP_INTERVAL: Duration = Duration::from_secs(5 * 60); // 5 min

        loop {
            // Periodic cleanup of expired file touches
            if last_cleanup.elapsed() > CLEANUP_INTERVAL {
                file_touch.expire_older_than(TOUCH_EXPIRY).await;
                last_cleanup = Instant::now();
            }

            let Some(delivery) = receiver.recv().await else {
                break;
            };
            // Keep the envelope alive through asynchronous queue delivery, not
            // just receipt. A queued selected-session callback must exclude
            // transfer even after its original turn/background task exits.
            let Some(_activity) =
                self::background_tasks::admit_runtime_delivery(&delivery.event).await
            else {
                continue;
            };
            match delivery.event.clone() {
                BusEvent::FileTouch(touch) => {
                    let path = touch.path.clone();
                    let session_id = touch.session_id.clone();

                    // Record this touch
                    file_touch
                        .record_touch(
                            path.clone(),
                            FileAccess {
                                session_id: session_id.clone(),
                                op: touch.op.clone(),
                                timestamp: Instant::now(),
                                absolute_time: std::time::SystemTime::now(),
                                intent: touch.intent.clone(),
                                summary: touch.summary.clone(),
                                detail: touch.detail.clone(),
                            },
                        )
                        .await;

                    // Record event for subscription
                    {
                        let members = swarm_members.read().await;
                        let member = members.get(&session_id);
                        let session_name = member.and_then(|m| m.friendly_name.clone());
                        let swarm_id = member.and_then(|m| m.swarm_id.clone());

                        drop(members);
                        record_swarm_event(
                            &event_history,
                            &event_counter,
                            &swarm_event_tx,
                            session_id.clone(),
                            session_name,
                            swarm_id,
                            SwarmEventType::FileTouch {
                                path: path.to_string_lossy().to_string(),
                                op: touch.op.as_str().to_string(),
                                intent: touch.intent.clone(),
                                summary: touch.summary.clone(),
                                detail: touch.detail.clone(),
                            },
                        )
                        .await;
                    }

                    // Find the swarm this session belongs to
                    let swarm_session_ids: Vec<String> = {
                        let members = swarm_members.read().await;
                        if let Some(member) = members.get(&session_id) {
                            if let Some(ref swarm_id) = member.swarm_id {
                                let swarms = swarms_by_id.read().await;
                                if let Some(swarm) = swarms.get(swarm_id) {
                                    swarm.iter().cloned().collect()
                                } else {
                                    vec![]
                                }
                            } else {
                                vec![]
                            }
                        } else {
                            vec![]
                        }
                    };

                    // Only notify on modifications, and only about prior peer modifications.
                    // Plain reads are still tracked for later context/listing but should not
                    // proactively alert the swarm.
                    let is_modification = touch.op.is_modification();
                    if is_modification {
                        crate::logging::info(&format!(
                            "[file-activity] modification by {} on {}, swarm_peers: {:?}",
                            &session_id[..8.min(session_id.len())],
                            path.display(),
                            swarm_session_ids
                                .iter()
                                .map(|s| &s[..8.min(s.len())])
                                .collect::<Vec<_>>()
                        ));
                    }
                    let previous_touches: Vec<FileAccess> = if is_modification {
                        if let Some(accesses) = file_touch.accesses_for_path(&path).await {
                            let swarm_session_ids_set: HashSet<String> =
                                swarm_session_ids.iter().cloned().collect();
                            let result =
                                latest_peer_touches(&accesses, &session_id, &swarm_session_ids_set);
                            crate::logging::info(&format!(
                                "[file-activity] {} prior peer touches ({} total accesses)",
                                result.len(),
                                accesses.len()
                            ));
                            result
                        } else {
                            crate::logging::info("[file-activity] no touches for this path yet");
                            vec![]
                        }
                    } else {
                        vec![]
                    };

                    // If swarm peers previously touched this file, notify both sides so they
                    // can coordinate before the work diverges further.
                    if !previous_touches.is_empty() {
                        crate::logging::info(&format!(
                            "[file-activity] {} touched by peers before modification — sending alerts",
                            path.display()
                        ));
                        let members = swarm_members.read().await;
                        let current_member = members.get(&session_id);
                        let current_name = current_member.and_then(|m| m.friendly_name.clone());

                        // Alert the current agent about previous peer touches (one per agent).
                        if let Some(member) = current_member {
                            for prev in &previous_touches {
                                let prev_member = members.get(&prev.session_id);
                                let prev_name = prev_member.and_then(|m| m.friendly_name.clone());
                                let scope = file_activity_scope_label(prev, &touch);
                                let intent_suffix = prev
                                    .intent
                                    .as_ref()
                                    .map(|intent| format!(" — intent: {}", intent))
                                    .unwrap_or_default();
                                let alert_msg = format!(
                                    "⚠ File activity: {} — {} — {} previously {} this file{}{}",
                                    path.display(),
                                    scope,
                                    prev_name.as_deref().unwrap_or(&prev.session_id[..8]),
                                    prev.op.as_str(),
                                    prev.summary
                                        .as_ref()
                                        .map(|s| format!(": {}", s))
                                        .unwrap_or_default(),
                                    intent_suffix
                                );
                                let notification = ServerEvent::Notification {
                                    from_session: prev.session_id.clone(),
                                    from_name: prev_name,
                                    notification_type: NotificationType::FileConflict {
                                        path: path.display().to_string(),
                                        operation: prev.op.as_str().to_string(),
                                        intent: prev.intent.clone(),
                                        summary: prev.summary.clone(),
                                        detail: prev.detail.clone(),
                                    },
                                    message: alert_msg.clone(),
                                };
                                let _ = member.event_tx.send(notification);

                                if !queue_soft_interrupt_for_session(
                                    &session_id,
                                    alert_msg.clone(),
                                    false,
                                    SoftInterruptSource::System,
                                    &soft_interrupt_queues,
                                    &sessions,
                                )
                                .await
                                {
                                    crate::logging::warn(&format!(
                                        "Failed to queue file-activity soft interrupt for session {}",
                                        session_id
                                    ));
                                }
                            }
                        }

                        // Alert previous agents about the current modification.
                        for prev in &previous_touches {
                            if let Some(prev_member) = members.get(&prev.session_id) {
                                let scope = file_activity_scope_label(prev, &touch);
                                let intent_suffix = touch
                                    .intent
                                    .as_ref()
                                    .map(|intent| format!(" — intent: {}", intent))
                                    .unwrap_or_default();
                                let alert_msg = format!(
                                    "⚠ File activity: {} — {} — {} just {} this file you previously worked with{}{}",
                                    path.display(),
                                    scope,
                                    current_name
                                        .as_deref()
                                        .unwrap_or(&session_id[..8.min(session_id.len())]),
                                    touch.op.as_str(),
                                    touch
                                        .summary
                                        .as_ref()
                                        .map(|s| format!(": {}", s))
                                        .unwrap_or_default(),
                                    intent_suffix
                                );
                                let notification = ServerEvent::Notification {
                                    from_session: session_id.clone(),
                                    from_name: current_name.clone(),
                                    notification_type: NotificationType::FileConflict {
                                        path: path.display().to_string(),
                                        operation: touch.op.as_str().to_string(),
                                        intent: touch.intent.clone(),
                                        summary: touch.summary.clone(),
                                        detail: touch.detail.clone(),
                                    },
                                    message: alert_msg.clone(),
                                };
                                let _ = prev_member.event_tx.send(notification);

                                if !queue_soft_interrupt_for_session(
                                    &prev.session_id,
                                    alert_msg.clone(),
                                    false,
                                    SoftInterruptSource::System,
                                    &soft_interrupt_queues,
                                    &sessions,
                                )
                                .await
                                {
                                    crate::logging::warn(&format!(
                                        "Failed to queue file-activity soft interrupt for session {}",
                                        prev.session_id
                                    ));
                                }
                            }
                        }
                    }
                }
                BusEvent::LaneEdited(edit) => {
                    // The coordinator steers a worker by editing its lane; the
                    // worker's todo list already matches, so just tell it.
                    let message = format!(
                        "[lane updated by coordinator] Your todo list (lane {}) changed: {}. Re-read it with the todo tool and continue from it.",
                        edit.lane, edit.summary
                    );
                    if !queue_soft_interrupt_for_session(
                        &edit.worker,
                        message,
                        false,
                        SoftInterruptSource::System,
                        &soft_interrupt_queues,
                        &sessions,
                    )
                    .await
                    {
                        crate::logging::warn(&format!(
                            "Failed to notify {} of lane edit {}",
                            edit.worker, edit.lane
                        ));
                    }
                }
                BusEvent::BackgroundTaskCompleted(task) => {
                    dispatch_background_task_completion(
                        &task,
                        &sessions,
                        &soft_interrupt_queues,
                        &swarm_members,
                        &swarms_by_id,
                        &event_history,
                        &event_counter,
                        &swarm_event_tx,
                    )
                    .await;
                }
                BusEvent::BackgroundTaskProgress(task) => {
                    dispatch_background_task_progress(&task, &swarm_members).await;
                }
                BusEvent::BackgroundTaskStalled(task) => {
                    dispatch_background_task_stalled(
                        &task,
                        &sessions,
                        &soft_interrupt_queues,
                        &swarm_members,
                        &swarms_by_id,
                        &event_history,
                        &event_counter,
                        &swarm_event_tx,
                    )
                    .await;
                }
                BusEvent::SwarmAwaitCompleted(event) => {
                    dispatch_swarm_await_completion(
                        &event,
                        &sessions,
                        &soft_interrupt_queues,
                        &swarm_members,
                        &swarms_by_id,
                        &event_history,
                        &event_counter,
                        &swarm_event_tx,
                    )
                    .await;
                }
                BusEvent::SwarmWorkerReported(event) => {
                    background_tasks::dispatch_swarm_worker_report(
                        &event,
                        &sessions,
                        &soft_interrupt_queues,
                        live_turn::LiveTurnSwarmContext::new(
                            &swarm_members,
                            &swarms_by_id,
                            &event_history,
                            &event_counter,
                            &swarm_event_tx,
                        ),
                    )
                    .await;
                }
                BusEvent::SessionUpdateStatus(status) => {
                    let (session_id, event) = runtime_ssh_host::maintenance_event(status);
                    fanout_session_event(&swarm_members, &session_id, event).await;
                }
                BusEvent::UiActivity(activity) => {
                    dispatch_ui_activity(&activity, &swarm_members).await;
                }
                BusEvent::ToolUpdated(event) => {
                    dispatch_swarm_tool_activity(&event, &swarm_members, &swarms_by_id).await;
                }
                BusEvent::SubagentStatus(event) => {
                    dispatch_swarm_runtime_status(&event, &swarm_members, &swarms_by_id).await;
                }
                BusEvent::BatchProgress(progress) => {
                    dispatch_swarm_batch_progress(&progress, &swarm_members, &swarms_by_id).await;
                }
                // Session todos are private to the session's transcript, but the
                // Compact todo names and progress are surfaced on the inline
                // swarm strip so a coordinator can see each managed agent's work.
                BusEvent::TodoUpdated(event) => {
                    dispatch_swarm_todo_progress(&event, &swarm_members, &swarms_by_id).await;
                    // Worker-owned lanes whose dependencies are met get a
                    // worker automatically; no launch call is involved.
                    self::lane_dispatch::on_todos_saved(&event.session_id, &event.todos);
                    // A coordinator's todo list is its lane plan: keep a
                    // running plan mirroring every save, so new or reshaped
                    // lanes are picked up without relaunching.
                    if let Some(swarm_id) = self::swarm_todo_link::resync_plan_after_todo_save(
                        &event.session_id,
                        &event.todos,
                        &swarm_members,
                        &swarm_plans,
                        &swarm_coordinators,
                    )
                    .await
                    {
                        self::swarm::broadcast_swarm_plan(
                            &swarm_id,
                            Some("todos_changed".to_string()),
                            &swarm_plans,
                            &swarm_members,
                            &swarms_by_id,
                        )
                        .await;
                    }
                }
                BusEvent::SwarmOutputTail(tail) => {
                    dispatch_swarm_output_tail(&tail, &swarm_members, &swarms_by_id).await;
                }
                _ => {
                    // Ignore other events
                }
            }
            drop(delivery);
        }
    }
}
