-- Users who ran at least one prompt.
-- Usage:
--   npm run prompt-users
--
-- A prompt user is a distinct non-CI telemetry_id with either:
--   * a turn_end row (fires only after a real user turn completes), or
--   * a session_end/session_crash row with had_user_prompt > 0.
--
-- turn_end is retained in D1 for 30 days. It captures in-flight and unclosed
-- sessions, so it is used for DAU and WAU where both comparison windows have
-- equivalent coverage. MAU growth and all-time users use durable lifecycle rows
-- only, avoiding a current-period boost that the prior 30-day window cannot get.
-- The all-time number is therefore a durable lower bound.
--
-- telemetry_id is per-machine, opt-outs are absent, and old rows written before
-- is_ci existed can be misclassified because they default to non-CI.
WITH recent AS (
    SELECT
        COUNT(DISTINCT CASE WHEN created_at >= datetime('now', '-1 day')
            THEN telemetry_id END) AS dau_24h,
        COUNT(DISTINCT CASE
            WHEN created_at >= datetime('now', '-8 days')
             AND created_at < datetime('now', '-7 days')
            THEN telemetry_id END) AS dau_24h_week_ago,
        COUNT(DISTINCT CASE WHEN created_at >= datetime('now', 'start of day')
            THEN telemetry_id END) AS today_utc_sofar,
        COUNT(DISTINCT CASE
            WHEN created_at >= datetime('now', '-1 day', 'start of day')
             AND created_at < datetime('now', 'start of day')
            THEN telemetry_id END) AS yesterday_utc,
        COUNT(DISTINCT CASE WHEN created_at >= datetime('now', '-7 days')
            THEN telemetry_id END) AS wau,
        COUNT(DISTINCT CASE
            WHEN created_at >= datetime('now', '-14 days')
             AND created_at < datetime('now', '-7 days')
            THEN telemetry_id END) AS wau_previous
    FROM events
    WHERE is_ci = 0
      AND created_at >= datetime('now', '-14 days')
      AND (
        event = 'turn_end'
        OR (event IN ('session_end', 'session_crash') AND had_user_prompt > 0)
      )
), durable AS (
    SELECT
        COUNT(DISTINCT CASE WHEN created_at >= datetime('now', '-30 days')
            THEN telemetry_id END) AS mau_durable,
        COUNT(DISTINCT CASE
            WHEN created_at >= datetime('now', '-60 days')
             AND created_at < datetime('now', '-30 days')
            THEN telemetry_id END) AS mau_durable_previous,
        COUNT(DISTINCT telemetry_id) AS all_time_durable_lower_bound
    FROM events
    WHERE is_ci = 0
      AND event IN ('session_end', 'session_crash')
      AND had_user_prompt > 0
)
SELECT
    dau_24h,
    dau_24h_week_ago,
    ROUND(100.0 * (dau_24h - dau_24h_week_ago)
        / NULLIF(dau_24h_week_ago, 0), 1) AS dau_wow_pct,
    today_utc_sofar,
    yesterday_utc,
    wau,
    wau_previous,
    ROUND(100.0 * (wau - wau_previous) / NULLIF(wau_previous, 0), 1) AS wau_wow_pct,
    mau_durable,
    mau_durable_previous,
    ROUND(100.0 * (mau_durable - mau_durable_previous)
        / NULLIF(mau_durable_previous, 0), 1) AS mau_mom_pct,
    all_time_durable_lower_bound
FROM recent, durable;
