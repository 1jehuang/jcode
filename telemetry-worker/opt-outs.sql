-- Explicit in-app telemetry opt-outs. Environment variables, endpoint blocks,
-- and offline users are intentionally unobservable and are not included.
SELECT
    COUNT(DISTINCT telemetry_id) AS in_app_opt_out_users,
    COUNT(DISTINCT CASE WHEN created_at >= datetime('now', '-30 days')
        THEN telemetry_id END) AS in_app_opt_out_users_30d,
    COUNT(DISTINCT CASE WHEN created_at >= datetime('now', '-7 days')
        THEN telemetry_id END) AS in_app_opt_out_users_7d,
    COUNT(DISTINCT CASE WHEN created_at >= datetime('now', '-1 day')
        THEN telemetry_id END) AS in_app_opt_out_users_24h
FROM events
WHERE event = 'telemetry_opt_out'
  AND is_ci = 0;
