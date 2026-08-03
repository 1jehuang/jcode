-- Privacy-safe per-session todo lifecycle and confidence aggregates.
--
-- The parent events.telemetry_id value for `todo_session` events is the same
-- fresh per-session correlation UUID stored below. The client deliberately does
-- not send its persistent telemetry ID on this event, preventing joins to an
-- install or account while allowing discovery-request joins within one session.

CREATE TABLE IF NOT EXISTS todo_session_details (
    event_id TEXT PRIMARY KEY,
    correlation_id TEXT NOT NULL UNIQUE,
    session_end_reason TEXT NOT NULL,
    todos_created INTEGER NOT NULL DEFAULT 0,
    todos_completed INTEGER NOT NULL DEFAULT 0,
    todos_abandoned INTEGER NOT NULL DEFAULT 0,
    todo_updates INTEGER NOT NULL DEFAULT 0,
    groups_completed INTEGER NOT NULL DEFAULT 0,
    groups_total INTEGER NOT NULL DEFAULT 0,
    max_todo_list_size INTEGER NOT NULL DEFAULT 0,
    confidence_min INTEGER,
    confidence_mean REAL,
    confidence_count INTEGER NOT NULL DEFAULT 0,
    completion_confidence_min INTEGER,
    completion_confidence_mean REAL,
    completion_confidence_count INTEGER NOT NULL DEFAULT 0,
    understands_user_intent_min INTEGER,
    understands_user_intent_mean REAL,
    understands_user_intent_count INTEGER NOT NULL DEFAULT 0,
    closed_feedback_loop_min INTEGER,
    closed_feedback_loop_mean REAL,
    closed_feedback_loop_count INTEGER NOT NULL DEFAULT 0,
    end_to_end_ownership_min INTEGER,
    end_to_end_ownership_mean REAL,
    end_to_end_ownership_count INTEGER NOT NULL DEFAULT 0,
    FOREIGN KEY (event_id) REFERENCES events(event_id)
);

CREATE INDEX IF NOT EXISTS idx_todo_session_completed
    ON todo_session_details(todos_completed);
CREATE INDEX IF NOT EXISTS idx_todo_session_groups_completed
    ON todo_session_details(groups_completed);
