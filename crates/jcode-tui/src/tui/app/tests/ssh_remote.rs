fn with_ssh_remote_test_home(f: impl FnOnce()) {
    with_temp_jcode_home(|| {
        struct RestoreEnv(Vec<(&'static str, Option<std::ffi::OsString>)>);
        impl Drop for RestoreEnv {
            fn drop(&mut self) {
                for (key, value) in self.0.drain(..) {
                    if let Some(value) = value {
                        crate::env::set_var(key, value);
                    } else {
                        crate::env::remove_var(key);
                    }
                }
            }
        }
        let _restore = RestoreEnv(
            ["JCODE_SSH_REMOTE", "JCODE_SOCKET", "JCODE_SELFDEV"]
                .into_iter()
                .map(|key| (key, std::env::var_os(key)))
                .collect(),
        );
        crate::env::set_var("JCODE_SSH_REMOTE", "test-remote");
        crate::env::remove_var("JCODE_SELFDEV");
        f();
    });
}

#[test]
fn ssh_remote_startup_ignores_colliding_local_session_and_onboarding() {
    with_ssh_remote_test_home(|| {
        let mut local = crate::session::Session::create(None, None);
        local.add_message(
            crate::message::Role::User,
            vec![crate::message::ContentBlock::Text {
                text: "local-only secret".into(),
                cache_control: None,
            }],
        );
        local.save().expect("save local collision");
        let app = App::new_for_remote(Some(local.id.clone()));
        assert!(app.display_messages().is_empty());
        assert_eq!(app.resume_session_id.as_deref(), Some(local.id.as_str()));
        assert!(app.remote_session_id.is_none());
        assert!(!app.onboarding_welcome_active());
        assert!(app.suggestion_prompts().is_empty());
        assert!(app.onboarding_startup_checked);
        assert!(!app.auto_server_reload);
        assert_eq!(
            app.server_display_name().as_deref(),
            Some("SSH test-remote")
        );
        assert!(super::prompt_history::history_file_path().is_none());
        let error = super::helpers::open_path_or_url_detached("/remote/file.md").unwrap_err();
        assert!(
            error
                .to_string()
                .contains("remote file opening is unavailable")
        );
        assert_eq!(crate::tui::subscribe_metadata(None).0, None);
        assert_eq!(
            crate::tui::subscribe_metadata(Some("/remote/project"))
                .0
                .as_deref(),
            Some("/remote/project")
        );
    });
}

#[test]
fn ssh_remote_subscribe_keeps_remote_only_id_and_filters_control_done() {
    with_ssh_remote_test_home(|| {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let dir = tempfile::tempdir().unwrap();
            let socket = dir.path().join("ssh.sock");
            crate::server::set_socket_path(socket.to_str().unwrap());
            #[allow(unused_mut)]
            let mut listener = crate::transport::Listener::bind(&socket).unwrap();
            let mut connection = crate::tui::backend::RemoteConnection::connect_with_session(
                Some("remote-only-session"), Some("ssh-client"), true, false,
                Some("/remote/project"),
            ).await.unwrap();
            let (stream, _) = listener.accept().await.unwrap();
            let (reader, mut writer) = stream.into_split();
            let mut reader = BufReader::new(reader);
            let mut line = String::new();
            reader.read_line(&mut line).await.unwrap();
            let request: crate::protocol::Request = serde_json::from_str(&line).unwrap();
            let id = match request {
                crate::protocol::Request::Subscribe {
                    id, target_session_id, client_has_local_history, continue_on_disconnect,
                    working_dir, terminal_env, ..
                } => {
                    assert_eq!(target_session_id.as_deref(), Some("remote-only-session"));
                    assert!(!client_has_local_history);
                    assert!(continue_on_disconnect);
                    assert!(terminal_env.is_empty());
                    assert_eq!(working_dir.as_deref(), Some("/remote/project"));
                    assert!(id >= 1_u64 << 62);
                    id
                }
                other => panic!("expected Subscribe, got {other:?}"),
            };
            let events = format!(
                "{{\"type\":\"done\",\"id\":{id}}}\n{{\"type\":\"done\",\"id\":{id}}}\n{{\"type\":\"text_delta\",\"text\":\"remote progress\"}}\n"
            );
            writer.write_all(events.as_bytes()).await.unwrap();
            assert!(matches!(connection.next_event().await,
                crate::tui::backend::RemoteRead::Event(crate::protocol::ServerEvent::TextDelta { text })
                if text == "remote progress"));
        });
    });
}

#[test]
fn ssh_remote_reconnect_waits_for_authoritative_history_without_local_reload() {
    with_ssh_remote_test_home(|| {
        let mut app = App::new_for_remote(Some("remote-only-session".into()));
        app.remote_session_id = Some("remote-only-session".into());
        app.push_display_message(DisplayMessage::system("stale local display"));
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let mut remote = crate::tui::backend::RemoteConnection::dummy();
            let mut state = super::remote::RemoteRunState::default();
            state.reconnect_attempts = 1;
            state.server_reload_in_progress = true;
            assert!(!super::remote::reload_handoff_active(&state));
            let mut terminal =
                ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();
            super::remote::handle_post_connect(
                &mut app,
                &mut terminal,
                &mut remote,
                &mut state,
                Some("remote-only-session"),
            )
            .await
            .unwrap();
            assert!(!app.should_quit);
            assert!(app.reload_requested.is_none());
            assert!(!remote.has_loaded_history());
            assert_eq!(
                app.remote_startup_phase,
                Some(super::RemoteStartupPhase::LoadingSession)
            );
        });
    });
}

#[test]
fn ssh_remote_history_is_authoritative_even_when_empty_or_server_version_differs() {
    with_ssh_remote_test_home(|| {
        let mut app = App::new_for_remote(Some("remote-only-session".into()));
        app.remote_session_id = Some("remote-only-session".into());
        app.push_display_message(DisplayMessage::system("stale transcript"));
        app.streaming.streaming_text = "stale partial answer".into();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let _entered = runtime.enter();
        let mut remote = crate::tui::backend::RemoteConnection::dummy();
        let event = crate::protocol::ServerEvent::History {
            id: 1,
            session_id: "remote-only-session".into(),
            messages: vec![],
            images: vec![],
            provider_name: Some("remote-provider".into()),
            provider_model: Some("remote-model".into()),
            subagent_model: None,
            autoreview_enabled: Some(false),
            autojudge_enabled: Some(false),
            available_models: vec![],
            available_model_routes: vec![],
            mcp_servers: vec![],
            skills: vec![],
            total_tokens: None,
            token_usage_totals: None,
            all_sessions: vec![],
            client_count: None,
            is_canary: Some(false),
            reload_recovery: None,
            server_version: Some("v0.0.1".into()),
            server_name: Some("remote-daemon".into()),
            server_icon: None,
            server_has_update: Some(true),
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
        };
        app.handle_server_event(event, &mut remote);
        assert!(remote.has_loaded_history());
        assert!(!app.pending_server_reload);
        assert!(app.display_messages().is_empty());
        assert!(app.streaming.streaming_text.is_empty());
        assert_eq!(app.remote_provider_model.as_deref(), Some("remote-model"));
    });
}

#[test]
fn ssh_remote_header_guides_remote_login_and_hides_local_scheduler() {
    with_ssh_remote_test_home(|| {
        let mut app = App::new_for_remote(None);
        app.session.model = Some("laptop-only-model".into());
        assert!(app.effective_remote_provider_model().is_none());
        assert_eq!(app.remote_effort_identity(), (None, None));
        assert!(app.remote_reasoning_effort_hint().is_none());
        app.remote_provider_name = Some("remote-provider".into());
        app.remote_provider_model = Some("remote-model".into());
        app.remote_skills = vec!["remote-only-skill".into()];
        let (persistent, secondary) = crate::tui::ui::header::build_header_sections(&app, 100);
        let text = persistent
            .iter()
            .chain(secondary.iter())
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            text.contains("/login to authenticate on test-remote"),
            "{text}"
        );
        assert!(!text.contains("/login to add provider"), "{text}");
        assert!(
            !text.contains('○'),
            "must not claim an empty remote credential inventory: {text}"
        );
        assert_eq!(crate::tui::TuiState::provider_model(&app), "remote-model");
        assert_eq!(crate::tui::TuiState::provider_name(&app), "remote-provider");
        assert!(text.contains("/remote-only-skill"), "{text}");
        app.remote_skills.clear();
        assert!(crate::tui::TuiState::available_skills(&app).is_empty());
        assert!(super::helpers::gather_ambient_info(false).is_none());
        assert!(super::helpers::gather_ambient_info(true).is_none());
        let info = crate::tui::TuiState::info_widget_data(&app);
        assert!(info.ambient_info.is_none());
        assert!(info.git_info.is_none());
        assert!(crate::tui::scheduled_notification_text(info.ambient_info.as_ref()).is_none());
    });
}
