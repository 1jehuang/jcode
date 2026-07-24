-- Sponsored-discovery funnel: browse -> select per sponsor.
-- Usage:
--   wrangler d1 execute jcode-telemetry --remote --file=discovery.sql
--
-- A `select` means the agent fetched a sponsor's setup instructions, which is
-- the closest proxy we have for an intent-to-set-up. It is NOT a completed
-- signup: nothing after this point is observable from the CLI, and for
-- cookie-only sponsors the referral is dropped there (see
-- docs/ATTRIBUTION_BENCHMARK.md, cli_flow_attributable).
-- benchmark_run=1 rows are our own live benchmarks; exclude them from demand.

-- 1. Setup fetches per sponsor, real traffic only.
SELECT 'selects_by_sponsor' AS report,
       d.selected_tool AS sponsor,
       COUNT(*) AS selects,
       COUNT(DISTINCT e.telemetry_id) AS users,
       MIN(SUBSTR(e.created_at, 1, 10)) AS first_day,
       MAX(SUBSTR(e.created_at, 1, 10)) AS last_day
FROM discovery_details d
JOIN events e ON e.event_id = d.event_id
WHERE d.phase = 'select' AND d.outcome = 'success' AND d.benchmark_run = 0
GROUP BY d.selected_tool
ORDER BY selects DESC;

-- 2. Category funnel: how many browses convert to a setup fetch.
SELECT 'category_funnel' AS report,
       d.category,
       SUM(d.phase = 'browse') AS browses,
       SUM(d.phase = 'select') AS selects,
       COUNT(DISTINCT CASE WHEN d.phase = 'browse' THEN e.telemetry_id END) AS browse_users,
       COUNT(DISTINCT CASE WHEN d.phase = 'select' THEN e.telemetry_id END) AS select_users
FROM discovery_details d
JOIN events e ON e.event_id = d.event_id
WHERE d.outcome = 'success' AND d.benchmark_run = 0
GROUP BY d.category
ORDER BY browses DESC;

-- 3. Failure reasons, so a broken catalog never looks like absent demand.
SELECT 'failures' AS report, d.phase, d.failure_reason, COUNT(*) AS n
FROM discovery_details d
WHERE d.outcome = 'failure' AND d.benchmark_run = 0
GROUP BY d.phase, d.failure_reason
ORDER BY n DESC;
