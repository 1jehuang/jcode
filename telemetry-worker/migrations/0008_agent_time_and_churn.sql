-- Agent-time, autonomy, and churn/pain attribution telemetry.

-- Session-level agent-hours and pain/churn attribution.
ALTER TABLE events ADD COLUMN session_stop_reason TEXT;
ALTER TABLE events ADD COLUMN agent_role TEXT;
ALTER TABLE events ADD COLUMN parent_session_id TEXT;
ALTER TABLE events ADD COLUMN agent_active_ms_total INTEGER DEFAULT 0;
ALTER TABLE events ADD COLUMN agent_model_ms_total INTEGER DEFAULT 0;
ALTER TABLE events ADD COLUMN agent_tool_ms_total INTEGER DEFAULT 0;
ALTER TABLE events ADD COLUMN session_idle_ms_total INTEGER DEFAULT 0;
ALTER TABLE events ADD COLUMN agent_blocked_ms_total INTEGER DEFAULT 0;
ALTER TABLE events ADD COLUMN time_to_first_agent_action_ms INTEGER;
ALTER TABLE events ADD COLUMN time_to_first_useful_action_ms INTEGER;
ALTER TABLE events ADD COLUMN spawned_agent_count INTEGER DEFAULT 0;
ALTER TABLE events ADD COLUMN background_task_count INTEGER DEFAULT 0;
ALTER TABLE events ADD COLUMN background_task_completed_count INTEGER DEFAULT 0;
ALTER TABLE events ADD COLUMN subagent_task_count INTEGER DEFAULT 0;
ALTER TABLE events ADD COLUMN subagent_success_count INTEGER DEFAULT 0;
ALTER TABLE events ADD COLUMN swarm_task_count INTEGER DEFAULT 0;
ALTER TABLE events ADD COLUMN swarm_success_count INTEGER DEFAULT 0;
ALTER TABLE events ADD COLUMN user_cancelled_count INTEGER DEFAULT 0;

CREATE INDEX IF NOT EXISTS idx_events_session_stop_reason ON events(session_stop_reason);
CREATE INDEX IF NOT EXISTS idx_events_agent_role ON events(agent_role);

CREATE TABLE IF NOT EXISTS turn_details (
    event_id TEXT PRIMARY KEY,
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
    workflow_chat_only INTEGER DEFAULT 0,
    workflow_coding_used INTEGER DEFAULT 0,
    workflow_research_used INTEGER DEFAULT 0,
    workflow_tests_used INTEGER DEFAULT 0,
    workflow_background_used INTEGER DEFAULT 0,
    workflow_subagent_used INTEGER DEFAULT 0,
    workflow_swarm_used INTEGER DEFAULT 0,
    FOREIGN KEY (event_id) REFERENCES events(event_id)
);
