use super::*;

#[test]
fn reconcile_keeps_running_request_not_yet_registered_in_live_task_map() {
    // Regression: spawn_with_notify writes the Running status file and starts
    // the build future *before* inserting the task into the in-process task
    // map. The build's own first wait_for_turn iteration (or another agent's
    // queue poll) could then see status=Running + is_live_task=false and prune
    // the request instantly: "Queued build request disappeared". Within the
    // bootstrap grace window a Running-but-unregistered task must survive.
    let _storage_guard = crate::storage::lock_test_env();
    let temp_home = tempfile::TempDir::new().expect("temp home");
    let _home_guard = EnvVarGuard::set("JCODE_HOME", temp_home.path());

    let mut session = session::Session::create(None, Some("Racing Build".to_string()));
    session.save().expect("save session");

    let status_path = temp_home.path().join("racing.status.json");
    storage::write_json(
        &status_path,
        &background::TaskStatusFile {
            task_id: "racing-task-not-in-live-map".to_string(),
            tool_name: "selfdev-build".to_string(),
            display_name: Some("selfdev build".to_string()),
            session_id: session.id.clone(),
            status: BackgroundTaskStatus::Running,
            exit_code: None,
            error: None,
            started_at: Utc::now().to_rfc3339(),
            completed_at: None,
            duration_secs: None,
            pid: None,
            owner_pid: None,
            owner_instance: None,
            detached: false,
            notify: true,
            wake: true,
            progress: None,
            event_history: Vec::new(),
            stall_wake_seconds: None,
        },
    )
    .expect("write running status file");

    let source = test_source_state(std::path::Path::new("/tmp/jcode"));
    let request = BuildRequest {
        request_id: "racing-request".to_string(),
        background_task_id: Some("racing-task-not-in-live-map".to_string()),
        session_id: session.id.clone(),
        session_short_name: session.short_name.clone(),
        session_title: Some("Racing Build".to_string()),
        release_notes: None,
        reason: "racing reason".to_string(),
        repo_dir: "/tmp/jcode".to_string(),
        repo_scope: source.repo_scope.clone(),
        worktree_scope: source.worktree_scope.clone(),
        command: "scripts/dev_cargo.sh build --profile selfdev -p jcode --bin jcode".to_string(),
        requested_at: Utc::now().to_rfc3339(),
        started_at: None,
        completed_at: None,
        state: BuildRequestState::Queued,
        version: Some("racing-build".to_string()),
        dedupe_key: Some("racing-dedupe".to_string()),
        requested_source: Some(source.clone()),
        built_source: None,
        published_version: None,
        last_progress: Some("queued".to_string()),
        validated: false,
        error: None,
        output_file: None,
        status_file: Some(status_path.display().to_string()),
        attached_to_request_id: None,
    };
    request.save().expect("save racing request");

    let pending =
        BuildRequest::pending_requests_for_scope(&source.worktree_scope).expect("pending requests");
    assert!(
        pending
            .iter()
            .any(|request| request.request_id == "racing-request"),
        "running-but-unregistered request must stay pending during bootstrap grace"
    );

    let reloaded = BuildRequest::load("racing-request")
        .expect("load racing request")
        .expect("racing request exists");
    assert_eq!(reloaded.state, BuildRequestState::Queued);
    assert!(reloaded.error.is_none());
}
