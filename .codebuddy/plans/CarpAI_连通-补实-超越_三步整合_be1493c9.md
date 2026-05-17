---
name: CarpAI 连通-补实-超越 三步整合
overview: 三阶段重构：(1)连通 - 打通6个独立模块之间的断层（LSP→跨文件修复→多文件编辑→Swarm→MCP→技能系统）；(2)补实 - 将占位/空壳实现替换为真实代码（WS层LSP、TypeChecker::check_with_lsp、Sub-Agent执行引擎、StdioTransport）；(3)超越 - 实现Swarm+LSP融合（SwarmTurnStrategy、LSP事件桥接、符号级冲突检测），超越Cursor的独立Agent模式
todos:
  - id: connect-deps
    content: 在根Cargo.toml中添加4个孤岛crate依赖并修复StdioTransport的unimplemented!()致命桩
    status: completed
  - id: connect-ast-bridge
    content: 实现AstEdit→FileSet类型适配器（bridge.rs），打通跨文件修复→多文件原子编辑数据流
    status: completed
    dependencies:
      - connect-deps
  - id: connect-ws-lsp
    content: WS层4个LSP处理器接入jcode-lsp，替换模拟数据为真实LSP调用
    status: completed
    dependencies:
      - connect-deps
  - id: solidify-type-checker
    content: TypeChecker::check_with_lsp()注入真实LSP诊断能力（feature门控）
    status: completed
    dependencies:
      - connect-ws-lsp
  - id: solidify-ast-multilang
    content: TreeSitterAstAdapter升级TypeScript/Python/Go真实tree-sitter解析
    status: completed
    dependencies:
      - connect-ws-lsp
  - id: solidify-sub-agent
    content: sub_agents.rs execute_task_real()接入实际Provider调用
    status: completed
    dependencies:
      - connect-deps
  - id: solidify-self-correction
    content: 自修正循环连接AiFixProvider trait，支持LLM修复建议注入
    status: completed
    dependencies:
      - solidify-type-checker
  - id: transcend-swarm-strategy
    content: 实现SwarmTurnStrategy，在swarm agent turn中注入LSP诊断上下文
    status: completed
    dependencies:
      - solidify-sub-agent
      - solidify-self-correction
  - id: transcend-lsp-bridge
    content: LSP publishDiagnostics→Swarm Channel事件桥接
    status: completed
    dependencies:
      - transcend-swarm-strategy
  - id: transcend-conflict-detect
    content: 基于LSP符号依赖图的Swarm任务冲突检测
    status: completed
    dependencies:
      - transcend-lsp-bridge
---

## 产品概述

CarpAI 三阶段架构升级：将6个互不连接的独立模块打通为端到端流水线，将占位/空壳实现替换为真实代码，并在连通补实基础上实现Swarm+LSP融合这一Cursor不具备的能力。

## 核心功能

### 阶段一：连通（打通断层）

- 建立LSP诊断 → 跨文件修复引擎的实时诊断管道（替换TypeChecker空桩）
- 实现AstEdit → FileSet类型适配器（打通跨文件修复→多文件原子编辑的数据流）
- 将4个孤岛crate接入主二进制依赖（jcode-cross-file-repair、jcode-multi-file-edit、jcode-skills、jcode-mcp-advanced）
- 修复StdioTransport的unimplemented!()致命桩
- WS层4个LSP处理器从模拟数据切换到真实jcode-lsp调用

### 阶段二：补实（替换空壳）

- TreeSitterAstAdapter non-Rust语言从正则降级升级为真实tree-sitter解析（TypeScript/Python/Go）
- TypeChecker::check_with_lsp()注入jcode-lsp诊断能力，支持非Rust语言
- sub_agents.rs execute_task_real()从格式化字符串空壳替换为实际Provider调用
- jcode-cross-file-repair的自修正循环连接到真实LLM修复建议生成

### 阶段三：超越（Swarm+LSP融合）

- 实现SwarmTurnStrategy（TurnStrategy trait的新实现），在swarm agent turn中自动注入LSP诊断上下文
- LSP publishDiagnostics → Swarm Channel事件桥接，让swarm内所有成员感知编译错误
- 基于LSP符号依赖图的冲突检测，避免多个swarm agent同时修改同一符号
- Sub-Agent执行引擎连接Provider+LSP上下文注入

## 技术栈

- 语言：Rust (edition 2024, workspace统一)
- 异步运行时：tokio
- LSP协议：lsp-types + JSON-RPC over stdio
- AST解析：tree-sitter 0.24 + tree-sitter-rust 0.23（需新增tree-sitter-typescript/tree-sitter-python/tree-sitter-go）
- Agent框架：现有TurnStrategy trait + Agent结构体
- Swarm框架：jcode-swarm-core（ChannelIndex双向索引 + SwarmMemberRecord）
- 构建系统：Cargo workspace

## 实现方案

### 阶段一：连通 — 依赖链打通

**1. 主二进制接入4个孤岛crate**

在根`Cargo.toml`的`[dependencies]`中添加：

- `jcode-cross-file-repair = { path = "crates/jcode-cross-file-repair" }`
- `jcode-multi-file-edit = { path = "crates/jcode-multi-file-edit" }`
- `jcode-skills = { path = "crates/jcode-skills" }`
- `jcode-mcp-advanced = { path = "crates/jcode-mcp-advanced" }`

这4个crate当前编译但未被链接，添加依赖后主二进制可以使用其API。

**2. AstEdit → FileSet 适配器**

核心问题：`jcode-cross-file-repair`输出`Vec<AstEdit>`，`jcode-multi-file-edit`输入`Vec<FileSet>`，类型不兼容。

方案：在`jcode-cross-file-repair`中新增`bridge.rs`模块，实现`AstEdit → FileOperation → FileSet`的转换函数：

- `AstEditOp::ReplaceFunction` → `FileEditOp::Replace { start_line, end_line, new_content }`
- `AstEditOp::AddImport` → `FileEditOp::Insert { line: 1, content }`
- `AstEditOp::RemoveImport` → `FileEditOp::Delete { start_line, end_line }`
- `AstEditOp::ChangeType` → `FileEditOp::Replace`
- `AstEditOp::RenameSymbol` → `FileEditOp::Replace`

在`jcode-cross-file-repair/Cargo.toml`中添加`jcode-multi-file-edit`依赖。

**3. StdioTransport修复**

`crates/jcode-mcp-advanced/src/transport.rs`的`new()`中`write_tx`和`read_rx`使用了`unimplemented!()`。

方案：改为`Option`延迟初始化模式：

- 字段类型改为`Arc<tokio::sync::Mutex<Option<...>>>`
- `new()`中填入`None`
- `connect()`中spawn子进程后设置为`Some(tx)`/`Some(rx)`
- 使用时通过`as_ref().expect("transport not connected")`确保已连接

**4. WS层LSP处理器接入真实jcode-lsp**

`src/ws/handlers/lsp.rs`中4个函数全部返回模拟数据。需要注入`LspServerManager`实例。

方案：

- 在`SessionManager`或WebSocket app state中持有`Arc<LspServerManager>`
- `handle_completion()` → 调用`lsp_manager.get_completion(file, line, character)`
- `handle_definition()` → 调用`lsp_manager.goto_definition(file, line, character)`
- `handle_references()` → 调用`lsp_manager.find_references(file, line, character)`
- `handle_diagnostics()` → 调用`lsp_manager.get_diagnostics(file)`

### 阶段二：补实 — 空壳替换

**5. TypeChecker::check_with_lsp()注入真实诊断**

当前`check_with_lsp()`永远返回`Ok(Vec::new())`。

方案：让`TypeChecker`持有可选的`Arc<LspServerManager>`引用：

```rust
pub struct TypeChecker {
    lsp_manager: Option<Arc<LspServerManager>>,
}
impl TypeChecker {
    pub fn with_lsp(manager: Arc<LspServerManager>) -> Self { ... }
    pub async fn check_with_lsp(&self, file: &str) -> Result<Vec<TypeError>> {
        if let Some(lsp) = &self.lsp_manager {
            let diags = lsp.get_diagnostics(file).await?;
            Ok(diags.into_iter().map(|d| TypeError::from_lsp_diagnostic(d, file)).collect())
        } else { Ok(Vec::new()) }
    }
}
```

在`jcode-cross-file-repair/Cargo.toml`中添加`jcode-lsp`为可选依赖（feature门控）。

**6. TreeSitterAstAdapter多语言升级**

当前non-Rust语言降级为正则匹配。需要为TypeScript/Python/Go添加真实tree-sitter解析。

方案：

- 添加`tree-sitter-typescript`、`tree-sitter-python`、`tree-sitter-go`依赖（feature门控）
- 在`LanguageKind::TypeScript/Python/Go`分支中调用对应parser
- `find_dependents()`从`line.contains(symbol)`升级为AST级import分析

**7. sub_agents.rs执行引擎**

`execute_task_real()`当前仅格式化输出字符串，未调用任何LLM。

方案：将`SubAgentTask`扩展，持有`Arc<dyn Provider>`引用（或通过回调），使`execute_task_real()`实际发送消息到Provider并收集响应：

- `SubAgentTask`新增`provider: Option<Arc<dyn Provider>>`字段
- `execute_task_real()`检查是否有provider，有则发送instruction+context作为用户消息，收集响应
- 无provider时回退到当前格式化行为（向后兼容）

**8. 自修正循环连接LLM**

`SelfCorrectionLoop::run()`当前的自修正逻辑是基于规则的简单修复。需要连接到LLM获取修复建议。

方案：在`SelfCorrectionLoop`中注入可选的`AiFixProvider` trait：

```rust
pub trait AiFixProvider: Send + Sync {
    async fn suggest_fix(&self, request: &AiFixRequest) -> Option<Fix>;
}
```

默认实现使用基于规则的修复（现有逻辑），可选注入LLM provider获取AI修复建议。

### 阶段三：超越 — Swarm+LSP融合

**9. SwarmTurnStrategy实现**

在`src/agent/turn_strategy.rs`中新增`SwarmTurnStrategy`，实现`TurnStrategy` trait：

核心行为（覆盖9个阶段中的关键几个）：

- `build_prompt()` — 在系统提示中注入当前文件的LSP诊断信息（编译错误、未使用变量等）
- `inject_memory()` — 注入swarm channel中其他agent的完成报告和LSP诊断事件
- `repair()` — 检查LSP诊断，如果有编译错误则优先修复
- `build_memory()` — 收集swarm channel中的最新事件

数据结构：

```rust
pub struct SwarmTurnStrategy {
    swarm_channel: Arc<ChannelIndex>,
    lsp_manager: Arc<LspServerManager>,
    working_files: Arc<RwLock<Vec<String>>>,
}
```

**10. LSP → Swarm Channel事件桥接**

实现`LspEventBridge`，将LSP的`publishDiagnostics`事件转发到swarm channel：

```rust
pub struct LspEventBridge {
    lsp_manager: Arc<LspServerManager>,
    swarm_channel: Arc<ChannelIndex>,
}
impl LspEventBridge {
    pub async fn start(&self) {
        // 订阅LSP诊断事件
        // 转换为SwarmMessage::LspDiagnostics
        // 广播到swarm channel
    }
}
```

**11. 符号级冲突检测**

利用LSP的`textDocument/references`和`textDocument/definition`构建符号依赖图，在swarm任务分配时检测写冲突：

```rust
pub struct SymbolConflictDetector {
    lsp_manager: Arc<LspServerManager>,
}
impl SymbolConflictDetector {
    pub async fn detect_conflicts(&self, tasks: &[SubAgentTask]) -> Vec<ConflictReport> {
        // 对每个任务，提取其修改的符号集合
        // 利用LSP references查找每个符号的依赖
        // 检测多个任务修改同一符号或互相依赖的符号
    }
}
```

在`ParallelTaskScheduler`调度前调用冲突检测，将冲突任务改为串行执行。

## 实现备注

- **编译验证**：每阶段完成后执行`cargo check`验证，避免累积错误
- **Feature门控**：jcode-lsp→jcode-cross-file-repair的依赖用feature门控（`lsp-bridge`），不强制所有消费者引入LSP
- **向后兼容**：SwarmTurnStrategy是新增实现，不影响现有StandardTurnStrategy
- **性能**：LSP诊断注入为异步操作，不阻塞agent turn主路径；符号冲突检测在任务分配时一次性执行
- **资源管理**：共享`LspServerManager`实例避免重复启动语言服务器；StdioTransport修复后MCP stdio传输可用

## 目录结构

```
CarpAI/
├── Cargo.toml                                          # [MODIFY] 添加4个孤岛crate依赖
├── src/
│   ├── agent/
│   │   ├── turn_strategy.rs                            # [MODIFY] 新增SwarmTurnStrategy实现
│   │   └── turn_loops.rs                                # [MODIFY] 支持TurnStrategy动态分发
│   ├── sub_agents.rs                                    # [MODIFY] execute_task_real()接入Provider
│   ├── ws/
│   │   ├── handlers/
│   │   │   └── lsp.rs                                  # [MODIFY] 4个处理器接入jcode-lsp
│   │   └── session.rs                                   # [MODIFY] SessionManager持有LspServerManager
│   ├── server/
│   │   └── swarm.rs                                     # [MODIFY] 集成LspEventBridge
│   └── refactor_engine.rs                               # [MODIFY] 接入jcode-cross-file-repair
├── crates/
│   ├── jcode-cross-file-repair/
│   │   ├── Cargo.toml                                   # [MODIFY] 添加jcode-lsp(feature门控)+jcode-multi-file-edit依赖
│   │   ├── src/
│   │   │   ├── lib.rs                                   # [MODIFY] 导出bridge模块
│   │   │   ├── type_checker.rs                          # [MODIFY] check_with_lsp()注入真实LSP诊断
│   │   │   ├── ast.rs                                   # [MODIFY] TreeSitterAstAdapter多语言升级
│   │   │   ├── bridge.rs                                # [NEW] AstEdit → FileSet类型适配器
│   │   │   └── self_correction.rs                        # [MODIFY] 可选AiFixProvider注入
│   ├── jcode-multi-file-edit/
│   │   └── (无改动，作为下游消费者)
│   ├── jcode-mcp-advanced/
│   │   └── src/transport.rs                             # [MODIFY] StdioTransport unimplemented→Option
│   ├── jcode-lsp/
│   │   └── (无Cargo.toml改动，新增tree-sitter-* feature门控)
│   └── jcode-swarm-core/
│       └── (无改动，Swarm融合在应用层实现)
```

## Agent Extensions

### SubAgent

- **code-explorer**: 用于在实现各桥接模块时搜索所有引用旧类型/API的文件并批量更新依赖关系和use语句；用于验证类型适配器的完整性