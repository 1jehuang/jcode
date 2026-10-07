-- Cut D1 billed row writes. Every events INSERT (and every retention DELETE)
-- also writes one row into each index on the table, and D1 bills each of those
-- as a row write. In Sept-Oct 2026 the events table carried 13 indexes, so a
-- single telemetry event cost ~14 billed row writes and the database billed
-- ~93M row writes/month (50M included) for ~155K events/day.
--
-- These indexes are not used by any query in the dashboard worker, the
-- report SQL files in this directory, or the worker itself:
--   * idx_events_event is a strict prefix of idx_events_event_created_telemetry.
--   * session_id, step, feedback_rating, session_stop_reason, agent_role,
--     account_id and (event, tier, created_at) are only ever selected or
--     filtered together with an event = ... predicate, which the composite
--     (event, created_at, telemetry_id) index already serves.
--
-- Kept: idx_events_event_id (UNIQUE, dedupes client retries),
-- idx_events_created_at, idx_events_telemetry_id,
-- idx_events_event_created_telemetry, idx_events_event_telemetry_created.
DROP INDEX IF EXISTS idx_events_event;
DROP INDEX IF EXISTS idx_events_session_id;
DROP INDEX IF EXISTS idx_events_step;
DROP INDEX IF EXISTS idx_events_feedback_rating;
DROP INDEX IF EXISTS idx_events_session_stop_reason;
DROP INDEX IF EXISTS idx_events_agent_role;
DROP INDEX IF EXISTS idx_events_account_id;
DROP INDEX IF EXISTS idx_events_event_tier_created;
-- Older migrations (0005) created these; already absent in production, but
-- keep fresh databases built from the migration chain consistent.
DROP INDEX IF EXISTS idx_events_turn_index;
DROP INDEX IF EXISTS idx_events_session_start_hour_utc;
DROP INDEX IF EXISTS idx_events_multi_sessioned;
