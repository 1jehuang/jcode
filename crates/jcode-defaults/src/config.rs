//! Core Configuration System
//!
//! Provides hierarchical configuration with smart defaults:
//! 1. Built-in defaults (works without any config)
//! 2. User config (~/.jcode/config.toml)
//! 3. Project config (.jcode/config.toml)
//! 4. Environment variables (JCODE_*)
//! 5. Command-line arguments (highest priority)

use std::path::{Path, PathBuf};
use std::collections::HashMap;
use serde::{Deserialize, Serialize};
use anyhow::{Result, Context};

/// Main jcode configuration structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JcodeConfig {
    /// LLM provider settings
    pub llm: LlmConfig,
    
    /// gRPC server settings
    pub grpc: GrpcConfig,
    
    /// REST API server settings
    pub rest: RestConfig,
    
    /// RAG system settings
    pub rag: RagConfig,
    
    /// VS Code integration settings
    pub vscode: VscodeConfig,
    
    /// Performance tuning
    pub performance: PerformanceConfig,
    
    /// Logging and debugging
    pub logging: LoggingConfig,
}

impl Default for JcodeConfig {
    fn default() -> Self {
        Self {
            llm: LlmConfig::default(),
            grpc: GrpcConfig::default(),
            rest: RestConfig::default(),
            rag: RagConfig::default(),
            vscode: VscodeConfig::default(),
            performance: PerformanceConfig::default(),
            logging: LoggingConfig::default(),
        }
    }
}

impl JcodeConfig {
    /// Create zero-config defaults (like Cursor's first-run experience)
    pub fn zero_config() -> Self {
        Self::default()
    }
    
    /// Load with auto-discovery (checks env vars, config files, etc.)
    pub async fn load_with_discovery() -> Result<Self> {
        let mut config = Self::zero_config();
        
        // Priority order:
        // 1. Built-in defaults (already set)
        // 2. Global user config (~/.jcode/config.toml)
        if let Some(user_config) = Self::load_user_config()? {
            config = config.merge(user_config);
        }
        
        // 3. Project config (.jcode/config.toml)
        if let Some(project_config) = Self::load_project_config()? {
            config = config.merge(project_config);
        }
        
        // 4. Environment variables (JCODE_LLM_MODEL, etc.)
        config = config.apply_env_overrides();
        
        Ok(config)
    }
    
    /// Load user-level configuration
    fn load_user_config() -> Result<Option<Self>> {
        let config_path = dirs::home_dir()
            .map(|p| p.join(".jcode").join("config.toml"));
        
        match config_path {
            Some(path) if path.exists() => {
                let content = std::fs::read_to_string(&path)
                    .with_context(|| format!("Failed to read {}", path.display()))?;
                let config: Self = toml::from_str(&content)
                    .with_context(|| format!("Failed to parse {}", path.display()))?;
                Ok(Some(config))
            }
            _ => Ok(None),
        }
    }
    
    /// Load project-level configuration
    fn load_project_config() -> Result<Option<Self>> {
        // Search for .jcode/config.toml in current directory and parents
        let current_dir = std::env::current_dir()?;
        let mut search_path = Some(current_dir.as_path());
        
        while let Some(path) = search_path {
            let config_path = path.join(".jcode").join("config.toml");
            
            if config_path.exists() {
                let content = std::fs::read_to_string(&config_path)
                    .with_context(|| format!("Failed to read {}", config_path.display()))?;
                let config: Self = toml::from_str(&content)
                    .with_context(|| format!("Failed to parse {}", config_path.display()))?;
                return Ok(Some(config));
            }
            
            search_path = path.parent();
        }
        
        Ok(None)
    }
    
    /// Apply environment variable overrides
    fn apply_env_overrides(mut self) -> Self {
        // LLM settings
        if let Ok(model) = std::env::var("JCODE_LLM_MODEL") {
            self.llm.default_model = model;
        }
        if let Ok(provider) = std::env::var("JCODE_LLM_PROVIDER") {
            self.llm.default_provider = provider;
        }
        if let Ok(api_key) = std::env::var("DEEPSEEK_API_KEY") || 
           std::env::var("OPENAI_API_KEY").is_ok() ||
           std::env::var("JCODE_LLM_API_KEY").is_ok() {
            // Auto-detect which API key is available
            self.llm.auto_detect_api_key = true;
        }
        
        // Server settings
        if let Ok(port) = std::env::var("JCODE_GRPC_PORT") {
            if let Ok(port_num) = port.parse::<u16>() {
                self.grpc.port = port_num;
            }
        }
        if let Ok(port) = std::env::var("JCODE_REST_PORT") {
            if let Ok(port_num) = port.parse::<u16>() {
                self.rest.port = port_num;
            }
        }
        
        self
    }
    
    /// Merge another config into this one (lower priority)
    fn merge(mut self, other: Self) -> Self {
        // Simple merge strategy: use other's values where set
        if other.llm.default_model != LlmConfig::default().default_model {
            self.llm.default_model = other.llm.default_model;
        }
        if other.llm.default_provider != LlmConfig::default().default_provider {
            self.llm.default_provider = other.llm.default_provider;
        }
        if other.grpc.port != GrpcConfig::default().port {
            self.grpc.port = other.grpc.port;
        }
        if other.rest.port != RestConfig::default().port {
            self.rest.port = other.rest.port;
        }
        
        self
    }
    
    /// Generate default config file for user
    pub fn generate_default_config_toml() -> String {
        toml::to_string_pretty(&Self::default()).unwrap_or_else(|e| {
            format!("# Error generating config: {}\n# Using fallback", e)
        })
    }
    
    /// Save configuration to user home directory
    pub fn save_user_config(&self) -> Result<PathBuf> {
        let config_dir = dirs::home_dir()
            .ok_or_else(|| anyhow::anyhow!("Cannot determine home directory"))?
            .join(".jcode");
        
        std::fs::create_dir_all(&config_dir)?;
        
        let config_path = config_dir.join("config.toml");
        let content = toml::to_string_pretty(self)?;
        std::fs::write(&config_path, content)?;
        
        Ok(config_path)
    }
}

/// LLM Provider Configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmConfig {
    /// Default model to use (auto-detected based on provider)
    #[serde(default = "default_model")]
    pub default_model: String,
    
    /// Default provider type
    #[serde(default = "default_provider")]
    pub default_provider: String,
    
    /// Auto-detect API keys from environment
    #[serde(default = "default_true")]
    pub auto_detect_api_key: bool,
    
    /// Provider-specific configurations
    #[serde(default)]
    pub providers: HashMap<String, ProviderSpecificConfig>,
    
    /// Connection timeout in seconds
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
    
    /// Maximum retries for failed requests
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,
}

fn default_model() -> String { "deepseek-chat".to_string() }
fn default_provider() -> String { "deepseek".to_string() }
fn default_true() -> bool { true }
fn default_timeout() -> u64 { 30 }
fn default_max_retries() -> u32 { 3 }

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            default_model: default_model(),
            default_provider: default_provider(),
            auto_detect_api_key: true,
            providers: HashMap::new(),
            timeout_secs: default_timeout(),
            max_retries: default_max_retries(),
        }
    }
}

/// Provider-specific configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderSpecificConfig {
    /// API endpoint URL
    #[serde(default)]
    pub api_base_url: Option<String>,
    
    /// API key (or empty to auto-detect)
    #[serde(default)]
    pub api_key: Option<String>,
    
    /// Model name override
    #[serde(default)]
    pub model_name: Option<String>,
    
    /// Additional parameters
    #[serde(default)]
    pub extra_params: HashMap<String, serde_json::Value>,
}

/// gRPC Server Configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GrpcConfig {
    /// Server listen address
    #[serde(default = "default_grpc_address")]
    pub address: String,
    
    /// Server port
    #[serde(default = "default_grpc_port")]
    pub port: u16,
    
    /// Enable TLS
    #[serde(default)]
    pub tls_enabled: bool,
    
    /// TLS certificate path
    #[serde(default)]
    pub tls_cert_path: Option<String>,
    
    /// TLS key path
    #[serde(default)]
    pub tls_key_path: Option<String>,
    
    /// Max message size in bytes
    #[serde(default = "default_max_message_size")]
    pub max_message_size: usize,
    
    /// Connection pool size
    #[serde(default = "default_pool_size")]
    pub pool_size: usize,
}

fn default_grpc_address() -> String { "[::]".to_string() }
fn default_grpc_port() -> u16 { 50051 }
fn default_max_message_size() -> usize { 4 * 1024 * 1024 } // 4MB
fn default_pool_size() -> usize { 100 }

impl Default for GrpcConfig {
    fn default() -> Self {
        Self {
            address: default_grpc_address(),
            port: default_grpc_port(),
            tls_enabled: false,
            tls_cert_path: None,
            tls_key_path: None,
            max_message_size: default_max_message_size(),
            pool_size: default_pool_size(),
        }
    }
}

/// REST API Server Configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RestConfig {
    /// Server listen address
    #[serde(default = "default_rest_address")]
    pub address: String,
    
    /// Server port
    #[serde(default = "default_rest_port")]
    pub port: u16,
    
    /// CORS allowed origins
    #[serde(default = "default_cors_origins")]
    pub cors_origins: Vec<String>,
    
    /// Request timeout in seconds
    #[serde(default = "default_rest_timeout")]
    pub timeout_secs: u64,
    
    /// Enable OpenAPI/Swagger UI
    #[serde(default = "default_true")]
    pub enable_docs: bool,
    
    /// Rate limiting requests per minute
    #[serde(default = "default_rate_limit")]
    pub rate_limit_rpm: u32,
}

fn default_rest_address() -> String { "127.0.0.1".to_string() }
fn default_rest_port() -> u16 { 3000 }
fn default_cors_origins() -> Vec<String> { vec!["*".to_string()] }
fn default_rest_timeout() -> u64 { 60 }
fn default_rate_limit() -> u32 { 60 }

impl Default for RestConfig {
    fn default() -> Self {
        Self {
            address: default_rest_address(),
            port: default_rest_port(),
            cors_origins: default_cors_origins(),
            timeout_secs: default_rest_timeout(),
            enable_docs: true,
            rate_limit_rpm: default_rate_limit(),
        }
    }
}

/// RAG System Configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RagConfig {
    /// Enable RAG by default
    #[serde(default = "default_true")]
    pub enabled: bool,
    
    /// Indexing strategy
    #[serde(default = "default_index_strategy")]
    pub indexing_strategy: String,
    
    /// Maximum context snippets to retrieve
    #[serde(default = "default_max_snippets")]
    pub max_retrieved_snippets: usize,
    
    /// Embedding model
    #[serde(default = "default_embedding_model")]
    pub embedding_model: String,
    
    /// Cache directory
    #[serde(default)]
    pub cache_dir: Option<String>,
}

fn default_index_strategy() -> String { "hybrid".to_string() } // hybrid, semantic, keyword
fn default_max_snippets() -> usize { 5 }
fn default_embedding_model() -> String { "text-embedding-ada-002".to_string() }

impl Default for RagConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            indexing_strategy: default_index_strategy(),
            max_retrieved_snippets: default_max_snippets(),
            embedding_model: default_embedding_model(),
            cache_dir: None,
        }
    }
}

/// VS Code Integration Configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VscodeConfig {
    /// Auto-detect VS Code installation
    #[serde(default = "default_true")]
    pub auto_detect: bool,
    
    /// VS Code extensions directory
    #[serde(default)]
    pub extensions_dir: Option<String>,
    
    /// Enable VS Code extension host communication
    #[serde(default = "default_true")]
    pub enable_extension_host: bool,
    
    /// Custom VS Code executable path
    #[serde(default)]
    pub vscode_executable_path: Option<String>,
    
    /// Enable inline completions (Tab completion like Copilot)
    #[serde(default = "default_true")]
    pub inline_completion_enabled: bool,
    
    /// Enable chat panel integration
    #[serde(default = "default_true")]
    pub chat_panel_enabled: bool,
    
    /// Enable terminal integration
    #[serde(default = "default_true")]
    pub terminal_integration: bool,
}

impl Default for VscodeConfig {
    fn default() -> Self {
        Self {
            auto_detect: true,
            extensions_dir: None,
            enable_extension_host: true,
            vscode_executable_path: None,
            inline_completion_enabled: true,
            chat_panel_enabled: true,
            terminal_integration: true,
        }
    }
}

/// Performance Tuning Configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerformanceConfig {
    /// Number of worker threads for async runtime
    #[serde(default = "default_worker_threads")]
    pub worker_threads: usize,
    
    /// Max concurrent requests per client
    #[serde(default = "default_max_concurrent")]
    pub max_concurrent_requests: usize,
    
    /// Connection keep-alive timeout
    #[serde(default = "default_keepalive")]
    pub keep_alive_secs: u64,
    
    /// Enable response caching
    #[serde(default = "default_true")]
    pub enable_cache: bool,
    
    /// Cache TTL in seconds
    #[serde(default = "default_cache_ttl")]
    pub cache_ttl_secs: u64,
    
    /// Stream buffer size
    #[serde(default = "default_buffer_size")]
    pub stream_buffer_size: usize,
}

fn default_worker_threads() -> usize { num_cpus::get() }
fn default_max_concurrent() -> usize { 100 }
fn default_keepalive() -> u64 { 75 }
fn default_cache_ttl() -> u64 { 300 } // 5 minutes
fn default_buffer_size() -> usize { 8192 } // 8KB

impl Default for PerformanceConfig {
    fn default() -> Self {
        Self {
            worker_threads: default_worker_threads(),
            max_concurrent_requests: default_max_concurrent(),
            keep_alive_secs: default_keepalive(),
            enable_cache: true,
            cache_ttl_secs: default_cache_ttl(),
            stream_buffer_size: default_buffer_size(),
        }
    }
}

/// Logging Configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoggingConfig {
    /// Log level (trace, debug, info, warn, error)
    #[serde(default = "default_log_level")]
    pub level: String,
    
    /// Log file path (None for stdout only)
    #[serde(default)]
    pub log_file: Option<String>,
    
    /// Enable JSON formatting for logs
    #[serde(default)]
    pub json_format: bool,
    
    /// Include source code location in logs
    #[serde(default = "default_true")]
    pub include_source_location: bool,
    
    /// Request/response logging
    #[serde(default = "default_true")]
    pub log_requests: bool,
}

fn default_log_level() -> String { "info".to_string() }

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: default_log_level(),
            log_file: None,
            json_format: false,
            include_source_location: true,
            log_requests: true,
        }
    }
}

/// Predefined configuration profiles for common use cases
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ConfigProfile {
    /// Development mode - verbose logging, hot reload
    Development,
    
    /// Production mode - optimized performance, minimal logging
    Production,
    
    /// Debug mode - maximum verbosity, all features enabled
    Debug,
    
    /// Minimal mode - lowest resource usage
    Minimal,
    
    /// Cursor-compatible mode - match Cursor's behavior exactly
    CursorCompatible,
}

impl ConfigProfile {
    /// Get profile name as string
    pub fn name(&self) -> &'static str {
        match self {
            Self::Development => "development",
            Self::Production => "production",
            Self::Debug => "debug",
            Self::Minimal => "minimal",
            Self::CursorCompatible => "cursor-compatible",
        }
    }
    
    /// Apply profile to base configuration
    pub fn apply_to(&self, mut config: JcodeConfig) -> JcodeConfig {
        match self {
            Self::Development => {
                config.logging.level = "debug".to_string();
                config.logging.include_source_location = true;
                config.logging.log_requests = true;
                config.performance.enable_cache = false; // Disable cache during dev
                config.vscode.auto_detect = true;
            }
            Self::Production => {
                config.logging.level = "warn".to_string();
                config.logging.log_file = Some("jcode-production.log".to_string());
                config.logging.json_format = true;
                config.performance.enable_cache = true;
                config.performance.cache_ttl_secs = 600; // 10 minutes
                config.rest.enable_docs = false; // Disable docs in production
            }
            Self::Debug => {
                config.logging.level = "trace".to_string();
                config.rag.enabled = true;
                config.logging.log_requests = true;
                config.performance.worker_threads = 1; // Single thread easier to debug
            }
            Self::Minimal => {
                config.logging.level = "error".to_string();
                config.rag.enabled = false;
                config.performance.max_concurrent_requests = 10;
                config.performance.enable_cache = false;
            }
            Self::CursorCompatible => {
                // Match Cursor's defaults as closely as possible
                config.llm.default_model = "gpt-4".to_string(); // Or Claude
                config.llm.default_provider = "openai-compatible".to_string();
                config.vscode.inline_completion_enabled = true;
                config.vscode.chat_panel_enabled = true;
                config.rest.port = 3000; // Same as Cursor Agent's default
                config.performance.stream_buffer_size = 4096; // Match Cursor's streaming
            }
        }
        
        config
    }
}
