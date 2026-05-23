# P2任务详细执行计划

**启动日期**: 2026-05-22  
**预计完成**: 2026-07-13 (7-12周)  
**状态**: 🚀 已启动

---

## 📊 P2任务概览

根据代码检查，所有P2模块已有基础框架，需要完善和集成。

| 模块 | 当前状态 | 完成度 | 文件位置 |
|------|---------|--------|---------|
| **TDD支持** | 基础框架 | 40% | `src/tdd/mod.rs` (353行) |
| **性能优化** | L1/L2缓存 | 50% | `src/performance_advanced/mod.rs` (361行) |
| **可视化Dashboard** | 后端API | 60% | `src/dashboard/` (多个文件) |

---

## 一、TDD支持完善 (2周)

### 当前实现状态

**文件**: `src/tdd/mod.rs` (353行)

**已完成**:
```rust
✅ TestGenerator - 测试生成器
   - generate_unit_test() - 生成单元测试骨架
   - generate_property_test() - 生成属性测试
   - extract_signature() - 提取函数签名
   
✅ EdgeCaseDetector - 边界情况检测
   - detect() - 检测常见边界情况
   - 空值、越界、并发等场景
   
✅ CoverageAnalyzer - 覆盖率分析（部分）
✅ TestRunner - 测试运行器（部分）
```

**缺失功能** (60%):
```rust
❌ 缺少LLM集成
   - 当前只生成TODO注释的测试骨架
   - 需要调用LLM生成实际测试代码
   
❌ 缺少智能断言生成
   - 无法自动推断expected_output
   - 需要语义理解能力
   
❌ 缺少测试执行集成
   - 无法自动运行生成的测试
   - 缺少结果反馈循环
   
❌ 缺少覆盖率可视化
   - 无HTML报告生成
   - 无缺口分析
```

---

### Week 1: LLM集成与智能测试生成

#### Day 1-2: 集成LLM Provider

**目标**: 让TestGenerator能够调用LLM生成完整测试

**实现方案**:
```rust
// src/tdd/mod.rs - 修改generate_unit_test
pub async fn generate_unit_test_llm(
    file_path: &str, 
    function_name: &str,
    llm_provider: Arc<dyn Provider>,  // 新增参数
) -> Result<String, String> {
    
    let content = tokio::fs::read_to_string(file_path).await?;
    let signature = Self::extract_signature(&content, function_name)?;
    let edge_cases = EdgeCaseDetector::detect(&content, function_name);
    
    // 构建prompt
    let prompt = format!(
        "Generate comprehensive unit tests for this Rust function:\n\n\
         Function signature: {}\n\n\
         File content:\n{}\n\n\
         Edge cases to cover:\n{}\n\n\
         Requirements:\n\
         1. Include basic functionality test\n\
         2. Cover all edge cases\n\
         3. Use proper assertions\n\
         4. Add descriptive comments\n\
         5. Follow Rust testing conventions",
        signature,
        content.lines().take(50).collect::<Vec<_>>().join("\n"),
        edge_cases.iter().map(|e| format!("- {}", e.description)).collect::<Vec<_>>().join("\n")
    );
    
    // 调用LLM
    let test_code = llm_provider.complete(&prompt).await?;
    
    Ok(test_code)
}
```

**验收标准**:
- [ ] 能生成可编译的测试代码
- [ ] 包含至少3个测试用例
- [ ] 覆盖基本功能和边界情况

---

#### Day 3-4: 智能断言生成

**目标**: 自动推断expected_output

**实现方案**:
```rust
// 新增模块: src/tdd/assertion_inference.rs
pub struct AssertionInferrer;

impl AssertionInferrer {
    /// 基于函数返回值类型推断预期输出
    pub async fn infer_expected_output(
        function_signature: &str,
        sample_inputs: Vec<String>,
        llm: Arc<dyn Provider>,
    ) -> Result<Vec<(String, String)>, String> {
        
        let mut results = Vec::new();
        
        for input in sample_inputs {
            let prompt = format!(
                "Given this Rust function:\n{}\n\n\
                 And this input: {}\n\n\
                 What would be the expected output?\n\
                 Return ONLY the output value.",
                function_signature, input
            );
            
            let output = llm.complete(&prompt).await?;
            results.push((input, output.trim().to_string()));
        }
        
        Ok(results)
    }
}
```

**验收标准**:
- [ ] 能正确推断简单函数的输出
- [ ] 支持多种返回类型（i32, String, Option, Result）
- [ ] 准确率 > 80%

---

#### Day 5: 测试执行集成

**目标**: 自动运行生成的测试并反馈结果

**实现方案**:
```rust
// src/tdd/test_runner.rs
pub struct TestRunner;

impl TestRunner {
    /// 运行指定文件的测试
    pub async fn run_tests(file_path: &str) -> Result<TestReport, String> {
        let output = Command::new("cargo")
            .arg("test")
            .arg("--file")
            .arg(file_path)
            .output()
            .await?;
        
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        
        // 解析测试结果
        let report = Self::parse_test_output(&stdout, &stderr)?;
        
        Ok(report)
    }
    
    /// 如果测试失败，提供修复建议
    pub async fn suggest_fixes(failed_tests: &[FailedTest]) -> Vec<String> {
        // 调用LLM分析失败原因并提供修复建议
        todo!()
    }
}

#[derive(Debug)]
pub struct TestReport {
    pub total: usize,
    pub passed: usize,
    pub failed: usize,
    pub duration_ms: u64,
    pub failures: Vec<FailedTest>,
}
```

**验收标准**:
- [ ] 能正确解析cargo test输出
- [ ] 生成结构化的测试报告
- [ ] 失败时提供有用的错误信息

---

### Week 2: 覆盖率分析与可视化

#### Day 1-2: 覆盖率数据收集

**实现方案**:
```rust
// src/tdd/coverage.rs
pub struct CoverageCollector;

impl CoverageCollector {
    /// 使用cargo-tarpaulin收集覆盖率
    pub async fn collect_coverage(crate_path: &str) -> Result<CoverageReport, String> {
        let output = Command::new("cargo")
            .arg("tarpaulin")
            .arg("--out")
            .arg("Json")
            .current_dir(crate_path)
            .output()
            .await?;
        
        let report: TarpaulinReport = serde_json::from_slice(&output.stdout)?;
        
        Ok(CoverageReport {
            line_coverage: report.line_coverage,
            branch_coverage: report.branch_coverage,
            uncovered_lines: Self::find_uncovered(&report),
            coverage_by_file: Self::group_by_file(&report),
        })
    }
}
```

---

#### Day 3-4: HTML报告生成

**实现方案**:
```rust
// src/tdd/report_generator.rs
pub struct ReportGenerator;

impl ReportGenerator {
    /// 生成HTML覆盖率报告
    pub async fn generate_html_report(
        coverage: &CoverageReport,
        output_dir: &Path,
    ) -> Result<PathBuf, String> {
        
        let html = format!(
            r#"<!DOCTYPE html>
<html>
<head><title>Coverage Report</title></head>
<body>
<h1>Test Coverage Report</h1>
<div class="summary">
    <p>Line Coverage: {:.1}%</p>
    <p>Branch Coverage: {:.1}%</p>
</div>
<table>
    <tr><th>File</th><th>Coverage</th><th>Uncovered Lines</th></tr>
    {}
</table>
</body>
</html>"#,
            coverage.line_coverage,
            coverage.branch_coverage,
            coverage.coverage_by_file.iter().map(|(file, cov)| {
                format!("<tr><td>{}</td><td>{:.1}%</td><td>{:?}</td></tr>", 
                        file, cov.percentage, cov.uncovered)
            }).collect::<Vec<_>>().join("\n")
        );
        
        let output_path = output_dir.join("coverage.html");
        tokio::fs::write(&output_path, html).await?;
        
        Ok(output_path)
    }
}
```

---

#### Day 5: TDD工作流集成

**目标**: 将TDD工具集成到Agent工作流

**实现方案**:
```rust
// src/tool/tdd_tool.rs
pub struct TddTool;

#[async_trait]
impl Tool for TddTool {
    fn name(&self) -> &str { "tdd_generate_tests" }
    
    async fn execute(&self, params: TddParams) -> Result<ToolOutput> {
        match params.action {
            TddAction::Generate => {
                let test_code = TestGenerator::generate_unit_test_llm(
                    &params.file_path,
                    &params.function_name,
                    self.llm.clone(),
                ).await?;
                
                // 写入测试文件
                let test_file = Self::get_test_file_path(&params.file_path);
                tokio::fs::write(&test_file, test_code).await?;
                
                // 运行测试
                let report = TestRunner::run_tests(&params.file_path).await?;
                
                Ok(ToolOutput::success(format!(
                    "Generated and ran tests. Passed: {}/{}",
                    report.passed, report.total
                )))
            }
            TddAction::AnalyzeCoverage => {
                let coverage = CoverageCollector::collect_coverage(&params.crate_path).await?;
                let report_path = ReportGenerator::generate_html_report(
                    &coverage,
                    Path::new("target/coverage"),
                ).await?;
                
                Ok(ToolOutput::success(format!(
                    "Coverage report generated at: {}",
                    report_path.display()
                )))
            }
        }
    }
}
```

**验收标准**:
- [ ] Agent可以调用tdd_generate_tests工具
- [ ] 自动生成、运行、报告测试
- [ ] 端到端流程完整

---

## 二、性能优化完善 (2周)

### 当前实现状态

**文件**: `src/performance_advanced/mod.rs` (361行)

**已完成**:
```rust
✅ LlmResponseCache - LLM响应缓存
   - L1内存缓存 (LRU)
   - L2磁盘缓存
   - TTL过期机制
   
✅ PredictivePrecomputer - 预测性预计算（框架）
✅ ParallelExecutor - 并行执行器（框架）
✅ LazyContextLoader - 懒加载上下文（框架）
```

**缺失功能** (50%):
```rust
❌ L3-L6缓存层缺失
❌ 语义缓存未实现（基于embedding相似度）
❌ 预计算策略不完整
❌ 并行执行缺少限流和监控
❌ 懒加载未集成到上下文选择器
```

---

### Week 3: 6层缓存架构完善

#### Day 1-2: L3-L6缓存层实现

**实现方案**:
```rust
// src/performance_advanced/mod.rs - 扩展LlmResponseCache
pub struct LlmResponseCache {
    l1_memory: Arc<RwLock<lru::LruCache<u64, CachedResponse>>>,  // L1: 热缓存 (<1ms)
    l2_disk: Arc<RwLock<HashMap<u64, CachedResponse>>>,           // L2: 温缓存 (<10ms)
    l3_redis: Option<Arc<RedisCache>>,                            // L3: 分布式缓存 (<50ms)
    l4_pgvector: Option<Arc<PgVectorCache>>,                      // L4: 语义缓存 (<100ms)
    l5_cdn: Option<Arc<CdnCache>>,                                // L5: CDN缓存 (<200ms)
    l6_model_cache: Option<Arc<ModelCache>>,                      // L6: 模型级缓存 (<1s)
    stats: Arc<RwLock<CacheStatsAdvanced>>,
}

impl LlmResponseCache {
    /// 完整的6层查找链
    pub async fn get_multi_level(&self, key: u64, prompt: &str) -> Option<String> {
        // L1: Memory
        if let Some(response) = self.l1_get(key).await {
            return Some(response);
        }
        
        // L2: Disk
        if let Some(response) = self.l2_get(key).await {
            self.l1_put(key, response.clone()).await;
            return Some(response);
        }
        
        // L3: Redis (if available)
        if let Some(redis) = &self.l3_redis {
            if let Some(response) = redis.get(key).await {
                self.l2_put(key, response.clone()).await;
                return Some(response);
            }
        }
        
        // L4: Semantic cache (pgvector)
        if let Some(pgvector) = &self.l4_pgvector {
            if let Some(response) = pgvector.semantic_search(prompt).await {
                self.l3_put(key, response.clone()).await;
                return Some(response);
            }
        }
        
        // L5, L6: Similar pattern...
        
        None // Cache miss
    }
}
```

---

#### Day 3-4: 语义缓存实现

**实现方案**:
```rust
// src/performance_advanced/semantic_cache.rs
pub struct SemanticCache {
    pool: PgPool,
    embedding_model: Arc<dyn EmbeddingModel>,
}

impl SemanticCache {
    /// 基于embedding相似度查找缓存
    pub async fn semantic_search(&self, prompt: &str, threshold: f64) 
        -> Result<Option<String>, Error> {
        
        // 1. 生成prompt的embedding
        let embedding = self.embedding_model.encode(prompt).await?;
        
        // 2. 在pgvector中搜索相似prompt
        let result = sqlx::query!(
            r#"
            SELECT response, similarity
            FROM semantic_cache
            ORDER BY embedding <-> $1::vector
            LIMIT 1
            "#,
            serde_json::to_string(&embedding)?
        ).fetch_optional(&self.pool).await?;
        
        if let Some(row) = result {
            if row.similarity >= threshold {
                return Ok(Some(row.response));
            }
        }
        
        Ok(None)
    }
    
    /// 存储新的prompt-response对
    pub async fn store(&self, prompt: &str, response: &str) -> Result<(), Error> {
        let embedding = self.embedding_model.encode(prompt).await?;
        
        sqlx::query!(
            r#"
            INSERT INTO semantic_cache (prompt, response, embedding, created_at)
            VALUES ($1, $2, $3::vector, NOW())
            ON CONFLICT (prompt_hash) DO NOTHING
            "#,
            prompt,
            response,
            serde_json::to_string(&embedding)?
        ).execute(&self.pool).await?;
        
        Ok(())
    }
}
```

---

#### Day 5: 缓存统计与监控

**实现方案**:
```rust
// src/performance_advanced/cache_monitor.rs
pub struct CacheMonitor {
    cache: Arc<LlmResponseCache>,
}

impl CacheMonitor {
    /// 获取详细的缓存统计
    pub async fn get_detailed_stats(&self) -> CacheDetailedStats {
        let stats = self.cache.stats.read().await;
        
        let total_requests = stats.l1_hits + stats.l2_hits + stats.misses;
        let hit_rate = if total_requests > 0 {
            (stats.l1_hits + stats.l2_hits) as f64 / total_requests as f64
        } else {
            0.0
        };
        
        CacheDetailedStats {
            l1_hit_rate: stats.l1_hits as f64 / total_requests as f64,
            l2_hit_rate: stats.l2_hits as f64 / total_requests as f64,
            overall_hit_rate: hit_rate,
            tokens_saved: stats.tokens_saved,
            avg_latency_saved_ms: stats.avg_latency_saved_ms,
            estimated_cost_savings_usd: Self::calculate_cost_savings(stats.tokens_saved),
        }
    }
}
```

---

### Week 4: 预计算与并行优化

#### Day 1-2: 预测性预计算完善

**实现方案**:
```rust
// src/performance_advanced/predictive_precompute.rs
pub struct PredictivePrecomputer {
    hot_paths: Arc<RwLock<HashSet<String>>>,
    precomputed_results: Arc<RwLock<HashMap<String, String>>>,
}

impl PredictivePrecomputer {
    /// 学习用户模式，识别热点路径
    pub async fn learn_patterns(user_actions: &[UserAction]) {
        // 分析用户行为序列
        // 识别频繁访问的代码路径
        // 标记为hot_path
    }
    
    /// 后台预计算热点路径
    pub async fn precompute_hot_paths(&self, llm: Arc<dyn Provider>) {
        let paths = self.hot_paths.read().await.clone();
        
        for path in paths {
            if !self.precomputed_results.read().await.contains_key(&path) {
                // 异步预计算
                let result = llm.complete(&path).await;
                if let Ok(response) = result {
                    self.precomputed_results.write().await.insert(path, response);
                }
            }
        }
    }
}
```

---

#### Day 3-4: 并行执行器完善

**实现方案**:
```rust
// src/performance_advanced/parallel_executor.rs
pub struct ParallelExecutor {
    max_concurrency: usize,
    semaphore: Arc<Semaphore>,
}

impl ParallelExecutor {
    /// 并行执行多个工具调用
    pub async fn execute_parallel<T: Tool>(
        &self,
        tools: Vec<(T, ToolParams)>,
    ) -> Vec<Result<ToolOutput, Error>> {
        
        let mut handles = Vec::new();
        
        for (tool, params) in tools {
            let permit = self.semaphore.clone().acquire_owned().await?;
            
            let handle = tokio::spawn(async move {
                let result = tool.execute(params).await;
                drop(permit); // 释放许可
                result
            });
            
            handles.push(handle);
        }
        
        // 等待所有任务完成
        let mut results = Vec::new();
        for handle in handles {
            results.push(handle.await?);
        }
        
        results
    }
}
```

---

#### Day 5: 懒加载上下文集成

**实现方案**:
```rust
// src/context/intelligent_selector.rs - 添加懒加载
impl IntelligentSelector {
    /// 懒加载上下文 - 只加载必要的文件
    pub async fn select_context_lazy(
        &self,
        query: &str,
        max_tokens: usize,
    ) -> Result<SelectedContext, Error> {
        
        // 1. 先加载高相关性文件
        let mut context = self.select_high_relevance(query, max_tokens / 2).await?;
        
        // 2. 如果还有预算，按需加载中等相关性文件
        if context.total_tokens < max_tokens {
            let remaining_budget = max_tokens - context.total_tokens;
            let medium_relevance = self.select_medium_relevance(query, remaining_budget).await?;
            context.files.extend(medium_relevance.files);
            context.total_tokens += medium_relevance.total_tokens;
        }
        
        Ok(context)
    }
}
```

---

## 三、可视化Dashboard完善 (3周)

### 当前实现状态

**文件**: `src/dashboard/` (多个文件)

**已完成**:
```rust
✅ DashboardServer - HTTP服务器
✅ DashboardRoutes - API路由
✅ SystemMetrics - 系统指标收集
✅ 基础HTML模板
```

**缺失功能** (40%):
```rust
❌ WebSocket实时更新
❌ React前端界面
❌ Agent状态可视化
❌ 任务执行历史图表
❌ 审计日志系统
```

---

### Week 5-6: Web Dashboard开发

#### Day 1-3: React前端框架搭建

**技术栈**:
- React 18 + TypeScript
- Recharts (图表库)
- WebSocket客户端
- TailwindCSS (样式)

**项目结构**:
```
web-dashboard/
├── src/
│   ├── components/
│   │   ├── SystemStatus.tsx
│   │   ├── AgentMonitor.tsx
│   │   ├── TaskHistory.tsx
│   │   └── PerformanceCharts.tsx
│   ├── hooks/
│   │   └── useWebSocket.ts
│   └── App.tsx
├── package.json
└── tsconfig.json
```

---

#### Day 4-7: WebSocket实时更新

**后端实现**:
```rust
// src/dashboard/websocket.rs
use tokio_tungstenite::tungstenite::Message;

pub struct DashboardWebSocket {
    clients: Arc<RwLock<HashMap<String, WebSocketSink>>>,
}

impl DashboardWebSocket {
    /// 广播系统指标更新
    pub async fn broadcast_metrics(&self, metrics: &SystemMetrics) {
        let message = serde_json::json!({
            "type": "metrics_update",
            "data": metrics
        }).to_string();
        
        for client in self.clients.read().await.values() {
            let _ = client.send(Message::Text(message.clone())).await;
        }
    }
    
    /// 广播Agent状态变化
    pub async fn broadcast_agent_status(&self, agent_id: &str, status: &AgentStatus) {
        let message = serde_json::json!({
            "type": "agent_status",
            "agent_id": agent_id,
            "status": status
        }).to_string();
        
        // Similar broadcast logic
    }
}
```

---

### Week 7: 高级功能

#### Day 1-3: 审计日志系统

**实现方案**:
```rust
// src/dashboard/audit_log.rs
pub struct AuditLogger {
    db: PgPool,
}

impl AuditLogger {
    /// 记录Agent操作
    pub async fn log_action(&self, action: AuditAction) -> Result<(), Error> {
        sqlx::query!(
            r#"
            INSERT INTO audit_log (
                timestamp, agent_id, action_type, details, ip_address
            ) VALUES (NOW(), $1, $2, $3, $4)
            "#,
            action.agent_id,
            action.action_type,
            serde_json::to_string(&action.details)?,
            action.ip_address
        ).execute(&self.db).await?;
        
        Ok(())
    }
    
    /// 查询审计日志
    pub async fn query_logs(&self, filters: AuditFilters) -> Result<Vec<AuditLogEntry>, Error> {
        // SQL query with filters
        todo!()
    }
}
```

---

#### Day 4-5: 集成测试与部署

**验收标准**:
- [ ] Dashboard可访问（http://localhost:3000）
- [ ] 实时显示系统指标
- [ ] Agent状态实时更新
- [ ] 任务历史可查询
- [ ] 审计日志完整

---

## 📈 预期成果

完成P2任务后，CarpAI将达到：

| 指标 | 当前 | P2完成后 | 提升 |
|------|------|---------|------|
| TDD支持 | 40% | 90% | +50% |
| 缓存命中率 | 50% | 85% | +35% |
| Dashboard功能 | 60% | 95% | +35% |
| **综合追平度** | **72%** | **92%** | **+20%** |

**距离Claude Code差距**: 从28%缩小到8%

---

## 🎯 下一步行动

### 立即执行（今天）
1. ✅ 阅读P2执行计划
2. ⏳ 开始Week 1: TDD LLM集成
3. ⏳ 配置开发环境

### 本周内
4. ⏳ 完成TestGenerator LLM集成
5. ⏳ 实现智能断言生成
6. ⏳ 集成测试执行器

---

**文档版本**: 1.0  
**最后更新**: 2026-05-22  
**负责人**: 杨其城 + AI助手
