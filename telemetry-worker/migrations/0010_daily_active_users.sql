-- Materialized daily active-user rollup for cheap, accurate DAU/WAU/MAU queries.
-- One row per UTC day and anonymous telemetry_id. Counters are incremented only
-- after the raw event row is inserted, so duplicate event_id retries are ignored.

CREATE TABLE IF NOT EXISTS daily_active_users (
    activity_date TEXT NOT NULL,
    telemetry_id TEXT NOT NULL,
    first_seen_at TEXT DEFAULT (datetime('now')),
    last_seen_at TEXT DEFAULT (datetime('now')),
    raw_active INTEGER DEFAULT 0,
    meaningful_active INTEGER DEFAULT 0,
    release_active INTEGER DEFAULT 0,
    meaningful_release_active INTEGER DEFAULT 0,
    session_start_count INTEGER DEFAULT 0,
    turn_end_count INTEGER DEFAULT 0,
    session_end_count INTEGER DEFAULT 0,
    session_crash_count INTEGER DEFAULT 0,
    last_build_channel TEXT,
    PRIMARY KEY (activity_date, telemetry_id)
);

CREATE INDEX IF NOT EXISTS idx_daily_active_date
    ON daily_active_users(activity_date);

CREATE INDEX IF NOT EXISTS idx_daily_active_date_release
    ON daily_active_users(activity_date, release_active, meaningful_release_active);
