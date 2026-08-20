-- Current UTC-day and trailing-24h DAU dashboard.
-- Usage:
--   wrangler d1 execute jcode-telemetry --remote --file=dau.sql
--
-- Note: production `events` never got migration 0005's per-turn columns (D1
-- caps tables at 100 columns), so turn_end activity lives in `turn_details`
-- keyed by event_id. The trailing-24h tiers join through it; the today tiers
-- read the daily_active_users rollup, which is classified at insert time from
-- the full client payload.

WITH today AS (
    SELECT
        COUNT(*) AS raw_today,
        SUM(CASE WHEN meaningful_active > 0 THEN 1 ELSE 0 END) AS meaningful_today,
        SUM(CASE WHEN release_active > 0 THEN 1 ELSE 0 END) AS raw_release_today,
        SUM(CASE WHEN meaningful_release_active > 0 THEN 1 ELSE 0 END) AS meaningful_release_today,
        -- Headline product metric: real users on the release channel, excluding
        -- automated CI traffic (ephemeral runners that mint a fresh id per job).
        SUM(CASE WHEN meaningful_release_active > 0 AND last_is_ci = 0 THEN 1 ELSE 0 END) AS meaningful_release_today_noci,
        SUM(CASE WHEN last_is_ci > 0 THEN 1 ELSE 0 END) AS ci_today
    FROM daily_active_users
    WHERE activity_date = date('now')
), pace AS (
    -- "Today" is a partial UTC day, so the today tiers always undercount and
    -- the panel looked like a cliff every morning. Compare today-so-far with
    -- the *same clock window* on prior days rather than extrapolating out to
    -- 24h: DAU is a distinct count and does not scale linearly with time.
    --
    -- Counts here use the headline population (release channel, not CI), not
    -- raw ids. Raw ids are dominated by throwaway dev-build traffic whose
    -- volume swings by 5x day to day, which is what made a normal day look
    -- first like a spike and then like a cliff.
    SELECT
        ROUND(
            100.0 * (strftime('%s', 'now') - strftime('%s', 'now', 'start of day')) / 86400.0,
            1
        ) AS day_elapsed_pct,
        COUNT(DISTINCT CASE WHEN created_at >= datetime('now', 'start of day') THEN telemetry_id END) AS users_sofar,
        COUNT(DISTINCT CASE
            WHEN created_at >= datetime('now', '-1 day', 'start of day')
             AND created_at <= datetime('now', '-1 day') THEN telemetry_id END) AS users_sofar_yday,
        COUNT(DISTINCT CASE
            WHEN created_at >= datetime('now', '-7 days', 'start of day')
             AND created_at <= datetime('now', '-7 days') THEN telemetry_id END) AS users_sofar_7d
    FROM events
    WHERE created_at >= datetime('now', '-7 days', 'start of day')
      AND build_channel IN ('release', 'ci_release')
      AND is_ci = 0
), recent AS (
    SELECT
        e.telemetry_id,
        e.event,
        e.build_channel,
        e.is_ci,
        CASE
            WHEN e.event IN ('session_end', 'session_crash') AND (
                e.turns > 0 OR e.had_user_prompt > 0 OR e.had_assistant_response > 0
                OR e.assistant_responses > 0 OR e.tool_calls > 0 OR e.executed_tool_calls > 0
                OR e.duration_secs > 0 OR e.error_provider_timeout > 0 OR e.error_auth_failed > 0
                OR e.error_tool_error > 0 OR e.error_mcp_error > 0 OR e.error_rate_limited > 0
                OR e.provider_switches > 0 OR e.model_switches > 0
            ) THEN 1
            WHEN e.event = 'turn_end' AND (
                td.assistant_responses > 0 OR td.tool_calls > 0 OR td.executed_tool_calls > 0
                OR td.file_write_calls > 0 OR td.tests_run > 0
            ) THEN 1
            ELSE 0
        END AS meaningful
    FROM events e
    LEFT JOIN turn_details td ON td.event_id = e.event_id
    WHERE e.event IN ('session_start', 'turn_end', 'session_end', 'session_crash')
      AND e.created_at > datetime('now', '-1 day')
), trailing_24h AS (
    SELECT
        COUNT(DISTINCT telemetry_id) AS raw_24h,
        COUNT(DISTINCT CASE WHEN meaningful = 1 THEN telemetry_id END) AS meaningful_24h,
        COUNT(DISTINCT CASE WHEN build_channel IN ('release', 'ci_release') THEN telemetry_id END) AS raw_release_24h,
        COUNT(DISTINCT CASE WHEN build_channel IN ('release', 'ci_release') AND meaningful = 1 THEN telemetry_id END) AS meaningful_release_24h,
        -- Same headline metric over a rolling 24h window, excluding CI traffic.
        COUNT(DISTINCT CASE WHEN build_channel IN ('release', 'ci_release') AND is_ci = 0 AND meaningful = 1 THEN telemetry_id END) AS meaningful_release_24h_noci,
        COUNT(DISTINCT CASE WHEN is_ci = 1 THEN telemetry_id END) AS ci_24h,
        -- Dev-build traffic: `debug`/`git_checkout` ids are overwhelmingly
        -- throwaway (a session_start and an onboarding_step, no session_end),
        -- and most are not env-detectable as CI. Tracked separately so swings
        -- in automation volume cannot be misread as product growth or churn.
        COUNT(DISTINCT CASE WHEN build_channel IN ('debug', 'git_checkout') THEN telemetry_id END) AS dev_build_24h
    FROM recent
)
SELECT
    -- Headline first: real users, release channel, excluding CI.
    trailing_24h.meaningful_release_24h_noci AS headline_users_24h,
    today.*,
    trailing_24h.*,
    pace.day_elapsed_pct,
    pace.users_sofar AS release_users_sofar,
    pace.users_sofar_yday AS release_users_sofar_yday,
    pace.users_sofar_7d AS release_users_sofar_7d,
    -- >1.0 means today is running ahead of that day at the same hour.
    ROUND(CAST(pace.users_sofar AS REAL) / NULLIF(pace.users_sofar_yday, 0), 2) AS pace_vs_yday,
    ROUND(CAST(pace.users_sofar AS REAL) / NULLIF(pace.users_sofar_7d, 0), 2) AS pace_vs_7d
FROM today, trailing_24h, pace;
