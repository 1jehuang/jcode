use super::*;

#[test]
fn create_and_resume_goal_persists_project_goal() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("tempdir");
    let project = temp.path().join("repo");
    std::fs::create_dir_all(&project).expect("project dir");
    let prev_home = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", temp.path());

    let goal = create_goal(
        GoalCreateInput {
            title: "Ship mobile MVP".to_string(),
            scope: GoalScope::Project,
            next_steps: vec!["finish reconnect flow".to_string()],
            progress_percent: Some(40),
            ..GoalCreateInput::default()
        },
        Some(&project),
    )
    .expect("create goal");
    assert_eq!(goal.id, "ship-mobile-mvp");

    let loaded = load_goal(&goal.id, Some(GoalScope::Project), Some(&project))
        .expect("load")
        .expect("goal exists");
    assert_eq!(loaded.title, "Ship mobile MVP");

    let manager = crate::memory::MemoryManager::new().with_project_dir(&project);
    let graph = manager.load_project_graph().expect("load graph");
    let goal_mem = graph
        .get_memory(&format!("goal:{}", goal.id))
        .expect("goal memory mirror");
    assert!(goal_mem.tags.iter().any(|tag| tag == "goal"));
    assert!(goal_mem.content.contains("Ship mobile MVP"));

    let session_id = "ses_goal_test";
    attach_goal_to_session(session_id, &goal, Some(&project)).expect("attach");
    let resumed = resume_goal(session_id, Some(&project))
        .expect("resume")
        .expect("goal resumed");
    assert_eq!(resumed.id, goal.id);

    if let Some(prev_home) = prev_home {
        crate::env::set_var("JCODE_HOME", prev_home);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
}

#[test]
fn write_goal_page_auto_focuses_first_goal_only() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("tempdir");
    let project = temp.path().join("repo");
    std::fs::create_dir_all(&project).expect("project dir");
    let prev_home = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", temp.path());

    let session_id = "ses_goal_panel";
    let goal = create_goal(
        GoalCreateInput {
            title: "Ship mobile MVP".to_string(),
            scope: GoalScope::Project,
            ..GoalCreateInput::default()
        },
        Some(&project),
    )
    .expect("create goal");

    let first = write_goal_page(session_id, Some(&project), &goal, GoalDisplayMode::Auto)
        .expect("first write");
    assert_eq!(
        first.focused_page_id.as_deref(),
        Some("goal.ship-mobile-mvp")
    );

    crate::side_panel::write_markdown_page(session_id, "notes", Some("Notes"), "# Notes", true)
        .expect("notes");
    let second = write_goal_page(session_id, Some(&project), &goal, GoalDisplayMode::Auto)
        .expect("second write");
    assert_eq!(second.focused_page_id.as_deref(), Some("notes"));

    if let Some(prev_home) = prev_home {
        crate::env::set_var("JCODE_HOME", prev_home);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
}
