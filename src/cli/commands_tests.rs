use super::*;
use crate::auth::{AuthState, AuthStatus, ProviderAuth};
use crate::message::{Message, StreamEvent, ToolDefinition};
use crate::provider::ModelRoute;
use crate::provider::{EventStream, Provider};
use crate::todo::ConfidenceState;
use crate::tool::Registry;
use async_trait::async_trait;
use std::io::{Read, Write};
use std::sync::Arc;
use tokio::sync::mpsc as tokio_mpsc;
use tokio_stream::wrappers::ReceiverStream;

#[test]
fn memory_cli_project_import_uses_explicit_directory_and_persists() {
    let _guard = crate::storage::lock_test_env();
    let _saved = SavedEnv::capture(&["JCODE_HOME"]);
    let temp = tempfile::tempdir().expect("temp dir");
    crate::env::set_var("JCODE_HOME", temp.path().join("home"));
    let project = temp.path().join("project");
    std::fs::create_dir_all(&project).expect("create project");

    let entry = crate::memory::MemoryEntry::new(
        crate::memory::MemoryCategory::Fact,
        "persisted project memory".to_string(),
    );
    let input = temp.path().join("memories.json");
    std::fs::write(
        &input,
        serde_json::to_vec(&vec![entry.clone()]).expect("serialize memory"),
    )
    .expect("write import");

    run_memory_command_for_dir(
        MemorySubcommand::Import {
            input: input.display().to_string(),
            scope: "project".to_string(),
            overwrite: false,
        },
        Some(project.clone()),
    )
    .expect("import project memory");

    let reloaded = crate::memory::MemoryManager::new()
        .with_project_dir(project)
        .load_project_graph()
        .expect("reload project graph");
    assert_eq!(
        reloaded
            .get_memory(&entry.id)
            .map(|memory| memory.content.as_str()),
        Some("persisted project memory")
    );
}

#[test]
fn memory_cli_project_import_fails_without_durable_project_store() {
    let temp = tempfile::tempdir().expect("temp dir");
    let input = temp.path().join("memories.json");
    std::fs::write(&input, "[]").expect("write import");

    let error = run_memory_command_for_dir(
        MemorySubcommand::Import {
            input: input.display().to_string(),
            scope: "project".to_string(),
            overwrite: false,
        },
        None,
    )
    .expect_err("project import must require a durable store");

    assert!(error.to_string().contains("without a project directory"));
}

struct SavedEnv {
    vars: Vec<(String, Option<String>)>,
}

impl SavedEnv {
    fn capture(keys: &[&str]) -> Self {
        Self {
            vars: keys
                .iter()
                .map(|key| (key.to_string(), std::env::var(key).ok()))
                .collect(),
        }
    }
}

impl Drop for SavedEnv {
    fn drop(&mut self) {
        for (key, value) in &self.vars {
            if let Some(value) = value {
                crate::env::set_var(key, value);
            } else {
                crate::env::remove_var(key);
            }
        }
    }
}

struct TestProvider;

#[async_trait]
impl Provider for TestProvider {
    async fn complete(
        &self,
        _messages: &[Message],
        _tools: &[ToolDefinition],
        _system: &str,
        _resume_session_id: Option<&str>,
    ) -> Result<EventStream> {
        let (tx, rx) = tokio_mpsc::channel::<Result<StreamEvent>>(4);
        tokio::spawn(async move {
            let _ = tx.send(Ok(StreamEvent::TextDelta("ok".to_string()))).await;
            let _ = tx
                .send(Ok(StreamEvent::MessageEnd {
                    stop_reason: Some("end_turn".to_string()),
                }))
                .await;
        });
        Ok(Box::pin(ReceiverStream::new(rx)))
    }

    fn name(&self) -> &str {
        "test"
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(Self)
    }
}

struct FailingTestProvider;

#[async_trait]
impl Provider for FailingTestProvider {
    async fn complete(
        &self,
        _messages: &[Message],
        _tools: &[ToolDefinition],
        _system: &str,
        _resume_session_id: Option<&str>,
    ) -> Result<EventStream> {
        Err(anyhow::anyhow!("one-shot sentinel failure"))
    }

    fn name(&self) -> &str {
        "failing-test"
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(Self)
    }
}

fn spawn_single_response_http_server(status: u16, body: &str) -> String {
    spawn_single_response_http_server_on_host("127.0.0.1", status, body)
}

fn spawn_single_response_http_server_on_host(host: &str, status: u16, body: &str) -> String {
    let listener = std::net::TcpListener::bind((host, 0)).expect("bind test server");
    let addr = listener.local_addr().expect("local addr");
    let body = body.to_string();
    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept connection");
        let mut buf = [0u8; 2048];
        let _ = stream.read(&mut buf);
        let status_text = match status {
            200 => "OK",
            400 => "Bad Request",
            404 => "Not Found",
            500 => "Internal Server Error",
            _ => "OK",
        };
        let response = format!(
            "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            status,
            status_text,
            body.len(),
            body
        );
        stream
            .write_all(response.as_bytes())
            .expect("write response");
    });
    format!("http://{}:{}/v1", host, addr.port())
}

#[test]
fn test_parse_tailscale_dns_name_trims_trailing_dot() {
    let payload = br#"{"Self":{"DNSName":"yashmacbook.tailabc.ts.net."}}"#;
    let parsed = parse_tailscale_dns_name(payload);
    assert_eq!(parsed.as_deref(), Some("yashmacbook.tailabc.ts.net"));
}

#[test]
fn test_parse_tailscale_dns_name_handles_missing_or_empty() {
    let missing = br#"{"Self":{}}"#;
    assert!(parse_tailscale_dns_name(missing).is_none());

    let empty = br#"{"Self":{"DNSName":"   "}}"#;
    assert!(parse_tailscale_dns_name(empty).is_none());
}

#[test]
fn test_parse_tailscale_dns_name_invalid_json() {
    assert!(parse_tailscale_dns_name(b"not-json").is_none());
}

#[test]
fn configured_auth_test_targets_only_include_configured_supported_providers() {
    let _guard = crate::storage::lock_test_env();
    // OpenRouter has no OAuth state to set: its availability is read straight
    // from `OPENROUTER_API_KEY` (or `openrouter.env`). Setting it here is what
    // makes the expectation below independent of whoever runs the test having
    // a key exported.
    let saved_openrouter_key = std::env::var("OPENROUTER_API_KEY").ok();
    crate::env::set_var("OPENROUTER_API_KEY", "test-key");

    let status = AuthStatus {
        anthropic: ProviderAuth {
            state: AuthState::Available,
            has_oauth: true,
            oauth_state: AuthState::Available,
            has_api_key: false,
        },
        openai: AuthState::NotConfigured,
        gemini: AuthState::Available,
        google: AuthState::Expired,
        copilot: AuthState::Available,
        cursor: AuthState::NotConfigured,
        openrouter: AuthState::Available,
        ..AuthStatus::default()
    };

    let targets = configured_auth_test_targets(&status);

    match saved_openrouter_key {
        Some(key) => crate::env::set_var("OPENROUTER_API_KEY", key),
        None => crate::env::remove_var("OPENROUTER_API_KEY"),
    }

    assert!(targets.contains(&ResolvedAuthTestTarget::Detailed(AuthTestTarget::Claude)));
    assert!(targets.contains(&ResolvedAuthTestTarget::Detailed(AuthTestTarget::Copilot)));
    assert!(targets.contains(&ResolvedAuthTestTarget::Detailed(AuthTestTarget::Gemini)));
    assert!(targets.contains(&ResolvedAuthTestTarget::Generic {
        provider: crate::provider_catalog::OPENROUTER_LOGIN_PROVIDER,
        choice: super::super::provider_init::ProviderChoice::Openrouter,
    }));

    assert!(!targets.contains(&ResolvedAuthTestTarget::Detailed(AuthTestTarget::Openai)));
    assert!(!targets.contains(&ResolvedAuthTestTarget::Detailed(AuthTestTarget::Google)));
    assert!(!targets.contains(&ResolvedAuthTestTarget::Detailed(AuthTestTarget::Cursor)));
}

#[test]
fn explicit_supported_provider_maps_to_single_auth_target() {
    let targets =
        resolve_auth_test_targets(&super::super::provider_init::ProviderChoice::Gemini, false)
            .expect("resolve target");
    assert_eq!(
        targets,
        vec![ResolvedAuthTestTarget::Detailed(AuthTestTarget::Gemini)]
    );
}

#[test]
fn explicit_generic_provider_maps_to_generic_auth_target() {
    let targets = resolve_auth_test_targets(
        &super::super::provider_init::ProviderChoice::Openrouter,
        false,
    )
    .expect("resolve target");
    assert_eq!(
        targets,
        vec![ResolvedAuthTestTarget::Generic {
            provider: crate::provider_catalog::OPENROUTER_LOGIN_PROVIDER,
            choice: super::super::provider_init::ProviderChoice::Openrouter,
        }]
    );
}

#[test]
fn collect_cli_model_names_prefers_available_routes_and_dedupes() {
    let routes = vec![
        ModelRoute {
            model: "gpt-5.4".to_string(),
            provider: "OpenAI".to_string(),
            api_method: "openai-oauth".to_string(),
            available: true,
            detail: String::new(),
            cheapness: None,
        },
        ModelRoute {
            model: "gpt-5.4".to_string(),
            provider: "auto".to_string(),
            api_method: "openrouter".to_string(),
            available: true,
            detail: String::new(),
            cheapness: None,
        },
        ModelRoute {
            model: "openrouter models".to_string(),
            provider: "—".to_string(),
            api_method: "openrouter".to_string(),
            available: false,
            detail: "OPENROUTER_API_KEY not set".to_string(),
            cheapness: None,
        },
    ];

    let models = collect_cli_model_names(
        &routes,
        vec!["gpt-5.4".to_string(), "claude-sonnet-4".to_string()],
    );

    assert_eq!(models, vec!["gpt-5.4", "claude-sonnet-4"]);
}

fn test_route(model: &str, provider: &str, api_method: &str) -> ModelRoute {
    ModelRoute {
        model: model.to_string(),
        provider: provider.to_string(),
        api_method: api_method.to_string(),
        available: true,
        detail: String::new(),
        cheapness: None,
    }
}

#[test]
fn cli_route_display_uses_typed_api_methods() {
    assert_eq!(cli_api_method_display("openai-oauth"), "oauth");
    assert_eq!(cli_api_method_display("openai-api-key"), "api key");
    assert_eq!(
        cli_api_method_display("openai-compatible:cerebras"),
        "api key"
    );
    assert_eq!(cli_api_method_display("mock-auth:profile"), "mock-auth");
    assert_eq!(
        cli_route_provider_display("DeepSeek", "openrouter"),
        "OpenRouter/DeepSeek"
    );
}

fn test_todo(
    id: &str,
    status: &str,
    priority: &str,
    confidence: Option<ConfidenceState>,
    completion_confidence: Option<ConfidenceState>,
) -> crate::todo::TodoItem {
    crate::todo::TodoItem {
        id: id.to_string(),
        content: format!("todo {id}"),
        status: status.to_string(),
        priority: priority.to_string(),
        confidence,
        completion_confidence,
        ..Default::default()
    }
}

#[test]
fn run_auto_poke_followup_targets_below_threshold_todos() {
    let todos = vec![
        test_todo(
            "a",
            "completed",
            "high",
            Some(ConfidenceState::Plausible),
            Some(ConfidenceState::Plausible),
        ),
        test_todo(
            "b",
            "completed",
            "low",
            Some(ConfidenceState::Plausible),
            Some(ConfidenceState::Plausible),
        ),
    ];

    let followup = build_run_auto_poke_follow_up_from_todos(&todos, false, None);

    match followup {
        Some(RunAutoPokeFollowUp::ConfidenceSummary {
            total_todos,
            message,
            ..
        }) => {
            assert_eq!(total_todos, 2);
            assert!(message.starts_with(crate::todo::TODO_COMPLETION_CONTINUATION_MESSAGE));
            assert!(message.contains("completion confidence"));
            assert!(!message.to_ascii_lowercase().contains("threshold"));
        }
        _ => panic!("expected confidence-summary follow-up"),
    }
}

#[test]
fn run_auto_poke_followup_challenges_abrupt_confidence_once() {
    let mut todo = test_todo(
        "a",
        "completed",
        "high",
        Some(ConfidenceState::Speculative),
        Some(ConfidenceState::Verified),
    );
    todo.confidence_history = vec![ConfidenceState::Speculative, ConfidenceState::Verified];

    let todos = [todo];
    match build_run_auto_poke_follow_up_from_todos(&todos, false, None) {
        Some(RunAutoPokeFollowUp::ConfidenceSummary {
            message,
            confidence_spike_challenge,
            ..
        }) => {
            assert!(confidence_spike_challenge);
            assert!(message.starts_with(crate::todo::TODO_CONFIDENCE_SPIKE_CONTINUATION_MESSAGE));
        }
        _ => panic!("expected confidence-spike challenge"),
    }
    assert!(build_run_auto_poke_follow_up_from_todos(&todos, true, None).is_none());
}

#[test]
fn run_auto_poke_followup_silent_when_confident_and_earned() {
    // All above threshold and no spikes: the old behavior sent an "all good"
    // summary anyway; now we spend no tokens and end the run.
    let todos = vec![
        {
            let mut todo = test_todo(
                "a",
                "completed",
                "high",
                Some(ConfidenceState::Verified),
                Some(ConfidenceState::Verified),
            );
            todo.confidence_history = vec![
                ConfidenceState::Plausible,
                ConfidenceState::Plausible,
                ConfidenceState::Validated,
                ConfidenceState::Verified,
            ];
            todo
        },
        test_todo(
            "b",
            "completed",
            "low",
            Some(ConfidenceState::Validated),
            Some(ConfidenceState::Validated),
        ),
    ];
    assert!(build_run_auto_poke_follow_up_from_todos(&todos, false, None).is_none());
}

#[test]
fn run_auto_poke_followup_prioritizes_incomplete_todos() {
    let todos = vec![
        test_todo(
            "a",
            "completed",
            "high",
            Some(ConfidenceState::Plausible),
            Some(ConfidenceState::Plausible),
        ),
        test_todo(
            "b",
            "in_progress",
            "medium",
            Some(ConfidenceState::Plausible),
            None,
        ),
    ];

    let followup = build_run_auto_poke_follow_up_from_todos(&todos, false, None);

    match followup {
        Some(RunAutoPokeFollowUp::Incomplete { count, message }) => {
            assert_eq!(count, 1);
            assert_eq!(
                message,
                "You have 1 incomplete todo. Continue working, or update the todo tool."
            );
        }
        _ => panic!("expected incomplete-todo follow-up"),
    }
}

#[test]
fn run_auto_poke_treats_completion_synonyms_and_case_as_finished() {
    for status in ["done", "finished", "complete", "Completed", " DONE "] {
        let todos = vec![test_todo(
            "a",
            status,
            "high",
            Some(ConfidenceState::Verified),
            Some(ConfidenceState::Verified),
        )];
        assert!(
            build_run_auto_poke_follow_up_from_todos(&todos, false, None).is_none(),
            "status {status:?} should not trigger an incomplete-todo poke"
        );
    }
}

#[test]
fn run_auto_poke_treats_cancelled_spelling_variants_as_finished() {
    for status in ["cancelled", "canceled", "Cancelled"] {
        let todos = vec![test_todo("a", status, "high", None, None)];
        assert!(
            build_run_auto_poke_follow_up_from_todos(&todos, false, None).is_none(),
            "status {status:?} should not trigger an incomplete-todo poke"
        );
    }
}

/// Headless `jcode run` is what the benchmarks and scripted use go through, so
/// the deferred quality review must reach that path too, not only the TUI.
#[test]
fn run_auto_poke_delivers_the_deferred_gate_digest_before_confidence() {
    let todos = vec![test_todo(
        "a",
        "completed",
        "high",
        Some(ConfidenceState::Plausible),
        Some(ConfidenceState::Plausible),
    )];
    // Without a digest, the confidence gate is what fires.
    assert!(matches!(
        build_run_auto_poke_follow_up_from_todos(&todos, false, None),
        Some(RunAutoPokeFollowUp::ConfidenceSummary { .. })
    ));
    // With one, the weak points are reviewed first, since that work can change
    // the very assessments the confidence gate judges.
    match build_run_auto_poke_follow_up_from_todos(
        &todos,
        false,
        Some("review these points".to_string()),
    ) {
        Some(RunAutoPokeFollowUp::GateDigest { message }) => {
            assert_eq!(message, "review these points");
        }
        _ => panic!("expected the gate digest to take precedence"),
    }
}

/// Open todos mean the agent is still working, so the review must wait for the
/// turn to actually end rather than interrupting mid-flight.
#[test]
fn run_auto_poke_prefers_incomplete_todos_over_the_gate_digest() {
    let todos = vec![test_todo(
        "a",
        "in_progress",
        "high",
        Some(ConfidenceState::Plausible),
        None,
    )];
    assert!(matches!(
        build_run_auto_poke_follow_up_from_todos(
            &todos,
            false,
            Some("review these points".to_string())
        ),
        Some(RunAutoPokeFollowUp::Incomplete { .. })
    ));
}

/// Regression: the digest is consumed from the log before the follow-up is
/// chosen, so a turn with open todos must not destroy it. Auto-poke iterates
/// many times with open todos on a long run, and each pass used to silently
/// discard the observations, meaning the reminder never survived to delivery.
#[test]
fn open_todos_do_not_consume_the_pending_gate_digest() {
    let _guard = crate::storage::lock_test_env();
    let previous_home = std::env::var_os("JCODE_HOME");
    let dir = tempfile::TempDir::new().expect("tempdir");
    crate::env::set_var("JCODE_HOME", dir.path());
    let session = "run-gate-digest-open-todos";

    crate::todo::append_gate_observations(
        session,
        &[crate::todo::GateObservation {
            kind: crate::todo::GateObservationKind::IntentUnderstanding,
            group: None,
            state: Some("partial".to_string()),
        }],
    )
    .expect("append");

    let open = vec![test_todo(
        "a",
        "in_progress",
        "high",
        Some(ConfidenceState::Plausible),
        None,
    )];
    assert!(matches!(
        build_run_auto_poke_follow_up_from_todos(
            &open,
            false,
            take_run_gate_digest_if_turn_ended(session, false, &open),
        ),
        Some(RunAutoPokeFollowUp::Incomplete { .. })
    ));
    assert!(
        !crate::todo::load_gate_observations(session)
            .expect("reload")
            .is_empty(),
        "observations must survive a poke iteration that still has open work"
    );

    // Once the work closes, the reminder is still there to deliver.
    let done = vec![test_todo(
        "a",
        "completed",
        "high",
        Some(ConfidenceState::Plausible),
        Some(ConfidenceState::Verified),
    )];
    match build_run_auto_poke_follow_up_from_todos(
        &done,
        false,
        take_run_gate_digest_if_turn_ended(session, false, &done),
    ) {
        Some(RunAutoPokeFollowUp::GateDigest { message }) => {
            assert!(message.starts_with(crate::todo::TODO_GATE_DIGEST_PREFIX));
        }
        other => panic!("expected the preserved digest to be delivered, got {other:?}"),
    }
    assert!(
        crate::todo::load_gate_observations(session)
            .expect("reload")
            .is_empty(),
        "delivering the digest should consume the log"
    );

    match previous_home {
        Some(value) => crate::env::set_var("JCODE_HOME", value),
        None => crate::env::remove_var("JCODE_HOME"),
    }
}

/// The log must be consumed on delivery, or one turn's observations would leak
/// into the next turn and be raised again against work they never described.
#[test]
fn take_run_gate_digest_consumes_the_log_and_respects_delivery() {
    let _guard = crate::storage::lock_test_env();
    let previous_home = std::env::var_os("JCODE_HOME");
    let dir = tempfile::TempDir::new().expect("tempdir");
    crate::env::set_var("JCODE_HOME", dir.path());
    let session = "run-gate-digest";

    crate::todo::append_gate_observations(
        session,
        &[crate::todo::GateObservation {
            kind: crate::todo::GateObservationKind::IntentUnderstanding,
            group: None,
            state: Some("partial".to_string()),
        }],
    )
    .expect("append");

    // Already delivered this turn: no second reminder, and the log is left for
    // the delivering path to have handled.
    assert!(take_run_gate_digest(session, true).is_none());

    let digest = take_run_gate_digest(session, false).expect("unresolved point should surface");
    assert!(digest.starts_with(crate::todo::TODO_GATE_DIGEST_PREFIX));
    // Consumed, so the next turn starts clean.
    assert!(
        crate::todo::load_gate_observations(session)
            .expect("reload")
            .is_empty()
    );
    assert!(take_run_gate_digest(session, false).is_none());

    match previous_home {
        Some(value) => crate::env::set_var("JCODE_HOME", value),
        None => crate::env::remove_var("JCODE_HOME"),
    }
}

#[test]
fn run_auto_poke_followup_rechecks_completion_confidence_until_it_passes() {
    let needs_validation = vec![test_todo(
        "a",
        "completed",
        "high",
        Some(ConfidenceState::Plausible),
        Some(ConfidenceState::Plausible),
    )];
    assert!(matches!(
        build_run_auto_poke_follow_up_from_todos(&needs_validation, false, None),
        Some(RunAutoPokeFollowUp::ConfidenceSummary { .. })
    ));
    assert!(matches!(
        build_run_auto_poke_follow_up_from_todos(&needs_validation, false, None),
        Some(RunAutoPokeFollowUp::ConfidenceSummary { .. })
    ));

    let validated = vec![test_todo(
        "a",
        "completed",
        "high",
        Some(ConfidenceState::Plausible),
        Some(ConfidenceState::Verified),
    )];
    assert!(matches!(
        build_run_auto_poke_follow_up_from_todos(&validated, false, None),
        Some(RunAutoPokeFollowUp::ConfidenceSummary {
            confidence_spike_challenge: true,
            ..
        })
    ));
    assert!(build_run_auto_poke_follow_up_from_todos(&validated, true, None).is_none());
}

#[test]
fn cli_provider_choice_filter_uses_typed_api_methods() {
    let routes = vec![
        test_route("claude-opus-4-6", "Anthropic", "claude-oauth"),
        test_route("claude-opus-4-6", "Anthropic", "claude-api"),
        test_route("gpt-5.5", "OpenAI", "openai-oauth"),
        test_route("gpt-5.5", "OpenAI", "openai-api-key"),
        test_route("gpt-5.6-pro[web]", "OpenAI", "chatgpt-web"),
        test_route("deepseek/deepseek-v4-pro", "auto", "openrouter"),
        test_route("grok-code-fast-1", "Copilot", "copilot"),
    ];

    let openai = filter_cli_model_routes_for_choice(
        &super::super::provider_init::ProviderChoice::Openai,
        &routes,
    );
    assert_eq!(openai.len(), 2);
    assert!(openai.iter().any(|route| matches!(
        route.api_method_kind(),
        crate::provider::ModelRouteApiMethod::OpenAIOAuth
    )));
    assert!(openai.iter().any(|route| {
        matches!(
            route.api_method_kind(),
            crate::provider::ModelRouteApiMethod::Other(ref value) if value == "chatgpt-web"
        )
    }));

    let claude = filter_cli_model_routes_for_choice(
        &super::super::provider_init::ProviderChoice::Claude,
        &routes,
    );
    assert_eq!(claude.len(), 2);
    assert!(
        claude
            .iter()
            .all(|route| route.api_method_kind().is_anthropic_credential_route())
    );
}

#[test]
fn cloud_sessions_args_match_jade_helper_contract() {
    let args = build_jade_sessions_args(CloudSessionsSubcommand::UploadLatest {
        sessions_dir: "/tmp/sessions".to_string(),
        raw: true,
        user_id: "jeremy".to_string(),
        profile: Some("test-profile".to_string()),
        region: Some("us-east-1".to_string()),
        helper: None,
    });

    assert_eq!(
        args,
        vec![
            "upload-latest",
            "--user-id",
            "jeremy",
            "--profile",
            "test-profile",
            "--region",
            "us-east-1",
            "--sessions-dir",
            "/tmp/sessions",
            "--raw",
        ]
    );

    let args = build_jade_sessions_args(CloudSessionsSubcommand::View {
        session_id: "session_123".to_string(),
        format: "html".to_string(),
        output: Some("/tmp/session.html".to_string()),
        open: true,
        user_id: "dev".to_string(),
        profile: Some("profile".to_string()),
        region: Some("region".to_string()),
        helper: None,
    });

    assert_eq!(
        args,
        vec![
            "view",
            "--user-id",
            "dev",
            "--profile",
            "profile",
            "--region",
            "region",
            "--format",
            "html",
            "--output",
            "/tmp/session.html",
            "--open",
            "session_123",
        ]
    );
}

#[test]
fn cloud_sessions_config_persists_secret_and_feeds_helper_env_without_args() {
    let _guard = crate::storage::lock_test_env();
    let _saved = SavedEnv::capture(&["JCODE_HOME", "JADE_TOKEN_FOR_TEST"]);
    let temp = tempfile::tempdir().expect("tempdir");
    crate::env::set_var("JCODE_HOME", temp.path());
    crate::env::set_var("JADE_TOKEN_FOR_TEST", "secret-token-value");

    run_cloud_sessions_configure(
        Some("https://jade.example".to_string()),
        None,
        Some("JADE_TOKEN_FOR_TEST".to_string()),
        Some("dev-admin".to_string()),
        Some("alice".to_string()),
        Some("/tmp/jade_sessions.py".to_string()),
        false,
    )
    .expect("configure");

    let path = cloud_sessions_config_path().expect("config path");
    assert!(path.exists());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(path.metadata().unwrap().permissions().mode() & 0o777, 0o600);
    }

    let config = load_cloud_sessions_config()
        .expect("load config")
        .expect("config exists");
    assert_eq!(config.api_base.as_deref(), Some("https://jade.example"));
    assert_eq!(config.api_token.as_deref(), Some("secret-token-value"));
    assert_eq!(config.api_token_id.as_deref(), Some("dev-admin"));
    assert_eq!(config.user_id.as_deref(), Some("alice"));
    assert_eq!(config.helper.as_deref(), Some("/tmp/jade_sessions.py"));

    let env = cloud_sessions_helper_env(&config);
    assert!(env.contains(&("JADE_API_BASE", "https://jade.example".to_string())));
    assert!(env.contains(&("JADE_API_TOKEN", "secret-token-value".to_string())));
    assert!(env.contains(&("JADE_API_TOKEN_ID", "dev-admin".to_string())));

    let args = build_jade_sessions_args_with_config(
        CloudSessionsSubcommand::List {
            limit: 2,
            json: true,
            user_id: "dev".to_string(),
            profile: None,
            region: None,
            helper: None,
        },
        &config,
    );
    assert_eq!(
        args,
        vec!["list", "--user-id", "alice", "--limit", "2", "--json"]
    );
    assert!(!args.iter().any(|arg| arg.contains("secret-token-value")));

    run_cloud_sessions_configure(None, None, None, None, None, None, true).expect("clear");
    assert!(!path.exists());
}

#[test]
fn is_syncable_session_stem_filters_non_session_files() {
    assert!(is_syncable_session_stem("session_abc_123"));
    assert!(is_syncable_session_stem("imported_codex_456"));
    assert!(!is_syncable_session_stem("req"));
    assert!(!is_syncable_session_stem("test_selfdev_session"));
    assert!(!is_syncable_session_stem("session_abc.journal"));
}

#[test]
fn collect_sync_candidates_picks_only_session_json() {
    let temp = tempfile::tempdir().expect("tempdir");
    let dir = temp.path();
    std::fs::write(dir.join("session_one.json"), b"{\"id\":\"one\"}").unwrap();
    std::fs::write(dir.join("imported_codex_two.json"), b"{\"id\":\"two\"}").unwrap();
    std::fs::write(dir.join("req.json"), b"{}").unwrap();
    std::fs::write(dir.join("session_three.journal.json"), b"{}").unwrap();
    std::fs::write(dir.join("session_four.bak"), b"{}").unwrap();

    let mut ids: Vec<String> = collect_sync_candidates(dir)
        .expect("collect")
        .into_iter()
        .map(|candidate| candidate.session_id)
        .collect();
    ids.sort();
    assert_eq!(ids, vec!["imported_codex_two", "session_one"]);
}

#[test]
fn cloud_sessions_sync_dry_run_reports_without_uploading_or_writing_state() {
    let _guard = crate::storage::lock_test_env();
    let _saved = SavedEnv::capture(&["JCODE_HOME", "JCODE_JADE_SESSIONS_HELPER"]);
    let temp = tempfile::tempdir().expect("tempdir");
    crate::env::set_var("JCODE_HOME", temp.path());

    // A dummy helper that should never run during a dry run.
    let helper = temp.path().join("never_runs.sh");
    std::fs::write(&helper, b"#!/bin/sh\nexit 7\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    crate::env::set_var("JCODE_JADE_SESSIONS_HELPER", &helper);

    let sessions_dir = temp.path().join("sessions");
    std::fs::create_dir_all(&sessions_dir).unwrap();
    std::fs::write(sessions_dir.join("session_alpha.json"), b"{\"id\":\"a\"}").unwrap();
    std::fs::write(sessions_dir.join("session_beta.json"), b"{\"id\":\"b\"}").unwrap();

    run_cloud_sessions_sync(CloudSessionsSyncRequest {
        sessions_dir: Some(sessions_dir.display().to_string()),
        since_days: None,
        all: true,
        max: 50,
        min_interval_mins: None,
        raw: false,
        dry_run: true,
        force: false,
        json: true,
        user_id: "dev".to_string(),
        profile: None,
        region: None,
        helper: None,
    })
    .expect("dry run sync");

    // Dry run must not persist any sync state.
    assert!(!cloud_sessions_sync_state_path().unwrap().exists());
}

#[test]
fn cloud_sessions_sync_respects_min_interval_throttle() {
    let _guard = crate::storage::lock_test_env();
    let _saved = SavedEnv::capture(&["JCODE_HOME", "JCODE_JADE_SESSIONS_HELPER"]);
    let temp = tempfile::tempdir().expect("tempdir");
    crate::env::set_var("JCODE_HOME", temp.path());

    // Helper that would fail loudly if it ever ran during a throttled run.
    let helper = temp.path().join("must_not_run.sh");
    std::fs::write(&helper, b"#!/bin/sh\nexit 13\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    crate::env::set_var("JCODE_JADE_SESSIONS_HELPER", &helper);

    let sessions_dir = temp.path().join("sessions");
    std::fs::create_dir_all(&sessions_dir).unwrap();
    std::fs::write(sessions_dir.join("session_gamma.json"), b"{\"id\":\"g\"}").unwrap();

    // Seed sync state with a very recent last_sync_at so throttle should trigger.
    let state = CloudSessionsSyncState {
        last_sync_at: Some(chrono::Utc::now().to_rfc3339()),
        ..Default::default()
    };
    save_cloud_sessions_sync_state(&state).expect("seed state");

    // Should be skipped (not error) because last sync was just now.
    run_cloud_sessions_sync(CloudSessionsSyncRequest {
        sessions_dir: Some(sessions_dir.display().to_string()),
        since_days: None,
        all: true,
        max: 50,
        min_interval_mins: Some(60),
        raw: false,
        dry_run: false,
        force: false,
        json: true,
        user_id: "dev".to_string(),
        profile: None,
        region: None,
        helper: None,
    })
    .expect("throttled sync returns ok without running helper");

    // The session should NOT be recorded as uploaded.
    let reloaded = load_cloud_sessions_sync_state().expect("reload state");
    assert!(!reloaded.sessions.contains_key("session_gamma"));
}

#[test]
fn render_cloud_sessions_dashboard_html_escapes_and_lists_rows() {
    let items: Vec<CloudSessionListItem> = serde_json::from_str(
        r#"[
          {"session_id":"session_x","title":"Hello <b> & \"world\"","message_count":12,"uploaded_at":"2026-05-29T00:00:00Z"},
          {"session_id":"session_y","short_name":"shorty","message_count":"3","uploaded_at":"2026-05-28T00:00:00Z"}
        ]"#,
    )
    .expect("parse items");

    let html =
        render_cloud_sessions_dashboard_html("alice", &items, &std::collections::BTreeMap::new());
    assert!(html.contains("Jade Cloud Sessions"));
    assert!(html.contains("user: alice"));
    assert!(html.contains("2 session(s)"));
    assert!(html.contains("session_x"));
    assert!(html.contains("shorty"));
    // Raw title must be escaped (no live markup, quotes escaped).
    assert!(!html.contains("Hello <b>"));
    assert!(html.contains("Hello &lt;b&gt; &amp; &quot;world&quot;"));
    // Numeric and string message counts both render.
    assert!(html.contains(">12<"));
    assert!(html.contains(">3<"));
}

#[test]
fn render_cloud_sessions_dashboard_html_handles_empty() {
    let html = render_cloud_sessions_dashboard_html("dev", &[], &std::collections::BTreeMap::new());
    assert!(html.contains("0 session(s)"));
    assert!(html.contains("No uploaded sessions found."));
}

#[test]
fn render_cloud_sessions_dashboard_html_links_rows_with_view_files() {
    let items: Vec<CloudSessionListItem> = serde_json::from_str(
        r#"[
          {"session_id":"session_x","title":"X","message_count":1,"uploaded_at":"2026-05-29T00:00:00Z"},
          {"session_id":"session_y","title":"Y","message_count":2,"uploaded_at":"2026-05-28T00:00:00Z"}
        ]"#,
    )
    .expect("parse items");
    let mut links = std::collections::BTreeMap::new();
    links.insert(
        "session_x".to_string(),
        "dash-views/session_x.html".to_string(),
    );

    let html = render_cloud_sessions_dashboard_html("alice", &items, &links);
    // Linked session gets an anchor to its relative viewer file.
    assert!(html.contains("<a href='dash-views/session_x.html'>session_x</a>"));
    // Session without a generated viewer stays plain text (no anchor).
    assert!(html.contains("<td class='id'>session_y</td>"));
}

#[test]
fn sanitize_filename_keeps_safe_chars_and_replaces_others() {
    assert_eq!(
        sanitize_filename("session_abc-123.json"),
        "session_abc-123.json"
    );
    assert_eq!(sanitize_filename("a/b c:d"), "a_b_c_d");
}

#[test]
fn dashboard_views_dir_is_sibling_of_dashboard() {
    let dir = dashboard_views_dir(std::path::Path::new("/tmp/out/dash.html"));
    assert_eq!(dir, std::path::PathBuf::from("/tmp/out/dash-views"));
}

#[test]
fn relative_link_is_relative_to_dashboard_parent() {
    let link = relative_link(
        std::path::Path::new("/tmp/out/dash.html"),
        std::path::Path::new("/tmp/out/dash-views/session_x.html"),
    );
    assert_eq!(link.as_deref(), Some("dash-views/session_x.html"));
}

#[test]
fn parse_cloud_session_list_json_accepts_array_and_object_wrappers() {
    // Real helper shape: a top-level array.
    let array = parse_cloud_session_list_json(
        r#"[{"session_id":"session_a","message_count":2,"uploaded_at":"2026-05-29T00:00:00Z"}]"#,
    )
    .expect("parse array");
    assert_eq!(array.len(), 1);
    assert_eq!(array[0].session_id.as_deref(), Some("session_a"));

    // Tolerated object wrappers.
    let items = parse_cloud_session_list_json(r#"{"items":[{"session_id":"session_b"}]}"#)
        .expect("parse items wrapper");
    assert_eq!(items[0].session_id.as_deref(), Some("session_b"));

    let sessions = parse_cloud_session_list_json(r#"{"sessions":[{"session_id":"session_c"}]}"#)
        .expect("parse sessions wrapper");
    assert_eq!(sessions[0].session_id.as_deref(), Some("session_c"));

    // Empty array stays empty.
    assert!(
        parse_cloud_session_list_json("[]")
            .expect("parse empty")
            .is_empty()
    );
}

#[test]
fn parse_cloud_session_list_json_rejects_unexpected_shapes() {
    // A bare object without a recognized array key is an error.
    let err = parse_cloud_session_list_json(r#"{"unexpected":true}"#)
        .expect_err("object without items/sessions");
    assert!(err.to_string().contains("items"));

    // A scalar is also rejected with a descriptive message.
    let err = parse_cloud_session_list_json("42").expect_err("scalar");
    assert!(err.to_string().contains("a number"));
}

#[test]
fn resolve_jade_sessions_helper_prefers_explicit_and_env_paths() {
    let _saved = SavedEnv::capture(&["JCODE_JADE_SESSIONS_HELPER"]);
    crate::env::set_var("JCODE_JADE_SESSIONS_HELPER", "/tmp/from-env.py");

    assert_eq!(
        resolve_jade_sessions_helper(Some("/tmp/explicit.py")).unwrap(),
        std::path::PathBuf::from("/tmp/explicit.py")
    );
    assert_eq!(
        resolve_jade_sessions_helper(None).unwrap(),
        std::path::PathBuf::from("/tmp/from-env.py")
    );
}

#[test]
fn auth_test_retryable_error_detection_handles_rate_limits() {
    let err = anyhow::anyhow!(
        "Gemini request generateContent failed (HTTP 429 Too Many Requests): RESOURCE_EXHAUSTED"
    );
    assert!(auth_test_error_is_retryable(&err));
}

#[test]
fn auth_test_retryable_error_detection_rejects_schema_errors() {
    let err = anyhow::anyhow!(
        "Gemini request generateContent failed (HTTP 400 Bad Request): invalid argument"
    );
    assert!(!auth_test_error_is_retryable(&err));
}

#[tokio::test]
async fn auth_test_choice_plan_preserves_explicit_model_for_local_provider() {
    let plan = auth_test_choice_plan(
        &super::super::provider_init::ProviderChoice::Ollama,
        Some("llama3.2"),
    )
    .await
    .expect("choice plan");

    match plan {
        AuthTestChoicePlan::Run { model } => assert_eq!(model.as_deref(), Some("llama3.2")),
        AuthTestChoicePlan::Skip(detail) => panic!("unexpected skip: {detail}"),
    }
}

#[tokio::test]
async fn auth_test_choice_plan_leaves_non_compat_provider_unchanged() {
    let plan = auth_test_choice_plan(
        &super::super::provider_init::ProviderChoice::Openrouter,
        None,
    )
    .await
    .expect("choice plan");

    match plan {
        AuthTestChoicePlan::Run { model } => assert!(model.is_none()),
        AuthTestChoicePlan::Skip(detail) => panic!("unexpected skip: {detail}"),
    }
}

#[tokio::test]
async fn auth_test_choice_plan_discovers_model_for_local_custom_compat_endpoint() {
    let _env_guard = crate::storage::lock_test_env();
    let _saved = SavedEnv::capture(&[
        "JCODE_OPENAI_COMPAT_API_BASE",
        "JCODE_OPENAI_COMPAT_API_KEY_NAME",
        "JCODE_OPENAI_COMPAT_ENV_FILE",
        "JCODE_OPENAI_COMPAT_DEFAULT_MODEL",
        "JCODE_OPENAI_COMPAT_LOCAL_ENABLED",
        "JCODE_OPENROUTER_API_BASE",
        "JCODE_OPENROUTER_API_KEY_NAME",
        "JCODE_OPENROUTER_ENV_FILE",
        "JCODE_OPENROUTER_ALLOW_NO_AUTH",
    ]);
    let api_base = spawn_single_response_http_server(200, r#"{"data":[{"id":"llama3.2"}]}"#);
    crate::env::set_var("JCODE_OPENAI_COMPAT_API_BASE", &api_base);
    crate::env::remove_var("JCODE_OPENAI_COMPAT_DEFAULT_MODEL");
    crate::env::remove_var("JCODE_OPENAI_COMPAT_LOCAL_ENABLED");
    crate::provider_catalog::apply_openai_compatible_profile_env(None);

    let plan = auth_test_choice_plan(
        &super::super::provider_init::ProviderChoice::OpenaiCompatible,
        None,
    )
    .await
    .expect("choice plan");

    match plan {
        AuthTestChoicePlan::Run { model } => assert_eq!(model.as_deref(), Some("llama3.2")),
        AuthTestChoicePlan::Skip(detail) => panic!("unexpected skip: {detail}"),
    }
}

#[tokio::test]
async fn auth_test_choice_plan_discovers_model_for_hosted_custom_compat_endpoint_with_api_key() {
    let _env_guard = crate::storage::lock_test_env();
    let _saved = SavedEnv::capture(&[
        "JCODE_OPENAI_COMPAT_API_BASE",
        "JCODE_OPENAI_COMPAT_API_KEY_NAME",
        "JCODE_OPENAI_COMPAT_ENV_FILE",
        "JCODE_OPENAI_COMPAT_DEFAULT_MODEL",
        "JCODE_OPENAI_COMPAT_LOCAL_ENABLED",
        "JCODE_OPENROUTER_API_BASE",
        "JCODE_OPENROUTER_API_KEY_NAME",
        "JCODE_OPENROUTER_ENV_FILE",
        "JCODE_OPENROUTER_ALLOW_NO_AUTH",
        "OPENAI_COMPAT_API_KEY",
        "NO_PROXY",
        "no_proxy",
    ]);
    // 0.0.0.0 is accepted as an insecure HTTP test host but is not treated as
    // localhost by resolve_openai_compatible_profile, so this exercises the
    // hosted/API-key code path while still serving the response locally.
    let api_base = spawn_single_response_http_server_on_host(
        "0.0.0.0",
        200,
        r#"{"data":[{"id":"hosted-compatible-model"}]}"#,
    );
    crate::env::set_var("JCODE_OPENAI_COMPAT_API_BASE", &api_base);
    crate::env::set_var("OPENAI_COMPAT_API_KEY", "test-key");
    crate::env::set_var("NO_PROXY", "0.0.0.0,127.0.0.1,localhost");
    crate::env::set_var("no_proxy", "0.0.0.0,127.0.0.1,localhost");
    crate::env::remove_var("JCODE_OPENAI_COMPAT_DEFAULT_MODEL");
    crate::env::remove_var("JCODE_OPENAI_COMPAT_LOCAL_ENABLED");
    crate::provider_catalog::apply_openai_compatible_profile_env(None);

    let resolved = crate::provider_catalog::resolve_openai_compatible_profile(
        crate::provider_catalog::OPENAI_COMPAT_PROFILE,
    );
    assert!(resolved.requires_api_key);

    let plan = auth_test_choice_plan(
        &super::super::provider_init::ProviderChoice::OpenaiCompatible,
        None,
    )
    .await
    .expect("choice plan");

    match plan {
        AuthTestChoicePlan::Run { model } => {
            assert_eq!(model.as_deref(), Some("hosted-compatible-model"))
        }
        AuthTestChoicePlan::Skip(detail) => panic!("unexpected skip: {detail}"),
    }
}

#[tokio::test]
async fn auth_test_choice_plan_skips_local_custom_compat_endpoint_without_models() {
    let _env_guard = crate::storage::lock_test_env();
    let _saved = SavedEnv::capture(&[
        "JCODE_OPENAI_COMPAT_API_BASE",
        "JCODE_OPENAI_COMPAT_API_KEY_NAME",
        "JCODE_OPENAI_COMPAT_ENV_FILE",
        "JCODE_OPENAI_COMPAT_DEFAULT_MODEL",
        "JCODE_OPENAI_COMPAT_LOCAL_ENABLED",
        "JCODE_OPENROUTER_API_BASE",
        "JCODE_OPENROUTER_API_KEY_NAME",
        "JCODE_OPENROUTER_ENV_FILE",
        "JCODE_OPENROUTER_ALLOW_NO_AUTH",
    ]);
    let api_base = spawn_single_response_http_server(200, r#"{"data":[]}"#);
    crate::env::set_var("JCODE_OPENAI_COMPAT_API_BASE", &api_base);
    crate::env::remove_var("JCODE_OPENAI_COMPAT_DEFAULT_MODEL");
    crate::env::remove_var("JCODE_OPENAI_COMPAT_LOCAL_ENABLED");
    crate::provider_catalog::apply_openai_compatible_profile_env(None);

    let plan = auth_test_choice_plan(
        &super::super::provider_init::ProviderChoice::OpenaiCompatible,
        None,
    )
    .await
    .expect("choice plan");

    match plan {
        AuthTestChoicePlan::Run { model } => panic!("unexpected run plan: {model:?}"),
        AuthTestChoicePlan::Skip(detail) => {
            assert!(detail.contains("reported no models"));
            assert!(detail.contains("openai-compatible"));
        }
    }
}

#[test]
fn collect_cli_model_names_falls_back_when_no_routes_are_available() {
    let routes = vec![ModelRoute {
        model: "claude-opus-4-6".to_string(),
        provider: "Anthropic".to_string(),
        api_method: "claude-oauth".to_string(),
        available: false,
        detail: "no credentials".to_string(),
        cheapness: None,
    }];

    let models = collect_cli_model_names(&routes, vec!["gpt-5.4".to_string()]);

    assert_eq!(models, vec!["claude-opus-4-6", "gpt-5.4"]);
}

#[test]
fn list_cli_providers_includes_auto_and_openai() {
    let providers = super::report_info::list_cli_providers();
    assert!(providers.iter().any(|provider| provider.id == "auto"));
    assert!(providers.iter().any(|provider| {
        provider.id == "openai"
            && provider.display_name == "OpenAI"
            && provider.auth_kind.as_deref() == Some("OAuth")
    }));
    assert!(providers.iter().any(|provider| provider.id == "groq"));
    assert!(providers.iter().any(|provider| provider.id == "xai"));
}

#[test]
fn list_cli_providers_includes_conifer() {
    let providers = super::report_info::list_cli_providers();
    let conifer = providers
        .iter()
        .find(|provider| provider.id == "conifer")
        .expect("Conifer should be discoverable through `provider list`");

    assert_eq!(conifer.display_name, "Conifer");
    assert_eq!(conifer.auth_kind.as_deref(), Some("API key"));
}

#[test]
fn version_command_plain_output_includes_core_fields() {
    let report = super::report_info::VersionReport {
        version: "v1.2.3 (abc1234)".to_string(),
        semver: "1.2.3".to_string(),
        base_semver: "1.2.0".to_string(),
        update_semver: "1.2.0".to_string(),
        git_hash: "abc1234".to_string(),
        git_tag: "v1.2.3".to_string(),
        build_time: "2026-03-18 18:00:00 +0000".to_string(),
        git_date: "2026-03-18 17:59:00 +0000".to_string(),
        release_build: false,
    };
    let text = format!(
        "version\t{}\nsemver\t{}\nbase_semver\t{}\nupdate_semver\t{}\ngit_hash\t{}\ngit_tag\t{}\nbuild_time\t{}\ngit_date\t{}\nrelease_build\t{}\n",
        report.version,
        report.semver,
        report.base_semver,
        report.update_semver,
        report.git_hash,
        report.git_tag,
        report.build_time,
        report.git_date,
        report.release_build
    );

    assert!(text.contains("version\tv1.2.3 (abc1234)"));
    assert!(text.contains("semver\t1.2.3"));
    assert!(text.contains("git_hash\tabc1234"));
    assert!(text.contains("release_build\tfalse"));
}

#[tokio::test]
async fn restore_agent_session_if_requested_restores_resumed_session() {
    let _guard = crate::storage::lock_test_env();

    let provider: Arc<dyn Provider> = Arc::new(TestProvider);
    let registry = Registry::new(provider.clone()).await;
    let mut original = crate::agent::Agent::new(provider.clone(), registry);
    let original_session_id = original.session_id().to_string();
    original
        .run_once_capture("seed session for resume test")
        .await
        .expect("seed session");

    let registry = Registry::new(provider.clone()).await;
    let mut resumed = crate::agent::Agent::new(provider, registry);
    let fresh_session_id = resumed.session_id().to_string();
    assert_ne!(fresh_session_id, original_session_id);

    restore_agent_session_if_requested(&mut resumed, Some(&original_session_id))
        .expect("restore session");

    assert_eq!(resumed.session_id(), original_session_id);
}

#[tokio::test]
async fn one_shot_output_modes_close_sessions_and_clear_active_pid_markers() {
    let _guard = crate::storage::lock_test_env();
    let _saved = SavedEnv::capture(&["JCODE_HOME", "JCODE_RUN_AUTO_POKE"]);
    let temp = tempfile::tempdir().expect("tempdir");
    crate::env::set_var("JCODE_HOME", temp.path());
    crate::env::set_var("JCODE_RUN_AUTO_POKE", "0");

    for (mode, emit_json, emit_ndjson) in [
        ("plain", false, false),
        ("json", true, false),
        ("ndjson", false, true),
    ] {
        let provider: Arc<dyn Provider> = Arc::new(TestProvider);
        let registry = Registry::new(provider.clone()).await;
        let mut agent = crate::agent::Agent::new(provider.clone(), registry);
        let session_id = agent.session_id().to_string();
        let marker = crate::storage::active_pids_dir()
            .expect("active PID directory")
            .join(&session_id);
        assert!(marker.exists(), "{mode} session should start active");

        run_single_message_with_agent(
            &mut agent,
            provider,
            "Return the test response.",
            emit_json,
            emit_ndjson,
        )
        .await
        .unwrap_or_else(|error| panic!("{mode} run failed: {error:#}"));

        assert!(
            !marker.exists(),
            "{mode} run left an active PID marker behind"
        );
        let persisted = crate::session::Session::load(&session_id)
            .unwrap_or_else(|error| panic!("load {mode} session: {error:#}"));
        assert!(
            matches!(persisted.status, crate::session::SessionStatus::Closed),
            "{mode} run persisted status {:?}",
            persisted.status
        );
        assert!(
            !crate::session::find_recent_crashed_sessions()
                .iter()
                .any(|(id, _)| id == &session_id),
            "{mode} run was rediscovered as a stale-PID crash"
        );
    }
}

#[tokio::test]
async fn resumed_one_shot_closes_the_restored_session() {
    let _guard = crate::storage::lock_test_env();
    let _saved = SavedEnv::capture(&["JCODE_HOME", "JCODE_RUN_AUTO_POKE"]);
    let temp = tempfile::tempdir().expect("tempdir");
    crate::env::set_var("JCODE_HOME", temp.path());
    crate::env::set_var("JCODE_RUN_AUTO_POKE", "0");

    let provider: Arc<dyn Provider> = Arc::new(TestProvider);
    let registry = Registry::new(provider.clone()).await;
    let mut original = crate::agent::Agent::new(provider.clone(), registry);
    original
        .run_once_capture("Seed the resumed one-shot session.")
        .await
        .expect("seed resumed session");
    let session_id = original.session_id().to_string();
    original.mark_closed();

    let registry = Registry::new(provider.clone()).await;
    let mut resumed = crate::agent::Agent::new(provider.clone(), registry);
    restore_agent_session_if_requested(&mut resumed, Some(&session_id))
        .expect("restore one-shot session");
    let marker = crate::storage::active_pids_dir()
        .expect("active PID directory")
        .join(&session_id);
    assert!(
        marker.exists(),
        "restored session should be active while running"
    );

    run_single_message_with_agent(
        &mut resumed,
        provider,
        "Finish the resumed one-shot session.",
        true,
        false,
    )
    .await
    .expect("run resumed one-shot session");

    assert!(!marker.exists(), "resumed run left its active PID marker");
    let persisted = crate::session::Session::load(&session_id).expect("load resumed session");
    assert!(matches!(
        persisted.status,
        crate::session::SessionStatus::Closed
    ));
    assert!(
        !crate::session::find_recent_crashed_sessions()
            .iter()
            .any(|(id, _)| id == &session_id),
        "resumed run was rediscovered as a stale-PID crash"
    );
}

#[tokio::test]
async fn one_shot_cleanup_preserves_the_original_command_error() {
    let _guard = crate::storage::lock_test_env();
    let _saved = SavedEnv::capture(&["JCODE_HOME", "JCODE_RUN_AUTO_POKE"]);
    let temp = tempfile::tempdir().expect("tempdir");
    crate::env::set_var("JCODE_HOME", temp.path());
    crate::env::set_var("JCODE_RUN_AUTO_POKE", "0");

    for (mode, emit_json, emit_ndjson) in [
        ("plain", false, false),
        ("json", true, false),
        ("ndjson", false, true),
    ] {
        let provider: Arc<dyn Provider> = Arc::new(FailingTestProvider);
        let registry = Registry::new(provider.clone()).await;
        let mut agent = crate::agent::Agent::new(provider.clone(), registry);
        let session_id = agent.session_id().to_string();
        let marker = crate::storage::active_pids_dir()
            .expect("active PID directory")
            .join(&session_id);

        let error = match run_single_message_with_agent(
            &mut agent,
            provider,
            "Fail this run.",
            emit_json,
            emit_ndjson,
        )
        .await
        {
            Ok(()) => panic!("{mode} provider failure should remain the command result"),
            Err(error) => error,
        };

        assert!(
            error.to_string().contains("one-shot sentinel failure"),
            "{mode} changed the original error: {error:#}"
        );
        assert!(
            !marker.exists(),
            "failed {mode} run left an active PID marker"
        );
        let persisted = crate::session::Session::load(&session_id)
            .unwrap_or_else(|error| panic!("load failed {mode} session: {error:#}"));
        assert!(matches!(
            persisted.status,
            crate::session::SessionStatus::Closed
        ));
    }
}
