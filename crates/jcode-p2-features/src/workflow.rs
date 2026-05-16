// ════════════════════════════════════════════════════════════════
// 工作流脚本引擎 — 自定义自动化工作流
//
// 支持的工作流格式 (.workflow 文件):
//
//   name: "My Workflow"
//   steps:
//     - name: "Check git status"
//       action: shell
//       command: "git status"
//       on_error: continue
//
//     - name: "Run tests"
//       action: tool_call
//       tool: "Bash"
//       input: { cmd: "npm test" }
//       depends_on: ["check-git"]
//
//     - name: "Notify"
//       action: notification
//       channel: email
//       template: "Build {{status}} in {{duration}}s"
//     depends_on: [run-tests]
// ════════════════════════════════════════════════════════════════

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// 工作流步骤动作类型
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ActionType {
    Shell,           // 执行 Shell 命令
    ToolCall,        // 调用工具
    Notification,    // 发送通知
    Condition,        // 条件分支
    Parallel,         // 并行执行一组子步骤
    SubWorkflow,      // 嵌套子工作流
    WaitForInput,     // 等待用户输入
}

/// 单个工作流步骤
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowStep {
    pub id: String,
    pub name: String,
    pub action: ActionType,
    
    // -- 动作参数 (根据 action 类型选择使用) --
    /// Shell 命令
    pub command: Option<String>,
    /// 工具调用参数
    pub tool_name: Option<String>,
    pub tool_input: Option<serde_json::Value>,
    /// 条件表达式
    pub condition: Option<String>,
    /// 并行子步骤
    pub parallel_steps: Option<Vec<WorkflowStep>>,
    /// 子工作流引用
    pub workflow_ref: Option<String>,
    /// 通知配置
    pub channel: Option<String>,
    pub template: Option<String>,

    /// 依赖的其他步骤 ID
    pub depends_on: Vec<String>,

    /// 错误处理策略
    pub on_error: ErrorStrategy,

    /// 超时 (秒)
    pub timeout_secs: Option<u64>,

    /// 重试次数
    pub retry_count: u32,

    /// 是否跳过
    pub skip: bool,

    /// 自定义元数据
    pub metadata: HashMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ErrorStrategy {
    Fail,           // 失败则整个工作流终止
    Continue,       // 跳过此步继续
    Retry,          // 重试 (配合 retry_count)
    Fallback { fallback_step_id: String }, // 回退到指定步骤
}

/// 工作流定义
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowDefinition {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub version: String,
    pub author: Option<String>,
    
    /// 全局变量
    pub variables: HashMap<String, serde_json::Value>,
    
    /// 有序步骤列表
    pub steps: Vec<WorkflowStep>,
    
    /// 全局错误处理
    pub on_error: ErrorStrategy,
    
    /// 全局超时
    pub timeout_secs: Option<u64>,
    
    /// 标签 (用于搜索/分类)
    pub tags: Vec<String>,
}

/// 工作流执行结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowResult {
    pub workflow_id: String,
    pub success: bool,
    
    /// 每步结果
    pub step_results: HashMap<String, StepResult>,
    
    /// 最终输出变量
    pub output_variables: HashMap<String, serde_json::Value>,
    
    /// 总耗时 (ms)
    pub duration_ms: u64,
    
    /// 错误信息 (如果失败)
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepResult {
    pub step_id: String,
    pub success: bool,
    pub output: Option<serde_json::Value>,
    pub duration_ms: u64,
    pub error: Option<String>,
    pub retries: u32,
}

/// 工作流上下文 (运行时状态)
pub struct WorkflowContext {
    /// 变量存储 (支持模板替换)
    pub variables: HashMap<String, serde_json::Value>,
    
    /// 已完成的步骤
    pub completed_steps: HashSet<String>,
    
    /// 当前正在执行的步骤
    pub running_steps: HashSet<String>,
    
    /// 步骤结果缓存
    pub step_results: HashMap<String, StepResult>,
    
    /// 取消信号
    pub cancelled: tokio::sync::watch::Sender<bool>,
}

impl WorkflowContext {
    pub fn new(variables: HashMap<String, serde_json::Value>) -> Self {
        let (tx, _rx) = tokio::sync::watch::channel(false);
        Self {
            variables,
            completed_steps: HashSet::new(),
            running_steps: HashSet::new(),
            step_results: HashMap::new(),
            cancelled: tx,
        }
    }

    /// 渲染模板字符串 ({{variable}} 替换)
    pub fn render_template(&self, template: &str) -> String {
        let mut result = template.to_string();
        
        for (key, value) in &self.variables {
            let placeholder = format!("{{{{{}}}}}", key);
            let replacement = match value {
                serde_json::Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            result = result.replace(&placeholder, &replacement);
        }

        result
    }

    /// 设置变量
    pub fn set_var(&mut self, key: impl Into<String>, value: serde_json::Value) {
        self.variables.insert(key.into(), value);
    }

    /// 获取变量
    pub fn get_var(&self, key: &str) -> Option<&serde_json::Value> {
        self.variables.get(key)
    }
}

/// 工作流引擎
pub struct WorkflowEngine {
    registered_workflows: HashMap<String, WorkflowDefinition>,
    execution_history: Vec<WorkflowResult>,
}

impl Default for WorkflowEngine {
    fn default() -> Self { Self::new() }
}

impl WorkflowEngine {
    pub fn new() -> Self {
        Self {
            registered_workflows: HashMap::new(),
            execution_history: Vec::new(),
        }
    }

    /// 注册工作流定义
    pub fn register_workflow(&mut self, def: WorkflowDefinition) {
        tracing::info!(name = %def.name, "Workflow registered");
        self.registered_workflows.insert(def.id.clone(), def);
    }

    /// 从 YAML/TOML 字符串加载工作流
    pub fn load_from_str(&mut self, content: &str) -> Result<WorkflowDefinition, String> {
        // 尝试解析为 TOML 或 JSON
        if let Ok(def) = toml::from_str::<WorkflowDefinition>(content) {
            self.register_workflow(def.clone());
            return Ok(def);
        }
        if let Ok(def) = serde_json::from_str::<WorkflowDefinition>(content) {
            self.register_workflow(def.clone());
            return Ok(def);
        }
        Err("Failed to parse workflow definition".into())
    }

    /// 执行工作流
    pub async fn execute(
        &mut self,
        workflow_id: &str,
        initial_vars: Option<HashMap<String, serde_json::Value>>,
    ) -> Result<WorkflowResult, String> {
        let def = self.registered_workflows.get(workflow_id)
            .ok_or(format!("Workflow '{}' not found", workflow_id))?
            .clone();

        let start = std::time::Instant::now();
        let mut ctx = WorkflowContext::new(initial_vars.unwrap_or_default());

        let result = self.execute_workflow(&def, &mut ctx).await;

        let duration_ms = start.elapsed().as_millis() as u64;
        
        let final_result = match result {
            Ok(steps_map) => WorkflowResult {
                workflow_id: workflow_id.to_string(),
                success: true,
                step_results: steps_map,
                output_variables: ctx.variables,
                duration_ms,
                error: None,
            },
            Err(e) => WorkflowResult {
                workflow_id: workflow_id.to_string(),
                success: false,
                step_results: ctx.step_results,
                output_variables: ctx.variables,
                duration_ms,
                error: Some(e),
            },
        };

        self.execution_history.push(final_result.clone());
        Ok(final_result)
    }

    async fn execute_workflow(
        &self,
        def: &WorkflowDefinition,
        ctx: &mut WorkflowContext,
    ) -> Result<HashMap<String, StepResult>, String> {
        let mut results = HashMap::new();

        for step in &def.steps {
            if step.skip {
                results.insert(step.id.clone(), StepResult {
                    step_id: step.id.clone(),
                    success: true,
                    output: None,
                    duration_ms: 0,
                    error: None,
                    retries: 0,
                });
                continue;
            }

            // 检查依赖是否满足
            for dep in &step.depends_on {
                if !ctx.completed_steps.contains(dep) {
                    if let Some(prev_result) = ctx.step_results.get(dep) {
                        if !prev_result.success {
                            return Err(format!(
                                "Dependency '{}' failed, skipping '{}'",
                                dep, step.name
                            ));
                        }
                    }
                }
            }

            // 执行步骤
            let step_result = self.execute_step(step, ctx).await;
            
            match step_result {
                Ok(sr) => {
                    ctx.completed_steps.insert(step.id.clone());
                    ctx.step_results.insert(step.id.clone(), sr);
                    let output_ref = ctx.step_results.get(&step.id).unwrap();
                    if let Some(output) = &output_ref.output {
                        ctx.set_var(format!("{}.output", step.name.replace(' ', "_").to_lowercase()), output.clone());
                    }
                }
                Err(_) => {
                    match &step.on_error {
                        ErrorStrategy::Fail => {
                            return Err(format!("Step '{}' failed", step.name));
                        }
                        ErrorStrategy::Continue => {}
                        ErrorStrategy::Retry => {} // TODO: 实现重试逻辑
                        ErrorStrategy::Fallback { fallback_step_id } => {
                            // TODO: 执行回退步骤
                        }
                    }
                }
            }

            // Err 分支已处理, Ok 分支也已处理 (含 step_results + set_var)
        }

        Ok(results)
    }

    async fn execute_step(
        &self,
        step: &WorkflowStep,
        _ctx: &WorkflowContext,
    ) -> Result<StepResult, String> {
        let start = std::time::Instant::now();

        match step.action {
            ActionType::Shell => {
                if let Some(cmd) = &step.command {
                    let rendered_cmd = _ctx.render_template(cmd);
                    
                    // TODO: 通过 CommandSandbox 安全检查后执行
                    // let output = execute_shell_command(&rendered_cmd, step.timeout_secs).await?;
                    
                    Ok(StepResult {
                        step_id: step.id.clone(),
                        success: true,
                        output: Some(serde_json::json!({ "stdout": "Command executed" })),
                        duration_ms: start.elapsed().as_millis() as u64,
                        error: None,
                        retries: 0,
                    })
                } else {
                    Err("Shell step has no command".into())
                }
            }
            ActionType::ToolCall => {
                // TODO: 通过工具系统调用
                Ok(StepResult {
                    step_id: step.id.clone(),
                    success: true,
                    output: None,
                    duration_ms: start.elapsed().as_millis() as u64,
                    error: None,
                    retries: 0,
                })
            }
            ActionType::Condition => {
                // TODO: 表达式求值
                Ok(StepResult {
                    step_id: step.id.clone(),
                    success: true,
                    output: Some(serde_json::json!({"condition_met": true})),
                    duration_ms: start.elapsed().as_millis() as u64,
                    error: None,
                    retries: 0,
                })
            }
            ActionType::Notification => {
                // TODO: 发送通知
                tracing::info!(template = ?&step.template, "Workflow notification");
                
                Ok(StepResult {
                    step_id: step.id.clone(),
                    success: true,
                    output: None,
                    duration_ms: start.elapsed().as_millis() as u64,
                    error: None,
                    retries: 0,
                })
            }
            _ => {
                Err(format!("Action type {:?} not yet implemented", step.action))
            }
        }
    }

    /// 列出已注册的工作流
    pub fn list_workflows(&self) -> Vec<&WorkflowDefinition> {
        self.registered_workflows.values().collect()
    }

    /// 获取执行历史
    pub fn history(&self) -> &[WorkflowResult] {
        &self.execution_history
    }
}
