use super::*;

pub(super) fn emit_lifecycle_event(
    event_name: &'static str,
    provider_end: &str,
    model_end: &str,
    reason: SessionEndReason,
    clear_state: bool,
) {
    if !is_enabled() {
        return;
    }
    let id = match get_or_create_id() {
        Some(id) => id,
        None => return,
    };
    let state = {
        let mut guard = match SESSION_STATE.lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        let now = Instant::now();
        if let Some(active) = guard.as_mut() {
            finalize_current_turn(&id, active, now, reason.as_str(), DeliveryMode::Background);
            observe_session_concurrency(active);
        }
        let state = match guard.as_ref() {
            Some(s) => SessionTelemetry {
                session_id: s.session_id.clone(),
                started_at: s.started_at,
                started_at_utc: s.started_at_utc,
                provider_start: s.provider_start.clone(),
                model_start: s.model_start.clone(),
                parent_session_id: s.parent_session_id.clone(),
                turns: s.turns,
                had_user_prompt: s.had_user_prompt,
                had_assistant_response: s.had_assistant_response,
                assistant_responses: s.assistant_responses,
                first_assistant_response_ms: s.first_assistant_response_ms,
                first_tool_call_ms: s.first_tool_call_ms,
                first_tool_success_ms: s.first_tool_success_ms,
                first_file_edit_ms: s.first_file_edit_ms,
                first_test_pass_ms: s.first_test_pass_ms,
                tool_calls: s.tool_calls,
                tool_failures: s.tool_failures,
                executed_tool_calls: s.executed_tool_calls,
                executed_tool_successes: s.executed_tool_successes,
                executed_tool_failures: s.executed_tool_failures,
                tool_latency_total_ms: s.tool_latency_total_ms,
                tool_latency_max_ms: s.tool_latency_max_ms,
                file_write_calls: s.file_write_calls,
                tests_run: s.tests_run,
                tests_passed: s.tests_passed,
                input_tokens: s.input_tokens,
                output_tokens: s.output_tokens,
                cache_read_input_tokens: s.cache_read_input_tokens,
                cache_creation_input_tokens: s.cache_creation_input_tokens,
                total_tokens: s.total_tokens,
                feature_memory_used: s.feature_memory_used,
                feature_swarm_used: s.feature_swarm_used,
                feature_web_used: s.feature_web_used,
                feature_email_used: s.feature_email_used,
                feature_mcp_used: s.feature_mcp_used,
                feature_side_panel_used: s.feature_side_panel_used,
                feature_goal_used: s.feature_goal_used,
                feature_selfdev_used: s.feature_selfdev_used,
                feature_background_used: s.feature_background_used,
                feature_subagent_used: s.feature_subagent_used,
                unique_mcp_servers: s.unique_mcp_servers.clone(),
                transport_https: s.transport_https,
                transport_persistent_ws_fresh: s.transport_persistent_ws_fresh,
                transport_persistent_ws_reuse: s.transport_persistent_ws_reuse,
                transport_cli_subprocess: s.transport_cli_subprocess,
                transport_native_http2: s.transport_native_http2,
                transport_other: s.transport_other,
                agent_active_ms_total: s.agent_active_ms_total,
                agent_model_ms_total: s.agent_model_ms_total,
                agent_tool_ms_total: s.agent_tool_ms_total,
                session_idle_ms_total: s.session_idle_ms_total,
                agent_blocked_ms_total: s.agent_blocked_ms_total,
                time_to_first_agent_action_ms: s.time_to_first_agent_action_ms,
                time_to_first_useful_action_ms: s.time_to_first_useful_action_ms,
                spawned_agent_count: s.spawned_agent_count,
                background_task_count: s.background_task_count,
                background_task_completed_count: s.background_task_completed_count,
                subagent_task_count: s.subagent_task_count,
                subagent_success_count: s.subagent_success_count,
                swarm_task_count: s.swarm_task_count,
                swarm_success_count: s.swarm_success_count,
                user_cancelled_count: s.user_cancelled_count,
                tool_cat_read_search: s.tool_cat_read_search,
                tool_cat_write: s.tool_cat_write,
                tool_cat_shell: s.tool_cat_shell,
                tool_cat_web: s.tool_cat_web,
                tool_cat_memory: s.tool_cat_memory,
                tool_cat_subagent: s.tool_cat_subagent,
                tool_cat_swarm: s.tool_cat_swarm,
                tool_cat_email: s.tool_cat_email,
                tool_cat_side_panel: s.tool_cat_side_panel,
                tool_cat_goal: s.tool_cat_goal,
                tool_cat_mcp: s.tool_cat_mcp,
                tool_cat_other: s.tool_cat_other,
                command_login_used: s.command_login_used,
                command_model_used: s.command_model_used,
                command_usage_used: s.command_usage_used,
                command_resume_used: s.command_resume_used,
                command_memory_used: s.command_memory_used,
                command_swarm_used: s.command_swarm_used,
                command_goal_used: s.command_goal_used,
                command_selfdev_used: s.command_selfdev_used,
                command_feedback_used: s.command_feedback_used,
                command_other_used: s.command_other_used,
                previous_session_gap_secs: s.previous_session_gap_secs,
                sessions_started_24h: s.sessions_started_24h,
                sessions_started_7d: s.sessions_started_7d,
                active_sessions_at_start: s.active_sessions_at_start,
                other_active_sessions_at_start: s.other_active_sessions_at_start,
                max_concurrent_sessions: s.max_concurrent_sessions,
                current_turn: None,
                resumed_session: s.resumed_session,
                start_event_sent: s.start_event_sent,
            },
            None => return,
        };
        if clear_state {
            *guard = None;
        }
        state
    };
    let errors = current_error_counts();
    if !session_has_meaningful_activity(&state, &errors) {
        reset_counters();
        return;
    }
    if !state.start_event_sent {
        let _ = emit_session_start_for_state(
            id.clone(),
            &state,
            DeliveryMode::Blocking(BLOCKING_LIFECYCLE_TIMEOUT),
        );
    }
    let duration = state.started_at.elapsed();
    let session_success = state.had_assistant_response
        || state.executed_tool_successes > 0
        || state.tests_passed > 0
        || state.file_write_calls > 0;
    let abandoned_before_response = state.had_user_prompt
        && !state.had_assistant_response
        && state.executed_tool_successes == 0;
    let workflow_coding_used = state.file_write_calls > 0 || state.tool_cat_write > 0;
    let workflow_research_used = state.feature_web_used || state.tool_cat_web > 0;
    let workflow_tests_used = state.tests_run > 0 || state.tests_passed > 0;
    let workflow_background_used = state.feature_background_used;
    let workflow_subagent_used = state.feature_subagent_used || state.tool_cat_subagent > 0;
    let workflow_swarm_used = state.feature_swarm_used || state.tool_cat_swarm > 0;
    let workflow_chat_only = state.had_user_prompt
        && !workflow_coding_used
        && !workflow_research_used
        && !workflow_tests_used
        && !workflow_background_used
        && !workflow_subagent_used
        && !workflow_swarm_used;
    let project_profile = detect_project_profile();
    let (active_days_7d, active_days_30d) = update_active_days(&id);
    let days_since_install = days_since_install(&id);
    let ended_at_utc = Utc::now();
    let (schema_version, build_channel, git_checkout, ci, from_cargo) = telemetry_envelope();
    let session_stop_reason = infer_session_stop_reason(
        event_name,
        reason,
        &state,
        &errors,
        duration.as_secs(),
        session_success,
        abandoned_before_response,
        workflow_coding_used,
    );
    let agent_role = infer_agent_role(&state);
    let time_to_first_agent_action_ms = time_to_first_agent_action_ms(&state);
    let time_to_first_useful_action_ms = time_to_first_useful_action_ms(&state);
    let event = SessionLifecycleEvent {
        event_id: new_event_id(),
        id,
        session_id: state.session_id.clone(),
        event: event_name,
        version: version(),
        os: std::env::consts::OS,
        arch: std::env::consts::ARCH,
        provider_start: state.provider_start,
        provider_end: sanitize_telemetry_label(provider_end),
        model_start: state.model_start,
        model_end: sanitize_telemetry_label(model_end),
        provider_switches: PROVIDER_SWITCHES.load(Ordering::Relaxed),
        model_switches: MODEL_SWITCHES.load(Ordering::Relaxed),
        duration_mins: duration.as_secs() / 60,
        duration_secs: duration.as_secs(),
        turns: state.turns,
        had_user_prompt: state.had_user_prompt,
        had_assistant_response: state.had_assistant_response,
        assistant_responses: state.assistant_responses,
        first_assistant_response_ms: state.first_assistant_response_ms,
        first_tool_call_ms: state.first_tool_call_ms,
        first_tool_success_ms: state.first_tool_success_ms,
        first_file_edit_ms: state.first_file_edit_ms,
        first_test_pass_ms: state.first_test_pass_ms,
        tool_calls: state.tool_calls,
        tool_failures: state.tool_failures,
        executed_tool_calls: state.executed_tool_calls,
        executed_tool_successes: state.executed_tool_successes,
        executed_tool_failures: state.executed_tool_failures,
        tool_latency_total_ms: state.tool_latency_total_ms,
        tool_latency_max_ms: state.tool_latency_max_ms,
        file_write_calls: state.file_write_calls,
        tests_run: state.tests_run,
        tests_passed: state.tests_passed,
        input_tokens: state.input_tokens,
        output_tokens: state.output_tokens,
        cache_read_input_tokens: state.cache_read_input_tokens,
        cache_creation_input_tokens: state.cache_creation_input_tokens,
        total_tokens: state.total_tokens,
        feature_memory_used: state.feature_memory_used,
        feature_swarm_used: state.feature_swarm_used,
        feature_web_used: state.feature_web_used,
        feature_email_used: state.feature_email_used,
        feature_mcp_used: state.feature_mcp_used,
        feature_side_panel_used: state.feature_side_panel_used,
        feature_goal_used: state.feature_goal_used,
        feature_selfdev_used: state.feature_selfdev_used,
        feature_background_used: state.feature_background_used,
        feature_subagent_used: state.feature_subagent_used,
        unique_mcp_servers: state.unique_mcp_servers.len() as u32,
        session_success,
        abandoned_before_response,
        session_stop_reason,
        agent_role,
        parent_session_id: state.parent_session_id.clone(),
        agent_active_ms_total: state.agent_active_ms_total,
        agent_model_ms_total: state.agent_model_ms_total,
        agent_tool_ms_total: state.agent_tool_ms_total,
        session_idle_ms_total: state.session_idle_ms_total,
        agent_blocked_ms_total: state.agent_blocked_ms_total,
        time_to_first_agent_action_ms,
        time_to_first_useful_action_ms,
        spawned_agent_count: state.spawned_agent_count,
        background_task_count: state.background_task_count,
        background_task_completed_count: state.background_task_completed_count,
        subagent_task_count: state.subagent_task_count,
        subagent_success_count: state.subagent_success_count,
        swarm_task_count: state.swarm_task_count,
        swarm_success_count: state.swarm_success_count,
        user_cancelled_count: state.user_cancelled_count,
        transport_https: state.transport_https,
        transport_persistent_ws_fresh: state.transport_persistent_ws_fresh,
        transport_persistent_ws_reuse: state.transport_persistent_ws_reuse,
        transport_cli_subprocess: state.transport_cli_subprocess,
        transport_native_http2: state.transport_native_http2,
        transport_other: state.transport_other,
        tool_cat_read_search: state.tool_cat_read_search,
        tool_cat_write: state.tool_cat_write,
        tool_cat_shell: state.tool_cat_shell,
        tool_cat_web: state.tool_cat_web,
        tool_cat_memory: state.tool_cat_memory,
        tool_cat_subagent: state.tool_cat_subagent,
        tool_cat_swarm: state.tool_cat_swarm,
        tool_cat_email: state.tool_cat_email,
        tool_cat_side_panel: state.tool_cat_side_panel,
        tool_cat_goal: state.tool_cat_goal,
        tool_cat_mcp: state.tool_cat_mcp,
        tool_cat_other: state.tool_cat_other,
        command_login_used: state.command_login_used,
        command_model_used: state.command_model_used,
        command_usage_used: state.command_usage_used,
        command_resume_used: state.command_resume_used,
        command_memory_used: state.command_memory_used,
        command_swarm_used: state.command_swarm_used,
        command_goal_used: state.command_goal_used,
        command_selfdev_used: state.command_selfdev_used,
        command_feedback_used: state.command_feedback_used,
        command_other_used: state.command_other_used,
        workflow_chat_only,
        workflow_coding_used,
        workflow_research_used,
        workflow_tests_used,
        workflow_background_used,
        workflow_subagent_used,
        workflow_swarm_used,
        project_repo_present: project_profile.repo_present,
        project_lang_rust: project_profile.lang_rust,
        project_lang_js_ts: project_profile.lang_js_ts,
        project_lang_python: project_profile.lang_python,
        project_lang_go: project_profile.lang_go,
        project_lang_markdown: project_profile.lang_markdown,
        project_lang_mixed: project_profile.mixed(),
        days_since_install,
        active_days_7d,
        active_days_30d,
        session_start_hour_utc: utc_hour(state.started_at_utc),
        session_start_weekday_utc: utc_weekday(state.started_at_utc),
        session_end_hour_utc: utc_hour(ended_at_utc),
        session_end_weekday_utc: utc_weekday(ended_at_utc),
        previous_session_gap_secs: state.previous_session_gap_secs,
        sessions_started_24h: state.sessions_started_24h,
        sessions_started_7d: state.sessions_started_7d,
        active_sessions_at_start: state.active_sessions_at_start,
        other_active_sessions_at_start: state.other_active_sessions_at_start,
        max_concurrent_sessions: state.max_concurrent_sessions,
        multi_sessioned: state.max_concurrent_sessions > 1
            || state.other_active_sessions_at_start > 0,
        resumed_session: state.resumed_session,
        end_reason: reason.as_str(),
        schema_version,
        build_channel,
        is_git_checkout: git_checkout,
        is_ci: ci,
        ran_from_cargo: from_cargo,
        errors,
    };
    if let Ok(payload) = serde_json::to_value(&event) {
        let _ = send_payload(payload, DeliveryMode::Blocking(BLOCKING_LIFECYCLE_TIMEOUT));
    }
    unregister_active_session(&state.session_id);
    if session_success {
        emit_onboarding_step_once("first_session_success", None, None);
    }
    reset_counters();
}
