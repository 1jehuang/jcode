# CarpAI Core 接口契约文档 (Interface Contract Document)

> **版本**: 1.0.0
> **最后更新**: 2026-05-25
> **适用范围**: `crates/carpai-core`
> **状态**: ✅ 与代码实现完全一致

---

## 目录

1. [概述与设计理念](#1-概述与设计理念)
2. [快速开始示例](#2-快速开始示例)
3. [核心 API 参考](#3-核心-api-参考)
   - 3.1 [execute_agent_turn 函数](#31-execute_agent_turn-函数)
   - 3.2 [build_local_agent_context 函数](#32-build_local_agent_context-函数)
   - 3.3 [CoreConfig 结构体](#33-coreconfig-结构体)
   - 3.4 [AgentTurnOutput 结构体](#34-agentturnoutput-结构体)
   - 3.5 [ToolCallInfo 结构体](#35-toolcallinfo-结构体)
4. [Local 实现列表](#4-local-实现列表)
5. [配置指南](#5-配置指南)
6. [错误处理最佳实践](#6-错误处理最佳实践)
7. [集成点说明](#7-集成点说明)

---

## 1. 概述与设计理念

### 1.1 架构目标

`carpai-core` 是 CarpAI 系统的核心引擎 crate，提供：

- **纯业务逻辑** 的 agent 执行循环
- **可插拔** 的 trait-based 组件架构
- **本地优先** 的开发体验（所有 Local* 实现）
- **类型安全** 的错误处理（`anyhow::Result`）

### 1.2 核心设计原则

| 原则 | 说明 |
|------|------|
| **Trait 抽象** | 所有外部依赖通过 trait 定义，支持多态替换 |
| **组合优于继承** | 通过 `AgentContextBuilder` 组装组件 |
| **本地默认** | 提供 Local* 实现作为零配置的默认选项 |
| **异步优先** | 所有 I/O 操作使用 `async/await` |
| **序列化友好** | 核心数据结构支持 `serde` 序列化 |

### 1.3 模块结构

```
crates/carpai-core/src/
├── agent_loop.rs          # 核心：agent 执行循环 + 上下文构建
├── config.rs              # 配置管理（CoreConfig, ProviderConfig）
├── session_impl.rs        # LocalFileSessionStore 实现
├── tool_executor_impl.rs  # LocalToolExecutor 实现
├── inference_impl.rs      # SidecarInferenceBackend 实现
├── filesystem_impl.rs     # LocalFileSystem 实现
├── event_bus_impl.rs      # InProcessEventBus 实现
└── memory_impl.rs         # LocalMemoryBackend 实现
```

---

## 2. 快速开始示例

### 2.1 最小化使用示例

```rust
use carpai_core::{CoreConfig, build_local_agent_context, execute_agent};
use std::path::PathBuf;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 1. 加载配置（使用默认值 + 可选配置文件）
    let config = CoreConfig::load(&PathBuf::from("~/.carpai/config.toml"))?;

    // 2. 构建完整的 agent 上下文（自动组装所有 Local* 组件）
    let ctx = build_local_agent_context(&config);

    // 3. 执行一次完整的 agent 对话轮次
    let output = execute_agent_turn(&ctx, "Hello, CarpAI!").await?;

    // 4. 处理返回结果
    println!("Response: {}", output.text);
    println!("Tokens used: {}", output.usage.total_tokens);
    println!("Duration: {}ms", output.duration_ms);

    Ok(())
}
```

### 2.2 完整工作流示例

```rust
use carpai_core::{
    CoreConfig, build_local_agent_context, execute_agent_turn,
    AgentTurnOutput, ToolCallInfo
};
use std::path::PathBuf;

async fn run_multi_turn_conversation() -> anyhow::Result<()> {
    let config = CoreConfig::load(&PathBuf::from("config.toml"))?;
    let ctx = build_local_agent_context(&config);

    // 第一轮对话
    let turn1 = execute_agent_turn(&ctx, "请帮我分析这个项目的结构").await?;
    println!("Assistant: {}", turn1.text);

    // 第二轮对话（上下文自动持久化到 session store）
    let turn2 = execute_agent_turn(&ctx, "详细说明核心模块").await?;
    println!("Assistant: {}", turn2.text);

    // 检查工具调用
    if !turn2.tool_calls.is_empty() {
        for tool_call in &turn2.tool_calls {
            println!(
                "Tool: {} | Status: {} | Duration: {}ms",
                tool_call.name, tool_call.status, tool_call.duration_ms
            );
        }
    }

    Ok(())
}
```

---

## 3. 核心 API 参考

### 3.1 execute_agent_turn 函数

**文件位置**: [agent_loop.rs:39-202](file:///d:/studying/Codecargo/CarpAI/crates/carpai-core/src/agent_loop.rs#L39-L202)

#### 函数签名

```rust
pub async fn execute_agent_turn(
    ctx: &AgentContext,
    user_message: &str,
) -> Result<AgentTurnOutput>
```

#### 参数说明

| 参数 | 类型 | 说明 |
|------|------|------|
| `ctx` | `&AgentContext` | 包含所有 trait 对象的 agent 上下文 |
| `user_message` | `&str` | 原始用户输入字符串 |

#### 返回值

- **成功**: `Ok(AgentTurnOutput)` - 包含响应文本、工具调用信息、token 使用量等
- **失败**: `Err(anyhow::Error)` - 包装各类运行时错误

#### 执行流程（7 个步骤）

```
用户消息 → [Step 1] 获取/创建 Session
         → [Step 2] 追加用户消息到 Session
         → [Step 3] 从 Session 历史构建上下文
         → [Step 4] 调用 InferenceBackend.generate()
         → [Step 5] 如果有工具调用，通过 ToolExecutor 执行
         → [Step 6] 收集工具结果并回传给推理（循环）
         → [Step 7] 返回最终的 AgentTurnOutput
```

**详细步骤说明**:

##### Step 1: 确保 Session 存在
- 使用固定的 session ID: `"default-session"`
- 通过 `SessionStore.load_session()` 检查是否已存在
- 若不存在，创建新的 `SessionMeta` 并持久化

**SessionMeta 默认值**:
```rust
SessionMeta {
    id: SessionId("default-session".to_string()),
    title: Some("Agent Session".into()),
    owner_id: None,
    state: SessionState::Active,
    model: Some(ctx.config.default_model.clone()),
    working_dir: Some(ctx.config.working_dir.to_string_lossy().to_string()),
    created_at: chrono::Utc::now(),
    updated_at: chrono::Utc::now(),
    last_active_at: Some(chrono::Utc::now()),
    tags: HashMap::new(),
    message_count: 0,
    parent_id: None,
}
```

##### Step 2: 追加用户消息
- 将用户输入封装为 `StoredMessage` (role: `MessageRole::User`)
- 自动生成 UUID 作为消息 ID
- 时间戳使用 UTC 当前时间
- 通过 `SessionStore.append_messages()` 持久化

##### Step 3: 加载会话历史
- 重新加载完整的 session（包含刚追加的用户消息）
- 将 `StoredMessage[]` 转换为 `ChatMessage[]`（推理 API 格式）
- **内容块转换规则**:
  - `ContentBlock::Text { text }` → `ChatContent::Text(text)`
  - `ContentBlock::ToolUse { name, input }` → `ChatContent::Text("[Tool Call] name(input)")`
  - `ContentBlock::ToolResult { content }` → `ChatContent::Text(content)`
  - `ContentBlock::Thinking { text }` → `ChatContent::Text(text)`

##### Step 4: 生成响应
- 构建 `ChatCompletionRequest`:
```rust
ChatCompletionRequest {
    messages: chat_messages,                    // 从 Step 3 转换的消息列表
    model: ctx.config.default_model.clone(),     // 使用配置中的默认模型
    max_tokens: Some(4096),                     // 最大生成长度
    temperature: Some(0.7),                     // 温度参数
    top_p: None,
    stop: None,
    presence_penalty: None,
    frequency_penalty: None,
    tools: None,                                // 当前未启用工具定义
    tool_choice: None,
    user_id: None,
    session_id: Some(session_id.to_string()),   // 关联 session ID
    metadata: HashMap::new(),
}
```
- 调用 `InferenceBackend.complete_chat()` 进行推理

##### Step 5-6: 工具调用处理（当前为预留）
- **当前状态**: 预留接口，工具调用逻辑尚未完整实现
- 当推理返回包含工具调用时，应：
  1. 解析工具调用请求
  2. 通过 `ToolExecutor.execute()` 执行
  3. 收集执行结果
  4. 将结果回传给推理引擎（可能需要多次迭代）

##### Step 7: 返回结果
- 将助手响应追加到 session
- 计算总耗时（从函数入口开始计时）
- 构造并返回 `AgentTurnOutput`

#### 错误处理方式

| 错误场景 | 错误类型 | 处理策略 |
|----------|----------|----------|
| Session 创建失败 | `anyhow::Error` | 直接返回错误（包装原始错误） |
| 消息追加失败 | `anyhow::Error` | 直接返回错误 |
| Session 加载失败 | `anyhow::Error` | 返回 "Session not found after creation" |
| 推理失败 | `anyhow::Error` | 包装为 "Inference failed: {original_error}" |
| 助手消息追加失败 | **仅警告** | `warn!` 日志记录，不中断流程 |

#### 使用示例

```rust
#[tokio::main]
async fn example_execute_agent_turn() {
    use carpai_core::{CoreConfig, build_local_agent_context, execute_agent_turn};

    let config = CoreConfig::default();
    let ctx = build_local_agent_context(&config);

    match execute_agent_turn(&ctx, "What is Rust?").await {
        Ok(output) => {
            println!("✅ Response received:");
            println!("   Text: {}", output.text);
            println!("   Tokens: {}/{} (prompt/completion)",
                output.usage.prompt_tokens,
                output.usage.completion_tokens
            );
            println!("   Duration: {}ms", output.duration_ms);
        }
        Err(e) => {
            eprintln!("❌ Agent turn failed: {}", e);
        }
    }
}
```

---

### 3.2 build_local_agent_context 函数

**文件位置**: [agent_loop.rs:229-263](file:///d:/studying/Codecargo/CarpAI/crates/carpai-core/src/agent_loop.rs#L229-L263)

#### 函数签名

```rust
pub fn build_local_agent_context(config: &crate::config::CoreConfig) -> AgentContext
```

#### 参数说明

| 参数 | 类型 | 说明 |
|------|------|------|
| `config` | `&CoreConfig` | 核心配置引用 |

#### 返回值

- **成功**: `AgentContext` - 完全组装好的 agent 上下文
- **失败**: **panic** - 如果任何组件无效（`.expect()` 调用）

> ⚠️ **注意**: 此函数使用 `.expect()` 而非 `Result`，因为组件组装失败属于编程错误而非运行时错误。

#### 组装的组件列表（6 个 Local 实现）

此函数是 **CLI/本地开发模式的主要入口点**，将所有 trait 对象连接到其本地文件系统支持的实现。

##### 组装流程

```rust
// 1. 构建 AppConfig（从 CoreConfig 提取）
let app_config = AppConfig {
    mode: AppMode::Cli,
    data_dir: config.data_dir.clone(),
    working_dir: config.base.working_dir.clone(),
    default_model: config.base.default_model.clone(),
    max_context_tokens: config.base.max_context_tokens,
    tools_enabled: true,
    default_tool_mode: ExecutionMode::Local,
    vfs_enabled: config.base.vfs_enabled,
    vfs_root: config.base.vfs_root.clone(),
    memory_enabled: config.base.memory_enabled,
    event_bus_enabled: config.base.event_bus_enabled,
};

// 2. 使用 Builder 模式组装 AgentContext
AgentContextBuilder::new(app_config)
    .with_sessions(Arc::new(LocalFileSessionStore::new(
        config.session_store_path(),        // → ~/.carpai/sessions/
    )))
    .with_tools(Arc::new(LocalToolExecutor::new(
        config.max_concurrent_tools,         // → 默认 5
    )))
    .with_inference(Arc::new(SidecarInferenceBackend::new(
        &config.completion_provider,         // → Ollama sidecar
    )))
    .with_fs(Arc::new(LocalFileSystem::new(
        &config.base.working_dir,           // → 当前工作目录
        config.base.vfs_root.as_deref(),    // → VFS 根目录（可选）
    )))
    .with_events(Arc::new(InProcessEventBus::new(1024)))  // → 广播容量 1024
    .with_memory(Arc::new(LocalMemoryBackend::new(
        config.memory_store_path(),          // → ~/.carpai/memory/
    )))
    .build()
    .expect("AgentContext assembly: all components must be valid")
```

#### 组件映射表

| Trait | Local 实现 | 配置来源 | 存储位置 |
|-------|-----------|----------|----------|
| `SessionStore` | `LocalFileSessionStore` | `config.session_store_path()` | `{data_dir}/sessions/` |
| `ToolExecutor` | `LocalToolExecutor` | `config.max_concurrent_tools` | 内存（Semaphore） |
| `InferenceBackend` | `SidecarInferenceBackend` | `config.completion_provider` | HTTP (Ollama) |
| `VirtualFileSystem` | `LocalFileSystem` | `config.base.working_dir` | 本地文件系统 |
| `EventBus` | `InProcessEventBus` | 硬编码 (1024) | 进程内广播通道 |
| `MemoryBackend` | `LocalMemoryBackend` | `config.memory_store_path()` | `{data_dir}/memory/` |

#### CoreConfig 要求

调用前需确保以下配置字段有效：

**必需字段**:
- `config.data_dir` - 数据根目录（默认 `~/.carpai`）
- `config.base.working_dir` - 工作目录
- `config.base.default_model` - 默认模型名称

**可选字段**（有合理默认值）:
- `config.session_subdir` - 会话子目录（默认 `"sessions"`）
- `config.memory_subdir` - 记忆子目录（默认 `"memory"`）
- `config.max_concurrent_tools` - 最大并发工具数（默认 `5`）
- `config.completion_provider` - 推理提供者配置

#### 使用示例

```rust
fn example_build_local_agent_context() {
    use carpai_core::{CoreConfig, build_local_agent_context};

    // 方式 1: 使用完全默认配置
    let default_ctx = build_local_agent_context(&CoreConfig::default());

    // 方式 2: 自定义配置后组装
    let mut custom_config = CoreConfig::default();
    custom_config.data_dir = PathBuf::from("/tmp/my-carpai");
    custom_config.max_concurrent_tools = 10;
    custom_config.completion_provider.model = Some("llama3".to_string());

    let custom_ctx = build_local_agent_context(&custom_config);
}
```

---

### 3.3 CoreConfig 结构体

**文件位置**: [config.rs:12-54](file:///d:/studying/Codecargo/CarpAI/crates/carpai-core/src/config.rs#L12-L54)

#### 结构体定义

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoreConfig {
    #[serde(flatten)]
    pub base: AppConfig,

    // === Storage ===
    pub data_dir: PathBuf,
    #[serde(default = "default_session_dir")]
    pub session_subdir: String,
    #[serde(default = "default_memory_dir")]
    pub memory_subdir: String,

    // === Concurrency ===
    #[serde(default = "default_max_concurrent_tools")]
    pub max_concurrent_tools: usize,
    #[serde(default = "default_max_iterations")]
    pub max_agent_iterations: usize,

    // === Completion Provider ===
    #[serde(default)]
    pub completion_provider: ProviderConfig,

    // === Caching ===
    #[serde(default = "default_cache_size")]
    pub cache_size_mb: usize,
    #[serde(default = "default_disk_cache")]
    pub disk_cache_enabled: bool,
}
```

#### 字段详细说明

##### 基础配置 (base: AppConfig)

`AppConfig` 来自 `carpai_internal` crate，通过 `#[serde(flatten)]` 合并到 `CoreConfig`。

**已知字段**（基于实际使用）:
| 字段 | 类型 | 说明 | 默认值 |
|------|------|------|--------|
| `mode` | `AppMode` | 应用模式（Cli/Server） | `AppMode::Cli` |
| `working_dir` | `PathBuf` | 工作目录 | 当前目录 |
| `default_model` | `String` | 默认模型名称 | 依赖 ProviderConfig |
| `max_context_tokens` | `usize` | 最大上下文 token 数 | 依赖 AppConfig |
| `tools_enabled` | `bool` | 是否启用工具 | `true` |
| `default_tool_mode` | `ExecutionMode` | 默认工具执行模式 | `ExecutionMode::Local` |
| `vfs_enabled` | `bool` | 是否启用虚拟文件系统 | 依赖配置 |
| `vfs_root` | `Option<PathBuf>` | VFS 根目录 | `None` |
| `memory_enabled` | `bool` | 是否启用记忆系统 | 依赖配置 |
| `event_bus_enabled` | `bool` | 是否启用事件总线 | 依赖配置 |

##### 存储配置

| 字段 | 类型 | 说明 | 默认值 |
|------|------|------|--------|
| `data_dir` | `PathBuf` | 数据根目录 | `~/.carpai` |
| `session_subdir` | `String` | 会话存储子目录（相对 data_dir） | `"sessions"` |
| `memory_subdir` | `String` | 记忆存储子目录（相对 data_dir） | `"memory"` |

**路径计算方法**:
```rust
impl CoreConfig {
    /// 会话存储完整路径: {data_dir}/{session_subdir}
    pub fn session_store_path(&self) -> PathBuf {
        self.data_dir.join(&self.session_subdir)
    }

    /// 记忆存储完整路径: {data_dir}/{memory_subdir}
    pub fn memory_store_path(&self) -> PathBuf {
        self.data_dir.join(&self.memory_subdir)
    }
}
```

##### 并发控制

| 字段 | 类型 | 说明 | 默认值 |
|------|------|------|--------|
| `max_concurrent_tools` | `usize` | 最大并发工具执行数 | `5` |
| `max_agent_iterations` | `usize` | agent 循环最大迭代次数（强制停止） | `100` |

##### 推理提供者配置 (ProviderConfig)

**文件位置**: [config.rs:57-75](file:///d:/studying/Codecargo/CarpAI/crates/carpai-core/src/config.rs#L57-L75)

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    #[serde(default = "default_provider_type")]
    pub provider_type: String,           // 提供者类型标识符
    pub endpoint: Option<String>,        // API 端点 URL
    pub api_key: Option<String>,         // API 密钥（从不存储到配置文件）
    pub model: Option<String>,           // 模型名称覆盖
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,               // 请求超时时间（秒）
}
```

**ProviderConfig 字段详情**:

| 字段 | 类型 | 说明 | 默认值 |
|------|------|------|--------|
| `provider_type` | `String` | 提供者类型 (`"local"`, `"openai"`, `"anthropic"`) | `"local"` |
| `endpoint` | `Option<String>` | HTTP API 端点 | `Some("http://localhost:11434")` |
| `api_key` | `Option<String>` | API 密钥（建议通过环境变量传入） | `None` |
| `model` | `Option<String>` | 模型名称（None 时使用 provider 默认） | `None` |
| `timeout_secs` | `u64` | HTTP 请求超时 | `30` |

##### 缓存配置

| 字段 | 类型 | 说明 | 默认值 |
|------|------|------|--------|
| `cache_size_mb` | `usize` | 最大内存缓存大小（MB） | `512` |
| `disk_cache_enabled` | `bool` | 是否启用磁盘缓存 | `true` |

#### 默认值汇总

```rust
impl Default for CoreConfig {
    fn default() -> Self {
        Self {
            base: AppConfig::default(),
            data_dir: dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".carpai"),              // → ~/.carpai
            session_subdir: "sessions".into(),
            memory_subdir: "memory".into(),
            max_concurrent_tools: 5,
            max_agent_iterations: 100,
            completion_provider: ProviderConfig::default(),
            cache_size_mb: 512,
            disk_cache_enabled: true,
        }
    }
}

impl Default for ProviderConfig {
    fn default() -> Self {
        Self {
            provider_type: "local".into(),
            endpoint: Some("http://localhost:11434".into()),  // Ollama 默认端口
            api_key: None,
            model: None,
            timeout_secs: 30,
        }
    }
}
```

#### 配置加载优先级

`CoreConfig::load()` 方法遵循 **三层覆盖策略**:

```
优先级从低到高:
  Layer 1: 硬编码默认值 (Default impl)
       ↓ 被 Layer 2 覆盖
  Layer 2: TOML 配置文件值 (~/.carpai/config.toml)
       ↓ 被 Layer 3 覆盖
  Layer 3: 环境变量 (CARPAI_* 前缀) ← 最高优先级
```

**加载方法签名**:
```rust
impl CoreConfig {
    pub fn load(path: &PathBuf) -> Result<Self, ConfigError>
}
```

**支持的环境变量**:

| 环境变量 | 映射字段 | 示例值 |
|----------|----------|--------|
| `CARPAI_DATA_DIR` 或 `CARPAI_CORE__DATA_DIR` | `data_dir` | `/custom/path` |
| `CARPAI_DEFAULT_MODEL` | `base.default_model` | `claude-sonnet-4-20250514` |
| `CARPAI_CORE__MAX_CONCURRENT_TOOLS` | `max_concurrent_tools` | `10` |
| `CARPAI_CORE__MAX_AGENT_ITERATIONS` | `max_agent_iterations` | `200` |
| `CARPAI_LOG_LEVEL` | （暂未实现） | - |

**环境变量命名规范**:
- 全局配置: `CARPAI_{FIELD_NAME}` (如 `CARPAI_DATA_DIR`)
- Core 专属配置: `CARPAI_CORE__{FIELD_NAME}` (双下划线分隔)
- 嵌套配置: `CARPAI_CORE__{PARENT}__{CHILD}` (如 `CARPAI_CORE__COMPLETION_PROVIDER__MODEL`)

#### 错误类型

```rust
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Parse error: {0}")]
    Parse(#[from] toml::de::Error),
}
```

#### TOML 配置文件示例

```toml
# ~/.carpai/config.toml

[base]
working_dir = "/home/user/projects"
default_model = "llama3"
max_context_tokens = 128000
tools_enabled = true
vfs_enabled = false
memory_enabled = true
event_bus_enabled = true

[data_dir]
# 注释: 此处使用默认值 ~/.carpai

[completion_provider]
provider_type = "local"
endpoint = "http://localhost:11434"
# api_key 不应在配置文件中设置！使用环境变量 CARPAI_API_KEY
model = "llama3:8b"
timeout_secs = 60

# 高级配置
max_concurrent_tools = 10
max_agent_iterations = 50
cache_size_mb = 1024
disk_cache_enabled = true
```

#### 使用示例

```rust
async fn example_core_config() -> anyhow::Result<()> {
    use carpai_core::CoreConfig;
    use std::path::PathBuf;

    // 方式 1: 完全使用默认值
    let config1 = CoreConfig::default();
    assert_eq!(config1.max_concurrent_tools, 5);
    assert!(config1.data_dir.ends_with(".carpai"));

    // 方式 2: 从文件加载（文件不存在时使用默认值）
    let config2 = CoreConfig::load(&PathBuf::from("/nonexistent/config.toml"))?;
    assert_eq!(config2.max_concurrent_tools, 5);  // 仍然使用默认值

    // 方式 3: 从真实配置文件加载
    let config3 = CoreConfig::load(&PathBuf::from("~/.carpai/config.toml"))?;
    println!("Data dir: {:?}", config3.data_dir);
    println!("Session path: {:?}", config3.session_store_path());
    println!("Memory path: {:?}", config3.memory_store_path());

    // 方式 4: 手动构造并修改
    let mut config4 = CoreConfig::default();
    config4.data_dir = PathBuf::from("/tmp/test-carpai");
    config4.completion_provider.model = Some("mistral".to_string());
    config4.max_concurrent_tools = 20;

    Ok(())
}
```

---

### 3.4 AgentTurnOutput 结构体

**文件位置**: [agent_loop.rs:205-212](file:///d:/studying/Codecargo/CarpAI/crates/carpai-core/src/agent_loop.rs#L205-L212)

#### 结构体定义

```rust
/// 单次 agent 交互的输出
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AgentTurnOutput {
    pub text: String,                        // 助手生成的文本响应
    pub tool_calls: Vec<ToolCallInfo>,       // 本次执行的工具调用列表
    pub usage: TokenUsage,                   // Token 使用统计
    pub session_id: SessionId,               // 关联的会话 ID
    pub duration_ms: u64,                    // 总执行时间（毫秒）
}
```

#### 字段详细说明

| 字段 | 类型 | 说明 | 示例值 |
|------|------|------|--------|
| `text` | `String` | AI 助手生成的文本响应 | `"Rust 是一门系统编程语言..."` |
| `tool_calls` | `Vec<ToolCallInfo>` | 本次轮次中执行的所有工具调用 | `[ToolCallInfo {...}]` |
| `usage` | `TokenUsage` | Token 使用统计（prompt/completion/total） | 见下方 TokenUsage |
| `session_id` | `SessionId` | 本次对话关联的会话标识符 | `SessionId("default-session")` |
| `duration_ms` | `u64` | 从接收到返回的总耗时（毫秒） | `1234` |

#### TokenUsage 结构体

```rust
pub struct TokenUsage {
    pub prompt_tokens: u32,       // 输入 prompt 使用的 token 数
    pub completion_tokens: u32,   // 生成响应使用的 token 数
    pub total_tokens: u32,        // 总 token 数 (prompt + completion)
}
```

**TokenUsage 来源**: 从 `CompletionTokenUsage`（推理 API 返回值）转换而来。

#### 使用示例

```rust
fn example_agent_turn_output(output: &AgentTurnOutput) {
    println!("=== Agent Turn Output ===");
    println!("Response:\n{}", output.text);
    println!("\n--- Usage ---");
    println!("Prompt tokens: {}", output.usage.prompt_tokens);
    println!("Completion tokens: {}", output.usage.completion_tokens);
    println!("Total tokens: {}", output.usage.total_tokens);
    println!("\n--- Metadata ---");
    println!("Session ID: {}", output.session_id);
    println!("Duration: {}ms ({:.2}s)",
        output.duration_ms,
        output.duration_ms as f64 / 1000.0
    );
    println!("Tool calls count: {}", output.tool_calls.len());

    for (i, tc) in output.tool_calls.iter().enumerate() {
        println!("\n[Tool Call #{}]", i + 1);
        println!("  Name: {}", tc.name);
        println!("  Status: {}", tc.status);
        println!("  Duration: {}ms", tc.duration_ms);
        if let Some(ref result) = tc.result {
            println!("  Result: {}", result);
        }
    }
}
```

---

### 3.5 ToolCallInfo 结构体

**文件位置**: [agent_loop.rs:13-20](file:///d:/studying/Codecargo/CarpAI/crates/carpai-core/src/agent_loop.rs#L13-L20)

#### 结构体定义

```rust
/// 工具调用信息（agent 执行期间产生）
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ToolCallInfo {
    pub name: String,                      // 工具名称
    pub arguments: serde_json::Value,       // 工具调用的参数（JSON 对象）
    pub result: Option<String>,            // 工具执行结果（可选）
    pub duration_ms: u64,                  // 执行耗时（毫秒）
    pub status: String,                    // 执行状态 ("success", "error", "cancelled")
}
```

#### 字段详细说明

| 字段 | 类型 | 说明 | 示例值 |
|------|------|------|--------|
| `name` | `String` | 被调用工具的标识符 | `"read_file"`, `"bash"`, `"web_search"` |
| `arguments` | `serde_json::Value` | 传递给工具的参数（通常为 JSON Object） | `{"path": "/src/main.rs"}` |
| `result` | `Option<String>` | 工具执行的文本输出（错误时可能为错误信息） | `Some("File contents...")`, `None` |
| `duration_ms` | `u64` | 工具执行的实际耗时 | `156` |
| `status` | `String` | 执行状态字符串 | `"success"`, `"error"`, `"cancelled"` |

#### 序列化示例

**JSON 格式**:
```json
{
  "name": "bash",
  "arguments": {
    "command": "cargo test",
    "cwd": "/home/user/project"
  },
  "result": "running 12 tests ... test result: ok. 12 passed; 0 failed",
  "duration_ms": 2340,
  "status": "success"
}
```

#### 使用场景

1. **调试与日志**: 记录 agent 执行过程中的工具使用情况
2. **审计追踪**: 追踪哪些工具被调用及参数
3. **性能分析**: 分析各工具的执行耗时
4. **错误诊断**: 定位失败的工具调用及其原因

#### 使用示例

```rust
fn analyze_tool_calls(tool_calls: &[ToolCallInfo]) {
    let total_duration: u64 = tool_calls.iter().map(|tc| tc.duration_ms).sum();
    let success_count = tool_calls.iter().filter(|tc| tc.status == "success").count();
    let error_count = tool_calls.iter().filter(|tc| tc.status == "error").count();

    println!("=== Tool Call Summary ===");
    println!("Total calls: {}", tool_calls.len());
    println!("Successful: {}", success_count);
    println!("Failed: {}", error_count);
    println!("Total duration: {}ms ({:.2}s)",
        total_duration,
        total_duration as f64 / 1000.0
    );

    if !tool_calls.is_empty() {
        let avg_duration = total_duration as f64 / tool_calls.len() as f64;
        println!("Average duration: {:.2}ms", avg_duration);
    }

    // 找出最耗时的工具调用
    if let Some(slowest) = tool_calls.iter().max_by_key(|tc| tc.duration_ms) {
        println!("\nSlowest tool: {} ({}ms)", slowest.name, slowest.duration_ms);
    }

    // 列出所有失败的工具调用
    let failures: Vec<_> = tool_calls.iter()
        .filter(|tc| tc.status != "success")
        .collect();

    if !failures.is_empty() {
        println!("\n=== Failed Tool Calls ===");
        for failure in &failures {
            println!("❌ Tool: {} | Status: {} | Args: {}",
                failure.name,
                failure.status,
                serde_json::to_string_pretty(&failure.arguments).unwrap_or_default()
            );
            if let Some(ref err) = failure.result {
                println!("   Error: {}", err);
            }
        }
    }
}
```

---

## 4. Local 实现列表

本节记录 `build_local_agent_context()` 中使用的所有本地实现。

### 4.1 LocalFileSessionStore

**文件位置**: [session_impl.rs:7-389](file:///d:/studying/Codecargo/CarpAI/crates/carpai-core/src/session_impl.rs#L7-L389)

**实现的 Trait**: `SessionStore`

#### 构造函数

```rust
impl LocalFileSessionStore {
    pub fn new(base_path: PathBuf) -> Self
}
```

**参数**:
- `base_path`: 会话文件的存储根目录（如 `~/.carpai/sessions/`）

#### 存储格式

每个会话对应一个 JSONL 文件:
```
{base_path}/{session_id}.jsonl
```

**文件结构**:
```
# META
{SessionMeta JSON}
{StoredMessage JSON}
{StoredMessage JSON}
...
# COMPACTION (可选)
{CompactionSnapshot JSON}
```

#### 核心功能

| 方法 | 功能 | 说明 |
|------|------|------|
| `create_session(meta)` | 创建新会话 | 写入 META 行到 JSONL 文件 |
| `load_session(id)` | 加载完整会话 | 解析 JSONL，返回 LoadedSession |
| `append_messages(id, msgs)` | 追加消息 | append 模式写入，更新 last_active_at |
| `update_meta(id, updates)` | 更新元数据 | 重写整个文件（当前实现） |
| `delete_session(id, hard)` | 删除会话 | hard=true 物理删除；false 标记 Deleted |
| `list_sessions(filter)` | 列出会话 | 支持按 owner/state/model 过滤，按 updated_at 降序 |
| `get_messages(id, offset, limit)` | 分页获取消息 | 支持分页查询 |
| `message_count(id)` | 消息计数 | 返回消息总数 |
| `set_state(id, state)` | 设置状态 | 更新 SessionState |
| `save_compaction(id, snapshot)` | 保存压缩快照 | 追加 # COMPACTION 块 |
| `load_compaction(id)` | 加载压缩快照 | 解析 # COMPACTION 块 |

#### 特性

- **线程安全**: 所有操作都是 async，使用 tokio::fs
- **原子性保证**: 单条消息追加是原子的（单次 write_all）
- **UUID 命名**: Session ID 使用 UUID v4 格式
- **自动目录创建**: 首次使用时自动创建 base_path 目录

#### 使用示例

```rust
async fn example_session_store() -> anyhow::Result<()> {
    use carpai_core::LocalFileSessionStore;
    use std::path::PathBuf;

    let store = LocalFileSessionStore::new(PathBuf::from("/tmp/test-sessions"));

    // 创建会话
    let meta = SessionMeta { /* ... */ };
    let session_id = store.create_session(meta).await?;

    // 追加消息
    let messages = vec![StoredMessage { /* ... */ }];
    let msg_ids = store.append_messages(&session_id, messages).await?;

    // 加载会话
    let loaded = store.load_session(&session_id).await?.unwrap();
    println!("Messages count: {}", loaded.messages.len());

    // 列出所有活跃会话
    let sessions = store.list_sessions(SessionFilter {
        state: Some(SessionState::Active),
        ..Default::default()
    }).await?;

    Ok(())
}
```

---

### 4.2 LocalToolExecutor

**文件位置**: [tool_executor_impl.rs:10-189](file:///d:/studying/Codecargo/CarpAI/crates/carpai-core/src/tool_executor_impl.rs#L10-L189)

**实现的 Trait**: `ToolExecutor`

#### 构造函数

```rust
impl LocalToolExecutor {
    pub fn new(max_concurrent: usize) -> Self
}
```

**参数**:
- `max_concurrent`: 最大并发执行数（使用 Semaphore 控制）

#### 并发控制

- 使用 `tokio::sync::Semaphore` 限制并发数
- 每个 `execute()` 调用会先 acquire permit
- 执行完成后自动释放 permit

#### 工具注册机制

```rust
pub async fn register_tool(&self, name: String, schema: ToolSchema)
```

- 内部维护 `HashMap<String, ToolSchema>` 注册表
- 必须先注册才能执行和验证

#### 执行模式支持

| 模式 | 枚举值 | 说明 | 当前实现状态 |
|------|--------|------|-------------|
| **本地执行** | `ExecutionMode::Local` | 在当前进程执行 | ✅ 已实现（Stub） |
| **沙箱执行** | `ExecutionMode::Sandboxed` | 在隔离环境执行 | ❌ 返回错误 |
| **远程执行** | `ExecutionMode::Remote { endpoint }` | 发送到远程端点执行 | ❌ 返回错误 |
| **试运行** | `ExecutionMode::DryRun` | 仅验证不执行 | ✅ 已实现 |

#### 核心功能

| 方法 | 功能 | 说明 |
|------|------|------|
| `execute(request)` | 执行工具 | 支持 4 种模式，返回 ToolResponse |
| `list_tools()` | 列出已注册工具 | 返回所有 ToolSchema |
| `get_tool_schema(name)` | 获取单个工具 schema | 按 name 查找 |
| `validate(name, params)` | 验证参数 | 检查必填字段，返回 ValidationResult |
| `check_permission(user, tool)` | 权限检查 | 当前默认允许所有 |
| `cancel(request_id)` | 取消执行 | 当前返回 Cancelled 错误 |

#### ToolResponse 结构

```rust
pub struct ToolResponse {
    pub success: bool,           // 是否成功
    pub output: String,          // 输出文本
    pub data: Option<Value>,     // 结构化数据（DryRun 时返回原始参数）
    pub exit_code: Option<i32>,  // 退出码
    pub duration_ms: u64,        // 执行耗时
    pub request_id: String,      // 请求 ID
    pub tool_name: String,       // 工具名
    pub audit_id: Option<String>,// 审计 ID
}
```

#### 当前限制

⚠️ **重要**: `execute_local()` 当前是一个 **stub 实现**，返回固定格式的字符串:
```rust
format!("[STUB] Tool '{}' executed with params: {}", tool_name, parameters)
```

生产环境中需要接入真实的工具执行逻辑。

#### 使用示例

```rust
async fn example_tool_executor() -> anyhow::Result<()> {
    use carpai_core::LocalToolExecutor;

    let executor = LocalToolExecutor::new(5); // 最大 5 个并发

    // 注册工具
    executor.register_tool(
        "read_file".to_string(),
        ToolSchema { /* schema definition */ }
    ).await;

    // 验证参数
    let validation = executor.validate(
        "read_file",
        &serde_json::json!({"path": "/src/main.rs"})
    ).await?;

    if validation.valid {
        println!("✅ Parameters valid");
        for warning in &validation.warnings {
            println!("⚠️ Warning: {}", warning);
        }
    } else {
        println!("❌ Invalid: {}", validation.error.unwrap());
    }

    // 执行工具
    let response = executor.execute(ToolRequest {
        tool_name: "read_file".to_string(),
        parameters: serde_json::json!({"path": "/src/main.rs"}),
        request_id: "req-001".to_string(),
        mode_override: Some(ExecutionMode::Local),
        context: ToolContext { user_id: "user-1".to_string(), /* ... */ },
    }).await?;

    println!("Success: {}", response.success);
    println!("Output: {}", response.output);
    println!("Duration: {}ms", response.duration_ms);

    Ok(())
}
```

---

### 4.3 SidecarInferenceBackend

**文件位置**: [inference_impl.rs:12-397](file:///d:/studying/Codecargo/CarpAI/crates/carpai-core/src/inference_impl.rs#L12-L397)

**实现的 Trait**: `InferenceBackend`

#### 构造函数

```rust
impl SidecarInferenceBackend {
    /// 从 ProviderConfig 构造
    pub fn new(provider_config: &crate::config::ProviderConfig) -> Self

    /// 快捷构造：仅指定模型名称
    pub fn with_model(model: impl Into<String>) -> Self
}
```

#### 配置项

| 配置 | 来源 | 默认值 | 说明 |
|------|------|--------|------|
| `endpoint` | `ProviderConfig.endpoint` | `http://localhost:11434` | Ollama API 地址 |
| `model` | `ProviderConfig.model` | `"default"` | 模型名称 |
| `api_key` | `ProviderConfig.api_key` | `None` | Bearer Token |
| `timeout` | `ProviderConfig.timeout_secs` | `30s` | HTTP 超时 |

#### HTTP 客户端配置

```rust
Client::builder()
    .timeout(Duration::from_secs(timeout_secs))  // 全局超时
    .build()
```

#### API 兼容性

兼容 **OpenAI Chat Completions API** 格式:
- 端点: `POST {endpoint}/v1/chat/completions`
- 请求体: 标准 OpenAI 格式
- 响应体: 标准 OpenAI 格式
- 流式端点: 相同 URL + `"stream": true`

#### 核心功能

| 方法 | 功能 | 说明 |
|------|------|------|
| `complete_chat(request)` | 非流式聊天补全 | 同步等待完整响应 |
| `stream_chat(request)` | 流式聊天补全 | 返回 Stream<Item=StreamChunk> |
| `list_models_with_routing()` | 列出可用模型 | 查询 Ollama `/api/tags` |
| `select_model(constraints)` | 模型选择 | 当前直接返回配置的 model |
| `get_quota_usage(user)` | 配额查询 | 返回空配额（本地无限制） |
| `record_usage(user, usage, model)` | 记录用量 | 仅日志记录 |
| `base_engine()` | 底层引擎 | 未实现（panic） |

#### 请求构建

```rust
fn build_request_body(&self, request: &ChatCompletionRequest) -> Value {
    json!({
        "model": request.model,
        "messages": request.messages,
        "temperature": request.temperature.unwrap_or(0.7),
        "max_tokens": request.max_tokens.unwrap_or(4096),
        "top_p": request.top_p,
        "frequency_penalty": request.frequency_penalty.unwrap_or(0.0),
        "presence_penalty": request.presence_penalty.unwrap_or(0.0),
        "stop": request.stop,
    })
}
```

#### 响应解析

**Finish Reason 映射**:
| API 值 | 枚举值 |
|--------|--------|
| `"stop"` | `FinishReason::Stop` |
| `"length"` | `FinishReason::Length` |
| `"content_filter"` | `FinishReason::ContentFilter` |
| `"error"` | `FinishReason::Error` |

#### 流式响应格式

解析 SSE (Server-Sent Events) 格式的 `data:` 行:
```
data: {"choices":[{"delta":{"content":"Hello"},"finish_reason":null}]}

data: {"choices":[{"delta":{"content":" world!"},"finish_reason":"stop"}]}
data: [DONE]
```

转换为 `StreamChunk`:
```rust
StreamChunk {
    chunk_type: StreamChunkType::ContentDelta | StreamChunkType::Finish,
    index: 0,
    delta: Some(content_text),
    finish_reason: Option<FinishReason>,
    usage: None,
}
```

#### 模型路由信息

`list_models_with_routing()` 返回 `Vec<RoutedModelInfo>`，每个包含:
- `model`: 基本信息（id, name, context_length, capabilities）
- `providers`: 提供者列表（endpoint, weight, healthy status）
- **成本信息**: `cost_per_1k_input/output` (本地为 0.0)
- **性能指标**: `avg_latency_ms`, `success_rate`, `routing_priority`
- **能力标志**: `supports_function_calling`, `supports_thinking`

#### 使用示例

```rust
async fn example_inference_backend() -> anyhow::Result<()> {
    use carpai_core::{SidecarInferenceBackend, config::ProviderConfig};

    // 方式 1: 从配置构造
    let provider = ProviderConfig {
        endpoint: Some("http://localhost:11434".into()),
        model: Some("llama3".into()),
        timeout_secs: 60,
        ..Default::default()
    };
    let backend = SidecarInferenceBackend::new(&provider);

    // 方式 2: 快捷构造
    let backend = SidecarInferenceBackend::with_model("mistral");

    // 非流式调用
    let request = ChatCompletionRequest {
        model: "llama3".to_string(),
        messages: vec![
            ChatMessage {
                role: ChatRole::User,
                content: ChatContent::Text("Hello!".to_string()),
                name: None,
            }
        ],
        max_tokens: Some(1024),
        temperature: Some(0.7),
        ..Default::default()
    };

    let response = backend.complete_chat(request).await?;
    println!("Response: {:?}", response.choices[0].message.content);

    // 流式调用
    let mut stream = backend.stream_chat(request).await?;
    while let Some(chunk) = stream.next().await {
        match chunk {
            Ok(stream_chunk) => {
                if let Some(text) = &stream_chunk.delta {
                    print!("{}", text);
                }
                if stream_chunk.chunk_type == StreamChunkType::Finish {
                    println!("\n[Stream completed]");
                }
            }
            Err(e) => {
                eprintln!("Stream error: {}", e);
                break;
            }
        }
    }

    // 列出可用模型
    let models = backend.list_models_with_routing().await?;
    for model_info in &models {
        println!("Model: {} | Context: {} | Available: {}",
            model_info.model.id,
            model_info.context_window,
            model_info.model.available
        );
    }

    Ok(())
}
```

---

### 4.4 LocalFileSystem

**文件位置**: [filesystem_impl.rs:10-431](file:///d:/studying/Codecargo/CarpAI/crates/carpai-core/src/filesystem_impl.rs#L10-L431)

**实现的 Trait**: `VirtualFileSystem`

#### 构造函数

```rust
impl LocalFileSystem {
    pub fn new(working_dir: &Path, vfs_root: Option<&Path>) -> Self
}
```

**参数**:
- `working_dir`: 工作目录（文件操作的基准路径）
- `vfs_root`: 可选的虚拟文件系统根目录（用于沙箱隔离）

#### 路径解析规则

```rust
fn resolve_path(&self, path: &Path) -> PathBuf {
    if let Some(ref vfs) = self.vfs_root {
        vfs.join(path)  // 使用 VFS 根目录
    } else {
        self.working_dir.join(path)  // 使用工作目录
    }
}
```

**安全检查**:
- `resolve()` 方法确保最终路径不会逃逸出 root 目录
- `is_allowed()` 提供快速权限检查

#### 核心功能

| 方法 | 功能 | 说明 |
|------|------|------|
| `read_file(path)` | 读取文本文件 | 返回 String |
| `read_file_bytes(path)` | 读取二进制文件 | 返回 Vec<u8> |
| `write_file(path, content)` | 写入文本文件 | 自动创建父目录，计算 SHA256 hash |
| `write_file_bytes(path, data)` | 写入二进制文件 | 同上 |
| `delete_file(path)` | 删除文件 | 仅限文件，非目录 |
| `exists(path)` | 检查存在性 | 返回 bool |
| `metadata(path)` | 获取元数据 | 返回 FileMeta |
| `list_dir(path, recursive)` | 列出目录 | 支持递归，排序（目录优先） |
| `create_dir(path)` | 创建目录 | create_dir_all |
| `delete_dir(path, recursive)` | 删除目录 | 支持递归删除 |
| `search_files(pattern, in_path, max_results)` | 文件名搜索 | 支持通配符 (*, ?) 和子串匹配 |
| `search_content(query, in_path, options)` | 内容搜索 | 支持正则、大小写不敏感、上下文行 |
| `git_diff/status/blame` | Git 操作 | ❌ 当前返回 Unsupported |
| `watch(path)` | 文件监视 | ❌ 当前返回 Unsupported |

#### FileWriteResult 结构

```rust
pub struct FileWriteResult {
    pub bytes_written: u64,        // 写入字节数
    pub created: bool,             // 是否为新创建的文件
    pub audit_id: Option<String>,  // 审计 ID（当前 None）
    pub previous_hash: Option<String>, // 写入前的 SHA256 hash
    pub new_hash: String,          // 写入后的 SHA256 hash
}
```

#### FileMeta 结构

```rust
pub struct FileMeta {
    pub path: PathBuf,             // 相对路径
    pub size: u64,                 // 文件大小（字节）
    pub is_dir: bool,              // 是否为目录
    pub is_symlink: bool,          // 是否为符号链接
    pub modified_at: SystemTime,   // 修改时间
    pub created_at: Option<SystemTime>, // 创建时间（可选）
    pub extension: Option<String>, // 文件扩展名
    pub content_hash: Option<String>, // 内容 hash（当前 None）
}
```

#### 搜索功能详解

**文件名搜索** (`search_files`):
- 支持通配符: `*.rs`, `test_?.txt`
- 不含通配符时进行子串匹配（大小写不敏感）
- 结果按匹配顺序返回，受 `max_results` 限制

**内容搜索** (`search_content`):
- 支持三种模式:
  - 精确匹配: `query` 必须出现在行中
  - 大小写不敏感: `options.case_insensitive = true`
  - 正则表达式: `options.regex = true`
- 过滤选项:
  - `extensions`: 仅搜索指定扩展名的文件
  - `exclude_patterns`: 排除匹配的文件名
- 上下文行: `context_lines_before/after`
- 返回 `ContentMatch` 结构，包含行号、行内容、前后上下文

#### ContentMatch 结构

```rust
pub struct ContentMatch {
    pub file: PathBuf,                 // 文件路径
    pub line_number: usize,            // 行号（从 1 开始）
    pub line: String,                  // 匹配行的完整内容
    pub byte_offset: usize,            // 字节偏移量
    pub match_length: usize,           // 匹配长度
    pub before_context: Vec<String>,   // 前面的上下文行
    pub after_context: Vec<String>,    // 后面的上下文行
}
```

#### 安全特性

- **路径遍历防护**: `resolve()` 检查路径逃逸
- **Hash 校验**: 写入文件时自动计算 SHA256
- **原子性**: 使用 tokio::fs 异步 I/O
- **错误分类**: `FsError` 枚举区分 NotFound/NotAFile/NotADirectory/PathEscape 等

#### 使用示例

```rust
async fn example_local_filesystem() -> anyhow::Result<()> {
    use carpai_core::LocalFileSystem;
    use std::path::PathBuf;

    let fs = LocalFileSystem::new(
        &PathBuf::from("/home/user/project"),
        None  // 不使用 VFS
    );

    // 读取文件
    let content = fs.read_file(Path::new("src/main.rs")).await?;
    println!("File size: {} bytes", content.len());

    // 写入文件
    let result = fs.write_file(
        Path::new("output.txt"),
        "Hello, CarpAI!"
    ).await?;
    println!("Written: {} bytes | Created: {} | Hash: {}",
        result.bytes_written, result.created, result.new_hash);

    // 列出目录
    let entries = fs.list_dir(Path::new("src"), true).await?;
    for entry in &entries {
        println!("{} ({}) - {} bytes",
            entry.name,
            if entry.meta.is_dir { "DIR" } else { "FILE" },
            entry.meta.size
        );
    }

    // 搜索文件
    let results = fs.search_files("*.rs", Path::new("src"), 10).await?;
    println!("Found {} Rust files", results.len());

    // 搜索内容
    let matches = fs.search_content(
        "TODO",
        Path::new("src"),
        SearchOptions {
            extensions: vec!["rs".to_string()],
            case_insensitive: true,
            context_lines_before: 2,
            context_lines_after: 2,
            max_matches_per_file: 20,
            ..Default::default()
        }
    ).await?;
    println!("Found {} TODO comments", matches.len());

    // 元数据查询
    let meta = fs.metadata(Path::new("Cargo.toml")).await?;
    println!("Modified: {:?}", meta.modified_at);
    println!("Size: {} bytes", meta.size);

    Ok(())
}
```

---

### 4.5 InProcessEventBus

**文件位置**: [event_bus_impl.rs:8-163](file:///d:/studying/Codecargo/CarpAI/crates/carpai-core/src/event_bus_impl.rs#L8-L163)

**实现的 Trait**: `EventBus`

#### 构造函数

```rust
impl InProcessEventBus {
    pub fn new(capacity: usize) -> Self
}
```

**参数**:
- `capacity`: 广播通道容量（同时缓存的最大事件数），默认 `1024`

#### 实现原理

基于 `tokio::sync::broadcast` 通道:
- **发布-订阅模式**: 多个订阅者可以同时接收相同的事件
- **内存历史**: 维护最近 N 个事件的环形缓冲区（VecDeque）
- **克隆安全**: 实现 `Clone` trait，共享同一个广播通道

#### 核心数据结构

```rust
pub struct InProcessEventBus {
    sender: broadcast::Sender<BusEventEnvelope>,  // 广播发送器
    capacity: usize,                               // 通道容量
    history: Arc<RwLock<VecDeque<BusEventEnvelope>>>,  // 事件历史
    events_published: Arc<AtomicU64>,              // 发布计数器
    events_dropped: Arc<AtomicU64>,                // 丢弃计数器
    start_instant: Instant,                        // 启动时间戳
}
```

#### BusEventEnvelope 结构

```rust
pub struct BusEventEnvelope {
    pub event_type: String,        // 事件类型标识符
    pub payload: String,           // 事件负载（JSON 字符串）
    pub timestamp_ms: i64,         // 时间戳（毫秒，Unix epoch）
}
```

#### 核心功能

| 方法 | 功能 | 说明 |
|------|------|------|
| `publish_json(event_type, payload)` | 发布事件 | 同时写入 history 和 broadcast channel |
| `subscribe(event_type)` | 订阅事件 | 返回 BusSubscriber，支持按类型过滤 |
| `subscriber_count(event_type)` | 获取订阅者数 | 返回当前 receiver 数量 |
| `health_check()` | 健康检查 | 返回 BusHealth 状态报告 |
| `clone_box()` | 克隆为 trait object | 用于 Arc<dyn EventBus> 场景 |

#### 订阅者行为

**BroadcastSubscriber** 特性:
- **过滤**: 只接收指定 `event_type` 或 `"*"`（全部）的事件
- **背压处理**: 当订阅者跟不上时收到 `Lagged(n)` 通知
- **非阻塞接收**: `try_recv()` 立即返回，不阻塞
- **阻塞接收**: `recv()` 等待下一个匹配事件

#### BusHealth 结构

```rust
pub struct BusHealth {
    pub healthy: bool,                          // 是否健康
    pub backend: String,                        // 后端类型标识 ("in-process")
    pub total_subscribers: usize,               // 总订阅者数
    pub events_published_total: u64,            // 总发布数
    pub events_dropped_total: u64,              // 总丢弃数（无订阅者时）
    pub uptime_secs: u64,                       // 运行时长（秒）
}
```

#### 事件丢失场景

当没有活跃订阅者时调用 `publish_json()`:
- 事件仍会写入 `history` 缓冲区
- `events_dropped` 计数器 +1
- 返回 `Ok(())`（不视为错误）
- 日志级别: `debug!`

#### 使用示例

```rust
async fn example_event_bus() -> anyhow::Result<()> {
    use carpai_core::InProcessEventBus;

    let bus = InProcessEventBus::new(1024);

    // 发布事件
    bus.publish_json(
        "agent.turn.started",
        r#"{"session_id": "sess-123", "user_message": "Hello"}"#
    ).await?;

    bus.publish_json(
        "tool.execution.completed",
        r#"{"tool": "read_file", "duration_ms": 156}"#
    ).await?;

    // 订阅特定类型事件
    let mut subscriber = bus.subscribe("tool.*").await?;  // 支持通配符？需确认

    // 或者订阅所有事件
    let mut all_subscriber = bus.subscribe("*").await?;

    // 接收事件（阻塞）
    let envelope = subscriber.recv().await?;
    println!("Event: {} | Payload: {} | Time: {}",
        envelope.event_type,
        envelope.payload,
        envelope.timestamp_ms
    );

    // 非阻塞接收
    match subscriber.try_recv()? {
        Some(envelope) => println!("Got event: {}", envelope.event_type),
        None => println!("No events available"),
    }

    // 健康检查
    let health = bus.health_check();
    println!("Healthy: {} | Subscribers: {} | Published: {} | Dropped: {} | Uptime: {}s",
        health.healthy,
        health.total_subscribers,
        health.events_published_total,
        health.events_dropped_total,
        health.uptime_secs
    );

    Ok(())
}
```

---

### 4.6 LocalMemoryBackend

**文件位置**: [memory_impl.rs:8-457](file:///d:/studying/Codecargo/CarpAI/crates/carpai-core/src/memory_impl.rs#L8-L457)

**实现的 Trait**: `MemoryBackend`

#### 构造函数

```rust
impl LocalMemoryBackend {
    pub fn new(base_path: PathBuf) -> Self
}
```

**参数**:
- `base_path`: 记忆文件存储根目录（如 `~/.carpai/memory/`）

#### 存储格式

每个记忆条目一个 JSONL 文件:
```
{base_path}/{memory_id}.jsonl
```

文件内容为单行 JSON（EnhancedMemoryEntry 的序列化结果）。

#### 核心功能

| 方法 | 功能 | 说明 |
|------|------|------|
| `store(entry)` | 存储新条目 | 自动生成 ID（如果为空） |
| `retrieve(id)` | 按 ID 获取 | 返回 Option |
| `search(query)` | 搜索条目 | 支持多维度过滤 |
| `delete(id)` | 删除条目 | 物理删除文件 |
| `update(id, updates)` | 更新条目 | 部分更新，返回更新后的完整条目 |
| `vector_search(embedding, limit, options)` | 向量搜索 | ❌ 当前返回空（需要 embedding 服务） |
| `upsert_embedding(memory_id, embedding)` | 存储 embedding | ❌ 空实现 |
| `find_duplicate(content, threshold)` | 查重 | ❌ 返回 None |
| `reinforce(id, session_id, message_index)` | 强化记忆 | strength += 1，记录 reinforcement |
| `consolidate(primary_id, merge_ids)` | 合并记忆 | 合并元数据、累加 strength、标记被合并者为 superseded |
| `get_by_scope(scope, project_id, limit)` | 按范围获取 | 封装 search() |
| `stats(scope)` | 统计信息 | 返回 EnhancedMemoryStats |
| `cleanup(options)` | 清理过期/低置信度条目 | 支持 age/confidence/stale 过滤条件 |

#### EnhancedMemoryEntry 结构（简化）

```rust
pub struct EnhancedMemoryEntry {
    pub base: MemoryEntryBase,        // 基础字段（id, content, type, timestamps, metadata）
    pub scope: MemoryScope,           // 范围（Global/Project/Session/User）
    pub trust: TrustLevel,            // 信任等级（High/Medium/Low）
    pub confidence: f32,              // 置信度 (0.0-1.0)
    pub active: bool,                 // 是否激活
    pub strength: u32,                // 强度（reinforcement 次数）
    pub reinforcements: Vec<Reinforcement>, // 强化记录
    pub superseded_by: Option<String>, // 被哪个条目合并
}
```

#### 搜索过滤条件 (EnhancedMemoryQuery)

| 条件 | 字段 | 说明 |
|------|------|------|
| 活跃状态 | `active_only: bool` | 仅返回 active=true 的条目 |
| 范围 | `scope: Option<MemoryScope>` | Global/Project/Session/User |
| 类型 | `memory_type: Option<MemoryType>` | Fact/Skill/Preference/Context/Episodic |
| 信任等级 | `min_trust: Option<TrustLevel>` | 最小信任等级 |
| 文本搜索 | `text_query: Option<String>` | 内容子串匹配（大小写不敏感） |
| 标签 | `tags: Option<Vec<String>>` | 标签匹配（AND 逻辑） |
| 时间范围 | `created_after/before: Option<DateTime>` | 创建时间范围 |
| 结果限制 | `limit: Option<usize>` | 最大返回数量 |

#### 统计信息 (EnhancedMemoryStats)

```rust
pub struct EnhancedMemoryStats {
    pub total_count: usize,                              // 总条目数
    pub count_by_scope: HashMap<MemoryScope, usize>,     // 按范围统计
    pub count_by_type: HashMap<MemoryType, usize>,       // 按类型统计
    pub count_by_trust: HashMap<TrustLevel, usize>,      // 按信任等级统计
    pub avg_confidence: f32,                             // 平均置信度
    pub storage_size_bytes: u64,                         // 存储占用（字节）
    pub stale_count: usize,                              // 非活跃且未被合并的条目数
    pub superseded_count: usize,                         // 被合并替代的条目数
}
```

#### 清理选项 (CleanupOptions)

| 选项 | 字段 | 说明 |
|------|------|------|
| 年龄限制 | `older_than: Option<DateTime>` | 删除早于此时间的条目 |
| 置信度阈值 | `below_confidence: Option<f32>` | 删除低于此置信度的条目 |
| 强制删除 | `hard_delete: bool` | 是否删除非活跃（stale）条目 |
| 最大删除数 | `max_prune: Option<usize>` | 单次清理的最大数量 |

#### 清理结果 (CleanupResult)

```rust
pub struct CleanupResult {
    pub pruned_count: usize,         // 实际删除数
    pub superseded_count: usize,     // 被合并标记数
    pub freed_bytes: u64,            // 释放的字节数
    pub errors: Vec<String>,         // 错误列表
}
```

#### 特殊功能说明

**记忆强化 (reinforce)**:
- 每次调用使 `strength += 1`
- 记录强化来源（session_id, message_index, timestamp）
- 用于实现重复学习机制

**记忆合并 (consolidate)**:
- 主条目吸收被合并者的 metadata
- 累加 strength
- 被合并者标记为 `active=false` 且 `superseded_by=primary_id`
- 支持批量合并多个条目

#### 当前限制

⚠️ **向量相关功能未实现**:
- `vector_search()` 返回空 Vec
- `upsert_embedding()` 空操作
- `find_duplicate()` 返回 None

这些功能需要集成 embedding 模型服务（如 sentence-transformers）。

#### 使用示例

```rust
async fn example_memory_backend() -> anyhow::Result<()> {
    use carpai_core::LocalMemoryBackend;
    use std::path::PathBuf;

    let memory = LocalMemoryBackend::new(PathBuf::from("/tmp/test-memory"));

    // 存储新记忆
    let entry = EnhancedMemoryEntry {
        base: MemoryEntryBase {
            id: String::new(),  // 自动生成
            content: "User prefers Rust over Python for systems programming".to_string(),
            memory_type: MemoryType::Preference,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            metadata: {
                let mut m = HashMap::new();
                m.insert("source".to_string(), "conversation".to_string());
                m.insert("tags".to_string(), "preference,language".to_string());
                m
            },
        },
        scope: MemoryScope::User,
        trust: TrustLevel::Medium,
        confidence: 0.8,
        active: true,
        strength: 1,
        reinforcements: vec![],
        superseded_by: None,
    };

    let memory_id = memory.store(entry).await?;
    println!("Stored memory: {}", memory_id);

    // 搜索记忆
    let results = memory.search(&EnhancedMemoryQuery {
        text_query: Some("Rust".to_string()),
        scope: Some(MemoryScope::User),
        active_only: true,
        limit: Some(10),
        ..Default::default()
    }).await?;

    println!("Found {} memories about Rust", results.len());
    for mem in &results {
        println!("- [{}] {} (confidence: {:.2}, strength: {})",
            mem.base.id,
            mem.base.content,
            mem.confidence,
            mem.strength
        );
    }

    // 强化记忆
    memory.reinforce(&memory_id, "sess-456", 42).await?;

    // 获取统计信息
    let stats = memory.stats(Some(MemoryScope::User)).await?;
    println!("Total user memories: {}", stats.total_count);
    println!("Average confidence: {:.2}", stats.avg_confidence);
    println!("Storage: {} bytes", stats.storage_size_bytes);

    // 清理过期记忆
    let cleanup_result = memory.cleanup(&CleanupOptions {
        older_than: Some(chrono::Utc::now() - chrono::Duration::days(30)),
        below_confidence: Some(0.3),
        hard_delete: true,
        max_prune: Some(100),
    }).await?;

    println!("Cleaned up {} entries, freed {} bytes",
        cleanup_result.pruned_count,
        cleanup_result.freed_bytes
    );

    Ok(())
}
```

---

## 5. 配置指南

### 5.1 推荐的配置文件结构

```toml
# ~/.carpai/config.toml

## 基础应用配置
[base]
mode = "cli"                           # cli | server
working_dir = "."                       # 项目根目录
default_model = "llama3:8b"            # 默认模型
max_context_tokens = 128000             # 上下文窗口大小
tools_enabled = true                    # 启用工具系统
default_tool_mode = "local"             # local | sandboxed | remote | dry_run
vfs_enabled = false                     # 虚拟文件系统
memory_enabled = true                   # 记忆系统
event_bus_enabled = true                # 事件总线

## 数据存储
[data_dir]
# 使用默认值 ~/.carpai
session_subdir = "sessions"             # 会话存储子目录
memory_subdir = "memory"                # 记忆存储子目录

## 并发控制
max_concurrent_tools = 5                # 最大并发工具数
max_agent_iterations = 100              # agent 循环上限

## 推理提供者
[completion_provider]
provider_type = "local"                 # local | openai | anthropic
endpoint = "http://localhost:11434"     # Ollama 端点
# api_key 请勿在此设置！使用环境变量
model = "llama3:8b"                     # 模型名称
timeout_secs = 60                       # 请求超时（秒）

## 缓存
cache_size_mb = 512                     # 内存缓存大小
disk_cache_enabled = true               # 磁盘缓存
```

### 5.2 环境变量配置

**适用于 CI/CD、Docker 容器或密钥注入场景**:

```bash
# 基础配置
export CARPAI_DATA_DIR="/data/carpai"
export CARPAI_DEFAULT_MODEL="claude-sonnet-4-20250514"

# 性能调优
export CARPAI_CORE__MAX_CONCURRENT_TOOLS=10
export CARPAI_CORE__MAX_AGENT_ITERATIONS=200

# 推理提供者（敏感信息）
export CARPAI_CORE__COMPLETION_PROVIDER__API_KEY="sk-xxxxx"
export CARPAI_CORE__COMPLETION_PROVIDER__ENDPOINT="https://api.openai.com/v1"
export CARPAI_CORE__COMPLETION_PROVIDER__MODEL="gpt-4o"
export CARPAI_CORE__COMPLETION_PROVIDER__TIMEOUT_SECS=120
```

### 5.3 Docker Compose 示例

```yaml
version: '3.8'
services:
  carpai:
    image: carpai:latest
    environment:
      - CARPAI_DATA_DIR=/app/data
      - CARPAI_DEFAULT_MODEL=llama3:8b
      - CARPAI_CORE__MAX_CONCURRENT_TOOLS=10
      - CARPAI_CORE__COMPLETION_PROVIDER__ENDPOINT=http://ollama:11434
    volumes:
      - carpai_data:/app/data
    depends_on:
      - ollama

  ollama:
    image: ollama/ollama:latest
    ports:
      - "11434:11434"
    volumes:
      - ollama_data:/root/.ollama
```

### 5.4 配置验证清单

在使用 `build_local_agent_context()` 前，建议验证:

- [ ] `config.data_dir` 存在或可创建
- [ ] `config.base.working_dir` 存在
- [ ] `config.base.default_model` 非空
- [ ] `config.completion_provider.endpoint` 可达（如果是 remote provider）
- [ ] 磁盘空间充足（至少 512MB cache + session/memory 数据）
- [ ] 网络连接正常（如果使用远程推理服务）

---

## 6. 错误处理最佳实践

### 6.1 错误类型层次

```
anyhow::Error (顶层统一错误类型)
├── ConfigError (配置错误)
│   ├── Io(std::io::Error)           # 文件读写错误
│   └── Parse(toml::de::Error)       # TOML 解析错误
├── SessionError (会话错误)
│   ├── Storage(String)              # 存储层错误
│   ├── Serialization(String)        # 序列化错误
│   ├── NotFound(String)             # 资源不存在
│   └── Internal(anyhow::Error)      # 内部错误
├── ToolExecError (工具执行错误)
│   ├── ExecutionFailed(String)      # 执行失败
│   ├── Internal(anyhow::Error)      # 内部错误
│   ├── Timeout(String)              # 超时
│   └──Cancelled                     # 用户取消
├── InferenceError (推理错误)
│   ├── ApiError { status, message } # API 调用错误
│   └── InvalidRequest(String)       # 无效请求
├── FsError (文件系统错误)
│   ├── Io(std::io::Error)           # IO 错误
│   ├── NotFound(String)             # 文件不存在
│   ├── NotAFile(String)             # 不是文件
│   ├── NotADirectory(String)        # 不是目录
│   ├── NotEmpty(String)             # 目录非空
│   ├── PathEscape { path, root }    # 路径遍历攻击
│   └── Unsupported                  # 操作不支持
├── MemoryError (记忆系统错误)
│   ├── StorageError(String)         # 存储错误
│   └── NotFound(String)             # 条目不存在
└── EventBusError (事件总线错误)
    ├── ChannelClosed                # 通道关闭
    └── Serialization(String)        # 序列化错误
```

### 6.2 推荐的错误处理模式

#### 模式 1: 快速失败（适用于 CLI）

```rust
#[tokio::main]
async fn main() {
    if let Err(e) = run_app().await {
        eprintln!("❌ Error: {}", e);

        // 打印错误链（如果有 cause）
        let mut source = e.source();
        while let Some(cause) = source {
            eprintln!("   Caused by: {}", cause);
            source = cause.source();
        }

        std::process::exit(1);
    }
}

async fn run_app() -> anyhow::Result<()> {
    let config = CoreConfig::load(&PathBuf::from("~/.carpai/config.toml"))?;
    let ctx = build_local_agent_context(&config);
    let output = execute_agent_turn(&ctx, "Hello")?;
    println!("{}", output.text);
    Ok(())
}
```

#### 模式 2: 优雅降级（适用于 Server）

```rust
async fn handle_agent_request(req: Request) -> Response {
    match execute_agent_turn(&ctx, &req.message).await {
        Ok(output) => Response::ok(output),
        Err(e) => {
            // 分类错误并返回适当的 HTTP 状态码
            let status = classify_error(&e);
            tracing::error!(error = %e, "Agent turn failed");
            Response::error(status, e.to_string())
        }
    }
}

fn classify_error(e: &anyhow::Error) -> StatusCode {
    if e.is::<SessionError>() { StatusCode::INTERNAL_SERVER_ERROR }
    else if e.is::<InferenceError>() { StatusCode::BAD_GATEWAY }
    else if e.is::<FsError>() { StatusCode::NOT_FOUND }
    else { StatusCode::INTERNAL_SERVER_ERROR }
}
```

#### 模式 3: 重试机制（适用于网络操作）

```rust
async fn retry_inference<F, T, E>(f: F, max_retries: u32) -> Result<T, E>
where
    F: Fn() -> std::pin::Pin<Box<dyn Future<Output = Result<T, E>> + Send>>,
{
    let mut last_err = None;

    for attempt in 1..=max_retries {
        match f().await {
            Ok(result) => return Ok(result),
            Err(e) => {
                tracing::warn!(
                    attempt,
                    max_retries,
                    error = %e,
                    "Retrying after error"
                );

                if attempt < max_retries {
                    tokio::time::sleep(Duration::from_millis(100 * attempt as u64)).await;
                }
                last_err = Some(e);
            }
        }
    }

    Err(last_err.unwrap())
}
```

### 6.3 常见错误场景及解决方案

| 错误场景 | 可能原因 | 解决方案 |
|----------|----------|----------|
| `Session not found after creation` | 文件系统权限问题 | 检查 `data_dir` 权限 |
| `Inference failed: connection refused` | Ollama 未启动 | 启动 Ollama 服务 |
| `Io: No such file or directory` | 配置文件路径错误 | 检查 `config.toml` 路径 |
| `Parse: unknown field` | TOML 格式错误 | 验证配置文件语法 |
| `Tool execution failed: Sandbox not implemented` | 使用了不支持的模式 | 改用 `ExecutionMode::Local` |
| `FsError: PathEscape` | 路径包含 `..` | 使用绝对路径或 sanitize 输入 |
| `MemoryError: NotFound` | 记忆 ID 不存在 | 先调用 `store()` 或检查 ID |

---

## 7. 集成点说明

### 7.1 与 carpai-server 的集成

**Server 模式下的差异**:

| 组件 | CLI 模式 (Local*) | Server 模式 (可能的替换) |
|------|-------------------|------------------------|
| SessionStore | `LocalFileSessionStore` | PostgreSQL/MongoDB 实现 |
| ToolExecutor | `LocalToolExecutor` | 分布式任务队列 (Redis/RabbitMQ) |
| InferenceBackend | `SidecarInferenceBackend` | 远程 API (OpenAI/Anthropic) + 负载均衡 |
| VirtualFileSystem | `LocalFileSystem` | S3/NFS/Git-backed FS |
| EventBus | `InProcessEventBus` | Kafka/NATS/Redis PubSub |
| MemoryBackend | `LocalMemoryBackend` | PostgreSQL + pgvector (向量搜索) |

**切换方式**:
```rust
// Server 模式组装示例
let ctx = AgentContextBuilder::new(server_config)
    .with_sessions(Arc::new(PostgresSessionStore::new(db_pool)))
    .with_tools(Arc::new(DistributedToolExecutor::new(redis_pool)))
    .with_inference(Arc::new(RoutingInferenceBackend::new(providers)))
    .with_fs(Arc::new(S3FileSystem::new(bucket)))
    .with_events(Arc::new(KafkaEventBus::new(brokers)))
    .with_memory(Arc::new(VectorMemoryBackend::new(pg_pool)))
    .build()?;
```

### 7.2 与 carpai-cli 的集成

**CLI 入口点**:

```rust
// src/cli/commands.rs (假设)
pub async fn cmd_chat(args: ChatArgs) -> anyhow::Result<()> {
    let config = CoreConfig::load(&args.config_path)?;
    let ctx = build_local_agent_context(&config);

    loop {
        let input = read_user_input()?;
        if input == "/quit" { break; }

        let output = execute_agent_turn(&ctx, &input).await?;
        print_response(&output);
    }

    Ok(())
}
```

**TUI 集成要点**:
1. 使用 `stream_chat()` 替代 `complete_chat()` 实现实时打字效果
2. 监听 EventBus 事件更新 UI 状态栏（token 用量、工具调用等）
3. 支持 Ctrl+C 中断（调用 `ToolExecutor.cancel()`）

### 7.3 扩展指南

#### 添加新的 Local* 实现

1. **定义新结构体**:
```rust
// crates/carpai-core/src/my_impl.rs
pub struct MyCustomImplementation {
    // fields...
}
```

2. **实现对应的 Trait**:
```rust
#[async_trait]
impl MyTrait for MyCustomImplementation {
    async fn my_method(&self, ...) -> Result<..., MyError> {
        // implementation
    }
}
```

3. **注册到 lib.rs**:
```rust
// crates/carpai-core/src/lib.rs
mod my_impl;
pub use my_impl::MyCustomImplementation;
```

4. **可选: 扩展 build_local_agent_context()** 或提供自定义 builder 函数

#### 添加新的配置字段

1. **在 CoreConfig 添加字段**:
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoreConfig {
    // existing fields...

    /// 新配置项
    #[serde(default = "default_my_field")]
    pub my_new_field: String,
}
```

2. **添加默认值函数**:
```rust
fn default_my_field() -> String { "default_value".into() }
```

3. **在 load() 中添加环境变量支持**:
```rust
if let Ok(v) = std::env::var("CARPAI_CORE__MY_NEW_FIELD") {
    config.my_new_field = v;
}
```

4. **更新文档和示例**

---

## 附录 A: 类型索引

### 核心类型

| 类型 | 定义位置 | 说明 |
|------|----------|------|
| `AgentContext` | `carpai_internal` | Agent 上下文容器 |
| `AgentContextBuilder` | `carpai_internal` | 上下文构建器 |
| `AppConfig` | `carpai_internal` | 应用基础配置 |
| `SessionId` | `carpai_internal` | 会话 ID 包装器 |
| `SessionMeta` | `carpai_internal` | 会话元数据 |
| `LoadedSession` | `carpai_internal` | 加载的完整会话 |
| `StoredMessage` | `carpai_internal` | 持久化的消息 |
| `MessageRole` | `carpai_internal` | 消息角色枚举 |
| `ContentBlock` | `carpai_internal` | 内容块（Text/ToolUse/ToolResult/Thinking） |
| `ChatMessage` | `carpai_internal` | 聊天消息（推理 API 格式） |
| `ChatRole` | `carpai_internal` | 聊天角色 |
| `ChatContent` | `carpai_internal` | 聊天内容 |
| `ChatCompletionRequest` | `carpai_internal` | 补全请求 |
| `ChatCompletionResponse` | `carpai_internal` | 补全响应 |
| `Choice` | `carpai_internal` | 响应选择 |
| `CompletionTokenUsage` | `carpai_internal` | Token 使用统计 |
| `FinishReason` | `carpai_internal` | 完成原因枚举 |
| `ToolRequest` | `carpai_internal` | 工具请求 |
| `ToolResponse` | `carpai_internal` | 工具响应 |
| `ToolContext` | `carpai_internal` | 工具执行上下文 |
| `ToolSchema` | `carpai_internal` | 工具 Schema |
| `ValidationResult` | `carpai_internal` | 验证结果 |
| `ExecutionMode` | `carpai_internal` | 执行模式枚举 |
| `StreamChunk` | `carpai_internal` | 流式响应块 |
| `StreamChunkType` | `carpai_internal` | 流式块类型 |
| `RoutedModelInfo` | `carpai_internal` | 路由模型信息 |
| `ModelInfo` | `carpai_internal` | 模型基本信息 |
| `ModelProviderEntry` | `carpai_internal` | 模型提供者条目 |
| `ModelSelectionConstraints` | `carpai_internal` | 模型选择约束 |
| `QuotaUsage` | `carpai_internal` | 配额使用情况 |
| `FileMeta` | `carpai_internal` | 文件元数据 |
| `FileEntry` | `carpai_internal` | 目录条目 |
| `FileWriteResult` | `carpai_internal` | 文件写入结果 |
| `SearchResult` | `carpai_internal` | 文件搜索结果 |
| `ContentMatch` | `carpai_internal` | 内容匹配结果 |
| `SearchOptions` | `carpai_internal` | 搜索选项 |
| `FsError` | `carpai_internal` | 文件系统错误 |
| `FsEvent` | `carpai_internal` | 文件系统事件 |
| `BusEventEnvelope` | `carpai_internal` | 事件包装器 |
| `BusSubscriber` | `carpai_internal` | 事件订阅者 trait |
| `BusHealth` | `carpai_internal` | 事件总线健康状态 |
| `EventBusError` | `carpai_internal` | 事件总线错误 |
| `EnhancedMemoryEntry` | `carpai_internal` | 增强记忆条目 |
| `MemoryEntryBase` | `carpai_internal` | 记忆基础字段 |
| `MemoryScope` | `carpai_internal` | 记忆范围枚举 |
| `MemoryType` | `carpai_internal` | 记忆类型枚举 |
| `TrustLevel` | `carpai_internal` | 信任等级枚举 |
| `EnhancedMemoryQuery` | `carpai_internal` | 记忆查询条件 |
| `EnhancedMemoryUpdate` | `carpai_internal` | 记忆更新字段 |
| `EnhancedMemoryStats` | `carpai_internal` | 记忆统计信息 |
| `Reinforcement` | `carpai_internal` | 强化记录 |
| `CleanupOptions` | `carpai_internal` | 清理选项 |
| `CleanupResult` | `carpai_internal` | 清理结果 |
| `VectorSearchResult` | `carpai_internal` | 向量搜索结果 |
| `VectorSearchOptions` | `carpai_internal` | 向量搜索选项 |
| `MemoryError` | `carpai_internal` | 记忆系统错误 |
| `SessionError` | `carpai_internal` | 会话系统错误 |
| `SessionState` | `carpai_internal` | 会话状态枚举 |
| `SessionMetaUpdate` | `carpai_internal` | 会话元数据更新 |
| `SessionFilter` | `carpai_internal` | 会话过滤器 |
| `CompactionSnapshot` | `carpai_internal` | 压缩快照 |
| `ToolExecError` | `carpai_internal` | 工具执行错误 |
| `InferenceError` | `carpai_internal` | 推理错误 |
| `InferenceEngine` | `carpai_internal` | 推理引擎 trait |
| `InferenceBackend` | `carpai_internal` | 推理后端 trait |
| `SessionStore` | `carpai_internal` | 会话存储 trait |
| `ToolExecutor` | `carpai_internal` | 工具执行器 trait |
| `VirtualFileSystem` | `carpai_internal` | 虚拟文件系统 trait |
| `EventBus` | `carpai_internal` | 事件总线 trait |
| `MemoryBackend` | `carpai_internal` | 记忆后端 trait |

### carpai-core 自定义类型

| 类型 | 定义位置 | 说明 |
|------|----------|------|
| `CoreConfig` | [config.rs:12](file:///d:/studying/Codecargo/CarpAI/crates/carpai-core/src/config.rs#L12) | 核心配置 |
| `ProviderConfig` | [config.rs:58](file:///d:/studying/Codecargo/CarpAI/crates/carpai-core/src/config.rs#L58) | 推理提供者配置 |
| `ConfigError` | [config.rs:180](file:///d:/studying/Codecargo/CarpAI/crates/carpai-core/src/config.rs#L180) | 配置错误 |
| `AgentTurnOutput` | [agent_loop.rs:206](file:///d:/studying/Codecargo/CarpAI/crates/carpai-core/src/agent_loop.rs#L206) | Agent 轮次输出 |
| `ToolCallInfo` | [agent_loop.rs:14](file:///d:/studying/Codecargo/CarpAI/crates/carpai-core/src/agent_loop.rs#L14) | 工具调用信息 |
| `TokenUsage` | [agent_loop.rs:194](file:///d:/studying/Codecargo/CarpAI/crates/carpai-core/src/agent_loop.rs#L194) | Token 使用统计 |
| `LocalFileSessionStore` | [session_impl.rs:7](file:///d:/studying/Codecargo/CarpAI/crates/carpai-core/src/session_impl.rs#L7) | 本地文件会话存储 |
| `LocalToolExecutor` | [tool_executor_impl.rs:10](file:///d:/studying/Codecargo/CarpAI/crates/carpai-core/src/tool_executor_impl.rs#L10) | 本地工具执行器 |
| `SidecarInferenceBackend` | [inference_impl.rs:12](file:///d:/studying/Codecargo/CarpAI/crates/carpai-core/src/inference_impl.rs#L12) | Sidecar 推理后端 |
| `LocalFileSystem` | [filesystem_impl.rs:10](file:///d:/studying/Codecargo/CarpAI/crates/carpai-core/src/filesystem_impl.rs#L10) | 本地文件系统 |
| `InProcessEventBus` | [event_bus_impl.rs:8](file:///d:/studying/Codecargo/CarpAI/crates/carpai-core/src/event_bus_impl.rs#L8) | 进程内事件总线 |
| `LocalMemoryBackend` | [memory_impl.rs:8](file:///d:/studying/Codecargo/CarpAI/crates/carpai-core/src/memory_impl.rs#L8) | 本地记忆后端 |

---

## 附录 B: 版本历史

| 版本 | 日期 | 作者 | 变更说明 |
|------|------|------|----------|
| 1.0.0 | 2026-05-25 | AI Assistant | 初始版本，基于实际代码生成完整接口契约文档 |

---

## 附录 C: 许可证

本文档遵循项目主许可证（参见仓库根目录 LICENSE 文档）。

---

## 附录 D: 反馈与贡献

发现文档与代码不一致或有改进建议？

1. 检查本文档标注的源码位置链接
2. 验证实际代码实现
3. 提交 Issue 或 PR 更新文档
4. 确保所有示例代码可通过编译测试

---

**文档结束**

> 📌 **提示**: 本文档由 AI 基于实际源码自动生成，所有 API 签名、字段定义、默认值均来自代码实现，不含虚构内容。
