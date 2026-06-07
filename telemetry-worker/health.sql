-- Telemetry health dashboard query.
-- Usage:
--   wrangler d1 execute jcode-telemetry --remote --file=health.sql

WITH install_ids AS (
    SELECT DISTINCT telemetry_id
    FROM events INDEXED BY idx_events_event_telemetry_created
    WHERE event = 'install'
), lifecycle AS (
    SELECT telemetry_id, created_at
    FROM events INDEXED BY idx_events_event_telemetry_created
    WHERE event IN ('session_end', 'session_crash')
), session_starts_by_id AS (
    SELECT DISTINCT telemetry_id
    FROM events INDEXED BY idx_events_event_telemetry_created
    WHERE event = 'session_start'
), event_counts AS (
    SELECT
        SUM(CASE WHEN event = 'install' THEN 1 ELSE 0 END) AS install_events,
        SUM(CASE WHEN event = 'session_start' THEN 1 ELSE 0 END) AS session_starts,
        SUM(CASE WHEN event = 'session_end' THEN 1 ELSE 0 END) AS session_ends,
        SUM(CASE WHEN event = 'session_crash' THEN 1 ELSE 0 END) AS session_crashes
    FROM events INDEXED BY idx_events_event_created_telemetry
    WHERE event IN ('install', 'session_start', 'session_end', 'session_crash')
), identity_counts AS (
    SELECT
        (SELECT COUNT(*) FROM install_ids) AS install_ids,
        (SELECT COUNT(DISTINCT telemetry_id) FROM lifecycle) AS lifecycle_ids,
        (SELECT COUNT(*) FROM session_starts_by_id) AS session_start_ids,
        (SELECT COUNT(DISTINCT lifecycle.telemetry_id)
         FROM lifecycle
         LEFT JOIN install_ids USING (telemetry_id)
         WHERE install_ids.telemetry_id IS NULL) AS lifecycle_ids_without_install
),
meaningful AS (
    SELECT
        SUM(CASE WHEN event IN ('session_end', 'session_crash') THEN 1 ELSE 0 END) AS meaningful_sessions,
        COUNT(DISTINCT telemetry_id) AS meaningful_users_30d
    FROM events
    INDEXED BY idx_events_event_created_telemetry
    WHERE event IN ('session_end', 'session_crash', 'turn_end')
      AND created_at > datetime('now', '-30 days')
      AND (
        (event IN ('session_end', 'session_crash') AND (
          turns > 0
          OR duration_mins > 0
          OR error_provider_timeout > 0
          OR error_auth_failed > 0
          OR error_tool_error > 0
          OR error_mcp_error > 0
          OR error_rate_limited > 0
          OR provider_switches > 0
          OR model_switches > 0
          OR had_user_prompt > 0
          OR had_assistant_response > 0
          OR assistant_responses > 0
          OR tool_calls > 0
          OR tool_failures > 0
          OR executed_tool_calls > 0
          OR feature_memory_used > 0
          OR feature_swarm_used > 0
          OR feature_web_used > 0
          OR feature_email_used > 0
          OR feature_mcp_used > 0
          OR feature_side_panel_used > 0
          OR feature_goal_used > 0
          OR feature_selfdev_used > 0
          OR feature_background_used > 0
          OR feature_subagent_used > 0
        ))
        OR (event = 'turn_end' AND (
          assistant_responses > 0
          OR tool_calls > 0
          OR executed_tool_calls > 0
          OR file_write_calls > 0
          OR tests_run > 0
          OR turn_success > 0
        ))
      )
),
outliers AS (
    SELECT
        MAX(session_events) AS max_session_events_one_id,
        SUM(CASE WHEN rn <= 5 THEN session_events ELSE 0 END) AS top5_session_events,
        SUM(session_events) AS total_session_events
    FROM (
        SELECT telemetry_id, COUNT(*) AS session_events,
               ROW_NUMBER() OVER (ORDER BY COUNT(*) DESC) AS rn
        FROM lifecycle
        GROUP BY telemetry_id
    )
),
ci_noise AS (
    SELECT
        COUNT(DISTINCT telemetry_id) AS ci_ids_30d,
        COUNT(DISTINCT CASE WHEN event = 'install' THEN telemetry_id END) AS ci_install_ids,
        COUNT(DISTINCT CASE WHEN event IN ('session_end', 'session_crash') THEN telemetry_id END) AS ci_lifecycle_ids
    FROM events
    INDEXED BY idx_events_event_created_telemetry
    WHERE event IN ('install', 'session_start', 'turn_end', 'session_end', 'session_crash')
      AND created_at > datetime('now', '-30 days')
      AND is_ci = 1
)
SELECT
    install_events,
    session_starts,
    session_ends,
    session_crashes,
    install_ids,
    lifecycle_ids,
    session_start_ids,
    lifecycle_ids_without_install,
    meaningful_sessions,
    meaningful_users_30d,
    ci_ids_30d,
    ci_install_ids,
    ci_lifecycle_ids,
    max_session_events_one_id,
    top5_session_events,
    total_session_events,
    ROUND(CAST(session_ends + session_crashes AS REAL) / NULLIF(session_starts, 0), 3) AS lifecycle_completion_ratio
FROM event_counts, identity_counts, meaningful, outliers, ci_noise;
