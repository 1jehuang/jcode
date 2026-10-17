-- Model spend dashboard from the `daily_model_usage` rollup (migration 0027).
--
-- Usage:
--   npm run model-usage
--
-- Source of truth for spend after usage_report shipped. One row per
-- (day, source, provider, model, build_channel, is_ci), so this reads a few
-- thousand rows at most and never approaches D1's per-query CPU limit, unlike
-- token-value.sql which scans raw session_end events.
--
-- Pricing follows token-value.sql: list rates from model_prices, with the
-- cache-subset correction for providers that count cached tokens inside input.
-- Read the dollar column as list-price-equivalent value, not revenue or COGS.
-- `unpriced_tokens` shows usage whose model has no price row; re-run
-- `npm run sync:model-prices` when it grows.

WITH priced AS (
    SELECT
        substr(u.usage_date, 1, 7) AS month,
        u.source,
        u.responses,
        u.input_tokens + u.output_tokens + u.cache_read_input_tokens
            + u.cache_creation_input_tokens AS gross_tokens,
        CASE WHEN p.input_usd_per_mtok IS NULL THEN 0 ELSE 1 END AS priced,
        CASE WHEN p.input_usd_per_mtok IS NULL THEN 0.0 ELSE
            (CASE WHEN COALESCE(p.input_includes_cache_read, 0) = 1
                  THEN MAX(u.input_tokens - u.cache_read_input_tokens, 0)
                  ELSE u.input_tokens END) * p.input_usd_per_mtok / 1000000.0
            + u.output_tokens * COALESCE(p.output_usd_per_mtok, 0) / 1000000.0
            + u.cache_read_input_tokens
                * COALESCE(p.cache_read_usd_per_mtok, p.input_usd_per_mtok * 0.1) / 1000000.0
            + u.cache_creation_input_tokens
                * COALESCE(p.cache_write_usd_per_mtok, p.input_usd_per_mtok * 1.25) / 1000000.0
        END AS usd
    FROM daily_model_usage u
    LEFT JOIN model_prices p ON p.model = u.model
    WHERE u.is_ci = 0
)
SELECT
    month,
    source,
    SUM(responses) AS responses,
    ROUND(SUM(gross_tokens) / 1e9, 2) AS gross_btok,
    ROUND(SUM(CASE WHEN priced = 0 THEN gross_tokens ELSE 0 END) / 1e9, 2) AS unpriced_btok,
    ROUND(SUM(usd), 2) AS list_usd
FROM priced
GROUP BY month, source
ORDER BY month DESC, list_usd DESC;
