-- Repair missing daily_active_users rows from the retained canonical event log.
-- This is intentionally insert-only: existing rows already contain richer
-- counters and must not be incremented a second time during a repair.
INSERT OR IGNORE INTO daily_active_users (
    activity_date,
    telemetry_id,
    raw_active,
    meaningful_active,
    release_active,
    meaningful_release_active,
    session_start_count,
    turn_end_count,
    session_end_count,
    session_crash_count,
    ci_active,
    last_is_ci,
    last_build_channel
)
SELECT
    date(created_at),
    telemetry_id,
    1,
    MAX(CASE
        WHEN event IN ('prompt_submitted', 'turn_end') THEN 1
        WHEN event IN ('session_end', 'session_crash') AND (
            had_user_prompt > 0 OR had_assistant_response > 0
            OR assistant_responses > 0 OR tool_calls > 0
            OR executed_tool_calls > 0 OR turns > 0
        ) THEN 1
        ELSE 0
    END),
    MAX(CASE WHEN build_channel IN ('release', 'ci_release') THEN 1 ELSE 0 END),
    MAX(CASE
        WHEN build_channel IN ('release', 'ci_release') AND (
            event IN ('prompt_submitted', 'turn_end')
            OR (event IN ('session_end', 'session_crash') AND (
                had_user_prompt > 0 OR had_assistant_response > 0
                OR assistant_responses > 0 OR tool_calls > 0
                OR executed_tool_calls > 0 OR turns > 0
            )))
        THEN 1 ELSE 0
    END),
    SUM(CASE WHEN event = 'session_start' THEN 1 ELSE 0 END),
    SUM(CASE WHEN event = 'turn_end' THEN 1 ELSE 0 END),
    SUM(CASE WHEN event = 'session_end' THEN 1 ELSE 0 END),
    SUM(CASE WHEN event = 'session_crash' THEN 1 ELSE 0 END),
    MAX(is_ci),
    MAX(is_ci),
    MAX(build_channel)
FROM events
WHERE event IN ('session_start', 'prompt_submitted', 'turn_end', 'session_end', 'session_crash')
  AND created_at >= datetime('now', '-1 day', 'start of day')
  AND created_at < datetime('now', 'start of day')
GROUP BY date(created_at), telemetry_id;
