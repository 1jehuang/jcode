use super::*;

#[test]
fn parses_explicit_group_selection_without_treating_flags_as_hosts() {
    assert_eq!(
        parse_cloud_command("/cloud box --include-session one --include-session two --dry-run"),
        Some(CloudCommand::MoveSelected {
            host: Some("box".into()),
            include_sessions: vec!["one".into(), "two".into()],
            session_only: false,
            dry_run: true,
        })
    );
    assert_eq!(
        parse_cloud_command("/cloud --session-only --host box"),
        Some(CloudCommand::MoveSelected {
            host: Some("box".into()),
            include_sessions: vec![],
            session_only: true,
            dry_run: false,
        })
    );
    for input in [
        "/cloud --include-session",
        "/cloud --host",
        "/cloud --unknown",
        "/cloud --session-only --include-session one",
        "/cloud --include-session one --include-session one",
        "/cloud box extra",
    ] {
        assert_eq!(
            parse_cloud_command(input),
            Some(CloudCommand::MoveUsage),
            "{input}"
        );
    }
}

#[test]
fn invalid_move_options_do_not_queue_or_spawn() {
    let mut app = crate::tui::app::tests::create_test_app();
    assert!(handle_cloud_command(&mut app, "/cloud --unknown", "source"));
    assert!(app.cloud_job.is_none());
    assert!(app.pending_cloud_handoff.is_none());
    assert!(
        app.display_messages
            .last()
            .unwrap()
            .content
            .contains("--include-session")
    );
}

#[cfg(unix)]
#[test]
fn preparation_preserves_explicit_group_selection() {
    let mut app = crate::tui::app::tests::create_test_app();
    let selection = CloudCommand::MoveSelected {
        host: None,
        include_sessions: vec!["other".into()],
        session_only: false,
        dry_run: true,
    };
    app.pending_cloud_handoff = Some(pending(selection, "original-session"));
    let _tx = fake_job(&mut app, CloudCommand::Prepare { host: None });
    let job = app.cloud_job.take().unwrap();
    finish_cloud_command(&mut app, job, output(true, r#"{"host":"prepared-host"}"#));
    assert_eq!(
        app.pending_cloud_handoff.as_ref().unwrap().command,
        CloudCommand::MoveSelected {
            host: Some("prepared-host".into()),
            include_sessions: vec!["other".into()],
            session_only: false,
            dry_run: true
        }
    );
    assert!(app.pending_cloud_handoff.as_ref().unwrap().prepared);
}

#[cfg(unix)]
#[test]
fn group_preview_does_not_handoff_or_quit() {
    let mut app = crate::tui::app::tests::create_test_app();
    let _tx = fake_job(
        &mut app,
        CloudCommand::MoveSelected {
            host: None,
            include_sessions: vec!["other".into()],
            session_only: false,
            dry_run: true,
        },
    );
    let job = app.cloud_job.take().unwrap();
    finish_cloud_command(
        &mut app,
        job,
        output(true, r#"{"host":"box","dry_run":true}"#),
    );
    assert!(!app.should_quit);
    assert!(app.cloud_handoff_requested.is_none());
    assert!(
        app.display_messages
            .last()
            .unwrap()
            .content
            .contains("No session ownership moved")
    );
}

#[cfg(unix)]
#[test]
fn group_completion_attaches_coordinator_and_reports_limits() {
    let mut app = crate::tui::app::tests::create_test_app();
    let _tx = fake_job(&mut app, CloudCommand::Move { host: None });
    let job = app.cloud_job.take().unwrap();
    finish_cloud_command(
        &mut app,
        job,
        output(
            true,
            r#"{"host":"box","session_id":"coordinator","members":["coordinator","worker"],"excluded_session_ids":["other"]}"#,
        ),
    );
    assert!(matches!(app.cloud_handoff_requested,
        Some(CloudHandoff::Remote { ref session_id, .. }) if session_id == "coordinator"));
    let text = &app.display_messages.last().unwrap().content;
    assert!(text.contains("Group return is not yet supported"));
    assert!(text.contains("Staying local: other"));
    assert!(!text.contains("Use /local to bring it back"));
}

#[test]
fn replacement_requires_explicit_flags_and_never_parses_as_a_host() {
    for text in [
        "/cloud replace",
        "/cloud replace --force",
        "/cloud replace --host",
        "/cloud replace --yes --host --dry-run",
        "/cloud replace --yes extra",
    ] {
        assert_eq!(
            parse_cloud_command(text),
            Some(CloudCommand::ReplaceUsage),
            "{text}"
        );
    }
    assert_eq!(
        parse_cloud_command("/cloud replace --dry-run --host box"),
        Some(CloudCommand::Replace {
            host: Some("box".into()),
            yes: false,
            dry_run: true,
        })
    );
    assert_eq!(
        parse_cloud_command("/cloud replace --yes"),
        Some(CloudCommand::Replace {
            host: None,
            yes: true,
            dry_run: false,
        })
    );
}

#[test]
fn replacement_usage_does_not_start_or_queue_work() {
    let mut app = crate::tui::app::tests::create_test_app();
    let session = super::super::commands::active_session_id(&app);
    assert!(handle_cloud_command(&mut app, "/cloud replace", &session));
    assert!(app.cloud_job.is_none());
    assert!(app.pending_cloud_handoff.is_none());
    assert!(
        app.display_messages
            .last()
            .unwrap()
            .content
            .contains("--dry-run")
    );
}

#[test]
fn replacement_waits_for_idle_and_preserves_drafts() {
    let mut app = crate::tui::app::tests::create_test_app();
    let session = super::super::commands::active_session_id(&app);
    app.is_processing = true;
    app.input = "keep my draft".into();
    app.queued_messages.push("keep queued prompt".into());
    handle_cloud_command(&mut app, "/cloud replace --yes", &session);
    assert!(app.pending_cloud_handoff.is_some());
    assert!(app.take_ready_cloud_handoff().is_none());
    app.is_processing = false;
    assert!(matches!(
        app.take_ready_cloud_handoff().unwrap().command,
        CloudCommand::Replace { .. }
    ));
    assert_eq!(app.input, "keep my draft");
    assert_eq!(app.queued_messages, ["keep queued prompt"]);
    assert!(!app.should_quit);
    assert!(app.cloud_handoff_requested.is_none());
}

#[test]
fn replacement_child_uses_local_directory_without_session_transfer_flags() {
    let mut cmd = std::process::Command::new("jcode");
    configure_replacement(
        &mut cmd,
        Some("box"),
        true,
        false,
        std::path::Path::new("/project with spaces"),
    );
    assert_eq!(
        cmd.get_current_dir().unwrap(),
        std::path::Path::new("/project with spaces")
    );
    let args: Vec<_> = cmd.get_args().map(|arg| arg.to_str().unwrap()).collect();
    assert_eq!(args, ["replace", "--json", "--yes", "--host", "box"]);
}

#[test]
#[cfg(unix)]
fn replacement_completion_stays_local_and_shows_backup() {
    for dry_run in [false, true] {
        let mut app = crate::tui::app::tests::create_test_app();
        let command = CloudCommand::Replace {
            host: Some("box".into()),
            yes: !dry_run,
            dry_run,
        };
        let tx = fake_job(&mut app, command);
        let response = serde_json::json!({"repo_root":"/project", "backup_path":"/project.backup", "dry_run":dry_run});
        tx.send(CloudEvent::Done(Ok(output(true, &response.to_string()))))
            .unwrap();
        assert!(app.poll_cloud_job());
        assert!(!app.should_quit);
        assert!(app.cloud_handoff_requested.is_none());
        assert_eq!(app.cloud_recovery_context.is_some(), !dry_run);
        if let Some((session, reminder)) = &app.cloud_recovery_context {
            assert_eq!(session, "original-session");
            assert!(reminder.contains("box"));
            assert!(reminder.contains("environment"));
        }
        let text = &app.display_messages.last().unwrap().content;
        assert!(
            text.contains(if dry_run {
                "--yes --host box"
            } else {
                "/project.backup"
            }),
            "{text}"
        );
    }
}

#[test]
#[cfg(unix)]
fn replacement_notice_reaches_next_local_turn_once() {
    let mut app = crate::tui::app::tests::create_test_app();
    let session = super::super::commands::active_session_id(&app);
    let tx = fake_job(
        &mut app,
        CloudCommand::Replace {
            host: Some("box".into()),
            yes: true,
            dry_run: false,
        },
    );
    app.cloud_job.as_mut().unwrap().session_id = session;
    tx.send(CloudEvent::Done(Ok(output(
        true,
        r#"{"repo_root":"/project","backup_path":null,"dry_run":false,"host":"box"}"#,
    ))))
    .unwrap();
    app.poll_cloud_job();
    let reminder = app.cloud_recovery_reminder().unwrap();
    assert!(
        !app.is_processing,
        "replacement does not start an unsolicited turn"
    );
    app.input = "continue configuring cloud".into();
    app.submit_input();
    assert!(app.cloud_recovery_context.is_none());
    assert!(app.session.messages.iter().any(|message| {
        message.content.iter().any(|block| {
            matches!(block, crate::message::ContentBlock::Text { text, .. } if text.contains(&reminder))
        })
    }));
    assert!(app.take_cloud_recovery_reminder().is_none());
}

#[test]
#[cfg(unix)]
fn replacement_invalid_response_cannot_claim_success() {
    let mut app = crate::tui::app::tests::create_test_app();
    let tx = fake_job(
        &mut app,
        CloudCommand::Replace {
            host: None,
            yes: true,
            dry_run: false,
        },
    );
    tx.send(CloudEvent::Done(Ok(output(true, "{}")))).unwrap();
    app.poll_cloud_job();
    assert!(
        app.display_messages
            .last()
            .unwrap()
            .content
            .contains("invalid response")
    );
    assert!(!app.should_quit);
    assert!(app.cloud_handoff_requested.is_none());
}

#[test]
fn replacement_is_discoverable_in_slash_suggestions() {
    let mut app = crate::tui::app::tests::create_test_app();
    app.input = "/cloud ".into();
    let suggestions = app.get_suggestions_for("/cloud ");
    assert!(
        suggestions
            .iter()
            .any(|(name, _)| name == "/cloud replace --dry-run")
    );
    for input in [
        "/cloud ",
        "/cloud replace",
        "/cloud replace ",
        "/cloud replace --",
    ] {
        assert!(
            !app.get_suggestions_for(input)
                .iter()
                .any(|(name, _)| name.ends_with("--yes"))
        );
    }
}

#[test]
fn cloud_recovery_is_bounded_and_keeps_command_and_cause() {
    let context = cloud_recovery_context(
        "/cloud custom-host",
        "session_test",
        &format!("{}\nError: permission denied", "界".repeat(9000)),
    );
    assert!(context.contains("/cloud custom-host"));
    assert!(context.contains("session_test"));
    assert!(context.ends_with("Error: permission denied"));
    assert!(context.chars().count() < 10000);
    assert!(context.contains("untrusted output"));
    assert!(context.contains("ssh -F ~/.jcode/cloud/ssh_config"));
}

#[test]
fn cloud_recovery_guides_read_only_ssh_without_mutating_access() {
    let context = cloud_recovery_context(
        "/cloud custom-host",
        "session_test",
        "Error: version mismatch",
    );
    for expected in [
        "jcode cloud machine status",
        "jcode cloud where --session session_test",
        "-o BatchMode=yes -o ConnectTimeout=10",
        "command -v jcode; jcode --version",
        "~/.jcode/logs/cloud-problems.jsonl",
        "Do not alter session ownership or leases, restart daemons",
        "not the managed alias",
        "Validate and quote values",
        "ask before state-changing recovery",
        "bypass host-key verification",
    ] {
        assert!(
            context.contains(expected),
            "missing safety guidance: {expected}"
        );
    }
}

#[test]
fn cloud_failure_context_is_session_scoped_and_consumed_once() {
    let mut app = crate::tui::app::tests::create_test_app();
    let session = super::super::commands::active_session_id(&app);
    show_cloud_failure(
        &mut app,
        "/cloud custom-host",
        &session,
        Some(1),
        "Error: permission denied",
    );
    assert!(
        !app.is_processing,
        "failure should not start an unsolicited agent turn"
    );
    assert!(
        app.cloud_recovery_reminder()
            .unwrap()
            .contains("permission denied")
    );
    let saved = app.cloud_recovery_context.clone();
    app.cloud_recovery_context.as_mut().unwrap().0 = "another-session".into();
    assert!(app.take_cloud_recovery_reminder().is_none());
    app.cloud_recovery_context = saved;
    assert!(app.take_cloud_recovery_reminder().is_some());
    assert!(app.take_cloud_recovery_reminder().is_none());
}

#[test]
fn cloud_failure_reaches_next_local_agent_turn() {
    let mut app = crate::tui::app::tests::create_test_app();
    let session = super::super::commands::active_session_id(&app);
    show_cloud_failure(
        &mut app,
        "/cloud custom-host",
        &session,
        Some(1),
        "Error: permission denied",
    );
    app.input = "fix that cloud error".into();
    app.submit_input();
    assert!(app.cloud_recovery_context.is_none());
    assert!(app.session.messages.iter().any(|message| {
        message.content.iter().any(|block| {
            matches!(block,
                crate::message::ContentBlock::Text { text, .. }
                if text.contains("Attempted command: /cloud custom-host")
                    && text.contains("Error: permission denied")
            )
        })
    }));
}

fn pending(command: CloudCommand, session_id: &str) -> PendingCloudHandoff {
    PendingCloudHandoff {
        command,
        trimmed: "/cloud chosen".into(),
        session_id: session_id.into(),
        request_id: next_request_id(),
        prepared: false,
        progress: None,
    }
}

fn fake_job(app: &mut App, command: CloudCommand) -> std::sync::mpsc::Sender<CloudEvent> {
    let (tx, rx) = std::sync::mpsc::channel();
    let progress_message = app.display_messages.len();
    app.push_display_message(DisplayMessage::system("Cloud session transfer"));
    app.cloud_job = Some(CloudJob {
        progress_message,
        steps: vec!["Starting cloud command".into()],
        started: std::time::Instant::now(),
        frame: 0,
        preparation_for: app
            .pending_cloud_handoff
            .clone()
            .filter(|_| matches!(command, CloudCommand::Prepare { .. })),
        command,
        trimmed: "/cloud".into(),
        session_id: "original-session".into(),
        rx,
        stderr: String::new(),
    });
    tx
}

#[cfg(unix)]
fn output(success: bool, stdout: &str) -> std::process::Output {
    use std::os::unix::process::ExitStatusExt;
    std::process::Output {
        status: std::process::ExitStatus::from_raw(if success { 0 } else { 256 }),
        stdout: stdout.as_bytes().to_vec(),
        stderr: Vec::new(),
    }
}

fn unsent_snapshot(app: &App) -> String {
    format!(
        "{:?}",
        (
            &app.input,
            &app.queued_messages,
            &app.queued_commands,
            &app.hidden_queued_system_messages,
            &app.pending_images,
            &app.pasted_contents,
            &app.interleave_message,
            &app.interleave_images,
            &app.pending_soft_interrupts,
            &app.pending_soft_interrupt_requests,
        )
    )
}

type UnsentCase = (&'static str, fn(&mut App));

fn unsent_cases() -> Vec<UnsentCase> {
    vec![
        ("draft", |app| app.input = "save this draft".into()),
        ("message", |app| {
            app.queued_messages.push("queued prompt".into())
        }),
        ("command", |app| app.queued_commands.push("/compact".into())),
        ("hidden", |app| {
            app.hidden_queued_system_messages.push("reminder".into())
        }),
        ("attachment", |app| {
            app.pending_images.push(("image/png".into(), "data".into()))
        }),
        ("paste", |app| {
            app.pasted_contents.push("pasted draft".into())
        }),
        ("interleave", |app| {
            app.interleave_message = Some("interleave".into())
        }),
        ("interleave image", |app| {
            app.interleave_images
                .push(("image/png".into(), "data".into()))
        }),
        ("soft interrupt", |app| {
            app.pending_soft_interrupts.push("interrupt".into())
        }),
        ("unacknowledged interrupt", |app| {
            app.pending_soft_interrupt_requests
                .push((1, "interrupt".into()))
        }),
    ]
}

#[test]
fn handoff_error_identifies_blocking_queue_without_its_contents() {
    let mut app = crate::tui::app::tests::create_test_app();
    let session = super::super::commands::active_session_id(&app);
    app.hidden_queued_system_messages
        .push("private reminder".into());
    handle_cloud_command(&mut app, "/cloud", &session);
    let error = &app.display_messages.last().unwrap().content;
    assert!(error.contains("queued system reminders would be lost"));
    assert!(!error.contains("private reminder"));

    app.hidden_queued_system_messages.clear();
    app.pending_cloud_handoff = Some(pending(CloudCommand::Return, &session));
    app.pending_soft_interrupt_requests
        .push((1, "private interrupt".into()));
    assert!(app.take_ready_cloud_handoff().is_none());
    let error = &app.display_messages.last().unwrap().content;
    assert!(error.contains("unacknowledged interrupts would be lost"));
    assert!(!error.contains("private interrupt"));
}

#[test]
fn acceptance_refuses_unsent_state_without_consuming_it() {
    for (label, populate) in unsent_cases() {
        let mut app = crate::tui::app::tests::create_test_app();
        app.is_processing = true;
        populate(&mut app);
        let before = unsent_snapshot(&app);
        let session = super::super::commands::active_session_id(&app);
        handle_cloud_command(&mut app, "/local", &session);
        assert!(app.pending_cloud_handoff.is_none(), "{label}");
        assert!(app.cloud_job.is_none(), "{label}");
        assert_eq!(unsent_snapshot(&app), before, "{label}");
    }
    let mut app = crate::tui::app::tests::create_test_app();
    app.input = "/local".into();
    app.pending_images
        .push(("image/png".into(), "attachment".into()));
    let before = unsent_snapshot(&app);
    app.submit_input();
    assert_eq!(unsent_snapshot(&app), before);
    assert!(app.pending_cloud_handoff.is_none());
    assert!(app.cloud_job.is_none());
}

#[test]
fn safe_dispatch_rechecks_new_unsent_work_without_spawning() {
    for (label, populate) in unsent_cases() {
        let mut app = crate::tui::app::tests::create_test_app();
        let session = super::super::commands::active_session_id(&app);
        app.is_processing = true;
        handle_cloud_command(&mut app, "/local", &session);
        assert!(app.pending_cloud_handoff.is_some());
        populate(&mut app);
        let before = unsent_snapshot(&app);
        app.is_processing = false;
        assert!(app.poll_cloud_job(), "{label}");
        assert!(app.pending_cloud_handoff.is_none(), "{label}");
        assert!(app.cloud_job.is_none(), "{label}");
        assert!(!app.cloud_handoff_blocks_work(), "{label}");
        assert_eq!(unsent_snapshot(&app), before, "{label}");
        assert!(
            app.display_messages
                .last()
                .unwrap()
                .content
                .contains("Nothing was removed")
        );
    }
}

#[test]
fn safe_dispatch_uses_original_session_and_cancels_after_session_change() {
    let mut app = crate::tui::app::tests::create_test_app();
    let session = super::super::commands::active_session_id(&app);
    app.pending_cloud_handoff = Some(pending(CloudCommand::Return, &session));
    app.is_processing = true;
    assert!(app.take_ready_cloud_handoff().is_none());
    app.is_processing = false;
    app.pending_turn = true;
    assert!(app.take_ready_cloud_handoff().is_none());
    app.pending_turn = false;
    let ready = app
        .take_ready_cloud_handoff()
        .expect("idle request dispatched");
    assert_eq!(ready.session_id, session);
    assert_eq!(ready.command, CloudCommand::Return);
    assert!(
        app.cloud_job.is_none(),
        "deterministic dispatch helper never spawns"
    );
    app.pending_cloud_handoff = Some(ready);
    app.is_remote = true;
    app.remote_session_id = Some("different-session".into());
    assert!(app.poll_cloud_job());
    assert!(app.pending_cloud_handoff.is_none());
    assert!(app.cloud_job.is_none());
    assert!(
        app.display_messages
            .last()
            .unwrap()
            .content
            .contains("active session changed")
    );
}

#[test]
fn busy_enter_keeps_new_draft_out_of_interleave() {
    for queue_mode in [false, true] {
        let mut app = crate::tui::app::tests::create_test_app();
        app.is_processing = true;
        app.queue_mode = queue_mode;
        let session = super::super::commands::active_session_id(&app);
        handle_cloud_command(&mut app, "/local", &session);
        app.input = "new draft".into();
        super::super::input::handle_enter(&mut app);
        assert_eq!(app.input, "new draft");
        assert!(app.interleave_message.is_none());
        assert!(app.queued_messages.is_empty());
        app.is_processing = false;
        assert!(app.poll_cloud_job());
        assert!(app.pending_cloud_handoff.is_none());
        assert_eq!(app.input, "new draft");
    }
}

#[test]
fn queued_handoff_restores_draft_before_dispatch_recheck() {
    let mut app = crate::tui::app::tests::create_test_app();
    app.input = "unsent original draft".into();
    app.queued_commands.push("/local".into());
    super::super::input::run_queued_commands_local(&mut app);
    assert_eq!(app.input, "unsent original draft");
    assert!(app.pending_cloud_handoff.is_some());
    assert!(
        app.cloud_job.is_none(),
        "acceptance cannot race draft restoration"
    );
    assert!(app.poll_cloud_job());
    assert!(app.pending_cloud_handoff.is_none());
    assert!(app.cloud_job.is_none());
    assert_eq!(app.input, "unsent original draft");
}

#[test]
fn canceled_preparation_does_not_block_work() {
    let mut app = crate::tui::app::tests::create_test_app();
    app.pending_cloud_handoff = Some(pending(CloudCommand::Move { host: None }, "source"));
    let _tx = fake_job(&mut app, CloudCommand::Prepare { host: None });
    handle_cloud_command(&mut app, "/cloud cancel", "source");
    assert!(!app.cloud_handoff_blocks_work());
    assert!(
        app.cloud_job.is_some(),
        "preflight may finish independently"
    );
    app.input = "continue working here".into();
    app.submit_input();
    assert!(app.pending_turn);
    assert!(app.input.is_empty());
}

#[test]
fn busy_return_queues_without_spawning_and_status_preserves_target() {
    let mut app = crate::tui::app::tests::create_test_app();
    app.is_processing = true;
    assert!(handle_cloud_command(&mut app, "/local", "original-session"));
    let expected = app.pending_cloud_handoff.clone();
    assert!(
        app.cloud_job.is_none(),
        "no transfer child may start during a turn"
    );
    assert!(!app.poll_cloud_job());
    assert!(handle_cloud_command(
        &mut app,
        "/cloud status",
        "different-session"
    ));
    assert_eq!(app.pending_cloud_handoff, expected);
    assert_eq!(expected.unwrap().session_id, "original-session");
    assert!(app.cloud_handoff_blocks_work());
    app.input = "do more work".into();
    app.submit_input();
    assert_eq!(app.input, "do more work");
    assert!(!app.pending_turn);
    app.queued_commands.push("/cloud cancel".into());
    app.queued_messages.push("queued followup".into());
    app.is_processing = false;
    super::super::input::run_queued_commands_local(&mut app);
    assert_eq!(app.queued_commands, ["/cloud cancel"]);
    assert_eq!(app.queued_messages, ["queued followup"]);
    assert!(app.pending_cloud_handoff.is_some());
    app.is_processing = true;
    for cancel in ["/cloud cancel", "/local cancel"] {
        handle_cloud_command(&mut app, cancel, "different-session");
        assert!(app.pending_cloud_handoff.is_none());
        assert!(app.cloud_job.is_none());
        handle_cloud_command(&mut app, "/local", "original-session");
    }
}

#[test]
fn dispatch_waits_for_idle_and_staged_turn_and_completed_handoff() {
    let mut app = crate::tui::app::tests::create_test_app();
    app.pending_cloud_handoff = Some(pending(CloudCommand::Return, "original-session"));
    app.is_processing = true;
    assert!(!app.cloud_handoff_can_dispatch());
    app.is_processing = false;
    app.pending_turn = true;
    assert!(!app.cloud_handoff_can_dispatch());
    assert!(!app.poll_cloud_job());
    assert!(app.cloud_job.is_none());
    app.pending_turn = false;
    assert!(app.cloud_handoff_can_dispatch());
    let _tx = fake_job(&mut app, CloudCommand::Return);
    assert!(!app.cloud_handoff_can_dispatch());
    app.cloud_job = None;
    app.cloud_handoff_requested = Some(CloudHandoff::Local {
        session_id: "original-session".into(),
    });
    assert!(!app.cloud_handoff_can_dispatch());
    app.cloud_handoff_requested = None;
    app.should_quit = true;
    assert!(!app.cloud_handoff_can_dispatch());
}

#[cfg(unix)]
#[test]
fn prepared_host_is_pinned_without_starting_busy_transfer() {
    let mut app = crate::tui::app::tests::create_test_app();
    app.is_processing = true;
    app.pending_cloud_handoff = Some(pending(
        CloudCommand::Move { host: None },
        "original-session",
    ));
    let tx = fake_job(&mut app, CloudCommand::Prepare { host: None });
    tx.send(CloudEvent::Done(Ok(output(
        true,
        r#"{"host":"resolved-host"}"#,
    ))))
    .unwrap();
    assert!(app.poll_cloud_job());
    let queued = app.pending_cloud_handoff.as_ref().unwrap();
    assert_eq!(
        queued.command,
        CloudCommand::Move {
            host: Some("resolved-host".into())
        }
    );
    assert_eq!(queued.session_id, "original-session");
    assert!(queued.prepared);
    assert!(!app.poll_cloud_job());
    assert!(app.cloud_job.is_none());
    assert!(app.cloud_handoff_requested.is_none());
    assert!(!app.should_quit);
    app.is_processing = false;
    assert!(app.cloud_handoff_can_dispatch());
}

#[cfg(unix)]
#[test]
fn replacement_and_cancel_ignore_stale_preparation_results() {
    for cancel in [false, true] {
        let mut app = crate::tui::app::tests::create_test_app();
        app.is_processing = true;
        app.pending_cloud_handoff = Some(pending(
            CloudCommand::Move {
                host: Some("old".into()),
            },
            "original-session",
        ));
        let tx = fake_job(
            &mut app,
            CloudCommand::Prepare {
                host: Some("old".into()),
            },
        );
        handle_cloud_command(&mut app, "/cloud new", "replacement-session");
        let expected = app.pending_cloud_handoff.clone();
        assert_eq!(
            expected.as_ref().unwrap().command,
            CloudCommand::Move {
                host: Some("new".into())
            }
        );
        if cancel {
            handle_cloud_command(&mut app, "/local cancel", "replacement-session");
        }
        tx.send(CloudEvent::Done(Ok(output(true, r#"{"host":"old"}"#))))
            .unwrap();
        assert!(app.poll_cloud_job());
        assert_eq!(
            app.pending_cloud_handoff,
            if cancel { None } else { expected }
        );
        assert!(app.cloud_handoff_requested.is_none());
        assert!(!app.should_quit);
    }
}

#[cfg(unix)]
#[test]
fn preparation_failure_clears_only_its_own_request() {
    for replace in [false, true] {
        let mut app = crate::tui::app::tests::create_test_app();
        app.is_processing = true;
        app.pending_cloud_handoff = Some(pending(
            CloudCommand::Move { host: None },
            "original-session",
        ));
        let tx = fake_job(&mut app, CloudCommand::Prepare { host: None });
        if replace {
            handle_cloud_command(&mut app, "/local", "replacement-session");
        }
        tx.send(CloudEvent::Done(Ok(output(false, "")))).unwrap();
        app.poll_cloud_job();
        assert_eq!(app.pending_cloud_handoff.is_some(), replace);
        assert!(app.cloud_job.is_none());
    }
}

#[test]
fn active_transfer_rejects_replacement_and_cancellation_without_killing_job() {
    let mut app = crate::tui::app::tests::create_test_app();
    let _tx = fake_job(&mut app, CloudCommand::Return);
    handle_cloud_command(&mut app, "/cloud another", "different-session");
    handle_cloud_command(&mut app, "/cloud cancel", "different-session");
    assert!(app.pending_cloud_handoff.is_none());
    assert_eq!(
        app.cloud_job.as_ref().unwrap().command,
        CloudCommand::Return
    );
    assert!(app.cloud_handoff_blocks_work());
}

#[cfg(unix)]
#[test]
fn polling_keeps_all_steps_and_stops_spinner_on_failure() {
    use std::os::unix::process::ExitStatusExt;
    let mut app = crate::tui::app::tests::create_test_app();
    let index = app.display_messages.len();
    app.push_display_message(DisplayMessage::system("Cloud session transfer"));
    let (tx, rx) = std::sync::mpsc::channel();
    app.cloud_job = Some(CloudJob {
        progress_message: index,
        steps: vec!["Starting cloud command".into()],
        started: std::time::Instant::now(),
        frame: 0,
        command: CloudCommand::Move { host: None },
        trimmed: "/cloud".into(),
        session_id: "test".into(),
        rx,
        stderr: String::new(),
        preparation_for: None,
    });
    tx.send(CloudEvent::Progress("Checking host".into()))
        .unwrap();
    tx.send(CloudEvent::Progress("Uploading session".into()))
        .unwrap();
    assert!(app.poll_cloud_job());
    let content = &app.display_messages[index].content;
    assert!(content.contains("✓ 1. Checking host"));
    assert!(content.contains("2. Uploading session"));
    let before = content.clone();
    app.cloud_job.as_mut().unwrap().started =
        std::time::Instant::now() - std::time::Duration::from_millis(250);
    assert!(app.poll_cloud_job());
    let after = &app.display_messages[index].content;
    assert_ne!(
        &before, after,
        "active spinner must advance without new events"
    );
    assert!(after.contains("✓ 1. Checking host"));
    tx.send(CloudEvent::Stderr("Error: test failure".into()))
        .unwrap();
    tx.send(CloudEvent::Done(Ok(std::process::Output {
        status: std::process::ExitStatus::from_raw(256),
        stdout: Vec::new(),
        stderr: Vec::new(),
    })))
    .unwrap();
    assert!(app.poll_cloud_job());
    assert!(app.cloud_job.is_none());
    assert!(
        app.display_messages[index]
            .content
            .contains("✗ 2. Uploading session")
    );
    assert!(!app.poll_cloud_job());
}

#[test]
fn cloud_markdown_keeps_steps_and_diagnostics_on_separate_rows() {
    let steps = vec!["Preparing host".into(), "Uploading session".into()];
    let content = render_cloud_steps(&steps, 0, None);
    let ordinary = crate::tui::ui::render_system_message(
        &DisplayMessage::system(content.clone()),
        80,
        crate::config::DiffDisplayMode::Off,
    );
    assert_eq!(
        ordinary.len(),
        1,
        "ordinary notices are intentionally compact"
    );
    let message = DisplayMessage::system(content).with_title("Cloud transfer");
    let rendered =
        crate::tui::ui::render_system_message(&message, 80, crate::config::DiffDisplayMode::Off);
    let rows: Vec<String> = rendered
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect()
        })
        .collect();
    let completed = rows
        .iter()
        .position(|row| row.contains("✓ 1. Preparing host"))
        .unwrap();
    let active = rows
        .iter()
        .position(|row| row.contains("⠋ 2. Uploading session"))
        .unwrap();
    assert!(
        completed < active,
        "each step must occupy its own rendered row: {rows:?}"
    );
    assert!(!rows[completed].contains("Uploading"));
    println!(
        "Cloud layout comparison: ordinary notice={} row; cloud report={} rows\n{}",
        ordinary.len(),
        rows.len(),
        rows.join("\n"),
    );

    let error = format_cloud_failure(
        "/cloud",
        "Error: remote failed\n\nCaused by:\n    No OpenAI account with label 'work' found",
    );
    // Error rows render one logical line per row (see
    // test_prepare_messages_keeps_multiline_error_rows), so the message
    // itself must keep each diagnostic on its own line.
    let rows: Vec<&str> = error.lines().collect();
    let error_row = rows
        .iter()
        .position(|row| row.contains("Error: remote failed"))
        .unwrap();
    let cause_row = rows
        .iter()
        .position(|row| row.contains("Caused by:"))
        .unwrap();
    let account_row = rows
        .iter()
        .position(|row| row.contains("No OpenAI account"))
        .unwrap();
    assert!(
        error_row < cause_row && cause_row < account_row,
        "diagnostic lines must not collapse: {rows:?}"
    );
}

#[test]
fn cloud_transcript_preserves_steps_and_animates_only_active_step() {
    let steps = vec!["Preparing host".into(), "Uploading session".into()];
    let first = render_cloud_steps(&steps, 0, None);
    let next = render_cloud_steps(&steps, 1, None);
    assert!(first.contains("✓ 1. Preparing host"));
    assert!(first.contains("⠋ 2. Uploading session"));
    assert!(next.contains("⠙ 2. Uploading session"));
    assert!(render_cloud_steps(&steps, 0, Some(true)).contains("✓ 2. Uploading session"));
    let failed = render_cloud_steps(&steps, 0, Some(false));
    assert!(failed.contains("✓ 1. Preparing host"));
    assert!(failed.contains("✗ 2. Uploading session"));
}

#[test]
fn cloud_failure_strips_only_known_boilerplate() {
    let message = format_cloud_failure(
        "/cloud",
        concat!(
            "[cloud prepare] checking host\n",
            "** WARNING: connection is not using a post-quantum key exchange algorithm.\n",
            "** This session may be vulnerable to \"store now, decrypt later\" attacks.\n",
            "** The server may need to be upgraded. See https://openssh.com/pq.html\n",
            "Error: remote failed\n\nCaused by:\n    Permission denied (publickey).\n",
            "WARNING: host key changed\n",
        ),
    );
    assert!(!message.contains("[cloud"));
    assert!(!message.contains("post-quantum"));
    assert!(!message.contains("store now"));
    assert!(!message.contains("openssh.com"));
    assert!(message.contains("\n\nError: remote failed\n\nCaused by:\n    Permission denied (publickey).\nWARNING: host key changed"));
    assert!(!message.contains("```"));
    assert!(!message.contains("Sign in"));
}

#[test]
fn cloud_failure_explains_missing_saved_account() {
    for error in [
        "Saved account 'work' no longer exists. Select an account with /model.",
        "No account with label 'work' found",
        "No OpenAI account with label 'work' found",
    ] {
        let message = format_cloud_failure("/cloud", error);
        assert!(message.contains(error));
        assert!(message.contains("missing on the cloud host"));
        assert!(message.contains("use /model locally"));
        assert!(message.contains("credentials are not copied"));
    }
}

#[test]
fn cloud_failure_handles_empty_details_and_nested_fences() {
    let message = format_cloud_failure("/local", "[cloud export] exporting\n");
    assert!(message.starts_with("/local failed."));
    assert!(message.contains("No error details were returned"));
    assert!(!message.contains("```"));
    let message = format_cloud_failure("/cloud", "Error:\r\n```shell\r\nfailed\r\n```\r\n");
    assert!(message.ends_with("\n\nError:\n```shell\nfailed\n```"));
    assert!(!message.contains('\r'));
}

#[cfg(unix)]
#[test]
fn every_cloud_completion_failure_shows_a_reason() {
    for command in [
        CloudCommand::Move { host: None },
        CloudCommand::Return,
        CloudCommand::Where,
        CloudCommand::Prepare { host: None },
    ] {
        for source in ["stream", "stderr", "stdout", "empty", "wait", "invalid"] {
            let mut app = crate::tui::app::tests::create_test_app();
            app.is_processing = true;
            if matches!(command, CloudCommand::Prepare { .. }) {
                app.pending_cloud_handoff = Some(pending(
                    CloudCommand::Move { host: None },
                    "original-session",
                ));
            }
            if source == "invalid" && matches!(command, CloudCommand::Where) {
                continue;
            }
            let tx = fake_job(&mut app, command.clone());
            let cause = "cloud checkout contains unreturned work; refusing to overwrite";
            let mut result = output(
                source == "invalid",
                if source == "stdout" { cause } else { "" },
            );
            if source == "stream" {
                tx.send(CloudEvent::Stderr(format!(
                    "Error: remote failed\nError: {cause}"
                )))
                .unwrap();
            } else if source == "stderr" {
                result.stderr = cause.as_bytes().to_vec();
            }
            tx.send(CloudEvent::Done(if source == "wait" {
                Err("could not wait for child".into())
            } else {
                Ok(result)
            }))
            .unwrap();
            assert!(app.poll_cloud_job());
            let message = &app.display_messages.last().unwrap().content;
            let notice = &app.status_notice.as_ref().expect("failure notice").0;
            let expected = match source {
                "empty" => "without error details",
                "invalid" => "invalid response",
                "wait" => "could not wait for child",
                _ => cause,
            };
            assert!(
                message.lines().next().unwrap().contains(expected),
                "{source}: {message}"
            );
            assert!(notice.contains(expected), "{source}: {notice}");
            assert!(!app.should_quit);
            assert!(app.cloud_handoff_requested.is_none());
            assert!(app.pending_cloud_handoff.is_none());
        }
    }
}

#[test]
fn parses_cloud_commands() {
    assert_eq!(
        parse_cloud_command("/cloud"),
        Some(CloudCommand::Move { host: None })
    );
    assert_eq!(
        parse_cloud_command("/cloud my-box"),
        Some(CloudCommand::Move {
            host: Some("my-box".into())
        })
    );
    assert_eq!(parse_cloud_command("/local"), Some(CloudCommand::Return));
    assert_eq!(
        parse_cloud_command("/cloud back"),
        Some(CloudCommand::Return)
    );
    assert_eq!(
        parse_cloud_command("/cloud status"),
        Some(CloudCommand::Where)
    );
    assert_eq!(
        parse_cloud_command("/cloud cancel"),
        Some(CloudCommand::Cancel)
    );
    assert_eq!(
        parse_cloud_command("/local cancel"),
        Some(CloudCommand::Cancel)
    );
    assert_eq!(parse_cloud_command("/cloudy"), None);
}
