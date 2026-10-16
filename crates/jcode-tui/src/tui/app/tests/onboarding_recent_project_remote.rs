#[test]
fn starting_recent_project_review_queues_remote_turn_without_stuck_sending() {
    let mut app = onboarding_test_app();
    app.is_remote = true;
    let repository = app
        .onboarding_recent_project_path()
        .expect("remote session should provide its working directory");
    let expected = App::onboarding_recent_project_review_prompt(&repository);

    app.onboarding_start_recent_project_review();

    assert!(!app.onboarding_flow_active());
    assert!(
        !app.pending_turn,
        "remote review must not set the local pending-turn flag"
    );
    assert!(
        !app.is_processing,
        "remote review must stay idle until the remote queue dispatches"
    );
    assert!(app.input.is_empty());
    assert_eq!(app.queued_messages, vec![expected]);
}

#[test]
fn recent_project_review_falls_back_cleanly_when_no_repo_is_known() {
    let mut app = onboarding_test_app();
    app.is_remote = true;
    app.session.working_dir = dirs::home_dir().map(|path| path.to_string_lossy().into_owned());

    app.onboarding_start_recent_project_review();

    assert!(!app.pending_turn);
    assert!(app.queued_messages.is_empty());
    assert!(!app.onboarding_flow_active());
    assert!(
        app.status_notice
            .as_ref()
            .is_some_and(|(notice, _)| { notice.contains("No active Git repository found") })
    );
}
