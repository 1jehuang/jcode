//! Server context - wraps AgentContext for server mode
//!
//! This module provides the ServerContext which is the main dependency injection
//! point for all gRPC/REST/WS handlers in server mode.

use std::sync::Arc;
use carpai_core::{AgentContext, AppConfig, AppMode};
use carpai_core::config::CoreConfig;
use tracing::info;

/// Server context holding all backend services
///
/// This is created once at startup and cloned (cheaply via Arc) per request.
#[derive(Clone)]
pub struct ServerContext {
    /// The core agent context with all trait objects
    pub agent_ctx: Arc<AgentContext>,

    /// Server configuration
    pub config: ServerAppConfig,
}

/// Server-specific application configuration
#[derive(Debug, Clone)]
pub struct ServerAppConfig {
    /// JWT secret key for token signing/validation
    pub jwt_secret: String,

    /// Valid API keys for authentication
    pub api_keys: Vec<String>,

    /// Server listen address
    pub listen_addr: String,

    /// CORS allowed origins
    pub cors_origins: Vec<String>,

    /// Enable/disable gRPC reflection
    pub grpc_reflection_enabled: bool,

    /// Maximum request body size (bytes)
    pub max_request_size: usize,

    /// Request timeout (seconds)
    pub request_timeout_secs: u64,
}

impl Default for ServerAppConfig {
    fn default() -> Self {
        Self {
            jwt_secret: "dev-secret-change-in-production".to_string(),
            api_keys: vec!["dev-api-key".to_string()],
            listen_addr: "[::1]:50051".to_string(),
            cors_origins: vec!["*".to_string()],
            grpc_reflection_enabled: true,
            max_request_size: 10 * 1024 * 1024, // 10MB
            request_timeout_secs: 300, // 5 minutes
        }
    }
}

impl ServerContext {
    /// Create a new ServerContext from CoreConfig
    ///
    /// This builds all local implementations and wraps them in a server-ready context.
    pub async fn from_config(core_config: &CoreConfig) -> anyhow::Result<Self> {
        info!("Building server context from config");

        // Build the agent context using carpai-core's builder
        let agent_ctx = carpai_core::build_local_agent_context(core_config);

        let server_config = ServerAppConfig::default();

        Ok(Self {
            agent_ctx: Arc::new(agent_ctx),
            config: server_config,
        })
    }

    /// Create a minimal ServerContext for testing purposes
    ///
    /// This is used when gRPC services need a context but full initialization
    /// is not required.
    pub fn default_for_test() -> Self {
        use carpai_core::{AppConfig, AppMode};
        let app_config = AppConfig {
            mode: AppMode::Server,
            ..Default::default()
        };
        let agent_ctx = carpai_internal::AgentContextBuilder::new(app_config)
            .build()
            .expect("Failed to build test AgentContext");

        Self {
            agent_ctx: Arc::new(agent_ctx),
            config: ServerAppConfig::default(),
        }
    }

    /// Create a child context for a specific user request
    pub fn for_user(&self, user_id: &str, tenant_id: Option<&str>) -> Self {
        let child_agent_ctx = self.agent_ctx.for_request(user_id, tenant_id, Default::default());

        Self {
            agent_ctx: Arc::new(child_agent_ctx),
            config: self.config.clone(),
        }
    }

    /// Get the current user ID from the context
    pub fn user_id(&self) -> &str {
        &self.agent_ctx.user_id
    }

    /// Get the current tenant ID from the context
    pub fn tenant_id(&self) -> Option<&str> {
        self.agent_ctx.tenant_id.as_deref()
    }
}
