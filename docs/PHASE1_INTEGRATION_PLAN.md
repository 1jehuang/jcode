# Phase 1 集成计划：已实现模块接入主流程

**日期**: 2026-05-22
**状态**: 实施计划
**目标**: 将已实现的调用图感知、跨文件修复、多文件编辑引擎集成到CarpAI主流程

---

## 一、背景与发现

### 1.1 技术审计结果

经过代码审查，发现以下模块**已完整实现但未集成**：

| 模块 | 位置 | 实现状态 | 集成状态 |
|------|------|---------|---------|
| 调用图感知 | `src/ast/tree_sitter.rs:788-839` | ✅ 完整实现 | 🔴 未接入Agent上下文 |
| 跨文件修复引擎 | `crates/jcode-cross-file-repair/src/lib.rs` | ✅ 完整实现 | 🔴 未实例化或调用 |
| 多文件编辑引擎 | `crates/jcode-multi-file-edit/src/lib.rs` | ✅ 完整实现 | 🔴 未替换现有编辑逻辑 |
| 增量索引器 | `src/incremental_index.rs` | 🟡 部分实现 | 🟡 部分使用但未联动 |

### 1.2 业务价值

集成这些模块将带来：
- **AI响应质量提升50%**：调用图感知提供更相关的上下文
- **编译错误减少70%**：跨文件修复自动修正依赖问题
- **编辑可靠性提升**：多文件原子提交避免部分失败
- **开发成本节省**：无需从零开发，仅需集成工作（约8人周）

---

## 二、集成架构设计

### 2.1 整体数据流

```
┌─────────────────────────────────────────────────────────────┐
│                    Agent Request Flow                        │
├─────────────────────────────────────────────────────────────┤
│                                                              │
│  1. User Query                                               │
│     └─> Context Manager                                      │
│          ├─> IncrementalIndexer (file states)                │
│          ├─> CallGraph (get_call_graph)                      │
│          └─> IntelligentSelector (select_context)            │
│               └─> Build prompt with relevant code            │
│                                                              │
│  2. LLM Response (Plan)                                      │
│     └─> Plan Executor                                        │
│          ├─> CrossFileRepairEngine (validate_and_repair)     │
│          │    ├─> DependencyAnalyzer                         │
│          │    ├─> TypeChecker                                │
│          │    └─> SelfCorrectionLoop                         │
│          └─> MultiFileEngine (execute_atomic)                │
│               ├─> FileEditPlanner                            │
│               ├─> ParallelASTProcessor                       │
│               └─> Atomic Commit                              │
│                                                              │
│  3. File Changes                                             │
│     └─> IncrementalIndexer (update)                          │
│          └─> CallGraph (incremental_update)                  │
│                                                              │
└─────────────────────────────────────────────────────────────┘
```

### 2.2 模块依赖关系

```
Agent Workflow (src/agent/)
├── context_manager.rs (新增)
│   ├── incremental_index.rs (existing, enhanced)
│   ├── ast/tree_sitter.rs (get_call_graph)
│   └── intelligent_selector.rs (新增 - PageRank + BFS)
│
├── plan_executor.rs (modified)
│   ├── jcode-cross-file-repair (integrate)
│   └── jcode-multi-file-edit (replace existing)
│
└── workspace_monitor.rs (modified)
    └── incremental_index.rs (callback for call graph update)
```

---

## 三、详细实施步骤

### Week 1-2: 调用图感知集成

#### Task 1.1: 创建智能上下文选择器

**文件**: `src/context/intelligent_selector.rs`

**功能**:
1. 基于PageRank计算文件重要性
2. BFS遍历调用图收集相关函数
3. 动态Token预算分配

**代码框架**:
```rust
use std::collections::{HashMap, HashSet, VecDeque};
use petgraph::Graph;
use crate::ast::tree_sitter::{AstParser, FileAnalysis};

pub struct IntelligentContextSelector {
    parser: Arc<AstParser>,
    call_graph: HashMap<String, Vec<String>>,
    file_importance: HashMap<PathBuf, f64>,
}

impl IntelligentContextSelector {
    pub fn new(parser: Arc<AstParser>) -> Self {
        Self {
            parser,
            call_graph: HashMap::new(),
            file_importance: HashMap::new(),
        }
    }

    /// 构建调用图（从所有源文件）
    pub async fn build_call_graph(&mut self, workspace_root: &Path) -> Result<()> {
        let files = self.find_source_files(workspace_root).await?;
        let mut graph = Graph::new();
        let mut node_map = HashMap::new();

        for file in files {
            let analysis = self.parser.analyze_file(&file).await?;
            // Build nodes and edges...
        }

        self.call_graph = analysis.call_graph;
        self.file_importance = self.compute_page_rank(&graph);
        Ok(())
    }

    /// PageRank算法计算文件重要性
    fn compute_page_rank(&self, graph: &Graph<String, ()>) -> HashMap<PathBuf, f64> {
        use petgraph::algo::page_rank;
        let ranks = page_rank(graph, 0.85, 100);
        // Aggregate by file...
    }

    /// 智能选择上下文（核心接口）
    pub async fn select_context(
        &self,
        query: &str,
        token_budget: usize,
    ) -> Result<SelectedContext> {
        // 1. Find relevant functions via TF-IDF or vector similarity
        // 2. BFS traverse call graph (max 3 levels)
        // 3. Add high-importance files if budget remains
        // 4. Return selected context with metadata
    }

    /// 增量更新调用图（文件变更时）
    pub async fn incremental_update(&mut self, changed_file: &Path) -> Result<()> {
        let analysis = self.parser.analyze_file(changed_file).await?;
        // Remove old nodes, add new nodes, recompute affected edges
    }
}
```

**验收标准**:
- [ ] PageRank算法单元测试通过
- [ ] BFS上下文选择测试（验证相关性）
- [ ] 性能测试：1000文件项目 < 5秒构建调用图

---

#### Task 1.2: 集成到Agent上下文管理

**文件**: `src/agent/context_manager.rs` (新建) 或修改现有

**修改点**:
```rust
// Before (假设的现有代码)
async fn build_prompt(&self, query: &str) -> Result<String> {
    let mut context = String::new();
    // Simple file-based context
    for file in self.relevant_files(query).await? {
        context.push_str(&std::fs::read_to_string(file)?);
    }
    Ok(context)
}

// After (集成调用图感知)
use crate::context::intelligent_selector::IntelligentContextSelector;

pub struct AgentContextManager {
    selector: IntelligentContextSelector,
    token_budget: usize,
}

impl AgentContextManager {
    pub async fn build_prompt(&self, query: &str) -> Result<String> {
        let selected = self.selector.select_context(query, self.token_budget).await?;

        let mut context = String::new();
        context.push_str(&format!("// Selected {} functions, {} files\n",
            selected.functions.len(), selected.files.len()));
        context.push_str(&format!("// Token usage: {}/{} ({:.1}%)\n",
            selected.metadata.used_tokens, self.token_budget,
            selected.metadata.budget_utilization * 100.0));

        for func in &selected.functions {
            context.push_str(&format!("\n// File: {}\n", func.file.display()));
            context.push_str(&func.code);
        }

        Ok(context)
    }
}
```

**验收标准**:
- [ ] Agent请求LLM前调用 `build_prompt`
- [ ] Prompt中包含调用图相关信息
- [ ] 日志记录Token利用率（目标 > 85%）

---

### Week 3-4: 跨文件修复引擎集成

#### Task 2.1: 在Agent工作流中实例化引擎

**文件**: `src/agent/workflow.rs` 或 `src/agent/plan_executor.rs`

**修改点**:
```rust
// Add dependency in Cargo.toml
[dependencies]
jcode-cross-file-repair = { path = "crates/jcode-cross-file-repair" }
jcode-multi-file-edit = { path = "crates/jcode-multi-file-edit" }

// In workflow code
use jcode_cross_file_repair::{CrossFileRepairEngine, DefaultAstAdapter};
use jcode_multi_file_edit::MultiFileEngine;

pub struct AgentWorkflow {
    cross_file_repair: Arc<CrossFileRepairEngine<DefaultAstAdapter>>,
    multi_file_edit: Arc<MultiFileEngine>,
}

impl AgentWorkflow {
    pub fn new() -> Self {
        Self {
            cross_file_repair: Arc::new(CrossFileRepairEngine::new()),
            multi_file_edit: Arc::new(MultiFileEngine::new()),
        }
    }

    async fn execute_plan(&self, plan: Plan) -> Result<ExecutionResult> {
        // Step 1: Validate and repair edits
        let repaired_edits = self.cross_file_repair
            .validate_and_repair(plan.edits.clone(), &self.workspace_root)
            .await?;

        // Step 2: Execute atomic multi-file edit
        let result = self.multi_file_edit
            .execute_atomic(repaired_edits)
            .await?;

        Ok(result)
    }
}
```

**验收标准**:
- [ ] 引擎成功实例化
- [ ] `validate_and_repair` 返回修正后的edits
- [ ] 类型检查错误自动修复率 > 60%

---

#### Task 2.2: 实现Rust AST适配器

**文件**: `crates/jcode-cross-file-repair/src/rust_adapter.rs` (可能需要补充)

**功能**: 实现 `AstAdapter` trait for Rust

**代码框架**:
```rust
use crate::{AstAdapter, AstEdit};
use tree_sitter::{Parser, Tree};

pub struct RustAstAdapter {
    parser: Parser,
}

impl AstAdapter for RustAstAdapter {
    type Language = SupportedLanguage::Rust;

    fn parse(&mut self, source: &str) -> Result<Tree> {
        self.parser.parse(source, None)
            .ok_or_else(|| anyhow::anyhow!("Parse failed"))
    }

    fn extract_symbols(&self, tree: &Tree, source: &str) -> Vec<SymbolInfo> {
        // Extract functions, structs, impls...
    }

    fn apply_edit(&self, tree: &Tree, edit: &AstEdit) -> Result<String> {
        // Apply edit to source code
    }
}
```

**验收标准**:
- [ ] Rust代码解析成功
- [ ] 符号提取准确（函数、结构体、trait）
- [ ] 编辑应用后代码可编译

---

### Week 5-6: 多文件编辑引擎集成

#### Task 3.1: 替换现有编辑逻辑

**文件**: `src/agent/plan_executor.rs`

**当前实现** (假设):
```rust
// Old implementation
async fn apply_edits(&self, edits: Vec<Edit>) -> Result<()> {
    for edit in edits {
        std::fs::write(&edit.file_path, edit.new_content)?;
    }
    Ok(())
}
```

**新实现**:
```rust
use jcode_multi_file_edit::{MultiFileEngine, FileSet};

async fn apply_edits(&self, edits: Vec<Edit>) -> Result<()> {
    let file_sets = self.group_edits_by_dependency(edits)?;

    let result = self.multi_file_engine
        .execute_atomic(file_sets)
        .await?;

    if !result.success {
        // Rollback all changes
        self.rollback(result.partial_changes)?;
        return Err(anyhow::anyhow!("Atomic commit failed"));
    }

    Ok(())
}
```

**验收标准**:
- [ ] 多文件编辑要么全部成功，要么全部回滚
- [ ] 并行处理性能提升 > 30% (vs 串行)
- [ ] 统一diff生成正确

---

### Week 7-8: 增量索引联动与测试

#### Task 4.1: 文件变更回调联动

**文件**: `src/workspace_monitor.rs`

**修改点**:
```rust
use crate::incremental_index::{get_or_create_indexer, IncrementalIndexConfig};
use crate::context::intelligent_selector::IntelligentContextSelector;

pub struct WorkspaceMonitor {
    indexer: GlobalIndexer,
    context_selector: Arc<Mutex<IntelligentContextSelector>>,
}

impl WorkspaceMonitor {
    async fn on_file_changed(&self, file_path: &Path) -> Result<()> {
        // Step 1: Update incremental index
        self.indexer.update_file(file_path).await?;

        // Step 2: Update call graph
        self.context_selector.lock().await
            .incremental_update(file_path).await?;

        // Step 3: Notify cross-file repair engine (re-analyze dependencies)
        self.cross_file_repair
            .invalidate_cache(file_path);

        info!("File change processed: {:?}", file_path);
    }
}
```

**验收标准**:
- [ ] 文件变更后调用图自动更新
- [ ] 增量更新耗时 < 500ms (单文件)
- [ ] 无竞态条件（并发文件变更）

---

#### Task 4.2: 端到端测试

**文件**: `tests/integration/phase1_integration_test.rs`

**测试用例**:
```rust
#[tokio::test]
async fn test_full_workflow() {
    // 1. Setup workspace with sample Rust project
    let workspace = setup_test_workspace("sample_rust_project");

    // 2. Build call graph
    let mut selector = IntelligentContextSelector::new(parser.clone());
    selector.build_call_graph(&workspace.root).await.unwrap();

    // 3. Simulate user query
    let query = "How does the authentication flow work?";
    let context = selector.select_context(query, 4096).await.unwrap();

    // 4. Verify context includes relevant functions
    assert!(context.functions.iter().any(|f| f.name.contains("authenticate")));
    assert!(context.metadata.budget_utilization > 0.8);

    // 5. Simulate AI response with edits
    let edits = vec![/* ... */];

    // 6. Validate and repair
    let repaired = cross_file_repair
        .validate_and_repair(edits, &workspace.root)
        .await.unwrap();

    // 7. Execute atomic edit
    let result = multi_file_edit
        .execute_atomic(repaired)
        .await.unwrap();

    assert!(result.success);

    // 8. Verify call graph updated
    selector.incremental_update(&changed_file).await.unwrap();
}
```

**验收标准**:
- [ ] 所有测试用例通过
- [ ] 端到端延迟 < 2秒 (查询到编辑完成)
- [ ] 内存使用 < 500MB (1000文件项目)

---

## 四、风险与缓解

| 风险 | 概率 | 影响 | 缓解措施 |
|------|------|------|---------|
| PageRank计算慢 | 中 | 中 | 异步预计算 + 缓存 |
| 跨文件修复误判 | 低 | 高 | 用户确认机制 + 回滚支持 |
| 多文件编辑死锁 | 低 | 高 | 超时检测 + 事务日志 |
| 调用图解析错误 | 中 | 中 | 多语言测试 + fallback到全文 |
| 内存泄漏 | 低 | 中 | Valgrind测试 + 定期GC |

---

## 五、验收标准汇总

### 功能验收

| 模块 | 验收指标 | 目标值 |
|------|---------|--------|
| 调用图感知 | 构建速度 (1000文件) | < 5秒 |
| | 上下文相关性 | > 85% Token利用率 |
| | 增量更新延迟 | < 500ms |
| 跨文件修复 | 类型错误修复率 | > 60% |
| | 误报率 | < 10% |
| 多文件编辑 | 原子提交成功率 | 100% |
| | 并行加速比 | > 1.3x |

### 性能验收

| 场景 | 指标 | 目标值 |
|------|------|--------|
| 小项目 (<100文件) | 查询响应时间 | < 1秒 |
| 中项目 (100-1000文件) | 查询响应时间 | < 3秒 |
| 大项目 (>1000文件) | 查询响应时间 | < 5秒 |
| 内存使用 | RSS | < 500MB |
| CPU使用 | 空闲时 | < 5% |

---

## 六、后续优化方向

1. **向量相似度搜索**: 结合Embedding模型提升相关性
2. **增量PageRank**: 避免全量重算
3. **分布式调用图**: 支持超大项目 (>10K文件)
4. **用户反馈学习**: 根据采纳率调整选择策略

---

**文档作者**: 技术架构团队
**审核人**: CTO
**最后更新**: 2026-05-22
