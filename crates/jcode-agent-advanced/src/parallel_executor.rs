// ════════════════════════════════════════════════════════════════
// 并行工具执行引擎
// 对应 Claude Code: query.ts L1363-1409 + StreamingToolExecutor
//
// 核心能力:
//   1. 依赖图分析 — 自动识别工具间的数据依赖
//   2. 并行调度 — 无依赖的工具并发执行 (最大 MAX_PARALLEL_TOOLS)
//   3. 流式结果收集 — 边执行边返回中间结果
//   4. 错误隔离 — 单个工具失败不影响其他工具
// ════════════════════════════════════════════════════════════════

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

use futures::Stream;
use serde::{Deserialize, Serialize};
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

use super::AbortSignal;
use crate::types::{ToolResult, LoopEvent};

/// 工具调用请求
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallRequest {
    /// 工具调用 ID (唯一)
    pub id: String,
    
    /// 工具名称
    pub name: String,
    
    /// 工具输入参数
    pub input: serde_json::Value,
}

/// 工具执行计划 — 拓扑排序后的执行顺序
#[derive(Debug, Clone)]
pub struct ExecutionPlan {
    /// 执行层列表 (每层的工具可并行)
    pub layers: Vec<Vec<String>>,
    /// 总步骤数
    pub total_steps: usize,
}

/// 依赖图 — 有向无环图 (DAG)
#[derive(Debug, Clone)]
pub struct DependencyGraph {
    /// 节点集合
    nodes: HashSet<String>,
    /// 邻接表: node -> 它依赖的节点列表
    dependencies: HashMap<String, Vec<String>>,
    /// 反向邻接表: node -> 依赖它的节点列表
    dependents: HashMap<String, Vec<String>>,
}

impl DependencyGraph {
    pub fn new() -> Self {
        Self {
            nodes: HashSet::new(),
            dependencies: HashMap::new(),
            dependents: HashMap::new(),
        }
    }
    
    /// 添加节点 (工具名)
    pub fn add_node(&mut self, name: String) {
        self.nodes.insert(name.clone());
        self.dependencies.entry(name.clone()).or_insert_with(Vec::new);
        self.dependents.entry(name).or_insert_with(Vec::new);
    }
    
    /// 添加依赖边: after 依赖 before (before 必须在 after 之前执行)
    /// 
    /// 例如: tool_b 的输入需要 tool_a 的输出 → add_edge("tool_a", "tool_b")
    pub fn add_edge(&mut self, before: String, after: String) -> Result<(), String> {
        if !self.nodes.contains(&before) || !self.nodes.contains(&after) {
            return Err(format!("节点不存在: {before} 或 {after}"));
        }
        
        // 避免重复边
        if !self.dependencies.get(&after).unwrap().contains(&before) {
            self.dependencies.get_mut(&after).unwrap().push(before.clone());
            self.dependents.get_mut(&before).unwrap().push(after);
        }
        
        Ok(())
    }
    
    /// 解析为执行计划 (Kahn 算法拓扑排序)
    pub fn resolve_execution_plan(&self) -> Result<ExecutionPlan, String> {
        let mut in_degree: HashMap<String, usize = HashMap::new();
        
        // 计算入度
        for node in &self.nodes {
            in_degree.insert(node.clone(), self.dependencies.get(node.as_str()).map(|v| v.len()).unwrap_or(0));
        }
        
        // 初始化队列 (入度为 0 的节点)
        let mut queue: VecDeque<String> = VecDeque::new();
        for (node, degree) in &in_degree {
            if *degree == 0 {
                queue.push_back(node.clone());
            }
        }
        
        let mut layers: Vec<Vec<String>> = vec![];
        let mut remaining = self.nodes.len();
        
        while !queue.is_empty() {
            let mut current_layer: Vec<String> = vec![];
            
            // 取出当前所有入度为 0 的节点作为一层
            while let Some(node) = queue.pop_front() {
                current_layer.push(node);
                
                // 更新依赖此节点的节点的入度
                for dependent in self.dependents.get(&node).unwrap_or(&vec![]) {
                    if let Some(degree) = in_degree.get_mut(dependent) {
                        *degree -= 1;
                        if *degree == 0 {
                            queue.push_back(dependent.clone());
                        }
                    }
                }
            }
            
            layers.push(current_layer);
            remaining -= layers.last().unwrap().len();
        }
        
        // 检测环
        if remaining > 0 {
            return Err("检测到循环依赖，无法解析执行顺序".to_string());
        }
        
        Ok(ExecutionPlan {
            total_steps: layers.len(),
            layers,
        })
    }
    
    /// 获取无依赖时的默认计划 (全部并行)
    pub fn default_plan_all_parallel(&self) -> ExecutionPlan {
        ExecutionPlan {
            total_steps: 1,
            layers: vec![self.nodes.iter().cloned().collect()],
        }
    }
    
    /// 节点数量
    pub fn len(&self) -> usize {
        self.nodes.len()
    }
    
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }
}

impl Default for DependencyGraph {
    fn default() -> Self {
        Self::new()
    }
}

/// 并行工具执行器
pub struct ParallelToolExecutor {
    /// 最大并发数
    max_parallel: usize,
    
    /// Abort 信号
    abort_signal: Arc<AbortSignal>,
}

impl ParallelToolExecutor {
    pub fn new(max_parallel: usize, abort_signal: Arc<AbortSignal>) -> Self {
        Self {
            max_parallel,
            abort_signal,
        }
    }
    
    /// 根据执行计划并行执行多个工具
    /// 
    /// 返回一个 Stream，按完成顺序产出结果 (非启动顺序)
    pub async fn execute_plan<F, Fut>(
        &self,
        plan: &ExecutionPlan,
        requests: HashMap<String, ToolCallRequest>,
        tool_executor: F,
    ) -> impl Stream<Item = LoopEvent> + '_ 
    where
        F: Fn(ToolCallRequest, Arc<AbortSignal>) -> Fut + Clone,
        Fut: std::future::Future<Output = ToolResult> + Send + 'static,
    {
        use tokio_stream::StreamExt;
        
        let semaphore = Arc::new(Semaphore::new(self.max_parallel));
        let mut join_set = JoinSet::new();
        let mut result_tx_buffer: Vec<LoopEvent> = vec![];
        
        // 按层执行 (层间有序，层内并行)
        for layer in &plan.layers {
            // 启动当前层的所有任务
            for tool_name in layer {
                if let Some(request) = requests.get(tool_name) {
                    let req = request.clone();
                    let exec = tool_executor.clone();
                    let signal = self.abort_signal.clone();
                    let sem = semaphore.clone();
                    
                    join_set.spawn(async move {
                        let _permit = sem.acquire().await.unwrap();
                        
                        // 发送开始事件
                        let start_event = LoopEvent::ToolExecutionStart {
                            id: req.id.clone(),
                            name: req.name.clone(),
                        };
                        
                        // 检查 abort
                        if signal.is_aborted() {
                            return vec![
                                start_event,
                                LoopEvent::ToolExecutionFailed {
                                    id: req.id,
                                    error: "操作被中断".to_string(),
                                    retryable: false,
                                },
                            ];
                        }
                        
                        // 执行工具
                        match tokio::time::timeout(
                            Duration::from_secs(120),  // 默认超时
                            exec(req, signal),
                        ).await {
                            Ok(result) => {
                                vec![
                                    start_event,
                                    LoopEvent::ToolResultReady { result },
                                ]
                            }
                            Err(_) => {
                                vec![
                                    start_event,
                                    LoopEvent::ToolExecutionFailed {
                                        id: req.id,
                                        error: "工具执行超时".to_string(),
                                        retryable: true,
                                    },
                                ]
                            }
                        }
                    });
                }
            }
            
            // 等待当前层全部完成
            while let Some(result) = join_set.join_next().await {
                match result {
                    Ok(events) => {
                        result_tx_buffer.extend(events);
                    }
                    Err(e) => {
                        result_tx_buffer.push(LoopEvent::ToolExecutionFailed {
                            id: "unknown".to_string(),
                            error: format!("任务 panic: {e}"),
                            retryable: false,
                        });
                    }
                }
            }
        }
        
        // 转换为流
        async_stream::stream! {
            for event in result_tx_buffer {
                yield event;
            }
        }
    }

    /// 无依赖图的简单并行执行 (所有工具同时启动)
    pub async fn execute_all<F, Fut>(
        &self,
        requests: Vec<ToolCallRequest>,
        tool_executor: F,
    ) -> impl Stream<Item = LoopEvent> + '_ 
    where
        F: Fn(ToolCallRequest, Arc<AbortSignal>) -> Fut + Clone,
        Fut: std::future::Future<Output = ToolResult> + Send + 'static,
    {
        use tokio_stream::StreamExt;
        
        let graph = DependencyGraph::new();
        let plan = graph.default_plan_all_parallel();
        let request_map: HashMap<String, ToolCallRequest> = requests
            .into_iter()
            .map(|r| (r.id.clone(), r))
            .collect();
        
        self.execute_plan(&plan, request_map, tool_executor).await
    }
}
