-- Users who ran at least one prompt.
-- Usage:
--   npm run prompt-users
--
-- A prompt user is a distinct non-CI telemetry_id with either:
--   * a prompt_submitted row (emitted immediately when a prompt is accepted),
--   * a turn_end row (fires only after a real user turn completes), or
--   * a session_end/session_crash row with had_user_prompt > 0.
--
-- prompt_submitted and turn_end are retained in D1 for 30 days. They capture
-- in-flight and unclosed sessions, so observable_mau is the accurate current
-- product metric. The lifecycle-only MAU comparison remains alongside it for a
-- retention-equivalent month-over-month trend. All-time is a durable lower bound.
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
        event IN ('prompt_submitted', 'turn_end')
        OR (event IN ('session_end', 'session_crash') AND had_user_prompt > 0)
      )
), observable AS (
    SELECT COUNT(DISTINCT telemetry_id) AS mau_observable
    FROM events
    WHERE is_ci = 0
      AND created_at >= datetime('now', '-30 days')
      AND (
        event IN ('prompt_submitted', 'turn_end')
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
    mau_observable,
    mau_durable,
    mau_durable_previous,
    ROUND(100.0 * (mau_durable - mau_durable_previous)
        / NULLIF(mau_durable_previous, 0), 1) AS mau_mom_pct,
    all_time_durable_lower_bound
FROM recent, observable, durable;
