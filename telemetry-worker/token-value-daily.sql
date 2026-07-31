-- Daily dollar value of tokens flowing through jcode, one row per day.
--
-- Usage:
--   npm run token-value:daily
--
-- token-value.sql answers "what is this worth and which models drive it" across
-- three stacked panels, which makes it wide and sorts the daily rows by dollars
-- rather than by date. This file is the plain time series: one row per day, in
-- date order, next to the token and session counts it came from, so it can be
-- read directly or piped somewhere that draws a chart.
--
-- Deliberately no per-user dollar column: it tracked tokens-per-user almost
-- exactly (coefficient of variation 0.147 vs 0.142 over a 10-day sample),
-- because the blended rate per million tokens barely moves day to day. It was
-- the same series twice in different units.
--
-- Requires migration 0023 plus a populated `model_prices` table:
--   npm run migrate:model-prices && npm run sync:model-prices
--
-- Same accounting rules as token-value.sql (see that file for the full notes):
-- session_end rows only, CI excluded, and cache reads subtracted from the input
-- bucket for providers that report them as a subset of prompt tokens.
WITH valued AS (
    SELECT
        substr(e.created_at, 1, 10) AS day,
        e.telemetry_id,
        p.price_kind,
        e.input_tokens + e.output_tokens + e.cache_read_input_tokens
            + e.cache_creation_input_tokens AS total_tokens,
        -- Per-session dollar value, computed once here so the aggregates below
        -- stay readable and cannot drift apart.
        CASE WHEN p.input_usd_per_mtok IS NULL THEN 0.0 ELSE
            CASE
                WHEN COALESCE(p.input_includes_cache_read, 0) = 1
                    THEN MAX(e.input_tokens - e.cache_read_input_tokens, 0)
                ELSE e.input_tokens
            END * p.input_usd_per_mtok / 1000000.0
            + e.output_tokens * COALESCE(p.output_usd_per_mtok, 0) / 1000000.0
            + e.cache_read_input_tokens
                * COALESCE(p.cache_read_usd_per_mtok, p.input_usd_per_mtok * 0.1)
                / 1000000.0
            + e.cache_creation_input_tokens
                * COALESCE(p.cache_write_usd_per_mtok, p.input_usd_per_mtok * 1.25)
                / 1000000.0
        END AS usd
    FROM events e
    LEFT JOIN model_prices p ON p.model = e.model_end
    WHERE e.event = 'session_end'
      AND e.created_at >= datetime('now', '-60 days')
      AND e.is_ci = 0
)
SELECT
    day,
    ROUND(SUM(usd), 2) AS usd,
    SUM(total_tokens) AS tokens,
    COUNT(*) AS sessions,
    COUNT(DISTINCT telemetry_id) AS users,
    -- Coverage guard: if this drops, re-run the price sync before quoting the
    -- dollar column. Unpriced models contribute tokens but no dollars.
    ROUND(
        100.0 * SUM(CASE WHEN price_kind = 'catalog' THEN total_tokens ELSE 0 END)
            / NULLIF(SUM(total_tokens), 0),
        1
    ) AS priced_pct
FROM valued
GROUP BY day
ORDER BY day;
