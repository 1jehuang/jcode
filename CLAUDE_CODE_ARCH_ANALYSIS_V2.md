# 第二轮分析：Claude Code CLI → JCode 新增移植

## 新增分析模块

| 模块 | 文件数 | 关键洞察 |
|------|--------|---------|
| `services/mcp/` | 22 | MCP 客户端架构、连接池、认证、权限 |
| `services/analytics/` | 9 | 双接收器遥测、磁盘事件持久化、退避重试 |
| `bridge/` | 31 | 多传输层、会话持久化、心跳管理 |
| `hooks/` | 107 | 14 种钩子事件类型、同步/异步响应模式 |
| `state/` | 6 | React 不可变状态 + 选择器模式 |
| `types/` | 21 | 命令/权限/钩子/日志/插件类型系统 |
| `utils/permissions/` | 24 | 完整权限检查管道、自动模式分类器 |
| `utils/settings/` | 17 | 5 级优先级设置系统、MDM 策略支持 |

## 已存在的通用功能（无需重复移植）

| 功能 | 模块 | 状态 |
|------|------|------|
| Hook 系统 | `jcode-hooks` | ✅ 已存在 |
| 安全沙箱 | `jcode-sandbox` | ✅ 已存在（含 DenialTracker、AutoMode、SSRF） |
| 上下文压缩 | `jcode-compaction-core` | ✅ 已存在 |
| 遥测 | `jcode-telemetry` | ✅ 已存在 |
| MCP | `src/tool/mcp/` | ✅ 已存在 |
| 会话持久化 | `jcode-session-persist` | ✅ 已存在 |
| 跨文件修复 | `jcode-cross-file-repair` | ✅ 已存在 |
| 子 Agent | `jcode-tool-core/sub_agent.rs` | ✅ 已存在 |
| 并行工具执行 | `streaming_executor.rs` | ✅ 已存在 |
| 拒绝追踪 | `jcode-sandbox/denial_tracker` | ✅ 已存在 |

## 新增 5 个移植模块

| 模块 | 文件 | 说明 |
|------|------|------|
| `result_budget.rs` | **新建** | 全局聚合预算 + 结果持久化到磁盘（源自 `toolResultStorage.ts`） |
| `file_history.rs` | **新建** | 文件快照 + 回滚（源自 `fileHistory.ts`） |
| `debug_log.rs` | **新建** | 会话级调试文件 + BufferedWriter（源自 `debug.ts` + `log.ts`） |
| `error_types.rs` | **新建** | 结构化错误层次（源自 `errors.ts`） |
| `settings_priority.rs` | **新建** | 5 级优先级设置系统（源自 `settings/constants.ts`） |

## 总移植总结（两轮）

共从 Claude Code CLI 中提炼 **15 个关键架构模式**，已移植 **10 个模块**到 jcode：

| 轮次 | 移植数 | 说明 |
|------|--------|------|
| 第一轮 | 5 | `define_tool!` 宏、别名、权限上下文、`getAllTools` 模式、并行技能加载 |
| 第二轮 | 5 | 结果预算管理、文件历史、调试日志、错误类型、设置优先级 |
| 已有 | 5 | Hook、沙箱/拒绝追踪、压缩、MCP、会话持久化 |
| **总计** | **15** | **15/15 关键模式已覆盖** |

## 建议后续增强方向

1. **Thinking 支持**（`thinking.ts`）— 为 provider 添加扩展思考能力
2. **MCP 客户端增强**（`services/mcp/client.ts`）— 连接池、认证流程
3. **特性标志**（`services/analytics/growthbook.ts`）— 集中式特性开关
4. **Bridge/Remote 协议**（`bridge/` 31 文件）— 远程 gRPC 模式增强
5. **提交归属**（`commitAttribution.ts`）— Telemetry 数据源
