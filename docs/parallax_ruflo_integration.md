# Parallax + Ruflo 集成执行计划

## 现状

核心代码已实现但尚未装配。UnifiedScheduler (1217行) + GOAP (890行) + UnifiedQueue (440行) 在 `jcode-unified-scheduler` crate 中，但 `enterprise-server` 的 `serve()` 流程从未实例化它。请求直接走 `jcode-llm` provider 通道绕过所有调度层。

## 目标

```
请求 → PriorityRuleEngine → UnifiedScheduler.submit_task()
       → [Parallax Phase 1] LayerAllocator 分配层到节点
       → [Parallax Phase 2] RequestRouter 路由到最优节点
       → VirtualMemoryManager 建立 KV Cache mmap
       → CpuInferenceEngine / 远端节点 执行推理
       → 返回结果
```

## 步骤

### Step 1: 装配 UnifiedScheduler 到 enterprise-server（~2小时）

**文件**: `crates/jcode-enterprise-server/src/enterprise.rs`

```diff
+ use jcode_unified_scheduler::UnifiedScheduler;

  pub struct EnterpriseServerState {
+     scheduler: Arc<UnifiedScheduler>,
  }

  pub async fn serve(self) -> Result<()> {
+     let scheduler = UnifiedScheduler::new();
+     let scheduler = Arc::new(scheduler);
+     // 将已注册的节点注入 scheduler
+     for (id, node) in &self.node_manager.nodes {
+         scheduler.register_node(id, node).await;
+     }
+     // 启动调度循环
+     let sched_handle = tokio::spawn({
+         let s = scheduler.clone();
+         async move { s.run().await }
+     });
```

### Step 2: API 入口集成 Priority → Scheduler（~1小时）

**文件**: `crates/jcode-enterprise-server/src/admin_api/openai_routes.rs`

```diff
  async fn chat_completions_handler(
      State(state): State<Arc<EnterpriseServerState>>,
      headers: HeaderMap,
      Json(req): Json<ChatCompletionRequest>,
  ) -> Result<Json<Value>, StatusCode> {
+     // 1. 评估优先级
+     let role = extract_role(&headers);
+     let priority = state.priority_engine.evaluate(role, &req.model, "chat");
+     
+     // 2. 提交到 UnifiedScheduler
+     let task = state.scheduler.submit_task(TaskRequest {
+         model: req.model.clone(),
+         priority: priority.into(),
+         messages: req.messages.clone(),
+     }).await;
+     
+     // 3. 等待调度结果（节点分配完成）
+     let assignment = state.scheduler.wait_for_assignment(task.id).await?;
+     
+     // 4. 路由到目标节点执行推理
+     let result = execute_on_node(&assignment, &req).await?;
+     Ok(Json(result))
  }
```

### Step 3: Parallax 层分配 → 实际推理路径（~3小时）

**文件**: `crates/jcode-enterprise-server/src/distributed.rs`

```diff
  pub async fn route_request(
      &self,
      model: &str,
      num_layers: u32,
  ) -> Result<InferenceRoute> {
+     // Phase 1: 水填算法分配层到节点
+     let layer_assignment = self.allocate_model_layers(model, num_layers);
+     
+     // Phase 2: 选择最优执行节点
+     let target = self.request_router.select_best_node(
+         &self.node_manager,
+         &layer_assignment,  // 新增参数: 层分配结果
+     ).await;
+     
+     // Phase 3: 为 KV Cache 建立 mmap 区域
+     if let Some(vmm) = &self.virtual_memory {
+         vmm.create_kv_cache_mmap(model, required_cache_size(num_layers)).await;
+     }
+     
+     Ok(InferenceRoute { target, layer_assignment })
  }
```

### Step 4: CpuInferenceEngine 启动预热（~1小时）

```diff
  // enterprise.rs serve()
- // cpu_engine: None
+ let cpu_engine = CpuInferenceEngine::new(config.cpu_inference.clone());
+ for model in &config.models {
+     cpu_engine.start_model(model).await?;
+ }
```

## 风险

| 风险 | 概率 | 缓解 |
|------|------|------|
| UnifiedScheduler 的 task 类型与 enterprise API 不匹配 | 中 | 先跑通最小路径（同步请求 bypass 队列） |
| 层分配后实际推理无法利用分布式的层 | 高 | Phase 1 先从单节点跑通，不要求跨节点流水线 |
| mmap KV Cache 与 llama.cpp 的实际行为不一致 | 中 | 第一次只做文件预分配，不要求生效 |

## 验证方式

```bash
# Step 1 验证: UnifiedScheduler 启动不报错
cargo check -p jcode-enterprise-server

# Step 2 验证: API 返回带优先级的响应头
curl -v http://localhost:8000/v1/chat/completions

# Step 3 验证: 日志显示层分配结果
grep "layer_assignment" /var/log/carpai.log
```
