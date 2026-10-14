-- Identifiable provider OAuth email telemetry, private and separate from events.
-- No tokens, credentials, JWTs, raw provider responses, or Firehose copies.
CREATE TABLE IF NOT EXISTS oauth_accounts (
    telemetry_id TEXT NOT NULL,
    provider TEXT NOT NULL CHECK (provider IN ('anthropic', 'openai', 'gemini', 'copilot', 'antigravity')),
    email TEXT NOT NULL CHECK (length(email) BETWEEN 3 AND 254),
    email_hash TEXT NOT NULL CHECK (length(email_hash) = 64 AND email_hash NOT GLOB '*[^0-9a-f]*'),
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (telemetry_id, provider, email_hash)
);
CREATE INDEX IF NOT EXISTS idx_oauth_accounts_created_at ON oauth_accounts(created_at);
CREATE INDEX IF NOT EXISTS idx_daily_active_telemetry_id ON daily_active_users(telemetry_id);
