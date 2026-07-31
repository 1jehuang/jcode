-- Token value dashboard: list-price dollar value of the token flow through jcode.
--
-- Usage:
--   npm run token-value             (wrangler d1 execute ... --file=token-value.sql)
--
-- Requires migration 0023 plus a populated `model_prices` table:
--   npm run migrate:model-prices
--   npm run sync:model-prices
--
-- Accounting notes (these are the parts that are easy to get wrong):
--
-- 1. Source rows are `session_end` only. `turn_end` carries the same token
--    counters but no model label, and session_end's counters are session
--    totals, so summing both would double count.
--
-- 2. `input_includes_cache_read` handles the provider split. OpenAI-compatible
--    APIs report cached tokens as a SUBSET of prompt tokens, Anthropic reports
--    them as a disjoint bucket. Without this correction, OpenAI traffic gets
--    billed for its cached context twice, at ~10x the correct rate.
--
-- 3. These are list/rack rates. Most jcode users are on subscriptions
--    (Claude Max, ChatGPT Pro, Copilot) or free routes, so read the result as
--    "list-price equivalent value of tokens served", not revenue or COGS.
--
-- 4. `unpriced_tokens` is reported next to every total. If coverage drops,
--    re-run the sync script rather than trusting the dollar figure.

WITH priced AS (
    SELECT
        substr(e.created_at, 1, 10) AS day,
        e.created_at,
        e.model_end AS model,
        e.provider_end AS provider,
        p.price_kind,
        -- Correct the input bucket so cached tokens are never priced twice.
        CASE
            WHEN COALESCE(p.input_includes_cache_read, 0) = 1
                THEN MAX(e.input_tokens - e.cache_read_input_tokens, 0)
            ELSE e.input_tokens
        END AS billable_input_tokens,
        e.cache_read_input_tokens AS cache_read_tokens,
        e.cache_creation_input_tokens AS cache_write_tokens,
        e.output_tokens,
        p.input_usd_per_mtok,
        p.output_usd_per_mtok,
        p.cache_read_usd_per_mtok,
        p.cache_write_usd_per_mtok
    FROM events e
    LEFT JOIN model_prices p ON p.model = e.model_end
    WHERE e.event = 'session_end'
      AND e.created_at >= datetime('now', '-30 days')
      AND e.is_ci = 0
), valued AS (
    SELECT
        day,
        created_at,
        model,
        provider,
        price_kind,
        billable_input_tokens,
        cache_read_tokens,
        cache_write_tokens,
        output_tokens,
        (billable_input_tokens + cache_read_tokens + cache_write_tokens + output_tokens)
            AS total_tokens,
        CASE WHEN input_usd_per_mtok IS NULL THEN
            0.0
        ELSE
            billable_input_tokens * input_usd_per_mtok / 1000000.0
            + output_tokens * COALESCE(output_usd_per_mtok, 0) / 1000000.0
            + cache_read_tokens * COALESCE(cache_read_usd_per_mtok, input_usd_per_mtok * 0.1)
                / 1000000.0
            + cache_write_tokens * COALESCE(cache_write_usd_per_mtok, input_usd_per_mtok * 1.25)
                / 1000000.0
        END AS usd
    FROM priced
)

-- Panel 1: daily totals for the last 30 days.
SELECT
    'daily' AS panel,
    day AS bucket,
    ROUND(SUM(usd), 2) AS usd_value,
    SUM(total_tokens) AS tokens,
    SUM(billable_input_tokens) AS input_tokens,
    SUM(cache_read_tokens) AS cache_read_tokens,
    SUM(output_tokens) AS output_tokens,
    SUM(CASE WHEN price_kind IS NULL OR price_kind = 'unpriced' THEN total_tokens ELSE 0 END)
        AS unpriced_tokens,
    ROUND(
        100.0 * SUM(CASE WHEN price_kind = 'catalog' THEN total_tokens ELSE 0 END)
            / NULLIF(SUM(total_tokens), 0),
        1
    ) AS priced_token_pct
FROM valued
GROUP BY day

UNION ALL

-- Panel 2: per-model value over the last 7 days, biggest spenders first.
-- Rolling 168 hours on created_at, matching panel 3's run rate. A
-- `day >= date('now','-7 days')` filter would span 8 calendar days (both the
-- -7 boundary day and today) and inflate the total by a day.
SELECT
    'model_7d' AS panel,
    model || ' (' || COALESCE(provider, '?') || ', ' || COALESCE(price_kind, 'no-row') || ')'
        AS bucket,
    ROUND(SUM(usd), 2) AS usd_value,
    SUM(total_tokens) AS tokens,
    SUM(billable_input_tokens) AS input_tokens,
    SUM(cache_read_tokens) AS cache_read_tokens,
    SUM(output_tokens) AS output_tokens,
    SUM(CASE WHEN price_kind IS NULL OR price_kind = 'unpriced' THEN total_tokens ELSE 0 END)
        AS unpriced_tokens,
    NULL AS priced_token_pct
FROM valued
WHERE created_at >= datetime('now', '-7 days')
GROUP BY model, provider, price_kind

UNION ALL

-- Panel 3: headline rollups. run_rate_usd_per_day is the 7-day mean, which is
-- the number to quote; single days swing a lot with CI-adjacent bursts.
--
-- The 7-day windows filter on `created_at >= datetime('now','-7 days')`, a
-- rolling 168 hours, so dividing the total by 7 gives a true per-day mean. The
-- calendar-day form (`day >= date('now','-7 days')`) covers 8 partial days and
-- overstates the run rate.
--
-- projected_usd_per_month is 30x that mean and assumes flat usage. Volume has
-- been growing, so treat it as a floor rather than a forecast.
SELECT
    'summary' AS panel,
    label AS bucket,
    usd_value,
    tokens,
    NULL AS input_tokens,
    NULL AS cache_read_tokens,
    NULL AS output_tokens,
    unpriced_tokens,
    priced_token_pct
FROM (
    SELECT
        'last_24h' AS label,
        ROUND(SUM(usd), 2) AS usd_value,
        SUM(total_tokens) AS tokens,
        SUM(CASE WHEN price_kind IS NULL OR price_kind = 'unpriced' THEN total_tokens ELSE 0 END)
            AS unpriced_tokens,
        ROUND(
            100.0 * SUM(CASE WHEN price_kind = 'catalog' THEN total_tokens ELSE 0 END)
                / NULLIF(SUM(total_tokens), 0),
            1
        ) AS priced_token_pct
    FROM valued
    -- Rolling 24 hours, not `date('now','-1 days')`, which spans two partial
    -- calendar days and roughly doubles the figure.
    WHERE created_at >= datetime('now', '-24 hours')
    UNION ALL
    SELECT
        'run_rate_usd_per_day_7d',
        ROUND(SUM(usd) / 7.0, 2),
        SUM(total_tokens) / 7,
        SUM(CASE WHEN price_kind IS NULL OR price_kind = 'unpriced' THEN total_tokens ELSE 0 END) / 7,
        ROUND(
            100.0 * SUM(CASE WHEN price_kind = 'catalog' THEN total_tokens ELSE 0 END)
                / NULLIF(SUM(total_tokens), 0),
            1
        )
    FROM valued
    WHERE created_at >= datetime('now', '-7 days')
    UNION ALL
    SELECT
        'projected_usd_per_month_from_7d',
        ROUND(SUM(usd) / 7.0 * 30.0, 2),
        SUM(total_tokens) / 7 * 30,
        NULL,
        NULL
    FROM valued
    WHERE created_at >= datetime('now', '-7 days')
    UNION ALL
    SELECT
        'last_30d_total',
        ROUND(SUM(usd), 2),
        SUM(total_tokens),
        SUM(CASE WHEN price_kind IS NULL OR price_kind = 'unpriced' THEN total_tokens ELSE 0 END),
        ROUND(
            100.0 * SUM(CASE WHEN price_kind = 'catalog' THEN total_tokens ELSE 0 END)
                / NULLIF(SUM(total_tokens), 0),
            1
        )
    FROM valued
)

ORDER BY panel, usd_value DESC;
