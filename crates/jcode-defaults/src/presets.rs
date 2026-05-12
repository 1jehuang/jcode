//! Quick-Start Presets and Optimization Profiles for CarpAI
//!
//! Provides one-click configuration presets for common use cases,
//! making CarpAI as easy to start as Cursor.

use serde::{Deserialize, Serialize};
use crate::config::JcodeConfig;

/// Pre-configured quick-start scenarios
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuickStartPreset {
    pub name: String,
    pub display_name: String,
    pub description: String,
    pub icon: String, // emoji for UI display
    pub config: JcodeConfig,
    pub prerequisites: Vec<String>,
    pub estimated_setup_time: String,
}

impl QuickStartPreset {
    /// Get all available CarpAI presets
    pub fn all() -> Vec<Self> {
        vec![
            Self::carpai_default_preset(),
            Self::deepseek_cloud_preset(),
            Self::openai_compatible_preset(),
            Self::local_vllm_preset(),
            Self::cursor_migration_preset(),
            Self::development_mode_preset(),
            Self::production_mode_preset(),
        ]
    }
    
    /// Find preset by name
    pub fn find(name: &str) -> Option<Self> {
        Self::all().into_iter().find(|p| p.name == name)
    }
    
    /// 🚀 **CarpAI Default Preset** (Recommended - Auto-detect optimal config)
    pub fn carpai_default_preset() -> Self {
        let mut config = JcodeConfig::zero_config();
        
        // Smart defaults that work for 80% of users
        config.llm.auto_detect_api_key = true;
        config.vscode.inline_completion_enabled = true;
        config.vscode.chat_panel_enabled = true;
        config.rag.enabled = true;
        
        // Performance optimized defaults
        config.performance.stream_buffer_size = 4096;
        config.performance.enable_cache = true;
        config.logging.level = "info".to_string();
        
        Self {
            name: "carpai-default".to_string(),
            display_name: "CarpAI Default".to_string(),
            description: "Smart auto-configuration (detects best provider and settings automatically)".to_string(),
            icon: "🚀".to_string(),
            config,
            prerequisites: vec![], // No requirements - truly zero-config!
            estimated_setup_time: "< 30 seconds (auto-detected)".to_string(),
        }
    }
    
    /// ☁️ Deepseek Cloud Preset (Best value for Chinese users)
    pub fn deepseek_cloud_preset() -> Self {
        let mut config = JcodeConfig::zero_config();
        
        config.llm.default_provider = "deepseek".to_string();
        config.llm.default_model = "deepseek-chat".to_string();
        config.llm.timeout_secs = 60;
        
        config.performance.stream_buffer_size = 4096;
        config.rag.max_retrieved_snippets = 8;
        
        Self {
            name: "deepseek-cloud".to_string(),
            display_name: "DeepSeek Cloud".to_string(),
            description: "Use DeepSeek cloud API (best value, excellent Chinese support)".to_string(),
            icon: "☁️".to_string(),
            config,
            prerequisites: vec![
                "DEEPSEEK_API_KEY environment variable set".to_string(),
                "Network access to api.deepseek.com".to_string(),
            ],
            estimated_setup_time: "< 1 minute".to_string(),
        }
    }
    
    /// 🤖 OpenAI-Compatible Preset
    pub fn openai_compatible_preset() -> Self {
        let mut config = JcodeConfig::zero_config();
        
        config.llm.default_provider = "openai-compatible".to_string();
        config.llm.default_model = "gpt-4-turbo".to_string();
        
        config.llm.providers.insert("openai-compatible".to_string(), 
            crate::config::ProviderSpecificConfig {
                api_base_url: Some("https://api.openai.com/v1".to_string()),
                ..Default::default()
            });
        
        config.rest.port = 3000;
        config.vscode.inline_completion_enabled = true;
        
        Self {
            name: "openai-compatible".to_string(),
            display_name: "OpenAI Compatible".to_string(),
            description: "Connect to OpenAI API or any OpenAI-compatible service".to_string(),
            icon: "🤖".to_string(),
            config,
            prerequisites: vec![
                "OPENAI_API_KEY environment variable set".to_string(),
                "Or configure custom OpenAI-compatible endpoint".to_string(),
            ],
            estimated_setup_time: "< 2 minutes".to_string(),
        }
    }
    
    /// 💻 Local vLLM Preset (Privacy-first)
    pub fn local_vllm_preset() -> Self {
        let mut config = JcodeConfig::zero_config();
        
        config.llm.default_provider = "vllm".to_string();
        config.llm.default_model = "Qwen2.5-72B-Instruct-AWQ".to_string();
        
        config.grpc.address = "127.0.0.1".to_string();
        config.rest.address = "127.0.0.1".to_string();
        config.performance.worker_threads = 4;
        config.performance.max_concurrent_requests = 20;
        config.logging.level = "info".to_string();
        
        config.llm.providers.insert("vllm".to_string(),
            crate::config::ProviderSpecificConfig {
                api_base_url: Some("http://localhost:8000/v1".to_string()),
                api_key: None,
                ..Default::default()
            });
        
        Self {
            name: "local-vllm".to_string(),
            display_name: "Local vLLM".to_string(),
            description: "Run local vLLM server (complete privacy, no API costs)".to_string(),
            icon: "💻".to_string(),
            config,
            prerequisites: vec![
                "vLLM server running on localhost:8000".to_string(),
                "GPU with at least 8GB VRAM recommended".to_string(),
                "Download model weights first".to_string(),
            ],
            estimated_setup_time: "5-10 minutes (model download)".to_string(),
        }
    }
    
    /// 🔄 Cursor Migration Preset (Drop-in replacement)
    pub fn cursor_migration_preset() -> Self {
        let mut config = JcodeConfig::zero_config();
        
        config.llm.default_model = "gpt-4".to_string();
        config.llm.default_provider = "openai-compatible".to_string();
        
        config.rest.port = 3000;
        config.vscode.inline_completion_enabled = true;
        config.vscode.chat_panel_enabled = true;
        config.vscode.terminal_integration = true;
        
        config.performance.stream_buffer_size = 4096;
        config.performance.keep_alive_secs = 75;
        config.logging.log_requests = false;
        
        Self {
            name: "cursor-migration".to_string(),
            display_name: "Cursor Migration".to_string(),
            description: "Migrate from Cursor with minimal changes (drop-in replacement)".to_string(),
            icon: "🔄".to_string(),
            config,
            prerequisites: vec![
                "Existing OPENAI_API_KEY or equivalent".to_string(),
                "VS Code installed (auto-detected)".to_string(),
                "Export your Cursor settings (optional)".to_string(),
            ],
            estimated_setup_time: "< 5 minutes".to_string(),
        }
    }
    
    /// 🔧 Development Mode Preset
    pub fn development_mode_preset() -> Self {
        let mut config = JcodeConfig::zero_config();
        
        config.logging.level = "debug".to_string();
        config.logging.include_source_location = true;
        config.logging.log_requests = true;
        
        config.performance.enable_cache = false;
        config.performance.worker_threads = 2;
        
        config.rest.enable_docs = true;
        config.rag.enabled = true;
        
        Self {
            name: "development".to_string(),
            display_name: "Development Mode".to_string(),
            description: "Full debug mode with verbose logging and hot-reload features".to_string(),
            icon: "🔧".to_string(),
            config,
            prerequisites: vec![],
            estimated_setup_time: "Instant".to_string(),
        }
    }
    
    /// 🏭 Production Mode Preset
    pub fn production_mode_preset() -> Self {
        let mut config = JcodeConfig::zero_config();
        
        config.logging.level = "warn".to_string();
        config.logging.json_format = true;
        config.logging.log_file = Some("/var/log/carpai/carpai.log".to_string());
        
        config.performance.enable_cache = true;
        config.performance.cache_ttl_secs = 600;
        config.performance.max_concurrent_requests = 100;
        
        config.rest.cors_origins = vec!["your-domain.com".to_string()];
        config.rest.rate_limit_rpm = 1000;
        config.rest.enable_docs = false;
        
        config.grpc.tls_enabled = true;
        
        Self {
            name: "production".to_string(),
            display_name: "Production Mode".to_string(),
            description: "Optimized for production deployment with security and performance tuning".to_string(),
            icon: "🏭".to_string(),
            config,
            prerequisites: vec![
                "TLS certificates configured".to_string(),
                "Reverse proxy (nginx/caddy) setup".to_string(),
                "Systemd service or Docker container".to_string(),
            ],
            estimated_setup_time: "10-15 minutes".to_string(),
        }
    }
}
