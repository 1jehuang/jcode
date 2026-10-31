use super::*;

fn item(id: &str, phase: u32, group: &str, after: Option<&[&str]>) -> TodoItem {
    TodoItem {
        id: id.into(),
        content: id.into(),
        status: "pending".into(),
        stage: Some(phase),
        group: Some(group.into()),
        lane_after: after.map(|ids| ids.iter().map(|id| (*id).into()).collect()),
        ..Default::default()
    }
}

#[test]
fn validates_by_first_appearance_not_numeric_phase() {
    let todos = vec![item("a", 9, "a", None), item("b", 2, "b", Some(&["a"]))];
    assert!(validate_lane_dependencies(&todos).is_ok());
    let mut invalid = todos.clone();
    invalid[0].lane_after = Some(vec!["b".into()]);
    assert!(validate_lane_dependencies(&invalid).is_err());
}

#[test]
fn rejects_empty_duplicate_missing_same_phase_and_inconsistent_edges() {
    for after in [
        vec![],
        vec!["a", "a"],
        vec!["missing"],
        vec!["b"],
        vec!["c"],
        vec![""],
    ] {
        let todos = vec![
            item("a", 1, "a", None),
            item("b", 2, "b", Some(&after)),
            item("c", 2, "c", None),
        ];
        assert!(validate_lane_dependencies(&todos).is_err(), "{after:?}");
    }
    let todos = vec![
        item("a", 1, "a", None),
        item("b", 2, "b", Some(&["a"])),
        item("c", 2, "b", None),
    ];
    assert!(validate_lane_dependencies(&todos).is_err());
}

#[test]
fn persistence_defaults_and_round_trips() {
    let legacy = item("a", 1, "a", None);
    let encoded = serde_json::to_value(&legacy).unwrap();
    assert!(encoded.get("lane_after").is_none());
    assert_eq!(
        serde_json::from_value::<TodoItem>(encoded)
            .unwrap()
            .lane_after,
        None
    );
    let explicit = item("b", 2, "b", Some(&["a"]));
    assert_eq!(
        serde_json::from_value::<TodoItem>(serde_json::to_value(&explicit).unwrap()).unwrap(),
        explicit
    );
}

#[test]
fn worker_cannot_overwrite_coordinator_prerequisites() {
    let coordinator = vec![item("a", 1, "a", None), item("b", 2, "b", Some(&["a"]))];
    let link = LaneLink {
        coordinator: "boss".into(),
        worker: "worker".into(),
        lane: lane_id(&coordinator[1]),
    };
    let mut worker = lane_todos_for_worker(&coordinator, &link.lane);
    assert_eq!(worker[0].lane_after, Some(vec!["a".into()]));
    worker[0].lane_after = Some(vec!["wrong".into()]);
    worker.push(item("new", 7, "elsewhere", None));
    let canonical = canonicalize_worker_lane(&coordinator, &link, &worker);
    assert!(
        canonical
            .iter()
            .all(|todo| todo.lane_after == Some(vec!["a".into()]))
    );
    let merged = merge_worker_lane(&coordinator, &link, &worker).unwrap();
    assert!(
        merged
            .iter()
            .skip(1)
            .all(|todo| todo.lane_after == Some(vec!["a".into()]))
    );
    validate_lane_dependencies(&merged).unwrap();
}

#[test]
fn worker_validation_uses_full_graph_and_rejects_dangling_downstream_edges() {
    use crate::store::{TodoFile, todo_file_path, validate_session_lane_dependencies};
    let dir = std::env::temp_dir().join(format!("todo-lane-after-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("todos")).unwrap();
    let coordinator = vec![
        item("a", 1, "a", None),
        item("b", 2, "b", Some(&["a"])),
        item("c", 3, "c", Some(&["b"])),
    ];
    let link = LaneLink {
        coordinator: "boss".into(),
        worker: "worker".into(),
        lane: lane_id(&coordinator[1]),
    };
    std::fs::write(
        todo_file_path(&dir, "boss", TodoFile::Todos),
        serde_json::to_vec(&coordinator).unwrap(),
    )
    .unwrap();
    std::fs::write(
        todo_file_path(&dir, "worker", TodoFile::LaneLinks),
        serde_json::to_vec(&vec![link.clone()]).unwrap(),
    )
    .unwrap();
    let mut worker = lane_todos_for_worker(&coordinator, &link.lane);
    assert!(
        validate_lane_dependencies(&worker).is_err(),
        "worker snapshot omits a"
    );
    assert!(validate_session_lane_dependencies(&dir, "worker", &worker).is_ok());
    worker[0].lane_after = Some(vec!["ignored-worker-config".into()]);
    assert!(validate_session_lane_dependencies(&dir, "worker", &worker).is_ok());
    assert!(
        validate_session_lane_dependencies(&dir, "worker", &[]).is_err(),
        "c still depends on b"
    );
    std::fs::remove_dir_all(dir).unwrap();
}
