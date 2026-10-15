#[test]
fn test_fast_release_command_starts_synthetic_user_turn() {
    let mut app = create_test_app();
    app.input = "/fast-release".to_string();
    app.submit_input();

    assert!(app.is_processing);
    assert!(app.pending_turn);
    let notice = app
        .display_messages()
        .last()
        .expect("missing launch notice");
    assert_eq!(notice.role, "system");
    assert!(notice
        .content
        .contains("Starting logical commits + push + fast local release"));
}

#[test]
fn test_cut_release_alias_starts_fast_release_turn() {
    let mut app = create_test_app();
    app.input = "/cut-release".to_string();
    app.submit_input();

    assert!(app.is_processing);
    assert!(app.pending_turn);
    let notice = app
        .display_messages()
        .last()
        .expect("missing launch notice");
    assert!(notice.content.contains("fast local release"));
}

#[test]
fn test_fast_release_prompt_uses_selfdev_cache() {
    let fast_prompt = super::commands::build_fast_release_prompt();
    assert!(fast_prompt.contains("quick-release.sh --prepare-fast"));
    assert!(fast_prompt.contains("quick-release.sh --fast-local"));
    assert!(fast_prompt.contains("warm target/selfdev cache"));
    assert!(fast_prompt.contains("Do not run the separate local macOS cross-build"));
    let prepare = fast_prompt.find("--prepare-fast").unwrap();
    let bump = fast_prompt.find("Bump the version").unwrap();
    assert!(prepare < bump);
}

#[test]
fn test_fast_macos_release_command_uses_prepared_cross_build() {
    let mut app = create_test_app();
    app.input = "/fast-macos-release".to_string();
    app.submit_input();

    assert!(app.is_processing);
    assert!(app.pending_turn);
    let notice = app
        .display_messages()
        .last()
        .expect("missing launch notice");
    assert!(notice.content.contains("fast macOS release"));

    let prompt = super::commands::build_fast_macos_release_prompt();
    assert!(prompt.contains("quick-release.sh --prepare-fast-macos"));
    assert!(prompt.contains("quick-release.sh --fast-macos-local"));
    assert!(prompt.contains("macOS arm64"));
    let prepare = prompt.find("--prepare-fast-macos").unwrap();
    let bump = prompt.find("Bump the version").unwrap();
    assert!(prepare < bump);
}

#[test]
fn test_help_topic_shows_fast_macos_release_details() {
    let mut app = create_test_app();
    app.input = "/help fast-macos-release".to_string();
    app.submit_input();

    let msg = app
        .display_messages()
        .last()
        .expect("missing help response");
    assert_eq!(msg.role, "system");
    assert!(msg.content.contains("/fast-macos-release"));
    assert!(msg.content.contains("--prepare-fast-macos"));
    assert!(msg.content.contains("--fast-macos-local"));
    assert!(msg.content.contains("osxcross"));
}

#[test]
fn test_remote_release_command_uses_tag_only_ci_path() {
    let mut app = create_test_app();
    app.input = "/remote-release".to_string();
    app.submit_input();

    assert!(app.is_processing);
    assert!(app.pending_turn);
    let notice = app
        .display_messages()
        .last()
        .expect("missing launch notice");
    assert_eq!(notice.role, "system");
    assert!(notice
        .content
        .contains("Starting logical commits + push + remote release"));

    let prompt = super::commands::build_remote_release_prompt();
    assert!(prompt.contains("quick-release.sh --remote"));
    assert!(prompt.contains("without any local build"));
    assert!(prompt.contains("publication gated"));
    assert!(prompt.contains("Only use the following Jcode-specific procedure"));
    assert!(prompt.contains("repository's own established release conventions"));
    assert!(prompt.contains("Do not assume the project uses Cargo"));
    assert!(prompt.contains("tag-triggered or workflow-dispatch CI release"));
}

#[test]
fn test_commit_push_release_alias_starts_synthetic_user_turn() {
    let mut app = create_test_app();
    app.input = "/commit-push-release".to_string();
    app.submit_input();

    assert!(app.is_processing);
    assert!(app.pending_turn);
    let notice = app
        .display_messages()
        .last()
        .expect("missing launch notice");
    assert_eq!(notice.role, "system");
    assert!(notice
        .content
        .contains("Starting logical commits + push + fast local release"));
}

#[test]
fn test_help_topic_shows_cut_release_command_details() {
    let mut app = create_test_app();
    app.input = "/help cut-release".to_string();
    app.submit_input();

    let msg = app
        .display_messages()
        .last()
        .expect("missing help response");
    assert_eq!(msg.role, "system");
    assert!(msg.content.contains("/fast-release"));
    assert!(msg.content.contains("--prepare-fast"));
    assert!(msg.content.contains("--fast-local"));
    assert!(msg.content.contains("target/selfdev"));
    assert!(msg.content.contains("compatibility alias"));
}

#[test]
fn test_help_topic_shows_remote_release_command_details() {
    let mut app = create_test_app();
    app.input = "/help remote-release".to_string();
    app.submit_input();

    let msg = app
        .display_messages()
        .last()
        .expect("missing help response");
    assert_eq!(msg.role, "system");
    assert!(msg.content.contains("/remote-release"));
    assert!(msg.content.contains("--remote"));
    assert!(msg.content.contains("without running any local build"));
    assert!(msg.content.contains("remains a draft"));
}

#[test]
fn test_release_prompts_require_green_metadata_ci_before_tagging_or_staging() {
    for prompt in [
        super::commands::build_fast_release_prompt(),
        super::commands::build_fast_macos_release_prompt(),
        super::commands::build_remote_release_prompt(),
        super::commands::build_merge_remote_release_prompt(),
    ] {
        assert!(prompt.contains("exact SHA of the pushed release-metadata commit"));
        assert!(prompt.contains("every mandatory check and job is green"));
        assert!(prompt.contains("missing, pending, skipped, cancelled, or failed"));
        assert!(prompt.contains("Never bypass CI prerequisites"));
        assert!(prompt.contains("stage-only on a draft"));
        assert!(prompt.contains("full publication"));
        assert!(prompt.contains("published release downloads"));
        assert!(prompt.contains("installer smoke checks"));
        assert!(!prompt.contains("push the tag immediately"));
        assert!(!prompt.contains("publish that Linux asset and the GitHub release immediately"));
        assert!(!prompt.contains("publish the prepared macOS arm64 asset and GitHub release immediately"));
        let gate = prompt.find("Before invoking a tagging or staging release helper").unwrap();
        let helper = prompt.find("Then run scripts/quick-release.sh").unwrap();
        assert!(gate < helper, "metadata CI must precede tagging or staging");
    }
}

#[test]
fn test_release_help_requires_ci_and_verified_publication() {
    for command in ["fast-release", "fast-macos-release", "remote-release", "cut-release"] {
        let mut app = create_test_app();
        app.input = format!("/help {command}");
        app.submit_input();
        let message = app.display_messages().last().unwrap();
        assert!(message.content.contains("successful exact-SHA CI"));
        assert!(message.content.contains("release-metadata commit") || message.content.contains("release metadata"));
        assert!(message.content.contains("installer smoke checks"));
        assert!(message.content.contains("Never bypass CI prerequisites"));
        if command != "remote-release" {
            assert!(message.content.contains("stage-only"));
            assert!(message.content.contains("never publish immediately"));
        }
    }
}
