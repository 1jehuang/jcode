-- Current UTC-day and trailing-24h DAU dashboard.
-- Usage:
--   wrangler d1 execute jcode-telemetry --remote --file=dau.sql

WITH today AS (
    SELECT
        COUNT(*) AS raw_today,
        SUM(CASE WHEN meaningful_active > 0 THEN 1 ELSE 0 END) AS meaningful_today,
        SUM(CASE WHEN release_active > 0 THEN 1 ELSE 0 END) AS raw_release_today,
        SUM(CASE WHEN meaningful_release_active > 0 THEN 1 ELSE 0 END) AS meaningful_release_today
    FROM daily_active_users
    WHERE activity_date = date('now')
), trailing_24h AS (
    SELECT
        COUNT(DISTINCT telemetry_id) AS raw_24h,
        COUNT(DISTINCT CASE WHEN event IN ('session_end', 'session_crash')
            AND (
                turns > 0 OR had_user_prompt > 0 OR had_assistant_response > 0
                OR assistant_responses > 0 OR tool_calls > 0 OR executed_tool_calls > 0
                OR duration_secs > 0 OR error_provider_timeout > 0 OR error_auth_failed > 0
                OR error_tool_error > 0 OR error_mcp_error > 0 OR error_rate_limited > 0
                OR provider_switches > 0 OR model_switches > 0
            ) THEN telemetry_id END) AS meaningful_24h,
        COUNT(DISTINCT CASE WHEN build_channel = 'release' THEN telemetry_id END) AS raw_release_24h,
        COUNT(DISTINCT CASE WHEN build_channel = 'release'
            AND event IN ('session_end', 'session_crash')
            AND (
                turns > 0 OR had_user_prompt > 0 OR had_assistant_response > 0
                OR assistant_responses > 0 OR tool_calls > 0 OR executed_tool_calls > 0
                OR duration_secs > 0 OR error_provider_timeout > 0 OR error_auth_failed > 0
                OR error_tool_error > 0 OR error_mcp_error > 0 OR error_rate_limited > 0
                OR provider_switches > 0 OR model_switches > 0
            ) THEN telemetry_id END) AS meaningful_release_24h
    FROM events INDEXED BY idx_events_event_created_telemetry
    WHERE event IN ('session_start', 'turn_end', 'session_end', 'session_crash')
      AND created_at > datetime('now', '-1 day')
)
SELECT * FROM today, trailing_24h;
