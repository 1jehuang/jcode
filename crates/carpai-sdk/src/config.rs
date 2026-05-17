//! Configuration for CarpAI SDK

use super::cache::CacheConfig;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use zeroize::{Zeroize, Zeroizing};

/// Main configuration for CarpAI client
///
/// # Examples
///
/// ```
/// use carpai_sdk::CarpAiConfig;
///
/// // Default configuration (auto-detects API key from env)
/// let config = CarpAiConfig::default();
///
/// // Custom configuration
/// let config = CarpAiConfig {
///     server: carpai_sdk::ServerConfig {
///         url: Some("http://localhost:8080".to_string()),
///         timeout_secs: 60,
///         ..Default::default()
///     },
///     cache: carpai_sdk::CacheConfig {
///         enabled: true,
///         max_size: 1000,
///         ttl_secs: 7200,
///         ..Default::default()
///     },
///     ..Default::default()
/// };
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CarpAiConfig {
    /// Server connection settings
    pub server: ServerConfig,

    /// Authentication configuration
    pub auth: AuthConfig,

    /// Cache settings
    pub cache: CacheConfig,

    /// Performance tuning
    pub performance: PerformanceConfig,

    /// Offline mode settings
    pub offline: OfflineConfig,

    /// IDE-specific settings
    pub ide: IdeConfig,

    /// Feature flags
    pub features: FeatureFlags,
}

impl CarpAiConfig {
    /// Load configuration from file
    pub fn from_file(path: &str) -> Result<Self, config::ConfigError> {
        let settings = config::Config::builder()
            .add_source(config::File::with_name(path))
            .build()?;

        settings.try_deserialize()
    }

    /// Create zero-configuration setup (auto-detect optimal settings)
    pub fn zero_config() -> Self {
        let mut config = Self::default();

        // Auto-detect settings
        config.server.auto_detect = true;
        config.auth.auto_detect_api_key = true;
        config.cache.enabled = true;
        config.offline.enabled = true;

        // Optimize for common use cases
        config.performance.stream_buffer_size = 4096;
        config.performance.enable_cache = true;

        config
    }

    /// Validate configuration
    pub fn validate(&self) -> Result<(), String> {
        if let Some(ref url) = self.server.url {
            if url.is_empty() {
                return Err("Server URL cannot be empty".to_string());
            }
        }

        if self.performance.max_retries == 0 {
            return Err("Max retries must be at least 1".to_string());
        }

        Ok(())
    }
}

/// Server connection configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    /// Server URL (e.g., "http://localhost:50051")
    pub url: Option<String>,

    /// gRPC server address (e.g., "localhost:50051")
    pub grpc_address: Option<String>,

    /// REST API base URL
    pub rest_url: Option<String>,

    /// Connection timeout in seconds
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,

    /// Auto-detect server settings
    #[serde(default)]
    pub auto_detect: bool,

    /// Enable TLS/SSL
    #[serde(default)]
    pub tls_enabled: bool,

    /// TLS certificate path (if using custom certs)
    pub tls_cert_path: Option<PathBuf>,
}

fn default_timeout() -> u64 { 30 }

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            url: Some("http://localhost:50051".to_string()),
            grpc_address: Some("localhost:50051".to_string()),
            rest_url: Some("http://localhost:8080".to_string()),
            timeout_secs: default_timeout(),
            auto_detect: false,
            tls_enabled: false,
            tls_cert_path: None,
        }
    }
}

/// Authentication configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthConfig {
    /// API key for authentication (zeroized on drop)
    #[serde(skip)]
    pub api_key: Option<Zeroizing<String>>,

    /// Auto-detect API key from environment variables
    #[serde(default)]
    pub auto_detect_api_key: bool,

    /// Authentication token (for session-based auth)
    #[serde(skip)]
    pub token: Option<Zeroizing<String>>,

    /// Token refresh interval in seconds
    #[serde(default = "default_token_refresh")]
    pub token_refresh_secs: u64,
}

fn default_token_refresh() -> u64 { 3600 } // 1 hour

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            api_key: None,
            auto_detect_api_key: true,
            token: None,
            token_refresh_secs: default_token_refresh(),
        }
    }
}

impl AuthConfig {
    /// Set API key securely (zeroized on drop)
    pub fn set_api_key(&mut self, key: String) {
        self.api_key = Some(Zeroizing::new(key));
    }

    /// Get API key (from config or environment)
    pub fn get_api_key(&self) -> Option<String> {
        if let Some(ref key) = self.api_key {
            return Some(key.to_string());
        }

        if self.auto_detect_api_key {
            // Check common environment variables
            std::env::var("CARPAI_API_KEY")
                .or_else(|_| std::env::var("OPENAI_API_KEY"))
                .or_else(|_| std::env::var("JCODE_API_KEY"))
                .ok()
        } else {
            None
        }
    }
}

/// Performance tuning configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerformanceConfig {
    /// Maximum number of concurrent requests
    #[serde(default = "default_concurrency")]
    pub max_concurrent_requests: usize,

    /// Stream buffer size in bytes
    #[serde(default = "default_buffer_size")]
    pub stream_buffer_size: usize,

    /// Maximum retries for failed requests
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,

    /// Retry delay in milliseconds (exponential backoff)
    #[serde(default = "default_retry_delay")]
    pub retry_delay_ms: u64,

    /// Enable response caching
    #[serde(default = "default_true")]
    pub enable_cache: bool,

    /// Request timeout in seconds
    #[serde(default = "default_request_timeout")]
    pub request_timeout_secs: u64,

    /// Rate limiting: requests per second
    #[serde(default = "default_rate_limit")]
    pub rate_limit_per_second: f64,
}

fn default_concurrency() -> usize { 10 }
fn default_buffer_size() -> usize { 8192 }
fn default_max_retries() -> u32 { 3 }
fn default_retry_delay() -> u64 { 1000 }
fn default_true() -> bool { true }
fn default_request_timeout() -> u64 { 120 }
fn default_rate_limit() -> f64 { 100.0 }

impl Default for PerformanceConfig {
    fn default() -> Self {
        Self {
            max_concurrent_requests: default_concurrency(),
            stream_buffer_size: default_buffer_size(),
            max_retries: default_max_retries(),
            retry_delay_ms: default_retry_delay(),
            enable_cache: default_true(),
            request_timeout_secs: default_request_timeout(),
            rate_limit_per_second: default_rate_limit(),
        }
    }
}

/// Offline mode configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OfflineConfig {
    /// Enable offline mode support
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Maximum cache age for offline use (in hours)
    #[serde(default = "default_offline_cache_age")]
    pub max_cache_age_hours: u64,

    /// Queue requests when offline
    #[serde(default = "default_true")]
    pub queue_requests_when_offline: bool,

    /// Maximum queued requests when offline
    #[serde(default = "default_max_queue")]
    pub max_queued_requests: usize,

    /// Auto-sync when back online
    #[serde(default = "default_true")]
    pub auto_sync_on_reconnect: bool,
}

fn default_offline_cache_age() -> u64 { 24 }
fn default_max_queue() -> usize { 1000 }

impl Default for OfflineConfig {
    fn default() -> Self {
        Self {
            enabled: default_true(),
            max_cache_age_hours: default_offline_cache_age(),
            queue_requests_when_offline: default_true(),
            max_queued_requests: default_max_queue(),
            auto_sync_on_reconnect: default_true(),
        }
    }
}

/// IDE-specific configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdeConfig {
    /// IDE type (vscode, jetbrains, neovim, etc.)
    pub ide_type: Option<String>,

    /// IDE-specific settings (JSON object)
    #[serde(default)]
    pub settings: serde_json::Value,

    /// Enable inline completion
    #[serde(default = "default_true")]
    pub inline_completion_enabled: bool,

    /// Enable chat panel
    #[serde(default = "default_true")]
    pub chat_panel_enabled: bool,

    /// Enable code actions
    #[serde(default)]
    pub code_actions_enabled: bool,
}

impl Default for IdeConfig {
    fn default() -> Self {
        Self {
            ide_type: None,
            settings: serde_json::json!({}),
            inline_completion_enabled: true,
            chat_panel_enabled: true,
            code_actions_enabled: false,
        }
    }
}

/// Feature flags
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeatureFlags {
    /// Enable streaming responses
    #[serde(default = "default_true")]
    pub streaming: bool,

    /// Enable multi-modal support (images/audio)
    #[serde(default)]
    pub multimodal: bool,

    /// Enable agent/tool-use capabilities
    #[serde(default)]
    pub agent_mode: bool,

    /// Enable RAG (Retrieval-Augmented Generation)
    #[serde(default = "default_true")]
    pub rag_enabled: bool,

    /// Enable telemetry/analytics
    #[serde(default)]
    pub telemetry: bool,

    /// Enable experimental features
    #[serde(default)]
    pub experimental: bool,
}

impl Default for FeatureFlags {
    fn default() -> Self {
        Self {
            streaming: true,
            multimodal: false,
            agent_mode: false,
            rag_enabled: true,
            telemetry: false,
            experimental: false,
        }
    }
}
