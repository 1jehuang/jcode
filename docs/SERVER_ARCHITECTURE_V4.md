# CarpAI 真正的服务端架构 — 重构计划 v4.0

> **版本**: v4.0 (FINAL — 取消双轨运行, 一次性搬迁, Qwen3.x 本地推理)
> **日期**: 2026-05-25
> **基于**: THREE_TEAM_REFACTOR_PLAN_V3_FINAL.md 修订
> **状态**: ✅ 编译已通过 (0 error), 可立即启动搬迁

---

## 一、核心决策变更（v3 → v4）

### 1.1 取消双轨运行（最大变更）

**v3 方案（已废弃）:**
```
Week 3-6: 双轨运行（新 crate 被调用但 src/ 不动）← 中间态
Week 7-10: 按 Batch 从 src/ 搬迁到对应 crate
```

**问题：**
- 双轨运行期间，开发团队需要在 `src/` 和 `crates/` 两处维护相同功能
- 时间分配碎片化，每个模块要写两遍（src/ 兼容层 + crates/ 正式实现）
- 增加联调复杂度：无法确定 bug 出自 src/ 还是 crates/
- 团队认知负担翻倍

**v4 方案（一次性搬迁）:**
```
Week 3-8: 按 Batch 直接从 src/ 搬迁到对应 crate，每批搬迁后立即删除 src/ 中的源文件
Week 9-10: 集成测试 + E2E 全链路验证
Week 11-12: 性能基准 + 部署文档 + 安全审计
```

**搬迁原则：**
1. **每个 Batch 是原子操作**：搬迁 → 编译通过 → 删除 src/ 源 → 更新 lib.rs → 提交
2. **不写兼容层/shim**：直接移动代码，调整 import 路径
3. **按依赖拓扑排序**：先搬无依赖的底层模块，再搬上层
4. **每批完成后 `cargo check --workspace` 必须通过**

### 1.2 CLI 本地推理策略（Qwen3.x 80/20）

**个人开发者使用场景：**

| 场景 | 推理位置 | 占比 | 说明 |
|------|---------|------|------|
| 代码补全 / 简单问答 / 重构建议 | **本地 Qwen3.x** | ~80% | 低延迟 (<500ms), 离线可用 |
| 复杂 Agent 循环 (多轮 tool call) | **本地 Qwen3.x** | 部分 | 取决于模型能力上限 |
| 超长上下文 (>128K tokens) / 多文件理解 | **Server 端** | ~20% | 需要 Cloud API (Claude/GPT) |
| 企业功能 (RBAC/审计/多租户) | **Server 端** | 100% | 仅 Server 模式 |

**架构示意：**
```
┌─────────────────────────────────────────────────────┐
│                  carpai-cli (TUI)                    │
│                                                     │
│  ┌──────────────┐    ┌──────────────────────────┐   │
│  │   AgentBridge │───▶│     carpai-core          │   │
│  │  (零业务逻辑) │    │                          │   │
│  └──────────────┘    │  ┌────────────────────┐  │   │
│                      │  │ InferenceRouter     │  │   │
│                      │  │  ├─ Qwen3xLocal    │─┼───┼──▶ 80% 本地推理
│                      │  │  │  (llama.cpp)    │  │   │      离线/低延迟
│                      │  │  ├─ RemoteFallback │─┼───┼──▶ 20% 送 Server
│                      │  │  │  (gRPC→Server)  │  │   │      Cloud API
│                      │  │  └─ HybridSelector │  │   │
│                      │  └────────────────────┘  │   │
│                      └──────────────────────────┘   │
└─────────────────────────────────────────────────────┘
                         │
                    gRPC/REST (可选)
                         ▼
┌─────────────────────────────────────────────────────┐
│               carpai-server (企业端)                 │
│                                                     │
│  ┌──────────┐  ┌──────────┐  ┌──────────────────┐  │
│  │ gRPC     │  │ REST     │  │  Provider Pool    │  │
│  │ Server   │  │ (OpenAI  │  │  ├─ Claude        │  │
│  │          │  │  兼容)   │  │  ├─ GPT-4o        │  │
│  └──────────┘  └──────────┘  │  ├─ Gemini        │  │
│                               │  └─ DeepSeek     │  │
│  ┌──────────┐  ┌──────────┐  └──────────────────┘  │
│  │ Auth     │  │ Enterprise│                         │
│  │ JWT/RBAC │  │ Multi-tenant                        │
│  └──────────┘  └──────────┘                         │
└─────────────────────────────────────────────────────┘
```

### 1.3 Qwen3.x 本地推理集成方案

```rust
// crates/carpai-core/src/inference_impl.rs

/// 本地推理路由器 — 80/20 策略的核心
pub struct InferenceRouter {
    /// 本地 Qwen3.x 引擎 (llama.cpp backend)
    local: Option<QwenLocalEngine>,
    /// 远程 Server fallback (gRPC client)
    remote: Option<RemoteInferenceClient>,
    /// 路由策略
    strategy: RoutingStrategy,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct RoutingStrategy {
    /// 启用本地推理
    pub local_enabled: bool,
    /// 本地模型路径 (GGUF 文件)
    pub local_model_path: Option<PathBuf>,
    /// 本地推理最大上下文窗口 (tokens)
    pub local_max_context: usize,
    /// 复杂度阈值: 超过此 token 数自动路由到远程
    pub remote_fallback_threshold: usize,
    /// 远程 Server URL
    pub remote_url: Option<String>,
}

impl InferenceBackend for InferenceRouter {
    async fn complete(
        &self,
        request: CompletionRequest,
    ) -> Result<CompletionResponse> {
        // 80/20 路由决策
        if self.should_route_locally(&request) {
            self.local_complete(request).await
        } else {
            self.remote_complete(request).await
        }
    }
    
    async fn stream_complete(
        &self,
        request: CompletionRequest,
    ) -> Result<Box<dyn Stream<Item = Result<StreamChunk>> + Send + Unpin>> {
        // 同样的路由逻辑
        if self.should_route_locally(&request) {
            self.local_stream(request).await
        } else {
            self.remote_stream(request).await
        }
    }
}

impl InferenceRouter {
    fn should_route_locally(&self, req: &CompletionRequest) -> bool {
        // 条件 1: 本地引擎可用
        let local_ok = self.local.is_some() && self.strategy.local_enabled;
        
        // 条件 2: 请求在本地能力范围内
        let within_capacity = req.messages.total_tokens()
            <= self.strategy.local_max_context;
        
        // 条件 3: 未超过复杂度阈值
        let not_too_complex = req.messages.len()
            < self.strategy.remote_fallback_threshold;
        
        local_ok && within_capacity && not_too_complex
    }
}
```

---

## 二、一次性搬迁执行计划（Week 3-8）

### 2.1 搬迁批次（按依赖拓扑排序）

```
Batch 0 (已完成): carpai-internal trait 层 + carpai-core Local 实现
═════════════════════════════════════════════════════════

Batch 1 (Week 3, Days 1-2): 配置 + 基础设施 (~5 模块)
  优先级: 🔴 最高 (所有其他 Batch 依赖此批)
  
  src/config.rs          → crates/carpai-core/src/config.rs (已有骨架, 补全)
  src/core/              → crates/carpai-core/src/platform/
  src/utils/mod.rs       → crates/carpai-core/src/utils.rs
  src/id.rs              → crates/carpai-core/src/id.rs
  src/safety.rs          → crates/carpai-core/src/safety.rs
  
  验证: cargo check -p carpai-core 通过
  提交: "refactor(batch1): 搬迁 config+core+utils+id+safety to carpai-core"

Batch 2 (Week 3, Days 3-4): 错误处理 + 性能 (~15 模块)
  优先级: 🟠 高 (大部分模块依赖错误类型)
  
  src/error_recovery.rs   → crates/carpai-core/src/error/recovery.rs
  src/error_types.rs      → crates/carpai-core/src/error/types.rs
  src/network_retry.rs    → crates/carpai-core/src/error/network.rs
  src/allowlist.rs        → crates/carpai-core/src/error/allowlist.rs
  src/perf.rs             → crates/carpai-core/src/perf/mod.rs
  src/cache_tracker.rs    → crates/carpai-core/src/perf/cache.rs
  src/cache_optimizer.rs  → crates/carpai-core/src/perf/cache_optimizer.rs
  src/cache_integration.rs→ crates/carpai-core/src/perf/cache_integration.rs
  src/cache_break_detector.rs → crates/carpai-core/src/perf/break_detector.rs
  src/concurrency_optimizer.rs → ...
  src/compression.rs      → ...
  src/circuit_breaker.rs  → ...
  src/backpressure.rs     → ...
  src/token_budget.rs     → ...
  src/denial_tracking.rs  → ...
  
  验证: cargo check -p carpai-core 通过
  提交: "refactor(batch2): 搬迁 error+performance modules to carpai-core"

Batch 3 (Week 4, Days 1-2): 文件操作 + Git (~10 模组)
  
  src/storage.rs          → crates/carpai-core/src/storage/mod.rs
  src/file_refs.rs        → crates/carpai-core/src/file_refs.rs
  src/file_state_cache.rs → crates/carpai-core/src/file_state.rs
  src/file_history.rs     → crates/carpai-core/src/file_history.rs
  src/checkpoint.rs       → crates/carpai-core/src/checkpoint.rs
  src/undo_redo.rs        → crates/carpai-core/src/undo.rs
  src/undo_manager.rs     → (合并入 undo.rs)
  src/git.rs              → crates/carpai-core/src/git/mod.rs
  src/git_workflow.rs     → crates/carpai-core/src/git/workflow.rs
  src/version_manager.rs  → crates/carpai-core/src/git/version.rs
  
  验证: cargo check -p carpai-core 通过
  提交: "refactor(batch3): 搬迁 storage+git modules to carpai-core"

Batch 4 (Week 4, Days 3-4): AST + 语义分析 (~8 模块)
  
  src/ast.rs              → crates/carpai-core/src/analysis/ast.rs
  src/classifier.rs       → crates/carpai-core/src/analysis/classifier.rs
  src/semantic.rs         → crates/carpai-core/src/analysis/semantic.rs
  src/context_pruner.rs   → crates/carpai-core/src/analysis/pruner.rs
  src/incremental_index.rs→ crates/carpai-core/src/analysis/index.rs
  src/proactive_context.rs→ crates/carpai-core/src/analysis/proactive.rs
  src/context.rs           → crates/carpai-core/src/analysis/context.rs
  src/reasoning.rs        → crates/carpai-core/src/analysis/reasoning.rs
  
  验证: cargo check -p carpai-core 通过
  提交: "refactor(batch4): 搬迁 ast+analysis modules to carpai-core"

Batch 5 (Week 5, Days 1-3): Agent 系统 (~12 模块) ⚠️ 最大批次
  优先级: 🔴 核心 (Agent 运行时是系统心脏)
  
  src/agent.rs            → crates/carpai-core/src/agent/mod.rs
  src/agent_runtime.rs    → crates/carpai-core/src/agent/runtime.rs
  src/sub_agents.rs       → crates/carpai-core/src/agent/sub_agents.rs
  src/skill_system.rs     → crates/carpai-core/src/agent/skills.rs
  src/plan_mode.rs        → crates/carpai-core/src/agent/plan_mode.rs
  src/task_planner.rs     → crates/carpai-core/src/agent/planner.rs
  src/task_manager.rs     → crates/carpai-core/src/agent/manager.rs
  src/task_decomposer.rs  → crates/carpai-core/src/agent/decomposer.rs
  src/task_scheduler.rs   → crates/carpai-core/src/agent/scheduler.rs
  src/plan_verifier.rs    → crates/carpai-core/src/agent/verifier.rs
  src/ultraplan.rs        → crates/carpai-core/src/agent/ultraplan.rs
  src/response_recovery.rs→ crates/carpai-core/src/agent/recovery.rs
  
  ⚠️ agent_runtime.rs 是上帝模块(711行), 搬迁时同步拆分:
    - agent/runtime/session_mgmt.rs  (会话管理逻辑)
    - agent/runtime/tool_dispatch.rs  (工具分发逻辑)
    - agent/runtime/inference_loop.rs  (推理循环逻辑)
  
  验证: cargo check -p carpai-core 通过
  提交: "refactor(batch5): 搬迁 agent system to carpai-core (+runtime split)"

Batch 6 (Week 5, Day 4 - Week 6, Day 1): 记忆系统 (~13 模块)
  
  src/memory.rs           → crates/carpai-core/src/memory/mod.rs
  src/memory_agent.rs     → crates/carpai-core/src/memory/agent.rs
  src/memory_graph.rs     → crates/carpai-core/src/memory/graph.rs
  src/memory_log.rs       → crates/carpai-core/src/memory/log.rs
  src/memory_types.rs     → crates/carpai-core/src/memory/types.rs
  src/memory_prompt.rs    → crates/carpai-core/src/memory/prompt.rs
  src/memory_advanced.rs  → crates/carpai-core/src/memory/advanced.rs
  src/semantic_memory.rs  → crates/carpai-core/src/memory/semantic.rs
  src/hierarchical_memory.rs → crates/carpai-core/src/memory/hierarchical.rs
  src/knowledge_graph.rs  → crates/carpai-core/src/memory/knowledge_graph.rs
  src/knowledge.rs        → crates/carpai-core/src/memory/knowledge.rs
  src/knowledge_agents.rs → crates/carpai-core/src/memory/knowledge_agents.rs
  src/protocol_memory.rs  → crates/carpai-core/src/memory/protocol.rs
  
  验证: cargo check -p carpai-core 通过
  提交: "refactor(batch6): 搬迁 memory system to carpai-core"

Batch 7 (Week 6, Days 2-3): 工具 + 补全 + 会话 (~14 模块)
  
  工具:
  src/tool.rs             → crates/carpai-core/src/tools/mod.rs
  src/tool/bash.rs        → crates/carpai-core/src/tools/bash.rs
  src/tool/batch.rs       → crates/carpai-core/src/tools/batch.rs
  src/tool/read.rs        → crates/carpai-core/src/tools/read.rs
  src/tool/open.rs        → crates/carpai-core/src/tools/open.rs
  src/tool/conversation_search.rs → crates/carpai-core/src/tools/search.rs
  src/mcp.rs              → crates/carpai-core/src/tools/mcp.rs
  src/slash_command.rs    → crates/carpai-core/src/tools/slash.rs
  
  补全:
  src/completion.rs       → crates/carpai-core/src/completion/mod.rs
  src/completion_engine.rs→ crates/carpai-core/src/completion/engine.rs
  src/completion_quality.rs → crates/carpai-core/src/completion/quality.rs
  src/auto_fallback.rs    → crates/carpai-core/src/completion/fallback.rs
  
  会话:
  src/session.rs          → crates/carpai-core/src/session/mod.rs
  src/session_export.rs   → crates/carpai-core/src/session/export.rs
  src/session_cost_tracker.rs → crates/carpai-core/src/session/cost.rs
  src/session_gc.rs       → crates/carpai-core/src/session/gc.rs
  src/runtime_manager.rs  → crates/carpai-core/src/session/runtime.rs
  
  验证: cargo check -p carpai-core 通过
  提交: "refactor(batch7): 搬迁 tools+completion+session to carpai-core"

Batch 8 (Week 6, Day 4 - Week 7, Day 1): 重构引擎 (~14 模块)
  
  src/refactor.rs                → crates/carpai-core/src/refactor/mod.rs
  src/refactor_engine.rs         → crates/carpai-core/src/refactor/engine.rs
  src/orchestrator.rs            → crates/carpai-core/src/refactor/orchestrator.rs
  src/precise_edit.rs            → crates/carpai-core/src/refactor/edit.rs
  src/atomic_edit_coordinator.rs → crates/carpai-core/src/refactor/coordinator.rs
  src/diff_engine.rs             → crates/carpai-core/src/refactor/diff.rs
  src/diff_integration.rs        → crates/carpai-core/src/refactor/diff_integ.rs
  src/streaming_diff_preview.rs  → crates/carpai-core/src/refactor/stream_preview.rs
  src/compilation_engine.rs      → crates/carpai-core/src/refactor/compiler.rs
  src/diagnostics.rs             → crates/carpai-core/src/refactor/diagnostics.rs
  src/transaction.rs             → crates/carpai-core/src/refactor/transaction.rs
  src/refactor_verify_pipeline.rs → crates/carpai-core/src/refactor/verify.rs
  src/delivery_pipeline.rs       → crates/carpai-core/src/refactor/delivery.rs
  
  验证: cargo check -p carpai-core 通过
  提交: "refactor(batch8): 搬迁 refactoring engine to carpai-core"

Batch 9 (Week 7, Days 2-3): Provider + 推理 + Embedding (~8 模块)
  
  src/provider/           → crates/carpai-core/src/provider/
  src/embedding.rs        → crates/carpai-core/src/embedding.rs
  src/inference_optimizer.rs → crates/carpai-core/src/inference/optimizer.rs
  src/inference_integration.rs → crates/carpai-core/src/inference/integration.rs
  src/auto_mode.rs        → crates/carpai-core/src/inference/auto_mode.rs
  src/rest_llm.rs         → crates/carpai-core/src/provider/rest_llm.rs
  src/gateway.rs          → crates/carpai-core/src/gateway.rs
  src/provider_catalog.rs → crates/carpai-core/src/provider/catalog.rs
  
  ⚠️ 此批包含 Qwen3.x 本地推理集成:
  新建 src/inference/qwen_local.rs → InferenceRouter + QwenLocalEngine
  
  验证: cargo check -p carpai-core 通过
  提交: "refactor(batch9): 搬迁 provider+inference (+Qwen3.x local)"

Batch 10 (Week 7, Day 4 - Week 8, Day 2): Server 专属模块 (~20 模块)
  → 搬迁到 carpai-server (非 carpai-core!)
  
  src/api/                → crates/carpai-server/src/api/
  src/grpc/               → crates/carpai-server/src/grpc/
  src/rest/               → crates/carpai-server/src/rest/
  src/ws/                 → crates/carpai-server/src/ws/
  src/auth/               → crates/carpai-server/src/auth/
  src/security/           → crates/carpai-server/src/security/
  src/server/             → crates/carpai-server/src/server.rs
  src/observability/      → crates/carpai-server/src/observability/
  src/metrics.rs          → crates/carpai-server/src/metrics.rs
  src/telemetry.rs        → crates/carpai-server/src/telemetry.rs
  src/prometheus.rs       → crates/carpai-server/src/prometheus.rs
  src/logging.rs          → crates/carpai-server/src/logging.rs
  src/audit_log.rs        → crates/carpai-server/src/audit_log.rs
  src/deny_log.rs         → crates/carpai-server/src/deny_log.rs
  src/transport/          → crates/carpai-server/src/transport/
  src/protocol/           → crates/carpai-server/src/protocol/
  src/bridge/             → crates/carpai-server/src/bridge.rs
  src/distributed/        → crates/carpai-server/src/distributed/
  src/enterprise/         → crates/carpai-server/src/enterprise/
  
  验证: cargo check -p carpai-server 通过
  提交: "refactor(batch10): 搬迁 server-specific modules to carpai-server"

Batch 11 (Week 8, Days 3-4): CLI 专属模块 (~15 模块)
  → 搬迁到 carpai-cli (非 carpai-core!)
  
  src/cli/                → crates/carpai-cli/src/cli/ (重写, 非搬运)
  src/tui/                → crates/carpai-cli/src/tui/ (重写, 基于 agent_bridge)
  src/terminal_launch.rs   → crates/carpai-cli/src/terminal_launch.rs
  src/stdin_detect.rs     → crates/carpai-cli/src/stdin_detect.rs
  src/setup_hints.rs      → crates/carpai-cli/src/setup_hints.rs
  src/dictation.rs        → crates/carpai-cli/src/dictation.rs
  src/browser.rs          → crates/carpai-cli/src/browser.rs
  src/ambient/            → crates/carpai-cli/src/ambient/
  src/ambient_runner.rs   → (合并入 ambient/)
  src/ambient_scheduler.rs→ (合并入 ambient/)
  src/overnight.rs        → crates/carpai-cli/src/overnight.rs
  src/catchup.rs          → crates/carpai-cli/src/catchup.rs
  src/notifications/      → crates/carpai-cli/src/notifications/
  src/telegram.rs         → crates/carpai-cli/src/notifications/telegram.rs
  src/gmail.rs            → crates/carpai-cli/src/notifications/gmail.rs
  src/browser_bridge.rs   → crates/carpai-cli/src/browser_bridge.rs
  src/copilot_usage.rs    → crates/carpai-cli/src/copilot_usage.rs
  src/dashboard.rs         → crates/carpai-cli/src/dashboard.rs
  src/debug_panel.rs      → crates/carpai-cli/src/debug_panel.rs
  src/side_panel.rs       → crates/carpai-cli/src/side_panel.rs
  src/buddy.rs            → crates/carpai-cli/src/buddy.rs
  src/voice.rs            → crates/carpai-cli/src/voice.rs
  src/vim.rs              → crates/carpai-cli/src/vim.rs
  src/login_qr.rs         → crates/carpai-cli/src/login_qr.rs
  src/startup_profile.rs  → crates/carpai-cli/src/startup_profile.rs
  src/todo.rs             → crates/carpai-cli/src/todo.rs
  src/render_optimizer.rs → crates/carpai-cli/src/render_optimizer.rs
  
  验证: cargo check -p carpai-cli 通过
  提交: "refactor(batch11): 搬迁 CLI-specific modules to carpai-cli"
```

### 2.2 死代码清理（与搬迁并行）

| 模块 | 处置 | 在哪个 Batch 清理 |
|------|------|------------------|
| crdt | 归档 `jcode-experimental/` | Batch 1 (直接删除 lib.rs 声明) |
| dap | 归档 `jcode-debug/` | Batch 1 |
| env | **删除** | Batch 1 |
| goal | **合并** task_planner | Batch 5 |
| import | **删除** | Batch 1 |
| login_qr | **删除** (→Paw-brave 处理) | Batch 1 |
| process_memory | **删除** | Batch 2 |
| process_title | **删除** | Batch 1 |
| prompt | **合并** memory/prompt.rs | Batch 6 |
| restart_snapshot | **删除** | Batch 7 |
| runtime_memory_log | **删除** | Batch 2 |
| safety | **合并** security/scanner.rs | Batch 10 (→server) |
| scheduler | **删除** | Batch 5 |
| external | **删除** | Batch 1 |
| plan | **合并** ultraplan | Batch 5 |
| workspace_manager | **合并** session/workspace.rs | Batch 7 |
| compaction | **合并** memory/compaction.rs | Batch 6 |
| subscription_catalog | **删除** | Batch 1 |
| todo | **删除** (→Paw-brave) | Batch 1 |
| update | **删除** (已被替换) | Batch 1 |
| usage | **删除** | Batch 1 |
| video_export | **删除** (→Paw-brave 或归档) | Batch 1 |
| p2_integration | 保留但标记 experimental | Batch 1 |
| protocol_memory | 合并 memory/ | Batch 6 |
| soft_interrupt_store | 合并 core/ | Batch 1 |
| memdir | 删除 | Batch 1 |
| nlp | 归档 | Batch 1 |
| prototype | 删除 | Batch 1 |
| retrieval | 合并 memory/ | Batch 6 |
| mab | 归档 | Batch 1 |
| tdd | 归档 | Batch 1 |
| performance_advanced | 合并 perf/ | Batch 2 |
| i18n | 合并 cli/ | Batch 11 |
| message | 合并 session/ | Batch 7 |
| channel | 删除 | Batch 1 |
| bus | 删除 (被 EventBus 替代) | Batch 1 |
| plugins | 归档 | Batch 1 |
| plugin_market | 归档 | Batch 1 |
| marketplace | 归档 | Batch 1 |
| build | 删除 | Batch 1 |
| build_module | 删除 | Batch 1 |
| ci | 删除 | Batch 1 |
| sandbox | 合并 tools/ | Batch 7 |
| hooks_system | 归档 | Batch 1 |
| ai_optimization | 合并 inference/ | Batch 9 |
| ab_testing | 归档 | Batch 1 |
| ai_enhanced | 合并 inference/ | Batch 9 |
| codereview | 合并 refactor/ | Batch 8 |
| workflow | 合并 agent/ | Batch 5 |
| ssh | 归档 | Batch 1 |
| registry | 合并 provider/ | Batch 9 |
| skill | 合并 agent/skills | Batch 5 |
| skills | 合并 agent/skills | Batch 5 |

### 2.3 每个 Batch 的标准作业流程

```bash
# 1. 创建目标目录结构
mkdir -p crates/carpai-core/src/<module_group>/

# 2. 移动文件 (git mv)
git mv src/<module>.rs crates/carpai-core/src/<module_group>/<module>.rs

# 3. 调整模块声明 (mod xxx → mod xxx; use xxx::...)
#    - 更新 crate 内部 import 路径
#    - crate::xxx → super::xxx 或绝对路径
#    - 移除 #[cfg(feature = "...")] 如果不再需要

# 4. 更新 src/lib.rs
#    - 删除: pub mod xxx;
#    - 如有 re-export: pub use carpai_core::xxx;

# 5. 编译验证
cargo check -p carpai-core      # 目标 crate
cargo check --workspace         # 全量

# 6. 提交
git add -A
git commit -m "refactor(batchN): move <modules> from src/ to carpai-core"
```

---

## 三、更新后的时间线

### 3.1 v4 完整时间线（12 周）

```
══════════════════════════════════════════════════════════════════
Phase 0: 基础设施 (Week 1-2) ✅ 已完成
══════════════════════════════════════════════════════════════════
  [✅] carpai-internal trait 层 (7 traits + AgentContext + AppConfig)
  [✅] carpai-core Local 实现 (6 个 impls + agent_loop + CoreConfig)
  [✅] carpai-cli 骨架 (Cargo.toml + main.rs + TUI skeleton)
  [✅] carpai-server 骨架 (Cargo.toml + main.rs + app.rs + config.rs)
  [✅] UTF-8 编码修复 (6 个文件, 60+ 处损坏字符)
  [✅] cargo check 根 crate 编译通过 (0 error)

══════════════════════════════════════════════════════════════════
Phase 1: 一次性搬迁 (Week 3-8) ← 当前阶段
══════════════════════════════════════════════════════════════════
  Week 3:
    Days 1-2:  Batch 1 (config+core+utils) + Batch 0 死代码清理
    Days 3-4:  Batch 2 (error+performance, ~15 模块)

  Week 4:
    Days 1-2:  Batch 3 (storage+git, ~10 模块)
    Days 3-4:  Batch 4 (ast+analysis, ~8 模块)

  Week 5: ⚠️ 最关键周
    Days 1-3:  Batch 5 (agent 系统, ~12 模块) ← 上帝模块拆分
    Day  4:    Batch 6 开始 (memory, ~13 模块)

  Week 6:
    Day  1:    Batch 6 完成
    Days 2-3:  Batch 7 (tools+completion+session, ~14 模块)
    Day  4:    Batch 8 开始 (refactoring, ~14 模块)

  Week 7:
    Days 1-2:  Batch 8 完成
    Days 2-3:  Batch 9 (provider+inference+Qwen3.x, ~8 模块)
    Day  4:    Batch 10 开始 (server 专属, ~20 模块)

  Week 8:
    Days 1-2:  Batch 10 完成
    Days 3-4:  Batch 11 (CLI 专属, ~15 模块)

  🎯 Week 8 结束标志:
    ✓ src/ 仅剩 lib.rs (re-export 层) + main.rs + bin/
    ✓ carpai-core 包含全部业务逻辑 (~120 模块)
    ✓ carpai-server 包含全部服务端代码 (~20 模块)
    ✓ carpai-cli 包含全部客户端代码 (~15 模块)
    ✓ cargo check --workspace 0 error

══════════════════════════════════════════════════════════════════
Phase 2: 集成 + 验证 (Week 9-10)
══════════════════════════════════════════════════════════════════
  Week 9:
    Days 1-2:  SDK 增强 (OpenAI 兼容 API + Session CRUD)
    Days 3-4:  E2E 测试链 1: CLI Local Mode (TUI → Qwen3.x → reply)
    Day  5:    E2E 测试链 2: Server Standalone (health → gRPC → REST)

  Week 10:
    Days 1-2:  E2E 测试链 3: CLI Remote Mode (CLI → gRPC → Server)
    Days 3-4:  E2E 测试链 4: SDK Basic Flow (connect → chat → receive)
    Day  5:    全量回归 + Bug bash

══════════════════════════════════════════════════════════════════
Phase 3: 生产就绪 (Week 11-12)
══════════════════════════════════════════════════════════════════
  Week 11:
    Days 1-3:  性能基准测试 (latency, throughput, memory)
    Days 4-5:  安全审计准备 (依赖扫描 + 权限检查)

  Week 12:
    Days 1-3:  部署文档 (Docker/K8s/systemd + 升级脚本)
    Days 4-5:  v1.0.0 release + changelog
```

### 3.2 与 v3 对比

| 维度 | v3 (双轨) | **v4 (一次性)** | 变化 |
|------|----------|---------------|------|
| 总工期 | 12 周 | **12 周** | 不变 |
| 双轨期 | Week 3-6 (4 周) | **取消 (0 周)** | **-4 周中间态** |
| 搬迁期 | Week 7-10 (4 周) | Week 3-8 (**6 周**) | +2 周 (更从容) |
| 集成测试 | Week 9-10 (2 周) | Week 9-10 (2 周) | 不变 |
| Qwen3.x 集成 | ❌ 未规划 | **Batch 9 内置** | **新增** |
| 开发团队认知负担 | 双倍 (src/ + crates/) | **单一 (仅 crates/)** | **减半** |
| Bug 定位复杂度 | 高 (不确定来源) | **低 (唯一来源)** | **大幅降低** |
| 回滚风险 | 中 (需维护两套) | **低 (git revert 即可)** | **降低** |

---

## 四、Qwen3.x 本地推理详细设计

### 4.1 架构位置

```
carpai-core/src/inference/
├── mod.rs                  # pub mod router, qwen_local, remote_fallback
├── router.rs               # InferenceRouter (80/20 路由)
├── qwen_local.rs           # QwenLocalEngine (llama.cpp 绑定)
├── remote_fallback.rs      # RemoteInferenceClient (gRPC → Server)
└── hybrid_selector.rs      # 复杂度评估 + 路由决策
```

### 4.2 QwenLocalEngine 设计

```rust
// crates/carpai-core/src/inference/qwen_local.rs

use jcode_cpu_inference::{ModelLifecycleManager, GracefulManager};
use carpai_internal::inference_backend::*;
use std::sync::Arc;

/// 本地 Qwen3.x 推理引擎
/// 
/// 基于 jcode-cpu-inference (llama.cpp Rust 绑定),
/// 支持 GGUF 格式量化模型 (Q4_K_M 推荐)。
pub struct QwenLocalEngine {
    model: Arc<ModelLifecycleManager>,
    manager: GracefulManager,
    config: QwenLocalConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QwenLocalConfig {
    /// GGUF 模型文件路径
    pub model_path: PathBuf,
    /// 上下文窗口大小 (tokens), 推荐: 8192-32768
    pub context_size: usize,
    /// GPU 层数 (-1 = CPU only)
    pub gpu_layers: i32,
    /// 并行 batch size
    pub batch_size: usize,
    /// 模型最大 token 数
    pub max_tokens: usize,
    /// 温度参数
    pub temperature: f32,
    /// Top-P 采样
    pub top_p: f32,
}

impl QwenLocalEngine {
    pub fn new(config: QwenLocalConfig) -> Result<Self> {
        let model = ModelLifecycleManager::new();
        let manager = GracefulManager::new(/* ... */);
        
        // 加载 GGUF 模型
        model.start_model(
            &config.model_path.display().to_string(),
            /* n_ctx */ config.context_size,
            /* n_gpu_layers */ config.gpu_layers,
        )?;
        
        Ok(Self { model, manager, config })
    }
    
    pub fn is_loaded(&self) -> bool {
        self.model.has_active_model()
    }
    
    /// 预热: 确保模型已加载到内存/GPU
    pub async fn warmup(&self) -> Result<()> {
        if !self.is_loaded() {
            anyhow::bail!("Qwen3.x model not loaded");
        }
        // 空推理预热
        self.complete(CompletionRequest {
            messages: vec![Message::user("hi")],
            max_tokens: 1,
            ..Default::default()
        }).await?;
        Ok(())
    }
}

#[async_trait]
impl InferenceBackend for QwenLocalEngine {
    async fn complete(&self, request: CompletionRequest) -> Result<CompletionResponse> {
        // 将通用请求格式转换为 llama.cpp 调用
        let prompt = self.format_messages(&request.messages)?;
        let result = self.model.generate(
            &prompt,
            /* max_tokens */ request.max_tokens.min(self.config.max_tokens),
            /* temperature */ request.temperature.unwrap_or(self.config.temperature),
            /* top_p */ request.top_p.unwrap_or(self.config.top_p),
        ).await?;
        
        Ok(CompletionResponse {
            content: result.text,
            finish_reason: result.finish_reason,
            usage: TokenUsage {
                prompt_tokens: result.prompt_tokens,
                completion_tokens: result.completion_tokens,
                total_tokens: result.total_tokens,
            },
            logprobs: None,
        })
    }
    
    async fn stream_complete(
        &self,
        request: CompletionRequest,
    ) -> Result<Box<dyn Stream<Item = Result<StreamChunk>> + Send + Unpin>> {
        // 流式推理...
        // (使用 tokio-stream 包装 llama.cpp streaming API)
        unimplemented!("streaming support for Qwen local - TODO")
    }
}
```

### 4.3 配置集成

```toml
# ~/.carpai/config.toml (CLI 个人开发者模式)

[mode]
mode = "cli"

[core.inference]
# 80/20 策略配置
local_enabled = true
local_model_path = "~/.carpai/models/qwen3.6-27b-q4_k_m.gguf"
local_max_context = 16384
remote_fallback_threshold = 8192   # 超过 8K tokens → 送 Server
remote_url = "https://api.your-company.com/v1"  # 可选

[qwen_local]
context_size = 16384
gpu_layers = -1  # 自动检测 GPU
max_tokens = 4096
temperature = 0.7
top_p = 0.9
```

### 4.4 模型下载与管理

```rust
// crates/carpai-core/src/inference/model_management.rs

use reqwest::Client;
use sha2::{Sha256, Digest};

pub struct ModelManager {
    data_dir: PathBuf,
    client: Client,
}

impl ModelManager {
    /// 下载预构建的 Qwen3.x GGUF 模型
    pub async fn download_qwen3x(
        &self,
        variant: &str,  // e.g., "qwen3.6-27b-q4_k_m"
        progress: impl Fn(u64, u64) + Send + 'static,
    ) -> Result<PathBuf> {
        let url = format!(
            "https://models.carpai.dev/local/{}.gguf",
            variant
        );
        let dest = self.data_dir.join("models").join(format!("{}.gguf", variant));
        
        // 断点续传下载 + SHA256 校验
        self.download_with_resume(&url, &dest, progress).await?;
        self.verify_checksum(&dest).await?;
        
        Ok(dest)
    }
    
    /// 列出已下载的本地模型
    pub fn list_models(&self) -> Vec<LocalModelInfo> {
        // 扫描 data_dir/models/*.gguf
        // 返回模型名、大小、修改时间
    }
    
    /// 检测系统是否有足够资源运行指定模型
    pub fn can_run_model(&self, variant: &str) -> ResourceCheck {
        // 检查: RAM > 16GB? GPU VRAM > 8GB? 磁盘空间?
    }
}
```

---

## 五、最终目标架构（搬迁完成后）

### 5.1 Crate 结构总览

```
CarpAI Monorepo (v4.0 搬迁完成)
│
├── crates/
│   │
│   ├── carpai-internal/          ✅ Layer 0: Pure Traits
│   │   ├── src/lib.rs            # 7 traits + AgentContext + AppConfig
│   │   ├── src/session.rs         # SessionStore trait
│   │   ├── src/tool_executor.rs   # ToolExecutor trait
│   │   ├── src/inference_backend.rs # InferenceBackend trait
│   │   ├── src/virtual_filesystem.rs # VirtualFileSystem trait
│   │   ├── src/event_bus.rs       # EventBus trait
│   │   ├── src/memory_backend.rs  # MemoryBackend trait
│   │   └── src/agent_context.rs   # DI Container
│   │
│   ├── carpai-core/              ✅ Layer 1: Business Logic (~120 模块)
│   │   ├── src/lib.rs             # Re-exports
│   │   ├── src/config.rs          # CoreConfig
│   │   ├── src/agent_loop.rs      # execute_agent_turn()
│   │   ├── src/agent/             # Agent 系统 (~12 模块)
│   │   ├── src/memory/            # 记忆系统 (~13 模块)
│   │   ├── src/tools/             # 工具系统 (~7 模块)
│   │   ├── src/completion/        # 补全引擎 (~4 模块)
│   │   ├── src/refactor/          # 重构引擎 (~14 模块)
│   │   ├── src/analysis/          # AST/语义分析 (~8 模块)
│   │   ├── src/session/           # 会话管理 (~6 模块)
│   │   ├── src/storage/           # 文件操作 (~7 模块)
│   │   ├── src/git/               # Git 集成 (~3 模块)
│   │   ├── src/error/             # 错误处理 (~4 模块)
│   │   ├── src/perf/              # 性能优化 (~11 模块)
│   │   ├── src/inference/         # 推理引擎 (~4 模块)
│   │   │   ├── router.rs          # InferenceRouter (80/20)
│   │   │   ├── qwen_local.rs      # Qwen3.x 本地
│   │   │   ├── remote_fallback.rs # 远程 Fallback
│   │   │   └── hybrid_selector.rs # 路由决策
│   │   ├── src/provider/          # LLM Provider (~10 模块)
│   │   ├── src/local_impls/       # Trait 实现 (6 个)
│   │   └── src/platform/          # 平台抽象
│   │
│   ├── carpai-server/            ✅ Layer 2a: Enterprise Server (~20 模块)
│   │   ├── src/main.rs            # fn main()
│   │   ├── src/lib.rs
│   │   ├── src/config.rs          # ServerConfig
│   │   ├── src/app.rs             # Router 组装
│   │   ├── src/grpc/              # gRPC 服务
│   │   ├── src/rest/              # REST API (OpenAI 兼容)
│   │   ├── src/ws/                # WebSocket
│   │   ├── src/auth/              # JWT/RBAC/API-Key
│   │   ├── src/enterprise/        # 多租户/配额/审计
│   │   └── src/observability/     # Metrics/Tracing/Audit
│   │
│   ├── carpai-cli/               ✅ Layer 2b: TUI Client (~15 模块)
│   │   ├── src/main.rs            # fn main() → clap CLI
│   │   ├── src/lib.rs
│   │   ├── src/config.rs          # CliConfig
│   │   ├── src/agent_bridge.rs    # TUI ↔ Core Bridge
│   │   ├── src/cli/               # Commands
│   │   ├── src/tui/               # Pure Rendering (ratatui)
│   │   ├── src/ambient/           # Background Tasks
│   │   ├── src/notifications/     # Telegram/Gmail/Browser
│   │   └── src/modes.rs           # Local/Remote mode
│   │
│   ├── carpai-sdk/               ✅ Layer 2c: IDE Plugin SDK
│   │   ├── src/lib.rs
│   │   ├── src/types.rs           # OpenAI Compatible Types
│   │   ├── src/client.rs          # HTTP + gRPC Client
│   │   └── src/wasm/              # WASM binding (optional)
│   │
│   └── [jcode-* crates]          ✅ 保持不变 (~100 子 crate)
│
├── src/                         🗑️ 过渡区 (清空中)
│   ├── lib.rs                    # 最终只剩 re-export 层或删除
│   ├── main.rs                   # 保留 (根 bin 入口)
│   └── bin/                      # 保留 (工具 bin)
│
└── docs/
    └── SERVER_ARCHITECTURE_V4.md # 本文档
```

### 5.2 依赖关系（最终状态）

```
                   ┌─────────────────┐
                   │  carpai-internal │  Layer 0: Traits (0 业务 deps)
                   └────────┬────────┘
                            │
              ┌─────────────┼─────────────┐
              ▼             ▼             ▼
     ┌────────────┐ ┌────────────┐ ┌────────────┐
     │ carpai-core │ │carpai-server│ │ carpai-cli  │
     │ ~120 模块   │ │ ~20 模块    │ │ ~15 模块   │
     │ 纯业务逻辑  │ │ gRPC+REST   │ │ TUI+Bridge │
     └──────┬─────┘ └──────┬─────┘ └──────┬─────┘
            │               │              │
            │         ┌─────┴─────┐        │
            │         ▼           ▼        │
            │  ┌──────────┐ ┌──────────┐   │
            │  │ jcode-   │ │ jcode-   │   │
            │  │ grpc     │ │ auth     │   │
            │  └──────────┘ └──────────┘   │
            │               │              │
            └───────┬───────┘              │
                    ▼                      ▼
              ┌────────────────────────────────┐
              │        carpai-sdk              │
              │   IDE Plugin (VSCode/JB/Nvim)  │
              └────────────────────────────────┘

❌ 禁止的反向依赖 (不变):
  - carpai-server → carpai-cli
  - carpai-cli → carpai-server
  - carpai-core → carpai-server OR carpai-cli
  - carpai-internal → 任何业务 crate
```

---

## 六、风险与缓解

| 风险 | 概率 | 影响 | 缓解措施 |
|------|------|------|---------|
| agent_runtime.rs 拆分引入回归 | 中 | 高 | 每个子模块独立编译验证 + 原位测试 |
| 循环依赖 (搬迁后新产生) | 中 | 高 | 严格依赖方向检查 + CI 拦截 |
| Qwen3.x 本地推理性能不足 | 中 | 中 | 80/20 策略可动态调整阈值 |
| Git 历史丢失 (git mv 问题) | 低 | 中 | 使用 git mv (非 cp+rm) |
| 某 Batch 超期 | 高 | 低 | Batch 内有 10% buffer time |
| Team 间接口不一致 | 中 | 高 | Week 3 接口契约冻结 |

---

## 七、验收标准

### 7.1 Phase 1 完成 (Week 8 End)

- [ ] `src/lib.rs` 模块声明 < 20 行 (仅 re-export 或空)
- [ ] `cargo check --workspace` : **0 error, < 50 warnings**
- [ ] `cargo test --workspace` : 核心测试通过
- [ ] `carpai-core` 独立编译: **0 error**
- [ ] `carpai-server` 独立编译: **0 error**
- [ ] `carpai-cli` 独立编译: **0 error**
- [ ] `carpai-internal` 无业务逻辑泄漏
- [ ] 无循环依赖 (cargo tree --duplicates 确认)

### 7.2 v1.0.0 Release (Week 12 End)

- [ ] 以上全部 +
- [ ] E2E 4 条测试链全部通过
- [ ] 性能基准: P99 latency < 2s (local), < 5s (remote)
- [ ] 安全审计: 0 CRITICAL/HIGH CVE
- [ ] 部署文档: Docker + K8s + systemd 三种方式
- [ ] Changelog + Release Notes

---

> **文档状态**: v4.0 FINAL | **下一步**: 等待 cargo check 结果确认 0 error 后，从 Batch 1 开始执行搬迁
