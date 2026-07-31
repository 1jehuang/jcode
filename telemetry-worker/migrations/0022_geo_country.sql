-- Coarse geographic dimension: 2-letter country code only.
--
-- Motivation: "where are our users?" was unanswerable because the worker never
-- read request.cf. Cloudflare supplies the country at the edge, so no new
-- client-side collection is needed and no IP address is ever stored.
--
-- Privacy posture (see TELEMETRY.md): country only, never IP / city / region /
-- coordinates / timezone. Country lives on the daily rollup rather than the
-- raw events table because the rollup is never pruned and the events table is
-- one column shy of D1's 100-column cap.
--
-- last_country is "last country observed for this telemetry_id on this day",
-- which makes user-by-country a simple COUNT(DISTINCT telemetry_id) per day.

ALTER TABLE daily_active_users ADD COLUMN last_country TEXT;

CREATE INDEX IF NOT EXISTS idx_daily_active_date_country
    ON daily_active_users(activity_date, last_country);

-- Durable per-day country rollup for events that never touch the DAU table
-- (install, upgrade, auth_success, web_pageview, ...). Counts only, so it
-- cannot be used to profile an individual and stays tiny under the size cap.
CREATE TABLE IF NOT EXISTS country_daily (
    activity_date TEXT NOT NULL,
    country TEXT NOT NULL,
    event TEXT NOT NULL,
    is_ci INTEGER NOT NULL DEFAULT 0,
    event_count INTEGER NOT NULL DEFAULT 0,
    first_seen_at TEXT DEFAULT (datetime('now')),
    last_seen_at TEXT DEFAULT (datetime('now')),
    PRIMARY KEY (activity_date, country, event, is_ci)
);

CREATE INDEX IF NOT EXISTS idx_country_daily_date ON country_daily(activity_date);
CREATE INDEX IF NOT EXISTS idx_country_daily_country ON country_daily(country);
