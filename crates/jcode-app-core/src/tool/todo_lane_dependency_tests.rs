use super::*;

fn item(id: &str, phase: u32, lane: &str) -> Value {
    json!({"id": id, "content": id, "status": "pending", "priority": "high", "stage": phase, "group": lane})
}

fn parsed(value: Value) -> Vec<TodoItem> {
    parse_todo_input(value).unwrap().todos.unwrap()
}

#[test]
fn hierarchical_after_is_copied_to_every_member() {
    let todos = parsed(json!({"phases": [
        {"phase": 9, "lanes": [{"lane": "base", "items": [{"id": "a", "content": "A", "status": "pending", "priority": "high"}]}]},
        {"phase": 2, "lanes": [{"lane": "next", "after": ["a"], "items": [
            {"id": "b", "content": "B", "status": "pending", "priority": "high"},
            {"id": "c", "content": "C", "status": "pending", "priority": "high"}
        ]}]}
    ]}));
    assert_eq!(todos[0].lane_after, None);
    assert_eq!(todos[1].lane_after, Some(vec!["a".into()]));
    assert_eq!(todos[1].lane_after, todos[2].lane_after);
    crate::todo::stages::validate_lane_dependencies(&todos).unwrap();
}

#[test]
fn incremental_edits_apply_to_whole_lane_and_new_members_inherit() {
    let stored = json!([
        item("a", 1, "base"),
        item("b", 2, "next"),
        item("c", 2, "next")
    ]);
    let updated = apply_todo_patch(
        json!({"update": [{"id": "b", "lane_after": ["a"]}]}),
        || stored,
    )
    .unwrap();
    let todos = parsed(updated.clone());
    assert_eq!(todos[1].lane_after, Some(vec!["a".into()]));
    assert_eq!(todos[1].lane_after, todos[2].lane_after);
    let mut added = item("d", 2, "next");
    added["after"] = json!("b");
    let updated = apply_todo_patch(json!({"add": [added]}), || updated["todos"].clone()).unwrap();
    let todos = parsed(updated.clone());
    assert!(
        todos
            .iter()
            .skip(1)
            .all(|todo| todo.lane_after == Some(vec!["a".into()]))
    );
    let cleared = apply_todo_patch(json!({"update": [{"id": "c", "lane_after": null}]}), || {
        updated["todos"].clone()
    })
    .unwrap();
    assert!(parsed(cleared).iter().all(|todo| todo.lane_after.is_none()));
}

#[test]
fn public_schema_exposes_lane_dependencies_only_at_correct_levels() {
    let schema = TodoTool.parameters_schema();
    assert!(schema["properties"]["update"]["items"]["properties"]["lane_dependencies"].is_object());
    assert!(schema["properties"]["add"]["items"]["properties"]["lane_dependencies"].is_object());
    assert!(
        schema["properties"]["update"]["items"]["properties"]
            .get("lane_after")
            .is_none()
    );
    assert!(schema["properties"]["add"]["items"]["properties"]["after"].is_object());
    let lane = &schema["properties"]["phases"]["items"]["properties"]["lanes"]["items"];
    assert!(lane["properties"]["dependencies"].is_object());
    assert!(lane["properties"].get("after").is_none());
    assert!(
        lane["properties"]["items"]["items"]["properties"]
            .get("lane_dependencies")
            .is_none()
    );
}

#[test]
fn named_dependencies_roundtrip_and_lane_updates() {
    let plan = apply_todo_patch(
        json!({"phases": [
            {"phase": 1, "lanes": [{"lane": "base", "items": [
                {"name": "Build", "status": "pending", "priority": "high"}
            ]}]},
            {"phase": 2, "lanes": [{"lane": "checks", "dependencies": ["Build"], "items": [
                {"name": "Test", "status": "pending", "priority": "high"},
                {"name": "Lint", "status": "pending", "priority": "high"}
            ]}]}
        ]}),
        || json!([]),
    )
    .unwrap();
    let todos = parsed(plan.clone());
    assert_eq!(todos[1].lane_after, Some(vec![todos[0].id.clone()]));
    assert_eq!(todos[1].lane_after, todos[2].lane_after);
    let public = names::public_todos(&todos).unwrap();
    assert_eq!(public[1]["lane_dependencies"], json!(["Build"]));
    assert!(public[1].get("lane_after").is_none());
    let cleared = apply_todo_patch(
        json!({"update": [
            {"name": "Test", "lane_dependencies": null}
        ]}),
        || plan["todos"].clone(),
    )
    .unwrap();
    assert!(
        parsed(cleared.clone())
            .iter()
            .all(|todo| todo.lane_after.is_none())
    );
    let restored = apply_todo_patch(
        json!({"update": [
            {"name": "Lint", "lane_dependencies": ["Build"]}
        ]}),
        || cleared["todos"].clone(),
    )
    .unwrap();
    assert_eq!(parsed(restored)[1].lane_after, todos[1].lane_after);
}

#[test]
fn conflicting_dependency_aliases_are_rejected() {
    let result = apply_todo_patch(
        json!({"phases": [{"phase": 1, "lanes": [
            {"lane": "base", "dependencies": null, "after": null, "items": []}
        ]}]}),
        || json!([]),
    );
    assert!(result.unwrap_err().to_string().contains("not both"));
    let result = apply_todo_patch(
        json!({"update": [
            {"name": "Test", "lane_dependencies": null, "lane_after": null}
        ]}),
        || json!([]),
    );
    assert!(result.unwrap_err().to_string().contains("not both"));
}
