#[test]
fn skills_command_lists_loaded_and_endorsed_skills() {
    let mut app = create_test_app();

    assert!(super::state_ui::handle_info_command(&mut app, "/skills"));
    let content = app.display_messages().last().unwrap().content.clone();

    assert!(content.contains("Loaded skills"), "{content}");
    assert!(
        content.contains("Endorsed skills (recommended by jcode)"),
        "{content}"
    );
    // Every endorsed skill should appear with an install status marker.
    for endorsed in crate::skill::endorsed_skills() {
        assert!(
            content.contains(&format!("/{}", endorsed.name)),
            "expected endorsed skill /{} in:\n{content}",
            endorsed.name
        );
    }
    assert!(
        content.contains("[installed]") || content.contains("[not installed]"),
        "{content}"
    );
    // NVIDIA CUDA-X skills are grouped under their own category with install hints.
    assert!(content.contains("NVIDIA CUDA-X"), "{content}");
    assert!(
        content.contains("/cuopt-numerical-optimization-api-python"),
        "{content}"
    );
    assert!(
        content.contains("install: npx skills add nvidia/skills"),
        "{content}"
    );
    assert!(
        content.contains("https://github.com/NVIDIA/skills"),
        "{content}"
    );
    assert_eq!(
        app.display_messages().last().unwrap().title.as_deref(),
        Some("Skills")
    );
}

#[test]
fn skills_command_marks_active_skill_in_remote_mode() {
    let mut app = create_test_app();
    app.is_remote = true;
    app.remote_skills = vec!["optimization".to_string(), "firefox-browser".to_string()];
    app.active_skill = Some("optimization".to_string());

    assert!(super::state_ui::handle_info_command(&mut app, "/skills"));
    let content = app.display_messages().last().unwrap().content.clone();

    assert!(content.contains("- /optimization (active)"), "{content}");
    assert!(content.contains("- /firefox-browser\n"), "{content}");
    // Endorsed list should mark remote-installed skills as installed.
    assert!(
        content.contains("/firefox-browser [installed]"),
        "{content}"
    );
}

/// Regression for issue #431 (and #457): skills added on disk after startup
/// must show up in `/skills` and the skills snapshot without a session
/// restart. With the session-scoped project overlay, project-local skills are
/// visible immediately, without even running `/skills` first.
#[test]
fn skills_command_refreshes_registry_from_disk_before_listing() {
    let mut app = create_test_app();

    // Point the session at a fresh project dir and add a project-local skill
    // after the app (and its skill snapshot) was created.
    let temp = tempfile::tempdir().expect("tempdir");
    let skill_dir = temp.path().join(".jcode").join("skills").join("late-skill");
    std::fs::create_dir_all(&skill_dir).expect("create skill dir");
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: late-skill\ndescription: Added after startup\n---\n# Late skill\n",
    )
    .expect("write SKILL.md");
    app.session.working_dir = Some(temp.path().to_string_lossy().to_string());

    // Project-local skills are a session overlay composed at read time, so
    // the new skill is available immediately (issue #457).
    assert!(
        app.current_skills_snapshot().get("late-skill").is_some(),
        "project-local skill must be visible immediately without reload"
    );

    assert!(super::state_ui::handle_info_command(&mut app, "/skills"));
    let content = app.display_messages().last().unwrap().content.clone();

    assert!(
        content.contains("- /late-skill"),
        "expected late-added skill in /skills output:\n{content}"
    );
    assert!(
        app.current_skills_snapshot().get("late-skill").is_some(),
        "registry snapshot must be synced so /late-skill invocations resolve"
    );
}

#[test]
fn skill_invocation_with_prompt_activates_and_submits_in_one_turn() {
    let mut app = create_test_app();
    let temp = tempfile::tempdir().expect("tempdir");
    let skill_dir = temp.path().join(".jcode/skills/prompt-skill");
    std::fs::create_dir_all(&skill_dir).expect("create skill dir");
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: prompt-skill\ndescription: Prompt regression skill\n---\nUse it.\n",
    )
    .expect("write skill");
    app.session.working_dir = Some(temp.path().to_string_lossy().to_string());
    app.input = "/prompt-skill \"then type prompt here and all that\"".to_string();
    app.cursor_pos = app.input.len();

    app.submit_input();

    assert_eq!(app.active_skill.as_deref(), Some("prompt-skill"));
    assert!(app.is_processing, "the trailing prompt should start a turn");
    let submitted = app
        .session
        .messages
        .last()
        .expect("submitted session message");
    assert!(matches!(
        submitted.content.as_slice(),
        [ContentBlock::Text { text, .. }] if text == "then type prompt here and all that"
    ));
}

#[test]
fn skill_invocation_with_prompt_attaches_pending_image_to_user_message() {
    let mut app = create_test_app();
    let temp = tempfile::tempdir().expect("tempdir");
    let skill_dir = temp.path().join(".jcode/skills/image-skill");
    std::fs::create_dir_all(&skill_dir).expect("create skill dir");
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: image-skill\ndescription: Image attachment regression skill\n---\nUse it.\n",
    )
    .expect("write skill");
    app.session.working_dir = Some(temp.path().to_string_lossy().to_string());
    app.pending_images = vec![("image/png".to_string(), "ZmFrZSBwbmcgYnl0ZXM=".to_string())];
    app.input = "/image-skill describe this screenshot".to_string();
    app.cursor_pos = app.input.len();

    app.submit_input();

    assert_eq!(app.active_skill.as_deref(), Some("image-skill"));
    assert!(app.is_processing, "the trailing prompt should start a turn");
    assert!(
        app.pending_images.is_empty(),
        "pending images must be consumed by the submitted turn"
    );
    let submitted = app
        .session
        .messages
        .last()
        .expect("submitted session message");
    assert_eq!(submitted.role, Role::User);
    assert!(matches!(
        submitted.content.as_slice(),
        [
            ContentBlock::Image { media_type, data },
            ContentBlock::Text { text, .. },
        ] if media_type == "image/png"
            && data == "ZmFrZSBwbmcgYnl0ZXM="
            && text == "describe this screenshot"
    ));
}

#[test]
fn unknown_skill_invocation_surfaces_error_and_sends_nothing() {
    let mut app = create_test_app();
    let temp = tempfile::tempdir().expect("tempdir");
    app.session.working_dir = Some(temp.path().to_string_lossy().to_string());
    app.input = "/definitely-not-a-real-skill".to_string();
    app.cursor_pos = app.input.len();
    let session_messages_before = app.session.messages.len();

    app.submit_input();

    assert!(!app.is_processing, "unknown skill must not start a turn");
    assert_eq!(
        app.session.messages.len(),
        session_messages_before,
        "no message should be sent to the model"
    );
    assert!(app.active_skill.is_none());
    let last = app.display_messages().last().expect("error message");
    assert_eq!(last.role, "error");
    assert_eq!(last.content, "Unknown skill: /definitely-not-a-real-skill");
}

#[test]
fn endorsed_but_not_installed_skill_invocation_surfaces_install_hint() {
    let mut app = create_test_app();
    // Empty working dir so no endorsed skill is actually installed there.
    let temp = tempfile::tempdir().expect("tempdir");
    app.session.working_dir = Some(temp.path().to_string_lossy().to_string());

    let endorsed = crate::skill::endorsed_skills()
        .iter()
        .find(|endorsed| {
            endorsed.install.is_some() && app.current_skills_snapshot().get(endorsed.name).is_none()
        })
        .expect("an endorsed skill with an install hint that is not installed");

    app.input = format!("/{}", endorsed.name);
    app.cursor_pos = app.input.len();
    let session_messages_before = app.session.messages.len();

    app.submit_input();

    assert!(!app.is_processing, "missing skill must not start a turn");
    assert_eq!(
        app.session.messages.len(),
        session_messages_before,
        "no message should be sent to the model"
    );
    assert!(app.active_skill.is_none());
    let last = app.display_messages().last().expect("error message");
    assert_eq!(last.role, "error");
    assert!(
        last.content.contains(&format!(
            "Skill /{} is endorsed but not installed",
            endorsed.name
        )),
        "{}",
        last.content
    );
    assert!(
        last.content
            .contains(&format!("`{}`", endorsed.install.unwrap())),
        "install hint missing from: {}",
        last.content
    );
    assert!(
        !last.content.contains("Unknown skill"),
        "endorsed skill must not be reported as a typo: {}",
        last.content
    );
}
