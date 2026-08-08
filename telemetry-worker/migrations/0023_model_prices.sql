-- Model price reference table (migration 0023).
--
-- Turns out we already collect everything needed to put a dollar figure on the
-- token flow through jcode: `events.input_tokens` / `output_tokens` /
-- `cache_read_input_tokens` / `cache_creation_input_tokens` plus the model and
-- provider labels (`model_end` / `provider_end` on session_end rows). What was
-- missing is a price per model, so the dashboards could only report raw token
-- counts. This table supplies list prices in USD per million tokens, synced
-- from the free models.dev catalog by `scripts/sync-model-prices.mjs` (the same
-- catalog the CLI uses for route cheapness estimates, see
-- crates/jcode-base/src/model_pricing.rs).
--
-- `model` is the raw telemetry label, NOT a models.dev id: users run jcode
-- through OpenRouter/Copilot/gateways that rename models
-- (`z-ai/glm-5.2`, `cc/claude-opus-5`, `openai/gpt-5.6-sol`, ...), so the sync
-- script normalizes those aliases and writes one row per observed label. Rows
-- with no price match are still inserted with NULL prices, which is how the
-- token-value dashboard reports its unpriced-token coverage instead of
-- silently undercounting.
--
-- Prices are list/rack rates. Most jcode traffic runs on subscriptions
-- (Claude Max, ChatGPT Pro, Copilot), so the resulting number is
-- "list-price equivalent value of tokens served", not anyone's actual bill.
CREATE TABLE IF NOT EXISTS model_prices (
    model TEXT PRIMARY KEY,
    -- Canonical models.dev id the price was taken from, for auditing.
    source_model TEXT,
    -- models.dev provider id the price was taken from (e.g. openai, anthropic).
    source_provider TEXT,
    -- USD per million tokens. NULL means "no price known", which the
    -- dashboards must treat as unpriced rather than free.
    input_usd_per_mtok REAL,
    output_usd_per_mtok REAL,
    cache_read_usd_per_mtok REAL,
    cache_write_usd_per_mtok REAL,
    -- 1 when the provider's reported input token count already includes cache
    -- reads (OpenAI-style `prompt_tokens_details.cached_tokens` is a subset of
    -- `prompt_tokens`), so the dashboard must subtract cache reads from input
    -- before pricing. Anthropic reports them as disjoint buckets and gets 0.
    -- See crates/jcode-compaction-core/src/lib.rs (estimate_compaction_tokens)
    -- and jcode-provider-openai/src/stream.rs (extract_cached_input_tokens).
    input_includes_cache_read INTEGER NOT NULL DEFAULT 0,
    -- 'catalog' when matched from models.dev, 'free' for known zero-cost
    -- routes, 'unpriced' when no match was found.
    price_kind TEXT NOT NULL DEFAULT 'catalog',
    updated_at TEXT DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_model_prices_price_kind ON model_prices(price_kind);
