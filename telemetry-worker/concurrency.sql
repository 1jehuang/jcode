-- Reliable concurrency, last 30 days by SERVER RECEIPT time (UTC).
-- Run: npm run concurrency [-- --json]. Requires migration 0026.
-- Installations are not people. A runtime incarnation is one Agent lifetime,
-- including children and idle sessions, within a shared JCODE_HOME on one host.
-- End peaks may cover time before this window. No historical exact repair.
WITH recent AS (
  SELECT * FROM concurrency_event_quality
  WHERE created_at >= datetime('now', '-30 days')
), clean AS (
  SELECT * FROM trusted_concurrency_events
  WHERE created_at >= datetime('now', '-30 days')
), incarnations AS (
  SELECT telemetry_id, concurrency_session_id,
         MAX(phase = 'start') AS has_start, MAX(phase = 'end') AS has_end,
         MAX(CASE WHEN phase = 'end' THEN max_concurrent_sessions END) AS peak,
         MAX(CASE WHEN phase = 'end' THEN max_concurrent_root_sessions END) AS root_peak,
         MAX(CASE WHEN phase = 'end' THEN max_concurrent_child_sessions END) AS child_peak
  FROM clean GROUP BY telemetry_id, concurrency_session_id
), completed AS (
  SELECT * FROM incarnations WHERE has_end = 1
), installs AS (
  SELECT telemetry_id, MAX(peak) AS peak, MAX(root_peak) AS root_peak,
         MAX(child_peak) AS child_peak
  FROM completed GROUP BY telemetry_id
), populations AS MATERIALIZED (
  SELECT 'session_end_peak' AS panel, telemetry_id, peak FROM completed
  UNION ALL SELECT 'installation_observed_peak', telemetry_id, peak FROM installs
), histogram AS (
  SELECT panel, telemetry_id,
    CASE WHEN peak = 1 THEN '01: 1' WHEN peak = 2 THEN '02: 2'
         WHEN peak = 3 THEN '03: 3' WHEN peak = 4 THEN '04: 4'
         WHEN peak = 5 THEN '05: 5' WHEN peak <= 10 THEN '06: 6-10'
         WHEN peak <= 20 THEN '07: 11-20' WHEN peak <= 50 THEN '08: 21-50'
         ELSE '09: 51+' END AS bucket, peak
  FROM populations
-- D1 has a much smaller compound SELECT budget than desktop SQLite. Keep
-- panel groups behind optimization fences so flattening cannot expand UNIONs.
), coverage_panels AS MATERIALIZED (
  SELECT 'coverage_summary' AS panel, 'dedicated_non_ci_events' AS bucket,
    COUNT(*) AS observations, COUNT(DISTINCT telemetry_id) AS installations,
    ROUND(100.0 * SUM(quality = 'trusted') / NULLIF(COUNT(*), 0), 2) AS trusted_pct,
    NULL AS avg_peak, NULL AS max_peak,
    'Percent trusted among received non-CI dedicated events, not all installed clients' AS notes
  FROM recent WHERE event = 'session_concurrency' AND is_ci = 0
  UNION ALL
  SELECT 'coverage_by_source', event || ':' || quality || ':' ||
    CASE WHEN is_ci = 1 THEN 'ci' WHEN is_ci = 0 THEN 'non_ci' ELSE 'ci_unknown' END,
    COUNT(*), COUNT(DISTINCT telemetry_id), NULL, NULL, NULL,
    'Raw event counts. Legacy and missing metrics never enter peak estimates'
  FROM recent GROUP BY event, quality, is_ci
  UNION ALL
  SELECT 'quality_reasons', quality || ':' || COALESCE(quality_reason, 'no_detail'),
    COUNT(*), COUNT(DISTINCT telemetry_id), NULL, NULL, NULL,
    'Dedicated events only, including CI. Missing keys differ from explicit null/zero'
  FROM recent WHERE event = 'session_concurrency' AND quality != 'trusted'
  GROUP BY quality, quality_reason
), completion_panels AS MATERIALIZED (
  SELECT 'completion_coverage', 'observed_runtime_incarnations', COUNT(*),
    COUNT(DISTINCT telemetry_id), NULL, NULL, NULL,
    'Trusted start or end received in window, deduplicated by installation + runtime UUID'
  FROM incarnations
  UNION ALL
  SELECT 'completion_coverage', 'start_without_end', COUNT(*),
    COUNT(DISTINCT telemetry_id), NULL, NULL, NULL,
    'May still be open, crash, lose end telemetry, or end outside the window. Excluded from peaks'
  FROM incarnations WHERE has_start = 1 AND has_end = 0
  UNION ALL
  SELECT 'completion_coverage', 'end_without_start', COUNT(*),
    COUNT(DISTINCT telemetry_id), NULL, NULL, NULL,
    'Valid end included. Start may be lost or outside the window'
  FROM incarnations WHERE has_start = 0 AND has_end = 1
), peak_panels AS MATERIALIZED (
  SELECT 'peak_summary', 'runtime_sessions_with_trusted_end', COUNT(*),
    COUNT(DISTINCT telemetry_id), NULL, ROUND(AVG(peak), 2), MAX(peak),
    'Session-weighted observed peaks. Includes root and child Agent lifetimes, not turns'
  FROM completed
  UNION ALL
  SELECT 'peak_summary', 'installations_with_trusted_end', COUNT(*), COUNT(*),
    NULL, ROUND(AVG(peak), 2), MAX(peak),
    'Installation-weighted highest observed end peak. Installations are NOT people'
  FROM installs
  UNION ALL
  SELECT 'peak_summary', 'root_observed_peak_per_installation', COUNT(*), COUNT(*),
    NULL, ROUND(AVG(root_peak), 2), MAX(root_peak),
    'Parentless/root Agent counts. Root and child maxima occur independently, do not sum'
  FROM installs
  UNION ALL
  SELECT 'peak_summary', 'child_observed_peak_per_installation', COUNT(*), COUNT(*),
    NULL, ROUND(AVG(child_peak), 2), MAX(child_peak),
    'Agent counts with parents, not people. Includes idle children'
  FROM installs
  UNION ALL
  SELECT panel, bucket, COUNT(*), COUNT(DISTINCT telemetry_id), NULL,
    ROUND(AVG(peak), 2), MAX(peak),
    'Non-CI validated v2 end observations only. No inference for legacy or missing clients'
  FROM histogram GROUP BY panel, bucket
), panels AS (
  SELECT * FROM coverage_panels
  UNION ALL SELECT * FROM completion_panels
  UNION ALL SELECT * FROM peak_panels
)
SELECT * FROM panels ORDER BY panel, bucket;
