-- Legacy session_end list-price-equivalent estimate, not authoritative usage.
-- Usage: npm run token-value:daily
-- Requires migration 0023 and npm run sync:model-prices.
-- Never add this series to model-usage.sql response rollups. Session totals
-- use the final model label, so mixed-model sessions are only estimates.
-- CI excluded. Cache-subset correction applies to legacy counters only.
-- Unknown used rates mark the entire row unpriced, never guessed or free.
WITH normalized AS (
    SELECT substr(e.created_at, 1, 10) AS day, e.telemetry_id,
        CASE WHEN COALESCE(p.input_includes_cache_read, 0) = 1
            THEN MAX(e.input_tokens - e.cache_read_input_tokens, 0)
            ELSE e.input_tokens END AS input_tokens,
        e.output_tokens, e.cache_read_input_tokens, e.cache_creation_input_tokens,
        p.price_kind, p.input_usd_per_mtok, p.output_usd_per_mtok,
        p.cache_read_usd_per_mtok, p.cache_write_usd_per_mtok
    FROM events e
    LEFT JOIN model_prices p ON p.model = e.model_end
    WHERE e.event = 'session_end'
      AND e.created_at >= date('now', '-60 days')
      AND datetime(e.created_at) >= datetime('now', '-60 days')
      AND e.is_ci = 0
), covered AS (
    SELECT *, input_tokens + output_tokens + cache_read_input_tokens
        + cache_creation_input_tokens AS total_tokens,
        CASE WHEN price_kind IN ('catalog', 'free')
          AND (input_tokens = 0 OR input_usd_per_mtok IS NOT NULL)
          AND (output_tokens = 0 OR output_usd_per_mtok IS NOT NULL)
          AND (cache_read_input_tokens = 0 OR cache_read_usd_per_mtok IS NOT NULL)
          AND (cache_creation_input_tokens = 0 OR cache_write_usd_per_mtok IS NOT NULL)
          THEN 1 ELSE 0 END AS priced
    FROM normalized
), valued AS (
    SELECT *, CASE WHEN priced = 0 THEN 0.0 ELSE
        (input_tokens * COALESCE(input_usd_per_mtok, 0)
         + output_tokens * COALESCE(output_usd_per_mtok, 0)
         + cache_read_input_tokens * COALESCE(cache_read_usd_per_mtok, 0)
         + cache_creation_input_tokens * COALESCE(cache_write_usd_per_mtok, 0))
            / 1000000.0 END AS usd
    FROM covered
)
SELECT day,
    ROUND(SUM(usd), 2) AS usd,
    SUM(total_tokens) AS tokens,
    COUNT(*) AS sessions,
    COUNT(DISTINCT telemetry_id) AS users,
    SUM(CASE WHEN priced = 0 THEN total_tokens ELSE 0 END) AS unpriced_tokens,
    ROUND(100.0 * SUM(CASE WHEN priced = 1 THEN total_tokens ELSE 0 END)
        / NULLIF(SUM(total_tokens), 0), 1) AS priced_pct
FROM valued
GROUP BY day
ORDER BY day;
