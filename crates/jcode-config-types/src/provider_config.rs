use serde::{Deserialize, Serialize};

use crate::CrossProviderFailoverMode;
use crate::{default_auto_retry_base_delay_secs, deserialize_auto_retry_base_delay_secs};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ProviderConfig {
    /// Default model to use (e.g. "claude-opus-4-8", "copilot:claude-opus-4.6")
    pub default_model: Option<String>,
    /// Default provider to use (claude|openai|copilot|openrouter)
    pub default_provider: Option<String>,
    /// Reasoning effort for OpenAI Responses API (none|minimal|low|medium|high|xhigh|max)
    pub openai_reasoning_effort: Option<String>,
    /// Reasoning effort for Anthropic Messages API output_config (none|low|medium|high|xhigh; max aliases to strongest supported)
    pub anthropic_reasoning_effort: Option<String>,
    /// Request one-hour Anthropic prompt caching instead of five minutes.
    pub anthropic_cache_ttl_1h: bool,
    /// OpenAI transport mode (auto|websocket|https)
    pub openai_transport: Option<String>,
    /// OpenAI service tier override (priority|ultrafast|flex|off). Ultrafast is opt-in.
    pub openai_service_tier: Option<String>,
    /// OpenAI native compaction mode: "auto", "explicit", or "off".
    pub openai_native_compaction_mode: String,
    /// Token threshold at which OpenAI auto native compaction should trigger.
    pub openai_native_compaction_threshold_tokens: usize,
    /// Preserve provider-native reasoning/thinking items for future-turn context when supported.
    pub preserve_reasoning_context: bool,
    /// How to handle cross-provider failover when the same input would be resent elsewhere.
    pub cross_provider_failover: CrossProviderFailoverMode,
    /// Skip the cross-provider countdown for definitive exhausted usage only.
    /// Manual cross-provider failover still disables automatic switching.
    pub exhausted_usage_immediate_failover: bool,
    /// Prefer bounded same-account retries for recoverable Anthropic rate limits
    /// before rotating to another account. Long reset delays still yield.
    pub anthropic_retry_recoverable_rate_limits: bool,
    /// Whether jcode should automatically try another account on the same provider
    /// before falling back to a different provider.
    pub same_provider_account_failover: bool,
    /// Automatically redeem an earned OpenAI banked reset when weekly OAuth
    /// usage is exhausted. Never purchases credits. Enabled by default.
    pub openai_auto_banked_reset: bool,
    /// Automatically redeem earned Anthropic Fable resets for exhausted weekly
    /// OAuth usage. Never purchases credits. Disabled by default.
    pub anthropic_auto_fable_reset: bool,
    /// Preserve earned resets while the account pool router has another ready route.
    /// Applies to both OpenAI and Anthropic automatic resets. Default: true.
    pub auto_reset_only_when_no_routes: bool,
    /// Copilot premium request mode: "normal", "one", or "zero"
    /// "zero" means all requests are free (no premium requests consumed)
    pub copilot_premium: Option<String>,
    /// Pin the `gemini` provider to Code Assist OAuth even when a Gemini
    /// Developer API key (`gemini.env` / `GEMINI_API_KEY`) is present. Without
    /// this an API key silently wins and every turn bills per token on the
    /// key's project. `JCODE_GEMINI_FORCE_OAUTH` overrides this value.
    pub gemini_force_oauth: bool,
    /// Google Cloud project for Gemini Code Assist OAuth. Workspace accounts
    /// require one; without it every turn fails with "requires setting
    /// GOOGLE_CLOUD_PROJECT". `GOOGLE_CLOUD_PROJECT` (or its legacy `_ID`
    /// alias) overrides this value. Config values are never exported to env.
    pub gemini_project: Option<String>,
    /// When set (non-empty), /model only lists routes from these providers.
    /// Entries match provider labels ("openai", "anthropic", "copilot",
    /// "openrouter", ...), api methods ("claude-oauth",
    /// "openai-compatible:myprofile", ...), or openai-compatible profile ids
    /// ("myprofile"). The active model's routes always stay visible.
    pub model_picker_providers: Option<Vec<String>>,
    /// Max seconds to wait for streaming data before timing out a request with
    /// no data received. Base budget only: high reasoning efforts scale it up
    /// automatically (see `jcode_base::provider::stream_idle_timeout_for_effort`).
    /// Default: 180. Overridable via `JCODE_STREAM_IDLE_TIMEOUT_SECS`.
    pub stream_idle_timeout_secs: u64,
    /// Maximum request attempts for transient provider errors, including the
    /// initial attempt. Default: 8. Overridable via `JCODE_MAX_RETRIES`.
    pub max_retries: u32,
    /// Maximum exponential-backoff delay between transient-error retries.
    /// Default: 30 seconds. Overridable via `JCODE_RETRY_BACKOFF_CAP_SECS`.
    pub retry_backoff_cap_secs: u64,
    /// Request-body compression policy: "auto" (compress only when the
    /// measured uplink makes it faster), "always", or "off". Applies only to
    /// endpoints verified to accept it: gzip (Anthropic, ChatGPT HTTPS), zstd
    /// (OpenAI API), and WebSocket permessage-deflate (Responses sockets).
    /// Overridable via `JCODE_REQUEST_COMPRESSION`.
    pub request_compression: String,
    /// Base delay (seconds) between auto-retry attempts after a remote request
    /// error. The actual backoff is linear: `base * attempt_number`. Default: 2.
    /// Overridable via `JCODE_AUTO_RETRY_BASE_DELAY_SECS`. Raise this for shared
    /// gateways that return 429 without a `retry-after` header, so retries are
    /// gentler instead of hammering the endpoint at 2s/4s/6s.
    #[serde(
        default = "default_auto_retry_base_delay_secs",
        deserialize_with = "deserialize_auto_retry_base_delay_secs"
    )]
    pub auto_retry_base_delay_secs: u64,
    /// Whether jcode should automatically retry transient remote failures.
    /// Default: true. Overridable via `JCODE_AUTO_RETRY_ENABLED`.
    pub auto_retry_enabled: bool,
    /// Maximum auto-retry attempts before giving up and surfacing the error.
    /// Default: 3. Overridable via `JCODE_AUTO_RETRY_MAX_ATTEMPTS`. Raise this
    /// for shared services that need more time to free up capacity.
    pub auto_retry_max_attempts: u8,
}

impl Default for ProviderConfig {
    fn default() -> Self {
        Self {
            default_model: None,
            default_provider: None,
            openai_reasoning_effort: Some("low".to_string()),
            anthropic_reasoning_effort: Some("low".to_string()),
            anthropic_cache_ttl_1h: true,
            openai_transport: None,
            openai_service_tier: Some("priority".to_string()),
            openai_native_compaction_mode: "auto".to_string(),
            openai_native_compaction_threshold_tokens: 200_000,
            preserve_reasoning_context: true,
            cross_provider_failover: CrossProviderFailoverMode::Countdown,
            exhausted_usage_immediate_failover: true,
            anthropic_retry_recoverable_rate_limits: true,
            same_provider_account_failover: true,
            openai_auto_banked_reset: true,
            anthropic_auto_fable_reset: false,
            auto_reset_only_when_no_routes: true,
            copilot_premium: None,
            gemini_force_oauth: false,
            gemini_project: None,
            model_picker_providers: None,
            stream_idle_timeout_secs: 180,
            max_retries: 8,
            retry_backoff_cap_secs: 30,
            request_compression: "auto".to_string(),
            auto_retry_base_delay_secs: 2,
            auto_retry_enabled: true,
            auto_retry_max_attempts: 3,
        }
    }
}
