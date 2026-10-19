CREATE TABLE IF NOT EXISTS daily_model_usage_v2 (
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

CREATE INDEX IF NOT EXISTS idx_daily_model_usage_v2_date
    ON daily_model_usage_v2(usage_date);

-- Atomic usage receipts and rollup. Apply before rolling out durable client retries.
-- Retention is 8 days by receipt time. Timestamped reports expire after 7 days.
CREATE TABLE IF NOT EXISTS usage_report_ledger (
    receipt_key BLOB NOT NULL,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    usage_contract_version INTEGER NOT NULL DEFAULT 1,
    usage_date TEXT,
    source TEXT,
    provider TEXT,
    model TEXT,
    build_channel TEXT,
    is_ci INTEGER,
    responses INTEGER,
    input_tokens INTEGER,
    output_tokens INTEGER,
    cache_read_input_tokens INTEGER,
    cache_creation_input_tokens INTEGER,
    total_tokens INTEGER,
    PRIMARY KEY (receipt_key)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS idx_usage_report_ledger_created ON usage_report_ledger(created_at);
CREATE TRIGGER IF NOT EXISTS usage_report_rollup AFTER INSERT ON usage_report_ledger
WHEN NEW.usage_contract_version = 1
BEGIN
    INSERT INTO daily_model_usage (
      usage_date, source, provider, model, build_channel, is_ci, responses,
      input_tokens, output_tokens, cache_read_input_tokens, cache_creation_input_tokens, total_tokens
    ) VALUES (
      NEW.usage_date, NEW.source, NEW.provider, NEW.model, NEW.build_channel, NEW.is_ci, NEW.responses,
      NEW.input_tokens, NEW.output_tokens, NEW.cache_read_input_tokens, NEW.cache_creation_input_tokens, NEW.total_tokens
    ) ON CONFLICT(usage_date, source, provider, model, build_channel, is_ci) DO UPDATE SET
      responses = responses + excluded.responses,
      input_tokens = input_tokens + excluded.input_tokens,
      output_tokens = output_tokens + excluded.output_tokens,
      cache_read_input_tokens = cache_read_input_tokens + excluded.cache_read_input_tokens,
      cache_creation_input_tokens = cache_creation_input_tokens + excluded.cache_creation_input_tokens,
      total_tokens = total_tokens + excluded.total_tokens,
      updated_at = datetime('now');
    -- Drop transient payload after the atomic rollup, retaining only receipt keys.
    UPDATE usage_report_ledger SET usage_date=NULL, source=NULL, provider=NULL, model=NULL,
      build_channel=NULL, is_ci=NULL, responses=NULL, input_tokens=NULL, output_tokens=NULL,
      cache_read_input_tokens=NULL, cache_creation_input_tokens=NULL, total_tokens=NULL
      WHERE receipt_key=NEW.receipt_key;
END;
CREATE TRIGGER IF NOT EXISTS usage_report_rollup_v2 AFTER INSERT ON usage_report_ledger
WHEN NEW.usage_contract_version = 2
BEGIN
    INSERT INTO daily_model_usage_v2 (
      usage_date, source, provider, model, build_channel, is_ci, responses,
      input_tokens, output_tokens, cache_read_input_tokens, cache_creation_input_tokens, total_tokens
    ) VALUES (
      NEW.usage_date, NEW.source, NEW.provider, NEW.model, NEW.build_channel, NEW.is_ci, NEW.responses,
      NEW.input_tokens, NEW.output_tokens, NEW.cache_read_input_tokens, NEW.cache_creation_input_tokens, NEW.total_tokens
    ) ON CONFLICT(usage_date, source, provider, model, build_channel, is_ci) DO UPDATE SET
      responses = responses + excluded.responses,
      input_tokens = input_tokens + excluded.input_tokens,
      output_tokens = output_tokens + excluded.output_tokens,
      cache_read_input_tokens = cache_read_input_tokens + excluded.cache_read_input_tokens,
      cache_creation_input_tokens = cache_creation_input_tokens + excluded.cache_creation_input_tokens,
      total_tokens = total_tokens + excluded.total_tokens,
      updated_at = datetime('now');
    -- Drop transient payload after the atomic rollup, retaining only receipt keys.
    UPDATE usage_report_ledger SET usage_date=NULL, source=NULL, provider=NULL, model=NULL,
      build_channel=NULL, is_ci=NULL, responses=NULL, input_tokens=NULL, output_tokens=NULL,
      cache_read_input_tokens=NULL, cache_creation_input_tokens=NULL, total_tokens=NULL
      WHERE receipt_key=NEW.receipt_key;
END;
