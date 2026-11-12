-- Metadata only. The consented transcript body is stored in the private R2
-- TRANSCRIPTS bucket and removed by its lifecycle policy.
CREATE TABLE IF NOT EXISTS transcript_uploads (
    upload_id TEXT PRIMARY KEY,
    telemetry_id TEXT NOT NULL,
    object_key TEXT NOT NULL UNIQUE,
    consent_version INTEGER NOT NULL,
    schema_version INTEGER NOT NULL,
    version TEXT NOT NULL,
    provider TEXT,
    model TEXT,
    end_reason TEXT NOT NULL,
    message_count INTEGER NOT NULL,
    byte_count INTEGER NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_transcript_uploads_telemetry_id
    ON transcript_uploads(telemetry_id);
CREATE INDEX IF NOT EXISTS idx_transcript_uploads_created_at
    ON transcript_uploads(created_at);
