use super::dispatchable_lanes;
use crate::todo::TodoItem;

fn item(id: &str, stage: u32, lane: &str, owner: Option<&str>) -> TodoItem {
    TodoItem {
        id: id.to_string(),
        content: format!("task {id}"),
        status: "pending".to_string(),
        priority: "high".to_string(),
        stage: Some(stage),
        group: Some(lane.to_string()),
        lane_owner: owner.map(str::to_string),
        ..Default::default()
    }
}

#[test]
fn self_owned_lanes_never_dispatch() {
    let todos = vec![item("a", 1, "api", None), item("b", 1, "docs", None)];
    assert!(dispatchable_lanes(&todos).is_empty());
}

#[test]
fn ready_worker_lanes_in_one_phase_start_together() {
    let todos = vec![
        item("a", 1, "api", Some("worker")),
        item("b", 1, "docs", Some("worker")),
        item("c", 1, "mine", None),
    ];
    let lanes = dispatchable_lanes(&todos);
    assert_eq!(lanes.len(), 2, "{lanes:?}");
}

#[test]
fn later_phase_waits_then_unlocks_on_completion() {
    let mut todos = vec![
        item("a", 1, "api", None),
        item("b", 2, "client", Some("worker")),
    ];
    assert!(dispatchable_lanes(&todos).is_empty());
    todos[0].status = "completed".to_string();
    assert_eq!(dispatchable_lanes(&todos).len(), 1);
}

#[test]
fn explicit_dependency_unlocks_lane_early() {
    let mut consumer = item("c", 2, "client", Some("worker"));
    consumer.lane_after = Some(vec!["a".into()]);
    let mut todos = vec![
        item("a", 1, "api", None),
        item("b", 1, "api", None),
        consumer,
    ];
    assert!(dispatchable_lanes(&todos).is_empty());
    todos[0].status = "completed".to_string();
    assert_eq!(dispatchable_lanes(&todos).len(), 1);
}

#[test]
fn owned_lane_is_not_dispatched_again() {
    let mut todos = vec![
        item("a", 1, "api", Some("worker")),
        item("b", 1, "api", Some("worker")),
    ];
    assert_eq!(dispatchable_lanes(&todos).len(), 1);
    todos[0].assigned_to = Some("worker-1".into());
    todos[1].assigned_to = Some("worker-1".into());
    // Plan edits and reloads re-evaluate the saved list; the persisted owner
    // keeps the lane from getting a second worker.
    todos.push({
        let mut extra = item("x", 1, "api", Some("worker"));
        extra.assigned_to = Some("worker-1".into());
        extra
    });
    assert!(dispatchable_lanes(&todos).is_empty());
}

#[test]
fn completed_lane_is_not_dispatched() {
    let mut done = item("a", 1, "api", Some("worker"));
    done.status = "completed".into();
    assert!(dispatchable_lanes(&[done]).is_empty());
}

#[test]
fn normalize_makes_owner_lane_wide() {
    let mut todos = vec![
        item("a", 1, "api", Some("worker")),
        item("b", 1, "api", None),
        item("c", 1, "docs", Some("self")),
    ];
    crate::todo::stages::normalize_lane_owners(&mut todos);
    assert_eq!(todos[1].lane_owner.as_deref(), Some("worker"));
    assert_eq!(todos[2].lane_owner, None);
}
