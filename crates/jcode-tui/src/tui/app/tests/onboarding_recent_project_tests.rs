fn seed_onboarding_git_project(app: &mut App) -> tempfile::TempDir {
    let directory = tempfile::tempdir().expect("temporary project");
    let status = std::process::Command::new("git")
        .args(["init", "-q"])
        .arg(directory.path())
        .status()
        .expect("initialize isolated Git project");
    assert!(status.success());
    app.session.working_dir = Some(directory.path().to_string_lossy().into_owned());
    // Pin the completed prefetch as well as the fallback. A background scan of
    // unrelated session history must not replace the repository mid-assertion.
    app.onboarding_recent_project_prefetch = Some(std::sync::Arc::new(std::sync::Mutex::new(
        Some(Some(directory.path().to_path_buf())),
    )));
    directory
}

#[test]
fn recent_project_review_prompt_is_bounded_read_only_and_requires_approval() {
    let repository = std::path::Path::new("/home/example/projects/demo");
    let prompt = App::onboarding_recent_project_review_prompt(repository);

    assert_eq!(
        prompt,
        "Find the most critical architecture problems in the repository at \"/home/example/projects/demo\". Do not fix them yet, and ask me whether I want them fixed once you find them."
    );
}

#[test]
fn preparing_recent_project_review_finishes_onboarding_and_seeds_the_first_turn() {
    with_temp_jcode_home(|| {
        let mut app = onboarding_test_app();
        let _repository = seed_onboarding_git_project(&mut app);
        let repository = app
            .onboarding_recent_project_path()
            .expect("test session should start in a Git repository");
        let expected = App::onboarding_recent_project_review_prompt(&repository);

        assert!(app.onboarding_prepare_recent_project_review());

        assert!(!app.onboarding_flow_active());
        assert_eq!(app.input, expected);
        assert_eq!(app.cursor_pos, app.input.len());
    });
}

#[test]
fn starting_recent_project_review_runs_as_a_visible_local_turn() {
    with_temp_jcode_home(|| {
        let mut app = onboarding_test_app();
        let _repository = seed_onboarding_git_project(&mut app);
        let repository = app
            .onboarding_recent_project_path()
            .expect("test session should start in a Git repository");
        let expected = App::onboarding_recent_project_review_prompt(&repository);

        app.onboarding_start_recent_project_review();

        assert!(!app.onboarding_flow_active());
        assert!(app.pending_turn, "local review should start a local turn");
        assert!(app.is_processing, "local review should enter Sending");
        assert!(app.queued_messages.is_empty());
        assert_eq!(
            app.session.messages.last().and_then(|message| {
                message.content.iter().find_map(|block| match block {
                    crate::message::ContentBlock::Text { text, .. } => Some(text.as_str()),
                    _ => None,
                })
            }),
            Some(expected.as_str())
        );
    });
}
