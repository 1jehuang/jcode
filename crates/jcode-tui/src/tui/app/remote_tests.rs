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
    let provider: Arc<dyn Provider> = Arc::new(MockProvider);
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    let registry = rt.block_on(crate::tool::Registry::new(provider.clone()));
    let mut app = crate::tui::app::App::new_for_test_harness(provider, registry);
    app.queue_mode = false;
    app.diff_mode = crate::config::DiffDisplayMode::Inline;
    app
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
    assert!(!reconnect::reload_handoff_active(&RemoteRunState::default()));
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
        if let Some(last) = app.display_messages().last() {
            if last.content.contains("auto-reload paused") {
                paused = true;
                break;
            }
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
