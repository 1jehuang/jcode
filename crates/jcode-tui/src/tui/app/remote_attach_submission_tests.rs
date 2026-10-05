//! Tests for cross-project attach notices and remote skill prompt submission.

use super::*;

#[test]
fn cross_project_attach_shows_the_user_which_project_the_session_belongs_to() {
    let mut app = create_test_app();
    app.client_launch_working_dir = Some("/client/project".to_string());
    app.session.working_dir = Some("/server/project".to_string());

    assert!(
        app.note_cross_project_attach(),
        "attaching from one project to a session in another must be surfaced"
    );

    let card = app
        .display_messages
        .iter()
        .find(|m| m.content.contains("/server/project"))
        .unwrap_or_else(|| {
            panic!(
                "no notice card names the session project; got: {:#?}",
                app.display_messages
            )
        });
    assert!(
        card.content.contains("/client/project"),
        "the notice must name the project the user launched in: {}",
        card.content
    );
}

#[test]
fn cross_project_attach_notice_fires_once_per_client() {
    // A reconnect re-runs remote startup, so the same mismatch must not re-announce
    // itself every time the client reattaches.
    let mut app = create_test_app();
    app.client_launch_working_dir = Some("/client/project".to_string());
    app.session.working_dir = Some("/server/project".to_string());

    assert!(app.note_cross_project_attach());
    let cards_after_first = app
        .display_messages
        .iter()
        .filter(|m| m.content.contains("/server/project"))
        .count();

    assert!(!app.note_cross_project_attach());
    let cards_after_second = app
        .display_messages
        .iter()
        .filter(|m| m.content.contains("/server/project"))
        .count();

    assert_eq!(
        cards_after_first, cards_after_second,
        "the notice must not be repeated on reattach"
    );
}

#[test]
fn same_project_attach_shows_no_notice() {
    // The common case must stay silent: a notice that fires on an ordinary session
    // would train the user to ignore it.
    let mut app = create_test_app();
    app.client_launch_working_dir = Some("/client/project".to_string());
    app.session.working_dir = Some("/client/project/".to_string());

    assert!(!app.note_cross_project_attach());
    assert!(
        app.display_messages.is_empty(),
        "a same-project attach must not post anything: {:#?}",
        app.display_messages
    );
}

#[test]
fn remote_skill_invocation_with_prompt_sends_remote_turn() {
    let mut app = create_test_app();
    app.is_remote = true;
    app.runtime_mode = crate::tui::app::AppRuntimeMode::RemoteClient;
    let temp = tempfile::tempdir().expect("create skill dir");
    let skill_dir = temp.path().join(".jcode/skills/remote-skill");
    std::fs::create_dir_all(&skill_dir).expect("create skill dir");
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: remote-skill\ndescription: Remote prompt regression skill\n---\nUse it.\n",
    )
    .expect("write skill");
    app.session.working_dir = Some(temp.path().to_string_lossy().to_string());

    let rt = tokio::runtime::Runtime::new().expect("runtime");
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    remote.mark_history_loaded();
    rt.block_on(crate::tui::app::remote::submit_remote_slash_input(
        &mut app,
        &mut remote,
        crate::tui::app::input::PreparedInput {
            raw_input: "/remote-skill explain the change".to_string(),
            expanded: "/remote-skill explain the change".to_string(),
            images: vec![],
        },
    ))
    .expect("remote skill prompt should send");

    assert_eq!(app.active_skill.as_deref(), Some("remote-skill"));
    assert!(app.is_processing, "remote skill prompt should start a turn");
    assert!(
        app.display_messages()
            .iter()
            .any(|message| message.role == "user"
                && message.content == "/remote-skill explain the change"),
        "remote skill prompt should be visible as the submitted user turn"
    );
}
