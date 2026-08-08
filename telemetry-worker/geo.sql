-- Where are our users? Coarse country breakdown (migration 0022).
-- Usage:
--   wrangler d1 execute jcode-telemetry --remote --file=geo.sql
--
-- Data source: Cloudflare resolves the 2-letter country at the edge
-- (request.cf.country). No IP address, city, region, or coordinates is ever
-- collected or stored. Rows only exist from the day migration 0022 shipped, so
-- historical users have last_country = NULL until they are next active.
--
-- Caveats: telemetry_id is per-machine; VPN/proxy users report the exit
-- country; opt-outs are never counted.

-- 1) Users by country, all time (distinct non-CI machines ever seen there).
SELECT
    'users_by_country_all_time' AS report,
    COALESCE(last_country, 'unknown') AS country,
    COUNT(DISTINCT telemetry_id) AS users
FROM daily_active_users
WHERE last_is_ci = 0
GROUP BY country
ORDER BY users DESC, country
LIMIT 50;

-- 2) Users by country, trailing 30 days (release channel, meaningful work).
SELECT
    'active_users_by_country_30d' AS report,
    COALESCE(last_country, 'unknown') AS country,
    COUNT(DISTINCT telemetry_id) AS users
FROM daily_active_users
WHERE activity_date >= date('now', '-30 days')
  AND last_is_ci = 0
  AND meaningful_release_active > 0
GROUP BY country
ORDER BY users DESC, country
LIMIT 50;

-- 3) Installs by country, trailing 90 days (aggregate rollup, prune-proof).
SELECT
    'installs_by_country_90d' AS report,
    country,
    SUM(event_count) AS installs
FROM country_daily
WHERE event = 'install'
  AND is_ci = 0
  AND activity_date >= date('now', '-90 days')
GROUP BY country
ORDER BY installs DESC, country
LIMIT 50;

-- 4) Website pageviews by country, trailing 30 days.
SELECT
    'web_pageviews_by_country_30d' AS report,
    country,
    SUM(event_count) AS pageviews
FROM country_daily
WHERE event = 'web_pageview'
  AND activity_date >= date('now', '-30 days')
GROUP BY country
ORDER BY pageviews DESC, country
LIMIT 50;

-- 5) Daily trend for the top countries (last 14 days, all event families).
SELECT
    'country_daily_trend_14d' AS report,
    activity_date,
    country,
    SUM(event_count) AS events
FROM country_daily
WHERE activity_date >= date('now', '-14 days')
  AND is_ci = 0
GROUP BY activity_date, country
ORDER BY activity_date DESC, events DESC
LIMIT 200;
