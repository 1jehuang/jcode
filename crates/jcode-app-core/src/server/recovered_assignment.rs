//! Reconnect a recovered turn to its original durable plan assignment without
//! replaying an assignment prompt or taking ownership of a newer assignment.

use super::comm_control::{TurnEndDisposition, turn_end_disposition};
use super::swarm::now_unix_ms;
use super::usage_limit_resume::UsageLimitResumePlan;
use super::{SwarmState, broadcast_swarm_plan_with_previous, persist_swarm_state_for, truncate_detail};

#[derive(Clone)]
pub(super) struct RecoveredAssignment {
    swarm: SwarmState,
    swarm_id: String,
    task_id: String,
    session_id: String,
}

impl RecoveredAssignment {
    /// Only a unique in-flight assignment belongs to the recovered turn. In
    /// ambiguous historical state, decline rather than complete an arbitrary node.
    pub(super) async fn capture(session_id: &str, swarm: &SwarmState) -> Option<Self> {
        let plans = swarm.plans.read().await;
        let mut matches = plans.iter().flat_map(|(swarm_id, plan)| {
            plan.items.iter().filter(move |item| {
                item.assigned_to.as_deref() == Some(session_id)
                    && matches!(item.status.as_str(), "running" | "running_stale")
            }).map(move |item| (swarm_id.clone(), item.id.clone()))
        });
        let (swarm_id, task_id) = matches.next()?;
        if matches.next().is_some() {
            crate::logging::warn(&format!("Multiple active plan assignments for recovered session {session_id}; refusing an ambiguous completion hook"));
            return None;
        }
        Some(Self { swarm: swarm.clone(), swarm_id, task_id, session_id: session_id.to_string() })
    }

    pub(super) async fn on_wait(&self, resume: &UsageLimitResumePlan) {
        self.record_wait(&resume.status_detail()).await;
    }

    async fn record_wait(&self, detail: &str) {
        let previous = {
            let mut plans = self.swarm.plans.write().await;
            let Some(plan) = plans.get_mut(&self.swarm_id) else { return; };
            let previous = plan.items.clone();
            let Some(item) = plan.items.iter_mut().find(|item| item.id == self.task_id) else { return; };
            // Check ownership and lifecycle in the SAME critical section as
            // the write. touch_swarm_task_progress alone has no such guard.
            if item.assigned_to.as_deref() != Some(self.session_id.as_str())
                || !matches!(item.status.as_str(), "running" | "running_stale") { return; }
            item.status = "running".to_string();
            let now = now_unix_ms();
            let progress = plan.task_progress.entry(self.task_id.clone()).or_default();
            progress.assigned_session_id = Some(self.session_id.clone());
            progress.last_heartbeat_unix_ms = Some(now);
            progress.heartbeat_count = Some(progress.heartbeat_count.unwrap_or(0) + 1);
            progress.last_checkpoint_unix_ms = Some(now);
            progress.checkpoint_count = Some(progress.checkpoint_count.unwrap_or(0) + 1);
            progress.last_detail = Some(truncate_detail(detail, 120));
            progress.checkpoint_summary = Some(truncate_detail(detail, 120));
            progress.stale_since_unix_ms = None;
            plan.version += 1;
            previous
        };
        crate::session_metrics::record_activity(&self.session_id);
        self.publish("task_rate_limited", &previous).await;
    }

    /// Called inside the turn publication's terminal critical section, while
    /// the recovered agent reservation still excludes successor turns.
    pub(super) async fn complete(&self, success: bool, error: Option<&str>) {
        let (previous, reason) = {
            let mut plans = self.swarm.plans.write().await;
            let Some(plan) = plans.get_mut(&self.swarm_id) else { return; };
            let previous = plan.items.clone();
            let Some(item) = plan.items.iter_mut().find(|item| item.id == self.task_id) else { return; };
            if item.assigned_to.as_deref() != Some(self.session_id.as_str())
                || !matches!(item.status.as_str(), "running" | "running_stale") { return; }
            let expanded = plan.node_meta.get(&self.task_id)
                .is_some_and(|meta| meta.expanded && !meta.is_gate);
            let prior = plan.task_progress.get(&self.task_id)
                .and_then(|progress| progress.no_artifact_requeues).unwrap_or(0);
            let disposition = if success {
                turn_end_disposition(plan.mode.eq_ignore_ascii_case("deep"), &item.status, expanded, prior)
            } else { TurnEndDisposition::FailNoArtifact };
            if disposition == TurnEndDisposition::LeaveAlone { return; }
            let progress = plan.task_progress.entry(self.task_id.clone()).or_default();
            let (reason, summary, terminal) = if !success {
                item.status = "failed".to_string();
                ("task_failed", format!("task failed: {}", error.unwrap_or("recovered turn failed")), true)
            } else {
                match disposition {
                    TurnEndDisposition::AutoComplete => {
                        item.status = "done".to_string();
                        ("task_completed", "task completed".to_string(), true)
                    }
                    TurnEndDisposition::RequeueNoArtifact => {
                        item.status = "queued".to_string();
                        item.assigned_to = None;
                        progress.assigned_session_id = None;
                        progress.no_artifact_requeues = Some(prior + 1);
                        ("task_requeued_no_artifact", "requeued: deep-mode turn ended without a complete_node artifact".to_string(), false)
                    }
                    TurnEndDisposition::FailNoArtifact => {
                        item.status = "failed".to_string();
                        ("task_failed_no_artifact", "failed: repeated deep-mode turns ended without a complete_node artifact".to_string(), true)
                    }
                    TurnEndDisposition::LeaveAlone => unreachable!(),
                }
            };
            let now = now_unix_ms();
            progress.last_heartbeat_unix_ms = Some(now);
            progress.last_checkpoint_unix_ms = Some(now);
            progress.checkpoint_summary = Some(truncate_detail(&summary, 120));
            progress.stale_since_unix_ms = None;
            progress.checkpoint_count = Some(progress.checkpoint_count.unwrap_or(0) + 1);
            if terminal { progress.completed_at_unix_ms = Some(now); }
            plan.version += 1;
            (previous, reason)
        };
        self.publish(reason, &previous).await;
    }

    async fn publish(&self, reason: &str, previous: &[crate::plan::PlanItem]) {
        persist_swarm_state_for(&self.swarm_id, &self.swarm).await;
        broadcast_swarm_plan_with_previous(&self.swarm_id, Some(reason.to_string()), Some(previous),
            &self.swarm.plans, &self.swarm.members, &self.swarm.swarms_by_id).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::{NodeMeta, PlanItem, VersionedPlan};
    use std::collections::HashMap;

    struct Home { _temp: tempfile::TempDir, previous: Option<std::ffi::OsString> }
    impl Home {
        fn new() -> Self {
            let temp = tempfile::TempDir::new().unwrap();
            let previous = std::env::var_os("JCODE_HOME");
            crate::env::set_var("JCODE_HOME", temp.path());
            Self { _temp: temp, previous }
        }
    }
    impl Drop for Home {
        fn drop(&mut self) {
            match &self.previous {
                Some(value) => crate::env::set_var("JCODE_HOME", value),
                None => crate::env::remove_var("JCODE_HOME"),
            }
        }
    }
    fn fixture(mode: &str) -> SwarmState {
        let mut plan = VersionedPlan::new();
        plan.mode = mode.to_string();
        plan.items.push(PlanItem {
            id: "task".to_string(), content: "original task".to_string(),
            status: "running_stale".to_string(), priority: "high".to_string(),
            assigned_to: Some("worker".to_string()), subsystem: None,
            file_scope: vec![], blocked_by: vec![],
        });
        SwarmState::new(HashMap::new(), HashMap::new(),
            HashMap::from([("swarm".to_string(), plan)]), HashMap::new())
    }

    #[tokio::test]
    async fn recovered_light_assignment_waits_then_completes_once() {
        let _lock = crate::storage::lock_test_env();
        let _home = Home::new();
        let swarm = fixture("light");
        let recovered = RecoveredAssignment::capture("worker", &swarm).await.unwrap();
        recovered.record_wait("usage limit, resuming later").await;
        {
            let plans = swarm.plans.read().await;
            assert_eq!(plans["swarm"].items[0].status, "running");
            assert_eq!(plans["swarm"].task_progress["task"].assigned_session_id.as_deref(), Some("worker"));
            assert!(plans["swarm"].task_progress["task"].completed_at_unix_ms.is_none());
        }
        recovered.complete(true, None).await;
        let version = swarm.plans.read().await["swarm"].version;
        recovered.complete(true, None).await;
        let plans = swarm.plans.read().await;
        assert_eq!(plans["swarm"].items[0].status, "done");
        assert_eq!(plans["swarm"].version, version, "duplicate publication is inert");
        assert!(plans["swarm"].task_progress["task"].completed_at_unix_ms.is_some());
    }

    #[tokio::test]
    async fn recovered_assignment_never_clobbers_reassignment_or_terminal_node() {
        let _lock = crate::storage::lock_test_env();
        let _home = Home::new();
        let swarm = fixture("light");
        let recovered = RecoveredAssignment::capture("worker", &swarm).await.unwrap();
        swarm.plans.write().await.get_mut("swarm").unwrap().items[0].assigned_to = Some("successor".to_string());
        recovered.record_wait("stale quota timer").await;
        recovered.complete(false, Some("old failure")).await;
        {
            let plans = swarm.plans.read().await;
            assert_eq!(plans["swarm"].items[0].status, "running_stale");
            assert!(plans["swarm"].task_progress.is_empty());
        }
        {
            let mut plans = swarm.plans.write().await;
            let item = &mut plans.get_mut("swarm").unwrap().items[0];
            item.assigned_to = Some("worker".to_string());
            item.status = "done".to_string();
        }
        recovered.record_wait("late retry").await;
        recovered.complete(false, Some("late failure")).await;
        assert_eq!(swarm.plans.read().await["swarm"].items[0].status, "done");
    }

    #[tokio::test]
    async fn recovered_deep_assignment_requires_artifact_and_preserves_composite_rules() {
        let _lock = crate::storage::lock_test_env();
        let _home = Home::new();
        let swarm = fixture("deep");
        let recovered = RecoveredAssignment::capture("worker", &swarm).await.unwrap();
        recovered.complete(true, None).await;
        {
            let plans = swarm.plans.read().await;
            assert_eq!(plans["swarm"].items[0].status, "queued");
            assert!(plans["swarm"].items[0].assigned_to.is_none());
            assert_eq!(plans["swarm"].task_progress["task"].no_artifact_requeues, Some(1));
        }
        {
            let mut plans = swarm.plans.write().await;
            let item = &mut plans.get_mut("swarm").unwrap().items[0];
            item.status = "running".to_string();
            item.assigned_to = Some("worker".to_string());
        }
        let retry = RecoveredAssignment::capture("worker", &swarm).await.unwrap();
        retry.complete(true, None).await;
        assert_eq!(swarm.plans.read().await["swarm"].items[0].status, "failed");
        let light = fixture("light");
        light.plans.write().await.get_mut("swarm").unwrap().node_meta.insert("task".to_string(), NodeMeta { expanded: true, ..Default::default() });
        let composite = RecoveredAssignment::capture("worker", &light).await.unwrap();
        composite.complete(true, None).await;
        assert_eq!(light.plans.read().await["swarm"].items[0].status, "running_stale");
    }

    #[tokio::test]
    async fn recovered_capture_rejects_queued_terminal_and_ambiguous_assignments() {
        let swarm = fixture("light");
        assert!(RecoveredAssignment::capture("other-worker", &swarm).await.is_none());
        {
            let mut plans = swarm.plans.write().await;
            plans.get_mut("swarm").unwrap().items[0].status = "queued".to_string();
        }
        assert!(RecoveredAssignment::capture("worker", &swarm).await.is_none());
        {
            let mut plans = swarm.plans.write().await;
            let plan = plans.get_mut("swarm").unwrap();
            plan.items[0].status = "running".to_string();
            let mut duplicate = plan.items[0].clone();
            duplicate.id = "other-task".to_string();
            plan.items.push(duplicate);
        }
        assert!(RecoveredAssignment::capture("worker", &swarm).await.is_none());
    }

    #[tokio::test]
    async fn recovered_failure_marks_only_owned_inflight_assignment_failed() {
        let _lock = crate::storage::lock_test_env();
        let _home = Home::new();
        let swarm = fixture("light");
        let recovered = RecoveredAssignment::capture("worker", &swarm).await.unwrap();
        recovered.complete(false, Some("provider failed permanently")).await;
        let plans = swarm.plans.read().await;
        assert_eq!(plans["swarm"].items[0].status, "failed");
        assert!(plans["swarm"].task_progress["task"].checkpoint_summary.as_deref().unwrap().contains("provider failed permanently"));
    }
}
