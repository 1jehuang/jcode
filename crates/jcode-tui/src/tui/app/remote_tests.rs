use super::reconnect;
use super::{
    RemoteRunState, auth_provider_hint_for_login_provider, handle_post_connect,
    handle_server_event, process_remote_followups,
};
use crate::protocol::{
    MemoryActivitySnapshot, MemoryPipelineSnapshot, MemoryStateSnapshot, MemoryStepStatusSnapshot,
    ServerEvent,
};
use crate::provider::Provider;
use crate::tui::info_widget::{MemoryState, StepStatus};
use anyhow::Result;
use std::sync::Arc;

struct MockProvider;

#[async_trait::async_trait]
impl Provider for MockProvider {
    async fn complete(
        &self,
        _messages: &[crate::message::Message],
        _tools: &[crate::message::ToolDefinition],
        _system: &str,
        _resume_session_id: Option<&str>,
    ) -> Result<crate::provider::EventStream> {
        Err(anyhow::anyhow!(
            "Mock provider should not be used for streaming completions in remote app tests"
        ))
    }

    fn name(&self) -> &str {
        "mock"
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(Self)
    }
}

fn create_test_app() -> crate::tui::app::App {
    ensure_test_jcode_home_if_unset();
    // `has_notification()` (via `unfocused_redraw_warranted`) consults a
    // process-wide ambient-info cache that another test may have populated
    // from its own JCODE_HOME (scheduled reminders read as a notification).
    // Reset it so these tests observe only their own state.
    crate::tui::app::helpers::clear_ambient_info_cache_for_tests();
    let provider: Arc<dyn Provider> = Arc::new(MockProvider);
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    let registry = rt.block_on(crate::tool::Registry::new(provider.clone()));
    let mut app = crate::tui::app::App::new_for_test_harness(provider, registry);
    app.queue_mode = false;
    app.diff_mode = crate::config::DiffDisplayMode::Inline;
    app
}

/// Point JCODE_HOME at a per-process temp dir when the environment does not
/// already pin one, so tests never read the developer's real `~/.jcode`
/// state (e.g. a populated ambient queue turns `has_notification()` on and
/// breaks the unfocused-redraw assertions). Mirrors the helper of the same
/// name used by the main app test suite.
fn ensure_test_jcode_home_if_unset() {
    use std::sync::OnceLock;

    static TEST_HOME: OnceLock<std::path::PathBuf> = OnceLock::new();

    if std::env::var_os("JCODE_HOME").is_some() {
        return;
    }

    let path = TEST_HOME.get_or_init(|| {
        let path = std::env::temp_dir().join(format!("jcode-test-home-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&path);
        path
    });
    crate::env::set_var("JCODE_HOME", path);
}

#[test]
fn reload_handoff_active_when_server_flag_is_set() {
    let state = RemoteRunState {
        server_reload_in_progress: true,
        ..RemoteRunState::default()
    };

    assert!(reconnect::reload_handoff_active(&state));
}

#[test]
fn client_focus_defaults_to_true() {
    let app = create_test_app();
    assert!(
        app.client_focused(),
        "a freshly created client should start focused so terminals that never \
         report focus events still animate/redraw normally"
    );
}

#[test]
fn idle_donut_pauses_while_unfocused() {
    let mut app = create_test_app();

    // Whether the donut runs while focused depends on the machine's perf tier
    // and `display.idle_animation` config, so we do not assert the focused case
    // absolutely. We only assert the invariant that matters for the swarm CPU
    // regression: it must never run while the terminal is unfocused.
    let redraw = app.set_client_focused(false);
    assert!(
        !redraw,
        "losing focus should not request an immediate redraw"
    );
    assert!(!app.client_focused());
    assert!(
        !crate::tui::idle_donut_active(&app),
        "idle animation must pause while the terminal is unfocused"
    );

    // Regaining focus requests a full repaint so the window is not stuck on the
    // last paused frame.
    let redraw = app.set_client_focused(true);
    assert!(redraw, "regaining focus should request a redraw");
    assert!(app.client_focused());
}

#[test]
fn unfocused_redraw_warranted_tracks_live_activity() {
    let mut app = create_test_app();
    // `unfocused_redraw_warranted` is only consulted while unfocused, and the
    // decorative donut is force-disabled when unfocused, so evaluate it in that
    // state to mirror the run loop.
    app.set_client_focused(false);

    // Idle empty session: no live output to paint while unfocused.
    assert!(
        !app.unfocused_redraw_warranted(),
        "an idle unfocused session has nothing changing worth a full-rate redraw"
    );

    // A streaming/processing session keeps painting even while unfocused so a
    // visible-but-unfocused window in a tiling WM still shows live progress.
    app.is_processing = true;
    assert!(
        app.unfocused_redraw_warranted(),
        "a processing session should keep redrawing while unfocused"
    );
}

#[test]
fn client_interaction_restores_focus_so_scroll_redraws_at_full_rate() {
    // Regression for the intermittent "can't scroll" bug. If a FocusGained is
    // dropped (flaky under tiling WMs / multiplexers) the window can get stuck
    // as "unfocused idle", which the run loop throttles to ~1 Hz. Any terminal
    // input (key/mouse/scroll) is only delivered to the focused window, so it
    // must restore the focused state and full-rate redraws immediately.
    let mut app = create_test_app();

    // Simulate a stuck-unfocused window (FocusLost seen, FocusGained dropped).
    app.set_client_focused(false);
    assert!(!app.client_focused());
    assert!(
        !app.unfocused_redraw_warranted(),
        "an idle unfocused session is throttled to ~1 Hz redraws"
    );

    // A mouse-wheel / key event arrives: the terminal only routes input to the
    // focused window, so interacting proves focus and must restore it.
    app.note_client_interaction();
    assert!(
        app.client_focused(),
        "interaction must restore focus so scrolling repaints at full rate"
    );
}

#[test]
fn auth_provider_hint_maps_openai_compatible_login_providers() {
    assert_eq!(
        auth_provider_hint_for_login_provider("Azure OpenAI"),
        Some("azure-openai")
    );
    assert_eq!(
        auth_provider_hint_for_login_provider("cerebras"),
        Some("cerebras")
    );
    assert_eq!(
        auth_provider_hint_for_login_provider("Cerebras"),
        Some("cerebras")
    );
    assert_eq!(
        auth_provider_hint_for_login_provider("minimax"),
        Some("minimax")
    );
    assert_eq!(
        auth_provider_hint_for_login_provider("not-a-provider"),
        None
    );
}

#[test]
fn auth_provider_hint_maps_direct_provider_logins_by_display_label() {
    // LoginCompleted carries the descriptor display label, which must still map
    // to the canonical server provider id so the auth-change refresh is
    // attributed correctly (regression: an Anthropic API-key login used to send
    // no hint, so the server reported "OpenAI credentials are active" and
    // skipped the post-login model switch).
    assert_eq!(
        auth_provider_hint_for_login_provider("Anthropic API"),
        Some("anthropic-api")
    );
    assert_eq!(
        auth_provider_hint_for_login_provider("anthropic-api"),
        Some("anthropic-api")
    );
    assert_eq!(
        auth_provider_hint_for_login_provider("claude-api"),
        Some("anthropic-api")
    );
    assert_eq!(
        auth_provider_hint_for_login_provider("Anthropic/Claude"),
        Some("claude")
    );
    assert_eq!(
        auth_provider_hint_for_login_provider("claude"),
        Some("claude")
    );
    assert_eq!(
        auth_provider_hint_for_login_provider("OpenAI"),
        Some("openai")
    );
    assert_eq!(
        auth_provider_hint_for_login_provider("OpenAI API"),
        Some("openai-api")
    );
    assert_eq!(
        auth_provider_hint_for_login_provider("OpenRouter"),
        Some("openrouter")
    );
    assert_eq!(
        auth_provider_hint_for_login_provider("AWS Bedrock"),
        Some("bedrock")
    );
}

#[test]
fn auth_provider_hint_resolves_every_emitted_login_completed_provider() {
    // Every string published as `LoginCompleted.provider` (see the emit sites in
    // src/tui/app/auth.rs) must resolve to a canonical server provider id so the
    // auth-change refresh is attributed to the right provider and the post-login
    // model auto-select runs. Before the loose display-name resolution, only
    // Azure and OpenAI-compatible logins resolved; every direct provider sent no
    // hint, so the server fell back to the session's active provider (the
    // "OpenAI credentials are active" bug) and skipped the model switch.
    //
    // Pairs of (emitted string, expected canonical hint). `None` is only correct
    // for auto-import, which intentionally has no single runtime to attribute to.
    let cases: &[(&str, Option<&str>)] = &[
        // OAuth logins emit lowercase descriptor ids.
        ("openai", Some("openai")),
        ("claude", Some("claude")),
        ("gemini", Some("gemini")),
        ("copilot", Some("copilot")),
        ("antigravity", Some("antigravity")),
        ("cursor", Some("cursor")),
        // API-key paste logins emit descriptor display labels.
        ("Anthropic API", Some("anthropic-api")),
        ("OpenAI API", Some("openai-api")),
        ("AWS Bedrock", Some("bedrock")),
        ("OpenRouter", Some("openrouter")),
        // Azure keeps its dedicated runtime id mapping.
        ("Azure OpenAI", Some("azure-openai")),
        // Auto-import has no single runtime to attribute the refresh to.
        ("auto-import", None),
    ];

    for (emitted, expected) in cases {
        assert_eq!(
            auth_provider_hint_for_login_provider(emitted),
            *expected,
            "login provider {emitted:?} should resolve to {expected:?}"
        );
    }

    // Every login provider descriptor must resolve to the EXACT canonical hint
    // implied by its target, across its display label, id, and every alias.
    // Asserting the exact value (not just `is_some`) is what catches a
    // wrong-attribution bug like the original "OpenAI credentials are active"
    // after an Anthropic login - a weak `is_some` check would have passed even
    // while the hint pointed at the wrong provider.
    use crate::provider_catalog::LoginProviderTarget;
    for descriptor in crate::provider_catalog::login_providers() {
        // Expected hint mirrors auth_provider_hint_for_login_provider's target
        // mapping, the single source of truth for post-login attribution.
        let expected: Option<String> = match descriptor.target {
            LoginProviderTarget::AutoImport => None,
            LoginProviderTarget::Azure => Some("azure-openai".to_string()),
            LoginProviderTarget::OpenAiCompatible(profile) => Some(profile.id.to_string()),
            _ => Some(descriptor.id.to_string()),
        };

        // The emitted string can be the descriptor id, its display label, or any
        // alias (LoginCompleted.provider varies by surface/auth path).
        let mut emitted: Vec<&str> = vec![descriptor.id, descriptor.display_name];
        emitted.extend_from_slice(descriptor.aliases);
        for label in emitted {
            assert_eq!(
                auth_provider_hint_for_login_provider(label),
                expected.as_deref(),
                "login provider {:?} (id {:?}, target {:?}) emitted as {label:?} must attribute \
                 to {expected:?}; a wrong/missing hint mislabels the catalog-refresh message and \
                 skips the post-login model switch",
                descriptor.display_name,
                descriptor.id,
                descriptor.target
            );
        }
    }
}

#[test]
fn auth_changed_event_for_anthropic_api_login_targets_claude_api_route() {
    let auth = super::auth_changed_event_for_login_provider("Anthropic API")
        .expect("Anthropic API login should produce a typed auth event");
    // The server maps the descriptor id `anthropic-api` to the `claude-api`
    // route family for model selection and labelling.
    assert_eq!(auth.provider.as_str(), "anthropic-api");
    assert_eq!(
        auth.auth_method,
        Some(crate::protocol::AuthMethod::RemoteTuiPasteApiKey)
    );
    assert_eq!(
        auth.credential_source,
        Some(crate::protocol::AuthCredentialSource::ApiKeyFile)
    );
    // Direct providers must not claim the OpenAI-compatible runtime/namespace.
    assert!(auth.expected_runtime.is_none());
    assert!(auth.expected_catalog_namespace.is_none());
}

#[test]
fn auth_changed_event_for_oauth_claude_login_is_not_marked_as_api_key_paste() {
    let auth = super::auth_changed_event_for_login_provider("claude")
        .expect("Claude OAuth login should produce a typed auth event");
    assert_eq!(auth.provider.as_str(), "claude");
    // OAuth logins are not API-key pastes.
    assert!(auth.auth_method.is_none());
    assert!(auth.credential_source.is_none());
}

#[test]
fn auth_changed_event_for_cerebras_login_carries_runtime_and_catalog_identity() {
    let auth = super::auth_changed_event_for_login_provider("Cerebras")
        .expect("Cerebras login should produce typed auth event");

    assert_eq!(auth.provider.as_str(), "cerebras");
    assert_eq!(
        auth.credential_source,
        Some(crate::protocol::AuthCredentialSource::ApiKeyFile)
    );
    assert_eq!(
        auth.auth_method,
        Some(crate::protocol::AuthMethod::RemoteTuiPasteApiKey)
    );
    assert_eq!(
        auth.expected_runtime
            .as_ref()
            .map(crate::protocol::RuntimeProviderKey::as_str),
        Some("openai-compatible")
    );
    assert_eq!(
        auth.expected_catalog_namespace
            .as_ref()
            .map(crate::protocol::CatalogNamespace::as_str),
        Some("cerebras")
    );
}

#[test]
fn reload_handoff_inactive_without_flag_or_marker() {
    // `reload_handoff_active` falls back to the on-disk reload marker in the
    // runtime dir. Point the runtime dir at an empty tempdir so a real
    // `jcode.reload` left by a live self-dev reload on this machine cannot
    // leak into the assertion.
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::TempDir::new().expect("create temp dir");
    let prev_runtime = std::env::var_os("JCODE_RUNTIME_DIR");
    crate::env::set_var("JCODE_RUNTIME_DIR", temp.path());

    let inactive = !reconnect::reload_handoff_active(&RemoteRunState::default());

    if let Some(prev_runtime) = prev_runtime {
        crate::env::set_var("JCODE_RUNTIME_DIR", prev_runtime);
    } else {
        crate::env::remove_var("JCODE_RUNTIME_DIR");
    }

    assert!(inactive);
}

#[test]
fn reload_wait_status_message_uses_waiting_language() {
    let mut app = create_test_app();
    app.resume_session_id = Some("ses_test_reload_wait".to_string());
    let state = RemoteRunState::default();

    let message = reconnect::reload_wait_status_message(&app, &state, "server reload in progress");

    assert!(message.contains("waiting for handoff"));
    assert!(!message.contains("retrying"));
}

#[test]
fn submit_prepared_remote_input_defers_until_history_loads() {
    // Regression for the intermittent "first prompt vanishes / weird render"
    // bug: when a manual submit lands before the bootstrap History payload is
    // applied, the History handler's `session_changed` branch calls
    // `clear_display_messages()` and wipes the just-echoed user message. The
    // submit path must hold the prompt until history loads instead of echoing
    // and sending it into that race.
    let mut app = create_test_app();
    app.is_remote = true;
    app.runtime_mode = crate::tui::app::AppRuntimeMode::RemoteClient;

    let rt = tokio::runtime::Runtime::new().expect("runtime");
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    // History has NOT loaded yet (fresh connect window).
    assert!(!remote.has_loaded_history());

    let prepared = crate::tui::app::input::PreparedInput {
        raw_input: "hi".to_string(),
        expanded: "hi".to_string(),
        images: vec![],
    };
    rt.block_on(crate::tui::app::remote::submit_prepared_remote_input(
        &mut app,
        &mut remote,
        prepared,
    ))
    .expect("submit should not error while history is loading");

    // The prompt must be held, not echoed or sent.
    assert!(
        !app.is_processing,
        "submit must not begin a remote send before history loads"
    );
    assert!(
        app.display_messages().iter().all(|m| m.role != "user"),
        "user message must not be echoed before history loads (would be clobbered)"
    );
    let held = app
        .pending_prompt_before_history
        .as_ref()
        .expect("prompt should be held until history loads");
    assert_eq!(held.raw_input, "hi");

    // Once history loads, the post-connect dispatcher fires the held prompt.
    remote.mark_history_loaded();
    rt.block_on(process_remote_followups(&mut app, &mut remote));

    assert!(
        app.pending_prompt_before_history.is_none(),
        "held prompt should be consumed once history is loaded"
    );
    assert!(
        app.display_messages()
            .iter()
            .any(|m| m.role == "user" && m.content == "hi"),
        "the held prompt should be echoed as a user message after history loads"
    );
    assert!(
        app.is_processing,
        "the held prompt should be sent once history is loaded"
    );
}

#[test]
fn process_remote_followups_auto_submits_staged_startup_prompt() {
    // Regression for issues #267/#268/#76: a headed swarm spawn stages its
    // initial prompt into `app.input` with `submit_input_on_startup = true`
    // (not `queued_messages`). The post-connect dispatcher must still submit it;
    // otherwise the spawned agent shows its prompt but never sends it.
    let mut app = create_test_app();
    app.is_remote = true;
    app.runtime_mode = crate::tui::app::AppRuntimeMode::RemoteClient;
    app.input = "Classify the issues in /tmp/batch.txt".to_string();
    app.cursor_pos = app.input.len();
    app.submit_input_on_startup = true;

    // The gate predicate is the actual fix site: a staged startup prompt counts
    // as pending work even though no message was queued via `queued_messages`.
    assert!(
        !app.has_queued_followups(),
        "a staged startup prompt is not a queued follow-up"
    );
    assert!(
        app.has_pending_startup_submission(),
        "staged startup prompt should be recognized as pending work so the \
         post-connect dispatcher invokes process_remote_followups"
    );

    let rt = tokio::runtime::Runtime::new().expect("runtime");
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    remote.mark_history_loaded();

    rt.block_on(process_remote_followups(&mut app, &mut remote));

    assert!(
        !app.submit_input_on_startup,
        "startup submission flag should be consumed after dispatch"
    );
    assert!(
        app.input.is_empty(),
        "input should be cleared once the startup prompt is submitted"
    );
    assert!(
        app.display_messages()
            .iter()
            .any(|message| message.role == "user"
                && message.content == "Classify the issues in /tmp/batch.txt"),
        "submitting the startup prompt should record it as a user message"
    );
}

#[test]
fn has_pending_startup_submission_requires_input_and_flag() {
    // Guards the predicate that gates post-connect startup dispatch.
    let mut app = create_test_app();
    assert!(!app.has_pending_startup_submission());

    app.submit_input_on_startup = true;
    assert!(
        !app.has_pending_startup_submission(),
        "flag alone with empty input is not a pending submission"
    );

    app.input = "   ".to_string();
    assert!(
        !app.has_pending_startup_submission(),
        "whitespace-only input is not a pending submission"
    );

    app.input = "do the work".to_string();
    assert!(app.has_pending_startup_submission());

    app.submit_input_on_startup = false;
    assert!(
        !app.has_pending_startup_submission(),
        "input without the auto-submit flag is just editor state, not pending"
    );
}

#[test]
fn process_remote_followups_auto_reloads_server_by_default() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    remote.mark_history_loaded();

    app.pending_server_reload = true;
    app.auto_server_reload = true;

    rt.block_on(process_remote_followups(&mut app, &mut remote));

    assert!(!app.pending_server_reload);
    let last = app
        .display_messages()
        .last()
        .expect("missing reload message");
    assert_eq!(last.title.as_deref(), Some("Reload"));
    assert!(last.content.contains("Reloading server with newer binary"));
}

#[test]
fn process_remote_followups_reloads_server_even_before_history_loads() {
    // Regression guard: when the server/client binaries differ, the History
    // handler defers session state and sets `pending_server_reload = true`
    // WITHOUT marking history as loaded. The reload must still fire; otherwise
    // history stays unloaded forever and every typed prompt stalls on
    // "Loading session..." until the user restarts.
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    // Intentionally do NOT mark history loaded, mirroring the deferred path.
    assert!(!remote.has_loaded_history());

    app.pending_server_reload = true;
    app.auto_server_reload = true;

    rt.block_on(process_remote_followups(&mut app, &mut remote));

    assert!(
        !app.pending_server_reload,
        "pending server reload should be consumed even while history is unloaded"
    );
    let last = app
        .display_messages()
        .last()
        .expect("missing reload message");
    assert_eq!(last.title.as_deref(), Some("Reload"));
    assert!(last.content.contains("Reloading server with newer binary"));
}

#[test]
fn process_remote_followups_respects_disabled_auto_server_reload() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    remote.mark_history_loaded();

    app.pending_server_reload = true;
    app.auto_server_reload = false;

    rt.block_on(process_remote_followups(&mut app, &mut remote));

    assert!(!app.pending_server_reload);
    let last = app.display_messages().last().expect("missing info message");
    assert_eq!(last.role, "system");
    assert!(last.content.contains("display.auto_server_reload = false"));
}

#[test]
fn process_remote_followups_pauses_auto_reload_after_repeated_attempts() {
    // Regression guard for issue #277: a false-positive "server has update" must
    // not auto-reload forever. After the breaker threshold we stop reloading and
    // surface a message instead.
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    let _guard = rt.enter();
    let mut app = create_test_app();
    app.auto_server_reload = true;

    // Simulate the server repeatedly reporting an update on every history event.
    let mut paused = false;
    for _ in 0..10 {
        let mut remote = crate::tui::backend::RemoteConnection::dummy();
        remote.mark_history_loaded();
        app.pending_server_reload = true;
        rt.block_on(process_remote_followups(&mut app, &mut remote));
        assert!(!app.pending_server_reload);
        if let Some(last) = app.display_messages().last()
            && last.content.contains("auto-reload paused")
        {
            paused = true;
            break;
        }
    }

    assert!(
        paused,
        "auto-reload should eventually pause to avoid an infinite reload loop"
    );
}

#[test]
fn handle_post_connect_dispatches_reload_followup_even_if_history_snapshot_looks_busy() {
    let _guard = crate::storage::lock_test_env();
    let temp_home = tempfile::TempDir::new().expect("create temp home");
    let prev_home = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", temp_home.path());

    let session_id = "session_reload_busy_snapshot";
    crate::tool::selfdev::ReloadContext {
        task_context: Some("Validate reload continuation after reconnect".to_string()),
        version_before: "old-build".to_string(),
        version_after: "new-build".to_string(),
        session_id: session_id.to_string(),
        timestamp: "2026-04-14T00:00:00Z".to_string(),
    }
    .save()
    .expect("save reload context");

    let rt = tokio::runtime::Runtime::new().expect("runtime");
    let mut app = crate::tui::app::App::new_for_remote(Some(session_id.to_string()));
    app.queue_mode = false;
    app.diff_mode = crate::config::DiffDisplayMode::Inline;
    app.is_processing = true;
    app.status = crate::tui::app::ProcessingStatus::RunningTool("batch".to_string());
    app.processing_started = Some(std::time::Instant::now());
    app.remote_resume_activity = Some(crate::tui::app::RemoteResumeActivity {
        session_id: session_id.to_string(),
        observed_at: std::time::Instant::now(),
        current_tool_name: Some("batch".to_string()),
    });

    let _enter = rt.enter();
    let backend = ratatui::backend::TestBackend::new(80, 24);
    let mut terminal = ratatui::Terminal::new(backend).expect("failed to create terminal");
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    remote.mark_history_loaded();
    let mut state = super::RemoteRunState {
        reconnect_attempts: 1,
        ..Default::default()
    };

    let outcome = rt
        .block_on(handle_post_connect(
            &mut app,
            &mut terminal,
            &mut remote,
            &mut state,
            Some(session_id),
        ))
        .expect("post connect should succeed");

    assert!(matches!(outcome, super::PostConnectOutcome::Ready));
    assert!(
        app.hidden_queued_system_messages.is_empty(),
        "reload continuation should dispatch instead of staying hidden"
    );
    assert!(matches!(
        app.status,
        crate::tui::app::ProcessingStatus::Sending
    ));
    assert!(app.current_message_id.is_some());
    assert!(app.rate_limit_pending_message.is_some());

    if let Ok(path) = crate::tool::selfdev::ReloadContext::path_for_session(session_id) {
        let _ = std::fs::remove_file(path);
    }
    if let Some(prev_home) = prev_home {
        crate::env::set_var("JCODE_HOME", prev_home);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
}

#[test]
fn handle_server_event_applies_remote_memory_activity_snapshot() {
    crate::memory::clear_activity();

    let rt = tokio::runtime::Runtime::new().expect("runtime");
    let _guard = rt.enter();
    let mut app = create_test_app();
    app.memory_enabled = true;
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    handle_server_event(
        &mut app,
        ServerEvent::MemoryActivity {
            activity: MemoryActivitySnapshot {
                state: MemoryStateSnapshot::SidecarChecking { count: 3 },
                state_age_ms: 180,
                pipeline: Some(MemoryPipelineSnapshot {
                    search: MemoryStepStatusSnapshot::Done,
                    search_result: None,
                    verify: MemoryStepStatusSnapshot::Running,
                    verify_result: None,
                    verify_progress: Some((1, 3)),
                    inject: MemoryStepStatusSnapshot::Pending,
                    inject_result: None,
                    maintain: MemoryStepStatusSnapshot::Pending,
                    maintain_result: None,
                }),
            },
        },
        &mut remote,
    );

    let activity = crate::memory::get_activity().expect("memory activity should be populated");
    assert_eq!(activity.state, MemoryState::SidecarChecking { count: 3 });
    let pipeline = activity.pipeline.expect("pipeline should be restored");
    assert_eq!(pipeline.search, StepStatus::Done);
    assert_eq!(pipeline.verify, StepStatus::Running);
    assert_eq!(pipeline.verify_progress, Some((1, 3)));
    assert!(activity.state_since.elapsed().as_millis() >= 100);

    crate::memory::clear_activity();
}

/// Reproduces the "stuck on loading session…" bug and verifies the watchdog
/// recovers it: a remote connection that never receives the bootstrap History
/// event (so `has_loaded_history()` stays false) must re-request `GetHistory`
/// once it has waited past the recovery delay, instead of staying stuck forever.
#[test]
fn remote_history_watchdog_rerequests_history_when_stuck() {
    use std::time::{Duration, Instant};
    use tokio::io::AsyncBufReadExt;

    let mut app = create_test_app();
    app.is_remote = true;
    app.remote_session_id = Some("session_stuck".to_string());

    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut line = String::new();
    let (redraw, attempts) = rt.block_on(async {
        let mut remote = crate::tui::backend::RemoteConnection::dummy();
        // The bug condition: history never loaded after (re)connect.
        assert!(!remote.has_loaded_history());
        let peer = remote
            .take_dummy_peer()
            .expect("dummy remote should retain peer stream");
        let (reader, _writer) = peer.into_split();
        let mut reader = tokio::io::BufReader::new(reader);

        // First tick simply starts tracking the wait; no re-request yet.
        let first = super::recover_stuck_remote_history(&mut app, &mut remote).await;
        assert!(!first, "first observation should only arm the watchdog");
        assert!(app.remote_history_wait_started.is_some());
        assert_eq!(app.remote_history_recovery_attempts, 0);

        // Simulate the connection having been stuck past the recovery delay.
        app.remote_history_wait_started = Instant::now().checked_sub(Duration::from_secs(60));

        let redraw = super::recover_stuck_remote_history(&mut app, &mut remote).await;
        reader
            .read_line(&mut line)
            .await
            .expect("history re-request should be readable by peer");
        (redraw, app.remote_history_recovery_attempts)
    });

    assert!(redraw, "re-requesting history should trigger a redraw");
    assert_eq!(
        attempts, 1,
        "watchdog should have re-requested history once"
    );
    assert!(matches!(
        serde_json::from_str::<crate::protocol::Request>(&line)
            .expect("history re-request should deserialize"),
        crate::protocol::Request::GetHistory { .. }
    ));
}

/// A partial inbound frame proves that the original History response is in
/// flight. The watchdog must not queue another full response behind it.
#[test]
fn remote_history_watchdog_does_not_rerequest_while_frame_is_arriving() {
    use std::time::{Duration, Instant};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

    let mut app = create_test_app();
    app.is_remote = true;
    app.remote_session_id = Some("session_large".to_string());
    app.remote_history_wait_started = Instant::now().checked_sub(Duration::from_secs(60));

    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let mut remote = crate::tui::backend::RemoteConnection::dummy();
        let peer = remote
            .take_dummy_peer()
            .expect("dummy remote should retain peer stream");
        let (reader, mut writer) = peer.into_split();
        let mut reader = tokio::io::BufReader::new(reader);

        // Deliberately omit the newline so next_event retains this partial
        // History-sized frame when its future is cancelled by the tick.
        writer
            .write_all(b"{\"type\":\"history\",\"messages\":[")
            .await
            .expect("partial frame should reach remote");
        assert!(
            tokio::time::timeout(Duration::from_millis(20), remote.next_event())
                .await
                .is_err(),
            "partial frame must remain incomplete"
        );
        assert!(remote.has_buffered_inbound_frame());

        let redraw = super::recover_stuck_remote_history(&mut app, &mut remote).await;
        assert!(!redraw, "in-flight history should not trigger recovery");
        assert_eq!(app.remote_history_recovery_attempts, 0);

        let mut line = String::new();
        assert!(
            tokio::time::timeout(Duration::from_millis(20), reader.read_line(&mut line))
                .await
                .is_err(),
            "watchdog must not write a duplicate GetHistory request"
        );
    });
}

/// Once history loads, the watchdog must clear its budget and do nothing.
#[test]
fn remote_history_watchdog_clears_budget_once_history_loads() {
    use std::time::{Duration, Instant};

    let mut app = create_test_app();
    app.is_remote = true;
    app.remote_history_wait_started = Instant::now().checked_sub(Duration::from_secs(60));
    app.remote_history_recovery_attempts = 2;

    let rt = tokio::runtime::Runtime::new().unwrap();
    let redraw = rt.block_on(async {
        let mut remote = crate::tui::backend::RemoteConnection::dummy();
        remote.mark_history_loaded();
        super::recover_stuck_remote_history(&mut app, &mut remote).await
    });

    assert!(!redraw);
    assert!(app.remote_history_wait_started.is_none());
    assert_eq!(app.remote_history_recovery_attempts, 0);
    assert!(app.remote_history_recovery_last_attempt.is_none());
}

/// After exhausting re-requests the watchdog surfaces an actionable `/restart`
/// hint exactly once instead of silently leaving the user stuck.
#[test]
fn remote_history_watchdog_advises_restart_after_giving_up() {
    use std::time::{Duration, Instant};

    let mut app = create_test_app();
    app.is_remote = true;
    app.remote_history_wait_started = Instant::now().checked_sub(Duration::from_secs(60));
    app.remote_history_recovery_attempts = super::REMOTE_HISTORY_RECOVERY_MAX_ATTEMPTS;
    app.remote_history_recovery_last_attempt = Some(Instant::now());

    let before = app.display_messages().len();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let redraw = rt.block_on(async {
        let mut remote = crate::tui::backend::RemoteConnection::dummy();
        super::recover_stuck_remote_history(&mut app, &mut remote).await
    });

    assert!(redraw);
    let messages = app.display_messages();
    assert_eq!(messages.len(), before + 1, "should add exactly one hint");
    assert!(
        messages.last().unwrap().content.contains("/restart"),
        "hint should advise /restart: {}",
        messages.last().unwrap().content
    );
    // last_attempt cleared so the hint is not repeated every tick.
    assert!(app.remote_history_recovery_last_attempt.is_none());

    // A subsequent tick must not add another hint.
    let rt2 = tokio::runtime::Runtime::new().unwrap();
    let redraw2 = rt2.block_on(async {
        let mut remote = crate::tui::backend::RemoteConnection::dummy();
        super::recover_stuck_remote_history(&mut app, &mut remote).await
    });
    assert!(!redraw2);
    assert_eq!(app.display_messages().len(), before + 1);
}

/// Regression for issue #427: picking an effort-variant model row (e.g.
/// "gpt-5.5 (high)") in remote mode must forward the chosen effort to the
/// server after the model-switch request. Previously the effort was applied
/// only to the local stand-in provider, so the server kept its configured
/// default (low by default) and silently ran the new model at low effort.
#[test]
fn forward_pending_reasoning_effort_sends_effort_request_to_server() {
    use tokio::io::AsyncBufReadExt;

    let mut app = create_test_app();
    app.is_remote = true;
    app.pending_reasoning_effort = Some("high".to_string());

    let rt = tokio::runtime::Runtime::new().expect("runtime");
    let line = rt.block_on(async {
        let mut remote = crate::tui::backend::RemoteConnection::dummy();
        let peer = remote
            .take_dummy_peer()
            .expect("dummy remote should retain peer stream");
        let (reader, _writer) = peer.into_split();
        let mut reader = tokio::io::BufReader::new(reader);

        super::forward_pending_reasoning_effort(&mut app, &mut remote).await;

        let mut line = String::new();
        reader
            .read_line(&mut line)
            .await
            .expect("effort request should be readable by peer");
        line
    });

    match serde_json::from_str::<crate::protocol::Request>(&line)
        .expect("effort request should deserialize")
    {
        crate::protocol::Request::SetReasoningEffort { effort, .. } => {
            assert_eq!(effort, "high", "the picker-selected effort must be sent");
        }
        other => panic!("expected SetReasoningEffort request, got {:?}", other),
    }

    assert!(
        app.pending_reasoning_effort.is_none(),
        "staged effort must be consumed after dispatch"
    );
    assert_eq!(
        app.remote_reasoning_effort.as_deref(),
        Some("high"),
        "requested effort should be tracked optimistically for the UI"
    );
}

/// The dispatcher must be a no-op when no effort variant was staged (plain
/// model rows without an effort suffix).
#[test]
fn forward_pending_reasoning_effort_is_noop_without_staged_effort() {
    let mut app = create_test_app();
    app.is_remote = true;
    assert!(app.pending_reasoning_effort.is_none());

    let rt = tokio::runtime::Runtime::new().expect("runtime");
    rt.block_on(async {
        let mut remote = crate::tui::backend::RemoteConnection::dummy();
        super::forward_pending_reasoning_effort(&mut app, &mut remote).await;
    });

    assert!(app.remote_reasoning_effort.is_none());
    assert!(app.pending_reasoning_effort.is_none());
}
