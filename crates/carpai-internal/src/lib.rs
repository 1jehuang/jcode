//! # CarpAI Internal API Layer
//!
//! This crate defines the internal trait interfaces that decouple:
//! - Agent runtime from concrete completion/inference engines
//! - TUI from direct provider dependencies
//! - Enterprise features from core logic
//! - CLI mode from Server mode
//!
//! ## Architecture (Phase 1 — Trait Layer)
//!
//! ```
//! IDE Plugin ──gRPC──> CarpAI Server ──REST/WS──> Web UI
//!                               │
//!                          Internal API (trait objects)
//!                               │
//!                     ┌─────────┼──────────────────┐
//!                     ▼         ▼                  ▼
//!               SessionStore  ToolExecutor    InferenceBackend
//!               VirtualFS     EventBus        MemoryBackend
//!               CodeCompletion AuthProvider
//!                                          AgentContext (assembly)
//! ```
//!
//! ## Module Map
//!
//! | Module | Trait | Purpose |
//! |--------|-------|---------|
//! | `completion` | `CodeCompletion` | Inline/chat code completion |
//! | `auth` | `AuthProvider` | JWT/API-key auth + RBAC |
//! | `inference` | `InferenceEngine` | Base LLM inference |
//! | `inference_backend` | `InferenceBackend` | Enhanced: routing, quota, fallback |
//! | `memory` | `MemoryStore` | Base memory persistence |
//! | `memory_backend` | `MemoryBackend` | Enhanced: vector search, dedup, tiers |
//! | `tools` | `ToolRegistry` | Tool discovery (existing) |
//! | `tool_executor` | `ToolExecutor` | Tool execution with sandboxing + audit |
//! | `session` | `SessionStore` | Session CRUD + message append + state machine |
//! | `filesystem` | `VirtualFileSystem` | File ops with path sandboxing + audit |
//! | `event_bus` | `EventBus` | Pub/sub event system (in-process / Redis / Kafka) |
//! | `agent_context` | `AgentContext` | Central DI container assembling all traits |

// --- Base Traits (pre-existing) ---
pub mod completion;
pub mod auth;
pub mod inference;
pub mod memory;
pub mod tools;

// --- Phase 1 New Traits ---
pub mod session;
pub mod tool_executor;
pub mod inference_backend;
pub mod filesystem;
pub mod event_bus;
pub mod memory_backend;
pub mod agent_context;

// ========================================================================
// Re-exports — Public API surface
// ========================================================================

// --- Base traits (backward compatible) ---
pub use completion::{CodeCompletion, CompletionCandidate, CompletionRequest};
pub use auth::{
    AuthProvider, AuthToken, UserInfo, Permission,
    ApiKeyValidator, UserTier, AuthError,
};
pub use inference::{
    InferenceEngine, InferenceRequest, InferenceResponse,
    TokenUsage, ModelInfo, HealthStatus,
};
pub use memory::{MemoryStore, MemoryEntry, MemoryQuery, MemoryError, MemoryType};
pub use tools::{ToolRegistry, ToolDefinition, ToolExecution};

// --- Phase 1 new traits ---
pub use session::{
    SessionStore, SessionId, SessionState, SessionMeta,
    StoredMessage, ContentBlock, MessageRole,
    LoadedSession, SessionFilter, CompactionSnapshot,
    SessionError, SessionMetaUpdate,
};
pub use tool_executor::{
    ToolExecutor, ToolRequest, ToolResponse, ToolSchema,
    ToolCategory, ExecutionMode, ToolContext,
    ToolExecError, ToolExecutionRecord, ValidationResult,
};
pub use inference_backend::{
    InferenceBackend, ChatCompletionRequest, ChatCompletionResponse,
    ChatMessage, ChatRole, ChatContent, StreamChunk,
    RoutedModelInfo, QuotaUsage, FallbackInfo,
    ModelSelectionConstraints, InferenceUserTier,
    // Response types
    LogProbs, TokenLogProb, TopLogProb, Choice,
    CompletionTokenUsage,
    // Streaming types
    StreamChunkType,
    // Fallback types
    FallbackReason,
};
pub use filesystem::{
    VirtualFileSystem, FsError, FileMeta, FileEntry,
    FileWriteResult, SearchResult, ContentMatch,
    SearchOptions, FsEvent,
};
pub use event_bus::{
    EventBus, BusSubscriber, BusEvent, BusHealth, EventBusError,
    BusEventEnvelope,
    // Built-in events
    SessionCreated, SessionMessagesAppended, SessionStateChanged,
    AgentTurnStarted, AgentTurnCompleted, ToolExecuted,
    FileModified, FileOperationType,
    InferenceCompleted,
    SystemHealthChanged, SystemStatus,
};
pub use memory_backend::{
    MemoryBackend, EnhancedMemoryEntry, EnhancedMemoryQuery,
    VectorSearchResult, Reinforcement, MemoryScope, TrustLevel,
    EnhancedMemoryStats, CleanupOptions, CleanupResult,
    EnhancedMemoryUpdate, VectorSearchOptions,
};

// --- Assembly ---
pub use agent_context::{
    AgentContext, AgentContextBuilder, AppConfig, AppMode,
    RequestMetadata,
};
