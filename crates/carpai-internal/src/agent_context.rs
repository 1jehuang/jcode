//! Agent Context — The central assembly of all CarpAI traits
//!
//! This is the **primary dependency injection point** for the entire system.
//! Every agent turn, every API handler, every background worker receives
//! an `AgentContext` (or its server equivalent `ServerContext`) that gives
//! it access to all backend services through trait objects.
//!
//! ## Architecture
//!
//! ```
//! ┌─────────────────────────────────────────────────────┐
//! │                   AgentContext                       │
//! │                                                      │
//! │  ┌──────────┐  ┌──────────┐  ┌──────────────────┐   │
//! │  │ Session  │  │  Tool    │  │ InferenceBackend  │   │
//! │  │ Store    │  │ Executor │  │ (routing+quota)  │   │
//! │  └──────────┘  └──────────┘  └──────────────────┘   │
//! │                                                      │
//! │  ┌──────────┐  ┌──────────┐  ┌──────────────────┐   │
//! │  │ Virtual  │  │ Event    │  │ MemoryBackend     │   │
//! │  │ FileSys  │  │ Bus      │  │ (vector+dedup)   │   │
//! │  └──────────┘  └──────────┘  └──────────────────┘   │
//! │                                                      │
//! │  ┌──────────┐  ┌──────────┐                         │
//! │  │ Code     │  │ Auth     │                         │
//! │  │ Completion│ Provider │                         │
//! │  └──────────┘  └──────────┘                         │
//! │                                                      │
//! │  config: AppConfig                                    │
//! │  tenant_id: Option<String>                            │
//! │  user_id: String                                      │
//! └─────────────────────────────────────────────────────┘
//! ```
//!
//! ## Usage Patterns
//!
//! ### CLI Mode (`carpai-cli`)
//! ```rust
//! let ctx = AgentContext::for_cli(config).await?;
//! // All backends are local: LocalFileSessionStore, LocalToolExecutor, etc.
//! run_agent_loop(ctx).await?;
//! ```
//!
//! ### Server Mode (`carpai-server`)
//! ```rust
//! let ctx = AgentContext::for_server(config).await?;
//! // All backends are enterprise: PgSessionStore, SandboxToolExecutor, etc.
//! axum::serve(app.into_make_service(), ctx);
//! ```

use std::path::PathBuf;
use std::sync::Arc;
use serde::{Deserialize, Serialize};

// Import all trait modules (used in AgentContext struct fields)
use crate::session::SessionStore; // SessionId: unused here, available via re-export from lib.rs
use crate::tool_executor::{ToolExecutor, ExecutionMode};
use crate::inference_backend::{InferenceBackend};
use crate::filesystem::{VirtualFileSystem};
use crate::event_bus::{EventBus};
use crate::memory_backend::{MemoryBackend};
// Import existing base traits
use crate::completion::CodeCompletion;
use crate::auth::AuthProvider;

// ========================================================================
// Configuration
// ========================================================================

/// Application configuration shared across all contexts
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    /// Application name / mode identifier
    pub mode: AppMode,

    /// Data root directory (sessions, memory, cache)
    pub data_dir: PathBuf,

    /// Working directory (project root)
    pub working_dir: PathBuf,

    /// Default model for inference
    pub default_model: String,

    /// Maximum context window for the agent
    pub max_context_tokens: usize,

    /// Whether to enable tool execution
    pub tools_enabled: bool,

    /// Default execution mode for tools
    pub default_tool_mode: ExecutionMode,

    /// Enable file system access via VFS
    pub vfs_enabled: bool,

    /// Root path for VFS (limits all file operations to this tree)
    pub vfs_root: Option<PathBuf>,

    /// Enable memory/persistence features
    pub memory_enabled: bool,

    /// Enable event bus
    pub event_bus_enabled: bool,
}

/// Application operating mode
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AppMode {
    /// Standalone CLI client (local everything)
    Cli,
    /// Enterprise server (remote backends, multi-tenant)
    Server,
    /// Hybrid — local UI but connects to a remote server
    Client,
}

impl std::fmt::Display for AppMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cli => write!(f, "cli"),
            Self::Server => write!(f, "server"),
            Self::Client => write!(f, "client"),
        }
    }
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            mode: AppMode::Cli,
            data_dir: PathBuf::from(".jcode/data"),
            working_dir: PathBuf::from("."),
            default_model: "default".into(),
            max_context_tokens: 200_000,
            tools_enabled: true,
            default_tool_mode: ExecutionMode::Local,
            vfs_enabled: true,
            vfs_root: None,
            memory_enabled: true,
            event_bus_enabled: true,
        }
    }
}

// ========================================================================
// The Central Context Struct
// ========================================================================

/// Agent runtime context — holds references to ALL backend services
///
/// This is the only struct that needs to be threaded through the agent
/// loop. It uses `Arc<dyn Trait>` for each backend so implementations
/// can be swapped at startup without changing any downstream code.
///
/// **Thread safety**: All fields are `Arc<dyn Trait>` where `Trait: Send + Sync`,
/// so `AgentContext` is cheaply cloneable and safe to share across tokio tasks.
#[derive(Clone)]
pub struct AgentContext {
    // --- Core Services ---
    
    /// Session persistence backend
    pub sessions: Arc<dyn SessionStore>,
    
    /// Tool execution engine (with sandboxing/permissions)
    pub tools: Arc<dyn ToolExecutor>,
    
    /// Inference backend (with routing, quota, fallback)
    pub inference: Arc<dyn InferenceBackend>,
    
    /// Virtual file system (with security sandboxing)
    pub fs: Arc<dyn VirtualFileSystem>,
    
    /// Event bus (pub/sub for real-time updates)
    pub events: Arc<dyn EventBus>,
    
    /// Memory backend (vector search + dedup + tiers)
    pub memory: Arc<dyn MemoryBackend>,
    
    /// Code completion engine (inline completions for IDE)
    pub completion: Option<Arc<dyn CodeCompletion>>,
    
    /// Authentication & authorization provider
    pub auth: Arc<dyn AuthProvider>,

    // --- Identity & Scope ---

    /// Application configuration
    pub config: AppConfig,

    /// Current user ID (set per-request in server mode)
    pub user_id: String,

    /// Current session ID (set at start of each agent turn)
    pub session_id: Option<String>,

    /// Tenant ID (server mode multi-tenancy)
    pub tenant_id: Option<String>,

    /// Request-scoped metadata (correlation IDs, client IP, etc.)
    pub request_metadata: RequestMetadata,
}

/// Per-request metadata for tracing and audit
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RequestMetadata {
    /// Correlation ID for distributed tracing
    #[serde(default)]
    pub correlation_id: Option<String>,

    /// Client IP address
    #[serde(default)]
    pub client_ip: Option<String>,

    /// User agent string
    #[serde(default)]
    pub user_agent: Option<String>,

    /// API key identifier (not the key itself!)
    #[serde(default)]
    pub api_key_id: Option<String>,

    /// Arbitrary tags for filtering
    #[serde(default)]
    pub tags: Vec<String>,
}

impl AgentContext {
    /// Create a new AgentContext with all required services
    ///
    /// This is typically called once at startup and then cloned per-request.
    pub fn new(
        config: AppConfig,
        sessions: Arc<dyn SessionStore>,
        tools: Arc<dyn ToolExecutor>,
        inference: Arc<dyn InferenceBackend>,
        fs: Arc<dyn VirtualFileSystem>,
        events: Arc<dyn EventBus>,
        memory: Arc<dyn MemoryBackend>,
        completion: Option<Arc<dyn CodeCompletion>>,
        auth: Arc<dyn AuthProvider>,
        user_id: String,
    ) -> Self {
        Self {
            config,
            sessions,
            tools,
            inference,
            fs,
            events,
            memory,
            completion,
            auth,
            user_id,
            session_id: None,
            tenant_id: None,
            request_metadata: RequestMetadata::default(),
        }
    }

    /// Create a child context for a specific session
    ///
    /// Clones all service pointers but sets the session_id.
    pub fn for_session(&self, session_id: &str) -> Self {
        let mut ctx = self.clone();
        ctx.session_id = Some(session_id.to_string());
        ctx
    }

    /// Create a child context for a specific request (server mode)
    ///
    /// Sets user_id, tenant_id, and request_metadata.
    pub fn for_request(
        &self,
        user_id: &str,
        tenant_id: Option<&str>,
        metadata: RequestMetadata,
    ) -> Self {
        let mut ctx = self.clone();
        ctx.user_id = user_id.to_string();
        ctx.tenant_id = tenant_id.map(|s| s.to_string());
        ctx.request_metadata = metadata;
        ctx
    }

    /// Quick check: is this running in server mode?
    pub fn is_server(&self) -> bool { self.config.mode == AppMode::Server }

    /// Quick check: is this running in CLI mode?
    pub fn is_cli(&self) -> bool { self.config.mode == AppMode::Cli }

    /// Get current session ID or panic if not set
    pub fn require_session_id(&self) -> &str {
        self.session_id.as_deref()
            .expect("AgentContext::require_session_id called but session_id is not set")
    }

    /// Publish an event to the event bus (convenience method)
    pub async fn publish_event<E: crate::event_bus::BusEvent>(&self, event: E) {
        use crate::event_bus::EventBusExt;
        let _ = self.events.publish(event).await;
    }

    /// Check if the current user has permission (convenience method)
    pub async fn has_permission(
        &self,
        permission: &crate::auth::Permission,
    ) -> Result<bool, crate::auth::AuthError> {
        self.auth.check_permission(&self.user_id, permission).await
    }
}

// ========================================================================
// Builder Pattern
// ========================================================================

/// Builder for constructing AgentContext with validation
pub struct AgentContextBuilder {
    config: AppConfig,
    sessions: Option<Arc<dyn SessionStore>>,
    tools: Option<Arc<dyn ToolExecutor>>,
    inference: Option<Arc<dyn InferenceBackend>>,
    fs: Option<Arc<dyn VirtualFileSystem>>,
    events: Option<Arc<dyn EventBus>>,
    memory: Option<Arc<dyn MemoryBackend>>,
    completion: Option<Arc<dyn CodeCompletion>>,
    auth: Option<Arc<dyn AuthProvider>>,
    user_id: String,
}

impl AgentContextBuilder {
    pub fn new(config: AppConfig) -> Self {
        Self {
            config,
            sessions: None,
            tools: None,
            inference: None,
            fs: None,
            events: None,
            memory: None,
            completion: None,
            auth: None,
            user_id: "system".into(),
        }
    }

    pub fn with_sessions(mut self, s: Arc<dyn SessionStore>) -> Self { self.sessions = Some(s); self }
    pub fn with_tools(mut self, t: Arc<dyn ToolExecutor>) -> Self { self.tools = Some(t); self }
    pub fn with_inference(mut self, i: Arc<dyn InferenceBackend>) -> Self { self.inference = Some(i); self }
    pub fn with_fs(mut self, f: Arc<dyn VirtualFileSystem>) -> Self { self.fs = Some(f); self }
    pub fn with_events(mut self, e: Arc<dyn EventBus>) -> Self { self.events = Some(e); self }
    pub fn with_memory(mut self, m: Arc<dyn MemoryBackend>) -> Self { self.memory = Some(m); self }
    pub fn with_completion(mut self, c: Arc<dyn CodeCompletion>) -> Self { self.completion = Some(c); self }
    pub fn with_auth(mut self, a: Arc<dyn AuthProvider>) -> Self { self.auth = Some(a); self }
    pub fn with_user_id(mut self, uid: &str) -> Self { self.user_id = uid.to_string(); self }

    /// Build the context, validating that all required services are present
    pub fn build(self) -> Result<AgentContext, &'static str> {
        let sessions = self.sessions.ok_or("SessionStore is required")?;
        let tools = self.tools.ok_or("ToolExecutor is required")?;
        let inference = self.inference.ok_or("InferenceBackend is required")?;
        let fs = self.fs.ok_or("VirtualFileSystem is required")?;
        let events = self.events.ok_or("EventBus is required")?;
        let memory = self.memory.ok_or("MemoryBackend is required")?;
        let auth = self.auth.ok_or("AuthProvider is required")?;

        Ok(AgentContext::new(
            self.config, sessions, tools, inference, fs, events, memory,
            self.completion, auth, self.user_id,
        ))
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_app_config_default() {
        let cfg = AppConfig::default();
        assert_eq!(cfg.mode, AppMode::Cli);
        assert!(cfg.tools_enabled);
    }

    #[test]
    fn test_app_mode_display() {
        assert_eq!(AppMode::Cli.to_string(), "cli");
        assert_eq!(AppMode::Server.to_string(), "server");
    }

    #[test]
    fn test_request_metadata_default() {
        let meta = RequestMetadata::default();
        assert!(meta.correlation_id.is_none());
        assert!(meta.tags.is_empty());
    }

    #[test]
    fn test_builder_requires_all_fields() {
        let cfg = AppConfig::default();
        let builder = AgentContextBuilder::new(cfg);
        // Missing all services → should fail
        assert!(builder.build().is_err());
    }
}
