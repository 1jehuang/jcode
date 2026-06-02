-- Backfill recent daily active-user rollups from canonical raw events.
-- Limited to the last 35 days to keep D1 work bounded while making DAU/WAU/MAU
-- dashboards immediately useful after the rollup table is introduced.

INSERT INTO daily_active_users (
    activity_date,
    telemetry_id,
    first_seen_at,
    last_seen_at,
    raw_active,
    meaningful_active,
    release_active,
    meaningful_release_active,
    session_start_count,
    turn_end_count,
    session_end_count,
    session_crash_count,
    last_build_channel
)
SELECT
    date(created_at) AS activity_date,
    telemetry_id,
    MIN(created_at) AS first_seen_at,
    MAX(created_at) AS last_seen_at,
    1 AS raw_active,
    MAX(CASE WHEN event IN ('session_end', 'session_crash') AND (
        turns > 0 OR had_user_prompt > 0 OR had_assistant_response > 0
        OR assistant_responses > 0 OR tool_calls > 0 OR executed_tool_calls > 0
        OR duration_secs > 0 OR error_provider_timeout > 0 OR error_auth_failed > 0
        OR error_tool_error > 0 OR error_mcp_error > 0 OR error_rate_limited > 0
        OR provider_switches > 0 OR model_switches > 0
    ) THEN 1 ELSE 0 END) AS meaningful_active,
    MAX(CASE WHEN build_channel = 'release' THEN 1 ELSE 0 END) AS release_active,
    MAX(CASE WHEN build_channel = 'release' AND event IN ('session_end', 'session_crash') AND (
        turns > 0 OR had_user_prompt > 0 OR had_assistant_response > 0
        OR assistant_responses > 0 OR tool_calls > 0 OR executed_tool_calls > 0
        OR duration_secs > 0 OR error_provider_timeout > 0 OR error_auth_failed > 0
        OR error_tool_error > 0 OR error_mcp_error > 0 OR error_rate_limited > 0
        OR provider_switches > 0 OR model_switches > 0
    ) THEN 1 ELSE 0 END) AS meaningful_release_active,
    SUM(CASE WHEN event = 'session_start' THEN 1 ELSE 0 END) AS session_start_count,
    SUM(CASE WHEN event = 'turn_end' THEN 1 ELSE 0 END) AS turn_end_count,
    SUM(CASE WHEN event = 'session_end' THEN 1 ELSE 0 END) AS session_end_count,
    SUM(CASE WHEN event = 'session_crash' THEN 1 ELSE 0 END) AS session_crash_count,
    MAX(build_channel) AS last_build_channel
FROM events INDEXED BY idx_events_event_created_telemetry
WHERE event IN ('session_start', 'turn_end', 'session_end', 'session_crash')
  AND created_at > datetime('now', '-35 days')
GROUP BY date(created_at), telemetry_id
ON CONFLICT(activity_date, telemetry_id) DO UPDATE SET
    first_seen_at = MIN(daily_active_users.first_seen_at, excluded.first_seen_at),
    last_seen_at = MAX(daily_active_users.last_seen_at, excluded.last_seen_at),
    raw_active = 1,
    meaningful_active = MAX(daily_active_users.meaningful_active, excluded.meaningful_active),
    release_active = MAX(daily_active_users.release_active, excluded.release_active),
    meaningful_release_active = MAX(daily_active_users.meaningful_release_active, excluded.meaningful_release_active),
    session_start_count = excluded.session_start_count,
    turn_end_count = excluded.turn_end_count,
    session_end_count = excluded.session_end_count,
    session_crash_count = excluded.session_crash_count,
    last_build_channel = COALESCE(excluded.last_build_channel, daily_active_users.last_build_channel);
