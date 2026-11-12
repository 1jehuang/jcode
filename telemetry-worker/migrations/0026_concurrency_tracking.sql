-- Additive only. No backfill or rewrite of legacy events/session_details.
-- Separate table keeps the nearly-full events table below D1's column cap.
CREATE TABLE IF NOT EXISTS concurrency_details (
    event_id TEXT PRIMARY KEY,
    concurrency_tracking_version INTEGER,
    concurrency_tracking_scope TEXT,
    concurrency_tracking_available INTEGER,
    concurrency_session_id TEXT,
    phase TEXT,
    agent_role TEXT,
    runtime_is_ci INTEGER,
    active_sessions_at_start INTEGER,
    other_active_sessions_at_start INTEGER,
    max_concurrent_sessions INTEGER,
    root_sessions_at_start INTEGER,
    child_sessions_at_start INTEGER,
    max_concurrent_root_sessions INTEGER,
    max_concurrent_child_sessions INTEGER,
    multi_sessioned INTEGER,
    quality TEXT NOT NULL CHECK (quality IN (
        'trusted', 'missing_version', 'legacy_version', 'unsupported_version',
        'legacy_scope', 'missing_fields', 'invalid', 'unavailable'
    )),
    quality_reason TEXT NOT NULL,
    raw_json TEXT NOT NULL,
    FOREIGN KEY (event_id) REFERENCES events(event_id) ON DELETE CASCADE
);

-- Coverage includes unclassified history and missing detail writes. Do not
-- infer v2 from app version, schema_version, timestamps or plausible counts.
CREATE VIEW IF NOT EXISTS concurrency_event_quality AS
SELECT e.id AS event_row_id, e.event_id, e.telemetry_id, e.session_id,
       e.event, e.created_at, e.version, e.build_channel, e.is_ci,
       c.concurrency_tracking_version, c.concurrency_tracking_scope,
       c.concurrency_tracking_available, c.concurrency_session_id,
       c.phase, c.agent_role, c.runtime_is_ci,
       c.active_sessions_at_start, c.other_active_sessions_at_start,
       c.max_concurrent_sessions, c.multi_sessioned,
       c.root_sessions_at_start, c.child_sessions_at_start,
       c.max_concurrent_root_sessions, c.max_concurrent_child_sessions,
       CASE
         WHEN c.event_id IS NULL AND e.event != 'session_concurrency' THEN 'legacy_unclassified'
         WHEN c.event_id IS NULL THEN 'missing_detail'
         WHEN c.quality != 'trusted' THEN c.quality
         -- Defense in depth: a manually changed quality label must not turn
         -- missing, negative, rounded or inconsistent metrics into clean data.
         WHEN e.event = 'session_concurrency'
          AND c.concurrency_tracking_version = 2
          AND c.concurrency_tracking_scope = 'runtime_agent_sessions'
          AND c.concurrency_tracking_available = 1
          AND c.phase IN ('start', 'end') AND c.agent_role IN ('root', 'child')
          AND length(trim(c.concurrency_session_id)) > 0
          AND length(trim(e.session_id)) > 0
          AND c.runtime_is_ci IN (0, 1) AND e.is_ci = c.runtime_is_ci
          AND typeof(c.active_sessions_at_start) = 'integer'
          AND c.active_sessions_at_start BETWEEN 1 AND 9007199254740991
          AND typeof(c.other_active_sessions_at_start) = 'integer'
          AND c.other_active_sessions_at_start = c.active_sessions_at_start - 1
          AND typeof(c.max_concurrent_sessions) = 'integer'
          AND c.max_concurrent_sessions BETWEEN c.active_sessions_at_start AND 9007199254740991
          AND c.multi_sessioned = (c.max_concurrent_sessions > 1)
          AND typeof(c.root_sessions_at_start) = 'integer'
          AND c.root_sessions_at_start BETWEEN 0 AND c.active_sessions_at_start
          AND typeof(c.child_sessions_at_start) = 'integer'
          AND c.child_sessions_at_start = c.active_sessions_at_start - c.root_sessions_at_start
          AND (c.agent_role != 'root' OR c.root_sessions_at_start >= 1)
          AND (c.agent_role != 'child' OR c.child_sessions_at_start >= 1)
          AND typeof(c.max_concurrent_root_sessions) = 'integer'
          AND c.max_concurrent_root_sessions BETWEEN c.root_sessions_at_start AND c.max_concurrent_sessions
          AND typeof(c.max_concurrent_child_sessions) = 'integer'
          AND c.max_concurrent_child_sessions BETWEEN c.child_sessions_at_start AND c.max_concurrent_sessions
          AND c.max_concurrent_root_sessions >= c.max_concurrent_sessions - c.max_concurrent_child_sessions
          AND (c.phase != 'start' OR (
            c.max_concurrent_sessions = c.active_sessions_at_start
            AND c.max_concurrent_root_sessions = c.root_sessions_at_start
            AND c.max_concurrent_child_sessions = c.child_sessions_at_start
          )) THEN 'trusted'
         ELSE 'invalid_storage'
       END AS quality,
       c.quality_reason
FROM events e LEFT JOIN concurrency_details c ON c.event_id = e.event_id
WHERE e.event IN ('session_concurrency', 'session_start', 'session_end', 'session_crash');

-- Includes both starts and ends. Start peaks are lower-bound observations,
-- not completed-session peaks. Runtime CI is excluded, not CI-built releases.
CREATE VIEW IF NOT EXISTS trusted_concurrency_events AS
SELECT * FROM concurrency_event_quality
WHERE event = 'session_concurrency' AND quality = 'trusted'
  AND is_ci = 0 AND runtime_is_ci = 0;
