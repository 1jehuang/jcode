#[test]
fn test_local_bus_dictation_completion_ignores_other_session() {
    let mut app = create_test_app();
    let session_id = app.session.id.clone();
    app.input = "draft".to_string();
    app.cursor_pos = app.input.len();
    app.dictation_in_flight = true;
    app.dictation_request_id = Some("dictation_123".to_string());
    app.dictation_target_session_id = Some(session_id);

    let handled = crate::tui::app::local::handle_bus_event(
        &mut app,
        Ok(crate::bus::BusEvent::DictationCompleted {
            dictation_id: "dictation_other".to_string(),
            session_id: Some("session_other".to_string()),
            text: " dictated text".to_string(),
            mode: crate::protocol::TranscriptMode::Append,
        }),
    );

    assert!(!handled);
    assert_eq!(app.input, "draft");
    assert!(app.dictation_in_flight);
}

#[test]
fn test_remote_bus_dictation_completion_ignores_other_session() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut remote = rt.block_on(async { crate::tui::backend::RemoteConnection::dummy() });
    app.is_remote = true;
    app.remote_session_id = Some("session_remote".to_string());
    app.dictation_in_flight = true;
    app.dictation_request_id = Some("dictation_123".to_string());
    app.dictation_target_session_id = Some("session_remote".to_string());

    rt.block_on(crate::tui::app::remote::handle_bus_event(
        &mut app,
        &mut remote,
        Ok(crate::bus::BusEvent::DictationCompleted {
            dictation_id: "dictation_other".to_string(),
            session_id: Some("session_other".to_string()),
            text: " dictated text".to_string(),
            mode: crate::protocol::TranscriptMode::Append,
        }),
    ));

    assert!(app.dictation_in_flight);
    assert_eq!(app.dictation_request_id.as_deref(), Some("dictation_123"));
}

#[test]
fn test_handle_server_event_transcript_send_prefixes_user_message() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.handle_server_event(
        crate::protocol::ServerEvent::Transcript {
            text: "dictated hello".to_string(),
            mode: crate::protocol::TranscriptMode::Send,
        },
        &mut remote,
    );

    let last = app
        .display_messages()
        .last()
        .expect("user message displayed");
    assert_eq!(last.role, "user");
    assert_eq!(last.content, "[transcription] dictated hello");
    assert!(app.messages.is_empty());
    assert!(matches!(
        app.session.messages.last().and_then(|message| message.content.last()),
        Some(crate::message::ContentBlock::Text { text, .. }) if text == "[transcription] dictated hello"
    ));
    assert!(
        app.pending_turn,
        "local transcript send should use normal submit path"
    );
}

#[test]
fn test_handle_server_event_session_close_requested_quits_client() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    let redraw = app.handle_server_event(
        crate::protocol::ServerEvent::SessionCloseRequested {
            reason: "Stopped by coordinator coord".to_string(),
        },
        &mut remote,
    );

    assert!(redraw);
    assert!(app.should_quit);
    let last = app
        .display_messages()
        .last()
        .expect("close message displayed");
    assert!(
        last.content
            .contains("Session close requested by coordinator")
    );
}

#[test]
fn test_handle_server_event_session_renamed_updates_remote_title() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    app.is_remote = true;
    app.remote_session_id = Some("session_remote_rename".to_string());
    app.session.title = Some("Generated title".to_string());

    let redraw = app.handle_server_event(
        crate::protocol::ServerEvent::SessionRenamed {
            session_id: "session_remote_rename".to_string(),
            title: Some("Release planning".to_string()),
            display_title: "Release planning".to_string(),
        },
        &mut remote,
    );

    assert!(redraw);
    assert_eq!(
        app.session.custom_title.as_deref(),
        Some("Release planning")
    );
    assert_eq!(app.session.display_title(), Some("Release planning"));
    assert!(app.display_messages().iter().any(|message| {
        message
            .content
            .contains("Renamed session to Release planning")
    }));
}

#[test]
fn test_handle_server_event_history_clears_connection_type_on_session_change_when_missing() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.remote_session_id = Some("session_old".to_string());
    app.connection_type = Some("websocket".to_string());

    app.handle_server_event(
        crate::protocol::ServerEvent::History {
            id: 1,
            session_id: "session_new".to_string(),
            messages: vec![],
            images: vec![],
            provider_name: Some("claude".to_string()),
            provider_model: Some("claude-sonnet-4-20250514".to_string()),
            subagent_model: None,
            autoreview_enabled: None,
            autojudge_enabled: None,
            available_models: vec![],
            available_model_routes: vec![],
            mcp_servers: vec![],
            skills: vec![],
            total_tokens: None,
            token_usage_totals: None,
            all_sessions: vec![],
            client_count: None,
            is_canary: None,
            reload_recovery: None,
            server_version: None,
            server_name: None,
            server_icon: None,
            server_has_update: None,
            was_interrupted: None,
            connection_type: None,
            status_detail: None,
            upstream_provider: None,
            resolved_credential: None,
            reasoning_effort: None,
            service_tier: None,
            compaction_mode: crate::config::CompactionMode::Reactive,
            activity: None,
            side_panel: crate::side_panel::SidePanelSnapshot::default(),
        },
        &mut remote,
    );

    assert_eq!(app.remote_session_id.as_deref(), Some("session_new"));
    assert_eq!(app.connection_type, None);
}

#[test]
fn test_handle_server_event_history_preserves_connection_type_for_same_session_when_missing() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.remote_session_id = Some("session_same".to_string());
    app.connection_type = Some("websocket".to_string());

    app.handle_server_event(
        crate::protocol::ServerEvent::History {
            id: 1,
            session_id: "session_same".to_string(),
            messages: vec![],
            images: vec![],
            provider_name: Some("claude".to_string()),
            provider_model: Some("claude-sonnet-4-20250514".to_string()),
            subagent_model: None,
            autoreview_enabled: None,
            autojudge_enabled: None,
            available_models: vec![],
            available_model_routes: vec![],
            mcp_servers: vec![],
            skills: vec![],
            total_tokens: None,
            token_usage_totals: None,
            all_sessions: vec![],
            client_count: None,
            is_canary: None,
            reload_recovery: None,
            server_version: None,
            server_name: None,
            server_icon: None,
            server_has_update: None,
            was_interrupted: None,
            connection_type: None,
            status_detail: None,
            upstream_provider: None,
            resolved_credential: None,
            reasoning_effort: None,
            service_tier: None,
            compaction_mode: crate::config::CompactionMode::Reactive,
            activity: None,
            side_panel: crate::side_panel::SidePanelSnapshot::default(),
        },
        &mut remote,
    );

    assert_eq!(app.remote_session_id.as_deref(), Some("session_same"));
    assert_eq!(app.connection_type.as_deref(), Some("websocket"));
}

#[test]
fn test_handle_server_event_history_session_change_clears_streaming_preview_diagram() {
    // Regression pin: a mermaid preview registered mid-stream (via
    // set_streaming_preview_diagram from the streaming markdown render) must
    // not leak into a different session when a session-changing History event
    // arrives while the stream is still in flight. The History handler's
    // session_changed branch (remote/server_events.rs) calls
    // clear_streaming_render_state() (app/input.rs), which clears the
    // streaming preview slot.
    //
    // Serialize with the other tests that mutate the process-global mermaid
    // preview slot / ACTIVE_DIAGRAMS registry.
    let _render_lock = scroll_render_test_lock();
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.remote_session_id = Some("session_old".to_string());
    // Simulate a mid-stream turn: in-flight streaming text and processing on.
    app.streaming.streaming_text = "```mermaid\ngraph TD; A-->B\n```".to_string();
    app.is_processing = true;
    // The streaming markdown renderer registered a preview for the in-flight
    // fenced block (markdown_render_full.rs set_streaming_preview_diagram).
    let preview_hash: u64 = 0xDEAD_BEEF_5EAF_0001;
    crate::tui::mermaid::set_streaming_preview_diagram(
        preview_hash,
        320,
        240,
        Some("stream-preview".to_string()),
    );
    assert!(
        crate::tui::mermaid::get_active_diagrams()
            .iter()
            .any(|d| d.hash == preview_hash),
        "test setup: streaming preview should be visible before the History event"
    );

    app.handle_server_event(
        crate::protocol::ServerEvent::History {
            id: 1,
            session_id: "session_new".to_string(),
            messages: vec![],
            images: vec![],
            provider_name: Some("claude".to_string()),
            provider_model: Some("claude-sonnet-4-20250514".to_string()),
            subagent_model: None,
            autoreview_enabled: None,
            autojudge_enabled: None,
            available_models: vec![],
            available_model_routes: vec![],
            mcp_servers: vec![],
            skills: vec![],
            total_tokens: None,
            token_usage_totals: None,
            all_sessions: vec![],
            client_count: None,
            is_canary: None,
            reload_recovery: None,
            server_version: None,
            server_name: None,
            server_icon: None,
            server_has_update: None,
            was_interrupted: None,
            connection_type: None,
            status_detail: None,
            upstream_provider: None,
            resolved_credential: None,
            reasoning_effort: None,
            service_tier: None,
            compaction_mode: crate::config::CompactionMode::Reactive,
            activity: None,
            side_panel: crate::side_panel::SidePanelSnapshot::default(),
        },
        &mut remote,
    );

    assert_eq!(app.remote_session_id.as_deref(), Some("session_new"));
    assert!(
        app.streaming.streaming_text.is_empty(),
        "session-changing History must drop in-flight streaming text"
    );
    assert!(
        !crate::tui::mermaid::get_active_diagrams()
            .iter()
            .any(|d| d.hash == preview_hash),
        "streaming preview diagram leaked across a session-changing History event"
    );
}

#[test]
fn test_handle_server_event_history_same_session_rewind_reapply_clears_streaming_preview_diagram() {
    // Regression pin: a remote /rewind (or /rewind undo) triggers a History
    // redelivery for the SAME session id, so the session_changed branch of the
    // History handler (and its clear_streaming_render_state call) never runs.
    // The forced re-apply path (replace_display_messages) must still drop any
    // registered streaming preview diagram, otherwise a preview registered
    // mid-stream keeps rendering a mermaid block from a message that was just
    // rewound away (it sits at index 0 of get_active_diagrams in Margin mode).
    //
    // Serialize with the other tests that mutate the process-global mermaid
    // preview slot / ACTIVE_DIAGRAMS registry.
    let _render_lock = scroll_render_test_lock();
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.remote_session_id = Some("session_rewind_preview".to_string());
    remote.set_session_id("session_rewind_preview".to_string());
    // A stale streaming preview is still registered (e.g. the turn ended via a
    // path that did not clear it, or the rewind raced the end of the stream).
    let preview_hash: u64 = 0xDEAD_BEEF_5EAF_0002;
    crate::tui::mermaid::set_streaming_preview_diagram(
        preview_hash,
        320,
        240,
        Some("stream-preview".to_string()),
    );
    assert!(
        crate::tui::mermaid::get_active_diagrams()
            .iter()
            .any(|d| d.hash == preview_hash),
        "test setup: streaming preview should be visible before the History event"
    );
    // The client-side /rewind path arms a pending notice before the server's
    // History redelivery arrives (remote/key_handling.rs).
    app.pending_remote_rewind_notice =
        Some(crate::tui::app::PendingRemoteRewindNotice {
            undo: false,
            message_index: Some(1),
            changed_messages: 2,
        });

    // Truncated payload after the rewind: same session id, fewer messages.
    app.handle_server_event(
        crate::protocol::ServerEvent::History {
            id: 2,
            session_id: "session_rewind_preview".to_string(),
            messages: vec![crate::protocol::HistoryMessage {
                role: "user".to_string(),
                content: "first message kept by the rewind".to_string(),
                tool_calls: None,
                tool_data: None,
            }],
            images: vec![],
            provider_name: Some("claude".to_string()),
            provider_model: Some("claude-sonnet-4-20250514".to_string()),
            subagent_model: None,
            autoreview_enabled: None,
            autojudge_enabled: None,
            available_models: vec![],
            available_model_routes: vec![],
            mcp_servers: vec![],
            skills: vec![],
            total_tokens: None,
            token_usage_totals: None,
            all_sessions: vec![],
            client_count: None,
            is_canary: None,
            reload_recovery: None,
            server_version: None,
            server_name: None,
            server_icon: None,
            server_has_update: None,
            was_interrupted: None,
            connection_type: None,
            status_detail: None,
            upstream_provider: None,
            resolved_credential: None,
            reasoning_effort: None,
            service_tier: None,
            compaction_mode: crate::config::CompactionMode::Reactive,
            activity: None,
            side_panel: crate::side_panel::SidePanelSnapshot::default(),
        },
        &mut remote,
    );

    // Same session id: this exercised the !session_changed re-apply path.
    assert_eq!(
        app.remote_session_id.as_deref(),
        Some("session_rewind_preview")
    );
    // Payload was applied (transcript rebuilt + rewind notice consumed).
    assert!(
        app.display_messages()
            .iter()
            .any(|m| m.content.contains("first message kept by the rewind")),
        "rewind-truncated History payload must be re-applied for the same session"
    );
    assert!(
        app.pending_remote_rewind_notice.is_none(),
        "pending rewind notice should be consumed by the History re-apply"
    );
    assert!(
        !crate::tui::mermaid::get_active_diagrams()
            .iter()
            .any(|d| d.hash == preview_hash),
        "streaming preview diagram leaked across a same-session rewind History re-apply"
    );
}

#[test]
fn test_handle_server_event_history_same_session_midstream_duplicate_is_dropped_and_keeps_preview() {
    // Multi-client rewind fan-out pin (server side has NO fan-out: a /rewind
    // History redelivery is written only to the rewinding connection's socket,
    // per-client event channel, server/client_lifecycle.rs:521 and
    // client_state.rs handle_get_history). This test pins the CLIENT side of
    // that contract: if a mid-stream client (is_processing=true, bootstrap
    // already complete: has_loaded_history=true) ever receives a same-session
    // History payload it did not request, the payload must be DROPPED
    // (server_events.rs should_apply_history_payload gate), the local
    // transcript must stay intact, and the live streaming preview diagram must
    // NOT be cleared (the c7612068 preview-clear only runs on the forced
    // re-apply path, which only the rewinding client arms by resetting
    // has_loaded_history in backend.rs rewind()/rewind_undo()).
    //
    // Serialize with the other tests that mutate the process-global mermaid
    // preview slot / ACTIVE_DIAGRAMS registry.
    let _render_lock = scroll_render_test_lock();
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.remote_session_id = Some("session_midstream_dup".to_string());
    remote.set_session_id("session_midstream_dup".to_string());
    remote.mark_history_loaded();

    // Mid-stream state on this (non-rewinding) client.
    app.push_display_message(DisplayMessage::user("kept local message".to_string()));
    app.streaming.streaming_text = "```mermaid\ngraph TD; A-->B\n```".to_string();
    app.is_processing = true;
    let preview_hash: u64 = 0xDEAD_BEEF_5EAF_0003;
    crate::tui::mermaid::set_streaming_preview_diagram(
        preview_hash,
        320,
        240,
        Some("stream-preview".to_string()),
    );

    // Same-session, rewind-truncated payload this client never requested.
    app.handle_server_event(
        crate::protocol::ServerEvent::History {
            id: 3,
            session_id: "session_midstream_dup".to_string(),
            messages: vec![crate::protocol::HistoryMessage {
                role: "user".to_string(),
                content: "truncated payload from another client's rewind".to_string(),
                tool_calls: None,
                tool_data: None,
            }],
            images: vec![],
            provider_name: Some("claude".to_string()),
            provider_model: Some("claude-sonnet-4-20250514".to_string()),
            subagent_model: None,
            autoreview_enabled: None,
            autojudge_enabled: None,
            available_models: vec![],
            available_model_routes: vec![],
            mcp_servers: vec![],
            skills: vec![],
            total_tokens: None,
            token_usage_totals: None,
            all_sessions: vec![],
            client_count: None,
            is_canary: None,
            reload_recovery: None,
            server_version: None,
            server_name: None,
            server_icon: None,
            server_has_update: None,
            was_interrupted: None,
            connection_type: None,
            status_detail: None,
            upstream_provider: None,
            resolved_credential: None,
            reasoning_effort: None,
            service_tier: None,
            compaction_mode: crate::config::CompactionMode::Reactive,
            activity: None,
            side_panel: crate::side_panel::SidePanelSnapshot::default(),
        },
        &mut remote,
    );

    // Payload dropped: local transcript untouched, unsolicited truncation not applied.
    assert!(
        app.display_messages()
            .iter()
            .any(|m| m.content.contains("kept local message")),
        "unsolicited same-session History must not replace a bootstrapped mid-stream transcript"
    );
    assert!(
        !app.display_messages()
            .iter()
            .any(|m| m.content.contains("truncated payload from another client's rewind")),
        "unsolicited same-session History payload should be dropped, not applied"
    );
    // Live stream state preserved: preview and streaming text survive.
    //
    // The preview slot is process-global and several event paths in OTHER
    // tests clear it without the render lock (Done handlers call
    // clear_streaming_render_state). If the slot looks cleared, re-run the
    // set -> handle_server_event -> check sequence: a genuine regression in
    // should_apply_history_payload clears the preview deterministically on
    // every iteration, while a parallel wipe is transient.
    let preview_survives = || {
        crate::tui::mermaid::get_active_diagrams()
            .iter()
            .any(|d| d.hash == preview_hash)
    };
    let mut survived = preview_survives();
    for _ in 0..50 {
        if survived {
            break;
        }
        crate::tui::mermaid::set_streaming_preview_diagram(
            preview_hash,
            320,
            240,
            Some("stream-preview".to_string()),
        );
        app.handle_server_event(
            crate::protocol::ServerEvent::History {
                id: 3,
                session_id: "session_midstream_dup".to_string(),
                messages: vec![crate::protocol::HistoryMessage {
                    role: "user".to_string(),
                    content: "truncated payload from another client's rewind".to_string(),
                    tool_calls: None,
                    tool_data: None,
                }],
                images: vec![],
                provider_name: Some("claude".to_string()),
                provider_model: Some("claude-sonnet-4-20250514".to_string()),
                subagent_model: None,
                autoreview_enabled: None,
                autojudge_enabled: None,
                available_models: vec![],
                available_model_routes: vec![],
                mcp_servers: vec![],
                skills: vec![],
                total_tokens: None,
                token_usage_totals: None,
                all_sessions: vec![],
                client_count: None,
                is_canary: None,
                reload_recovery: None,
                server_version: None,
                server_name: None,
                server_icon: None,
                server_has_update: None,
                was_interrupted: None,
                connection_type: None,
                status_detail: None,
                upstream_provider: None,
                resolved_credential: None,
                reasoning_effort: None,
                service_tier: None,
                compaction_mode: crate::config::CompactionMode::Reactive,
                activity: None,
                side_panel: crate::side_panel::SidePanelSnapshot::default(),
            },
            &mut remote,
        );
        survived = preview_survives();
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    assert!(
        survived,
        "a dropped duplicate History must not clear a live streaming preview diagram"
    );
    assert!(
        !app.streaming.streaming_text.is_empty(),
        "a dropped duplicate History must not clear in-flight streaming text"
    );
    // Cleanup the global preview slot for other tests.
    crate::tui::mermaid::clear_streaming_preview_diagram();
}

#[test]
fn test_handle_server_event_history_same_session_rewind_then_late_done_does_not_resurrect_content()
{
    // Regression pin for the Done-vs-History writer ordering race: a remote
    // /rewind makes the server write the truncated History payload DIRECTLY to
    // the socket (client_lifecycle.rs handle_get_history), while a `Done` from
    // the just-finished turn can still be queued in the per-client mpsc event
    // forwarder. The client can therefore apply the rewind-truncated History
    // FIRST and process the stale Done SECOND. The Done handler flushes
    // stream_buffer and commits any non-empty streaming_text as an assistant
    // message plus a turn footer, resurrecting content that was just rewound
    // away. The same-session force-reapply path must drop stale streaming
    // state (text, buffered ops, tool cards) when a rewind notice is pending
    // so the late Done settles the turn without appending anything.
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.remote_session_id = Some("session_rewind_done_race".to_string());
    remote.set_session_id("session_rewind_done_race".to_string());
    // Stale streaming state left over from the turn whose Done is still queued
    // behind the History payload.
    app.is_processing = true;
    app.status = ProcessingStatus::Streaming;
    app.current_message_id = Some(7);
    app.streaming.streaming_text = "rewound-away assistant text".to_string();
    app.streaming_tool_calls.push(crate::message::ToolCall {
        id: "tool_stale".to_string(),
        name: "bash".to_string(),
        input: serde_json::json!({}),
        intent: None,
        thought_signature: None,
    });
    // The client-side /rewind path arms the pending notice before the server's
    // History redelivery arrives (remote/key_handling.rs).
    app.pending_remote_rewind_notice = Some(crate::tui::app::PendingRemoteRewindNotice {
        undo: false,
        message_index: Some(1),
        changed_messages: 2,
    });

    // Truncated payload after the rewind: same session id, fewer messages.
    app.handle_server_event(
        crate::protocol::ServerEvent::History {
            id: 2,
            session_id: "session_rewind_done_race".to_string(),
            messages: vec![crate::protocol::HistoryMessage {
                role: "user".to_string(),
                content: "first message kept by the rewind".to_string(),
                tool_calls: None,
                tool_data: None,
            }],
            images: vec![],
            provider_name: Some("claude".to_string()),
            provider_model: Some("claude-sonnet-4-20250514".to_string()),
            subagent_model: None,
            autoreview_enabled: None,
            autojudge_enabled: None,
            available_models: vec![],
            available_model_routes: vec![],
            mcp_servers: vec![],
            skills: vec![],
            total_tokens: None,
            token_usage_totals: None,
            all_sessions: vec![],
            client_count: None,
            is_canary: None,
            reload_recovery: None,
            server_version: None,
            server_name: None,
            server_icon: None,
            server_has_update: None,
            was_interrupted: None,
            connection_type: None,
            status_detail: None,
            upstream_provider: None,
            resolved_credential: None,
            reasoning_effort: None,
            service_tier: None,
            compaction_mode: crate::config::CompactionMode::Reactive,
            activity: None,
            side_panel: crate::side_panel::SidePanelSnapshot::default(),
        },
        &mut remote,
    );

    // The re-apply must have dropped the stale streaming state.
    assert!(
        app.streaming.streaming_text.is_empty(),
        "same-session rewind re-apply must drop buffered streaming text"
    );
    assert!(
        app.streaming_tool_calls.is_empty(),
        "same-session rewind re-apply must drop stale streaming tool cards"
    );
    assert!(
        app.pending_remote_rewind_notice.is_none(),
        "pending rewind notice should be consumed by the History re-apply"
    );
    let transcript_len_after_history = app.display_messages().len();

    // The stale Done from the rewound-away turn arrives AFTER the truncated
    // History (mpsc forwarder ordering).
    app.handle_server_event(
        crate::protocol::ServerEvent::Done { id: 7 },
        &mut remote,
    );

    assert!(!app.is_processing, "late Done should settle the turn");
    assert!(
        !app
            .display_messages()
            .iter()
            .any(|m| m.content.contains("rewound-away assistant text")),
        "late Done must not resurrect assistant text that the rewind removed"
    );
    assert_eq!(
        app.display_messages().len(),
        transcript_len_after_history,
        "late Done must not append messages (assistant text or turn footer) onto the rewind-truncated transcript"
    );
}

#[test]
fn test_handle_server_event_history_session_change_clears_pending_interleaves() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.remote_session_id = Some("session_old".to_string());
    app.queued_messages.push("queued later".to_string());
    app.interleave_message = Some("unsent interleave".to_string());
    app.pending_soft_interrupts = vec!["acked interleave".to_string()];
    app.pending_soft_interrupt_requests = vec![(12, "acked interleave".to_string())];

    app.handle_server_event(
        crate::protocol::ServerEvent::History {
            id: 1,
            session_id: "session_new".to_string(),
            messages: vec![],
            images: vec![],
            provider_name: Some("claude".to_string()),
            provider_model: Some("claude-sonnet-4-20250514".to_string()),
            subagent_model: None,
            autoreview_enabled: None,
            autojudge_enabled: None,
            available_models: vec![],
            available_model_routes: vec![],
            mcp_servers: vec![],
            skills: vec![],
            total_tokens: None,
            token_usage_totals: None,
            all_sessions: vec![],
            client_count: None,
            is_canary: None,
            server_version: None,
            server_name: None,
            server_icon: None,
            server_has_update: None,
            was_interrupted: None,
            reload_recovery: None,
            connection_type: None,
            status_detail: None,
            upstream_provider: None,
            resolved_credential: None,
            reasoning_effort: None,
            service_tier: None,
            compaction_mode: crate::config::CompactionMode::Reactive,
            activity: None,
            side_panel: crate::side_panel::SidePanelSnapshot::default(),
        },
        &mut remote,
    );

    assert_eq!(app.remote_session_id.as_deref(), Some("session_new"));
    assert!(app.queued_messages().is_empty());
    assert!(app.interleave_message.is_none());
    assert!(app.pending_soft_interrupts.is_empty());
    assert!(app.pending_soft_interrupt_requests.is_empty());
}

#[test]
fn test_handle_post_connect_marker_without_reload_context_does_not_queue_selfdev_continuation() {
    let _guard = crate::storage::lock_test_env();
    let temp_home = tempfile::TempDir::new().expect("create temp home");
    let prev_home = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", temp_home.path());

    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _enter = rt.enter();
    let backend = ratatui::backend::TestBackend::new(80, 24);
    let mut terminal = ratatui::Terminal::new(backend).expect("failed to create terminal");
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    remote.mark_history_loaded();

    let session_id = "session_marker_only";
    let jcode_dir = crate::storage::jcode_dir().expect("jcode dir");
    std::fs::write(
        jcode_dir.join(format!("client-reload-pending-{}", session_id)),
        "Reloaded with build test123\n",
    )
    .expect("write client reload marker");

    let mut state = super::remote::RemoteRunState {
        reconnect_attempts: 1,
        ..Default::default()
    };

    rt.block_on(super::remote::handle_post_connect(
        &mut app,
        &mut terminal,
        &mut remote,
        &mut state,
        Some(session_id),
    ))
    .expect("post connect should succeed");

    assert!(app.hidden_queued_system_messages.is_empty());
    assert!(
        !app.display_messages()
            .iter()
            .any(|m| m.content.starts_with("Reload complete - continuing")),
        "marker-only reconnect should not queue selfdev continuation"
    );
    assert!(app.reload_info.is_empty());
    assert!(
        app.display_messages()
            .iter()
            .any(|m| m.content.contains("✓ Reconnected successfully.")),
        "reconnect success message should still be shown"
    );

    if let Some(prev_home) = prev_home {
        crate::env::set_var("JCODE_HOME", prev_home);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
}

#[test]
fn test_handle_post_connect_defers_reload_followup_to_server_history_payload() {
    let _guard = crate::storage::lock_test_env();
    let temp_home = tempfile::TempDir::new().expect("create temp home");
    let prev_home = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", temp_home.path());

    let session_id = "session_hidden_reload_followup";
    crate::tool::selfdev::ReloadContext {
        task_context: Some("Investigate queued prompt delivery after reload".to_string()),
        version_before: "old-build".to_string(),
        version_after: "new-build".to_string(),
        session_id: session_id.to_string(),
        timestamp: "2026-03-26T00:00:00Z".to_string(),
    }
    .save()
    .expect("save reload context");

    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _enter = rt.enter();
    let backend = ratatui::backend::TestBackend::new(80, 24);
    let mut terminal = ratatui::Terminal::new(backend).expect("failed to create terminal");
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    let mut state = super::remote::RemoteRunState {
        reconnect_attempts: 1,
        ..Default::default()
    };

    let outcome = rt
        .block_on(super::remote::handle_post_connect(
            &mut app,
            &mut terminal,
            &mut remote,
            &mut state,
            Some(session_id),
        ))
        .expect("post connect should succeed");

    assert!(matches!(outcome, super::remote::PostConnectOutcome::Ready));
    assert!(app.hidden_queued_system_messages.is_empty());
    assert!(!app.is_processing);
    assert!(matches!(app.status, ProcessingStatus::Idle));
    assert!(app.current_message_id.is_none());
    assert!(app.rate_limit_pending_message.is_none());
    assert!(app.reload_info.is_empty());

    cleanup_reload_context_file(session_id);
    if let Some(prev_home) = prev_home {
        crate::env::set_var("JCODE_HOME", prev_home);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
}

#[test]
fn test_handle_post_connect_clears_deferred_dispatch_before_reload_followup() {
    let _guard = crate::storage::lock_test_env();
    let temp_home = tempfile::TempDir::new().expect("create temp home");
    let prev_home = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", temp_home.path());

    let session_id = "session_reload_deferred_dispatch";
    crate::tool::selfdev::ReloadContext {
        task_context: Some(
            "Verify deferred dispatch does not block reload continuation".to_string(),
        ),
        version_before: "old-build".to_string(),
        version_after: "new-build".to_string(),
        session_id: session_id.to_string(),
        timestamp: "2026-04-15T00:00:00Z".to_string(),
    }
    .save()
    .expect("save reload context");

    let mut app = create_test_app();
    app.pending_queued_dispatch = true;
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _enter = rt.enter();
    let backend = ratatui::backend::TestBackend::new(80, 24);
    let mut terminal = ratatui::Terminal::new(backend).expect("failed to create terminal");
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    remote.mark_history_loaded();

    let mut state = super::remote::RemoteRunState {
        reconnect_attempts: 1,
        ..Default::default()
    };

    let outcome = rt
        .block_on(super::remote::handle_post_connect(
            &mut app,
            &mut terminal,
            &mut remote,
            &mut state,
            Some(session_id),
        ))
        .expect("post connect should succeed");

    assert!(matches!(outcome, super::remote::PostConnectOutcome::Ready));
    assert!(
        !app.pending_queued_dispatch,
        "post-connect should clear deferred dispatch before sending reload continuation"
    );
    assert!(app.hidden_queued_system_messages.is_empty());
    assert!(
        app.is_processing,
        "reload continuation should still dispatch"
    );
    assert!(matches!(app.status, ProcessingStatus::Sending));
    assert!(app.current_message_id.is_some());

    cleanup_reload_context_file(session_id);
    if let Some(prev_home) = prev_home {
        crate::env::set_var("JCODE_HOME", prev_home);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
}

#[test]
fn test_handle_post_connect_requests_client_reload_after_server_reload_even_without_newer_binary() {
    use std::time::{Duration, SystemTime};

    let _guard = crate::storage::lock_test_env();
    let temp_home = tempfile::TempDir::new().expect("create temp home");
    let prev_home = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", temp_home.path());

    let mut app = create_test_app();
    app.client_binary_mtime = Some(SystemTime::now() + Duration::from_secs(3600));
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _enter = rt.enter();
    let backend = ratatui::backend::TestBackend::new(80, 24);
    let mut terminal = ratatui::Terminal::new(backend).expect("failed to create terminal");
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    remote.mark_history_loaded();
    app.remote_session_id = Some("session_reload_after_reconnect".to_string());

    let mut state = super::remote::RemoteRunState {
        reconnect_attempts: 1,
        server_reload_in_progress: true,
        ..Default::default()
    };

    let outcome = rt
        .block_on(super::remote::handle_post_connect(
            &mut app,
            &mut terminal,
            &mut remote,
            &mut state,
            Some("session_reload_after_reconnect"),
        ))
        .expect("post connect should succeed");

    assert!(matches!(outcome, super::remote::PostConnectOutcome::Quit));
    assert_eq!(
        app.reload_requested.as_deref(),
        Some("session_reload_after_reconnect")
    );
    assert!(app.should_quit);

    if let Some(prev_home) = prev_home {
        crate::env::set_var("JCODE_HOME", prev_home);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
}

#[test]
fn test_handle_server_event_token_usage_uses_per_call_deltas() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.streaming.streaming_tps_collect_output = true;

    app.handle_server_event(
        crate::protocol::ServerEvent::TokenUsage {
            input: 100,
            output: 10,
            cache_read_input: None,
            cache_creation_input: None,
        },
        &mut remote,
    );
    app.handle_server_event(
        crate::protocol::ServerEvent::TokenUsage {
            input: 100,
            output: 30,
            cache_read_input: None,
            cache_creation_input: None,
        },
        &mut remote,
    );
    app.handle_server_event(
        crate::protocol::ServerEvent::TokenUsage {
            input: 100,
            output: 30,
            cache_read_input: None,
            cache_creation_input: None,
        },
        &mut remote,
    );

    assert_eq!(app.streaming.streaming_output_tokens, 30);
    assert_eq!(app.streaming.streaming_total_output_tokens, 30);
    assert_eq!(app.token_accounting.total_input_tokens, 100);
    assert_eq!(app.token_accounting.total_output_tokens, 30);
}

#[test]
fn test_handle_server_event_tool_exec_pauses_tps_but_collects_final_tool_usage() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.streaming.streaming_tps_elapsed = Duration::from_secs(2);

    app.handle_server_event(
        crate::protocol::ServerEvent::ToolStart {
            id: "tool-1".to_string(),
            name: "read".to_string(),
        },
        &mut remote,
    );

    assert!(app.streaming.streaming_tps_collect_output);
    assert!(app.streaming.streaming_tps_start.is_some());

    app.streaming.streaming_tps_start = Some(Instant::now() - Duration::from_secs(3));

    app.handle_server_event(
        crate::protocol::ServerEvent::ToolExec {
            id: "tool-1".to_string(),
            name: "read".to_string(),
        },
        &mut remote,
    );

    assert!(app.streaming.streaming_tps_collect_output);
    assert!(app.streaming.streaming_tps_start.is_none());
    assert!(app.streaming.streaming_tps_elapsed >= Duration::from_secs(5));

    app.handle_server_event(
        crate::protocol::ServerEvent::TokenUsage {
            input: 100,
            output: 25,
            cache_read_input: None,
            cache_creation_input: None,
        },
        &mut remote,
    );

    assert_eq!(app.streaming.streaming_total_output_tokens, 25);
    assert_eq!(app.streaming.streaming_tps_observed_output_tokens, 25);

    app.handle_server_event(
        crate::protocol::ServerEvent::TextDelta {
            text: "hello".to_string(),
        },
        &mut remote,
    );

    assert!(app.streaming.streaming_tps_collect_output);
    assert!(app.streaming.streaming_tps_start.is_some());
}

#[test]
fn test_handle_server_event_kv_cache_request_resets_tps_output_watermark_for_next_api_call() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.streaming.streaming_tps_collect_output = true;

    app.handle_server_event(
        crate::protocol::ServerEvent::TokenUsage {
            input: 100,
            output: 40,
            cache_read_input: None,
            cache_creation_input: None,
        },
        &mut remote,
    );

    app.handle_server_event(
        crate::protocol::ServerEvent::KvCacheRequest {
            system_static_hash: 1,
            tools_hash: 2,
            messages_hash: 3,
            message_hashes: vec![11, 22],
            message_count: 2,
            tool_count: 1,
            system_static_chars: 10,
            tools_json_chars: 20,
            messages_json_chars: 30,
            ephemeral_hash: None,
            ephemeral_chars: 0,
            ephemeral_message_count: 0,
        },
        &mut remote,
    );

    assert!(!app.streaming.streaming_tps_collect_output);

    app.handle_server_event(
        crate::protocol::ServerEvent::ConnectionPhase {
            phase: "streaming".to_string(),
        },
        &mut remote,
    );

    assert!(app.streaming.streaming_tps_collect_output);

    app.handle_server_event(
        crate::protocol::ServerEvent::TokenUsage {
            input: 120,
            output: 15,
            cache_read_input: None,
            cache_creation_input: None,
        },
        &mut remote,
    );

    assert_eq!(app.streaming.streaming_total_output_tokens, 55);
    assert_eq!(app.streaming.streaming_tps_observed_output_tokens, 55);
}

#[test]
fn test_handle_server_event_message_end_marks_stream_as_finalizing_without_stall_mode() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.is_processing = true;
    app.status = ProcessingStatus::Streaming;
    app.streaming.streaming_tps_collect_output = true;

    let needs_redraw =
        app.handle_server_event(crate::protocol::ServerEvent::MessageEnd, &mut remote);

    assert!(needs_redraw);
    assert!(app.stream_message_ended);
    assert!(matches!(app.status, ProcessingStatus::Streaming));
    assert!(app.streaming.streaming_tps_collect_output);
}

#[test]
fn test_handle_server_event_tps_connection_phase_streaming_starts_collection_only_for_streaming() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.handle_server_event(
        crate::protocol::ServerEvent::ConnectionPhase {
            phase: "waiting for response".to_string(),
        },
        &mut remote,
    );

    assert!(!app.streaming.streaming_tps_collect_output);
    assert!(app.streaming.streaming_tps_start.is_none());

    app.handle_server_event(
        crate::protocol::ServerEvent::ConnectionPhase {
            phase: "streaming".to_string(),
        },
        &mut remote,
    );

    assert!(app.streaming.streaming_tps_collect_output);
    assert!(app.streaming.streaming_tps_start.is_some());
    assert!(matches!(app.status, ProcessingStatus::Streaming));
}

#[test]
fn test_connection_phase_elapsed_resets_per_attempt_not_per_turn() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    // Simulate a long-running turn: the whole-turn timer has been ticking for
    // well over the "suspiciously long" yellow threshold.
    app.is_processing = true;
    app.processing_started = Some(Instant::now() - Duration::from_secs(120));
    assert!(crate::tui::TuiState::elapsed(&app).unwrap() > Duration::from_secs(60));

    // A later round-trip enters the connecting phase. The per-attempt timer
    // must start fresh, so it reads as a brief connect (well under 10s) instead
    // of inheriting the 120s whole-turn elapsed and rendering yellow.
    app.handle_server_event(
        crate::protocol::ServerEvent::ConnectionPhase {
            phase: "connecting".to_string(),
        },
        &mut remote,
    );

    assert!(matches!(
        app.status,
        ProcessingStatus::Connecting(crate::message::ConnectionPhase::Connecting)
    ));
    let phase_elapsed = crate::tui::TuiState::connection_phase_elapsed(&app)
        .expect("connection phase elapsed should be tracked");
    assert!(
        phase_elapsed < Duration::from_secs(5),
        "per-attempt connection elapsed should be fresh, got {:?}",
        phase_elapsed
    );

    // Sub-phase transitions within the same attempt must not restart the timer.
    let started = app.connection_phase_started;
    app.handle_server_event(
        crate::protocol::ServerEvent::ConnectionPhase {
            phase: "waiting for response".to_string(),
        },
        &mut remote,
    );
    assert_eq!(
        app.connection_phase_started, started,
        "sub-phase transitions should keep the same per-attempt start"
    );

    // Streaming clears the per-attempt timer.
    app.handle_server_event(
        crate::protocol::ServerEvent::ConnectionPhase {
            phase: "streaming".to_string(),
        },
        &mut remote,
    );
    assert!(app.connection_phase_started.is_none());
}

#[test]
fn test_handle_server_event_tps_message_end_counts_late_usage_without_timer_running() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.handle_server_event(
        crate::protocol::ServerEvent::ConnectionPhase {
            phase: "streaming".to_string(),
        },
        &mut remote,
    );
    app.streaming.streaming_tps_start = Some(Instant::now() - Duration::from_secs(4));

    app.handle_server_event(crate::protocol::ServerEvent::MessageEnd, &mut remote);

    assert!(app.streaming.streaming_tps_collect_output);
    assert!(app.streaming.streaming_tps_start.is_none());
    assert!(app.streaming.streaming_tps_elapsed >= Duration::from_secs(4));

    app.handle_server_event(
        crate::protocol::ServerEvent::TokenUsage {
            input: 100,
            output: 20,
            cache_read_input: None,
            cache_creation_input: None,
        },
        &mut remote,
    );

    assert_eq!(app.streaming.streaming_total_output_tokens, 20);
    assert_eq!(app.streaming.streaming_tps_observed_output_tokens, 20);
    assert!(app.streaming.streaming_tps_observed_elapsed >= Duration::from_secs(4));
    assert!(app.streaming.streaming_tps_start.is_none());
}

#[test]
fn test_handle_server_event_tps_redundant_late_usage_after_message_end_does_not_double_count() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.handle_server_event(
        crate::protocol::ServerEvent::ConnectionPhase {
            phase: "streaming".to_string(),
        },
        &mut remote,
    );
    app.streaming.streaming_tps_start = Some(Instant::now() - Duration::from_secs(5));

    app.handle_server_event(
        crate::protocol::ServerEvent::TokenUsage {
            input: 100,
            output: 10,
            cache_read_input: None,
            cache_creation_input: None,
        },
        &mut remote,
    );
    app.handle_server_event(crate::protocol::ServerEvent::MessageEnd, &mut remote);
    app.handle_server_event(
        crate::protocol::ServerEvent::TokenUsage {
            input: 100,
            output: 30,
            cache_read_input: None,
            cache_creation_input: None,
        },
        &mut remote,
    );
    app.handle_server_event(
        crate::protocol::ServerEvent::TokenUsage {
            input: 100,
            output: 30,
            cache_read_input: None,
            cache_creation_input: None,
        },
        &mut remote,
    );

    assert_eq!(app.streaming.streaming_total_output_tokens, 30);
    assert_eq!(app.streaming.streaming_tps_observed_output_tokens, 30);
    assert_eq!(*remote.call_output_tokens_seen(), 30);
}

#[test]
fn test_handle_server_event_interrupted_clears_stream_state_and_sets_idle() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.is_processing = true;
    app.status = ProcessingStatus::Streaming;
    app.processing_started = Some(Instant::now());
    app.current_message_id = Some(42);
    app.streaming.streaming_text = "partial".to_string();
    app.streaming_tool_calls.push(crate::message::ToolCall {
        id: "tool_1".to_string(),
        name: "bash".to_string(),
        input: serde_json::Value::Null,
        intent: None, thought_signature: None, });
    app.interleave_message = Some("queued interrupt".to_string());
    app.pending_soft_interrupts
        .push("pending soft interrupt".to_string());
    app.pending_soft_interrupt_requests
        .push((77, "pending soft interrupt".to_string()));

    remote.handle_tool_start("tool_1", "bash");
    remote.handle_tool_input("{\"command\":\"sleep 10\"}");
    remote.handle_tool_exec("tool_1", "edit");

    app.handle_server_event(crate::protocol::ServerEvent::Interrupted, &mut remote);

    assert!(!app.is_processing);
    assert!(matches!(app.status, ProcessingStatus::Idle));
    assert!(app.processing_started.is_none());
    assert!(app.current_message_id.is_none());
    assert!(app.streaming.streaming_text.is_empty());
    assert!(app.streaming_tool_calls.is_empty());
    assert!(app.interleave_message.is_none());
    assert_eq!(app.queued_messages(), &["queued interrupt"]);
    assert_eq!(app.pending_soft_interrupts, vec!["pending soft interrupt"]);
    assert_eq!(
        app.pending_soft_interrupt_requests,
        vec![(77, "pending soft interrupt".to_string())]
    );

    let last = app
        .display_messages()
        .last()
        .expect("missing interrupted message");
    assert_eq!(last.role, "system");
    assert_eq!(last.content, "Interrupted");
}

#[test]
fn test_remote_interrupted_defers_queued_followup_dispatch_by_one_cycle() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    remote.mark_history_loaded();

    app.is_processing = true;
    app.status = ProcessingStatus::Streaming;
    app.current_message_id = Some(42);
    app.queued_messages.push("queued later".to_string());

    app.handle_server_event(crate::protocol::ServerEvent::Interrupted, &mut remote);

    assert!(app.pending_queued_dispatch);
    assert_eq!(app.queued_messages(), &["queued later"]);
    assert!(!app.is_processing);

    rt.block_on(remote::process_remote_followups(&mut app, &mut remote));
    assert_eq!(app.queued_messages(), &["queued later"]);
    assert!(!app.is_processing);

    app.pending_queued_dispatch = false;
    rt.block_on(remote::process_remote_followups(&mut app, &mut remote));
    assert!(app.queued_messages().is_empty());
    assert!(app.is_processing);
    assert!(matches!(app.status, ProcessingStatus::Sending));
    assert!(app.current_message_id.is_some());
}

#[test]
fn test_remote_interrupted_recovers_pending_interleaves_in_order() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    remote.mark_history_loaded();

    app.is_processing = true;
    app.status = ProcessingStatus::Streaming;
    app.current_message_id = Some(42);
    app.interleave_message = Some("unsent interleave".to_string());
    app.pending_soft_interrupts = vec!["acked interleave".to_string()];
    app.pending_soft_interrupt_requests = vec![(55, "acked interleave".to_string())];
    app.queued_messages.push("queued later".to_string());

    app.handle_server_event(crate::protocol::ServerEvent::Interrupted, &mut remote);

    assert!(app.pending_queued_dispatch);
    assert_eq!(
        app.queued_messages(),
        &["unsent interleave", "queued later"]
    );
    assert_eq!(app.pending_soft_interrupts, vec!["acked interleave"]);

    rt.block_on(remote::process_remote_followups(&mut app, &mut remote));
    assert!(app.pending_soft_interrupts.is_empty());
    assert!(app.pending_soft_interrupt_requests.is_empty());
    assert_eq!(
        app.queued_messages(),
        &["acked interleave", "unsent interleave", "queued later"]
    );
    assert!(!app.is_processing);

    app.pending_queued_dispatch = false;
    rt.block_on(remote::process_remote_followups(&mut app, &mut remote));

    assert!(app.pending_soft_interrupts.is_empty());
    assert!(app.pending_soft_interrupt_requests.is_empty());
    assert!(app.queued_messages().is_empty());
    assert!(app.is_processing);
    assert!(matches!(app.status, ProcessingStatus::Sending));

    let user_messages: Vec<&str> = app
        .display_messages()
        .iter()
        .filter(|msg| msg.role == "user")
        .map(|msg| msg.content.as_str())
        .collect();
    assert_eq!(
        user_messages,
        vec!["acked interleave", "unsent interleave", "queued later"]
    );
}

#[test]
fn test_remote_done_recovers_stranded_soft_interrupt_as_queued_followup() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    remote.mark_history_loaded();

    app.is_processing = true;
    app.status = ProcessingStatus::Streaming;
    app.current_message_id = Some(42);
    app.pending_soft_interrupts = vec!["late interleave".to_string()];
    app.pending_soft_interrupt_requests = vec![(55, "late interleave".to_string())];
    app.queued_messages.push("queued later".to_string());

    app.handle_server_event(crate::protocol::ServerEvent::Done { id: 42 }, &mut remote);

    assert!(!app.is_processing);
    assert_eq!(app.pending_soft_interrupts, vec!["late interleave"]);
    assert_eq!(
        app.pending_soft_interrupt_requests,
        vec![(55, "late interleave".to_string())]
    );
    assert_eq!(app.queued_messages(), &["queued later"]);

    rt.block_on(remote::process_remote_followups(&mut app, &mut remote));

    assert!(app.pending_soft_interrupts.is_empty());
    assert!(app.pending_soft_interrupt_requests.is_empty());
    assert!(app.queued_messages().is_empty());
    assert!(app.is_processing);
    assert!(matches!(app.status, ProcessingStatus::Sending));
    assert!(app.current_message_id.is_some());

    let user_messages: Vec<&str> = app
        .display_messages()
        .iter()
        .filter(|msg| msg.role == "user")
        .map(|msg| msg.content.as_str())
        .collect();
    assert_eq!(user_messages, vec!["late interleave", "queued later"]);
}

#[test]
fn test_remote_done_auto_pokes_again_when_todos_remain() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        let mut remote = crate::tui::backend::RemoteConnection::dummy();

        crate::todo::save_todos(
            &app.session.id,
            &[crate::todo::TodoItem {
                group: None,
                id: "todo-1".to_string(),
                content: "Continue working".to_string(),
                status: "pending".to_string(),
                priority: "high".to_string(),
                blocked_by: Vec::new(),
                assigned_to: None,
                confidence: None,
                completion_confidence: None,
                confidence_history: Vec::new(),
            }],
        )
        .expect("save todos");

        app.is_remote = true;
        app.auto_poke_incomplete_todos = true;
        app.is_processing = true;
        app.status = ProcessingStatus::Streaming;
        app.current_message_id = Some(42);

        let needs_redraw =
            app.handle_server_event(crate::protocol::ServerEvent::Done { id: 42 }, &mut remote);

        assert!(needs_redraw);
        assert!(app.pending_queued_dispatch);
        assert_eq!(app.queued_messages().len(), 1);
        assert!(app.queued_messages()[0].contains("Continue working, or update the todo tool."));
    });
}

#[test]
fn test_handle_server_event_side_pane_images_populates_pane_live() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.is_remote = true;
    app.side_panel = crate::side_panel::SidePanelSnapshot::default();
    app.remote_session_id = Some("session_active".to_string());
    assert!(app.remote_side_pane_images.is_empty());

    let needs_redraw = app.handle_server_event(
        crate::protocol::ServerEvent::SidePaneImages {
            session_id: "session_active".to_string(),
            images: vec![crate::session::RenderedImage {
                media_type: "image/png".to_string(),
                data: "image-data".to_string(),
                label: Some("openclaw.png".to_string()),
                source: crate::session::RenderedImageSource::ToolResult {
                    tool_name: "read".to_string(),
                },
                anchor: None,
            }],
        },
        &mut remote,
    );

    assert!(needs_redraw, "live side-pane image should request a redraw");
    assert_eq!(app.remote_side_pane_images.len(), 1);
    // Images render inline in the transcript now, so a live image must not flip
    // the side panel or arm the old auto-hide timer.
    assert!(!app.side_panel_user_hidden);
    assert!(<App as crate::tui::TuiState>::pin_images(&app));
    assert!(app.pinned_images_auto_hide_deadline.is_none());
}

#[test]
fn test_native_generated_image_renders_inline_without_opening_side_panel() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.is_remote = true;
    app.side_panel = crate::side_panel::SidePanelSnapshot::default();
    app.remote_session_id = Some("session_active".to_string());

    let generated_redraw = app.handle_server_event(
        crate::protocol::ServerEvent::GeneratedImage {
            id: "image_call_123".to_string(),
            path: "/tmp/generated.png".to_string(),
            metadata_path: Some("/tmp/generated.json".to_string()),
            output_format: "png".to_string(),
            revised_prompt: Some("a green square".to_string()),
        },
        &mut remote,
    );
    let image_redraw = app.handle_server_event(
        crate::protocol::ServerEvent::SidePaneImages {
            session_id: "session_active".to_string(),
            images: vec![crate::session::RenderedImage {
                media_type: "image/png".to_string(),
                data: "image-data".to_string(),
                label: Some("/tmp/generated.png".to_string()),
                source: crate::session::RenderedImageSource::ToolResult {
                    tool_name: crate::message::GENERATED_IMAGE_TOOL_NAME.to_string(),
                },
                anchor: Some(crate::session::RenderedImageAnchor::ToolCall {
                    id: "image_call_123".to_string(),
                }),
            }],
        },
        &mut remote,
    );

    assert!(generated_redraw);
    assert!(image_redraw);
    assert!(!app.side_panel.has_pages());
    assert_eq!(app.remote_side_pane_images.len(), 1);
    assert_eq!(
        app.remote_side_pane_images[0].anchor,
        Some(crate::session::RenderedImageAnchor::ToolCall {
            id: "image_call_123".to_string(),
        })
    );
    let generated_row = app
        .display_messages
        .iter()
        .find(|message| {
            message
                .tool_data
                .as_ref()
                .is_some_and(|tool| tool.id == "image_call_123")
        })
        .expect("generated image tool row");
    assert_eq!(generated_row.title.as_deref(), Some("Generated image"));
}

#[test]
fn test_handle_server_event_side_pane_images_ignores_inactive_session() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.is_remote = true;
    app.remote_session_id = Some("session_active".to_string());

    let needs_redraw = app.handle_server_event(
        crate::protocol::ServerEvent::SidePaneImages {
            session_id: "session_other".to_string(),
            images: vec![crate::session::RenderedImage {
                media_type: "image/png".to_string(),
                data: "image-data".to_string(),
                label: None,
                source: crate::session::RenderedImageSource::ToolResult {
                    tool_name: "read".to_string(),
                },
                anchor: None,
            }],
        },
        &mut remote,
    );

    assert!(!needs_redraw);
    assert!(app.remote_side_pane_images.is_empty());
}

#[test]
fn test_handle_server_event_mcp_status_updates_tools_without_status_notice() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.handle_server_event(
        crate::protocol::ServerEvent::McpStatus {
            servers: vec!["agentcard:8".to_string()],
        },
        &mut remote,
    );

    assert_eq!(app.mcp_server_names, vec![("agentcard".to_string(), 8)]);
    assert_eq!(app.status_notice(), None);
}

#[test]
fn test_handle_server_event_reasoning_delta_shows_thinking_status() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.is_processing = true;
    // Server emits ConnectionPhase::Streaming when reasoning starts (to kick the
    // TPS timer), so the status arrives as Streaming.
    app.status = ProcessingStatus::Streaming;

    app.handle_server_event(
        crate::protocol::ServerEvent::ReasoningDelta {
            text: "weighing options".to_string(),
        },
        &mut remote,
    );

    // Live reasoning should read as "thinking", not "streaming".
    assert!(matches!(app.status, ProcessingStatus::Thinking(_)));

    // Real output text flips the status back to streaming.
    app.handle_server_event(
        crate::protocol::ServerEvent::TextDelta {
            text: "Here is the answer".to_string(),
        },
        &mut remote,
    );
    assert!(matches!(app.status, ProcessingStatus::Streaming));
}

#[test]
fn test_handle_server_event_reasoning_delta_keeps_tool_status() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.is_processing = true;
    app.status = ProcessingStatus::RunningTool("bash".to_string());

    app.handle_server_event(
        crate::protocol::ServerEvent::ReasoningDelta {
            text: "post-tool reflection".to_string(),
        },
        &mut remote,
    );

    // A running tool must not be masked by reasoning text.
    assert!(matches!(app.status, ProcessingStatus::RunningTool(_)));
}

#[test]
fn test_pending_startup_notice_survives_history_bootstrap_for_fresh_session() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    // A fresh client has no remote session yet; the startup notice card is
    // pushed before the History bootstrap arrives.
    app.remote_session_id = None;
    app.set_pending_startup_notice("Launch hotkeys", "cmd+; -> home\ncmd+' -> last project");
    assert!(
        app.display_messages()
            .iter()
            .any(|m| m.content.contains("cmd+;")),
        "card should be visible before bootstrap"
    );

    // The bootstrap for a brand-new session clears the transcript.
    app.handle_server_event(
        crate::protocol::ServerEvent::History {
            id: 1,
            session_id: "session_new".to_string(),
            messages: vec![],
            images: vec![],
            provider_name: Some("claude".to_string()),
            provider_model: Some("claude-sonnet-4-20250514".to_string()),
            subagent_model: None,
            autoreview_enabled: None,
            autojudge_enabled: None,
            available_models: vec![],
            available_model_routes: vec![],
            mcp_servers: vec![],
            skills: vec![],
            total_tokens: None,
            token_usage_totals: None,
            all_sessions: vec![],
            client_count: None,
            is_canary: None,
            reload_recovery: None,
            server_version: None,
            server_name: None,
            server_icon: None,
            server_has_update: None,
            was_interrupted: None,
            connection_type: None,
            status_detail: None,
            upstream_provider: None,
            resolved_credential: None,
            reasoning_effort: None,
            service_tier: None,
            compaction_mode: crate::config::CompactionMode::Reactive,
            activity: None,
            side_panel: crate::side_panel::SidePanelSnapshot::default(),
        },
        &mut remote,
    );

    // The card must still be present on the idle screen after the bootstrap.
    let card_count = app
        .display_messages()
        .iter()
        .filter(|m| m.content.contains("cmd+;"))
        .count();
    assert_eq!(
        card_count, 1,
        "startup notice should be re-applied exactly once after bootstrap"
    );
}
