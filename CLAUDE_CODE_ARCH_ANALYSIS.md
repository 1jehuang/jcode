# Claude Code CLI → JCode 架构移植报告

## 分析源

`D:\studying\Codecargo\opensource\claude_code_src\src` — Anthropic Claude Code CLI (TypeScript/React Ink)

## 提炼的关键架构模式

### 1. `buildTool()` 工厂模式 → `define_tool!` 宏

**源**: `Tool.ts` 中的 `buildTool<D>(def: D): BuiltTool<D>`
- 接受部分工具定义 (`ToolDef`)，填充安全默认值
- 默认值：`isEnabled=true`, `isConcurrencySafe=false`, `isReadOnly=false`, `isDestructive=false`, `checkPermissions=allow`

**移植**: `crates/jcode-tool-core/src/macros.rs`
- `define_tool!()` — 简化工具定义，自动生成 `Tool` trait 实现
- `build_tool_adapter!()` — 动态工具适配器（用于 MCP 等场景）
- `tool_matcher!()` — 生成工具名称匹配函数

### 2. 工具别名系统

**源**: `Tool.ts` 中的 `aliases?: string[]` + `toolMatchesName()`
- 每个工具声明自己的别名
- `findToolByName()` 支持别名查找

**移植**: `crates/jcode-tool-core/src/lib.rs`
- `Tool` trait 新增 `aliases() -> &[&str]` 方法
- `src/tool/mod.rs` 中 `resolve_tool_name()` 使用别名
- `Registry::execute()` 通过别名查找工具

### 3. 权限上下文系统

**源**: `Tool.ts` 中的 `ToolPermissionContext`, `PermissionResult`
- 细粒度权限：`mode`, `alwaysAllowRules`, `alwaysDenyRules`, `alwaysAskRules`
- `PermissionResult`: `{ behavior: 'allow' | 'deny' | 'ask', updatedInput }`

**移植**: `crates/jcode-tool-core/src/permissions.rs`
- `PermissionMode` — Default, Bypass, Auto
- `PermissionRule`, `PermissionResult` — 规则和结果
- `ToolPermissionContext` — 完整的权限上下文
- `ToolFilterContext` — 工具过滤上下文（简单模式/协调器模式/远程模式）
- `ToolSafetyContext` — 远程/桥接安全白名单

### 4. `getAllBaseTools()` 模式

**源**: `tools.ts` 中的 `getAllBaseTools(): Tools`
- 单一真相源，返回数组
- 使用 feature flag 条件引入工具
- `filterToolsByDenyRules()` 过滤
- `assembleToolPool()` 合并内置 + MCP 工具

**移植**: `src/tool/mod.rs`
- `Registry::get_all_tools()` — 列出所有工具
- `Registry::filter_tools()` — 按过滤上下文筛选
- `Registry::assemble_tool_pool()` — 合并内置+外部工具，去重排序

### 5. 并行命令加载 + 优雅降级

**源**: `commands.ts` 中的 `loadAllCommands()`, `getSkills()`
- 使用 `Promise.all()` 并行从 4+ 源加载
- 每个源独立 try/catch，失败不影响其他源
- `getSkills()` 返回空数组而非崩溃
- `memoize()` 缓存昂贵加载结果

**移植**: `crates/jcode-skills/src/lib.rs`
- `SkillSource` 枚举 — 定义多个技能源
- `load_from_sources()` — 使用 `join_all()` 并行加载
- `register_fallible()` — 单个技能注册失败不级联
- 目录加载优雅处理文件不存在/权限错误

### 6. 任务系统模式

**源**: `tasks.ts` 中的 `getAllTasks()` / `getTaskByType()`
- 简洁的数组注册模式
- 通过类型查找

**可移植**: 未来可简化 `jcode-agent-advanced` 的任务注册

### 7. 协调器系统提示

**源**: `coordinatorMode.ts` 中的 `getCoordinatorSystemPrompt()`
- 370+ 行的全面协调器提示
- 工具白名单映射
- 并发策略说明

**移植**: 可参考增强 `jcode-swarm-core` 的系统提示生成

### 8. 远程安全命令白名单

**源**: `commands.ts` 中的 `REMOTE_SAFE_COMMANDS` / `BRIDGE_SAFE_COMMANDS`
- 白名单机制
- `filterCommandsForRemoteMode()` 过滤

**移植**: `crates/jcode-tool-core/src/permissions.rs` 中的 `ToolSafetyContext`

### 9. 并发安全检查

**源**: `Tool.ts` 中的 `isConcurrencySafe(input): boolean`
- 只读/搜索工具可并行
- 写入/命令工具必须串行

**移植**: `crates/jcode-tool-core/src/lib.rs` 中的 `is_concurrency_safe()` — 默认与 `is_read_only()` 一致

### 10. 数据驱动配置

**源**: 全局 `feature()` 函数 + Statsig GrowthBook
- 在编译时消除死代码
- 运行时通过环境变量控制

**建议**: jcode 可考虑类似的特征标志系统

## 已修改文件清单

| 文件 | 修改类型 | 说明 |
|------|---------|------|
| `crates/jcode-tool-core/src/lib.rs` | 增强 | 新增 `aliases()`, `is_concurrency_safe()`, `is_enabled()` |
| `crates/jcode-tool-core/src/macros.rs` | **新建** | `define_tool!`, `build_tool_adapter!`, `tool_matcher!` |
| `crates/jcode-tool-core/src/permissions.rs` | **新建** | 权限上下文 + 过滤 + 安全白名单 |
| `src/tool/mod.rs` | 增强 | 别名感知的 `resolve_tool_name()`, `get_all_tools()`, `filter_tools()`, `assemble_tool_pool()` |
| `crates/jcode-skills/src/lib.rs` | 增强 | 并行加载 (`load_from_sources`), 优雅降级 (`register_fallible`) |
| `crates/jcode-skills/Cargo.toml` | 增强 | 新增 `futures`, `tokio-stream` 依赖 |

## 关键架构对比

| 维度 | Claude Code CLI | JCode (改进后) |
|------|----------------|-----------------|
| 工具定义 | `buildTool()` 工厂 | `define_tool!()` 宏 |
| 工具别名 | `aliases[]` 字段 | `aliases()` trait 方法 |
| 权限模型 | `PermissionContext` + `PermissionResult` | `ToolPermissionContext` + `PermissionResult` |
| 工具注册 | `getAllBaseTools()` 数组 | `base_tools()` + `OnceLock` 缓存 |
| 工具过滤 | `filterToolsByDenyRules()` | `filter_tools()` |
| 工具合并 | `assembleToolPool()` | `assemble_tool_pool()` |
| 技能加载 | `Promise.all()` 并行 + try/catch | `join_all()` 并行 + fallible |
| 上下文溢出 | `maxResultSizeChars` 字段 | `guard_context_overflow()` |
| 安全白名单 | `REMOTE_SAFE_COMMANDS` | `ToolSafetyContext` |
