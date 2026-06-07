-- Add CI / dev-environment attribution to the daily active-user rollup so the
-- headline DAU/WAU/MAU metrics can exclude automated CI traffic cheaply.
--
-- Ephemeral CI runners mint a fresh telemetry_id every job, so unfiltered they
-- look like brand-new users (and brand-new installs), inflating active-user and
-- install counts and depressing retention. We keep the raw rows for transparency
-- and crash visibility, but tag them so product dashboards can filter is_ci = 0.

ALTER TABLE daily_active_users ADD COLUMN ci_active INTEGER DEFAULT 0;
ALTER TABLE daily_active_users ADD COLUMN last_is_ci INTEGER DEFAULT 0;

-- Index to make "real (non-CI) release users today" cheap.
CREATE INDEX IF NOT EXISTS idx_daily_active_date_ci
    ON daily_active_users(activity_date, last_is_ci, meaningful_release_active);

-- Best-effort backfill from canonical raw events for the last 35 days. A day is
-- marked CI if any of that id's lifecycle events that day were emitted under CI.
UPDATE daily_active_users
SET ci_active = 1, last_is_ci = 1
WHERE (activity_date, telemetry_id) IN (
    SELECT date(created_at), telemetry_id
    FROM events INDEXED BY idx_events_event_created_telemetry
    WHERE event IN ('session_start', 'turn_end', 'session_end', 'session_crash')
      AND created_at > datetime('now', '-35 days')
      AND is_ci = 1
    GROUP BY date(created_at), telemetry_id
);
