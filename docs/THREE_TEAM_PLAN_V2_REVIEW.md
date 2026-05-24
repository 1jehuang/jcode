# THREE_TEAM_REFACTOR_PLAN v2.0 正式评审报告

> **评审人**: Team Alpha (架构组)
> **评审日期**: 2026-05-24
> **评审对象**: `docs/THREE_TEAM_REFACTOR_PLAN.md` (v2.0)
> **对照基准**: 原始 16 周 Monorepo 重构计划
> **当前进度**: Phase 0 ✅ (carpai-internal 编译通过, 0 error) | Phase 1B ✅ (Local 实现已完成)

---

## 一、执行摘要

| 维度 | 评级 | 说明 |
|------|------|------|
| **整体可行性** | 🟡 可行（需调整） | 架构方向正确，但时间线过于激进 |
| **与原计划兼容性** | 🟡 部分兼容 | 核心路径一致，SDK 阶段缺失，集成测试时间不足 |
| **技术方案正确性** | ✅ 正确 | Crate 层次、依赖方向、命名规范均合理 |
| **风险评估完整性** | ✅ 完善 | 覆盖了协作风险和技术风险 |
| **可执行性** | 🟡 需优化 | 模块迁移量/时间比不合理 |

**总评**: **有条件通过** — 需解决 4 个 Blocker 和 4 个 Improvement 后执行。

---

## 二、与原 16 周时间线的冲突矩阵

### 2.1 阶段对比表

```
原计划 (16周)                    v2.0 (8周)                      冲突
══════════════════               ════════════════                 ═════
Week 1-2: 编译修复+安全+FG       ✅ 已完成 (Phase 0)              ✅ 无冲突
Week 3-4: 6大Trait定义           ✅ 已完成+Local实现             ✅ 超前完成
Week 5-8: server独立+引擎注入     Week1-8 Beta并行 (gRPC+REST+WS)  🟡 范围更广
Week 9-10: CLI双模式             Week1-8 Gamma并行 (TUI+双模式)   🟡 时间提前
Week 11-12: SDK增强(OpenAI API)  ❌ 完全缺失                      🔴 BLOCKER
Week 13-16: 集成+性能+部署        Week7-8 统一联调 (仅2天)         🔴 严重压缩
```

### 2.2 冲突详情

#### 🔴 Blocker #1: SDK 增强阶段完全缺失

**现状**: v2.0 中 `carpai-sdk` 标记为 "✅ 已存在 (IDE 插件 SDK)"

**实际问题**:
- 当前 `carpai-sdk` 仅是 IDE 插件骨架代码
- **没有** OpenAI 兼容 `/v1/chat/completions` 端点定义
- **没有** Agent Session CRUD API
- **没有** gRPC/HTTP client 库供 IDE 插件调用 Server

**影响**: 如果不补上，IDE 插件（VSCode/JetBrains/Neovim）无法接入重构后的架构。

**决策: 独立 SDK 方案 (已确认 ✅)**

在 v2.0 的 **Week 9-10** 追加 `carpai-sdk` 增强:

- 定义 OpenAI 兼容请求/响应类型 (复用 `inference_backend` 已有的 `ChatCompletionRequest/Response`)
- 定义 Agent Session CRUD API 接口契约
- 实现 gRPC client + HTTP client helper (轻量, 仅依赖 reqwest/tonic)
- 输出 OpenAPI 3.0 spec (供多语言 SDK 自动生成)

**为什么不做"并入 Server REST 层"**: 已否决。理由:
- IDE 插件编译体积膨胀 10-20x (拉入 axum/sqlx/redis 等服务端依赖)
- IDE 进程内存占用增加 50-100MB (不必要的 DB 连接池/JWT 运行时)
- 安全审计边界扩大 (插件进程不应包含服务端代码)
- 无法支持多语言 SDK 自动生成 (Python/Go/Java 等)
- 版本耦合: Server 升级强制所有插件同步升级

#### 🔴 Blocker #2: 时间线压缩 50% 导致的集成风险

**数据对比**:
| 指标 | 原计划 | v2.0 | 压缩比 |
|------|--------|------|--------|
| 总工期 | 16 周 | 8 周 | 50% |
| 集成+测试 | 4 周 (W13-16) | 2 天 (W8D4) | **93%** |
| 性能基准 | 2 周 (W15-16) | 0 周 | 100% 砍掉 |
| 部署文档 | 1 周 (W16) | 0 周 | 100% 砍掉 |

**核心问题**: v2.0 的 Week 8 Day 4 计划一天内完成:
- E2E 测试 (4 条链路: CLI-local, Server, CLI-remote, IDE)
- clippy --workspace 全量清理
- cargo doc 全量生成
- 性能基准报告
- 最终 tag

这在工程上不可行。单是 `cargo check --workspace` 在这个规模的项目就需要 10-30 分钟。

**建议**: 将总工期调整为 **10-12 周**, 其中:
- Week 1-6: 三组并行开发 (不变)
- Week 7-8: 组内集成测试 (Beta/Gamma 各自验证)
- Week 9-10: 跨组联调 + E2E (Alpha 主导)
- Week 11-12: 性能基准 + 部署文档 + 安全审计

#### 🔴 Blocker #3: 模块迁移量/时间比严重失衡

**v2.0 计划**:
- Week 2-3: 迁移 ~40 个模块
- 分配时间: **6 人天**
- 平均每个模块: **0.15 天 (~1.2 小时)**

**实际经验值** (基于已完成的 Phase 0/1 工作):
- 每个模块迁移平均需要 **0.5-1.5 天**
- 包含: 复制文件 → 修改 import 路径 → 解耦隐式依赖 → 处理编译错误 → 验证测试
- 特别是跨模块依赖 (如 `agent.rs` 依赖 `memory.rs`, `tool.rs`, `session.rs`) 需要批量处理

**修正估算**:

| 批次 | 模块数 | 建议时间 | 内容 |
|------|--------|---------|------|
| Batch A (Agent 系统) | ~12 | 3-4 天 | agent.rs, runtime, sub_agents, skill_system 等 |
| Batch B (记忆+会话) | ~14 | 3-4 天 | memory_*, session_, knowledge_* |
| Batch C (工具+补全) | ~8 | 2 天 | tool*, completion* |
| Batch D (重构+AST+Git+错误) | ~18 | 4-5 天 | refactor*, ast*, git*, error* |
| **合计** | **~52** | **12-15 天** | |

**建议**: 将模块迁移扩展到 **Week 2-5** (而非 Week 2-3), 或分两轮:
- Round 1 (Week 2-3): 只迁移核心路径模块 (agent_loop, session_impl, tool_executor_impl, inference_impl)
- Round 2 (Week 4-5): 迁移辅助模块 (perf, cache, error_recovery 等)

#### 🔴 Blocker #4: Local 实现位置需迁移

**当前状态**: Local 实现已写在 `crates/carpai-internal/src/local_impls.rs`

**v2.0 要求**: 放在 `crates/carpai-core/` 下 (如 `session_impl.rs`, `tool_executor_impl.rs`)

**为什么必须迁移**:
1. `carpai-internal` 的设计定位是 **Pure Trait Layer** (零业务逻辑, 仅接口定义)
2. Local 实现包含具体业务逻辑 (文件 I/O, HashMap 存储, 进程 spawn)
3. 放在 internal 会破坏纯净性原则
4. v2.0 的依赖图明确要求 `carpai-core` 是实现层

**迁移计划**: 见本文档第三节。

---

## 三、AppConfig 三层配置方案决策

### 3.1 现状分析

我们已有的 `AppConfig` (`carpai-internal/src/agent_context.rs`):

```rust
pub struct AppConfig {
    pub mode: AppMode,              // Cli / Server / Client
    pub data_dir: PathBuf,          // 数据根目录
    pub working_dir: PathBuf,       // 工作目录
    pub default_model: String,
    pub max_context_tokens: usize,
    pub tools_enabled: bool,
    pub default_tool_mode: ExecutionMode,
    pub vfs_enabled: bool,
    pub vfs_root: Option<PathBuf>,
    pub memory_enabled: bool,
    pub event_bus_enabled: bool,
}
```

### 3.2 工程师提议的三层方案 (v2.0 §3.1)

```
AppConfig (Layer 0 — trait 层基础配置)
    ↓ serde(flatten)
CoreConfig (Layer 1 — 业务逻辑层配置)
    ↓ serde(flatten)
ServerConfig / CliConfig (Layer 2 — 产品层配置)
```

### 3.3 最终决策: 采用三层方案 ✅

**理由**:
1. 符合单向依赖规则 (每层只依赖上层)
2. `serde(flatten)` 实现无缝组合，配置文件格式统一
3. 每层只关注自己的关注点
4. 方便单元测试 (每层可独立 mock)

### 3.4 具体分层定义

#### Layer 0: AppConfig (保持在 carpai-internal)

```rust
// crates/carpai-internal/src/agent_context.rs
// 【不变】仅做以下微调:
// - 移除 data_dir (数据存储位置不应在 trait 层决定)
// - 新增 log_level (全局日志级别, 所有产品都需要)

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    /// 应用运行模式
    pub mode: AppMode,

    /// 工作目录 (项目根目录)
    pub working_dir: PathBuf,

    /// 默认推理模型
    pub default_model: String,

    /// 最大上下文 token 数
    pub max_context_tokens: usize,

    /// 是否启用工具执行
    pub tools_enabled: bool,

    /// 默认工具执行模式
    pub default_tool_mode: ExecutionMode,

    /// 是否启用 VFS
    pub vfs_enabled: bool,

    /// VFS 根路径限制
    pub vfs_root: Option<PathBuf>,

    /// 是否启用记忆系统
    pub memory_enabled: bool,

    /// 是否启用事件总线
    pub event_bus_enabled: bool,

    // === 新增 ===
    /// 日志级别 (trace/debug/info/warn/error)
    #[serde(default = "default_log_level")]
    pub log_level: String,
}

fn default_log_level() -> String { "info".into() }
```

#### Layer 1: CoreConfig (新建于 carpai-core)

```rust
// crates/carpai-core/src/config.rs
use carpai_internal::{AppConfig, ExecutionMode};
use serde::{Deserialize, Serialize};

/// 补全 Provider 配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub provider_type: String,     // "local" | "openai" | "anthropic"
    pub endpoint: Option<String>,
    pub api_key: Option<String>,   // 从环境变量读取, 不写入配置文件
    pub model: Option<String>,     // 覆盖 default_model
    pub timeout_secs: u64,
}

/// Core 层配置 — 业务逻辑需要的参数
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoreConfig {
    /// 基础配置 (来自 trait 层)
    #[serde(flatten)]
    pub base: AppConfig,

    // === 存储相关 ===
    /// 数据目录 (会话、记忆、缓存等的根目录)
    pub data_dir: PathBuf,
    /// 会话存储子目录名
    #[serde(default = "default_session_dir")]
    pub session_subdir: String,
    /// 记忆持久化子目录名
    #[serde(default = "default_memory_dir")]
    pub memory_subdir: String,

    // === 并发控制 ===
    /// 最大并发工具执行数
    #[serde(default = "default_max_concurrent_tools")]
    pub max_concurrent_tools: usize,
    /// Agent 循环最大迭代次数 (防无限循环)
    #[serde(default = "default_max_agent_iterations")]
    pub max_agent_iterations: usize,

    // === 补全 ===
    /// 代码补全 provider 配置
    #[serde(default)]
    pub completion_provider: ProviderConfig,

    // === 缓存 ===
    /// 内存缓存大小限制 (MB)
    #[serde(default = "default_cache_size")]
    pub cache_size_mb: usize,
    /// 是否启用磁盘缓存
    #[serde(default = "default_disk_cache")]
    pub disk_cache_enabled: bool,
}

fn default_session_dir() -> String { "sessions".into() }
fn default_memory_dir() -> String { "memory".into() }
fn default_max_concurrent_tools() -> usize { 5 }
fn default_max_agent_iterations() -> usize { 100 }
fn default_cache_size() -> usize { 512 }
fn default_disk_cache() -> bool { true }

impl CoreConfig {
    /// 获取会话存储的完整路径
    pub fn session_store_path(&self) -> PathBuf {
        self.data_dir.join(&self.session_subdir)
    }

    /// 获取记忆存储的完整路径
    pub fn memory_store_path(&self) -> PathBuf {
        self.data_dir.join(&self.memory_subdir)
    }

    /// 从文件加载配置 (支持 defaults → file → env vars 三级覆盖)
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        // 1. 先用 default()
        let mut config = Self::default();

        // 2. 从文件覆盖 (如果存在)
        if path.exists() {
            let content = std::fs::read_to_string(path)
                .map_err(|e| ConfigError::Io(e))?;
            let file_config: CoreConfig = toml::from_str(&content)
                .map_err(|e| ConfigError::Parse(e))?;
            config = file_config;
        }

        // 3. 从环境变量覆盖 (CARPAI_ 前缀)
        if let Ok(val) = std::env::var("CARPAI_DATA_DIR") {
            config.data_dir = PathBuf::from(val);
        }
        if let Ok(val) = std::env::var("CARPAI_DEFAULT_MODEL") {
            config.base.default_model = val;
        }
        if let Ok(val) = std::env::var("CARPAI_LOG_LEVEL") {
            config.base.log_level = val;
        }
        // ... 更多环境变量映射

        Ok(config)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Parse error: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("Missing required field: {0}")]
    MissingField(String),
}
```

#### Layer 2a: ServerConfig (carpai-server)

```rust
// crates/carpai-server/src/config.rs
use carpai_core::CoreConfig;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TlsConfig {
    pub cert_path: PathBuf,
    pub key_path: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseConfig {
    pub url: String,           // postgres://user:pass@host:db
    pub max_connections: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RedisConfig {
    pub url: String,           // redis://host:port
    pub pool_size: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    #[serde(flatten)]
    pub core: CoreConfig,

    // === 网络监听 ===
    #[serde(default = "default_listen_addr")]
    pub listen_addr: String,
    #[serde(default = "default_port")]
    pub port: u16,

    // === TLS ===
    pub tls: Option<TlsConfig>,

    // === 数据库 ===
    pub database: DatabaseConfig,
    pub redis: Option<RedisConfig>,

    // === 认证 ===
    /// JWT HMAC 密钥 (必须 ≥ 256 bit)
    pub jwt_secret: String,
    /// Token 过期时间 (小时)
    #[serde(default = "default_jwt_expiry")]
    pub jwt_expiry_hours: u64,

    // === 多租户 ===
    #[serde(default)]
    pub multi_tenant: bool,
    /// 默认租户 ID
    #[serde(default = "default_tenant")]
    pub default_tenant_id: String,

    // === 企业功能开关 ===
    #[serde(default)]
    pub audit_log_enabled: bool,
    #[serde(default)]
    pub rate_limit_enabled: bool,
    #[serde(default = "default_rate_limit")]
    pub rate_limit_rpm: u64,
}

fn default_listen_addr() -> String { "0.0.0.0".into() }
fn default_port() -> u16 { 8080 }
fn default_jwt_expiry() -> u64 { 24 }
fn default_tenant() -> String { "default".into() }
fn default_rate_limit() -> u64 { 60 }
```

#### Layer 2b: CliConfig (carpai-cli)

```rust
// crates/carpai-cli/src/config.rs
use carpai_core::CoreConfig;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThemeConfig {
    pub syntax_theme: String,        // "base16-dark" etc.
    pub ui_color: String,            // hex color
    pub enable_bold: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeybindConfig {
    pub send_message: String,        // "Enter"
    pub interrupt: String,           // "Esc" or "Ctrl+C"
    pub toggle_help: String,         // "?"
    pub toggle_file_tree: String,    // "Ctrl+T"
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClipboardConfig {
    #[serde(default = "default_auto_copy")]
    pub auto_copy_response: bool,
    pub external_editor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StartupConfig {
    #[serde(default)]
    pub show_banner: bool,
    #[serde(default = "default_startup_timeout")]
    pub model_load_timeout_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CliConfig {
    #[serde(flatten)]
    pub core: CoreConfig,

    // === UI ===
    #[serde(default)]
    pub theme: ThemeConfig,
    #[serde(default)]
    pub keybinds: KeybindConfig,

    // === 编辑器集成 ===
    #[serde(default)]
    pub clipboard: ClipboardConfig,

    // === 启动 ===
    #[serde(default)]
    pub startup: StartupConfig,

    // === 远程模式 ===
    /// 远程 Server 地址 (设置后自动进入 remote mode)
    pub remote_server_url: Option<String>,
    /// 远程连接超时 (秒)
    #[serde(default = "default_remote_timeout")]
    pub remote_timeout_secs: u64,
}

fn default_auto_copy() -> bool { false }
fn default_startup_timeout() -> u64 { 30 }
fn default_remote_timeout() -> u64 { 10 }
```

### 3.5 配置文件示例

```toml
# ~/.carpai/config.toml (CLI 模式)
mode = "cli"
working_dir = "/home/user/projects/myapp"
default_model = "claude-sonnet-4-20250514"
max_context_tokens = 200000
log_level = "info"

[core]
data_dir = "~/.carpai"
max_concurrent_tools = 5
cache_size_mb = 512

[core.completion_provider]
provider_type = "local"
endpoint = "http://localhost:11434"

[theme]
syntax_theme = "base16-dark"

[keybinds]
send_message = "Enter"
interrupt = "Escape"
```

```toml
# /etc/carpai/server.toml (Server 模式)
mode = "server"
working_dir = "/var/lib/carpai"
log_level = "warn"

[core]
data_dir = "/var/lib/carpai/data"
max_concurrent_tools = 20

[listen]
addr = "0.0.0.0"
port = 8080

[database]
url = "postgres://carpai:secret@db:5432/carpai"
max_connections = 20

[jwt]
secret = "<generated-hex>"
expiry_hours = 24

[multi_tenant]
enabled = true
default_tenant = "org-default"
```

---

## 四、v2.0 计划调整建议汇总

### 4.1 必须调整 (Blockers)

| # | 问题 | v2.0 原方案 | 调整后方案 | 影响 |
|---|------|------------|-----------|------|
| B1 | SDK 增强缺失 | 不包含 | **Week 9-10 追加独立 SDK** (已确认方案A, 方案B已否决) | +2 周 |
| B2 | 时间线过激进 | 8 周 | **10-12 周** | +2-4 周 |
| B3 | 模块迁移不现实 | 40 模块/6 天 | 52 模块/12-15 天 (Week 2-5) | +6-9 天 |
| B4 | Local 实现位置错误 | 在 carpai-internal | **迁移到 carpai-core** | 本轮执行 |

### 4.2 建议调整 (Improvements)

| # | 问题 | 建议 |
|---|------|------|
| I1 | async_stream/pin_utils 依赖 | local_impls 迁移到 core 时改用手动 Stream impl，避免引入额外依赖 |
| I2 | LogProbs 类型未定义 | 在 inference_backend 或 local_impls 中补充完整定义 |
| I3 | 性能模块延后 | perf/cache/concurrency_optimizer 从 Week 6 提前到 Week 4 |
| I4 | E2E 测试时间不足 | 从 1 天增加到 2-3 天 |

### 4.3 保持不变 (Good Parts)

- ✅ 三组分工比例 40/30/30 合理
- ✅ 依赖方向铁律正确且必要
- ✅ 命名规范完善且可执行
- ✅ 接口契约冻结机制 (Week 3) 关键
- ✅ 代码合并策略清晰 (分支模型 + CI 门禁)
- ✅ 死代码清理清单准确 (18 个模块)
- ✅ 风险缓解措施到位 (5 协作风险 + 4 技术风险)
- ✅ 同步会议节奏合理 (Daily + Weekly Sync + Integration Prep)
- ✅ Bug 分类与分派流程明确

---

## 五、修订后的推荐时间线 (12 周)

```
Week:   1    2    3    4    5    6    7    8    9   10   11   12
        ├────┴────┤├────┴────┤├────┴────┤├────┴────┤├────┴────┤├───┴───┤
Alpha:  [CORE][MIGRATE-A][MIGRATE-B/C/D][CLEAN][PERF][SUPPORT][INTEGRATE][SDK-DEF]
              │                        │                          │      │
Beta:   [SKEL][GRPC][REST_WS][AUTH][WIRE][ENTERP][TEST]          │      │
                                    │                              │      │
Gamma:  [SKEL][TUI_STRIP][CMD][AMBIENT][DASH][POLISH][TEST]       │      │
                                          │                         │      │
                                          └─────────────────────────┼──────┘
                                                                    │
                                                         Week 9-12: SDK + E2E + PERF + DOCS
```

**关键里程碑**:
- **Week 4 End**: `cargo check -p carpai-core` 通过 (Alpha Phase 1 Done)
- **Week 8 End**: `cargo check -p carpai-server` + `cargo check -p carpai-cli` 通过
- **Week 10 End**: `cargo check --workspace` 通过 + E2E 全链路通过
- **Week 12 End**: v1.0.0 release (含性能基准 + 部署文档 + 安全审计)

---

## 六、结论与下一步行动

### 立即执行 (本轮)

1. ✅ 创建 `crates/carpai-core/` crate (Cargo.toml + src/lib.rs)
2. ✅ 将 `local_impls.rs` 从 `carpai-internal` 迁移到 `carpai-core`
3. ✅ 定义 `CoreConfig` (三层配置方案的 Layer 1)
4. ✅ 更新 `carpai-internal` 移除 local_impls 相关声明
5. ✅ 验证两个 crate 分别编译通过

### 近期执行 (Week 1-2 of v2.0)

6. 开始 Batch A 模块迁移 (Agent 系统, ~12 个模块)
7. 定义接口契约 (`execute_agent_turn`, `AgentTurnOutput`, `build_local_agent_context`)
8. 发布接口契约文档给 Beta/Gamma

### 中期执行 (Week 3-4)

9. 完成 Batch B/C/D 迁移
10. 死代码清理 (18 个模块)
11. 建立 `cargo check -p carpai-core` 编译基线

---

> **文档版本**: v1.0
> **下次更新**: carpai-core crate 创建完成后，评估是否需要进一步调整
> **审批状态**: 待确认 (请审阅后回复 "APPROVED" 或 "REVISE")
