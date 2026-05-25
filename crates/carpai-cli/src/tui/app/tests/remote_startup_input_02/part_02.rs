#[test]
fn test_handle_background_task_completed_renders_markdown_preview() {
    let mut app = create_test_app();
    let event = BusEvent::BackgroundTaskCompleted(BackgroundTaskCompleted {
        task_id: "bg123".to_string(),
        tool_name: "bash".to_string(),
        display_name: None,
        session_id: app.session.id.clone(),
        status: BackgroundTaskStatus::Completed,
        exit_code: Some(0),
        output_preview: "[stderr] one\n[stdout] two\n".to_string(),
        output_file: std::env::temp_dir().join("bg123.output"),
        duration_secs: 7.1,
        notify: true,
        wake: false,
    });

    super::local::handle_bus_event(&mut app, Ok(event));

    let rendered = app
        .display_messages()
        .last()
        .expect("background task message");
    assert_eq!(rendered.role, "background_task");
    assert!(
        rendered
            .content
            .contains("**Background task** `bg123` · `bash` · ✓ completed · 7.1s · exit 0")
    );
    assert!(rendered.content.contains("```text"));
    assert!(rendered.content.contains("[stderr] one"));
    assert!(
        rendered
            .content
            .contains("_Full output:_ `bg action=\"output\" task_id=\"bg123\"`")
    );
    assert_eq!(
        app.status_notice(),
        Some("Background task completed · bash".to_string())
    );
}

#[test]
fn test_handle_background_task_completed_with_wake_starts_pending_turn() {
    let mut app = create_test_app();
    let event = BusEvent::BackgroundTaskCompleted(BackgroundTaskCompleted {
        task_id: "bgwake".to_string(),
        tool_name: "selfdev-build".to_string(),
        display_name: None,
        session_id: app.session.id.clone(),
        status: BackgroundTaskStatus::Completed,
        exit_code: Some(0),
        output_preview: "done\n".to_string(),
        output_file: std::env::temp_dir().join("bgwake.output"),
        duration_secs: 1.2,
        notify: true,
        wake: true,
    });

    super::local::handle_bus_event(&mut app, Ok(event));

    assert!(app.pending_turn);
    assert!(app.is_processing());
    assert!(matches!(
        crate::tui::TuiState::status(&app),
        ProcessingStatus::Sending
    ));
}

#[test]
fn test_handle_background_task_progress_updates_status_notice() {
    let mut app = create_test_app();
    let event = BusEvent::BackgroundTaskProgress(BackgroundTaskProgressEvent {
        task_id: "bgprogress".to_string(),
        tool_name: "bash".to_string(),
        display_name: None,
        session_id: app.session.id.clone(),
        progress: BackgroundTaskProgress {
            kind: BackgroundTaskProgressKind::Determinate,
            percent: Some(42.0),
            message: Some("Running tests".to_string()),
            current: Some(21),
            total: Some(50),
            unit: Some("tests".to_string()),
            eta_seconds: None,
            updated_at: chrono::Utc::now().to_rfc3339(),
            source: BackgroundTaskProgressSource::Reported,
        },
    });

    super::local::handle_bus_event(&mut app, Ok(event));

    assert_eq!(
        app.status_notice(),
        Some("Background task · bash · 42% · Running tests".to_string())
    );
    let progress_messages: Vec<_> = app
        .display_messages()
        .iter()
        .filter(|message| message.role == "background_task")
        .collect();
    assert_eq!(progress_messages.len(), 1);
    assert!(
        progress_messages[0]
            .content
            .starts_with("**Background task progress** `bgprogress` · `bash`\n\n")
    );
}

#[test]
fn test_handle_background_task_progress_debounces_identical_notice_updates() {
    let mut app = create_test_app();
    let first_event = BusEvent::BackgroundTaskProgress(BackgroundTaskProgressEvent {
        task_id: "bgprogress".to_string(),
        tool_name: "bash".to_string(),
        display_name: None,
        session_id: app.session.id.clone(),
        progress: BackgroundTaskProgress {
            kind: BackgroundTaskProgressKind::Determinate,
            percent: Some(42.0),
            message: Some("Running tests".to_string()),
            current: Some(21),
            total: Some(50),
            unit: Some("tests".to_string()),
            eta_seconds: None,
            updated_at: chrono::Utc::now().to_rfc3339(),
            source: BackgroundTaskProgressSource::Reported,
        },
    });
    super::local::handle_bus_event(&mut app, Ok(first_event));
    let first_at = app.status_notice.as_ref().map(|(_, at)| *at).unwrap();

    let second_event = BusEvent::BackgroundTaskProgress(BackgroundTaskProgressEvent {
        task_id: "bgprogress".to_string(),
        tool_name: "bash".to_string(),
        display_name: None,
        session_id: app.session.id.clone(),
        progress: BackgroundTaskProgress {
            kind: BackgroundTaskProgressKind::Determinate,
            percent: Some(42.0),
            message: Some("Running tests".to_string()),
            current: Some(21),
            total: Some(50),
            unit: Some("tests".to_string()),
            eta_seconds: None,
            updated_at: chrono::Utc::now().to_rfc3339(),
            source: BackgroundTaskProgressSource::Reported,
        },
    });
    super::local::handle_bus_event(&mut app, Ok(second_event));

    let second_at = app.status_notice.as_ref().map(|(_, at)| *at).unwrap();
    assert_eq!(
        first_at, second_at,
        "identical progress notice should be debounced"
    );
}

#[test]
fn test_handle_background_task_progress_updates_existing_card() {
    let mut app = create_test_app();
    let session_id = app.session.id.clone();

    for (percent, message) in [(42.0, "Running tests"), (75.0, "Packaging artifacts")] {
        super::local::handle_bus_event(
            &mut app,
            Ok(BusEvent::BackgroundTaskProgress(
                BackgroundTaskProgressEvent {
                    task_id: "bgprogress".to_string(),
                    tool_name: "bash".to_string(),
                    display_name: None,
                    session_id: session_id.clone(),
                    progress: BackgroundTaskProgress {
                        kind: BackgroundTaskProgressKind::Determinate,
                        percent: Some(percent),
                        message: Some(message.to_string()),
                        current: None,
                        total: None,
                        unit: None,
                        eta_seconds: None,
                        updated_at: chrono::Utc::now().to_rfc3339(),
                        source: BackgroundTaskProgressSource::Reported,
                    },
                },
            )),
        );
    }

    let progress_messages: Vec<_> = app
        .display_messages()
        .iter()
        .filter(|message| message.role == "background_task")
        .collect();
    assert_eq!(progress_messages.len(), 1);
    assert!(
        progress_messages[0]
            .content
            .contains("75% · Packaging artifacts")
    );
    assert!(!progress_messages[0].content.contains("42% · Running tests"));
}

#[test]
fn test_handle_server_event_input_shell_result_renders_markdown_blocks() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.handle_server_event(
        crate::protocol::ServerEvent::InputShellResult {
            result: crate::message::InputShellResult {
                command: "pwd".to_string(),
                cwd: Some("/tmp/project".to_string()),
                output: "/tmp/project\n".to_string(),
                exit_code: Some(0),
                duration_ms: 5,
                truncated: false,
                failed_to_start: false,
            },
        },
        &mut remote,
    );

    let rendered = app.display_messages().last().expect("shell result message");
    assert_eq!(rendered.role, "system");
    assert!(rendered.content.contains("**Shell command**"));
    assert!(rendered.content.contains("```bash"));
    assert!(rendered.content.contains("pwd"));
    assert!(rendered.content.contains("```text"));
    assert!(rendered.content.contains("/tmp/project"));
    assert_eq!(
        app.status_notice(),
        Some("Shell command completed".to_string())
    );
}

#[test]
fn test_streaming_tokens() {
    let mut app = create_test_app();

    assert_eq!(app.streaming_tokens(), (0, 0));

    app.streaming_input_tokens = 100;
    app.streaming_output_tokens = 50;

    assert_eq!(app.streaming_tokens(), (100, 50));
}

#[test]
fn test_build_turn_footer_uses_compact_duration_labels() {
    let app = create_test_app();

    assert_eq!(
        app.build_turn_footer(Some(316.1)),
        Some("5m 16s".to_string())
    );
    assert_eq!(app.build_turn_footer(Some(9.2)), Some("9.2s".to_string()));
}
