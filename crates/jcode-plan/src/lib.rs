use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap, HashSet};

/// A swarm plan item.
///
/// This is intentionally separate from session todos: plan data is shared at the
/// server/swarm level, while todos remain session-local.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlanItem {
    pub content: String,
    pub status: String,
    pub priority: String,
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subsystem: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub file_scope: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blocked_by: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assigned_to: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlanGraphSummary {
    pub ready_ids: Vec<String>,
    pub blocked_ids: Vec<String>,
    pub active_ids: Vec<String>,
    pub completed_ids: Vec<String>,
    pub terminal_ids: Vec<String>,
    pub unresolved_dependency_ids: Vec<String>,
    pub cycle_ids: Vec<String>,
}

pub fn is_completed_status(status: &str) -> bool {
    matches!(status, "completed" | "done")
}

pub fn is_terminal_status(status: &str) -> bool {
    matches!(
        status,
        "completed" | "done" | "failed" | "stopped" | "crashed"
    )
}

pub fn is_active_status(status: &str) -> bool {
    matches!(status, "running" | "running_stale")
}

pub fn is_runnable_status(status: &str) -> bool {
    matches!(status, "queued" | "ready" | "pending" | "todo")
}

pub fn priority_rank(priority: &str) -> u8 {
    match priority {
        "high" | "urgent" | "p0" => 0,
        "medium" | "normal" | "p1" => 1,
        "low" | "p2" => 2,
        _ => 1,
    }
}

pub fn completed_item_ids(items: &[PlanItem]) -> HashSet<String> {
    items
        .iter()
        .filter(|item| is_completed_status(&item.status))
        .map(|item| item.id.clone())
        .collect()
}

pub fn unresolved_dependencies<'a>(
    item: &'a PlanItem,
    known_ids: &HashSet<&'a str>,
    completed_ids: &HashSet<&str>,
) -> Vec<String> {
    item.blocked_by
        .iter()
        .filter(|dep| known_ids.contains(dep.as_str()) && !completed_ids.contains(dep.as_str()))
        .cloned()
        .collect()
}

pub fn missing_dependencies<'a>(item: &'a PlanItem, known_ids: &HashSet<&'a str>) -> Vec<String> {
    item.blocked_by
        .iter()
        .filter(|dep| !known_ids.contains(dep.as_str()))
        .cloned()
        .collect()
}

pub fn is_unblocked<'a>(
    item: &'a PlanItem,
    known_ids: &HashSet<&'a str>,
    completed_ids: &HashSet<&str>,
) -> bool {
    missing_dependencies(item, known_ids).is_empty()
        && unresolved_dependencies(item, known_ids, completed_ids).is_empty()
}

pub fn cycle_item_ids(items: &[PlanItem]) -> Vec<String> {
    let item_ids: HashSet<&str> = items.iter().map(|item| item.id.as_str()).collect();
    let mut indegree: HashMap<&str, usize> = HashMap::new();
    let mut dependents: HashMap<&str, Vec<&str>> = HashMap::new();

    for item in items {
        indegree.entry(item.id.as_str()).or_insert(0);
    }

    for item in items {
        for dependency in item
            .blocked_by
            .iter()
            .filter(|dependency| item_ids.contains(dependency.as_str()))
        {
            *indegree.entry(item.id.as_str()).or_insert(0) += 1;
            dependents
                .entry(dependency.as_str())
                .or_default()
                .push(item.id.as_str());
        }
    }

    let mut queue: Vec<&str> = indegree
        .iter()
        .filter_map(|(id, degree)| (*degree == 0).then_some(*id))
        .collect();
    let mut visited = HashSet::new();

    while let Some(id) = queue.pop() {
        if !visited.insert(id) {
            continue;
        }
        if let Some(children) = dependents.get(id) {
            for child in children {
                if let Some(degree) = indegree.get_mut(child) {
                    *degree = degree.saturating_sub(1);
                    if *degree == 0 {
                        queue.push(child);
                    }
                }
            }
        }
    }

    let mut cycle_ids: Vec<String> = indegree
        .into_iter()
        .filter_map(|(id, degree)| (degree > 0 && !visited.contains(id)).then_some(id.to_string()))
        .collect();
    cycle_ids.sort();
    cycle_ids
}

pub fn summarize_plan_graph(items: &[PlanItem]) -> PlanGraphSummary {
    let known_ids: HashSet<&str> = items.iter().map(|item| item.id.as_str()).collect();
    let completed_ids = completed_item_ids(items);
    let completed_refs: HashSet<&str> = completed_ids.iter().map(String::as_str).collect();
    let cycle_ids = cycle_item_ids(items);
    let cycle_set: HashSet<&str> = cycle_ids.iter().map(String::as_str).collect();

    let mut ready_ids = Vec::new();
    let mut blocked_ids = Vec::new();
    let mut active_ids = Vec::new();
    let mut completed = BTreeSet::new();
    let mut terminal = BTreeSet::new();
    let mut unresolved = BTreeSet::new();

    for item in items {
        let missing = missing_dependencies(item, &known_ids);
        let unresolved_for_item = unresolved_dependencies(item, &known_ids, &completed_refs);
        let is_cyclic = cycle_set.contains(item.id.as_str());

        unresolved.extend(missing.iter().cloned());

        if is_active_status(&item.status) {
            active_ids.push(item.id.clone());
        }
        if is_completed_status(&item.status) {
            completed.insert(item.id.clone());
        }
        if is_terminal_status(&item.status) {
            terminal.insert(item.id.clone());
        }

        let has_dependency_blocker = !unresolved_for_item.is_empty() || is_cyclic;
        if is_runnable_status(&item.status) && missing.is_empty() && !has_dependency_blocker {
            ready_ids.push(item.id.clone());
        } else if !is_terminal_status(&item.status)
            && !is_active_status(&item.status)
            && (!missing.is_empty() || has_dependency_blocker || item.status == "blocked")
        {
            blocked_ids.push(item.id.clone());
        }
    }

    ready_ids.sort();
    blocked_ids.sort();
    active_ids.sort();

    PlanGraphSummary {
        ready_ids,
        blocked_ids,
        active_ids,
        completed_ids: completed.into_iter().collect(),
        terminal_ids: terminal.into_iter().collect(),
        unresolved_dependency_ids: unresolved.into_iter().collect(),
        cycle_ids,
    }
}

pub fn next_runnable_item_ids(items: &[PlanItem], limit: Option<usize>) -> Vec<String> {
    let ready_ids: HashSet<String> = summarize_plan_graph(items).ready_ids.into_iter().collect();
    let mut ready_items: Vec<&PlanItem> = items
        .iter()
        .filter(|item| ready_ids.contains(&item.id))
        .collect();

    ready_items.sort_by(|left, right| {
        priority_rank(&left.priority)
            .cmp(&priority_rank(&right.priority))
            .then_with(|| left.id.cmp(&right.id))
    });

    let iter = ready_items.into_iter().map(|item| item.id.clone());
    match limit {
        Some(limit) => iter.take(limit).collect(),
        None => iter.collect(),
    }
}

pub fn newly_ready_item_ids(before: &[PlanItem], after: &[PlanItem]) -> Vec<String> {
    let before_ready: HashSet<String> =
        summarize_plan_graph(before).ready_ids.into_iter().collect();
    let mut after_ready = summarize_plan_graph(after).ready_ids;
    after_ready.retain(|item_id| !before_ready.contains(item_id));
    after_ready
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: &str, status: &str, blocked_by: &[&str]) -> PlanItem {
        PlanItem {
            id: id.to_string(),
            content: id.to_string(),
            status: status.to_string(),
            priority: "high".to_string(),
            subsystem: None,
            file_scope: Vec::new(),
            blocked_by: blocked_by.iter().map(|value| value.to_string()).collect(),
            assigned_to: None,
        }
    }

    #[test]
    fn summarize_plan_graph_reports_ready_and_blocked_items() {
        let items = vec![
            item("a", "completed", &[]),
            item("b", "queued", &["a"]),
            item("c", "queued", &["b"]),
        ];

        let summary = summarize_plan_graph(&items);
        assert_eq!(summary.ready_ids, vec!["b".to_string()]);
        assert_eq!(summary.blocked_ids, vec!["c".to_string()]);
        assert_eq!(summary.completed_ids, vec!["a".to_string()]);
        assert_eq!(summary.cycle_ids, Vec::<String>::new());
    }

    #[test]
    fn summarize_plan_graph_reports_missing_dependencies() {
        let items = vec![
            item("a", "queued", &["missing-task"]),
            item("b", "running", &[]),
        ];

        let summary = summarize_plan_graph(&items);
        assert_eq!(summary.ready_ids, Vec::<String>::new());
        assert_eq!(summary.blocked_ids, vec!["a".to_string()]);
        assert_eq!(summary.active_ids, vec!["b".to_string()]);
        assert_eq!(
            summary.unresolved_dependency_ids,
            vec!["missing-task".to_string()]
        );
    }

    #[test]
    fn newly_ready_item_ids_reports_tasks_unblocked_by_completion() {
        let before = vec![
            item("setup", "running", &[]),
            item("follow-up", "queued", &["setup"]),
            item("later", "queued", &["follow-up"]),
        ];
        let after = vec![
            item("setup", "completed", &[]),
            item("follow-up", "queued", &["setup"]),
            item("later", "queued", &["follow-up"]),
        ];

        assert_eq!(newly_ready_item_ids(&before, &after), vec!["follow-up"]);
    }

    #[test]
    fn summarize_plan_graph_reports_cycles() {
        let items = vec![
            item("a", "queued", &["c"]),
            item("b", "queued", &["a"]),
            item("c", "queued", &["b"]),
        ];

        let summary = summarize_plan_graph(&items);
        assert_eq!(summary.ready_ids, Vec::<String>::new());
        assert_eq!(
            summary.blocked_ids,
            vec!["a".to_string(), "b".to_string(), "c".to_string()]
        );
        assert_eq!(
            summary.cycle_ids,
            vec!["a".to_string(), "b".to_string(), "c".to_string()]
        );
    }

    #[test]
    fn status_helpers_match_runtime_expectations() {
        assert!(is_completed_status("completed"));
        assert!(is_terminal_status("failed"));
        assert!(is_active_status("running_stale"));
        assert!(is_runnable_status("queued"));
        assert!(!is_terminal_status("queued"));
    }

    #[test]
    fn next_runnable_items_prefers_higher_priority() {
        let items = vec![
            item("done", "completed", &[]),
            item("b", "queued", &["done"]),
            PlanItem {
                priority: "low".to_string(),
                ..item("c", "queued", &["done"])
            },
            PlanItem {
                priority: "high".to_string(),
                ..item("a", "queued", &["done"])
            },
        ];

        assert_eq!(next_runnable_item_ids(&items, None), vec!["a", "b", "c"]);
        assert_eq!(next_runnable_item_ids(&items, Some(2)), vec!["a", "b"]);
    }
}
