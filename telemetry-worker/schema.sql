-- Schema for jcode telemetry D1 database

CREATE TABLE IF NOT EXISTS events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    telemetry_id TEXT NOT NULL,
    event TEXT NOT NULL,
    version TEXT NOT NULL,
    os TEXT NOT NULL,
    arch TEXT NOT NULL,
    provider_start TEXT,
    provider_end TEXT,
    model_start TEXT,
    model_end TEXT,
    provider_switches INTEGER DEFAULT 0,
    model_switches INTEGER DEFAULT 0,
    duration_mins INTEGER,
    turns INTEGER,
    had_user_prompt INTEGER DEFAULT 0,
    had_assistant_response INTEGER DEFAULT 0,
    assistant_responses INTEGER DEFAULT 0,
    tool_calls INTEGER DEFAULT 0,
    tool_failures INTEGER DEFAULT 0,
    transport_https INTEGER DEFAULT 0,
    transport_persistent_ws_fresh INTEGER DEFAULT 0,
    transport_persistent_ws_reuse INTEGER DEFAULT 0,
    transport_cli_subprocess INTEGER DEFAULT 0,
    transport_native_http2 INTEGER DEFAULT 0,
    transport_other INTEGER DEFAULT 0,
    resumed_session INTEGER DEFAULT 0,
    end_reason TEXT,
    error_provider_timeout INTEGER DEFAULT 0,
    error_auth_failed INTEGER DEFAULT 0,
    error_tool_error INTEGER DEFAULT 0,
    error_mcp_error INTEGER DEFAULT 0,
    error_rate_limited INTEGER DEFAULT 0,
    created_at TEXT DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_events_telemetry_id ON events(telemetry_id);
CREATE INDEX IF NOT EXISTS idx_events_event ON events(event);
CREATE INDEX IF NOT EXISTS idx_events_created_at ON events(created_at);
