-- Per-day, per-model token usage rollup fed by `usage_report` events.
--
-- `usage_report` is emitted once per provider response with the model that
-- actually served it and the caller's own session id, so it does not depend on
-- sessions ending cleanly and is not skewed by the process-global telemetry
-- session in multi-agent servers. The worker upserts this compact rollup
-- instead of storing a raw events row per response, so volume does not grow
-- the database and spend queries read a few thousand rows instead of millions.
--
-- Dimensions are coarse and content-free: date, source, provider, model,
-- build channel and CI flag. No telemetry_id is stored here.

CREATE TABLE IF NOT EXISTS daily_model_usage (
    usage_date TEXT NOT NULL,
    source TEXT NOT NULL,
    provider TEXT NOT NULL,
    model TEXT NOT NULL,
    build_channel TEXT NOT NULL DEFAULT '',
    is_ci INTEGER NOT NULL DEFAULT 0,
    responses INTEGER NOT NULL DEFAULT 0,
    input_tokens INTEGER NOT NULL DEFAULT 0,
    output_tokens INTEGER NOT NULL DEFAULT 0,
    cache_read_input_tokens INTEGER NOT NULL DEFAULT 0,
    cache_creation_input_tokens INTEGER NOT NULL DEFAULT 0,
    total_tokens INTEGER NOT NULL DEFAULT 0,
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (usage_date, source, provider, model, build_channel, is_ci)
);

CREATE INDEX IF NOT EXISTS idx_daily_model_usage_date
    ON daily_model_usage(usage_date);
