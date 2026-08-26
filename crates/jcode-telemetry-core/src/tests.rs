use super::*;
use std::{
    ffi::OsString,
    sync::{Mutex, MutexGuard, OnceLock},
};

// All of these tests mutate process-global state: the env-var opt-out tests
// flip `JCODE_NO_TELEMETRY` / `DO_NOT_TRACK`, while the session tests drive the
// global `SESSION_STATE`. They must be serialized against *each other* with a
// single shared lock. Using two separate locks previously let an env test
// disable telemetry (`is_enabled() == false`) while a session test was calling
// `begin_session_with_mode`, which then returned early and left `SESSION_STATE`
// as `None`; the session test's `expect(...)` panicked while holding the
// `SESSION_STATE` lock and poisoned it, cascading into `PoisonError` failures
// in every other session test.
struct TestEnvironment {
    _home: tempfile::TempDir,
    previous_home: Option<OsString>,
    previous_no_telemetry: Option<OsString>,
    previous_do_not_track: Option<OsString>,
    _lock: MutexGuard<'static, ()>,
}

impl Drop for TestEnvironment {
    fn drop(&mut self) {
        restore_env_var("JCODE_HOME", self.previous_home.take());
        restore_env_var("JCODE_NO_TELEMETRY", self.previous_no_telemetry.take());
        restore_env_var("DO_NOT_TRACK", self.previous_do_not_track.take());
    }
}

fn restore_env_var(key: &str, value: Option<OsString>) {
    if let Some(value) = value {
        jcode_core::env::set_var(key, value);
    } else {
        jcode_core::env::remove_var(key);
    }
}

fn global_test_lock() -> TestEnvironment {
    static TEST_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    let lock = TEST_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let home = tempfile::tempdir().expect("create isolated telemetry test home");
    let previous_home = std::env::var_os("JCODE_HOME");
    let previous_no_telemetry = std::env::var_os("JCODE_NO_TELEMETRY");
    let previous_do_not_track = std::env::var_os("DO_NOT_TRACK");
    jcode_core::env::set_var("JCODE_HOME", home.path());
    jcode_core::env::remove_var("JCODE_NO_TELEMETRY");
    jcode_core::env::remove_var("DO_NOT_TRACK");

    TestEnvironment {
        _home: home,
        previous_home,
        previous_no_telemetry,
        previous_do_not_track,
        _lock: lock,
    }
}

#[test]
fn permanent_telemetry_statuses_trip_the_process_breaker() {
    assert!(telemetry_status_is_permanent(400));
    assert!(telemetry_status_is_permanent(401));
    assert!(telemetry_status_is_permanent(404));
    assert!(!telemetry_status_is_permanent(408));
    assert!(!telemetry_status_is_permanent(425));
    assert!(!telemetry_status_is_permanent(429));
    assert!(!telemetry_status_is_permanent(500));
}

#[test]
fn background_delivery_queue_is_bounded() {
    let (started_tx, started_rx) = std::sync::mpsc::sync_channel(1);
    let (release_tx, release_rx) = std::sync::mpsc::sync_channel(1);
    let sender = spawn_background_worker(1, move |_| {
        let _ = started_tx.send(());
        let _ = release_rx.recv();
    })
    .expect("start test telemetry worker");

    sender
        .send(serde_json::json!({"event": "first"}))
        .expect("enqueue first payload");
    started_rx.recv().expect("worker started first payload");
    sender
        .try_send(serde_json::json!({"event": "second"}))
        .expect("bounded queue accepts one waiting payload");
    assert!(matches!(
        sender.try_send(serde_json::json!({"event": "third"})),
        Err(std::sync::mpsc::TrySendError::Full(_))
    ));

    release_tx.send(()).expect("release telemetry worker");
}

#[test]
fn telemetry_endpoint_uses_production_custom_domain() {
    assert_eq!(TELEMETRY_ENDPOINT, "https://telemetry.jcode.sh/v1/event");
    assert_eq!(
        TRANSCRIPT_ENDPOINT,
        "https://telemetry.jcode.sh/v1/transcript"
    );
}

#[test]
fn transcript_upload_requires_separate_content_consent() {
    let _guard = lock_test_env();
    TEST_EMITTED_PAYLOADS.lock().unwrap().clear();

    assert!(!record_transcript(
        "provider",
        "model",
        SessionEndReason::NormalExit,
        serde_json::json!([{"role": "user", "content": "secret prompt"}]),
    ));
    assert!(TEST_EMITTED_PAYLOADS.lock().unwrap().is_empty());
}

#[test]
fn opted_in_transcript_payload_contains_full_structured_messages() {
    let _guard = lock_test_env();
    TEST_EMITTED_PAYLOADS.lock().unwrap().clear();
    assert!(set_content_sharing_enabled(true));
    let messages = serde_json::json!([
        {"role": "user", "content": [{"type": "text", "text": "secret prompt"}]},
        {"role": "assistant", "content": [{"type": "reasoning", "text": "private reasoning"}]}
    ]);

    assert!(record_transcript(
        "provider",
        "model",
        SessionEndReason::NormalExit,
        messages.clone(),
    ));
    let payloads = TEST_EMITTED_PAYLOADS.lock().unwrap();
    let payload = payloads.last().expect("transcript payload");
    assert_eq!(payload["event"], "transcript");
    assert_eq!(payload["consent_version"], 1);
    assert_eq!(payload["message_count"], 2);
    assert_eq!(payload["messages"], messages);
    assert!(uuid::Uuid::parse_str(payload["upload_id"].as_str().unwrap()).is_ok());
}

fn lock_test_env() -> TestEnvironment {
    global_test_lock()
}

fn lock_telemetry_test_state() -> TestEnvironment {
    global_test_lock()
}

#[test]
fn test_opt_out_env_var() {
    let _guard = lock_test_env();
    jcode_core::env::set_var("JCODE_NO_TELEMETRY", "1");
    assert!(!is_enabled());
    jcode_core::env::remove_var("JCODE_NO_TELEMETRY");
}

#[test]
fn test_do_not_track() {
    let _guard = lock_test_env();
    jcode_core::env::set_var("DO_NOT_TRACK", "1");
    assert!(!is_enabled());
    jcode_core::env::remove_var("DO_NOT_TRACK");
}

#[test]
fn false_opt_out_values_keep_telemetry_enabled() {
    let _guard = lock_test_env();
    for value in ["", "0", "false", "no", "off"] {
        jcode_core::env::set_var("JCODE_NO_TELEMETRY", value);
        jcode_core::env::set_var("DO_NOT_TRACK", value);
        assert!(is_enabled(), "{value:?} should not opt out");
    }
}

#[test]
fn in_app_opt_out_emits_once_then_stops_telemetry() {
    let _guard = lock_test_env();
    TEST_EMITTED_PAYLOADS.lock().unwrap().clear();

    assert!(set_usage_telemetry_enabled(false));
    assert!(!is_enabled());
    assert!(set_usage_telemetry_enabled(false));

    let payloads = TEST_EMITTED_PAYLOADS.lock().unwrap();
    let opt_outs = payloads
        .iter()
        .filter(|payload| payload["event"] == "telemetry_opt_out")
        .collect::<Vec<_>>();
    assert_eq!(opt_outs.len(), 1);
    assert_eq!(opt_outs[0]["step"], "telemetry_settings");
}

#[test]
fn record_turn_emits_prompt_submitted_immediately() {
    let _guard = lock_telemetry_test_state();
    TEST_EMITTED_PAYLOADS.lock().unwrap().clear();
    if let Ok(mut session) = SESSION_STATE.lock() {
        *session = None;
    }
    begin_session_with_mode("openai", "gpt-test", None, false);

    record_turn();

    let payloads = TEST_EMITTED_PAYLOADS.lock().unwrap().clone();
    let prompt = payloads
        .iter()
        .find(|payload| payload["event"] == "prompt_submitted")
        .expect("prompt_submitted should be emitted before turn completion");
    assert_eq!(prompt["turn_index"], 1);
    assert!(prompt["session_id"].is_string());
    if let Ok(mut session) = SESSION_STATE.lock() {
        *session = None;
    }
}

#[test]
fn telemetry_status_on_fresh_home_is_read_only() {
    let _guard = lock_test_env();

    let snapshot = status();

    assert!(snapshot.enabled);
    assert_eq!(snapshot.opt_out_source, None);
    assert_eq!(snapshot.telemetry_id, None);
    assert!(!snapshot.content_sharing_enabled);
    assert!(!telemetry_id_path().expect("telemetry id path").exists());
}

#[test]
fn telemetry_status_reads_an_existing_id_without_replacing_it() {
    let _guard = lock_test_env();
    let path = telemetry_id_path().expect("telemetry id path");
    write_private_file(&path, "existing-id\n");

    assert_eq!(status().telemetry_id.as_deref(), Some("existing-id"));
    assert_eq!(std::fs::read_to_string(path).unwrap(), "existing-id\n");
}

#[test]
fn telemetry_status_reports_marker_file_opt_out() {
    let _guard = lock_test_env();
    assert!(set_usage_telemetry_enabled(false));

    let snapshot = status();

    assert!(!snapshot.enabled);
    assert_eq!(
        snapshot.opt_out_source,
        Some(TelemetryOptOutSource::MarkerFile)
    );
}

#[test]
fn environment_opt_out_takes_precedence_over_marker_file() {
    let _guard = lock_test_env();
    assert!(set_usage_telemetry_enabled(false));
    jcode_core::env::set_var("DO_NOT_TRACK", "1");

    let snapshot = status();

    assert!(!snapshot.enabled);
    assert_eq!(
        snapshot.opt_out_source,
        Some(TelemetryOptOutSource::Environment)
    );
}

#[test]
fn test_is_ci_detects_ci_env() {
    let _guard = lock_test_env();
    // Clear any inherited CI markers so the baseline is deterministic.
    for key in [
        "JCODE_CI",
        "CI",
        "CONTINUOUS_INTEGRATION",
        "BUILD_NUMBER",
        "GITHUB_ACTIONS",
        "BUILDKITE",
        "JENKINS_URL",
        "GITLAB_CI",
        "CIRCLECI",
        "TRAVIS",
        "TEAMCITY_VERSION",
        "TF_BUILD",
        "CODEBUILD_BUILD_ID",
        "DRONE",
        "APPVEYOR",
        "WOODPECKER",
        "BITBUCKET_BUILD_NUMBER",
        "NEXTEST",
        "JCODE_E2E_BIN",
    ] {
        jcode_core::env::remove_var(key);
    }
    assert!(
        !is_ci(),
        "expected non-CI baseline after clearing CI markers"
    );
    jcode_core::env::set_var("CI", "true");
    assert!(
        is_ci(),
        "CI env var should mark the run as CI (gates install skip)"
    );
    jcode_core::env::remove_var("CI");
    assert!(!is_ci());

    jcode_core::env::set_var("JCODE_CI", "1");
    assert!(is_ci(), "explicit JCODE_CI=1 should mark any runtime as CI");
    jcode_core::env::remove_var("JCODE_CI");

    jcode_core::env::set_var("CI", "true");
    jcode_core::env::set_var("JCODE_CI", "0");
    assert!(
        !is_ci(),
        "explicit JCODE_CI=0 should override inherited provider markers"
    );
    jcode_core::env::remove_var("JCODE_CI");
    jcode_core::env::remove_var("CI");

    jcode_core::env::set_var("CI", "true");
    jcode_core::env::set_var("JCODE_CI", "invalid");
    assert!(
        is_ci(),
        "invalid explicit values should fall back to provider detection"
    );
    jcode_core::env::remove_var("JCODE_CI");
    jcode_core::env::remove_var("CI");

    // Vendor-specific markers count on their own: several providers never set
    // the generic `CI` variable, and those runners used to look like people.
    for key in [
        "TEAMCITY_VERSION",
        "TF_BUILD",
        "DRONE",
        "BITBUCKET_BUILD_NUMBER",
    ] {
        jcode_core::env::set_var(key, "1");
        assert!(is_ci(), "{key} should mark the run as CI");
        jcode_core::env::remove_var(key);
        assert!(!is_ci());
    }

    // Test harnesses are automation: they mint a throwaway id per process.
    jcode_core::env::set_var("NEXTEST", "1");
    assert!(is_ci(), "nextest runs should be tagged as automation");
    jcode_core::env::remove_var("NEXTEST");
    assert!(!is_ci());
}

#[test]
fn test_error_counters() {
    let _guard = lock_telemetry_test_state();
    if let Ok(mut session) = SESSION_STATE.lock() {
        *session = None;
    }
    begin_session_with_mode("openai", "gpt-5.4", None, false);
    record_error(ErrorCategory::ProviderTimeout);
    record_error(ErrorCategory::ProviderTimeout);
    record_error(ErrorCategory::ToolError);
    {
        let guard = SESSION_STATE.lock().unwrap();
        let state = guard.as_ref().expect("session telemetry state");
        assert_eq!(state.error_provider_timeout, 2);
        assert_eq!(state.error_tool_error, 1);
        let errors = current_error_counts(state);
        assert_eq!(errors.provider_timeout, 2);
        assert_eq!(errors.tool_error, 1);
    }
    if let Ok(mut session) = SESSION_STATE.lock() {
        *session = None;
    }
}

#[test]
fn test_error_counter_caps_per_session() {
    let _guard = lock_telemetry_test_state();
    if let Ok(mut session) = SESSION_STATE.lock() {
        *session = None;
    }
    begin_session_with_mode("openai", "gpt-5.4", None, false);
    // A runaway retry loop once logged 18k+ auth failures in one session and
    // distorted daily aggregates. The counter must saturate at the cap.
    for _ in 0..600 {
        record_error(ErrorCategory::AuthFailed);
    }
    {
        let guard = SESSION_STATE.lock().unwrap();
        let state = guard.as_ref().expect("session telemetry state");
        assert_eq!(state.error_auth_failed, 500);
    }
    if let Ok(mut session) = SESSION_STATE.lock() {
        *session = None;
    }
}

#[test]
fn test_error_counters_no_session_is_noop() {
    let _guard = lock_telemetry_test_state();
    // Errors recorded with no active session must not bump any counter that a
    // future session could observe (issue #394: counts drifting across the
    // session boundary).
    if let Ok(mut session) = SESSION_STATE.lock() {
        *session = None;
    }
    record_error(ErrorCategory::AuthFailed);
    record_provider_switch();
    record_model_switch();
    begin_session_with_mode("openai", "gpt-5.4", None, false);
    {
        let guard = SESSION_STATE.lock().unwrap();
        let state = guard.as_ref().expect("session telemetry state");
        assert_eq!(state.error_auth_failed, 0);
        assert_eq!(state.provider_switches, 0);
        assert_eq!(state.model_switches, 0);
    }
    if let Ok(mut session) = SESSION_STATE.lock() {
        *session = None;
    }
}

#[test]
fn test_session_reason_labels() {
    assert_eq!(SessionEndReason::NormalExit.as_str(), "normal_exit");
    assert_eq!(SessionEndReason::Disconnect.as_str(), "disconnect");
}

#[test]
fn test_session_start_event_serialization() {
    let event = SessionStartEvent {
        event_id: "event-1".to_string(),
        id: "test-uuid".to_string(),
        session_id: "session-1".to_string(),
        event: "session_start",
        version: "0.6.1".to_string(),
        os: "linux",
        arch: "x86_64",
        provider_start: "claude".to_string(),
        model_start: "claude-sonnet-4".to_string(),
        resumed_session: true,
        session_start_hour_utc: 13,
        session_start_weekday_utc: 2,
        previous_session_gap_secs: Some(3600),
        sessions_started_24h: 3,
        sessions_started_7d: 8,
        active_sessions_at_start: 2,
        other_active_sessions_at_start: 1,
        schema_version: TELEMETRY_SCHEMA_VERSION,
        build_channel: "release".to_string(),
        is_git_checkout: false,
        is_ci: false,
        ran_from_cargo: false,
    };
    let json = serde_json::to_value(&event).unwrap();
    assert_eq!(json["event"], "session_start");
    assert_eq!(json["resumed_session"], true);
    assert_eq!(json["session_id"], "session-1");
    assert_eq!(json["sessions_started_24h"], 3);
}

#[test]
fn test_discovery_event_serialization_excludes_free_text() {
    let event = DiscoveryEvent {
        event_id: "event-discovery-1".to_string(),
        id: "test-uuid".to_string(),
        session_id: Some("session-1".to_string()),
        event: "discovery",
        version: "0.41.0".to_string(),
        os: "linux",
        arch: "x86_64",
        request_id: "request-1".to_string(),
        phase: "select".to_string(),
        category: Some("payments".to_string()),
        selected_tool: Some("agentcard".to_string()),
        outcome: "success".to_string(),
        failure_reason: None,
        http_status: Some(200),
        latency_ms: 123,
        response_bytes: Some(456),
        result_count: Some(1),
        query_present: true,
        reason_present: true,
        benchmark_run: true,
        custom_endpoint: false,
        schema_version: TELEMETRY_SCHEMA_VERSION,
        build_channel: "release".to_string(),
        is_git_checkout: false,
        is_ci: false,
        ran_from_cargo: false,
    };
    let json = serde_json::to_value(&event).unwrap();
    assert_eq!(json["event"], "discovery");
    assert_eq!(json["request_id"], "request-1");
    assert_eq!(json["phase"], "select");
    assert_eq!(json["selected_tool"], "agentcard");
    assert_eq!(json["latency_ms"], 123);
    assert_eq!(json["benchmark_run"], true);
    assert!(json.get("query").is_none());
    assert!(json.get("reason").is_none());
}

#[test]
fn test_session_end_event_serialization() {
    let event = SessionLifecycleEvent {
        event_id: "event-2".to_string(),
        id: "test-uuid".to_string(),
        session_id: "session-2".to_string(),
        event: "session_end",
        version: "0.6.1".to_string(),
        os: "linux",
        arch: "x86_64",
        provider_start: "claude".to_string(),
        provider_end: "openrouter".to_string(),
        model_start: "claude-sonnet-4-20250514".to_string(),
        model_end: "anthropic/claude-sonnet-4".to_string(),
        provider_switches: 1,
        model_switches: 2,
        duration_mins: 45,
        duration_secs: 2700,
        turns: 23,
        had_user_prompt: true,
        had_assistant_response: true,
        assistant_responses: 3,
        first_assistant_response_ms: Some(1200),
        first_tool_call_ms: Some(900),
        first_tool_success_ms: Some(1500),
        first_file_edit_ms: Some(2200),
        first_test_pass_ms: Some(4100),
        tool_calls: 4,
        tool_failures: 1,
        executed_tool_calls: 5,
        executed_tool_successes: 4,
        executed_tool_failures: 1,
        tool_latency_total_ms: 3200,
        tool_latency_max_ms: 1400,
        file_write_calls: 2,
        tests_run: 1,
        tests_passed: 1,
        input_tokens: 1234,
        output_tokens: 567,
        cache_read_input_tokens: 890,
        cache_creation_input_tokens: 12,
        total_tokens: 2703,
        feature_memory_used: true,
        feature_swarm_used: false,
        feature_web_used: true,
        feature_email_used: false,
        feature_mcp_used: true,
        feature_side_panel_used: true,
        feature_goal_used: false,
        feature_selfdev_used: false,
        feature_background_used: false,
        feature_subagent_used: true,
        feature_todo_used: false,
        unique_mcp_servers: 2,
        session_success: true,
        abandoned_before_response: false,
        session_stop_reason: "completed_successfully",
        agent_role: "foreground",
        parent_session_id: None,
        agent_active_ms_total: 180_000,
        agent_model_ms_total: 120_000,
        agent_tool_ms_total: 60_000,
        session_idle_ms_total: 30_000,
        agent_blocked_ms_total: 0,
        time_to_first_agent_action_ms: Some(900),
        time_to_first_useful_action_ms: Some(1500),
        spawned_agent_count: 3,
        background_task_count: 1,
        background_task_completed_count: 1,
        subagent_task_count: 1,
        subagent_success_count: 1,
        swarm_task_count: 1,
        swarm_success_count: 0,
        user_cancelled_count: 1,
        transport_https: 2,
        transport_persistent_ws_fresh: 1,
        transport_persistent_ws_reuse: 5,
        transport_cli_subprocess: 0,
        transport_native_http2: 0,
        transport_other: 0,
        tool_cat_read_search: 2,
        tool_cat_write: 2,
        tool_cat_shell: 1,
        tool_cat_web: 1,
        tool_cat_memory: 1,
        tool_cat_subagent: 1,
        tool_cat_swarm: 0,
        tool_cat_email: 0,
        tool_cat_side_panel: 1,
        tool_cat_goal: 0,
        tool_cat_mcp: 1,
        tool_cat_other: 0,
        tool_cat_todo: 0,
        todo_gate_ownership_count: 0,
        todo_gate_feedback_loop_count: 0,
        todo_gate_alignment_count: 0,
        todo_gate_intent_count: 0,
        todo_gate_completion_count: 0,
        todo_gate_spike_count: 0,
        command_login_used: false,
        command_model_used: true,
        command_usage_used: false,
        command_resume_used: false,
        command_memory_used: true,
        command_swarm_used: false,
        command_goal_used: false,
        command_selfdev_used: false,
        command_feedback_used: false,
        command_other_used: false,
        workflow_chat_only: false,
        workflow_coding_used: true,
        workflow_research_used: true,
        workflow_tests_used: true,
        workflow_background_used: false,
        workflow_subagent_used: true,
        workflow_swarm_used: false,
        project_repo_present: true,
        project_lang_rust: true,
        project_lang_js_ts: false,
        project_lang_python: false,
        project_lang_go: false,
        project_lang_markdown: true,
        project_lang_mixed: true,
        days_since_install: Some(12),
        active_days_7d: 4,
        active_days_30d: 9,
        session_start_hour_utc: 13,
        session_start_weekday_utc: 2,
        session_end_hour_utc: 14,
        session_end_weekday_utc: 2,
        previous_session_gap_secs: Some(1800),
        sessions_started_24h: 5,
        sessions_started_7d: 12,
        active_sessions_at_start: 2,
        other_active_sessions_at_start: 1,
        max_concurrent_sessions: 3,
        multi_sessioned: true,
        resumed_session: false,
        end_reason: "normal_exit",
        schema_version: TELEMETRY_SCHEMA_VERSION,
        build_channel: "release".to_string(),
        is_git_checkout: false,
        is_ci: false,
        ran_from_cargo: false,
        errors: ErrorCounts {
            provider_timeout: 2,
            auth_failed: 0,
            tool_error: 1,
            mcp_error: 0,
            rate_limited: 0,
        },
    };
    let json = serde_json::to_value(&event).unwrap();
    assert_eq!(json["event"], "session_end");
    assert_eq!(json["assistant_responses"], 3);
    assert_eq!(json["duration_secs"], 2700);
    assert_eq!(json["executed_tool_calls"], 5);
    assert_eq!(json["transport_https"], 2);
    assert_eq!(json["tool_cat_write"], 2);
    assert_eq!(json["workflow_coding_used"], true);
    assert_eq!(json["active_days_30d"], 9);
    assert_eq!(json["transport_persistent_ws_reuse"], 5);
    assert_eq!(json["multi_sessioned"], true);
    assert_eq!(json["end_reason"], "normal_exit");
    assert_eq!(json["input_tokens"], 1234);
    assert_eq!(json["output_tokens"], 567);
    assert_eq!(json["cache_read_input_tokens"], 890);
    assert_eq!(json["cache_creation_input_tokens"], 12);
    assert_eq!(json["total_tokens"], 2703);
    assert_eq!(json["errors"]["provider_timeout"], 2);
    assert_eq!(json["session_stop_reason"], "completed_successfully");
    assert_eq!(json["agent_active_ms_total"], 180_000);
    assert_eq!(json["time_to_first_useful_action_ms"], 1500);
    assert_eq!(json["subagent_task_count"], 1);
    assert_eq!(json["user_cancelled_count"], 1);
}

#[test]
fn test_record_token_usage_aggregates_session_and_turn() {
    let _guard = lock_telemetry_test_state();
    if let Ok(mut session) = SESSION_STATE.lock() {
        *session = None;
    }
    begin_session_with_mode("openai", "gpt-5.4", None, false);
    record_turn();
    record_token_usage(100, 25, Some(200), Some(10));
    record_token_usage(50, 5, None, Some(2));

    {
        let guard = SESSION_STATE.lock().unwrap();
        let state = guard.as_ref().expect("session telemetry state");
        assert_eq!(state.input_tokens, 150);
        assert_eq!(state.output_tokens, 30);
        assert_eq!(state.cache_read_input_tokens, 200);
        assert_eq!(state.cache_creation_input_tokens, 12);
        assert_eq!(state.total_tokens, 392);
        let turn = state.current_turn.as_ref().expect("current turn");
        assert_eq!(turn.input_tokens, 150);
        assert_eq!(turn.output_tokens, 30);
        assert_eq!(turn.cache_read_input_tokens, 200);
        assert_eq!(turn.cache_creation_input_tokens, 12);
        assert_eq!(turn.total_tokens, 392);
    }
    if let Ok(mut session) = SESSION_STATE.lock() {
        *session = None;
    }
}

#[test]
fn test_record_todo_tool_and_gates_aggregate_session_and_turn() {
    let _guard = lock_telemetry_test_state();
    if let Ok(mut session) = SESSION_STATE.lock() {
        *session = None;
    }
    begin_session_with_mode("openai", "gpt-5.4", None, false);
    record_turn();
    record_tool_execution("todo", &serde_json::json!({}), true, 5);
    record_tool_execution("todo", &serde_json::json!({}), true, 5);
    record_todo_gate(TodoGateKind::Ownership);
    record_todo_gate(TodoGateKind::ClosedFeedbackLoop);
    record_todo_gate(TodoGateKind::Alignment);
    record_todo_gate(TodoGateKind::IntentUnderstanding);
    record_todo_gate(TodoGateKind::Completion);
    record_todo_gate(TodoGateKind::ConfidenceSpike);
    record_todo_gate(TodoGateKind::ClosedFeedbackLoop);
    record_todo_gate(TodoGateKind::FeedbackLoopRelevance);
    record_todo_gate(TodoGateKind::FeedbackLoopCoverage);

    {
        let guard = SESSION_STATE.lock().unwrap();
        let state = guard.as_ref().expect("session telemetry state");
        assert_eq!(state.tool_cat_todo, 2);
        assert!(state.feature_todo_used);
        assert_eq!(state.tool_cat_other, 0);
        assert_eq!(state.todo_gate_ownership_count, 1);
        assert_eq!(state.todo_gate_feedback_loop_count, 4);
        assert_eq!(state.todo_gate_alignment_count, 1);
        assert_eq!(state.todo_gate_intent_count, 1);
        assert_eq!(state.todo_gate_completion_count, 1);
        assert_eq!(state.todo_gate_spike_count, 1);
        let turn = state.current_turn.as_ref().expect("current turn");
        assert_eq!(turn.tool_cat_todo, 2);
        assert!(turn.feature_todo_used);
        assert_eq!(turn.todo_gate_ownership_count, 1);
        assert_eq!(turn.todo_gate_feedback_loop_count, 4);
        assert_eq!(turn.todo_gate_alignment_count, 1);
        assert_eq!(turn.todo_gate_intent_count, 1);
        assert_eq!(turn.todo_gate_completion_count, 1);
        assert_eq!(turn.todo_gate_spike_count, 1);
    }
    if let Ok(mut session) = SESSION_STATE.lock() {
        *session = None;
    }
}

#[test]
fn test_record_connection_type_buckets_transport() {
    let _guard = lock_telemetry_test_state();
    if let Ok(mut session) = SESSION_STATE.lock() {
        *session = None;
    }
    begin_session_with_mode("openai", "gpt-5.4", None, false);
    record_connection_type("websocket/persistent-fresh");
    record_connection_type("websocket/persistent-reuse");
    record_connection_type("https/sse");
    record_connection_type("native http2");
    record_connection_type("cli subprocess");
    record_connection_type("weird-transport");

    {
        let guard = SESSION_STATE.lock().unwrap();
        let state = guard.as_ref().expect("session telemetry state");
        assert_eq!(state.transport_persistent_ws_fresh, 1);
        assert_eq!(state.transport_persistent_ws_reuse, 1);
        assert_eq!(state.transport_https, 1);
        assert_eq!(state.transport_native_http2, 1);
        assert_eq!(state.transport_cli_subprocess, 1);
        assert_eq!(state.transport_other, 1);
    }
    if let Ok(mut session) = SESSION_STATE.lock() {
        *session = None;
    }
}

#[test]
fn test_sanitize_telemetry_label_strips_ansi_and_controls() {
    assert_eq!(
        sanitize_telemetry_label("\u{1b}[1mclaude-opus-4-6\u{1b}[0m\n"),
        "claude-opus-4-6"
    );
}

#[test]
fn test_onboarding_step_event_serialization_includes_failure_reason() {
    let event = OnboardingStepEvent {
        event_id: "event-3".to_string(),
        id: "test-uuid".to_string(),
        session_id: None,
        event: "onboarding_step",
        version: "0.6.1".to_string(),
        os: "linux",
        arch: "x86_64",
        step: "auth_failed",
        auth_provider: Some("openai".to_string()),
        auth_method: Some("oauth".to_string()),
        auth_failure_reason: Some("callback_timeout".to_string()),
        milestone_elapsed_ms: Some(1234),
        schema_version: TELEMETRY_SCHEMA_VERSION,
        build_channel: "release".to_string(),
        is_git_checkout: false,
        is_ci: false,
        ran_from_cargo: false,
    };
    let json = serde_json::to_value(&event).unwrap();
    assert_eq!(json["step"], "auth_failed");
    assert_eq!(json["auth_failure_reason"], "callback_timeout");
}

#[test]
fn test_onboarding_step_milestone_key_includes_provider_and_method() {
    assert_eq!(
        onboarding_step_milestone_key("auth_success", Some("jcode"), Some("API key")),
        "auth_success_jcode_api_key"
    );
    assert_eq!(
        onboarding_step_milestone_key("login_picker_opened", None, None),
        "login_picker_opened"
    );
}

#[test]
fn test_install_marker_tracks_current_telemetry_id() {
    let _guard = lock_test_env();
    let prev_home = std::env::var_os("JCODE_HOME");
    let temp = tempfile::TempDir::new().expect("create temp dir");
    jcode_core::env::set_var("JCODE_HOME", temp.path());

    assert!(!install_recorded_for_id("id-a"));
    mark_install_recorded("id-a");
    assert!(install_recorded_for_id("id-a"));
    assert!(!install_recorded_for_id("id-b"));

    if let Some(prev_home) = prev_home {
        jcode_core::env::set_var("JCODE_HOME", prev_home);
    } else {
        jcode_core::env::remove_var("JCODE_HOME");
    }
}

#[test]
fn test_install_conversion_id_is_validated_and_consumed() {
    let _guard = lock_test_env();
    let prev_home = std::env::var_os("JCODE_HOME");
    let temp = tempfile::TempDir::new().expect("create temp dir");
    jcode_core::env::set_var("JCODE_HOME", temp.path());

    let path = install_conversion_id_path().expect("conversion path");
    write_private_file(&path, "11111111-2222-4333-8444-555555555555\n");
    assert_eq!(
        read_install_conversion_id().as_deref(),
        Some("11111111-2222-4333-8444-555555555555")
    );
    clear_install_conversion_id();
    assert_eq!(read_install_conversion_id(), None);

    write_private_file(&path, "not-a-conversion-id");
    assert_eq!(read_install_conversion_id(), None);
    assert!(!path.exists());

    assert!(install_conversion_id_is_fresh(std::time::SystemTime::now()));
    assert!(!install_conversion_id_is_fresh(
        std::time::SystemTime::now() - std::time::Duration::from_secs(91 * 24 * 60 * 60)
    ));

    if let Some(prev_home) = prev_home {
        jcode_core::env::set_var("JCODE_HOME", prev_home);
    } else {
        jcode_core::env::remove_var("JCODE_HOME");
    }
}

#[test]
fn test_attributed_install_bypasses_existing_install_marker() {
    let _guard = lock_test_env();
    let prev_home = std::env::var_os("JCODE_HOME");
    let temp = tempfile::TempDir::new().expect("create temp dir");
    jcode_core::env::set_var("JCODE_HOME", temp.path());

    let id = get_or_create_id().expect("telemetry id");
    mark_install_recorded(&id);
    assert!(install_recorded_for_id(&id));
    assert!(read_install_conversion_id().is_none());
    assert!(!should_record_install_for_id(&id, None));

    let path = install_conversion_id_path().expect("conversion path");
    write_private_file(&path, "11111111-2222-4333-8444-555555555555\n");
    assert!(install_recorded_for_id(&id));
    let conversion_id = read_install_conversion_id();
    assert!(conversion_id.is_some());
    assert!(should_record_install_for_id(&id, conversion_id.as_deref()));

    if let Some(prev_home) = prev_home {
        jcode_core::env::set_var("JCODE_HOME", prev_home);
    } else {
        jcode_core::env::remove_var("JCODE_HOME");
    }
}

#[test]
fn test_install_conversion_id_is_removed_when_telemetry_is_disabled() {
    let _guard = lock_test_env();
    let prev_home = std::env::var_os("JCODE_HOME");
    let temp = tempfile::TempDir::new().expect("create temp dir");
    jcode_core::env::set_var("JCODE_HOME", temp.path());
    let path = install_conversion_id_path().expect("conversion path");
    write_private_file(&path, "11111111-2222-4333-8444-555555555555\n");

    jcode_core::env::set_var("JCODE_NO_TELEMETRY", "1");
    record_install_if_first_run();
    jcode_core::env::remove_var("JCODE_NO_TELEMETRY");
    assert!(!path.exists());

    if let Some(prev_home) = prev_home {
        jcode_core::env::set_var("JCODE_HOME", prev_home);
    } else {
        jcode_core::env::remove_var("JCODE_HOME");
    }
}

fn current_todo_payload(reason: SessionEndReason) -> serde_json::Value {
    let state = SESSION_STATE
        .lock()
        .expect("session state lock")
        .as_ref()
        .expect("active telemetry session")
        .clone();
    serde_json::to_value(lifecycle::todo_session_event(
        &state,
        reason,
        TELEMETRY_SCHEMA_VERSION,
        "test".to_string(),
        false,
        false,
        false,
    ))
    .expect("todo telemetry serializes")
}

#[test]
fn todo_session_aggregates_transitions_abandonment_and_high_water_mark() {
    let _guard = lock_telemetry_test_state();
    *SESSION_STATE.lock().unwrap() = None;
    begin_session("test", "test");

    record_todo_update(TodoTelemetryUpdate {
        todos_created: 3,
        current_incomplete: 2,
        list_size: 3,
        groups_completed: 1,
        groups_total: 2,
        confidence: TelemetryScoreSummary::from_scores([70, 80, 90]),
        ..Default::default()
    });
    record_todo_update(TodoTelemetryUpdate {
        todos_created: 1,
        todos_completed: 2,
        todos_abandoned: 1,
        current_incomplete: 1,
        list_size: 2,
        groups_completed: 2,
        groups_total: 3,
        confidence: TelemetryScoreSummary::from_scores([75, 95]),
        completion_confidence: TelemetryScoreSummary::from_scores([96, 100]),
        understands_user_intent: TelemetryScoreSummary::from_scores([94]),
        closed_feedback_loop: TelemetryScoreSummary::from_scores([85, 95]),
        feedback_loop_relevance: TelemetryScoreSummary::from_scores([75, 98]),
        feedback_loop_coverage: TelemetryScoreSummary::from_scores([75, 98]),
        end_to_end_ownership: TelemetryScoreSummary::from_scores([96, 100]),
    });
    {
        let mut session = SESSION_STATE.lock().unwrap();
        let state = session.as_mut().unwrap();
        increment_tool_category(state, ToolCategory::Todo);
        increment_tool_category(state, ToolCategory::Todo);
    }

    let payload = current_todo_payload(SessionEndReason::NormalExit);
    assert_eq!(payload["todos_created"], 4);
    assert_eq!(payload["todos_completed"], 2);
    assert_eq!(payload["todos_abandoned"], 2);
    assert_eq!(payload["todo_updates"], 2);
    assert_eq!(payload["groups_completed"], 2);
    assert_eq!(payload["groups_total"], 3);
    assert_eq!(payload["max_todo_list_size"], 3);
    assert_eq!(payload["confidence_min"], 75);
    assert_eq!(payload["confidence_mean"], 85.0);
    assert_eq!(payload["completion_confidence_count"], 2);
    assert_eq!(payload["feedback_loop_relevance_min"], 75);
    assert_eq!(payload["feedback_loop_relevance_count"], 2);
    assert_eq!(payload["feedback_loop_coverage_min"], 75);
    assert_eq!(payload["feedback_loop_coverage_count"], 2);
    *SESSION_STATE.lock().unwrap() = None;
}

#[test]
fn todo_session_with_zero_todos_emits_zero_numeric_state() {
    let _guard = lock_telemetry_test_state();
    *SESSION_STATE.lock().unwrap() = None;
    begin_session("test", "test");
    let payload = current_todo_payload(SessionEndReason::NormalExit);
    for field in [
        "todos_created",
        "todos_completed",
        "todos_abandoned",
        "todo_updates",
        "groups_completed",
        "groups_total",
        "max_todo_list_size",
        "confidence_count",
        "completion_confidence_count",
        "understands_user_intent_count",
        "closed_feedback_loop_count",
        "end_to_end_ownership_count",
    ] {
        assert_eq!(payload[field], 0, "{field}");
    }
    assert!(payload["confidence_min"].is_null());
    assert!(payload["confidence_mean"].is_null());
    *SESSION_STATE.lock().unwrap() = None;
}

#[test]
fn todo_session_payload_has_no_unapproved_string_fields() {
    let _guard = lock_telemetry_test_state();
    *SESSION_STATE.lock().unwrap() = None;
    begin_session(
        "provider text must not appear",
        "model text must not appear",
    );
    record_todo_update(TodoTelemetryUpdate {
        todos_created: 1,
        current_incomplete: 1,
        list_size: 1,
        confidence: TelemetryScoreSummary::from_scores([88]),
        ..Default::default()
    });
    let payload = current_todo_payload(SessionEndReason::Disconnect);
    let allowed_string_fields = [
        "event_id",
        "id",
        "correlation_id",
        "event",
        "version",
        "os",
        "arch",
        "session_end_reason",
        "build_channel",
    ];

    fn reject_unapproved_strings(value: &serde_json::Value, field: Option<&str>, allowed: &[&str]) {
        match value {
            serde_json::Value::String(_) => assert!(
                field.is_some_and(|field| allowed.contains(&field)),
                "unapproved string field in todo telemetry: {field:?}"
            ),
            serde_json::Value::Array(values) => {
                for value in values {
                    reject_unapproved_strings(value, field, allowed);
                }
            }
            serde_json::Value::Object(values) => {
                for (key, value) in values {
                    reject_unapproved_strings(value, Some(key), allowed);
                }
            }
            _ => {}
        }
    }
    reject_unapproved_strings(&payload, None, &allowed_string_fields);
    assert_eq!(payload["id"], payload["correlation_id"]);
    assert_eq!(payload["event"], "todo_session");
    let correlation = payload["correlation_id"].as_str().unwrap();
    assert_eq!(
        uuid::Uuid::parse_str(correlation)
            .unwrap()
            .get_version_num(),
        4
    );
    assert!(!payload.to_string().contains("provider text"));
    assert!(!payload.to_string().contains("model text"));
    *SESSION_STATE.lock().unwrap() = None;
}

#[test]
fn todo_correlation_id_is_fresh_for_each_session() {
    let _guard = lock_telemetry_test_state();
    *SESSION_STATE.lock().unwrap() = None;
    begin_session("test", "test");
    let first = current_session_correlation_id().expect("first correlation id");
    begin_session("test", "test");
    let second = current_session_correlation_id().expect("second correlation id");
    assert_ne!(first, second);
    assert_eq!(uuid::Uuid::parse_str(&first).unwrap().get_version_num(), 4);
    assert_eq!(uuid::Uuid::parse_str(&second).unwrap().get_version_num(), 4);
    *SESSION_STATE.lock().unwrap() = None;
}

#[test]
fn todo_telemetry_opt_out_emits_nothing() {
    let _guard = lock_telemetry_test_state();
    *SESSION_STATE.lock().unwrap() = None;
    TEST_EMITTED_PAYLOADS.lock().unwrap().clear();
    jcode_core::env::set_var("JCODE_NO_TELEMETRY", "1");

    begin_session("private-provider", "private-model");
    record_todo_update(TodoTelemetryUpdate {
        todos_created: 1,
        current_incomplete: 1,
        list_size: 1,
        ..Default::default()
    });
    end_session("private-provider", "private-model");
    let correlation = current_session_correlation_id();
    let emitted = TEST_EMITTED_PAYLOADS.lock().unwrap().clone();

    jcode_core::env::remove_var("JCODE_NO_TELEMETRY");
    assert!(SESSION_STATE.lock().unwrap().is_none());
    assert!(correlation.is_none());
    assert!(emitted.is_empty(), "opt-out emitted payloads: {emitted:?}");
}

/// Regression: `begin_session` used to overwrite a live `SESSION_STATE`
/// without ending it, orphaning the previous `session_start`. That is why
/// only ~25% of release `session_start` events ever had a matching
/// `session_end`. Starting a second session must close the first.
#[test]
fn test_begin_session_closes_superseded_session() {
    let _guard = lock_telemetry_test_state();
    jcode_core::env::set_var("JCODE_TELEMETRY_DISABLED", "1");

    begin_session("prov-a", "model-a");
    let first_id = SESSION_STATE
        .lock()
        .unwrap()
        .as_ref()
        .map(|s| s.session_id.clone());

    begin_session("prov-b", "model-b");
    let second = SESSION_STATE.lock().unwrap();
    let second_state = second.as_ref().expect("second session should be live");

    assert_ne!(
        first_id.as_deref(),
        Some(second_state.session_id.as_str()),
        "second begin_session should install a distinct session"
    );
    assert_eq!(second_state.provider_start, "prov-b");
    assert!(
        !second_state.start_event_sent,
        "a fresh session must not inherit the superseded session's sent flag"
    );
    drop(second);

    jcode_core::env::remove_var("JCODE_TELEMETRY_DISABLED");
}

#[test]
fn test_superseded_reason_has_label() {
    assert_eq!(SessionEndReason::Superseded.as_str(), "superseded");
}
