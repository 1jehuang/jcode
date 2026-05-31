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
        let mut notice = format!(
            "Swarm plan synced (v{}, {} items)",
            self.version,
            self.items.len()
        );
        if let Some(summary) = &self.summary {
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

    #[test]
    fn swarm_plan_status_notice_includes_graph_hints() {
        let snapshot = RemoteSwarmPlanSnapshot {
            swarm_id: "swarm-a".to_string(),
            version: 5,
            items: Vec::new(),
            participants: Vec::new(),
            reason: None,
            summary: Some(crate::protocol::PlanGraphStatus {
                swarm_id: Some("swarm-a".to_string()),
                version: 5,
                item_count: 2,
                ready_ids: vec!["task-2".to_string()],
                blocked_ids: Vec::new(),
                active_ids: Vec::new(),
                completed_ids: Vec::new(),
                cycle_ids: Vec::new(),
                unresolved_dependency_ids: Vec::new(),
                next_ready_ids: vec!["task-2".to_string()],
                newly_ready_ids: vec!["task-3".to_string()],
            }),
        };

        let notice = snapshot.status_notice();
        assert!(notice.contains("v5"));
        assert!(notice.contains("next: task-2"));
        assert!(notice.contains("newly ready: task-3"));
    }
}
