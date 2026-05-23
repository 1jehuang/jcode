# P0任务执行计划 - Inline Completion激活

**开始日期**: 2026-05-22  
**预计完成**: 2026-06-05 (2周)  
**负责人**: 杨其城 + AI助手

---

## 任务清单

### ✅ Task 1: 集成Inline Completion到TUI主循环

**状态**: 🔄 进行中  
**优先级**: P0-1 (最高)

#### 子任务:
1. **在TUI App State中添加CompletionEngine**
   - 文件: `src/tui/app/state.rs`
   - 添加字段: `completion_engine: Option<Arc<CompletionEngine>>`
   - 初始化时创建引擎实例

2. **在键盘事件中触发补全**
   - 文件: `src/tui/app/event_handler.rs`
   - 监听Tab键或Ctrl+Space
   - 调用`trigger_prefetch`

3. **渲染Ghost Text**
   - 文件: `src/tui/ui/editor.rs`
   - 从prefetch cache获取补全
   - 以灰色文本显示在当前行后

4. **接受/拒绝补全**
   - Tab键接受
   - Escape键拒绝
   - 继续输入自动拒绝

**验收标准**:
- [ ] TUI中按Tab能看到AI补全建议
- [ ] Ghost Text正确显示
- [ ] 补全延迟 < 200ms (cache hit) / < 2s (LLM call)
- [ ] 用户可以接受或拒绝补全

---

### ✅ Task 2: 集成Inline Completion到VSCode插件

**状态**: ⏳ 待开始  
**优先级**: P0-2

#### 子任务:
1. **实现后端API endpoint**
   - 文件: `src/api/completions.rs` (新建)
   - Endpoint: `POST /api/v1/completions/inline`
   - 接收: `{file_path, content, cursor_line, cursor_column}`
   - 返回: `{completions: [{text, score}]}`

2. **连接CompletionEngine**
   - 在API handler中调用`engine.complete()`
   - 返回排序后的候选列表

3. **完善VSCode Provider**
   - 文件: `editors/vscode-carpai/src/inlineCompletionProvider.ts`
   - 调用后端API
   - 渲染InlineCompletionItem

4. **添加配置选项**
   - `carpai.inlineCompletion.enabled`: true/false
   - `carpai.inlineCompletion.debounceMs`: 200
   - `carpai.inlineCompletion.maxSuggestions`: 3

**验收标准**:
- [ ] VSCode中能显示AI补全建议
- [ ] 补全实时更新（debounce后）
- [ ] 用户可按Tab接受
- [ ] 配置项生效

---

### ✅ Task 3: 集成Task Decomposer到Agent工作流

**状态**: ⏳ 待开始  
**优先级**: P0-3

#### 子任务:
1. **创建LLM-based Plan Generator**
   - 文件: `src/agent/plan_generator.rs` (新建)
   - 调用LLM分解复杂目标为子任务
   - 输出: `Vec<DecomposedTask>`

2. **集成到Agent System Prompt**
   - 文件: `src/agent/prompting.rs`
   - 在system prompt中添加规划指令
   - Agent遇到复杂任务时自动调用decomposer

3. **实现ProgressTracker**
   - 文件: `src/agent/progress_tracker.rs` (新建)
   - 跟踪每个子任务状态
   - 支持失败重试和动态调整

4. **修改turn_execution**
   - 文件: `src/agent/turn_execution.rs`
   - 在执行前检查是否需要规划
   - 如有需要，先分解再执行

**验收标准**:
- [ ] Agent能自主分解复杂任务
- [ ] 任务执行有进度追踪
- [ ] 失败时能自动重试
- [ ] 用户能看到任务分解结果

---

### ✅ Task 4: 补充核心单元测试

**状态**: ⏳ 待开始  
**优先级**: P0-4

#### 子任务:
1. **Inline Completion测试**
   - 文件: `crates/jcode-completion/tests/inline_completion_tests.rs` (新建)
   - 测试完整流程：AST解析 -> LLM调用 -> 记忆排序
   - 测试cache hit/miss场景
   - 测试behavior learner

2. **Task Decomposer测试**
   - 文件: `src/task_decomposer_tests.rs` (新建)
   - 测试DAG构建
   - 测试拓扑排序
   - 测试依赖解析

3. **Integration Tests**
   - 文件: `tests/p0_integration_tests.rs` (新建)
   - TUI补全流程端到端测试
   - Agent规划执行测试
   - VSCode API测试

4. **Benchmark Tests**
   - 文件: `benches/completion_bench.rs` (新建)
   - 测量补全延迟
   - 测量cache命中率
   - 测量内存使用

**验收标准**:
- [ ] 测试覆盖率 > 60%
- [ ] 所有测试通过
- [ ] Benchmark显示性能达标
- [ ] CI/CD包含这些测试

---

## 技术实现细节

### 1. CompletionEngine初始化

```rust
// src/tui/app/state.rs
use jcode_completion::{CompletionEngine, ProviderCandidateGenerator};
use std::sync::Arc;

pub struct AppState {
    // ... existing fields ...
    completion_engine: Option<Arc<CompletionEngine>>,
}

impl AppState {
    pub async fn init_completion_engine(&mut self) {
        let provider = Box::new(ProviderCandidateGenerator::new(
            self.provider.clone(), // 复用现有LLM provider
        ));
        
        let engine = CompletionEngine::new(
            provider,
            None, // 暂时不使用LSP
            Some(self.config.storage_path.clone()),
        );
        
        self.completion_engine = Some(Arc::new(engine));
    }
}
```

### 2. API Endpoint实现

```rust
// src/api/completions.rs
use axum::{Json, extract::State};
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
pub struct InlineCompletionRequest {
    file_path: String,
    content: String,
    cursor_line: usize,
    cursor_column: usize,
}

#[derive(Serialize)]
pub struct InlineCompletionResponse {
    completions: Vec<CompletionSuggestion>,
}

pub async fn inline_completion(
    State(state): State<AppState>,
    Json(req): Json<InlineCompletionRequest>,
) -> Json<InlineCompletionResponse> {
    if let Some(engine) = &state.completion_engine {
        let ranked = engine.complete(
            &req.file_path,
            &req.content,
            req.cursor_line,
            req.cursor_column,
        ).await;
        
        let completions = ranked.into_iter()
            .take(3)
            .map(|r| CompletionSuggestion {
                text: r.candidate.text,
                score: r.rank_score,
            })
            .collect();
        
        Json(InlineCompletionResponse { completions })
    } else {
        Json(InlineCompletionResponse { completions: vec![] })
    }
}
```

### 3. Task Decomposer集成

```rust
// src/agent/plan_generator.rs
use crate::task_decomposer::{TaskDecomposer, DecomposedTask};

pub struct PlanGenerator {
    llm: Arc<dyn Provider>,
}

impl PlanGenerator {
    pub async fn generate_plan(&self, goal: &str) -> Result<Vec<DecomposedTask>> {
        let prompt = format!(
            "Break down this complex goal into executable sub-tasks:\n\n\
             Goal: {}\n\n\
             For each task, provide:\n\
             - id: unique identifier\n\
             - title: short description\n\
             - description: detailed explanation\n\
             - depends_on: list of task IDs this depends on\n\
             - priority: high/medium/low\n\n\
             Output as JSON array.",
            goal
        );
        
        let response = self.llm.complete(&prompt).await?;
        let tasks: Vec<DecomposedTask> = serde_json::from_str(&response)?;
        
        Ok(tasks)
    }
}
```

---

## 时间表

| 周次 | 任务 | 负责人 | 状态 |
|------|------|--------|------|
| Week 1 Day 1-2 | Task 1: TUI集成 | 杨其城 | 🔄 进行中 |
| Week 1 Day 3-4 | Task 2: VSCode集成 | 杨其城 | ⏳ 待开始 |
| Week 1 Day 5 | Task 3: Task Decomposer | 杨其城 | ⏳ 待开始 |
| Week 2 Day 1-3 | Task 4: 测试编写 | QA团队 | ⏳ 待开始 |
| Week 2 Day 4-5 | 联调与优化 | 全体 | ⏳ 待开始 |

---

## 风险与缓解

### 风险1: LLM调用延迟过高
**影响**: 用户体验差  
**缓解**: 
- 强化prefetch cache
- 使用streaming响应
- 设置超时降级策略

### 风险2: Task Decomposer生成无效计划
**影响**: Agent执行失败  
**缓解**:
- 添加计划验证逻辑
- 实现replanner
- 人工审核模式（初期）

### 风险3: 测试覆盖率不足
**影响**: 生产环境问题  
**缓解**:
- TDD开发方式
- CI强制要求覆盖率
- Code review检查

---

## 成功指标

### 功能指标
- ✅ TUI中补全可用率 > 90%
- ✅ VSCode中补全可用率 > 90%
- ✅ Agent自主规划成功率 > 70%

### 性能指标
- ✅ Cache hit延迟 < 100ms
- ✅ Cache miss延迟 < 2s
- ✅ 补全接受率 > 25%

### 质量指标
- ✅ 单元测试覆盖率 > 60%
- ✅ Integration tests全部通过
- ✅ 无critical bugs

---

**文档作者**: AI助手  
**审核人**: 杨其城  
**最后更新**: 2026-05-22
