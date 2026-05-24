# CarpAI Interface Contract v1.0 (Week 3 Frozen)

> **Status**: FROZEN — Breaking changes require version bump to v1.1
> **Date**: 2026-05-24
> **Team**: solo-Turbo (API Owner)
> **Audience**: ma-guoyang (Server), Paw-brave (CLI/TUI/SDK)

---

## 1. Architecture Overview

```
┌─────────────────────────────────────────────────────────────────────┐
│                    Layer 2: Products                                 │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐              │
│  │ carpai-cli   │  │ carpai-server│  │ carpai-sdk   │              │
│  │ (TUI/REPL)   │  │ (REST/gRPC)  │  │ (IDE Plugin) │              │
│  └──────┬───────┘  └──────┬───────┘  └──────┬───────┘              │
└─────────┼─────────────────┼─────────────────┼──────────────────────┘
          │                 │                 │
          ▼                 ▼                 ▼
┌─────────────────────────────────────────────────────────────────────┐
│                  Layer 1: carpai-core (Business Logic)               │
│                                                                     │
│  execute_agent_turn()    build_local_agent_context()                │
│  CoreConfig (3-layer loading)                                        │
│                                                                     │
│  LocalFileSessionStore     LocalToolExecutor                         │
│  SidecarInferenceBackend  LocalFileSystem                            │
│  InProcessEventBus        LocalMemoryBackend                        │
│                                                                     │
│  CompletionEngine         ToolRegistry / MCP                         │
│  SmartCompleter           AutoFallbackRouter                         │
│  RefactorEngine           GitWorkflow                                │
└──────────────────────────────────┬──────────────────────────────────┘
                                   │
                                   ▼
┌─────────────────────────────────────────────────────────────────────┐
│                 Layer 0: carpai-internal (Pure Traits)               │
│                                                                     │
│  SessionStore      ToolExecutor       InferenceBackend              │
│  VirtualFileSystem EventBus           MemoryBackend                 │
│  CodeCompletion    AuthProvider                                      │
│                                                                     │
│  AgentContext ← Central DI Container (Arc<dyn Trait>)               │
└─────────────────────────────────────────────────────────────────────┘
```

### 数据流向

```
User Input → execute_agent_turn(ctx, msg)
    ├── SessionStore.create_session / append_messages
    ├── InferenceBackend.complete_chat(request)
    │   └── [tool_calls] → ToolExecutor.execute(request)
    │       └── VirtualFileSystem.read_file / write_file
    ├── MemoryBackend.store / search
    └── EventBus.publish(event)
        └── [subscribers receive typed events]
```

---

## 2. Public API Reference

### 2.1 Core Entry Points

#### `execute_agent_turn()` — 执行一轮完整的 Agent 交互

```rust
pub async fn execute_agent_turn(
    ctx: &AgentContext,
    user_message: &str,
) -> Result<AgentTurnOutput>
```

**执行流程**：
1. 通过 `SessionStore` 获取或创建会话
2. 追加用户消息到会话
3. 从会话历史构建上下文
4. 调用 `InferenceBackend.complete_chat()` 生成响应
5. 若存在 tool_calls，通过 `ToolExecutor` 执行（循环）
6. 返回最终的 `AgentTurnOutput`

#### `build_local_agent_context()` — 构建本地模式 AgentContext

```rust
pub fn build_local_agent_context(config: &CoreConfig) -> AgentContext
```

**组装的本地实现**：

| Trait | Implementation |
|-------|---------------|
| `SessionStore` | `LocalFileSessionStore` (JSONL on disk) |
| `ToolExecutor` | `LocalToolExecutor` (direct process spawn) |
| `InferenceBackend` | `SidecarInferenceBackend` (wraps sidecar) |
| `VirtualFileSystem` | `LocalFileSystem` (std::fs + path sandboxing) |
| `EventBus` | `InProcessEventBus` (tokio::broadcast) |
| `MemoryBackend` | `LocalMemoryBackend` (file-based JSON + vector index) |

> **注意**：`completion` 和 `auth` 在本地模式下使用桩实现，`build_local_agent_context` 内部通过 `AgentContextBuilder::build()` 验证所有必需服务。

#### `AgentTurnOutput` — 单轮交互输出

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentTurnOutput {
    pub text: String,                          // 最终文本响应
    pub tool_calls: Vec<ToolCallInfo>,          // 本次调用的工具列表
    pub usage: TokenUsage,                      // Token 使用量
    pub session_id: SessionId,                  // 会话 ID
    pub duration_ms: u64,                       // 总耗时 (ms)
}
```

**`ToolCallInfo`**:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallInfo {
    pub name: String,                           // 工具名称
    pub arguments: serde_json::Value,            // 调用参数
    pub result: Option<String>,                 // 执行结果
    pub duration_ms: u64,                       // 工具执行耗时
    pub status: String,                         // "success" | "error" | "timeout"
}
```

---

### 2.2 Configuration

#### `AppConfig` — 应用基础配置 (Layer 0)

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub mode: AppMode,                          // 运行模式: Cli / Server / Client
    pub data_dir: PathBuf,                      // 数据根目录
    pub working_dir: PathBuf,                   // 工作目录 (项目根)
    pub default_model: String,                  // 默认推理模型
    pub max_context_tokens: usize,              // 最大上下文窗口
    pub tools_enabled: bool,                    // 是否启用工具执行
    pub default_tool_mode: ExecutionMode,       // 默认工具执行模式
    pub vfs_enabled: bool,                      // 是否启用 VFS
    pub vfs_root: Option<PathBuf>,              // VFS 根路径限制
    pub memory_enabled: bool,                   // 是否启用记忆功能
    pub event_bus_enabled: bool,                // 是否启用事件总线
}
```

**默认值**:

| 字段 | 默认值 |
|------|--------|
| `mode` | `AppMode::Cli` |
| `data_dir` | `.jcode/data` |
| `working_dir` | `.` |
| `default_model` | `"default"` |
| `max_context_tokens` | `200_000` |
| `tools_enabled` | `true` |
| `default_tool_mode` | `ExecutionMode::Local` |
| `vfs_enabled` | `true` |
| `vfs_root` | `None` |
| `memory_enabled` | `true` |
| `event_bus_enabled` | `true` |

#### `AppMode` — 应用运行模式

```rust
pub enum AppMode {
    Cli,      // 独立 CLI 客户端 (全本地)
    Server,   // 企业服务器 (远程后端, 多租户)
    Client,   // 混合模式 (本地 UI + 远程服务器)
}
```

#### `CoreConfig` — 核心配置 (Layer 1, extends AppConfig)

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoreConfig {
    #[serde(flatten)]
    pub base: AppConfig,                        // 基础配置 (继承自 AppConfig)

    // === Storage ===
    pub data_dir: PathBuf,                      // 本地存储根目录 (~/.carpai)
    pub session_subdir: String,                 // 会话子目录 (默认 "sessions")
    pub memory_subdir: String,                  // 记忆子目录 (默认 "memory")

    // === Concurrency ===
    pub max_concurrent_tools: usize,             // 最大并发工具数 (默认 5)
    pub max_agent_iterations: usize,             // Agent 循环最大迭代次数 (默认 100)

    // === Provider ===
    pub completion_provider: ProviderConfig,     // 推理提供者配置

    // === Caching ===
    pub cache_size_mb: usize,                   // 内存缓存大小 MB (默认 512)
    pub disk_cache_enabled: bool,               // 启用磁盘缓存 (默认 true)
}
```

**`ProviderConfig`**:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub provider_type: String,                  // 提供者类型 (默认 "local")
    pub endpoint: Option<String>,               // API 端点 URL (默认 "http://localhost:11434")
    pub api_key: Option<String>,                // API Key (建议从环境变量读取)
    pub model: Option<String>,                  // 模型名覆盖
    pub timeout_secs: u64,                      // 请求超时秒数 (默认 30)
}
```

**CoreConfig 默认值汇总**:

| 字段 | 默认值 |
|------|--------|
| `data_dir` | `~/.carpai` (home dir) |
| `session_subdir` | `"sessions"` |
| `memory_subdir` | `"memory"` |
| `max_concurrent_tools` | `5` |
| `max_agent_iterations` | `100` |
| `provider_type` | `"local"` |
| `endpoint` | `http://localhost:11434` |
| `timeout_secs` | `30` |
| `cache_size_mb` | `512` |
| `disk_cache_enabled` | `true` |

#### 三层配置加载优先级

```
优先级从低到高:
  Layer 1: 硬编码默认值 (Default trait)
  Layer 2: TOML 配置文件 (~/.carpai/config.toml 或指定路径)
  Layer 3: 环境变量 (CARPAI_* 前缀, 最高优先级)
```

**环境变量映射表**:

| 环境变量 | 映射字段 |
|----------|----------|
| `CARPAI_DATA_DIR` 或 `CARPAI_CORE__DATA_DIR` | `config.data_dir` |
| `CARPAI_DEFAULT_MODEL` | `config.base.default_model` |
| `CARPAI_CORE__MAX_CONCURRENT_TOOLS` | `config.max_concurrent_tools` |
| `CARPAI_CORE__MAX_AGENT_ITERATIONS` | `config.max_agent_iterations` |
| `CARPAI_LOG_LEVEL` | (预留, 尚未接入) |

**加载方法签名**:

```rust
impl CoreConfig {
    pub fn load(path: &PathBuf) -> Result<Self, ConfigError>;
    pub fn session_store_path(&self) -> PathBuf;   // data_dir/session_subdir
    pub fn memory_store_path(&self) -> PathBuf;    // data_dir/memory_subdir
}
```

---

### 2.3 Traits (7 Core Traits)

---

#### Trait 1: `SessionStore` — 统一会话持久化接口

```rust
#[async_trait]
pub trait SessionStore: Send + Sync {
    // --- CRUD ---
    async fn create_session(&self, meta: SessionMeta) -> Result<SessionId, SessionError>;
    async fn load_session(&self, id: &SessionId) -> Result<Option<LoadedSession>, SessionError>;
    async fn update_meta(&self, id: &SessionId, updates: SessionMetaUpdate) -> Result<(), SessionError>;
    async fn delete_session(&self, id: &SessionId, hard: bool) -> Result<(), SessionError>;

    // --- Messages ---
    async fn append_messages(&self, session_id: &SessionId, messages: Vec<StoredMessage>)
        -> Result<Vec<String>, SessionError>;
    async fn get_messages(&self, session_id: &SessionId, offset: usize, limit: usize)
        -> Result<Vec<StoredMessage>, SessionError>;
    async fn message_count(&self, session_id: &SessionId) -> Result<usize, SessionError>;

    // --- State Transitions ---
    async fn set_state(&self, id: &SessionId, new_state: SessionState) -> Result<(), SessionError>;

    // --- Compaction ---
    async fn save_compaction(&self, session_id: &SessionId, snapshot: CompactionSnapshot)
        -> Result<(), SessionError>;
    async fn load_compaction(&self, session_id: &SessionId) -> Result<Option<CompactionSnapshot>, SessionError>;

    // --- Listing / Search ---
    async fn list_sessions(&self, filter: SessionFilter) -> Result<Vec<SessionMeta>, SessionError>;
    async fn count_sessions(&self, filter: &SessionFilter) -> Result<usize, SessionError>;
}
```

**方法总数**: 12 个

**错误类型**: `SessionError`

```rust
pub enum SessionError {
    NotFound(String),
    InvalidTransition { from: SessionState, to: SessionState },
    Conflict,
    Storage(String),
    Serialization(String),
    QuotaExceeded { owner: String, limit: usize, current: usize },
    Internal(#[from] anyhow::Error),
}
```

**合法状态转换矩阵**:

| From \ To | Active | Paused | Archived | Deleted |
|-----------|--------|--------|----------|---------|
| **Active** | - | ✅ | ✅ | ✅ |
| **Paused** | ✅ | - | ✅ | ✅ |
| **Archived** | ❌ | ❌ | - | ✅ |
| **Deleted** | ✅ (admin) | ❌ | ❌ | - |

---

#### Trait 2: `ToolExecutor` — 统一工具执行接口 (带沙箱与审计)

```rust
#[async_trait]
pub trait ToolExecutor: Send + Sync {
    /// 执行工具 (主入口: 权限检查 → 解析 → 执行 → 审计记录)
    async fn execute(&self, request: ToolRequest) -> Result<ToolResponse, ToolExecError>;

    /// 列出所有可用工具 (含 LLM function calling schema)
    async fn list_tools(&self) -> Result<Vec<ToolSchema>, ToolExecError>;

    /// 获取单个工具的 schema
    async fn get_tool_schema(&self, name: &str) -> Result<Option<ToolSchema>, ToolExecError>;

    /// 仅验证参数, 不执行
    async fn validate(&self, name: &str, params: &serde_json::Value)
        -> Result<ValidationResult, ToolExecError>;

    /// 检查用户是否有权限使用某工具
    async fn check_permission(&self, user_id: &str, tool_name: &str)
        -> Result<bool, ToolExecError>;

    /// 取消正在运行的工具执行
    async fn cancel(&self, request_id: &str) -> Result<(), ToolExecError>;
}
```

**方法总数**: 6 个

**错误类型**: `ToolExecError`

```rust
pub enum ToolExecError {
    NotFound(String),
    InvalidParameters(String),
    PermissionDenied { user: String, tool: String },
    ExecutionFailed(String),
    Timeout(u64),
    Disabled(String),
    Sandbox(String),
    Cancelled,
    RateLimitExceeded,
    Internal(#[from] anyhow::Error),
}
```

---

#### Trait 3: `InferenceBackend` — 企业级推理后端 (路由/配额/回退)

```rust
#[async_trait]
pub trait InferenceBackend: Send + Sync {
    /// 完成聊天对话 (Agent 主入口, 含路由+配额+回退)
    async fn complete_chat(&self, request: ChatCompletionRequest)
        -> Result<ChatCompletionResponse, InferenceError>;

    /// 流式聊天完成
    async fn stream_chat(&self, request: ChatCompletionRequest)
        -> Result<Box<dyn tokio_stream::Stream<Item = Result<StreamChunk, InferenceError>> + Send>, InferenceError>;

    /// 获取可用模型列表 (含路由元数据)
    async fn list_models_with_routing(&self) -> Result<Vec<RoutedModelInfo>, InferenceError>;

    /// 为给定请求选择最优模型 (成本/延迟优化)
    async fn select_model(&self, constraints: &ModelSelectionConstraints)
        -> Result<String, InferenceError>;

    /// 查询用户/租户配额使用情况
    async fn get_quota_usage(&self, user_id: &str) -> Result<QuotaUsage, InferenceError>;

    /// 记录 token 使用量
    async fn record_usage(&self, user_id: &str, usage: &CompletionTokenUsage, model: &str)
        -> Result<(), InferenceError>;

    /// 获取底层基础引擎 (用于直接访问)
    fn base_engine(&self) -> Arc<dyn InferenceEngine>;
}
```

**方法总数**: 7 个 (6 async + 1 sync)

**关联类型/依赖**: 依赖 base trait `InferenceEngine`, 错误类型为 `InferenceError` (re-export from `inference.rs`)

---

#### Trait 4: `VirtualFileSystem` — 虚拟文件系统接口 (安全沙箱)

```rust
#[async_trait]
pub trait VirtualFileSystem: Send + Sync {
    // --- Basic File Operations ---
    async fn read_file(&self, path: &Path) -> Result<String, FsError>;
    async fn read_file_bytes(&self, path: &Path) -> Result<Vec<u8>, FsError>;
    async fn write_file(&self, path: &Path, content: &str) -> Result<FileWriteResult, FsError>;
    async fn write_file_bytes(&self, path: &Path, data: &[u8]) -> Result<FileWriteResult, FsError>;
    async fn delete_file(&self, path: &Path) -> Result<(), FsError>;
    async fn exists(&self, path: &Path) -> Result<bool, FsError>;
    async fn metadata(&self, path: &Path) -> Result<FileMeta, FsError>;

    // --- Directory Operations ---
    async fn list_dir(&self, path: &Path, recursive: bool) -> Result<Vec<FileEntry>, FsError>;
    async fn create_dir(&self, path: &Path) -> Result<(), FsError>;
    async fn delete_dir(&self, path: &Path, recursive: bool) -> Result<(), FsError>;

    // --- Search ---
    async fn search_files(&self, pattern: &str, in_path: &Path, max_results: usize)
        -> Result<Vec<SearchResult>, FsError>;
    async fn search_content(&self, query: &str, in_path: &Path, options: SearchOptions)
        -> Result<Vec<ContentMatch>, FsError>;

    // --- Git Operations (optional extension) ---
    async fn git_diff(&self, path: &Path, staged: bool) -> Result<String, FsError>;
    async fn git_status(&self, path: &Path) -> Result<String, FsError>;
    async fn git_blame(&self, path: &Path) -> Result<String, FsError>;

    // --- Watch (optional) ---
    async fn watch(&self, path: &Path)
        -> Result<Pin<Box<dyn tokio_stream::Stream<Item = FsEvent> + Send>>, FsError>;

    // --- Admin / Security (sync methods) ---
    fn resolve(&self, path: &Path) -> Result<PathBuf, FsError>;
    fn root(&self) -> &Path;
    fn is_allowed(&self, path: &Path) -> bool;
}
```

**方法总数**: 20 个 (17 async + 3 sync)

**错误类型**: `FsError`

```rust
pub enum FsError {
    NotFound(String),
    PathEscape { path: String, root: String },
    PermissionDenied(String),
    AlreadyExists(String),
    NotEmpty(String),
    Io(#[from] std::io::Error),
    NotAFile(String),
    NotADirectory(String),
    Encoding(String),
    Unsupported,
    QuotaExceeded { limit_mb: u64, current_mb: u64 },
    Internal(#[from] anyhow::Error),
}
```

---

#### Trait 5: `EventBus` — 统一发布-订阅事件总线

```rust
#[async_trait]
pub trait EventBus: Send + Sync + 'static {
    /// 发布事件 (内部序列化为 JSON)
    async fn publish_json(&self, event_type: &str, payload: &str) -> Result<(), EventBusError>;

    /// 订阅指定类型的事件
    async fn subscribe(&self, event_type: &str)
        -> Result<Box<dyn BusSubscriber + Send>, EventBusError>;

    /// 获取当前订阅者数量
    fn subscriber_count(&self, event_type: &str) -> usize;

    /// 健康检查
    fn health_check(&self) -> BusHealth;

    /// 克隆事件总线 (返回 Arc<Self> 包装的新 trait object)
    fn clone_box(&self) -> Arc<dyn EventBus>;
}
```

**方法总数**: 5 个 (2 async + 3 sync)

**扩展 trait `EventBusExt`** (自动 blanket impl):

```rust
#[async_trait]
pub trait EventBusExt: EventBus {
    async fn publish<E: BusEvent>(&self, event: E) -> Result<(), EventBusError>;
}
// 所有 EventBus 实现者自动获得此方法
```

**订阅者 trait `BusSubscriber`**:

```rust
#[async_trait]
pub trait BusSubscriber: Send + Debug {
    async fn recv(&mut self) -> Result<BusEventEnvelope, EventBusError>;
    fn try_recv(&mut self) -> Result<Option<BusEventEnvelope>, EventBusError>;
    fn len(&self) -> usize;
    fn is_empty(&self) -> bool { self.len() == 0 }
}
```

**错误类型**: `EventBusError`

```rust
pub enum EventBusError {
    SubscriptionFailed(String),
    PublishFailed(String),
    ConnectionLost,
    Deserialization(String),
    ChannelClosed,
    Internal(#[from] anyhow::Error),
}
```

**健康状态**:

```rust
pub struct BusHealth {
    pub healthy: bool,
    pub backend: String,              // "in_process" | "redis" | "kafka"
    pub total_subscribers: usize,
    pub events_published_total: u64,
    pub events_dropped_total: u64,
    pub uptime_secs: u64,
}
```

**内置事件类型一览**:

| 事件结构体 | event_type | 持久化 |
|-----------|-----------|--------|
| `SessionCreated` | `"session.created"` | 否 |
| `SessionMessagesAppended` | `"session.messages_appended"` | 否 |
| `SessionStateChanged` | `"session.state_changed"` | 否 |
| `AgentTurnStarted` | `"agent.turn_started"` | 否 |
| `AgentTurnCompleted` | `"agent.turn_completed"` | 否 |
| `ToolExecuted` | `"agent.tool_executed"` | 否 |
| `FileModified` | `"fs.file_modified"` | **是** |
| `InferenceCompleted` | `"inference.completed"` | **是** |
| `SystemHealthChanged` | `"system.health_changed"` | 否 |

---

#### Trait 6: `MemoryBackend` — 企业级记忆后端 (向量搜索/去重/分层)

```rust
#[async_trait]
pub trait MemoryBackend: Send + Sync {
    // --- Base Operations ---
    async fn store(&self, entry: EnhancedMemoryEntry) -> Result<String, MemoryError>;
    async fn retrieve(&self, id: &str) -> Result<Option<EnhancedMemoryEntry>, MemoryError>;
    async fn search(&self, query: &EnhancedMemoryQuery) -> Result<Vec<EnhancedMemoryEntry>, MemoryError>;
    async fn delete(&self, id: &str) -> Result<(), MemoryError>;
    async fn update(&self, id: &str, updates: &EnhancedMemoryUpdate) -> Result<EnhancedMemoryEntry, MemoryError>;

    // --- Vector Operations ---
    async fn vector_search(&self, embedding: &[f32], limit: usize, options: &VectorSearchOptions)
        -> Result<VectorSearchResult, MemoryError>;
    async fn upsert_embedding(&self, memory_id: &str, embedding: Vec<f32>) -> Result<(), MemoryError>;

    // --- Dedup & Consolidation ---
    async fn find_duplicate(&self, content: &str, threshold: f32) -> Result<Option<String>, MemoryError>;
    async fn reinforce(&self, id: &str, session_id: &str, message_index: usize) -> Result<(), MemoryError>;
    async fn consolidate(&self, primary_id: &str, merge_ids: &[String]) -> Result<EnhancedMemoryEntry, MemoryError>;

    // --- Scoped Access ---
    async fn get_by_scope(&self, scope: MemoryScope, project_id: Option<&str>, limit: usize)
        -> Result<Vec<EnhancedMemoryEntry>, MemoryError>;

    // --- Statistics ---
    async fn stats(&self, scope: Option<MemoryScope>) -> Result<EnhancedMemoryStats, MemoryError>;
    async fn cleanup(&self, options: &CleanupOptions) -> Result<CleanupResult, MemoryError>;
}
```

**方法总数**: 13 个 (全部 async)

**错误类型**: `MemoryError` (re-export from `memory.rs`)

---

#### Trait 7: `CodeCompletion` — 代码补全接口 (base trait from completion.rs)

> 此 trait 在 `carpai-internal/src/completion.rs` 中定义, 为 IDE 插件提供内联/聊天式代码补全能力。

```rust
// 定义于 carpai_internal::completion, re-exported via lib.rs
pub trait CodeCompletion: Send + Sync {
    // 具体方法签名参见 completion.rs 模块
    // 核心类型:
    //   - CompletionCandidate: 补全候选
    //   - CompletionRequest: 补全请求
}
```

> **Phase 1D 补充**: `carpai-core/completion/` 提供了完整补全系统:
> - `SmartCompleter` — FIM (Fill-In-Middle) 补全器
> - `AutoFallbackRouter` — 多提供者自动回退路由
> - `FimCompleter` / `ContextBuilder` / `AcceptanceTracker`
> - `CompletionEngine` / `CompletionProvider` / `CompletionOutput`

---

### 2.4 Key Types

#### 2.4.1 Session Types

**`SessionId`** — 会话唯一标识符

```rust
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SessionId(pub String);

impl Display for SessionId { /* 输出 inner String */ }
impl From<String> for SessionId;
impl From<&str> for SessionId;
```

**`SessionState`** — 会话生命周期状态

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum SessionState {
    #[default]
    Active,      // 活跃中 — 接受消息
    Paused,      // 已暂停 — 保留但不接受输入
    Archived,    // 已归档 — 只读, 已压缩
    Deleted,     // 已删除 — 软删除, 待清理
}

impl SessionState {
    pub fn can_transition_to(&self, target: &SessionState) -> bool;
    pub fn is_writable(&self) -> bool;  // 仅 Active 返回 true
}
```

**`SessionMeta`** — 会话元数据 (轻量级, 高频查询)

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMeta {
    pub id: SessionId,                              // 会话 ID
    pub parent_id: Option<SessionId>,               // 父会话 ID (fork 关系)
    pub title: Option<String>,                      // 会话标题
    pub created_at: DateTime<Utc>,                  // 创建时间
    pub updated_at: DateTime<Utc>,                  // 更新时间
    pub last_active_at: Option<DateTime<Utc>>,      // 最后活跃时间
    pub state: SessionState,                        // 当前状态
    pub model: Option<String>,                      // 使用的模型
    pub working_dir: Option<String>,               // 工作目录
    pub message_count: usize,                       // 消息数量
    pub owner_id: Option<String>,                  // 所有者/租户 ID
    pub tags: HashMap<String, String>,             // 自定义标签
}
```

**`StoredMessage`** — 单条持久化消息

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredMessage {
    pub id: String,                                // 消息 UUID
    pub role: MessageRole,                         // 角色
    pub content: Vec<ContentBlock>,                // 内容块 (多模态/多部分)
    pub timestamp: DateTime<Utc>,                  // 记录时间
    pub token_usage: Option<TokenUsage>,           // Token 用量 (仅 assistant)
    pub model: Option<String>,                     // 生成模型 (仅 assistant)
}
```

**`MessageRole`** — 消息角色

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MessageRole {
    System,
    User,
    Assistant,
    Tool,
}
```

**`ContentBlock`** — 内容块变体

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ContentBlock {
    Text { text: String },                                                // 纯文本
    ToolUse { id: String, name: String, input: serde_json::Value },        // 工具调用
    ToolResult { tool_use_id: String, content: String, is_error: bool },  // 工具返回
    Thinking { text: String, signature: Option<String> },                  // 推理思考块
}
```

**`TokenUsage`** — Token 使用量 (per-message)

```rust
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct TokenUsage {
    pub input_tokens: usize,
    pub output_tokens: usize,
    pub total_tokens: usize,
}
```

**辅助类型**:

```rust
pub struct LoadedSession {
    pub meta: SessionMeta,
    pub messages: Vec<StoredMessage>,
    pub compaction: Option<CompactionSnapshot>,
}

pub struct CompactionSnapshot {
    pub compacted_at: DateTime<Utc>,
    pub original_message_count: usize,
    pub system_summary: String,
    pub retained_message_ids: Vec<String>,
}

pub struct SessionFilter {
    pub owner_id: Option<String>,
    pub state: Option<SessionState>,
    pub model: Option<String>,
    pub active_after: Option<DateTime<Utc>>,
    pub active_before: Option<DateTime<Utc>>,
    pub tag_filter: Option<(String, String)>,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
    pub sort_by: SessionSortField,              // 默认 UpdatedAt
    pub sort_desc: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum SessionSortField { #[default] UpdatedAt, CreatedAt, LastActiveAt, Title, MessageCount }

pub struct SessionMetaUpdate {
    pub title: Option<String>,
    pub state: Option<SessionState>,
    pub model: Option<String>,
    pub working_dir: Option<String>,
    pub last_active_at: Option<DateTime<Utc>>,
    pub tags: Option<HashMap<String, String>>,
}
```

---

#### 2.4.2 Tool Types

**`ToolRequest`** — 工具执行请求

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolRequest {
    pub tool_name: String,                       // 工具名 (必须匹配注册名)
    pub parameters: serde_json::Value,           // 参数 (必须符合 JSON Schema)
    pub context: ToolContext,                     // 执行上下文
    pub request_id: String,                       // 唯一请求 ID (自动生成: "req-{uuid}")
    pub mode_override: Option<ExecutionMode>,     // 执行模式覆盖 (None = 用默认)
}
```

**`ToolResponse`** — 工具执行响应

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResponse {
    pub success: bool,                            // 是否成功
    pub output: String,                           // 输出内容 (stdout 或结构化结果)
    pub data: Option<serde_json::Value>,          // 结构化数据
    pub exit_code: Option<i32>,                   // 退出码
    pub duration_ms: u64,                         // 执行耗时 ms
    pub request_id: String,                       // 回显请求 ID
    pub tool_name: String,                        // 执行的工具名
    pub audit_id: Option<String>,                 // 审计记录 ID
}
```

**`ToolSchema`** — 工具描述 (LLM function calling 格式)

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSchema {
    pub name: String,                             // 唯一标识
    pub description: String,                      // LLM 用于决策的自然语言描述
    pub parameters_json_schema: serde_json::Value,// JSON Schema
    pub category: ToolCategory,                   // 分类
    pub requires_confirmation: bool,              // 是否需要确认
    pub timeout_secs: u64,                        // 默认超时 (秒)
    pub default_mode: ExecutionMode,              // 无覆盖时的执行模式
    pub required_permissions: Vec<String>,        // 所需权限范围
}
```

**`ToolCategory`** — 工具分类枚举

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ToolCategory {
    FileSystem,     // 文件操作 (读写编辑删除)
    CodeEdit,       // 代码分析编辑 (AST 编辑, 重构)
    Shell,          // Shell/命令执行 (bash, powershell)
    Web,            // Web/HTTP (curl, fetch)
    Database,       // 数据库操作 (SQL)
    Inference,      // AI/ML 推理 (embedding, 分类)
    SystemInfo,     // 系统信息 (os, cpu, memory)
    VersionControl, // 版本控制 (git 操作)
    Search,         // 搜索 (代码搜索, grep, 语义搜索)
    Custom,         // 自定义/用户定义
}
```

**`ExecutionMode`** — 执行模式

```rust
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ExecutionMode {
    Local,                                   // 直接本地执行 (CLI 模式)
    Sandboxed,                               // 沙箱执行 (Docker, gVisor)
    Remote { endpoint: String },             // 委托远程 MCP 服务端
    DryRun,                                  // 试运行 — 仅验证不执行
}
```

**`ToolContext`** — 执行上下文

```rust
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ToolContext {
    pub user_id: String,                      // 用户/租户 ID
    pub session_id: String,                   // 所属会话 ID
    pub working_dir: Option<PathBuf>,         // 文件操作工作目录
    pub env_vars: HashMap<String, String>,    // 注入的环境变量
    pub timeout: Option<Duration>,            // 覆盖超时
    pub require_confirmation: bool,           // 执行前是否需确认
    pub metadata: HashMap<String, String>,    // 透传元数据
}
```

**`ValidationResult`** — 参数验证结果

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationResult {
    pub valid: bool,                           // 参数是否合法
    pub error: Option<String>,                 // 错误信息 (非法时)
    pub warnings: Vec<String>,                 // 警告 (非致命问题)
}
```

**`ToolExecutionRecord`** — 审计记录 (每次执行一条)

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolExecutionRecord {
    pub id: String,                            // 记录 ID
    pub timestamp: DateTime<Utc>,              // 时间戳
    pub user_id: String,                       // 执行者
    pub session_id: String,                    // 会话
    pub tool_name: String,                     // 工具名
    pub parameters_redacted: serde_json::Value,// 参数 (敏感字段已脱敏)
    pub success: bool,                         // 成功/失败
    pub duration_ms: u64,                      // 耗时
    pub exit_code: Option<i32>,               // 退出码
    pub mode: ExecutionMode,                   // 执行模式
    pub client_ip: Option<String>,            // 客户端 IP (server 模式)
}
```

---

#### 2.4.3 Inference Types

**`ChatCompletionRequest`** — 聊天完成请求 (OpenAI 兼容格式)

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatCompletionRequest {
    pub messages: Vec<ChatMessage>,                             // 对话消息列表
    pub model: String,                                          // 模型标识 ("auto" 表示路由选择)
    pub max_tokens: Option<usize>,                              // 最大生成 token 数
    pub temperature: Option<f32>,                                // 温度 (0.0-2.0)
    pub top_p: Option<f32>,                                     // Top-p 采样 (0.0-1.0)
    pub stop: Option<Vec<String>>,                              // 停止序列
    pub presence_penalty: Option<f32>,                           // 存在惩罚 (-2.0~2.0)
    pub frequency_penalty: Option<f32>,                          // 频率惩罚 (-2.0~2.0)
    pub tools: Option<Vec<ChatToolDefinition>>,                 // 函数调用定义
    pub tool_choice: Option<ToolChoice>,                         // 工具选择策略
    pub user_id: Option<String>,                                 // 配额追踪用户 ID
    pub session_id: Option<String>,                              // 对话上下文会话 ID
    pub metadata: HashMap<String, String>,                       // 审计/路由元数据
}
```

**`ChatMessage`** — 聊天消息

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: ChatRole,
    pub content: ChatContent,                                    // 文本或多部分
    pub name: Option<String>,                                    // function/result 消息名
}
```

**`ChatRole`** — 聊天角色

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChatRole { System, User, Assistant, Tool }
```

**`ChatContent`** — 内容 (字符串或多部分)

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ChatContent {
    Text(String),                                               // 纯文本
    Parts(Vec<ContentPart>),                                     // 多部分 (多模态)
}
impl From<String> for ChatContent;
impl From<&str> for ChatContent;

pub struct ContentPart {
    #[serde(rename = "type")]
    pub part_type: ContentType,                                  // Text | ImageUrl
    pub text: Option<String>,
}

pub enum ContentType { Text, ImageUrl }

pub struct ChatToolDefinition {
    #[serde(rename = "type")]
    pub tool_type: ToolType,                                     // Function
    pub function: FunctionDefinition,
}

pub enum ToolType { #[default] Function }

pub struct FunctionDefinition {
    pub name: String,
    pub description: Option<String>,
    pub parameters: serde_json::Value,                           // JSON Schema
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ToolChoice {
    None,
    #[default] Auto,
    Required,
    Specific(String),
}
```

**`ChatCompletionResponse`** — 聊天完成响应

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatCompletionResponse {
    pub id: String,                                              // 响应唯一 ID
    pub object: String,                                         // "chat.completion"
    pub created: u64,                                           // Unix 时间戳
    pub model: String,                                          // 实际响应模型 (可能与请求不同)
    pub choices: Vec<Choice>,                                   // 响应选项 (通常一个)
    pub usage: CompletionTokenUsage,                            // Token 使用量
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,                               // 使用的提供者 (内部)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fallback_info: Option<FallbackInfo>,                    // 回退信息 (如有)
}
```

**`Choice`** — 单个响应选项

```rust
pub struct Choice {
    pub index: usize,                                           // 索引 (单选始终为 0)
    pub message: ChatMessage,                                   // 消息内容
    pub finish_reason: FinishReason,                            // 结束原因
    #[serde(default)]
    pub logprobs: Option<LogProbs>,                             // 对数概率 (可选)
}
```

**`FinishReason`** — 结束原因 (from inference.rs)

```rust
// re-export from crate::inference::FinishReason
// 典型值: stop, length, tool_calls, content_filter
```

**`LogProbs`** / `TokenLogProb` / `TopLogProb`**:

```rust
pub struct LogProbs {
    pub content: Vec<TokenLogProb>,
}
pub struct TokenLogProb {
    pub token: String,
    pub logprob: f64,
    pub top_logprobs: Vec<TopLogProb>,
}
pub struct TopLogProb {
    pub token: String,
    pub logprob: f64,
}
```

**`CompletionTokenUsage`** — 聊天完成 Token 使用量

```rust
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct CompletionTokenUsage {
    pub prompt_tokens: usize,
    pub completion_tokens: usize,
    pub total_tokens: usize,
    #[serde(default)]
    pub cache_creation_input_tokens: Option<usize>,             // 缓存创建 token (Anthropic 等)
    #[serde(default)]
    pub cache_read_input_tokens: Option<usize>,                 // 缓存读取 token
}
```

**`StreamChunk`** — 流式响应分片

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamChunk {
    pub chunk_type: StreamChunkType,                            // 分片类型
    pub index: usize,                                           // 选项索引
    pub delta: Option<String>,                                  // 文本增量
    pub finish_reason: Option<FinishReason>,                    // 结束原因 (最终分片)
    pub usage: Option<CompletionTokenUsage>,                    // 累计用量 (最终分片)
}

pub enum StreamChunkType {
    ContentDelta,                                              // 内容文本增量
    ReasoningDelta,                                            // 推理内容增量
    Finish,                                                    // 最终分片
    Error,                                                     // 流式错误
}
```

**路由相关类型**:

```rust
pub struct RoutedModelInfo {
    pub model: ModelInfo,                                       // 基础模型信息
    pub providers: Vec<ModelProviderEntry>,                     // 提供者列表
    pub cost_per_1k_input: f64,                                 // 每 1K input token 成本 (USD)
    pub cost_per_1k_output: f64,                                // 每 1K output token 成本 (USD)
    pub avg_latency_ms: f64,                                    // 平均延迟 ms (滚动窗口)
    pub success_rate: f64,                                      // 成功率 (滚动窗口)
    pub routing_priority: u32,                                  // 自动选择优先级 (越小越优先)
    pub supports_function_calling: bool,                        // 支持函数调用
    pub supports_thinking: bool,                                // 支持扩展思考
    pub context_window: usize,                                  // 上下文窗口大小
}

pub struct ModelProviderEntry {
    pub provider: String,                                       // 提供者名 ("openai", "anthropic", "local")
    pub endpoint: Option<String>,                               // 端点 URL
    pub weight: u32,                                            // 负载均衡权重
    pub healthy: bool,                                          // 当前是否健康
}

pub struct ModelSelectionConstraints {
    pub max_cost_usd: Option<f64>,                              // 最大成本 USD
    pub max_latency_ms: Option<u64>,                            // 最大延迟 ms
    pub require_function_calling: bool,                         // 必须支持函数调用
    pub require_thinking: bool,                                 // 必须支持扩展思考
    pub min_context_window: Option<usize>,                      // 最小上下文窗口
    pub preferred_providers: Vec<String>,                       // 偏好提供者
    pub exclude_models: Vec<String>,                            // 排除模型
    pub user_tier: InferenceUserTier,                           // 用户等级 (默认 Free)
}
// Default: 全部 None/false/空, tier=Free

pub type InferenceUserTier = UserTier;  // re-export from auth module
```

**配额类型**:

```rust
pub struct QuotaUsage {
    pub user_id: String,
    pub tokens_used: u64,
    pub token_limit: u64,
    pub requests_used: u64,
    pub request_limit: u64,
    pub period_start: DateTime<Utc>,
    pub period_end: DateTime<Utc>,
    pub reset_in_secs: u64,
}

impl QuotaUsage {
    pub fn is_token_exceeded(&self) -> bool;
    pub fn is_request_exceeded(&self) -> bool;
    pub fn tokens_remaining(&self) -> u64;
    pub fn token_fraction(&self) -> f64;  // 0.0 ~ 1.0+
}
```

**回退类型**:

```rust
pub struct FallbackInfo {
    pub original_model: String,                                // 原始请求模型
    pub actual_model: String,                                  // 实际服务模型
    pub reason: FallbackReason,                                // 回退原因
    pub attempts: u32,                                         // 回退尝试次数
    pub total_fallback_ms: u64,                                // 回退总耗时 ms
}

pub enum FallbackReason {
    Overloaded,                                                // 过载
    Error(String),                                             // 错误
    LatencyExceeded,                                           // 超过延迟阈值
    CapacityReached,                                           // 达到容量上限
    UnsupportedCapability,                                     // 不支持所需能力
    QuotaExhausted,                                            // 配额耗尽
}
```

---

#### 2.4.4 File System Types

**`FileMeta`** — 文件元数据

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileMeta {
    pub path: PathBuf,                                         // 绝对路径
    pub size: u64,                                             // 文件大小 (bytes)
    pub is_dir: bool,                                          // 是否目录
    pub is_symlink: bool,                                      // 是否符号链接
    pub modified_at: SystemTime,                               // 最后修改时间
    pub created_at: Option<SystemTime>,                        // 创建时间
    pub extension: Option<String>,                             // 文件扩展名 ("rs", "ts")
    pub content_hash: Option<String>,                          // SHA-256 hex (按需计算)
}
```

**`FileWriteResult`** — 写文件结果

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileWriteResult {
    pub bytes_written: u64,                                    // 写入字节数
    pub created: bool,                                         // 是否新建文件 (vs 覆写)
    pub audit_id: Option<String>,                              // 审计记录 ID
    pub previous_hash: Option<String>,                         // 变更检测: 旧 hash
    pub new_hash: String,                                      // 新 hash
}
```

**`FileEntry`** — list_dir 条目

```rust
pub struct FileEntry {
    pub name: String,
    pub path: PathBuf,
    pub meta: FileMeta,
}
```

**`SearchResult`** — 文件名搜索结果

```rust
pub struct SearchResult {
    pub path: PathBuf,
    pub meta: FileMeta,
    pub score: f64,                                            // 相关性分数 (0.0-1.0)
}
```

**`SearchOptions`** — 内容搜索选项

```rust
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SearchOptions {
    pub case_insensitive: bool,                                // 默认 false
    pub regex: bool,                                           // 使用正则表达式
    pub max_matches_per_file: usize,                           // 每文件最大匹配数
    pub context_lines_before: usize,                           // 匹配前上下文行数
    pub context_lines_after: usize,                            // 匹配后上下文行数
    pub extensions: Vec<String>,                               // 包含的扩展名 (空=全部)
    pub exclude_patterns: Vec<String>,                         // 排除模式 (glob)
}
```

**`ContentMatch`** — 内容匹配结果 (grep 风格)

```rust
pub struct ContentMatch {
    pub file: PathBuf,                                         // 匹配所在文件
    pub line_number: usize,                                    // 行号 (1-indexed)
    pub line: String,                                          // 匹配行内容
    pub byte_offset: usize,                                    // 匹配起始字节偏移
    pub match_length: usize,                                   // 匹配长度
    pub before_context: Vec<String>,                           // 前置上下文行
    pub after_context: Vec<String>,                            // 后置上下文行
}
```

**`FsEvent`** — 文件监视事件

```rust
pub enum FsEvent {
    Created { path: PathBuf },
    Modified { path: PathBuf },
    Deleted { path: PathBuf },
    Renamed { old_path: PathBuf, new_path: PathBuf },
    Error { path: PathBuf, error: String },
}
```

---

#### 2.4.5 Event Types (详细字段)

**`BusEventEnvelope`** — 事件信封

```rust
pub struct BusEventEnvelope {
    pub event_type: String,                                    // 事件类型标识
    pub payload: String,                                       // JSON 序列化的载荷
    pub timestamp_ms: i64,                                     // 时间戳 (ms epoch)
}
```

**`SessionCreated`**

```rust
pub struct SessionCreated {
    pub session_id: String,
    pub owner_id: Option<String>,
    pub title: Option<String>,
    #[serde(default)]
    pub timestamp: i64,
}
// event_type: "session.created"
```

**`SessionMessagesAppended`**

```rust
pub struct SessionMessagesAppended {
    pub session_id: String,
    pub message_ids: Vec<String>,
    pub role: String,
    #[serde(default)]
    pub timestamp: i64,
}
// event_type: "session.messages_appended"
```

**`SessionStateChanged`**

```rust
pub struct SessionStateChanged {
    pub session_id: String,
    pub old_state: String,
    pub new_state: String,
    #[serde(default)]
    pub timestamp: i64,
}
// event_type: "session.state_changed"
```

**`AgentTurnStarted`**

```rust
pub struct AgentTurnStarted {
    pub session_id: String,
    pub turn_id: String,
    pub user_message: String,
    pub model: Option<String>,
    #[serde(default)]
    pub timestamp: i64,
}
// event_type: "agent.turn_started"
```

**`AgentTurnCompleted`**

```rust
pub struct AgentTurnCompleted {
    pub session_id: String,
    pub turn_id: String,
    pub success: bool,
    pub duration_ms: u64,
    pub tool_calls_count: usize,
    pub tokens_used: usize,
    #[serde(default)]
    pub timestamp: i64,
}
// event_type: "agent.turn_completed"
```

**`ToolExecuted`**

```rust
pub struct ToolExecuted {
    pub session_id: String,
    pub turn_id: String,
    pub tool_name: String,
    pub success: bool,
    pub duration_ms: u64,
    pub output_length: usize,
    #[serde(default)]
    pub timestamp: i64,
}
// event_type: "agent.tool_executed"
```

**`FileModified`**

```rust
pub enum FileOperationType { Created, Written, Deleted, Renamed }

pub struct FileModified {
    pub session_id: Option<String>,
    pub file_path: String,
    pub operation: FileOperationType,
    pub size_bytes: u64,
    #[serde(default)]
    pub timestamp: i64,
}
// event_type: "fs.file_modified", durable=true
```

**`InferenceCompleted`**

```rust
pub struct InferenceCompleted {
    pub session_id: Option<String>,
    pub model: String,
    pub provider: String,
    pub prompt_tokens: usize,
    pub completion_tokens: usize,
    pub latency_ms: u64,
    pub cost_usd: f64,
    #[serde(default)]
    pub timestamp: i64,
}
// event_type: "inference.completed", durable=true
```

**`SystemHealthChanged`**

```rust
pub enum SystemStatus { Healthy, Degraded, Down, Unknown }

pub struct SystemHealthChanged {
    pub component: String,
    pub status: SystemStatus,
    pub message: Option<String>,
    #[serde(default)]
    pub timestamp: i64,
}
// event_type: "system.health_changed"
```

---

#### 2.4.6 Memory Types

**`EnhancedMemoryEntry`** — 增强记忆条目

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnhancedMemoryEntry {
    #[serde(flatten)]
    pub base: MemoryEntry,                                     // 基础字段 (来自 memory.rs)
    #[serde(default = "default_confidence")]
    pub confidence: f32,                                       // 置信度 (0.0-1.0), 随时间衰减 (默认 1.0)
    #[serde(default)]
    pub strength: u32,                                         // 强化次数
    #[serde(default = "default_active")]
    pub active: bool,                                          // 是否活跃 (未被替代) (默认 true)
    #[serde(default)]
    pub superseded_by: Option<String>,                         // 替代此记忆的新记忆 ID
    #[serde(default)]
    pub reinforcements: Vec<Reinforcement>,                    // 强化痕迹
    #[serde(default)]
    pub scope: MemoryScope,                                    // 可见范围 (默认 Project)
    #[serde(default)]
    pub trust: TrustLevel,                                     // 信任等级 (默认 Medium)
    #[serde(default)]
    pub source_session: Option<String>,                        // 来源会话 ID (跨会话学习)
}
```

**`MemoryScope`** — 记忆可见范围

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum MemoryScope {
    #[default]
    Project,                                                   // 项目级别
    Global,                                                    // 全局级别
    All,                                                       // 全部可见
}
impl MemoryScope {
    pub fn includes_project(self) -> bool;
    pub fn includes_global(self) -> bool;
}
```

**`TrustLevel`** — 信任等级

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum TrustLevel { High, #[default] Medium, Low }
```

**`Reinforcement`** — 强化痕迹

```rust
pub struct Reinforcement {
    pub session_id: String,
    pub message_index: usize,
    pub timestamp: DateTime<Utc>,
}
```

**查询与更新类型**:

```rust
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EnhancedMemoryQuery {
    pub text_query: Option<String>,
    pub embedding: Option<Vec<f32>>,
    pub scope: Option<MemoryScope>,
    pub memory_type: Option<MemoryType>,
    pub min_trust: Option<TrustLevel>,
    #[serde(default = "default_true")]
    pub active_only: bool,                                     // 默认 true
    pub metadata_filter: Option<HashMap<String, String>>,
    pub tags: Option<Vec<String>>,
    pub created_after: Option<DateTime<Utc>>,
    pub created_before: Option<DateTime<Utc>>,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
    pub min_similarity: Option<f32>,
    pub sort_by: MemorySortField,                              // 默认 Relevance
    #[serde(default)]
    pub sort_desc: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum MemorySortField {
    #[default] Relevance, CreatedAt, UpdatedAt, Confidence, Strength, AccessCount,
}

pub struct VectorSearchOptions {
    pub min_similarity: f32,
    pub limit: usize,
    pub scope_filter: Option<MemoryScope>,
    #[serde(default)]
    pub include_inactive: bool,
}

pub struct VectorSearchResult {
    pub memory_id: String,
    pub similarity: f64,
    pub entry: EnhancedMemoryEntry,
}

pub struct EnhancedMemoryUpdate {
    pub content: Option<String>,
    pub metadata: Option<HashMap<String, String>>,
    pub tags: Option<Vec<String>>,
    pub scope: Option<MemoryScope>,
    pub trust: Option<TrustLevel>,
    pub active: Option<bool>,
}
```

**统计与清理**:

```rust
pub struct EnhancedMemoryStats {
    pub total_count: usize,
    pub count_by_scope: HashMap<MemoryScope, usize>,
    pub count_by_type: HashMap<MemoryType, usize>,
    pub count_by_trust: HashMap<TrustLevel, usize>,
    pub avg_confidence: f32,
    pub storage_size_bytes: u64,
    pub stale_count: usize,
    pub superseded_count: usize,
}

pub struct CleanupOptions {
    pub older_than: Option<DateTime<Utc>>,                     // 过期时间阈值
    pub below_confidence: Option<f32>,                         // 低置信度阈值
    pub max_prune: Option<usize>,                              // 最大清理条数
    pub hard_delete: bool,                                     // 是否硬删除
}

pub struct CleanupResult {
    pub pruned_count: usize,
    pub superseded_count: usize,
    pub freed_bytes: u64,
    pub errors: Vec<String>,
}
```

---

#### 2.4.7 Auth Types

> 定义于 `carpai_internal::auth`, re-exported via lib.rs。

```rust
pub struct AuthToken(pub String);
pub struct UserInfo {
    pub user_id: String,
    pub username: String,
    pub email: Option<String>,
    pub tier: UserTier,
}
pub enum Permission {
    ToolExecute(String),       // 执行特定工具
    FileSystemRead,            // 文件读取
    FileSystemWrite,           // 文件写入
    Admin,                     // 管理员
}
pub trait ApiKeyValidator: Send + Sync {
    async fn validate(&self, key: &str) -> Result<AuthToken, AuthError>;
}
pub enum UserTier { Free, Pro, Enterprise }
pub trait AuthProvider: Send + Sync {
    async fn authenticate(&self, token: &str) -> Result<UserInfo, AuthError>;
    async fn check_permission(&self, user_id: &str, permission: &Permission) -> Result<bool, AuthError>;
}
pub enum AuthError {
    InvalidToken,
    ExpiredToken,
    InsufficientPermissions { user: String, required: Permission },
    Internal(#[from] anyhow::Error),
}
```

---

#### 2.4.8 AgentContext — 中央 DI 容器

```rust
#[derive(Clone)]
pub struct AgentContext {
    // --- Core Services (全部 Arc<dyn Trait>) ---
    pub sessions: Arc<dyn SessionStore>,
    pub tools: Arc<dyn ToolExecutor>,
    pub inference: Arc<dyn InferenceBackend>,
    pub fs: Arc<dyn VirtualFileSystem>,
    pub events: Arc<dyn EventBus>,
    pub memory: Arc<dyn MemoryBackend>,
    pub completion: Option<Arc<dyn CodeCompletion>>,          // 可选
    pub auth: Arc<dyn AuthProvider>,

    // --- Identity & Scope ---
    pub config: AppConfig,
    pub user_id: String,
    pub session_id: Option<String>,                           // 每轮设置
    pub tenant_id: Option<String>,                            // server 模式多租户
    pub request_metadata: RequestMetadata,
}
```

**便捷方法**:

```rust
impl AgentContext {
    pub fn new(config, sessions, tools, inference, fs, events, memory, completion, auth, user_id) -> Self;
    pub fn for_session(&self, session_id: &str) -> Self;       // 克隆 + 设置 session_id
    pub fn for_request(&self, user_id, tenant_id, metadata) -> Self;
    pub fn is_server(&self) -> bool;
    pub fn is_cli(&self) -> bool;
    pub fn require_session_id(&self) -> &str;                  // panic if not set
    pub async fn publish_event<E: BusEvent>(&self, event: E);  // 便捷发布
    pub async fn has_permission(&self, permission: &Permission) -> Result<bool, AuthError>;
}
```

**`RequestMetadata`**:

```rust
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RequestMetadata {
    #[serde(default)] pub correlation_id: Option<String>,
    #[serde(default)] pub client_ip: Option<String>,
    #[serde(default)] pub user_agent: Option<String>,
    #[serde(default)] pub api_key_id: Option<String>,
    #[serde(default)] pub tags: Vec<String>,
}
```

**`AgentContextBuilder`**:

```rust
pub struct AgentContextBuilder { /* ... */ }

impl AgentContextBuilder {
    pub fn new(config: AppConfig) -> Self;                     // user_id 默认 "system"
    pub fn with_sessions(self, s: Arc<dyn SessionStore>) -> Self;
    pub fn with_tools(self, t: Arc<dyn ToolExecutor>) -> Self;
    pub fn with_inference(self, i: Arc<dyn InferenceBackend>) -> Self;
    pub fn with_fs(self, f: Arc<dyn VirtualFileSystem>) -> Self;
    pub fn with_events(self, e: Arc<dyn EventBus>) -> Self;
    pub fn with_memory(self, m: Arc<dyn MemoryBackend>) -> Self;
    pub fn with_completion(self, c: Arc<dyn CodeCompletion>) -> Self;
    pub fn with_auth(self, a: Arc<dyn AuthProvider>) -> Self;
    pub fn with_user_id(self, uid: &str) -> Self;
    pub fn build(self) -> Result<AgentContext, &'static str>;  // 验证必需字段
}
// 必需字段: sessions, tools, inference, fs, events, memory, auth
// 可选字段: completion
```

---

### 2.5 Error Types 汇总

| Error Enum | 定义位置 | 主要变体 |
|-----------|---------|---------|
| `SessionError` | session.rs | NotFound, InvalidTransition, Conflict, Storage, Serialization, QuotaExceeded, Internal |
| `ToolExecError` | tool_executor.rs | NotFound, InvalidParameters, PermissionDenied, ExecutionFailed, Timeout, Disabled, Sandbox, Cancelled, RateLimitExceeded, Internal |
| `InferenceError` | inference.rs (re-export) | (参见 inference 模块) |
| `FsError` | filesystem.rs | NotFound, PathEscape, PermissionDenied, AlreadyExists, NotEmpty, Io, NotAFile, NotADirectory, Encoding, Unsupported, QuotaExceeded, Internal |
| `EventBusError` | event_bus.rs | SubscriptionFailed, PublishFailed, ConnectionLost, Deserialization, ChannelClosed, Internal |
| `MemoryError` | memory.rs (re-export) | (参见 memory 模块) |
| `AuthError` | auth.rs | InvalidToken, ExpiredToken, InsufficientPermissions, Internal |
| `ConfigError` | config.rs (carpai-core) | Io, Parse |

---

## 3. Usage Examples

### 3.1 Minimal Hello World

```rust
use carpai_core::{build_local_agent_context, execute_agent_turn, CoreConfig};
use std::path::PathBuf;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = CoreConfig::load(&PathBuf::from("~/.carpai/config.toml"))
        .unwrap_or_else(|_| CoreConfig::default());

    let ctx = build_local_agent_context(&config);

    let output = execute_agent_turn(&ctx, "Hello, CarpAI!").await?;
    println!("{}", output.text);
    println!("Tokens: {:?}", output.usage);
    println!("Duration: {}ms", output.duration_ms);

    Ok(())
}
```

### 3.2 Custom Configuration (TOML + Env Override)

**`~/.carpai/config.toml`**:

```toml
[base]
mode = "cli"
default_model = "claude-sonnet-4-20250514"
max_context_tokens = 200000
tools_enabled = true
vfs_enabled = true
memory_enabled = true

[data_dir]
# 将被 CARPAI_CORE__DATA_DIR 环境变量覆盖
# session_subdir = "sessions"
# memory_subdir = "memory"

[max_concurrent_tools]
# 将被 CARPAI_CORE__MAX_CONCURRENT_TOOLS 环境变量覆盖

[completion_provider]
provider_type = "anthropic"
endpoint = "https://api.anthropic.com"
model = "claude-sonnet-4-20250514"
timeout_secs = 60
```

**环境变量覆盖**:

```bash
export CARPAI_CORE__DATA_DIR=/data/carpai
export CARPAI_CORE__MAX_CONCURRENT_TOOLS=10
export CARPAI_DEFAULT_MODEL="claude-opus-4-20250514"
```

**Rust 加载**:

```rust
let config = CoreConfig::load(&PathBuf::from("~/.carpai/config.toml"))?;
// 此时: data_dir="/data/carpai" (env > toml > default)
//       max_concurrent_tools=10 (env > toml > default)
//       default_model="claude-opus-4-20250514" (env > toml > default)
```

### 3.3 Error Handling

```rust
use carpai_core::{execute_agent_turn, build_local_agent_context, CoreConfig};
use carpai_internal::{SessionError, ToolExecError};

async fn run_with_error_handling() -> anyhow::Result<()> {
    let config = CoreConfig::default();
    let ctx = build_local_agent_context(&config);

    match execute_agent_turn(&ctx, "Refactor this code").await {
        Ok(output) => {
            println!("Success: {} ({} tokens)", output.text, output.usage.total_tokens);
            for tc in &output.tool_calls {
                println!("  Tool: {} -> {}", tc.name, tc.status);
            }
        }
        Err(e) => {
            let err_str = e.to_string();

            if err_str.contains("Session error") {
                eprintln!("会话错误: 无法创建或加载会话");
            } else if err_str.contains("Inference failed") {
                eprintln!("推理失败: 检查模型配置和网络连接");
            } else if err_str.contains("Tool execution failed") {
                eprintln!("工具执行失败: 检查工具权限和沙箱配置");
            } else {
                eprintln!("未知错误: {}", e);
            }
        }
    }

    Ok(())
}
```

### 3.4 Using AgentContextBuilder Directly

```rust
use carpai_internal::{
    AgentContextBuilder, AppConfig, AppMode, ExecutionMode,
    SessionStore, ToolExecutor, InferenceBackend,
    VirtualFileSystem, EventBus, MemoryBackend, AuthProvider,
};
use std::sync::Arc;

async fn custom_assembly(
    sessions: Arc<dyn SessionStore>,
    tools: Arc<dyn ToolExecutor>,
    inference: Arc<dyn InferenceBackend>,
    fs: Arc<dyn VirtualFileSystem>,
    events: Arc<dyn EventBus>,
    memory: Arc<dyn MemoryBackend>,
    auth: Arc<dyn AuthProvider>,
) -> anyhow::Result<AgentContext> {
    let config = AppConfig {
        mode: AppMode::Server,
        ..AppConfig::default()
    };

    let ctx = AgentContextBuilder::new(config)
        .with_sessions(sessions)
        .with_tools(tools)
        .with_inference(inference)
        .with_fs(fs)
        .with_events(events)
        .with_memory(memory)
        .with_auth(auth)
        .with_user_id("admin-user")
        .build()?;  // 验证所有必需字段

    Ok(ctx)
}
```

---

## 4. Serialization Formats

### 4.1 CoreConfig JSON Example

```json
{
  "base": {
    "mode": "cli",
    "data_dir": ".jcode/data",
    "working_dir": ".",
    "default_model": "default",
    "max_context_tokens": 200000,
    "tools_enabled": true,
    "default_tool_mode": "Local",
    "vfs_enabled": true,
    "vfs_root": null,
    "memory_enabled": true,
    "event_bus_enabled": true
  },
  "data_dir": "C:\\Users\\user\\.carpai",
  "session_subdir": "sessions",
  "memory_subdir": "memory",
  "max_concurrent_tools": 5,
  "max_agent_iterations": 100,
  "completion_provider": {
    "provider_type": "local",
    "endpoint": "http://localhost:11434",
    "api_key": null,
    "model": null,
    "timeout_secs": 30
  },
  "cache_size_mb": 512,
  "disk_cache_enabled": true
}
```

### 4.2 SessionMeta JSON Example

```json
{
  "id": { "0": "sess-a1b2c3d4" },
  "parent_id": null,
  "title": "Rust Refactoring Session",
  "created_at": "2026-05-24T10:00:00Z",
  "updated_at": "2026-05-24T12:30:00Z",
  "last_active_at": "2026-05-24T12:30:00Z",
  "state": "Active",
  "model": "claude-sonnet-4-20250514",
  "working_dir": "D:\\studying\\Codecargo\\CarpAI",
  "message_count": 42,
  "owner_id": "user-123",
  "tags": { "project": "carpai", "language": "rust" }
}
```

### 4.3 ChatCompletionResponse JSON Example

```json
{
  "id": "chatcmpl-abc123",
  "object": "chat.completion",
  "created": 1716580800,
  "model": "claude-sonnet-4-20250514",
  "choices": [
    {
      "index": 0,
      "message": {
        "role": "Assistant",
        "content": { "Text": "Here's the refactored code..." },
        "name": null
      },
      "finish_reason": "stop",
      "logprobs": null
    }
  ],
  "usage": {
    "prompt_tokens": 15000,
    "completion_tokens": 800,
    "total_tokens": 15800,
    "cache_creation_input_tokens": null,
    "cache_read_input_tokens": null
  },
  "provider": "anthropic",
  "fallback_info": null
}
```

### 4.4 ToolRequest JSON Example

```json
{
  "tool_name": "read_file",
  "parameters": {
    "path": "src/main.rs"
  },
  "context": {
    "user_id": "user-123",
    "session_id": "sess-a1b2c3d4",
    "working_dir": "D:\\studying\\Codecargo\\CarpAI",
    "env_vars": {},
    "timeout": null,
    "require_confirmation": false,
    "metadata": {}
  },
  "request_id": "req-f4e5d6c7",
  "mode_override": null
}
```

### 4.5 ToolResponse JSON Example

```json
{
  "success": true,
  "output": "fn main() {\n    println!(\"Hello, world!\");\n}\n",
  "data": {
    "path": "src/main.rs",
    "line_count": 3,
    "language": "rust"
  },
  "exit_code": 0,
  "duration_ms": 15,
  "request_id": "req-f4e5d6c7",
  "tool_name": "read_file",
  "audit_id": "audit-xyz789"
}
```

---

## 5. Change Log Template

## Changelog

### v1.0 (Week 3 Frozen) — 2026-05-24

**初始发布**: 7 个核心 Trait + 6 个 Local 实现

- **Trait Layer (carpai-internal)**:
  - `SessionStore` — 12 方法 CRUD + 状态机 + 压缩
  - `ToolExecutor` — 6 方法 (execute, list, validate, permission, cancel)
  - `InferenceBackend` — 7 方法 (complete, stream, routing, quota, fallback)
  - `VirtualFileSystem` — 20 方法 (文件/目录/搜索/Git/watch/安全)
  - `EventBus` — 5 方法 (publish, subscribe, health_check) + 9 种内置事件
  - `MemoryBackend` — 13 方法 (CRUD + 向量搜索 + 去重 + 强化 + 分层)
  - `CodeCompletion` — IDE 补全接口 (base trait)

- **Business Logic (carpai-core)**:
  - `CoreConfig` — 三层加载 (defaults → TOML → env vars, `CARPAI_*` 前缀)
  - `execute_agent_turn()` — Agent 主循环入口
  - `build_local_agent_context()` — CLI 模式 DI 组装
  - `AgentTurnOutput` / `ToolCallInfo` — 交互输出结构

- **Tool System**:
  - MCP 协议支持 (`McpServer`, `McpClient`, `McpManager`, `McpBridge`)
  - `ToolRegistry` / `SlashCommandRegistry`
  - 10 种 `ToolCategory` 分类
  - 4 种 `ExecutionMode` (Local/Sandboxed/Remote/DryRun)

- **Completion System**:
  - `SmartCompleter` (FIM 补全)
  - `AutoFallbackRouter` (多提供者自动回退)
  - `CompletionEngine` / `CompletionProvider`

- **Central Assembly**:
  - `AgentContext` — 8 个 `Arc<dyn Trait>` 字段 + 身份/作用域
  - `AgentContextBuilder` — Builder 模式 + 验证
  - `AppConfig` / `AppMode` / `RequestMetadata`

### v1.1 (Proposed)

- TBD based on integration feedback from ma-guoyang / Paw-brave teams
- Potential areas:
  - Streaming agent turns (SSE/WebSocket)
  - Multi-session orchestration
  - Rate limiting at trait level
  - Observability hooks (tracing/metrics)

---

## 6. Compatibility Guarantees

### Stability Tiers

| Tier | 说明 | 示例 | Breaking Change Policy |
|------|------|------|----------------------|
| **Stable** | 公开 API, 可安全依赖 | 7 个 Trait 的方法签名、核心 Struct 字段 | 需要 semver minor+ 版本升级 |
| **Provisional** | 功能完整但可能微调 | 辅助类型的默认值、内部实现细节 | 可能 patch 版本调整 |
| **Experimental** | 开发中, 可能大幅变更 | Phase 1B/1C/1D 新增子系统 | 不保证稳定性 |

### Stable API Surface (v1.0 冻结范围)

以下在 v1.0 中 **不会发生 breaking change**:

- ✅ 7 个 Trait 的所有 `async fn` 方法签名 (名称、参数类型、返回类型)
- ✅ 所有 public struct 的 **已有字段名和类型**
- ✅ 所有 enum 的 **已有 variant 名称**
- ✅ `CoreConfig` 的三层加载机制和 `CARPAI_*` 环境变量前缀
- ✅ `execute_agent_turn()` 和 `build_local_agent_context()` 的函数签名
- ✅ `AgentContext` 的 8 个 service 字段名
- ✅ `AgentContextBuilder` 的 builder 方法名
- ✅ 所有 Error enum 的 **已有 variant**
- ✅ 9 种内置事件的 `event_type()` 返回值

### Deprecation Policy

1. 废弃标记期 ≥ 2 周 (对于 weekly release cycle)
2. 废弃项在文档中标注 `@deprecated since v1.x`
3. 废弃项在下个 minor version 中移除
4. 不接受 silent breaking changes

### Versioning Scheme (Semver)

```
MAJOR.MINOR.PATCH

MAJOR: 破坏性 API 变更 (Trait 签名删除/重命名、Struct 必需字段删除)
MINOR: 向后兼容新增 (新 Trait 方法带 default impl、新增 optional 字段、新 event type)
PATCH: Bug fix、内部实现变更、文档更新
```

### 当前版本信息

- **Contract Version**: v1.0
- **对应代码基线**: CarpAI refactoring v3.0 Phase 1A+1D
- **冻结日期**: 2026-05-24 (Week 3)
- **下次评审**: Week 5 (基于 ma-guoyang/Paw-brave 集成反馈)

### 下游团队集成 Checklist

- [ ] ma-guoyang: 确认 `PgSessionStore` / `RedisEventBus` 实现可对接上述 Trait
- [ ] Paw-brave: 确认 TUI/SDK 可通过 `execute_agent_turn()` + `stream_chat()` 驱动 UI
- [ ] 双方: 确认 `CoreConfig` TOML 格式满足各自产品配置需求
- [ ] 双方: 确认事件类型 (`BusEvent`) 满足监控/日志需求
- [ ] 双方: 确认错误类型 (`*Error` enums) 满足错误展示/恢复策略需求
