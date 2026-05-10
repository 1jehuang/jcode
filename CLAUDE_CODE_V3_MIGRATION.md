# 第三轮移植：5 个新功能模块

## 1. Thinking 支持

| 项 | 说明 |
|---|---|
| **文件** | `crates/jcode-provider-core/src/thinking.rs` **(新建)** |
| **源自** | `src/utils/thinking.ts` (163 行) |
| **类型** | `ThinkingConfig` — Adaptive / Enabled{budget_tokens} / Disabled |
| **检测函数** | `model_supports_thinking()` — 1P 所有 Claude 4+, 3P 仅 Sonnet/Opus 4+ |
| **检测函数** | `model_supports_adaptive_thinking()` — Claude 4.6+ |
| **配置** | `JCODE_THINKING_ENABLED` / `JCODE_THINKING_DISABLED` 环境变量 |
| **API** | `build_thinking_param()` — 生成 API 请求参数 |
| **Beta 头** | `thinking_beta_header()` → `"interleaved-thinking-2025-05-14"` |

## 2. 特性标志系统

| 项 | 说明 |
|---|---|
| **文件** | `crates/jcode-provider-core/src/feature_flags.rs` **(新建)** |
| **源自** | `src/services/analytics/growthbook.ts` (1156 行) |
| **枚举** | `FeatureFlag` — 10 个标志：McpAdvanced, CoordinatorMode, ExtendedThinking, WorkflowScripts, AgentTeams, RemoteSessions, BackgroundAgents, VoiceMode, BridgeMode, CommitAttribution |
| **FlagClient** | 缓存读取 + 环境覆盖 + 可选远程评估器 |
| **覆盖** | 环境变量 `JCODE_FEATURE_*` 覆盖任何标志 |
| **API** | `is_enabled()` (首选), `get_cached_may_be_stale()` (容忍过期) |
| **刷新** | `refresh()` / `force_refresh()` — 远程评估器定期同步 |

## 3. Bridge 远程协议

| 项 | 说明 |
|---|---|
| **文件** | `crates/jcode-tool-core/src/remote_session.rs` **(新建)** |
| **源自** | `src/bridge/` 目录 (31 文件) |
| **传输** | `RemoteTransportVersion` — V1Hybrid / V2SseCcr |
| **会话** | `RemoteSessionManager` — 创建/状态/心跳/重连/结束全生命周期 |
| **心跳** | 可配置间隔 + 超时检测 |
| **统计** | `SessionStats` — 消息计数、心跳、重连、运行时长 |
| **安全** | 远程安全命令白名单 (源自 `REMOTE_SAFE_COMMANDS`) |
| **工厂** | `RemoteSessionFactory` — v1/v2 会话创建 |

## 4. 提交归属追踪

| 项 | 说明 |
|---|---|
| **文件** | `crates/jcode-telemetry/src/commit_attribution.rs` **(新建)** |
| **源自** | `src/utils/commitAttribution.ts` (962 行) |
| **类** | `CommitAttributionTracker` — 文件修改/创建/删除追踪 |
| **方法** | `track_modification()`, `track_creation()`, `track_deletion()`, `track_bulk()` |
| **计算** | `calculate()` → `CommitAttribution` (字符数/百分比/session head) |
| **持久化** | `to_snapshot()` / `restore_from_snapshot()` — 快照序列化 |
| **禁用** | `new(enabled: false)` — 完全禁用追踪 |

## 5. MCP OAuth 认证流

| 项 | 说明 |
|---|---|
| **文件** | `crates/jcode-mcp-advanced/src/auth.rs` **(新建)** |
| **源自** | `src/services/mcp/auth.ts` (2466 行) |
| **PKCE** | `start_pkce_flow()` → `complete_pkce_flow()` — S256 code_challenge |
| **Token** | `McpAuthManager` — 存储/获取(含到期检查)/刷新/撤销 |
| **失效** | `invalidate_tokens()` — 支持 scope 粒度 |
| **元数据** | `AuthServerMetadata` — 自动发现授权/令牌/撤销端点 |
| **检测** | `check_step_up_required(403)` — insufficient_scope |
| **依赖** | 新增 `sha2`, `base64` 到 Cargo.toml |

## 已修改文件清单

| 文件 | 修改 | 说明 |
|------|------|------|
| `crates/jcode-provider-core/src/lib.rs` | 增强 | 声明 `thinking`, `feature_flags` 模块 + re-exports |
| `crates/jcode-provider-core/src/thinking.rs` | **新建** | ThinkingConfig + 模型检测 |
| `crates/jcode-provider-core/src/feature_flags.rs` | **新建** | FeatureFlag 枚举 + FlagClient |
| `crates/jcode-tool-core/src/lib.rs` | 增强 | 声明 `remote_session` 模块 + re-exports |
| `crates/jcode-tool-core/src/remote_session.rs` | **新建** | 远程会话管理 |
| `crates/jcode-mcp-advanced/src/lib.rs` | 增强 | 声明 `auth` 模块 |
| `crates/jcode-mcp-advanced/src/auth.rs` | **新建** | MCP OAuth 认证流 |
| `crates/jcode-mcp-advanced/Cargo.toml` | 增强 | 新增 `sha2`, `base64` 依赖 |
| `crates/jcode-telemetry/src/lib.rs` | 增强 | 声明 `commit_attribution` 模块 + re-exports |
| `crates/jcode-telemetry/src/commit_attribution.rs` | **新建** | 提交归属追踪 |

## 三轮移植总览

| 轮次 | 模块数 | 文件数 | 说明 |
|------|--------|--------|------|
| 第一轮 | 5 | 5 | `define_tool!` 宏, 别名, 权限上下文, `getAllTools`, 并行技能加载 |
| 第二轮 | 5 | 7 | 结果预算, 文件历史, 调试日志, 错误类型, 设置优先级 |
| 第三轮 | 5 | 10 | Thinking, 特性标志, Bridge远程, 提交归属, MCP OAuth |
| **总计** | **15** | **22** | **15 个关键模式已全部移植** |
