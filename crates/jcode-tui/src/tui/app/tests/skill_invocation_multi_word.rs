#[test]
fn skill_invocation_matches_a_multi_word_skill_name() {
    let mut app = create_test_app();
    let temp = tempfile::tempdir().expect("tempdir");
    let skill_dir = temp.path().join(".jcode/skills/my-custom-skill");
    std::fs::create_dir_all(&skill_dir).expect("create skill dir");
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: My Custom Skill\ndescription: Commit staged changes\n---\nUse it.\n",
    )
    .expect("write skill");
    app.session.working_dir = Some(temp.path().to_string_lossy().to_string());
    app.input = "/My Custom Skill".to_string();
    app.cursor_pos = app.input.len();

    app.submit_input();

    assert_eq!(app.active_skill.as_deref(), Some("My Custom Skill"));
    // A bare /skill now submits a turn (placeholder prompt) so the skill body
    // is injected on the next run_turn_interactive; the activation message is
    // therefore no longer the *last* display message (a user message is
    // appended after it). Assert the activation message exists and never shows
    // an error, rather than that it is the last message.
    let activation = app
        .display_messages()
        .iter()
        .find(|m| m.content.contains("Activated skill: My Custom Skill"))
        .expect("activation message");
    assert!(activation.content.contains("Activated skill: My Custom Skill"));
    assert!(
        !app.display_messages().iter().any(|m| m.content.contains("Unknown skill")),
        "no display message should report an unknown skill"
    );
}
