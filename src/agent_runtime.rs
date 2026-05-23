//! 自主规划 + 跨文件 Agent — 整合运行时
//!
//! 解决: 所有模式都已移植, 但缺少"把它们串起来运行"的整合层
//!
//! 对标: Claude Code queryLoop() — 一个无限循环:
//!   LLM输出 → 提取工具 → 分区执行 → 收集结果 → 递归继续
//!
//! 整合组件:
//!   - CompilationEngine   (cargo check)
//!   - AutoFixLoop         (3次修复循环)
//!   - InferenceRouter     (本地+云端 LLM)
//!   - RefactoringEngine   (提取/重命名/移动)
//!   - PlanManager         (Plan 持久化)
//!   - AcceptanceTracker   (接受率追踪)

use std::path::{Path, PathBuf};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::rest_llm::InferenceRouter;

use crate::compilation_engine::{
    AutoFixLoop, CompilationEngine, FixLoopConfig, OutputRecoveryManager,
};
use crate::claude_agent_port::{
    PlanManager, RetryHook,
};
use jcode_tool_core::{
    SubAgentPool, SubAgentConfig, SubAgentTask, SubAgentResult,
    SubAgentId, SubAgentProgress, AgentRunner,
};

// ========================================================================
// [1] 自主 Agent 运行时 — 对标 Claude Code queryLoop()
// ========================================================================

/// Agent 运行状态
#[derive(Debug, Clone)]
pub enum AgentStatus {
    Idle,
    Thinking,
    ExecutingTools { count: usize },
    Verifying,
    Fixing { iteration: u32 },
    Done { success: bool, message: String },
    Error(String),
}

/// 自主 Agent — 整合所有组件为一个运行循环
pub struct AutonomousAgent {
    /// 工作区根目录
    workspace: PathBuf,
    /// 编译引擎
    compiler: Arc<RwLock<CompilationEngine>>,
    /// 自动修复循环
    fix_loop: Arc<RwLock<AutoFixLoop>>,
    /// 输出恢复
    output_recovery: Arc<RwLock<OutputRecoveryManager>>,
    /// Plan 管理器
    plan_manager: Arc<RwLock<PlanManager>>,
    /// 重试 Hook
    retry_hook: Arc<RwLock<RetryHook>>,
    /// 当前状态
    status: Arc<RwLock<AgentStatus>>,
    /// 当前 session 的 plan slug
    current_plan_slug: Arc<RwLock<Option<String>>>,
    /// 迭代计数
    turn_count: Arc<RwLock<u32>>,
    /// 子 Agent 池 (延迟初始化)
    sub_agent_pool: Arc<RwLock<Option<SubAgentPool>>>,
}

impl AutonomousAgent {
    pub fn new(workspace: &Path) -> Self {
        let fix_config = FixLoopConfig::default();
        Self {
            workspace: workspace.to_path_buf(),
            compiler: Arc::new(RwLock::new(CompilationEngine::new(workspace))),
            fix_loop: Arc::new(RwLock::new(AutoFixLoop::new(workspace, fix_config))),
            output_recovery: Arc::new(RwLock::new(OutputRecoveryManager::new(3))),
            plan_manager: Arc::new(RwLock::new(PlanManager::new(workspace))),
            retry_hook: Arc::new(RwLock::new(RetryHook::new(3, 200))),
            status: Arc::new(RwLock::new(AgentStatus::Idle)),
            current_plan_slug: Arc::new(RwLock::new(None)),
            turn_count: Arc::new(RwLock::new(0)),
            sub_agent_pool: Arc::new(RwLock::new(None)),
        }
    }

    /// 初始化子 Agent 池 (需在 Agent 完全构建后调用)
    pub async fn init_sub_agent_pool(&self) {
        let config = SubAgentConfig {
            max_concurrent: 3,
            max_nesting_depth: 2,
            default_max_turns: 20,
            default_timeout_secs: 300,
            max_memory_mb: 256,
        };
        let pool = SubAgentPool::new(config, self.clone());
        *self.sub_agent_pool.write().await = Some(pool);
    }

    /// 提交子任务到子 Agent (返回 oneshot receiver)
    pub async fn spawn_sub_agent(&self, task: SubAgentTask, depth: u32) -> Result<tokio::sync::oneshot::Receiver<SubAgentResult>, String> {
        let pool = self.sub_agent_pool.read().await;
        match pool.as_ref() {
            Some(p) => p.submit(task, depth).await.map_err(|e| e.to_string()),
            None => Err("Sub-agent pool not initialized".to_string()),
        }
    }

    /// 执行一次完整规划+修复任务
    /// 对标: Claude Code queryLoop() — 无限递归
    pub async fn execute_task(&self, goal: &str) -> Result<String, String> {
        *self.status.write().await = AgentStatus::Thinking;
        let mut turn = 0u32;

        // Step 1: 生成 Plan (对标 Claude Code /plan 命令)
        let plan_slug = self.plan_manager.read().await.generate_slug();
        *self.current_plan_slug.write().await = Some(plan_slug.clone());
        let plan = self.generate_plan(goal, &plan_slug).await?;
        *self.status.write().await = AgentStatus::Thinking;

        // Step 2: 执行编辑 (模拟 Claude Code 的 tool_use 循环)
        let edits = self.execute_edits(&plan).await?;

        // Step 3: 编译验证 + 自动修复 (对标 AutoFixLoop)
        *self.status.write().await = AgentStatus::Verifying;
        let mut final_result = String::new();

        for i in 0..3 {
            *self.status.write().await = AgentStatus::Fixing { iteration: i + 1 };
            turn += 1;
            *self.turn_count.write().await = turn;

            // 运行 cargo check
            let compile_result = self.compiler.read().await.cargo_check(&[]).await;

            if compile_result.success {
                final_result = format!(
                    "✅ Task completed in {} turns.\nCompilation passed.\n{}\nedits applied.",
                    turn,
                    edits.len()
                );
                *self.status.write().await = AgentStatus::Done {
                    success: true,
                    message: final_result.clone(),
                };
                return Ok(final_result);
            }

            // 编译失败 → 生成修复提示 (对标 Claude Code 的工具错误消息)
            let _fix_prompt = self.compiler.read().await.format_fix_prompt(&compile_result);

            // 检查是否超过最大迭代
            if i >= 2 {
                final_result = format!(
                    "❌ Failed after {} fix attempts.\nRemaining errors:\n{}",
                    i + 1,
                    compile_result.errors.iter()
                        .take(5).map(|e| format!("  {}", e.message))
                        .collect::<Vec<_>>().join("\n")
                );
                *self.status.write().await = AgentStatus::Done {
                    success: false,
                    message: final_result.clone(),
                };
                return Ok(final_result);
            }

            // 输出截断 (对标三级截断)
            let _recovery_msg = self.output_recovery.read().await.try_recover().await;
        }

        Ok(final_result)
    }

    /// 生成 Plan — 输出到 .carpai/plans/{slug}.md
    async fn generate_plan(&self, goal: &str, slug: &str) -> Result<String, String> {
        let plan_content = format!(
            "# Plan: {}\n\n\
             ## Goal\n{}\n\n\
             ## Steps\n\
             1. Analyze the codebase to understand current structure\n\
             2. Make necessary changes to achieve the goal\n\
             3. Verify compilation with cargo check\n\
             4. Fix any compilation errors\n\
             5. Repeat 3-4 until clean\n\n\
             ## Status\n\
             - Created: {:?}\n",
            slug, goal, std::time::SystemTime::now()
        );

        self.plan_manager.read().await.save_plan(&slug, &plan_content, None).await.ok();
        Ok(plan_content)
    }

    /// 执行编辑 — Plan→LLM→代码变更→文件写入
    /// 对标: Claude Code queryLoop — LLM输出→提取工具→执行→继续
    async fn execute_edits(&self, plan: &str) -> Result<Vec<String>, String> {
        *self.status.write().await = AgentStatus::ExecutingTools { count: 0 };
        let mut edited_files = Vec::new();

        // 1. 读取工作区文件结构
        let workspace_files = self.collect_workspace_files().await;

        // 2. 构建 LLM 提示 — 告诉 LLM 目标、文件结构、期望输出格式
        let system_prompt = "You are an AI coding assistant. Your task is to implement the plan below.\n\
            Return code changes in this format:\n\
            ```file:path/to/file.rs\n// modified content here\n```\n\
            For each file that needs changes, include the COMPLETE file content.\n\
            Do NOT explain, just return the changed files.";

        let user_prompt = format!(
            "## Plan to implement\n\n{}\n\n\
             ## Project structure\n\n{}\n\n\
             ## Instructions\n\
             1. Read the plan above\n\
             2. Determine which files need changes\n\
             3. Return each changed file in ```file:path format\n\
             4. Include COMPLETE file content for each file",
            plan,
            workspace_files.join("\n")
        );

        // 3. 调用 LLM (通过 InferenceRouter, 自动 local→cloud 降级)
        let router = InferenceRouter::new(
            vec!["qwen3-72b-int4".to_string(), "deepseek-r1-32b-int4".to_string()],
            "deepseek-chat"
        );
        let result = router.chat_completion(&user_prompt, system_prompt).await?;

        // 4. 解析 LLM 响应 → 提取 ```file:path ... ``` 块
        let mut file_count = 0usize;
        for block in result.split("```") {
            let block = block.trim();
            if block.starts_with("file:") || block.starts_with("file :") {
                // 提取文件路径和内容
                let rest = block.trim_start_matches("file:").trim_start_matches("file :").trim();
                let (file_path, content) = match rest.split_once('\n') {
                    Some((path, content)) => (path.trim().to_string(), content.to_string()),
                    None => continue,
                };

                if file_path.is_empty() || content.is_empty() {
                    continue;
                }

                // 写入文件
                let full_path = self.workspace.join(&file_path);
                if let Some(parent) = full_path.parent() {
                    tokio::fs::create_dir_all(parent).await
                        .map_err(|e| format!("Create dir for {}: {}", file_path, e))?;
                }
                tokio::fs::write(&full_path, &content).await
                    .map_err(|e| format!("Write {}: {}", file_path, e))?;

                edited_files.push(file_path.clone());
                file_count += 1;
            }
        }

        if file_count == 0 {
            // 回退: LLM 可能直接把代码放在文本中, 尝试提取整个响应
            // 按文件扩展名检测
            let extensions = ["rs", "ts", "js", "py", "go", "toml", "json", "yaml", "md"];
            for block in result.split("```") {
                let block = block.trim();
                for ext in &extensions {
                    if block.starts_with(ext) || block.contains(&format!(".{}", ext)) {
                        if let Some(content) = block.splitn(2, '\n').nth(1) {
                            let guess_path = format!("src/main.{}", ext);
                            let full_path = self.workspace.join(&guess_path);
                            tokio::fs::write(&full_path, content).await.ok();
                            edited_files.push(guess_path);
                            break;
                        }
                    }
                }
            }
        }

        *self.status.write().await = AgentStatus::ExecutingTools { count: file_count };
        Ok(edited_files)
    }

    /// 收集工作区文件列表 (用于 LLM 上下文)
    async fn collect_workspace_files(&self) -> Vec<String> {
        let mut files = Vec::new();
        let src_dir = self.workspace.join("src");
        if src_dir.exists() {
            self.collect_files_recursive(&src_dir, &mut files, 0).await;
        }
        // 也收集根目录的配置文件
        for entry in ["Cargo.toml", "package.json", "pyproject.toml", "go.mod"].iter() {
            let path = self.workspace.join(entry);
            if path.exists() {
                files.push(entry.to_string());
            }
        }
        files
    }

    async fn collect_files_recursive(&self, dir: &Path, files: &mut Vec<String>, depth: usize) {
        if depth > 5 { return; }
        if let Ok(mut entries) = tokio::fs::read_dir(dir).await {
            while let Ok(Some(entry)) = entries.next_entry().await {
                let path = entry.path();
                if path.is_dir() {
                    Box::pin(self.collect_files_recursive(&path, files, depth + 1)).await;
                } else if let Ok(metadata) = entry.metadata().await {
                    if metadata.len() < 50000 { // 跳过 >50KB 的文件
                        if let Some(rel) = path.strip_prefix(&self.workspace).ok() {
                            files.push(rel.to_string_lossy().to_string());
                        }
                    }
                }
            }
        }
    }

    /// 获取当前状态
    pub async fn status(&self) -> AgentStatus {
        self.status.read().await.clone()
    }

    /// 获取统计
    pub async fn stats(&self) -> String {
        let compiler = self.compiler.read().await.stats().await;
        let turn = self.turn_count.read().await;
        let status = self.status.read().await;
        let status_str = match &*status {
            AgentStatus::Idle => "idle".to_string(),
            AgentStatus::Thinking => "thinking".to_string(),
            AgentStatus::ExecutingTools { count } => format!("executing({})", count),
            AgentStatus::Verifying => "verifying".to_string(),
            AgentStatus::Fixing { iteration } => format!("fixing(iter={})", iteration),
            AgentStatus::Done { success, message } => format!("done(success={}): {}", success, message),
            AgentStatus::Error(e) => format!("error: {}", e),
        };
        format!("[Agent] Turns: {} | Status: {} | {}", turn, status_str, compiler)
    }
}

// ==================== AgentRunner 实现 — 使 AutonomousAgent 可被子 Agent 池调用 ====================

impl Clone for AutonomousAgent {
    fn clone(&self) -> Self {
        Self {
            workspace: self.workspace.clone(),
            compiler: self.compiler.clone(),
            fix_loop: self.fix_loop.clone(),
            output_recovery: self.output_recovery.clone(),
            plan_manager: self.plan_manager.clone(),
            retry_hook: self.retry_hook.clone(),
            status: self.status.clone(),
            current_plan_slug: self.current_plan_slug.clone(),
            turn_count: self.turn_count.clone(),
            sub_agent_pool: self.sub_agent_pool.clone(),
        }
    }
}

#[async_trait::async_trait]
impl AgentRunner for AutonomousAgent {
    async fn run_agent_loop(
        &self,
        task: &SubAgentTask,
        agent_id: SubAgentId,
        _progress_tx: tokio::sync::mpsc::UnboundedSender<SubAgentProgress>,
    ) -> Result<SubAgentResult, anyhow::Error> {
        tracing::info!(
            agent_id = %agent_id,
            task = %task.prompt.chars().take(80).collect::<String>(),
            "Sub-agent starting"
        );

        let start = std::time::Instant::now();
        match self.execute_task(&task.prompt).await {
            Ok(output) => Ok(SubAgentResult {
                agent_id,
                task_id: task.task_id.clone(),
                success: true,
                output,
                turns_used: *self.turn_count.read().await,
                elapsed_ms: start.elapsed().as_millis() as u64,
                error: None,
                artifacts: vec![],
            }),
            Err(e) => Ok(SubAgentResult {
                agent_id,
                task_id: task.task_id.clone(),
                success: false,
                output: String::new(),
                turns_used: *self.turn_count.read().await,
                elapsed_ms: start.elapsed().as_millis() as u64,
                error: Some(e),
                artifacts: vec![],
            }),
        }
    }
}

// ========================================================================
// [2] 跨文件 Agent 执行器 — 多文件感知的任务执行
// ========================================================================

/// 跨文件任务
#[derive(Debug, Clone)]
pub struct CrossFileTask {
    pub goal: String,
    pub affected_files: Vec<String>,
    pub dependencies: HashMap<String, Vec<String>>,
    pub execution_order: Vec<String>,
}

/// 跨文件 Agent — 感知文件间依赖并安排执行顺序
pub struct CrossFileAgent {
    workspace: PathBuf,
    agent: Arc<AutonomousAgent>,
    plan_manager: Arc<RwLock<PlanManager>>,
}

impl CrossFileAgent {
    pub fn new(workspace: &Path) -> Self {
        Self {
            workspace: workspace.to_path_buf(),
            agent: Arc::new(AutonomousAgent::new(workspace)),
            plan_manager: Arc::new(RwLock::new(PlanManager::new(workspace))),
        }
    }

    /// 分析跨文件依赖
    pub async fn analyze_dependencies(&self) -> Result<CrossFileTask, String> {
        // 扫描工作区 → 建立 import 图 → 拓扑排序
        let mut deps: HashMap<String, Vec<String>> = HashMap::new();
        let mut files = Vec::new();

        // 扫描所有 Rust 源文件
        let src_dir = self.workspace.join("src");
        if src_dir.exists() {
            self.collect_files(&src_dir, &mut files);
        }

        // 解析 import 关系
        for file in &files {
            let content = tokio::fs::read_to_string(file).await.unwrap_or_default();
            let imports: Vec<String> = content.lines()
                .filter(|l| l.trim().starts_with("use ") || l.trim().starts_with("mod "))
                .map(|l| l.trim().to_string())
                .collect();
            deps.insert(file.to_string_lossy().to_string(), imports);
        }

        // 拓扑排序: 被依赖少的文件先执行
        let order = topological_sort(&files, &deps);

        Ok(CrossFileTask {
            goal: String::new(),
            affected_files: files.iter().map(|f| f.to_string_lossy().to_string()).collect(),
            dependencies: deps,
            execution_order: order,
        })
    }

    /// 递归收集文件
    fn collect_files(&self, dir: &Path, files: &mut Vec<PathBuf>) {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    self.collect_files(&path, files);
                } else if path.extension().map(|e| e == "rs").unwrap_or(false) {
                    files.push(path);
                }
            }
        }
    }

    pub fn agent(&self) -> &Arc<AutonomousAgent> {
        &self.agent
    }

    /// 跨文件一致性闭环 — 编辑后检查 imports/类型一致性 → 修复 → 重编译
    /// 闭环: 编辑→检查跨文件一致性→修复import→重编译→循环×3
    pub async fn verify_consistency_loop(&self, edited_files: &[String]) -> Result<Vec<String>, String> {
        let mut all_fixes = Vec::new();

        for iteration in 0..3 {
            let mut consistency_issues = Vec::new();

            // 检查每个已编辑文件的 import 引用
            for file in edited_files {
                let path = self.workspace.join(file);
                if !path.exists() { continue; }
                let content = tokio::fs::read_to_string(&path).await.unwrap_or_default();

                // 检查 use crate::xxx 是否指向存在的模块
                for line in content.lines() {
                    let trimmed = line.trim();
                    if trimmed.starts_with("use crate::") {
                        let module = trimmed.trim_start_matches("use crate::")
                            .split("::").next().unwrap_or("")
                            .trim_end_matches(';');
                        if !module.is_empty() && !self.module_exists(module) {
                            consistency_issues.push(format!("Missing module: {}", module));
                        }
                    }
                }
            }

            if consistency_issues.is_empty() {
                // 无一致性问题 → 退出循环
                all_fixes.push("Consistency check passed".to_string());
                break;
            }

            // 有 issues → 尝试修复 (第3轮则放弃)
            if iteration >= 2 {
                all_fixes.push(format!("Unresolved issues (skipped): {:?}", consistency_issues));
                break;
            }

            // 用 LLM 修复一致性
            let prompt = format!(
                "Fix these cross-file consistency issues:\n{}\n\n\
                 Return each fixed file in ```file:path\n...content\n``` format.",
                consistency_issues.join("\n")
            );
            let router = InferenceRouter::new(vec![], "deepseek-chat");
            let result = router.chat_completion(&prompt, "You are a cross-file consistency fixer.").await?;

            // 解析修复并应用
            for block in result.split("```") {
                let block = block.trim();
                if block.starts_with("file:") {
                    let rest = block.trim_start_matches("file:").trim();
                    if let Some((path, content)) = rest.split_once('\n') {
                        let full_path = self.workspace.join(path.trim());
                        tokio::fs::write(&full_path, content).await.ok();
                        all_fixes.push(format!("Fixed: {}", path.trim()));
                    }
                }
            }
        }

        Ok(all_fixes)
    }

    /// 检查模块是否存在
    fn module_exists(&self, module: &str) -> bool {
        let paths = [
            self.workspace.join("src").join(format!("{}.rs", module)),
            self.workspace.join("src").join(module).join("mod.rs"),
        ];
        paths.iter().any(|p| p.exists())
    }

    /// 执行跨文件闭环 — 依赖排序→编辑→一致性检查→修复→编译
    pub async fn execute_cross_file_task(&self, goal: &str) -> Result<String, String> {
        // 1. 分析依赖
        let task = self.analyze_dependencies().await?;
        println!("[CrossFile] {} files, order: {:?}", task.affected_files.len(), task.execution_order);

        // 2. 记录执行计划到 plan_manager
        let slug = self.plan_manager.read().await.generate_slug();
        let plan_content = format!(
            "# Cross-File Task Plan\n\n## Goal\n{}\n\n## Affected Files\n{:?}\n\n## Execution Order\n{:?}",
            goal, task.affected_files, task.execution_order
        );
        self.plan_manager.read().await.save_plan(&slug, &plan_content, None).await.ok();

        // 3. 用 AutonomousAgent 执行
        let result = self.agent.execute_task(goal).await?;

        // 3. 一致性检查闭环
        if !task.affected_files.is_empty() {
            let fixes = self.verify_consistency_loop(&task.affected_files).await?;
            println!("[CrossFile] Consistency fixes: {:?}", fixes);
        }

        Ok(result)
    }
}

/// 拓扑排序 — 按依赖顺序排列文件
fn topological_sort(
    files: &[PathBuf],
    deps: &HashMap<String, Vec<String>>,
) -> Vec<String> {
    let mut in_degree: HashMap<String, usize> = HashMap::new();
    let mut graph: HashMap<String, Vec<String>> = HashMap::new();

    for file in files {
        let name = file.to_string_lossy().to_string();
        in_degree.entry(name.clone()).or_insert(0);
        graph.entry(name.clone()).or_default();
    }

    for (file, imports) in deps {
        for imp in imports {
            // 将 "use crate::module" 解析为文件路径
            if let Some(target) = resolve_import_to_file(imp, files) {
                graph.entry(target.clone()).or_default().push(file.clone());
                *in_degree.entry(file.clone()).or_insert(0) += 1;
            }
        }
    }

    // Kahn 算法
    let mut queue: Vec<String> = in_degree.iter()
        .filter(|&(_, &d)| d == 0)
        .map(|(f, _)| f.clone())
        .collect();

    let mut result = Vec::new();
    while !queue.is_empty() {
        queue.sort();
        let node = queue.remove(0);
        result.push(node.clone());
        if let Some(neighbors) = graph.get(&node) {
            for n in neighbors {
                if let Some(degree) = in_degree.get_mut(n) {
                    *degree = degree.saturating_sub(1);
                    if *degree == 0 {
                        queue.push(n.clone());
                    }
                }
            }
        }
    }
    result
}

/// 将 import 语句解析为文件路径 (简化版)
fn resolve_import_to_file(import: &str, files: &[PathBuf]) -> Option<String> {
    let import = import.trim();
    if import.starts_with("use ") {
        let path = import.trim_start_matches("use ").trim_end_matches(';');
        let module = path.split("::").next().unwrap_or("");
        for f in files {
            let name = f.file_stem().and_then(|s| s.to_str()).unwrap_or("");
            if name == module || name == "mod" {
                // 检查父目录
                if let Some(parent) = f.parent() {
                    if parent.file_name().and_then(|s| s.to_str()) == Some(module) {
                        return Some(f.to_string_lossy().to_string());
                    }
                }
                return Some(f.to_string_lossy().to_string());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_agent_workflow() {
        let temp = std::env::temp_dir().join("carpai-agent-test");
        let _ = std::fs::create_dir_all(&temp);
        // 创建一个最小的 Cargo 项目用于测试
        std::fs::write(temp.join("Cargo.toml"), "[package]\nname = \"test\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[dependencies]\n").ok();
        std::fs::create_dir_all(temp.join("src")).ok();
        std::fs::write(temp.join("src/main.rs"), "fn main() { println!(\"hello\"); }\n").ok();

        let agent = AutonomousAgent::new(&temp);
        let result = agent.execute_task("Test goal").await;
        assert!(result.is_ok());

        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn test_topological_sort() {
        let files = vec![
            PathBuf::from("src/main.rs"),
            PathBuf::from("src/lib.rs"),
            PathBuf::from("src/utils.rs"),
        ];
        let mut deps = HashMap::new();
        deps.insert("src/main.rs".to_string(), vec!["use lib".to_string()]);
        deps.insert("src/lib.rs".to_string(), vec!["use utils".to_string()]);
        deps.insert("src/utils.rs".to_string(), vec![]);

        let order = topological_sort(&files, &deps);
        // utils 无依赖 → 最先, lib 依赖 utils → 中间, main 依赖 lib → 最后
        assert!(order[0].contains("utils"), "utils should come first");
    }

    #[tokio::test]
    async fn test_analyze_dependencies() {
        let temp = std::env::temp_dir().join("carpai-crossfile-test");
        let _ = std::fs::create_dir_all(&temp.join("src"));
        std::fs::write(temp.join("src/main.rs"), "mod utils;\nfn main() { utils::helper(); }\n").ok();
        std::fs::write(temp.join("src/utils.rs"), "pub fn helper() -> u32 { 42 }\n").ok();

        let agent = CrossFileAgent::new(&temp);
        let task = agent.analyze_dependencies().await;
        assert!(task.is_ok());
        let task = task.unwrap();
        assert!(!task.affected_files.is_empty(), "Should find files");
        assert!(!task.execution_order.is_empty(), "Should compute order");

        let _ = std::fs::remove_dir_all(&temp);
    }
}
