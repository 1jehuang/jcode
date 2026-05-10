//! Configuration file support for jcode
//!
//! Config is loaded from `~/.jcode/config.toml` (or `$JCODE_HOME/config.toml`)
//! Environment variables override config file settings.

pub use jcode_config_types::{
    AgentsConfig, AmbientConfig, AuthConfig, AutoJudgeConfig, AutoReviewConfig, CompactionConfig,
    CompactionMode, CrossProviderFailoverMode, DiagramDisplayMode, DiagramPanePosition,
    DiffDisplayMode, DisplayConfig, FeatureConfig, GatewayConfig, KeybindingsConfig,
    MarkdownSpacingMode, NamedProviderAuth, NamedProviderConfig, NamedProviderModelConfig,
    NamedProviderType, NativeScrollbarConfig, ProviderConfig, SafetyConfig,
    SessionPickerResumeAction, UpdateChannel,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::OnceLock;

static CONFIG: OnceLock<Config> = OnceLock::new();

/// Get the global config instance (loaded once on first access)
pub fn config() -> &'static Config {
    CONFIG.get_or_init(Config::load)
}

/// Main configuration struct
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Config {
    /// Keybinding configuration
    pub keybindings: KeybindingsConfig,

    /// External dictation / speech-to-text integration
    pub dictation: DictationConfig,

    /// Display/UI configuration
    pub display: DisplayConfig,

    /// Feature toggles
    pub features: FeatureConfig,

    /// Auth trust / consent configuration
    pub auth: AuthConfig,

    /// Provider configuration
    pub provider: ProviderConfig,

    /// Named provider profiles, keyed by profile name.
    ///
    /// Example:
    /// [providers.my-gateway]
    /// type = "openai-compatible"
    /// base_url = "https://llm.example.com/v1"
    /// api_key_env = "MY_GATEWAY_API_KEY"
    pub providers: BTreeMap<String, NamedProviderConfig>,

    /// Agent-specific model defaults
    pub agents: AgentsConfig,

    /// Ambient mode configuration
    pub ambient: AmbientConfig,

    /// Safety / notification configuration
    pub safety: SafetyConfig,

    /// WebSocket gateway configuration (for iOS/web clients)
    pub gateway: GatewayConfig,

    /// Compaction configuration
    pub compaction: CompactionConfig,

    /// Auto-review configuration
    pub autoreview: AutoReviewConfig,

    /// Auto-judge configuration
    pub autojudge: AutoJudgeConfig,

    /// gRPC server configuration (mTLS, API Token)
    pub grpc: GrpcConfig,
}

/// 外部 dictation / speech-to-text integration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DictationConfig {
    /// Shell command to run. Must print the transcript to stdout.
    pub command: String,
    /// How to apply the resulting transcript.
    pub mode: crate::protocol::TranscriptMode,
    /// Optional in-app hotkey to trigger dictation.
    pub key: String,
    /// Maximum time to wait for the command to finish (0 = no timeout).
    pub timeout_secs: u64,
}

impl Default for DictationConfig {
    fn default() -> Self {
        Self {
            command: String::new(),
            mode: crate::protocol::TranscriptMode::Send,
            key: "off".to_string(),
            timeout_secs: 90,
        }
    }
}

/// gRPC server configuration (mTLS, API Token)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GrpcConfig {
    /// gRPC server listen port (default: 50051)
    pub port: u16,
    /// API Token for gRPC auth
    pub api_token: String,
    /// mTLS: server certificate file path (PEM)
    pub tls_cert_path: String,
    /// mTLS: server private key file path (PEM)
    pub tls_key_path: String,
    /// mTLS: CA certificate (for client verification)
    pub tls_ca_cert_path: String,
    /// Enable mTLS (require both server cert + client CA verification)
    pub mtls_enabled: bool,
    /// Enable API Token validation
    pub token_auth_enabled: bool,
}

impl Default for GrpcConfig {
    fn default() -> Self {
        Self {
            port: 50051,
            api_token: String::new(),
            tls_cert_path: String::new(),
            tls_key_path: String::new(),
            tls_ca_cert_path: String::new(),
            mtls_enabled: false,
            token_auth_enabled: false,
        }
    }
}

mod config_file;
mod default_file;
mod display_summary;
mod env_overrides;

#[cfg(test)]
#[path = "config_tests.rs"]
mod tests;
