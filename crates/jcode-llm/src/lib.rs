//! jcode-llm: LLM Provider Integration
//!
//! ## Overview
//!
//! This crate provides a unified abstraction layer for integrating with various LLM providers,
//! supporting both cloud APIs (Deepseek) and local deployments (vLLM, llama.cpp).
//!
//! ## Supported Providers
//!
//! - **Deepseek**: Deepseek-V4-flash, Deepseek-R1, etc. (via cloud API)
//! - **vLLM**: High-throughput local serving with OpenAI-compatible API
//! - **llama.cpp**: Lightweight local inference server (OpenAI-compatible)
//! - **OpenAI Compatible**: Any OpenAI-compatible API endpoint
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────┐
//! │                  LLM Provider Layer               │
//!  ┌──────────┐  ┌──────────┐  ┌──────────────────────┐  
//!  │ Deepseek │  │   vLLM   │  │   llama.cpp         │  
//!  │ (Cloud)  │  │ (Local)  │  │   (Local)           │  
//!  └──────────┘  └──────────┘  └──────────────────────┘  
//!         │              │                │             
//!         ▼              ▼                ▼             
//!  ┌─────────────────────────────────────────────────┐   
//!  │           Unified LLM Client                   │   
//!  │  - Chat Completion (sync + streaming)          │   
//!  │  - Function Calling                           │   
//!  │  - Embeddings                                  │   
//!  │  - Token Counting                              │   
//!  └─────────────────────────────────────────────────┘   
//! ```

pub mod provider;
pub mod types;
pub mod config;
pub mod error;
pub mod rest_api;

// Re-exports for convenience
pub use types::*;
pub use provider::{
    LlmProvider, 
    DeepseekProvider, 
    OpenAiCompatibleProvider,
    LlmProviderFactory,
};
pub use config::LlmConfig;
pub use error::{LlmError, LlmResult};

/// Pre-built configurations for common providers
pub mod presets {
    use crate::{types::ProviderType, config::LlmConfig};
    
    /// Deepseek Chat (general purpose, cost-effective)
    pub fn deepseek_chat() -> LlmConfig {
        LlmConfig {
            provider_type: ProviderType::Deepseek,
            model_name: "deepseek-chat".to_string(),
            api_base_url: Some("https://api.deepseek.com/v1".to_string()),
            api_key_env: "DEEPSEEK_API_KEY".to_string(),
            max_tokens: 8192,
            temperature: 0.7,
            ..Default::default()
        }
    }

    /// Deepseek R1 (reasoning model with chain-of-thought)
    pub fn deepseek_r1() -> LlmConfig {
        LlmConfig {
            provider_type: ProviderType::Deepseek,
            model_name: "deepseek-reasoner".to_string(),
            api_base_url: Some("https://api.deepseek.com/v1".to_string()),
            api_key_env: "DEEPSEEK_API_KEY".to_string(),
            max_tokens: 16384,
            temperature: 0.0, // Deterministic for reasoning models
            ..Default::default()
        }
    }

    /// Local vLLM deployment (e.g., Qwen2.5-72B-Instruct-AWQ)
    pub fn local_vllm_qwen2_5_72b(port: u16) -> LlmConfig {
        LlmConfig::local_vllm("Qwen2.5-72B-Instruct-AWQ", port)
    }

    /// Local vLLM deployment (e.g., DeepSeek-V3)
    pub fn local_vllm_deepseek_v3(port: u16) -> LlmConfig {
        LlmConfig::local_vllm("DeepSeek-V3", port)
    }

    /// Local llama.cpp server (lightweight, good for CPU inference)
    pub fn local_llamacpp_qwen2_5_7b(port: u16) -> LlmConfig {
        LlmConfig::local_llamacpp("qwen2.5:7b", port)
    }

    /// Local llama.cpp with Deepseek-R1-Distill-Qwen-32B
    pub fn local_llamacpp_deepseek_r1_32b(port: u16) -> LlmConfig {
        LlmConfig::local_llamacpp("deepseek-r1-distill-qwen:32b", port)
    }
}
