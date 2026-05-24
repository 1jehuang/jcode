//! CarpAI Core - Business Logic Layer (Layer 1)
//!
//! This crate contains all business logic implementations for the CarpAI system.
//! It depends on `carpai-internal` (Layer 0: Pure Traits) and provides concrete
//! implementations using local storage and execution for CLI/development mode.
//!
//! ## Architecture
//!
//! carpai-cli / carpai-server (Layer 2: Products)
//!         |
//!         v
//!   carpai-core (Layer 1: Business Logic)  <-- THIS CRATE
//!         |
//!   +-- LocalFileSessionStore    (SessionStore impl)
//!   +-- LocalToolExecutor         (ToolExecutor impl)
//!   +-- SidecarInferenceBackend   (InferenceBackend impl)
//!   +-- LocalFileSystem          (VirtualFileSystem impl)
//!   +-- InProcessEventBus        (EventBus impl)
//!   +-- LocalMemoryBackend       (MemoryBackend impl)
//!
//!   +-- execute_agent_turn()     (Agent main loop)
//!   +-- build_local_agent_context()  (DI assembly)
//!
//!         v
//!   carpai-internal (Layer 0: Pure Traits)

// --- Configuration ---
pub mod config;

// --- Local Implementations (Layer 1) ---
pub mod session_impl;
pub mod tool_executor_impl;
pub mod inference_impl;
pub mod filesystem_impl;
pub mod event_bus_impl;
pub mod memory_impl;

// --- Agent Loop ---
pub mod agent_loop;

// --- Agent System (Phase 1B) ---
pub mod agent;

// ========================================================================
// Re-exports from carpai-internal (convenience layer)
// ========================================================================

// --- Core Types ---
pub use carpai_internal::{
    AgentContext,
    AgentContextBuilder,
    AppConfig,
    AppMode,
    RequestMetadata,
};

// --- Traits ---
pub use carpai_internal::{
    SessionStore,
    ToolExecutor,
    InferenceBackend,
    VirtualFileSystem,
    EventBus,
    MemoryBackend,
    CodeCompletion,
    AuthProvider,
};

// --- Session Types ---
pub use carpai_internal::{
    SessionId,
    SessionState,
    SessionMeta,
    StoredMessage,
    ContentBlock,
    MessageRole,
    LoadedSession,
    SessionFilter,
    CompactionSnapshot,
    SessionError,
    SessionMetaUpdate,
};

// --- Tool Types ---
pub use carpai_internal::{
    ToolRequest,
    ToolResponse,
    ToolSchema,
    ToolCategory,
    ExecutionMode,
    ToolContext,
    ToolExecError,
    ToolExecutionRecord,
    ValidationResult,
};

// --- Inference Types ---
pub use carpai_internal::{
    ChatCompletionRequest,
    ChatCompletionResponse,
    ChatMessage,
    ChatRole,
    ChatContent,
    StreamChunk,
    LogProbs,
    TokenLogProb,
    TokenUsage,
    RoutedModelInfo,
    QuotaUsage,
    FallbackInfo,
    ModelSelectionConstraints,
    CompletionTokenUsage,
    StreamChunkType,
    FallbackReason,
};

// --- FileSystem Types ---
pub use carpai_internal::{
    FsError,
    FileMeta,
    FileEntry,
    FileWriteResult,
    SearchResult,
    ContentMatch,
    SearchOptions,
    FsEvent,
};

// --- EventBus Types ---
pub use carpai_internal::{
    BusSubscriber,
    BusEvent,
    BusHealth,
    EventBusError,
    BusEventEnvelope,
    SessionCreated,
    SessionMessagesAppended,
    SessionStateChanged,
    AgentTurnStarted,
    AgentTurnCompleted,
    ToolExecuted,
    FileModified,
    FileOperationType,
    InferenceCompleted,
    SystemHealthChanged,
    SystemStatus,
};

// --- Memory Types ---
pub use carpai_internal::{
    EnhancedMemoryEntry,
    EnhancedMemoryQuery,
    VectorSearchResult,
    Reinforcement,
    MemoryScope,
    TrustLevel,
    EnhancedMemoryStats,
    CleanupOptions,
    CleanupResult,
    EnhancedMemoryUpdate,
    VectorSearchOptions,
};

// --- Auth Types ---
pub use carpai_internal::{
    AuthToken,
    UserInfo,
    Permission,
    ApiKeyValidator,
    UserTier,
    AuthError,
};

// ========================================================================
// Re-exports from carpai-core modules
// ========================================================================

// --- Config ---
pub use config::CoreConfig;

// --- Local Implementations ---
pub use session_impl::LocalFileSessionStore;
pub use tool_executor_impl::LocalToolExecutor;
pub use inference_impl::SidecarInferenceBackend;
pub use filesystem_impl::LocalFileSystem;
pub use event_bus_impl::InProcessEventBus;
pub use memory_impl::LocalMemoryBackend;

// --- Agent Loop API ---
pub use agent_loop::{execute_agent_turn, AgentTurnOutput, build_local_agent_context};

// --- Agent System Re-exports (Phase 1B) ---
pub use agent::runtime::{AutonomousAgent, CrossFileAgent, AgentStatus};
pub use agent::sub_agents::{
    SubAgentTask, SubAgentConfig, SubAgentResult, SubAgentStatus,
    ParallelTaskScheduler, OrchestrationResult,
};
pub use agent::plan_mode::{Plan, PlanStep, StepStatus, PLAN_MODE_SYSTEM_PROMPT};
pub use agent::skill_system::SkillRegistry;
pub use agent::task::planner::TaskPlanner;
