-- Expand early telemetry schema to match the current worker/client payload.
-- Safe to run against an already-migrated database: duplicate-column errors
-- indicate the column is already present.

ALTER TABLE events ADD COLUMN had_user_prompt INTEGER DEFAULT 0;
ALTER TABLE events ADD COLUMN had_assistant_response INTEGER DEFAULT 0;
ALTER TABLE events ADD COLUMN assistant_responses INTEGER DEFAULT 0;
ALTER TABLE events ADD COLUMN tool_calls INTEGER DEFAULT 0;
ALTER TABLE events ADD COLUMN tool_failures INTEGER DEFAULT 0;
ALTER TABLE events ADD COLUMN resumed_session INTEGER DEFAULT 0;
ALTER TABLE events ADD COLUMN end_reason TEXT;

