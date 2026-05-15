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
use std::path::PathBuf;
use std::sync::{LazyLock, RwLock};
use std::time::SystemTime;

#[derive(Debug, Clone, PartialEq, Eq)]
struct ConfigCacheFingerprint {
    path: Option<PathBuf>,
    modified: Option<SystemTime>,
    len: Option<u64>,
    env: Vec<(String, String)>,
}

impl ConfigCacheFingerprint {
    fn current() -> Self {
        let path = Config::path();
        let metadata = path.as_ref().and_then(|path| std::fs::metadata(path).ok());
        Self {
            path,
            modified: metadata
                .as_ref()
                .and_then(|metadata| metadata.modified().ok()),
            len: metadata.as_ref().map(std::fs::Metadata::len),
            env: config_env_fingerprint(),
        }
    }
}

struct ConfigCache {
    config: &'static Config,
    fingerprint: ConfigCacheFingerprint,
    force_reload: bool,
}

static CONFIG_CACHE: LazyLock<RwLock<ConfigCache>> = LazyLock::new(|| {
    let fingerprint = ConfigCacheFingerprint::current();
    RwLock::new(ConfigCache {
        config: leak_config(Config::load()),
        fingerprint,
        force_reload: false,
    })
});

fn leak_config(config: Config) -> &'static Config {
    Box::leak(Box::new(config))
}

/// Get the global config instance.
///
/// The returned reference is backed by a reloadable process cache. Each call
/// checks the config file path/metadata and relevant environment overrides; when
/// those inputs change, the next call reloads config.toml and invalidates
/// dependent auth/model caches. Older references remain valid for the duration of
/// any in-flight operation.
pub fn config() -> &'static Config {
    let fingerprint = ConfigCacheFingerprint::current();
    if let Ok(cache) = CONFIG_CACHE.read()
        && !cache.force_reload
        && cache.fingerprint == fingerprint
    {
        return cache.config;
    }

    let mut reloaded = false;
    let config = {
        let mut cache = CONFIG_CACHE
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let fingerprint = ConfigCacheFingerprint::current();
        if cache.force_reload || cache.fingerprint != fingerprint {
            cache.config = leak_config(Config::load());
            cache.fingerprint = fingerprint;
            cache.force_reload = false;
            reloaded = true;
        }
        cache.config
    };

    if reloaded {
        notify_config_reloaded();
    }

    config
}

fn config_env_fingerprint() -> Vec<(String, String)> {
    let mut values = std::env::vars_os()
        .filter_map(|(key, value)| {
            let key = key.to_string_lossy().to_string();
            if key == "JCODE_HOME"
                || key == "HOME"
                || key == "XDG_CONFIG_HOME"
                || key.starts_with("JCODE_")
            {
                Some((key, value.to_string_lossy().to_string()))
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    values.sort_by(|left, right| left.0.cmp(&right.0));
    values
}

pub(crate) fn invalidate_config_cache() {
    let mut cache = CONFIG_CACHE
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    cache.force_reload = true;
    drop(cache);
    notify_config_reloaded();
}

fn notify_config_reloaded() {
    crate::auth::AuthStatus::invalidate_cache();
    crate::bus::Bus::global().publish_models_updated();
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

    /// Policy-level overrides.
    pub policy: PolicyConfig,

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
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct PolicyConfig {
    /// Providers excluded from auth probes, auto-selection, fallback, and model routing.
    pub disabled_providers: Vec<String>,
}

impl Config {
    pub fn disabled_provider_entries(&self) -> impl Iterator<Item = &str> {
        self.provider
            .disabled_providers
            .iter()
            .chain(self.policy.disabled_providers.iter())
            .map(String::as_str)
    }
}

/// External dictation / speech-to-text integration.
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

mod config_file;
mod default_file;
mod display_summary;
mod env_overrides;

#[cfg(test)]
#[path = "config_tests.rs"]
mod tests;
