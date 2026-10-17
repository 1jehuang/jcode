use crate::plan::PlanItem;
use crate::protocol::PlanGraphStatus;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct RemoteSwarmPlanSnapshot {
    pub swarm_id: String,
    pub version: u64,
    pub items: Vec<PlanItem>,
    pub participants: Vec<String>,
    pub reason: Option<String>,
    pub summary: Option<PlanGraphStatus>,
}

impl RemoteSwarmPlanSnapshot {
    pub fn status_notice(&self) -> String {
        // The graph summary is authoritative for how many items the plan holds:
        // `items` can lag or be trimmed in transit, so prefer `summary.item_count`
        // whenever a summary is present.
        let item_count = self
            .summary
            .as_ref()
            .map(|summary| summary.item_count)
            .unwrap_or_else(|| self.items.len());
        let mut notice = format!(
            "Swarm plan synced (v{}, {} items)",
            self.version, item_count
        );
        if let Some(summary) = &self.summary {
            // Task-DAG progress breakdown: how the graph currently partitions by
            // scheduling state. Only show segments that are non-empty so the line
            // stays compact.
            let mut segments = Vec::new();
            if !summary.completed_ids.is_empty() {
                segments.push(format!("{} done", summary.completed_ids.len()));
            }
            if !summary.active_ids.is_empty() {
                segments.push(format!("{} running", summary.active_ids.len()));
            }
            if !summary.ready_ids.is_empty() {
                segments.push(format!("{} ready", summary.ready_ids.len()));
            }
            if !summary.blocked_ids.is_empty() {
                segments.push(format!("{} blocked", summary.blocked_ids.len()));
            }
            if !summary.cycle_ids.is_empty() {
                segments.push(format!("{} in cycle", summary.cycle_ids.len()));
            }
            if !summary.unresolved_dependency_ids.is_empty() {
                segments.push(format!(
                    "{} unresolved deps",
                    summary.unresolved_dependency_ids.len()
                ));
            }
            if !segments.is_empty() {
                notice.push_str(&format!(" · graph: {}", segments.join(", ")));
            }
            if !summary.next_ready_ids.is_empty() {
                notice.push_str(&format!(" · next: {}", summary.next_ready_ids.join(", ")));
            }
            if !summary.newly_ready_ids.is_empty() {
                notice.push_str(&format!(
                    " · newly ready: {}",
                    summary.newly_ready_ids.join(", ")
                ));
            }
        }
        notice
    }
}

#[cfg(test)]
mod tests {
    use super::RemoteSwarmPlanSnapshot;
    use crate::plan::PlanItem;
    use crate::protocol::PlanGraphStatus;

    fn plan_item(id: &str, status: &str) -> PlanItem {
        PlanItem {
            content: format!("task {id}"),
            status: status.to_string(),
            priority: "normal".to_string(),
            id: id.to_string(),
            subsystem: None,
            file_scope: Vec::new(),
            blocked_by: Vec::new(),
            assigned_to: None,
        }
    }

    fn snapshot(items: Vec<PlanItem>, summary: Option<PlanGraphStatus>) -> RemoteSwarmPlanSnapshot {
        let version = summary.as_ref().map(|s| s.version).unwrap_or(0);
        RemoteSwarmPlanSnapshot {
            swarm_id: "swarm-a".to_string(),
            version,
            items,
            participants: Vec::new(),
            reason: None,
            summary,
        }
    }

    fn summary_fixture() -> PlanGraphStatus {
        PlanGraphStatus {
            swarm_id: Some("swarm-a".to_string()),
            version: 5,
            item_count: 4,
            ready_ids: vec!["task-2".to_string()],
            blocked_ids: vec!["task-4".to_string()],
            active_ids: Vec::new(),
            completed_ids: vec!["task-1".to_string()],
            failed_ids: Vec::new(),
            failed_reasons: Default::default(),
            cycle_ids: Vec::new(),
            unresolved_dependency_ids: Vec::new(),
            next_ready_ids: vec!["task-2".to_string()],
            newly_ready_ids: vec!["task-3".to_string()],
        }
    }

    fn fixture_items() -> Vec<PlanItem> {
        vec![
            plan_item("task-1", "completed"),
            plan_item("task-2", "pending"),
            plan_item("task-3", "pending"),
            plan_item("task-4", "pending"),
        ]
    }

    #[test]
    fn swarm_plan_status_notice_includes_graph_hints() {
        let notice = snapshot(fixture_items(), Some(summary_fixture())).status_notice();
        assert!(notice.contains("v5"));
        assert!(notice.contains("4 items"));
        assert!(notice.contains("graph: 1 done, 1 ready, 1 blocked"));
        assert!(notice.contains("next: task-2"));
        assert!(notice.contains("newly ready: task-3"));
        assert!(!notice.contains("low-conf"));
    }

    #[test]
    fn swarm_plan_status_notice_prefers_summary_item_count() {
        // items can lag or be trimmed relative to the graph summary; the
        // summary's item_count is authoritative when present.
        let notice = snapshot(Vec::new(), Some(summary_fixture())).status_notice();
        assert!(notice.contains("(v5, 4 items)"), "notice: {notice}");
    }

    #[test]
    fn swarm_plan_status_notice_empty_plan_without_summary() {
        let notice = snapshot(Vec::new(), None).status_notice();
        assert_eq!(notice, "Swarm plan synced (v0, 0 items)");
    }

    #[test]
    fn swarm_plan_status_notice_empty_summary_has_no_graph_suffix() {
        let summary = PlanGraphStatus::empty_for_swarm("swarm-a");
        let notice = snapshot(Vec::new(), Some(summary)).status_notice();
        assert_eq!(notice, "Swarm plan synced (v0, 0 items)");
    }

    #[test]
    fn swarm_plan_status_notice_all_done_shows_only_done_segment() {
        let mut summary = summary_fixture();
        summary.ready_ids = Vec::new();
        summary.blocked_ids = Vec::new();
        summary.next_ready_ids = Vec::new();
        summary.newly_ready_ids = Vec::new();
        summary.completed_ids = vec![
            "task-1".to_string(),
            "task-2".to_string(),
            "task-3".to_string(),
            "task-4".to_string(),
        ];
        let items = fixture_items()
            .into_iter()
            .map(|mut item| {
                item.status = "completed".to_string();
                item
            })
            .collect();
        let notice = snapshot(items, Some(summary)).status_notice();
        assert_eq!(notice, "Swarm plan synced (v5, 4 items) · graph: 4 done");
    }

    #[test]
    fn swarm_plan_status_notice_reports_cycles() {
        let mut summary = summary_fixture();
        summary.ready_ids = Vec::new();
        summary.next_ready_ids = Vec::new();
        summary.newly_ready_ids = Vec::new();
        summary.cycle_ids = vec!["task-2".to_string(), "task-4".to_string()];
        let notice = snapshot(fixture_items(), Some(summary)).status_notice();
        assert!(notice.contains("2 in cycle"), "notice: {notice}");
    }

    #[test]
    fn swarm_plan_status_notice_reports_unresolved_dependencies() {
        let mut summary = summary_fixture();
        summary.unresolved_dependency_ids = vec!["task-9".to_string()];
        let notice = snapshot(fixture_items(), Some(summary)).status_notice();
        assert!(notice.contains("1 unresolved deps"), "notice: {notice}");
    }
}
