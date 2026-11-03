-- Authoritative contract-v2 per-response usage rollup (migration 0031), CI excluded.
-- Pre-v2 daily_model_usage has mixed native cache semantics and is not combined.
-- usage_report normalizes input to UNCACHED/disjoint tokens before ingestion.
-- Never apply model_prices.input_includes_cache_read here: that flag is for
-- legacy session_end counters only. Never add legacy token-value dashboards.
-- Sources agent/compaction/sidecar remain separate, not duplicate session totals.
-- Dollars are list-price equivalents, not revenue, bills or COGS. Any missing
-- rate for a used bucket makes the entire row unpriced. No guessed cache rates.
-- Usage: npm run model-usage
WITH normalized AS (
    SELECT u.*, p.price_kind,
        p.input_usd_per_mtok, p.output_usd_per_mtok,
        p.cache_read_usd_per_mtok, p.cache_write_usd_per_mtok,
        u.input_tokens + u.output_tokens + u.cache_read_input_tokens
            + u.cache_creation_input_tokens AS gross_tokens,
        CASE WHEN p.price_kind IN ('catalog', 'free')
          AND (u.input_tokens = 0 OR p.input_usd_per_mtok IS NOT NULL)
          AND (u.output_tokens = 0 OR p.output_usd_per_mtok IS NOT NULL)
          AND (u.cache_read_input_tokens = 0 OR p.cache_read_usd_per_mtok IS NOT NULL)
          AND (u.cache_creation_input_tokens = 0 OR p.cache_write_usd_per_mtok IS NOT NULL)
          THEN 1 ELSE 0 END AS priced
    FROM daily_model_usage_v2 u
    LEFT JOIN model_prices p ON p.model = u.model
    WHERE u.is_ci = 0
), valued AS (
    SELECT *, CASE WHEN priced = 0 THEN 0.0 ELSE
        (input_tokens * COALESCE(input_usd_per_mtok, 0)
         + output_tokens * COALESCE(output_usd_per_mtok, 0)
         + cache_read_input_tokens * COALESCE(cache_read_usd_per_mtok, 0)
         + cache_creation_input_tokens * COALESCE(cache_write_usd_per_mtok, 0))
            / 1000000.0 END AS usd
    FROM normalized
)
SELECT
    substr(usage_date, 1, 7) AS month,
    source,
    SUM(responses) AS responses,
    SUM(gross_tokens) AS tokens,
    SUM(CASE WHEN priced = 0 THEN gross_tokens ELSE 0 END) AS unpriced_tokens,
    ROUND(SUM(gross_tokens) / 1e9, 2) AS gross_btok,
    ROUND(SUM(CASE WHEN priced = 0 THEN gross_tokens ELSE 0 END) / 1e9, 2) AS unpriced_btok,
    ROUND(SUM(usd), 2) AS list_usd
FROM valued
GROUP BY month, source
ORDER BY month DESC, list_usd DESC;
