-- Canonical "total users" definitions for jcode telemetry.
-- Usage:
--   wrangler d1 execute jcode-telemetry --remote --file=users.sql
--
-- Headline number: total_users. A "user" is a distinct, non-CI telemetry_id that
-- ever either installed jcode or did meaningful work in it. We exclude CI traffic
-- (ephemeral runners mint a fresh id per job) and exclude empty open/close
-- sessions that never did anything. Raw, less-filtered tiers are reported
-- alongside it so no signal is hidden.
--
-- Caveats (see README "Accuracy notes"): telemetry_id is per-machine, so one
-- person on N machines counts as N; opt-outs and network-blocked clients are
-- never counted; CI rows created before the is_ci column existed default to 0
-- and may slip in.

SELECT
    -- HEADLINE: real people who installed or meaningfully used jcode.
    COUNT(DISTINCT CASE WHEN is_ci = 0 AND (
            event = 'install'
            OR (event IN ('session_end', 'session_crash') AND (
                turns > 0 OR had_user_prompt > 0 OR had_assistant_response > 0
                OR assistant_responses > 0 OR tool_calls > 0 OR executed_tool_calls > 0
                OR duration_secs > 0 OR error_provider_timeout > 0 OR error_auth_failed > 0
                OR error_tool_error > 0 OR error_mcp_error > 0 OR error_rate_limited > 0
                OR provider_switches > 0 OR model_switches > 0
            ))
            OR (event = 'turn_end' AND (
                assistant_responses > 0 OR tool_calls > 0 OR executed_tool_calls > 0
                OR file_write_calls > 0 OR tests_run > 0 OR turn_success > 0
            ))
        ) THEN telemetry_id END) AS total_users,

    -- Core users: did meaningful work (excludes install-only, never-used ids).
    COUNT(DISTINCT CASE WHEN is_ci = 0 AND (
            (event IN ('session_end', 'session_crash') AND (
                turns > 0 OR had_user_prompt > 0 OR had_assistant_response > 0
                OR assistant_responses > 0 OR tool_calls > 0 OR executed_tool_calls > 0
                OR duration_secs > 0 OR error_provider_timeout > 0 OR error_auth_failed > 0
                OR error_tool_error > 0 OR error_mcp_error > 0 OR error_rate_limited > 0
                OR provider_switches > 0 OR model_switches > 0
            ))
            OR (event = 'turn_end' AND (
                assistant_responses > 0 OR tool_calls > 0 OR executed_tool_calls > 0
                OR file_write_calls > 0 OR tests_run > 0 OR turn_success > 0
            ))
        ) THEN telemetry_id END) AS core_users,

    -- Reach: every distinct non-CI id that ever launched jcode (incl. empty
    -- open/close sessions). Upper bound on "people who ran it at least once".
    COUNT(DISTINCT CASE WHEN is_ci = 0 THEN telemetry_id END) AS reached_users,

    -- Installs only (non-CI), for comparison with total_users.
    COUNT(DISTINCT CASE WHEN is_ci = 0 AND event = 'install' THEN telemetry_id END) AS installed_users,

    -- Unfiltered grand total (includes CI + dev). Never use as the headline;
    -- kept for transparency and for sizing CI noise.
    COUNT(DISTINCT telemetry_id) AS all_ids_including_ci,

    -- CI-only ids, so the gap between all_ids and total_users is explainable.
    COUNT(DISTINCT CASE WHEN is_ci = 1 THEN telemetry_id END) AS ci_ids
FROM events;
