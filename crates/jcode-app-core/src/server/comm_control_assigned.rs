use super::*;
use futures::FutureExt;
use std::time::Duration;

pub(super) async fn begin_assigned_turn_publication(
    target_session: &str,
    swarm_id: &str,
    task_id: &str,
    swarm_plans: &Arc<RwLock<HashMap<String, VersionedPlan>>>,
) -> Option<Arc<super::super::swarm::TurnPublication>> {
    loop {
        let mut plans = swarm_plans.write().await;
        let plan = plans.get_mut(swarm_id)?;
        let item = plan.items.iter_mut().find(|item| item.id == task_id)?;
        if item
            .assigned_to
            .as_deref()
            .is_some_and(|owner| owner != target_session)
            || !matches!(
                item.status.as_str(),
                "queued" | "ready" | "pending" | "todo" | "running" | "running_stale"
            )
        {
            return None;
        }
        // Publication callbacks acquire plan locks in the opposite order.
        // Poll replacement once, never wait while owning the plan lock.
        // begin_turn_publication mutates only after its final await; dropping
        // a Pending attempt therefore leaves the old publication untouched.
        if let Some(publication) =
            super::super::swarm::begin_turn_publication(target_session).now_or_never()
        {
            super::super::usage_limit_resume::cancel_pending_resume(target_session);
            item.assigned_to = Some(target_session.to_string());
            return Some(publication);
        }
        drop(plans);
        // Reassignment may win while publication is busy. Revalidate before
        // every retry, before touching either the RAM ticket or disk record.
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "task execution restart needs session state, plan state, and event sinks together"
)]
pub(super) fn spawn_assigned_task_run(
    agent_arc: Arc<Mutex<Agent>>,
    target_session: String,
    swarm_id: String,
    task_id: String,
    assignment_text: String,
    swarm_members: Arc<RwLock<HashMap<String, SwarmMember>>>,
    swarms_by_id: Arc<RwLock<HashMap<String, HashSet<String>>>>,
    swarm_plans: Arc<RwLock<HashMap<String, VersionedPlan>>>,
    swarm_coordinators: Arc<RwLock<HashMap<String, String>>>,
    event_history: Arc<RwLock<std::collections::VecDeque<SwarmEvent>>>,
    event_counter: Arc<std::sync::atomic::AtomicU64>,
    swarm_event_tx: broadcast::Sender<SwarmEvent>,
) -> tokio::task::JoinHandle<()> {
    let assignment_text = append_swarm_completion_report_instructions(&assignment_text);
    tokio::spawn(async move {
        let mut initial_guard = Arc::clone(&agent_arc).lock_owned().await;
        if swarm_members
            .read()
            .await
            .get(&target_session)
            .is_some_and(|member| {
                matches!(member.status.as_str(), "stopped" | "failed" | "crashed")
            })
        {
            return;
        }
        let Some(publication) =
            begin_assigned_turn_publication(&target_session, &swarm_id, &task_id, &swarm_plans)
                .await
        else {
            return;
        };
        let swarm_state = SwarmState {
            members: Arc::clone(&swarm_members),
            swarms_by_id: Arc::clone(&swarms_by_id),
            plans: Arc::clone(&swarm_plans),
            coordinators: Arc::clone(&swarm_coordinators),
        };
        let mut start_accepted = false;
        if !publication
            .publish_start(async {
                {
                    let now_ms = now_unix_ms();
                    let mut plans = swarm_plans.write().await;
                    // A Stop or lifecycle transition can win while assignment
                    // start waits for plan/publication ownership. Never revive
                    // that member merely because its plan item still runs.
                    if !swarm_members
                        .read()
                        .await
                        .get(&target_session)
                        .is_some_and(|member| {
                            !matches!(
                                member.status.as_str(),
                                "stopped"
                                    | "completed"
                                    | "done"
                                    | "suspended"
                                    | "paused"
                                    | "failed"
                                    | "crashed"
                            )
                        })
                    {
                        return;
                    }
                    if let Some(plan) = plans.get_mut(&swarm_id)
                        && let Some(item) = plan.items.iter_mut().find(|item| item.id == task_id)
                        && item.assigned_to.as_deref() == Some(target_session.as_str())
                        && matches!(
                            item.status.as_str(),
                            "queued" | "ready" | "pending" | "todo" | "running" | "running_stale"
                        )
                    {
                        start_accepted = true;
                        item.status = "running".to_string();
                        let progress = plan.task_progress.entry(task_id.clone()).or_default();
                        progress.assigned_session_id = Some(target_session.clone());
                        progress.assignment_summary = Some(truncate_detail(&assignment_text, 120));
                        progress.started_at_unix_ms = Some(now_ms);
                        progress.last_heartbeat_unix_ms = Some(now_ms);
                        progress.last_detail = Some(truncate_detail(&assignment_text, 120));
                        progress.last_checkpoint_unix_ms = Some(now_ms);
                        progress.checkpoint_summary = Some("task started".to_string());
                        progress.completed_at_unix_ms = None;
                        progress.stale_since_unix_ms = None;
                        progress.heartbeat_count = Some(progress.heartbeat_count.unwrap_or(0) + 1);
                        progress.checkpoint_count =
                            Some(progress.checkpoint_count.unwrap_or(0) + 1);
                        plan.version += 1;
                    }
                }
                if !start_accepted {
                    return;
                }
                persist_swarm_state_for(&swarm_id, &swarm_state).await;
                broadcast_swarm_plan(
                    &swarm_id,
                    Some("task_running".to_string()),
                    &swarm_plans,
                    &swarm_members,
                    &swarms_by_id,
                )
                .await;
                set_member_task_label(&target_session, &assignment_text, &swarm_members).await;
                update_member_status(
                    &target_session,
                    "running",
                    Some(truncate_detail(&assignment_text, 120)),
                    &swarm_members,
                    &swarms_by_id,
                    Some(&event_history),
                    Some(&event_counter),
                    Some(&swarm_event_tx),
                )
                .await;
            })
            .await
            || !start_accepted
        {
            return;
        }

        let (heartbeat_stop_tx, mut heartbeat_stop_rx) = watch::channel(false);
        let heartbeat_task = {
            let target_session = target_session.clone();
            let swarm_id = swarm_id.clone();
            let task_id = task_id.clone();
            let state = swarm_state.clone();
            let publication = Arc::clone(&publication);
            tokio::spawn(async move {
                let mut interval = tokio::time::interval(swarm_task_heartbeat_interval());
                interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                interval.tick().await;
                loop {
                    tokio::select! {
                        biased;
                        _ = publication.cancelled.cancelled() => break,
                        changed = heartbeat_stop_rx.changed() => {
                            if changed.is_err() || *heartbeat_stop_rx.borrow() { break; }
                        }
                        _ = interval.tick() => {
                            // Do not queue on a plan lock while holding publication:
                            // quota continuation needs that ownership too.
                            let mut touched = false;
                            let mut revived = false;
                            if !publication.publish_start(async {
                                let Ok(mut plans) = state.plans.try_write() else { return; };
                                let Some(plan) = plans.get_mut(&swarm_id) else { return; };
                                let Some(item) = plan.items.iter_mut().find(|item| item.id == task_id) else { return; };
                                if item.assigned_to.as_deref() != Some(target_session.as_str())
                                    || !matches!(item.status.as_str(), "running" | "running_stale") { return; }
                                revived = item.status == "running_stale";
                                item.status = "running".to_string();
                                let progress = plan.task_progress.entry(task_id.clone()).or_default();
                                progress.last_heartbeat_unix_ms = Some(now_unix_ms());
                                progress.heartbeat_count = Some(progress.heartbeat_count.unwrap_or(0) + 1);
                                progress.stale_since_unix_ms = None;
                                if revived { plan.version += 1; }
                                touched = true;
                            }).await { break; }
                            if touched {
                                persist_swarm_state_for(&swarm_id, &state).await;
                                if revived {
                                    broadcast_swarm_plan(&swarm_id, Some("task_heartbeat".to_string()),
                                        &state.plans, &state.members, &state.swarms_by_id).await;
                                }
                            }
                        }
                    }
                }
            })
        };
        let event_tx = task_progress_event_sender(
            Arc::clone(&publication),
            target_session.clone(),
            swarm_id.clone(),
            task_id.clone(),
            Arc::clone(&swarm_members),
            Arc::clone(&swarms_by_id),
            Arc::clone(&swarm_plans),
            Arc::clone(&swarm_coordinators),
            Arc::clone(&event_history),
            Arc::clone(&event_counter),
            swarm_event_tx.clone(),
        );
        let start_message_index = initial_guard.message_count();
        let signal = initial_guard.graceful_shutdown_signal();
        let result = tokio::select! {
            biased;
            _ = publication.cancelled.cancelled() => None,
            result = publication.run(signal,
                super::super::client_lifecycle::process_locked_message_streaming_mpsc(
                    &mut initial_guard, &assignment_text, vec![], None, event_tx.clone(),
                )
            ) => result,
        };
        let Some(result) = result else {
            let _ = heartbeat_stop_tx.send(true);
            let _ = heartbeat_task.await;
            return;
        };
        // Keep the owned reservation through terminal publication, including
        // initial successes and all quota continuations.
        let (result, completion_report, _resume_guard) = match result {
            Err(error) => {
                let on_wait = {
                    let swarm_id = swarm_id.clone();
                    let task_id = task_id.clone();
                    let target_session = target_session.clone();
                    let state = swarm_state.clone();
                    move |plan: &super::super::usage_limit_resume::UsageLimitResumePlan| {
                        let detail = plan.status_detail();
                        let swarm_id = swarm_id.clone();
                        let task_id = task_id.clone();
                        let target_session = target_session.clone();
                        let state = state.clone();
                        Box::pin(async move {
                            touch_swarm_task_progress(
                                &swarm_id,
                                &task_id,
                                Some(&target_session),
                                Some(detail.clone()),
                                Some(detail),
                                &state.members,
                                &state.swarms_by_id,
                                &state.plans,
                                &state.coordinators,
                            )
                            .await;
                            broadcast_swarm_plan(
                                &swarm_id,
                                Some("task_rate_limited".to_string()),
                                &state.plans,
                                &state.members,
                                &state.swarms_by_id,
                            )
                            .await;
                        }) as futures::future::BoxFuture<'static, ()>
                    }
                };
                let swarm_ctx = super::super::live_turn::LiveTurnSwarmContext::new(
                    &swarm_members,
                    &swarms_by_id,
                    &event_history,
                    &event_counter,
                    &swarm_event_tx,
                );
                match super::super::usage_limit_resume::resume_turn_after_usage_limit_with_publication(
                    Arc::clone(&agent_arc), Some(initial_guard), &target_session, error,
                    None, &event_tx, &swarm_ctx, &on_wait, Some(&publication),
                ).await {
                    super::super::usage_limit_resume::ResumeOutcome::NotUsageLimit { error, guard }
                    | super::super::usage_limit_resume::ResumeOutcome::Failed { error, guard, .. } => (Err(error), None, guard),
                    super::super::usage_limit_resume::ResumeOutcome::Completed { guard, completion_report } =>
                        (Ok(()), completion_report, Some(guard)),
                    super::super::usage_limit_resume::ResumeOutcome::Superseded => {
                        let _ = heartbeat_stop_tx.send(true);
                        let _ = heartbeat_task.await;
                        if publication.cancelled.is_cancelled() { return; }
                        publication.complete(async {
                            let previous_items = {
                                let plans = swarm_plans.read().await;
                                plans.get(&swarm_id).map(|plan| plan.items.clone()).unwrap_or_default()
                            };
                            {
                                let now_ms = now_unix_ms();
                                let mut plans = swarm_plans.write().await;
                                if let Some(plan) = plans.get_mut(&swarm_id)
                                    && let Some(item) = plan.items.iter_mut().find(|item| item.id == task_id)
                                    && item.status == "running"
                                    && item.assigned_to.as_deref() == Some(target_session.as_str()) {
                                    item.status = "queued".to_string();
                                    item.assigned_to = None;
                                    let progress = plan.task_progress.entry(task_id.clone()).or_default();
                                    progress.assigned_session_id = None;
                                    progress.last_heartbeat_unix_ms = Some(now_ms);
                                    progress.last_checkpoint_unix_ms = Some(now_ms);
                                    progress.checkpoint_summary = Some("requeued: usage-limit resume was superseded".to_string());
                                    progress.stale_since_unix_ms = None;
                                    progress.checkpoint_count = Some(progress.checkpoint_count.unwrap_or(0) + 1);
                                    plan.version += 1;
                                }
                            }
                            persist_swarm_state_for(&swarm_id, &swarm_state).await;
                            broadcast_swarm_plan_with_previous(&swarm_id, Some("task_requeued_superseded".to_string()),
                                Some(&previous_items), &swarm_plans, &swarm_members, &swarms_by_id).await;
                            super::super::swarm::update_member_status_if(&target_session, "rate_limited", "ready", None,
                                &swarm_members, &swarms_by_id, Some(&event_history), Some(&event_counter), Some(&swarm_event_tx)).await;
                        }).await;
                        return;
                    }
                }
            }
            Ok(()) => {
                let report = initial_guard.latest_assistant_text_after(start_message_index);
                (Ok(()), report, Some(initial_guard))
            }
        };
        drop(event_tx);
        let _ = heartbeat_stop_tx.send(true);
        let _ = heartbeat_task.await;
        publication.complete(async {
            match result {
                Ok(_) => {
                    let previous_items = {
                        let plans = swarm_plans.read().await;
                        plans.get(&swarm_id).map(|plan| plan.items.clone()).unwrap_or_default()
                    };
                    let mut applied_disposition = TurnEndDisposition::LeaveAlone;
                    {
                        let now_ms = now_unix_ms();
                        let mut plans = swarm_plans.write().await;
                        if let Some(plan) = plans.get_mut(&swarm_id)
                            && let Some(item) = plan.items.iter_mut().find(|item| item.id == task_id)
                            && item.assigned_to.as_deref() == Some(target_session.as_str()) {
                            let expanded = plan.node_meta.get(&task_id).map(|m| m.expanded && !m.is_gate).unwrap_or(false);
                            let is_deep = plan.mode.eq_ignore_ascii_case("deep");
                            let prior_requeues = plan.task_progress.get(&task_id).and_then(|p| p.no_artifact_requeues).unwrap_or(0);
                            applied_disposition = turn_end_disposition(is_deep, &item.status, expanded, prior_requeues);
                            match applied_disposition {
                                TurnEndDisposition::AutoComplete => {
                                    item.status = "done".to_string();
                                    let progress = plan.task_progress.entry(task_id.clone()).or_default();
                                    progress.last_heartbeat_unix_ms = Some(now_ms);
                                    progress.last_checkpoint_unix_ms = Some(now_ms);
                                    progress.checkpoint_summary = Some("task completed".to_string());
                                    progress.completed_at_unix_ms = Some(now_ms);
                                    progress.stale_since_unix_ms = None;
                                    progress.checkpoint_count = Some(progress.checkpoint_count.unwrap_or(0) + 1);
                                    plan.version += 1;
                                }
                                TurnEndDisposition::RequeueNoArtifact => {
                                    item.status = "queued".to_string();
                                    item.assigned_to = None;
                                    let progress = plan.task_progress.entry(task_id.clone()).or_default();
                                    progress.assigned_session_id = None;
                                    progress.no_artifact_requeues = Some(prior_requeues + 1);
                                    progress.last_heartbeat_unix_ms = Some(now_ms);
                                    progress.last_checkpoint_unix_ms = Some(now_ms);
                                    progress.checkpoint_summary = Some("requeued: deep-mode turn ended without a complete_node artifact".to_string());
                                    progress.stale_since_unix_ms = None;
                                    progress.checkpoint_count = Some(progress.checkpoint_count.unwrap_or(0) + 1);
                                    plan.version += 1;
                                }
                                TurnEndDisposition::FailNoArtifact => {
                                    item.status = "failed".to_string();
                                    let progress = plan.task_progress.entry(task_id.clone()).or_default();
                                    progress.last_heartbeat_unix_ms = Some(now_ms);
                                    progress.last_checkpoint_unix_ms = Some(now_ms);
                                    progress.checkpoint_summary = Some("failed: repeated deep-mode turns ended without a complete_node artifact".to_string());
                                    progress.completed_at_unix_ms = Some(now_ms);
                                    progress.stale_since_unix_ms = None;
                                    progress.checkpoint_count = Some(progress.checkpoint_count.unwrap_or(0) + 1);
                                    plan.version += 1;
                                }
                                TurnEndDisposition::LeaveAlone => {}
                            }
                        }
                    }
                    persist_swarm_state_for(&swarm_id, &swarm_state).await;
                    let reason = match applied_disposition {
                        TurnEndDisposition::RequeueNoArtifact => "task_requeued_no_artifact",
                        TurnEndDisposition::FailNoArtifact => "task_failed_no_artifact",
                        _ => "task_completed",
                    };
                    broadcast_swarm_plan_with_previous(&swarm_id, Some(reason.to_string()),
                        Some(&previous_items), &swarm_plans, &swarm_members, &swarms_by_id).await;
                    update_member_status_with_report(&target_session, "completed", None, completion_report,
                        &swarm_members, &swarms_by_id, Some(&event_history), Some(&event_counter), Some(&swarm_event_tx)).await;
                }
                Err(error) => {
                    {
                        let now_ms = now_unix_ms();
                        let mut plans = swarm_plans.write().await;
                        if let Some(plan) = plans.get_mut(&swarm_id)
                            && let Some(item) = plan.items.iter_mut().find(|item| item.id == task_id)
                            && item.assigned_to.as_deref() == Some(target_session.as_str())
                            && matches!(item.status.as_str(), "running" | "running_stale") {
                            item.status = "failed".to_string();
                            let progress = plan.task_progress.entry(task_id.clone()).or_default();
                            progress.last_heartbeat_unix_ms = Some(now_ms);
                            progress.last_checkpoint_unix_ms = Some(now_ms);
                            progress.checkpoint_summary = Some(truncate_detail(&format!("task failed: {}", error), 120));
                            progress.completed_at_unix_ms = Some(now_ms);
                            progress.stale_since_unix_ms = None;
                            progress.checkpoint_count = Some(progress.checkpoint_count.unwrap_or(0) + 1);
                            plan.version += 1;
                        }
                    }
                    persist_swarm_state_for(&swarm_id, &swarm_state).await;
                    broadcast_swarm_plan(&swarm_id, Some("task_failed".to_string()),
                        &swarm_plans, &swarm_members, &swarms_by_id).await;
                    update_member_status(&target_session, "failed", Some(truncate_detail(&error.to_string(), 120)),
                        &swarm_members, &swarms_by_id, Some(&event_history), Some(&event_counter), Some(&swarm_event_tx)).await;
                }
            }
        }).await;
    })
}
