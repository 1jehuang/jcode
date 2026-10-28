use super::*;

struct LeaseGuard(String);

impl Drop for LeaseGuard {
    fn drop(&mut self) {
        crate::storage::remove_session_lease(&self.0);
    }
}

fn cloud_target(app: &App) -> (ResumeTarget, LeaseGuard) {
    let session_id = app.session.id.clone();
    crate::storage::write_session_lease(&crate::storage::SessionLease {
        session_id: session_id.clone(),
        epoch: 1,
        away_host: Some("cloud-test-host".into()),
        repo_root: Some("/remote/project with spaces".into()),
        last_sync_snapshot: None,
        last_sync_head: None,
        updated_at: "2026-10-27T00:00:00Z".into(),
    })
    .unwrap();
    (
        ResumeTarget::JcodeSession {
            session_id: session_id.clone(),
        },
        LeaseGuard(session_id),
    )
}

#[test]
fn cloud_resume_current_session_hands_off_instead_of_using_local_copy() {
    let mut app = crate::tui::app::tests::create_test_app();
    let (target, _lease) = cloud_target(&app);
    app.handle_session_picker_current_terminal_selection(&[target]);
    assert_eq!(
        app.cloud_handoff_requested,
        Some(CloudHandoff::RemoteResume {
            session_id: app.session.id.clone(),
            host: "cloud-test-host".into(),
            working_dir: Some("/remote/project with spaces".into()),
        })
    );
    assert!(app.should_quit);
    assert!(app.workspace_client.take_pending_resume_session().is_none());
}

#[test]
fn cloud_resume_preserves_unsent_work_and_pending_turns() {
    let mut app = crate::tui::app::tests::create_test_app();
    let (target, _lease) = cloud_target(&app);
    for draft in ["unsent draft", "/cloud"] {
        app.input = draft.into();
        app.handle_session_picker_current_terminal_selection(std::slice::from_ref(&target));
        assert_eq!(app.input, draft);
        assert!(!app.should_quit);
        assert!(app.cloud_handoff_requested.is_none());
    }
    app.input.clear();
    app.queued_commands.push("/model".into());
    app.handle_session_picker_current_terminal_selection(std::slice::from_ref(&target));
    assert_eq!(app.queued_commands.len(), 1);
    assert!(!app.should_quit);
    assert!(app.cloud_handoff_requested.is_none());
    app.queued_commands.clear();
    app.pending_turn = true;
    app.handle_session_picker_current_terminal_selection(&[target]);
    assert!(!app.should_quit);
    assert!(app.cloud_handoff_requested.is_none());
    assert!(app.workspace_client.take_pending_resume_session().is_none());
}

#[test]
fn cloud_resume_new_terminal_uses_ssh_without_local_socket() {
    let app = crate::tui::app::tests::create_test_app();
    let (target, _lease) = cloud_target(&app);
    let (_, args, _) =
        super::super::helpers::build_resume_command(&target, Some("/local/daemon.sock"));
    assert!(
        args.windows(2)
            .any(|pair| pair == ["--ssh", "cloud-test-host"])
    );
    assert!(
        args.windows(2)
            .any(|pair| { pair == ["--remote-working-dir", "/remote/project with spaces"] })
    );
    assert!(
        args.windows(2)
            .any(|pair| pair == ["--resume", app.session.id.as_str()])
    );
    assert!(
        !args
            .iter()
            .any(|arg| arg == "--socket" || arg == "/local/daemon.sock")
    );
}

#[test]
fn cloud_resume_terminal_spawn_fallback_still_hands_off_remotely() {
    let mut app = crate::tui::app::tests::create_test_app();
    let (target, _lease) = cloud_target(&app);
    // Unit-test terminal spawning deliberately returns false. Exercise the
    // real picker fallback rather than calling the current-terminal path.
    app.handle_session_picker_selection(&[target]);
    assert!(matches!(
        app.cloud_handoff_requested,
        Some(CloudHandoff::RemoteResume { .. })
    ));
    assert!(app.workspace_client.take_pending_resume_session().is_none());
}

#[test]
fn cloud_resume_unmigrated_target_retains_local_socket() {
    let app = crate::tui::app::tests::create_test_app();
    let target = ResumeTarget::JcodeSession {
        session_id: app.session.id.clone(),
    };
    let (_, args, _) =
        super::super::helpers::build_resume_command(&target, Some("/local/daemon.sock"));
    assert!(
        args.windows(2)
            .any(|pair| pair == ["--socket", "/local/daemon.sock"])
    );
    assert!(!args.iter().any(|arg| arg == "--ssh"));
}
