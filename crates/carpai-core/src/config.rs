use std::path::PathBuf;
use serde::{de::Error as _, Deserialize, Serialize};
use carpai_internal::AppConfig;

/// Layer 1: Core configuration (extends Layer 0 AppConfig)
///
/// Three-layer loading priority:
/// 1. Hardcoded defaults
/// 2. TOML config file (~/.carpai/config.toml or /etc/carpai/server.toml)
/// 3. Environment variables (CARPAI_* prefix)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoreConfig {
    #[serde(flatten)]
    pub base: AppConfig,

    // === Storage ===

    /// Root data directory for all local storage (sessions, memory, cache)
    pub data_dir: PathBuf,

    /// Subdirectory for session JSONL files (relative to data_dir)
    #[serde(default = "default_session_dir")]
    pub session_subdir: String,

    /// Subdirectory for memory persistence (relative to data_dir)
    #[serde(default = "default_memory_dir")]
    pub memory_subdir: String,

    // === Concurrency ===

    /// Maximum number of tools that can execute concurrently
    #[serde(default = "default_max_concurrent_tools")]
    pub max_concurrent_tools: usize,

    /// Maximum agent loop iterations before forced stop
    #[serde(default = "default_max_iterations")]
    pub max_agent_iterations: usize,

    // === Completion Provider (for SidecarInferenceBackend) ===

    /// Provider configuration
    #[serde(default)]
    pub completion_provider: ProviderConfig,

    // === Caching ===

    /// Maximum in-memory cache size in MB
    #[serde(default = "default_cache_size")]
    pub cache_size_mb: usize,

    /// Enable disk-backed cache
    #[serde(default = "default_disk_cache")]
    pub disk_cache_enabled: bool,
}

/// Inference provider configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    /// Provider type identifier
    #[serde(default = "default_provider_type")]
    pub provider_type: String,

    /// API endpoint URL (for remote providers)
    pub endpoint: Option<String>,

    /// API key (read from environment variable, never stored in config file)
    pub api_key: Option<String>,

    /// Model name override
    pub model: Option<String>,

    /// Request timeout in seconds
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
}

impl CoreConfig {
    /// Get the full path to the session store directory
    pub fn session_store_path(&self) -> PathBuf {
        self.data_dir.join(&self.session_subdir)
    }

    /// Get the full path to the memory store directory
    pub fn memory_store_path(&self) -> PathBuf {
        self.data_dir.join(&self.memory_subdir)
    }

    /// Load configuration from a TOML file with environment variable overrides
    ///
    /// # Loading Priority
    /// 1. Default values (hardcoded)
    /// 2. Values from the TOML file (if it exists)
    /// 3. Environment variables (CARPAI_CORE__*) - highest priority
    ///
    /// # Example environment variables:
    /// ```bash
    /// CARPAI_CORE__DATA_DIR=/custom/path
    /// CARPAI_CORE__MAX_CONCURRENT_TOOLS=10
    /// CARPAI_CORE__COMPLETION_PROVIDER__MODEL=claude-sonnet-4-20250514
    /// ```
    pub fn load(path: &PathBuf) -> Result<Self, ConfigError> {
        let mut config = Self::default();

        if path.exists() {
            let content = std::fs::read_to_string(path).map_err(ConfigError::Io)?;
            config = toml::from_str(&content).map_err(ConfigError::Parse)?;
        }

        if let Ok(v) = std::env::var("CARPAI_DATA_DIR") {
            config.data_dir = v.into();
        }
        if let Ok(v) = std::env::var("CARPAI_CORE__DATA_DIR") {
            config.data_dir = v.into();
        }
        if let Ok(v) = std::env::var("CARPAI_DEFAULT_MODEL") {
            config.base.default_model = v;
        }
        if let Ok(v) = std::env::var("CARPAI_LOG_LEVEL") {
            // log_level is not in AppConfig yet, skip or extend
        }
        if let Ok(v) = std::env::var("CARPAI_CORE__MAX_CONCURRENT_TOOLS") {
            config.max_concurrent_tools = v.parse().map_err(|_| {
                ConfigError::Parse(toml::de::Error::custom("invalid MAX_CONCURRENT_TOOLS"))
            })?;
        }
        if let Ok(v) = std::env::var("CARPAI_CORE__MAX_AGENT_ITERATIONS") {
            config.max_agent_iterations = v.parse().map_err(|_| {
                ConfigError::Parse(toml::de::Error::custom("invalid MAX_AGENT_ITERATIONS"))
            })?;
        }

        Ok(config)
    }
}

impl Default for CoreConfig {
    fn default() -> Self {
        Self {
            base: AppConfig::default(),
            data_dir: dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".carpai"),
            session_subdir: default_session_dir(),
            memory_subdir: default_memory_dir(),
            max_concurrent_tools: default_max_concurrent_tools(),
            max_agent_iterations: default_max_iterations(),
            completion_provider: ProviderConfig::default(),
            cache_size_mb: default_cache_size(),
            disk_cache_enabled: default_disk_cache(),
        }
    }
}

impl Default for ProviderConfig {
    fn default() -> Self {
        Self {
            provider_type: default_provider_type(),
            endpoint: Some("http://localhost:11434".into()),
            api_key: None,
            model: None,
            timeout_secs: default_timeout(),
        }
    }
}

// --- Defaults ---

fn default_session_dir() -> String { "sessions".into() }
fn default_memory_dir() -> String { "memory".into() }
fn default_max_concurrent_tools() -> usize { 5 }
fn default_max_iterations() -> usize { 100 }
fn default_cache_size() -> usize { 512 }
fn default_disk_cache() -> bool { true }
fn default_provider_type() -> String { "local".into() }
fn default_timeout() -> u64 { 30 }

// --- Error type ---

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Parse error: {0}")]
    Parse(#[from] toml::de::Error),
}

// --- Tests ---

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = CoreConfig::default();
        assert!(config.data_dir.ends_with(".carpai"));
        assert_eq!(config.max_concurrent_tools, 5);
        assert_eq!(config.completion_provider.provider_type, "local");
    }

    #[test]
    fn test_paths() {
        let config = CoreConfig::default();
        let session_path = config.session_store_path();
        assert!(session_path.ends_with("sessions"));
        let memory_path = config.memory_store_path();
        assert!(memory_path.ends_with("memory"));
    }

    #[test]
    fn test_load_nonexistent_file() {
        let config = CoreConfig::load(&PathBuf::from("/nonexistent/config.toml")).unwrap();
        assert_eq!(config.max_concurrent_tools, 5); // should use default
    }
}
