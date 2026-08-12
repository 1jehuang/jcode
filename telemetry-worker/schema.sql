-- Schema for jcode telemetry D1 database

CREATE TABLE IF NOT EXISTS events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    telemetry_id TEXT NOT NULL,
    event TEXT NOT NULL,
    version TEXT NOT NULL,
    os TEXT NOT NULL,
    arch TEXT NOT NULL,
    provider_start TEXT,
    provider_end TEXT,
    model_start TEXT,
    model_end TEXT,
    provider_switches INTEGER DEFAULT 0,
    model_switches INTEGER DEFAULT 0,
    duration_mins INTEGER,
    duration_secs INTEGER,
    turns INTEGER,
    had_user_prompt INTEGER DEFAULT 0,
    had_assistant_response INTEGER DEFAULT 0,
    assistant_responses INTEGER DEFAULT 0,
    first_assistant_response_ms INTEGER,
    first_tool_call_ms INTEGER,
    first_tool_success_ms INTEGER,
    tool_calls INTEGER DEFAULT 0,
    tool_failures INTEGER DEFAULT 0,
    executed_tool_calls INTEGER DEFAULT 0,
    executed_tool_successes INTEGER DEFAULT 0,
    executed_tool_failures INTEGER DEFAULT 0,
    tool_latency_total_ms INTEGER DEFAULT 0,
    tool_latency_max_ms INTEGER DEFAULT 0,
    file_write_calls INTEGER DEFAULT 0,
    tests_run INTEGER DEFAULT 0,
    tests_passed INTEGER DEFAULT 0,
    input_tokens INTEGER DEFAULT 0,
    output_tokens INTEGER DEFAULT 0,
    cache_read_input_tokens INTEGER DEFAULT 0,
    cache_creation_input_tokens INTEGER DEFAULT 0,
    total_tokens INTEGER DEFAULT 0,
    feature_memory_used INTEGER DEFAULT 0,
    feature_swarm_used INTEGER DEFAULT 0,
    feature_web_used INTEGER DEFAULT 0,
    feature_email_used INTEGER DEFAULT 0,
    feature_mcp_used INTEGER DEFAULT 0,
    feature_side_panel_used INTEGER DEFAULT 0,
    feature_goal_used INTEGER DEFAULT 0,
    feature_selfdev_used INTEGER DEFAULT 0,
    feature_background_used INTEGER DEFAULT 0,
    feature_subagent_used INTEGER DEFAULT 0,
    unique_mcp_servers INTEGER DEFAULT 0,
    session_success INTEGER DEFAULT 0,
    abandoned_before_response INTEGER DEFAULT 0,
    session_stop_reason TEXT,
    agent_role TEXT,
    parent_session_id TEXT,
    agent_active_ms_total INTEGER DEFAULT 0,
    agent_model_ms_total INTEGER DEFAULT 0,
    agent_tool_ms_total INTEGER DEFAULT 0,
    session_idle_ms_total INTEGER DEFAULT 0,
    agent_blocked_ms_total INTEGER DEFAULT 0,
    time_to_first_agent_action_ms INTEGER,
    time_to_first_useful_action_ms INTEGER,
    spawned_agent_count INTEGER DEFAULT 0,
    background_task_count INTEGER DEFAULT 0,
    background_task_completed_count INTEGER DEFAULT 0,
    subagent_task_count INTEGER DEFAULT 0,
    subagent_success_count INTEGER DEFAULT 0,
    swarm_task_count INTEGER DEFAULT 0,
    swarm_success_count INTEGER DEFAULT 0,
    user_cancelled_count INTEGER DEFAULT 0,
    transport_https INTEGER DEFAULT 0,
    transport_persistent_ws_fresh INTEGER DEFAULT 0,
    transport_persistent_ws_reuse INTEGER DEFAULT 0,
    transport_cli_subprocess INTEGER DEFAULT 0,
    transport_native_http2 INTEGER DEFAULT 0,
    transport_other INTEGER DEFAULT 0,
    resumed_session INTEGER DEFAULT 0,
    end_reason TEXT,
    auth_provider TEXT,
    auth_method TEXT,
    -- Failure reason label for onboarding_step step='auth_failed' events
    -- (classify_auth_failure_message labels, e.g. callback_timeout,
    -- validation_failed, oauth_rate_limited). Added in migration 0015.
    auth_failure_reason TEXT,
    from_version TEXT,
    event_id TEXT,
    session_id TEXT,
    schema_version INTEGER DEFAULT 1,
    build_channel TEXT,
    is_git_checkout INTEGER DEFAULT 0,
    is_ci INTEGER DEFAULT 0,
    ran_from_cargo INTEGER DEFAULT 0,
    step TEXT,
    milestone_elapsed_ms INTEGER,
    feedback_rating TEXT,
    feedback_reason TEXT,
    feedback_text TEXT,
    -- NOTE: schema-v5 per-turn fields (turn_index, turn timings, turn_success,
    -- turn_abandoned, turn_end_reason) and session cadence fields (hour/weekday,
    -- previous_session_gap_secs, sessions_started_24h/7d, concurrency) live in
    -- turn_details / session_details, NOT here. D1 caps tables at 100 columns
    -- and events sits at 96 in production, so it has no headroom. See
    -- migrations/0013_detail_table_turn_session_fields.sql.
    error_provider_timeout INTEGER DEFAULT 0,
    error_auth_failed INTEGER DEFAULT 0,
    error_tool_error INTEGER DEFAULT 0,
    error_mcp_error INTEGER DEFAULT 0,
    error_rate_limited INTEGER DEFAULT 0,
    -- Token subscription plan fields (migration 0016). These two are the only
    -- subscription columns on events because the table is near D1's
    -- 100-column cap (96 in production before 0016); web-only fields live in
    -- web_details below.
    account_id TEXT,
    tier TEXT,
    created_at TEXT DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_events_telemetry_id ON events(telemetry_id);
CREATE INDEX IF NOT EXISTS idx_events_event ON events(event);
CREATE INDEX IF NOT EXISTS idx_events_created_at ON events(created_at);
CREATE INDEX IF NOT EXISTS idx_events_event_created_telemetry ON events(event, created_at, telemetry_id);
CREATE INDEX IF NOT EXISTS idx_events_event_telemetry_created ON events(event, telemetry_id, created_at);
CREATE UNIQUE INDEX IF NOT EXISTS idx_events_event_id ON events(event_id);
CREATE INDEX IF NOT EXISTS idx_events_session_id ON events(session_id);
CREATE INDEX IF NOT EXISTS idx_events_step ON events(step);
CREATE INDEX IF NOT EXISTS idx_events_feedback_rating ON events(feedback_rating);
CREATE INDEX IF NOT EXISTS idx_events_account_id ON events(account_id);
CREATE INDEX IF NOT EXISTS idx_events_event_tier_created ON events(event, tier, created_at);

-- Metadata for separately consented transcript uploads. Transcript bodies live
-- in the private R2 TRANSCRIPTS bucket, never in the ordinary events table.
CREATE TABLE IF NOT EXISTS transcript_uploads (
    upload_id TEXT PRIMARY KEY,
    telemetry_id TEXT NOT NULL,
    object_key TEXT NOT NULL UNIQUE,
    consent_version INTEGER NOT NULL,
    schema_version INTEGER NOT NULL,
    version TEXT NOT NULL,
    provider TEXT,
    model TEXT,
    end_reason TEXT NOT NULL,
    message_count INTEGER NOT NULL,
    byte_count INTEGER NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_transcript_uploads_telemetry_id
    ON transcript_uploads(telemetry_id);
CREATE INDEX IF NOT EXISTS idx_transcript_uploads_created_at
    ON transcript_uploads(created_at);

-- Website beacon detail rows (web_pageview / web_cta_click / web_vital /
-- web_error), keyed by event_id like session_details / turn_details. Added in
-- migration 0016 and extended with privacy-safe quality fields in 0018.
CREATE TABLE IF NOT EXISTS web_details (
    event_id TEXT PRIMARY KEY,
    path TEXT,
    referrer TEXT,
    visitor_id TEXT,
    utm_source TEXT,
    utm_medium TEXT,
    utm_campaign TEXT,
    cta TEXT,
    metric_name TEXT,
    metric_value REAL,
    rating TEXT,
    error_kind TEXT,
    pageview_id TEXT,
    conversion_id TEXT,
    placement TEXT,
    install_method TEXT,
    FOREIGN KEY (event_id) REFERENCES events(event_id)
);

CREATE INDEX IF NOT EXISTS idx_web_details_visitor_id ON web_details(visitor_id);
CREATE INDEX IF NOT EXISTS idx_web_details_path ON web_details(path);
CREATE INDEX IF NOT EXISTS idx_web_details_cta ON web_details(cta);
CREATE INDEX IF NOT EXISTS idx_web_details_conversion_id ON web_details(conversion_id)
    WHERE conversion_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_web_details_pageview_id ON web_details(pageview_id)
    WHERE pageview_id IS NOT NULL;

-- Cross-system install attribution. conversion_id is a per-click random UUID
-- minted by the website and removed after 90 days by the retention job.
CREATE TABLE IF NOT EXISTS install_details (
    event_id TEXT PRIMARY KEY,
    conversion_id TEXT,
    stage TEXT,
    outcome TEXT,
    source TEXT,
    placement TEXT,
    install_method TEXT,
    failure_stage TEXT,
    FOREIGN KEY (event_id) REFERENCES events(event_id)
);

CREATE INDEX IF NOT EXISTS idx_install_details_conversion_id ON install_details(conversion_id)
    WHERE conversion_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_install_details_stage_outcome ON install_details(stage, outcome);

-- Privacy-safe sponsored-discovery attempt details. Free-text query and reason
-- content are never sent by the client and therefore cannot be stored here.
CREATE TABLE IF NOT EXISTS discovery_details (
    event_id TEXT PRIMARY KEY,
    request_id TEXT NOT NULL,
    phase TEXT NOT NULL,
    category TEXT,
    selected_tool TEXT,
    outcome TEXT NOT NULL,
    failure_reason TEXT,
    http_status INTEGER,
    latency_ms INTEGER NOT NULL DEFAULT 0,
    response_bytes INTEGER,
    result_count INTEGER,
    query_present INTEGER NOT NULL DEFAULT 0,
    reason_present INTEGER NOT NULL DEFAULT 0,
    custom_endpoint INTEGER NOT NULL DEFAULT 0,
    benchmark_run INTEGER NOT NULL DEFAULT 0,
    FOREIGN KEY (event_id) REFERENCES events(event_id)
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_discovery_request_id ON discovery_details(request_id);
CREATE INDEX IF NOT EXISTS idx_discovery_phase_outcome ON discovery_details(phase, outcome);
CREATE INDEX IF NOT EXISTS idx_discovery_category_outcome ON discovery_details(category, outcome);
CREATE INDEX IF NOT EXISTS idx_discovery_selected_tool ON discovery_details(selected_tool);
CREATE INDEX IF NOT EXISTS idx_discovery_failure_reason ON discovery_details(failure_reason);
CREATE INDEX IF NOT EXISTS idx_discovery_benchmark_run ON discovery_details(benchmark_run);

-- One privacy-safe aggregate per client runtime session. `correlation_id` is a
-- fresh UUID and the parent event's telemetry_id is the same ephemeral value,
-- never the install telemetry ID, so these rows cannot be joined to accounts or
-- activity across sessions.
CREATE TABLE IF NOT EXISTS todo_session_details (
    event_id TEXT PRIMARY KEY,
    correlation_id TEXT NOT NULL UNIQUE,
    session_end_reason TEXT NOT NULL,
    todos_created INTEGER NOT NULL DEFAULT 0,
    todos_completed INTEGER NOT NULL DEFAULT 0,
    todos_abandoned INTEGER NOT NULL DEFAULT 0,
    todo_updates INTEGER NOT NULL DEFAULT 0,
    groups_completed INTEGER NOT NULL DEFAULT 0,
    groups_total INTEGER NOT NULL DEFAULT 0,
    max_todo_list_size INTEGER NOT NULL DEFAULT 0,
    confidence_min INTEGER,
    confidence_mean REAL,
    confidence_count INTEGER NOT NULL DEFAULT 0,
    completion_confidence_min INTEGER,
    completion_confidence_mean REAL,
    completion_confidence_count INTEGER NOT NULL DEFAULT 0,
    understands_user_intent_min INTEGER,
    understands_user_intent_mean REAL,
    understands_user_intent_count INTEGER NOT NULL DEFAULT 0,
    closed_feedback_loop_min INTEGER,
    closed_feedback_loop_mean REAL,
    closed_feedback_loop_count INTEGER NOT NULL DEFAULT 0,
    end_to_end_ownership_min INTEGER,
    end_to_end_ownership_mean REAL,
    end_to_end_ownership_count INTEGER NOT NULL DEFAULT 0,
    FOREIGN KEY (event_id) REFERENCES events(event_id)
);

CREATE INDEX IF NOT EXISTS idx_todo_session_completed
    ON todo_session_details(todos_completed);
CREATE INDEX IF NOT EXISTS idx_todo_session_groups_completed
    ON todo_session_details(groups_completed);

CREATE TABLE IF NOT EXISTS session_details (
    event_id TEXT PRIMARY KEY,
    session_start_hour_utc INTEGER,
    session_start_weekday_utc INTEGER,
    session_end_hour_utc INTEGER,
    session_end_weekday_utc INTEGER,
    previous_session_gap_secs INTEGER,
    sessions_started_24h INTEGER DEFAULT 0,
    sessions_started_7d INTEGER DEFAULT 0,
    active_sessions_at_start INTEGER DEFAULT 0,
    other_active_sessions_at_start INTEGER DEFAULT 0,
    max_concurrent_sessions INTEGER DEFAULT 0,
    multi_sessioned INTEGER DEFAULT 0,
    first_file_edit_ms INTEGER,
    first_test_pass_ms INTEGER,
    tool_cat_read_search INTEGER DEFAULT 0,
    tool_cat_write INTEGER DEFAULT 0,
    tool_cat_shell INTEGER DEFAULT 0,
    tool_cat_web INTEGER DEFAULT 0,
    tool_cat_memory INTEGER DEFAULT 0,
    tool_cat_subagent INTEGER DEFAULT 0,
    tool_cat_swarm INTEGER DEFAULT 0,
    tool_cat_email INTEGER DEFAULT 0,
    tool_cat_side_panel INTEGER DEFAULT 0,
    tool_cat_goal INTEGER DEFAULT 0,
    tool_cat_mcp INTEGER DEFAULT 0,
    tool_cat_other INTEGER DEFAULT 0,
    -- Todo tool telemetry (migration 0021). The events table is at D1's
    -- column cap, so session-level todo fields live here.
    tool_cat_todo INTEGER DEFAULT 0,
    feature_todo_used INTEGER DEFAULT 0,
    todo_gate_ownership_count INTEGER DEFAULT 0,
    todo_gate_hill_count INTEGER DEFAULT 0,
    todo_gate_completion_count INTEGER DEFAULT 0,
    todo_gate_spike_count INTEGER DEFAULT 0,
    command_login_used INTEGER DEFAULT 0,
    command_model_used INTEGER DEFAULT 0,
    command_usage_used INTEGER DEFAULT 0,
    command_resume_used INTEGER DEFAULT 0,
    command_memory_used INTEGER DEFAULT 0,
    command_swarm_used INTEGER DEFAULT 0,
    command_goal_used INTEGER DEFAULT 0,
    command_selfdev_used INTEGER DEFAULT 0,
    command_feedback_used INTEGER DEFAULT 0,
    command_other_used INTEGER DEFAULT 0,
    workflow_chat_only INTEGER DEFAULT 0,
    workflow_coding_used INTEGER DEFAULT 0,
    workflow_research_used INTEGER DEFAULT 0,
    workflow_tests_used INTEGER DEFAULT 0,
    workflow_background_used INTEGER DEFAULT 0,
    workflow_subagent_used INTEGER DEFAULT 0,
    workflow_swarm_used INTEGER DEFAULT 0,
    project_repo_present INTEGER DEFAULT 0,
    project_lang_rust INTEGER DEFAULT 0,
    project_lang_js_ts INTEGER DEFAULT 0,
    project_lang_python INTEGER DEFAULT 0,
    project_lang_go INTEGER DEFAULT 0,
    project_lang_markdown INTEGER DEFAULT 0,
    project_lang_mixed INTEGER DEFAULT 0,
    days_since_install INTEGER,
    active_days_7d INTEGER DEFAULT 0,
    active_days_30d INTEGER DEFAULT 0,
    FOREIGN KEY (event_id) REFERENCES events(event_id)
);

CREATE TABLE IF NOT EXISTS turn_details (
    event_id TEXT PRIMARY KEY,
    turn_index INTEGER,
    turn_started_ms INTEGER,
    turn_active_duration_ms INTEGER,
    idle_before_turn_ms INTEGER,
    idle_after_turn_ms INTEGER,
    turn_success INTEGER DEFAULT 0,
    turn_abandoned INTEGER DEFAULT 0,
    turn_end_reason TEXT,
    input_tokens INTEGER DEFAULT 0,
    output_tokens INTEGER DEFAULT 0,
    total_tokens INTEGER DEFAULT 0,
    assistant_responses INTEGER DEFAULT 0,
    first_assistant_response_ms INTEGER,
    first_tool_call_ms INTEGER,
    first_tool_success_ms INTEGER,
    first_file_edit_ms INTEGER,
    first_test_pass_ms INTEGER,
    tool_calls INTEGER DEFAULT 0,
    tool_failures INTEGER DEFAULT 0,
    executed_tool_calls INTEGER DEFAULT 0,
    executed_tool_successes INTEGER DEFAULT 0,
    executed_tool_failures INTEGER DEFAULT 0,
    tool_latency_total_ms INTEGER DEFAULT 0,
    tool_latency_max_ms INTEGER DEFAULT 0,
    file_write_calls INTEGER DEFAULT 0,
    tests_run INTEGER DEFAULT 0,
    tests_passed INTEGER DEFAULT 0,
    feature_memory_used INTEGER DEFAULT 0,
    feature_swarm_used INTEGER DEFAULT 0,
    feature_web_used INTEGER DEFAULT 0,
    feature_email_used INTEGER DEFAULT 0,
    feature_mcp_used INTEGER DEFAULT 0,
    feature_side_panel_used INTEGER DEFAULT 0,
    feature_goal_used INTEGER DEFAULT 0,
    feature_selfdev_used INTEGER DEFAULT 0,
    feature_background_used INTEGER DEFAULT 0,
    feature_subagent_used INTEGER DEFAULT 0,
    unique_mcp_servers INTEGER DEFAULT 0,
    tool_cat_read_search INTEGER DEFAULT 0,
    tool_cat_write INTEGER DEFAULT 0,
    tool_cat_shell INTEGER DEFAULT 0,
    tool_cat_web INTEGER DEFAULT 0,
    tool_cat_memory INTEGER DEFAULT 0,
    tool_cat_subagent INTEGER DEFAULT 0,
    tool_cat_swarm INTEGER DEFAULT 0,
    tool_cat_email INTEGER DEFAULT 0,
    tool_cat_side_panel INTEGER DEFAULT 0,
    tool_cat_goal INTEGER DEFAULT 0,
    tool_cat_mcp INTEGER DEFAULT 0,
    tool_cat_other INTEGER DEFAULT 0,
    -- Todo tool telemetry (migration 0021).
    tool_cat_todo INTEGER DEFAULT 0,
    feature_todo_used INTEGER DEFAULT 0,
    todo_gate_ownership_count INTEGER DEFAULT 0,
    todo_gate_hill_count INTEGER DEFAULT 0,
    todo_gate_completion_count INTEGER DEFAULT 0,
    todo_gate_spike_count INTEGER DEFAULT 0,
    workflow_chat_only INTEGER DEFAULT 0,
    workflow_coding_used INTEGER DEFAULT 0,
    workflow_research_used INTEGER DEFAULT 0,
    workflow_tests_used INTEGER DEFAULT 0,
    workflow_background_used INTEGER DEFAULT 0,
    workflow_subagent_used INTEGER DEFAULT 0,
    workflow_swarm_used INTEGER DEFAULT 0,
    FOREIGN KEY (event_id) REFERENCES events(event_id)
);

CREATE TABLE IF NOT EXISTS daily_active_users (
    activity_date TEXT NOT NULL,
    telemetry_id TEXT NOT NULL,
    first_seen_at TEXT DEFAULT (datetime('now')),
    last_seen_at TEXT DEFAULT (datetime('now')),
    raw_active INTEGER DEFAULT 0,
    meaningful_active INTEGER DEFAULT 0,
    release_active INTEGER DEFAULT 0,
    meaningful_release_active INTEGER DEFAULT 0,
    session_start_count INTEGER DEFAULT 0,
    turn_end_count INTEGER DEFAULT 0,
    session_end_count INTEGER DEFAULT 0,
    session_crash_count INTEGER DEFAULT 0,
    ci_active INTEGER DEFAULT 0,
    last_is_ci INTEGER DEFAULT 0,
    last_build_channel TEXT,
    -- Coarse geo: 2-letter country code from Cloudflare's edge (migration
    -- 0022). Country only; IP / city / coordinates are never stored.
    last_country TEXT,
    PRIMARY KEY (activity_date, telemetry_id)
);

CREATE INDEX IF NOT EXISTS idx_daily_active_date
    ON daily_active_users(activity_date);

CREATE INDEX IF NOT EXISTS idx_daily_active_date_release
    ON daily_active_users(activity_date, release_active, meaningful_release_active);

CREATE INDEX IF NOT EXISTS idx_daily_active_date_ci
    ON daily_active_users(activity_date, last_is_ci, meaningful_release_active);

-- Coarse geographic dimension (migration 0022). Country only, derived from
-- Cloudflare's edge (request.cf.country); IP addresses are never stored.
CREATE INDEX IF NOT EXISTS idx_daily_active_date_country
    ON daily_active_users(activity_date, last_country);

CREATE TABLE IF NOT EXISTS country_daily (
    activity_date TEXT NOT NULL,
    country TEXT NOT NULL,
    event TEXT NOT NULL,
    is_ci INTEGER NOT NULL DEFAULT 0,
    event_count INTEGER NOT NULL DEFAULT 0,
    first_seen_at TEXT DEFAULT (datetime('now')),
    last_seen_at TEXT DEFAULT (datetime('now')),
    PRIMARY KEY (activity_date, country, event, is_ci)
);

CREATE INDEX IF NOT EXISTS idx_country_daily_date ON country_daily(activity_date);
CREATE INDEX IF NOT EXISTS idx_country_daily_country ON country_daily(country);

-- List prices per model, in USD per million tokens (migration 0023). Populated
-- by scripts/sync-model-prices.mjs from https://models.dev/api.json, keyed on
-- the raw telemetry model label (not a models.dev id) so gateway aliases can be
-- priced. Powers token-value.sql. See migrations/0023_model_prices.sql for the
-- full rationale, especially input_includes_cache_read.
CREATE TABLE IF NOT EXISTS model_prices (
    model TEXT PRIMARY KEY,
    source_model TEXT,
    source_provider TEXT,
    input_usd_per_mtok REAL,
    output_usd_per_mtok REAL,
    cache_read_usd_per_mtok REAL,
    cache_write_usd_per_mtok REAL,
    input_includes_cache_read INTEGER NOT NULL DEFAULT 0,
    price_kind TEXT NOT NULL DEFAULT 'catalog',
    updated_at TEXT DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_model_prices_price_kind ON model_prices(price_kind);
