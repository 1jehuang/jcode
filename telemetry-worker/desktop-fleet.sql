-- Jcode Desktop fleet update reach dashboard.
-- Usage:
--   npm run desktop-fleet
--   wrangler d1 execute jcode-telemetry --remote --file=desktop-fleet.sql
--
-- Active install = telemetry_id with a desktop_active row in the last 7 days
-- (is_ci = 0), represented by its latest row. "Newest" version is the most
-- recently first-seen Desktop version among real (non-CI) installs.

-- 1. Share of active installs on the newest version, overall and by os.
WITH latest AS (
    SELECT telemetry_id, version, os,
           ROW_NUMBER() OVER (PARTITION BY telemetry_id ORDER BY created_at DESC, id DESC) AS rn
    FROM events
    WHERE event = 'desktop_active' AND is_ci = 0
      AND created_at >= datetime('now', '-7 days')
),
active AS (SELECT version, os FROM latest WHERE rn = 1),
newest AS (
    SELECT version FROM events
    WHERE event = 'desktop_active' AND is_ci = 0
    GROUP BY version ORDER BY MIN(created_at) DESC LIMIT 1
),
scoped AS (
    SELECT os AS scope, version FROM active
    UNION ALL
    SELECT 'all' AS scope, version FROM active
)
SELECT 'on_newest' AS report,
       scope AS os,
       (SELECT version FROM newest) AS newest_version,
       COUNT(*) AS active_installs,
       SUM(version = (SELECT version FROM newest)) AS on_newest,
       ROUND(100.0 * SUM(version = (SELECT version FROM newest)) / COUNT(*), 1) AS pct_on_newest
FROM scoped
GROUP BY scope
ORDER BY scope = 'all' DESC, active_installs DESC;

-- 2. Version distribution of active installs (last 7 days, latest row per id).
WITH latest AS (
    SELECT telemetry_id, version, os,
           ROW_NUMBER() OVER (PARTITION BY telemetry_id ORDER BY created_at DESC, id DESC) AS rn
    FROM events
    WHERE event = 'desktop_active' AND is_ci = 0
      AND created_at >= datetime('now', '-7 days')
)
SELECT 'version_distribution' AS report,
       version,
       COUNT(*) AS active_installs,
       ROUND(100.0 * COUNT(*) / SUM(COUNT(*)) OVER (), 1) AS pct
FROM latest
WHERE rn = 1
GROUP BY version
ORDER BY active_installs DESC;

-- 3. desktop_update outcomes by install_kind / failure_stage, last 14 days.
SELECT 'update_outcomes' AS report,
       d.install_kind,
       d.update_outcome,
       COALESCE(d.update_failure_stage, '-') AS failure_stage,
       e.os,
       COUNT(*) AS attempts,
       COUNT(DISTINCT e.telemetry_id) AS installs
FROM events e
JOIN desktop_update_details d ON d.event_id = e.event_id
WHERE e.event = 'desktop_update' AND e.is_ci = 0
  AND e.created_at >= datetime('now', '-14 days')
GROUP BY d.install_kind, d.update_outcome, failure_stage, e.os
ORDER BY attempts DESC;

-- 4. Rollout speed: for each version, hours from its first desktop_active to
--    the first UTC day where >= 50% of that day's active installs ran it, then
--    the median across versions that reached 50%. Daily granularity.
WITH firsts AS (
    SELECT version, MIN(created_at) AS first_seen
    FROM events
    WHERE event = 'desktop_active' AND is_ci = 0
    GROUP BY version
),
daily AS (
    SELECT date(created_at) AS day, telemetry_id, MAX(version) AS version
    FROM events
    WHERE event = 'desktop_active' AND is_ci = 0
    GROUP BY day, telemetry_id
),
day_share AS (
    SELECT f.version, d.day,
           1.0 * SUM(d.version = f.version) / COUNT(*) AS share
    FROM firsts f
    JOIN daily d ON d.day >= date(f.first_seen)
    GROUP BY f.version, d.day
),
reached AS (
    SELECT f.version,
           (julianday(MIN(s.day) || ' 23:59:59') - julianday(f.first_seen)) * 24.0 AS hours_to_half
    FROM firsts f
    JOIN day_share s ON s.version = f.version AND s.share >= 0.5
    GROUP BY f.version
),
ordered AS (
    SELECT hours_to_half,
           ROW_NUMBER() OVER (ORDER BY hours_to_half) AS rn,
           COUNT(*) OVER () AS n
    FROM reached
)
SELECT 'median_hours_to_half_fleet' AS report,
       (SELECT COUNT(*) FROM firsts) AS versions_seen,
       (SELECT COUNT(*) FROM reached) AS versions_reaching_half,
       ROUND(AVG(hours_to_half), 1) AS median_hours
FROM ordered
WHERE rn IN ((n + 1) / 2, (n + 2) / 2);
