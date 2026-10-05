use serde::{Deserialize, Serialize};

/// Search engine used by the websearch tool.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash, Default)]
#[serde(rename_all = "lowercase")]
pub enum WebSearchEngine {
    /// DuckDuckGo HTML search, no API key required.
    #[default]
    Duckduckgo,
    /// Bing search. Uses the Bing API when configured, otherwise Bing HTML search.
    Bing,
    /// SearXNG metasearch instance (JSON API). Requires `searxng_url` (or the
    /// `JCODE_SEARXNG_URL` env var) to point at a SearXNG instance. Useful on
    /// hosts where DuckDuckGo/Bing block the request via TLS fingerprinting.
    Searxng,
    /// Provider-native server-side search (Anthropic `web_search`, OpenAI
    /// Responses `web_search`). Search runs on the model provider's side, so it
    /// works on hosts where scraping is blocked and needs no extra API key.
    /// When the active provider/model does not support it, the local
    /// `websearch` tool is used with `fallback_engines` instead.
    Native,
}

impl WebSearchEngine {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Duckduckgo => "duckduckgo",
            Self::Bing => "bing",
            Self::Searxng => "searxng",
            Self::Native => "native",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "duckduckgo" | "ddg" => Some(Self::Duckduckgo),
            "bing" => Some(Self::Bing),
            "searxng" | "searx" => Some(Self::Searxng),
            "native" | "provider" => Some(Self::Native),
            _ => None,
        }
    }

    /// True for engines the local `websearch` tool can run itself.
    pub fn is_local(self) -> bool {
        !matches!(self, Self::Native)
    }
}

/// Configuration for the websearch tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct WebSearchConfig {
    /// Preferred engine when the tool input does not specify one.
    pub engine: WebSearchEngine,
    /// Keyless HTML engines to try after the preferred engine fails.
    pub fallback_engines: Vec<WebSearchEngine>,
    /// Optional Bing API key for primary Bing searches. Fallback Bing uses keyless HTML search.
    pub bing_api_key: Option<String>,
    /// Environment variable containing the Bing API key.
    pub bing_api_key_env: String,
    /// Bing market, e.g. "en-US" or "zh-CN".
    pub bing_market: String,
    /// Base URL of a SearXNG instance (e.g. "https://searx.example.org"), used
    /// by the `searxng` engine. When empty, the `searxng_url_env` variable is
    /// consulted instead.
    pub searxng_url: Option<String>,
    /// Environment variable containing the SearXNG base URL.
    pub searxng_url_env: String,
    /// Prefer the model provider's own server-side search whenever the active
    /// provider/model supports it (Anthropic first-party API, OpenAI
    /// Responses). The local `websearch` tool is then replaced by the hosted
    /// one. Providers without native search keep the local tool and `engine`.
    /// Default true. `engine = "native"` also turns this on.
    pub prefer_native: bool,
    /// Maximum provider-native searches per request.
    /// Anthropic bills roughly $10 per 1,000 searches on API keys, so this caps
    /// spend. Anthropic only; OpenAI has no per-request cap.
    pub native_max_uses: Option<u32>,
    /// Restrict provider-native search to these domains. Mutually exclusive
    /// with `native_blocked_domains` (Anthropic rejects both).
    pub native_allowed_domains: Vec<String>,
    /// Never return results from these domains (Anthropic only).
    pub native_blocked_domains: Vec<String>,
    /// Anthropic server tool version, e.g. "web_search_20250305" (default) or
    /// "web_search_20260209". Newer versions are sent with
    /// `allowed_callers = ["direct"]` so models without programmatic tool
    /// calling are not rejected.
    pub native_anthropic_tool_version: String,
}

impl Default for WebSearchConfig {
    fn default() -> Self {
        Self {
            engine: WebSearchEngine::Duckduckgo,
            fallback_engines: vec![WebSearchEngine::Bing],
            bing_api_key: None,
            bing_api_key_env: "JCODE_BING_API_KEY".to_string(),
            bing_market: "en-US".to_string(),
            searxng_url: None,
            searxng_url_env: "JCODE_SEARXNG_URL".to_string(),
            prefer_native: true,
            native_max_uses: Some(DEFAULT_NATIVE_WEB_SEARCH_MAX_USES),
            native_allowed_domains: Vec::new(),
            native_blocked_domains: Vec::new(),
            native_anthropic_tool_version: DEFAULT_ANTHROPIC_WEB_SEARCH_TOOL.to_string(),
        }
    }
}

/// Default cap on provider-native searches per request.
pub const DEFAULT_NATIVE_WEB_SEARCH_MAX_USES: u32 = 5;
/// Default Anthropic web search server tool version.
pub const DEFAULT_ANTHROPIC_WEB_SEARCH_TOOL: &str = "web_search_20250305";

impl WebSearchConfig {
    /// True when provider-native search should be used where available.
    pub fn native_enabled(&self) -> bool {
        self.prefer_native || self.engine == WebSearchEngine::Native
    }
}
