//! Claude Code Agent 自主性 + 规划能力深度移植
//!
//! 移植源: D:\studying\Codecargo\opensource\claude_code_src\src
//!
//! 5大模式移植:
//! 1. 并发安全工具分区 (toolOrchestration.ts:91-116)
//! 2. 递归 Agent 循环 (query.ts:307-1728)
//! 3. Plan 文件化持久化 (plans.ts:79-231)
//! 4. 结构化错误注入 + Memory Hint (messages.ts:176-221)
//! 5. Auto-Retry via Hook (toolExecution.ts:1073-1101)

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::RwLock;
use tokio::time::sleep;

// ========================================================================
// [1] 并发安全工具分区 — 写独占、读共享
// 移植自: toolOrchestration.ts:91-116 + StreamingToolExecutor.ts:129-135
// ========================================================================

/// 工具并发安全等级
#[derive(Debug, Clone, PartialEq)]
pub enum ConcurrencySafety {
    /// 只读操作 — 可与其他只读操作并行
    ReadOnly,
    /// 写入操作 — 必须独占执行
    WriteExclusive,
}

/// 工具调用批次
#[derive(Debug, Clone)]
pub struct ToolBatch {
    /// 是否可以并发执行
    pub can_concur: bool,
    /// 批次中的工具调用
    pub tools: Vec<ToolCallInfo>,
}

/// 工具调用信息
#[derive(Debug, Clone)]
pub struct ToolCallInfo {
    pub name: String,
    pub input: serde_json::Value,
    pub safety: ConcurrencySafety,
}

/// 将工具调用分区为并发/串行批次
/// [Read, Read, Write, Read] → [{concurrent:[Read,Read]}, {serial:[Write]}, {concurrent:[Read]}]
pub fn partition_tool_calls(tools: Vec<ToolCallInfo>) -> Vec<ToolBatch> {
    let mut batches: Vec<ToolBatch> = Vec::new();

    for tool in tools {
        let is_safe = tool.safety == ConcurrencySafety::ReadOnly;

        // 检查是否可以追加到当前批次
        let can_append = batches.last().map(|b| {
            b.can_concur && is_safe
        }).unwrap_or(false);

        if can_append {
            batches.last_mut().unwrap().tools.push(tool);
        } else {
            batches.push(ToolBatch {
                can_concur: is_safe,
                tools: vec![tool],
            });
        }
    }

    batches
}

/// 判断工具是否为只读
pub fn tool_is_readonly(name: &str) -> bool {
    matches!(name,
        "read" | "list_files" | "search" | "grep" | "find" |
        "code_search" | "globbing" | "get_file_info" |
        "web_fetch" | "web_search" |
        "mcp__read" | "mcp__search" | "mcp__list"
    )
}

/// 判断工具是否可与其他只读工具并行
pub fn can_execute_concurrently(
    tool: &ToolCallInfo,
    executing_tools: &[&ToolCallInfo],
) -> bool {
    if executing_tools.is_empty() {
        return true;
    }
    // 只读工具可与其它只读工具并行
    if tool.safety == ConcurrencySafety::ReadOnly {
        return executing_tools.iter().all(|t| t.safety == ConcurrencySafety::ReadOnly);
    }
    // 写入工具必须独占
    false
}

// ========================================================================
// [2] 递归 Agent 循环模式
// 移植自: query.ts:307-1728 — queryLoop()
// ========================================================================

/// Agent 循环配置
#[derive(Debug, Clone)]
pub struct AgentLoopConfig {
    pub max_turns: u32,
    pub max_tool_calls_per_turn: u32,
    pub concurrency_limit: usize,
    pub enable_auto_retry: bool,
}

impl Default for AgentLoopConfig {
    fn default() -> Self {
        Self {
            max_turns: 25,
            max_tool_calls_per_turn: 10,
            concurrency_limit: 5,
            enable_auto_retry: true,
        }
    }
}

/// Agent 状态
#[derive(Debug, Clone)]
pub struct AgentState {
    pub turn: u32,
    pub messages: Vec<AgentMessage>,
    pub tool_results: Vec<ToolResult>,
    pub plan: Option<String>,
}

#[derive(Debug, Clone)]
pub struct AgentMessage {
    pub role: String,  // "user" | "assistant"
    pub content: String,
}

#[derive(Debug, Clone)]
pub struct ToolResult {
    pub tool_name: String,
    pub success: bool,
    pub output: String,
    pub error: Option<String>,
}

/// 递归 Agent 循环 — port of query.ts queryLoop()
/// 每次迭代: LLM输出 → 提取工具 → 分区执行 → 收集结果 → 递归继续
pub async fn agent_loop(
    initial_prompt: &str,
    config: &AgentLoopConfig,
    tool_executor: impl Fn(Vec<ToolCallInfo>) -> Vec<ToolResult>,
) -> Result<Vec<AgentMessage>, String> {
    let mut state = AgentState {
        turn: 0,
        messages: vec![AgentMessage {
            role: "user".to_string(),
            content: initial_prompt.to_string(),
        }],
        tool_results: vec![],
        plan: None,
    };

    loop {
        state.turn += 1;
        if state.turn > config.max_turns {
            break;
        }

        // Step 1: LLM 推理 → 生成工具调用 (模拟)
        let llm_output = simulate_llm_call(&state, config).await?;

        // Step 2: 从 LLM 输出中提取工具调用
        let tool_calls = extract_tool_calls(&llm_output);
        if tool_calls.is_empty() {
            // 无工具调用 → LLM 已给出最终回答
            state.messages.push(AgentMessage {
                role: "assistant".to_string(),
                content: llm_output,
            });
            break;
        }

        // Step 3: 分区 → 按批次执行
        let batches = partition_tool_calls(tool_calls);
        let mut all_results = Vec::new();
        for batch in batches {
            let batch_results = if batch.can_concur {
                // 并发执行
                tool_executor(batch.tools)
            } else {
                // 串行执行
                let mut results = Vec::new();
                for tool in batch.tools {
                    results.push(tool_executor(vec![tool]).into_iter().next().unwrap());
                }
                results
            };
            all_results.extend(batch_results);
        }

        // Step 4: 检查错误 → 注入 Memory Correction Hint
        let has_errors: Vec<&ToolResult> = all_results.iter().filter(|r| !r.success).collect();
        let memory_hint = if !has_errors.is_empty() {
            "\n\nNote: Some tools reported errors. Pay close attention to error messages and fix accordingly."
        } else {
            ""
        };

        // Step 5: 构建下一轮消息
        let mut next_prompt = String::new();
        for result in &all_results {
            if result.success {
                next_prompt.push_str(&format!("[{}] output:\n{}\n", result.tool_name, result.output));
            } else {
                let error_msg = result.error.as_deref().unwrap_or("unknown error");
                next_prompt.push_str(&format!(
                    "[{}] ERROR: {}{}\n",
                    result.tool_name, error_msg, memory_hint
                ));
            }
        }

        state.messages.push(AgentMessage {
            role: "assistant".to_string(),
            content: llm_output,
        });
        state.messages.push(AgentMessage {
            role: "user".to_string(),
            content: next_prompt,
        });
    }

    Ok(state.messages)
}

/// 模拟 LLM 调用 (实际项目中替换为真实的 Provider 调用)
async fn simulate_llm_call(state: &AgentState, config: &AgentLoopConfig) -> Result<String, String> {
    // 在真实实现中，这里调用 jcode-llm 的 Provider
    let _ = config;
    Ok(format!("[Turn {}] I analyzed the request...", state.turn))
}

/// 从 LLM 输出中提取工具调用 (简化版)
fn extract_tool_calls(_output: &str) -> Vec<ToolCallInfo> {
    // 在真实实现中，解析 tool_use XML 块或 JSON
    Vec::new()
}

// ========================================================================
// [3] Plan 文件化持久化 — slug + 多级恢复
// 移植自: plans.ts:79-231
// ========================================================================

/// Plan 管理器
pub struct PlanManager {
    plans_dir: PathBuf,
}

impl PlanManager {
    pub fn new(workspace_root: &Path) -> Self {
        let plans_dir = workspace_root.join(".carpai").join("plans");
        Self { plans_dir }
    }

    /// 生成唯一 word slug
    pub fn generate_slug(&self) -> String {
        let words = [
            "blue", "red", "green", "gold", "silver", "swift", "calm", "bold",
            "crisp", "dark", "dawn", "deep", "dusk", "fair", "fine", "glad",
            "holy", "keen", "kind", "lean", "lite", "mild", "neat", "pure",
            "rare", "safe", "slim", "soft", "warm", "wise", "cool", "wild",
        ];
        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos();
        let idx1 = (seed % words.len() as u128) as usize;
        let idx2 = ((seed / 7) % words.len() as u128) as usize;
        format!("{}-{}", words[idx1], words[idx2])
    }

    /// 写入 Plan 到文件
    pub async fn save_plan(&self, slug: &str, content: &str, agent_id: Option<&str>) -> Result<PathBuf, String> {
        tokio::fs::create_dir_all(&self.plans_dir).await
            .map_err(|e| format!("Dir creation: {}", e))?;

        let filename = match agent_id {
            Some(aid) => format!("{}-agent-{}.md", slug, aid),
            None => format!("{}.md", slug),
        };
        let path = self.plans_dir.join(&filename);
        tokio::fs::write(&path, content).await
            .map_err(|e| format!("Write: {}", e))?;
        Ok(path)
    }

    /// 读取 Plan
    pub async fn load_plan(&self, slug: &str) -> Result<String, String> {
        let path = self.plans_dir.join(format!("{}.md", slug));
        tokio::fs::read_to_string(&path).await
            .map_err(|e| format!("Read: {}", e))
    }

    /// 三级恢复 Plan (文件系统 → 消息 → 附件)
    pub async fn recover_plan(&self, slug: &str, fallback: Option<&str>) -> Option<String> {
        // Level 1: 文件系统
        if let Ok(content) = self.load_plan(slug).await {
            if !content.is_empty() {
                return Some(content);
            }
        }
        // Level 2: 会话中的快照
        if let Some(snapshot) = fallback {
            if !snapshot.is_empty() {
                return Some(snapshot.to_string());
            }
        }
        // Level 3: 新建空 plan
        None
    }

    /// 列出所有 Plan 文件
    pub async fn list_plans(&self) -> Result<Vec<String>, String> {
        let mut plans = Vec::new();
        if !self.plans_dir.exists() {
            return Ok(plans);
        }
        let mut dir = tokio::fs::read_dir(&self.plans_dir).await
            .map_err(|e| format!("Read dir: {}", e))?;
        while let Some(entry) = dir.next_entry().await.map_err(|e| e.to_string())? {
            if let Some(name) = entry.file_name().to_str() {
                if name.ends_with(".md") {
                    plans.push(name.to_string());
                }
            }
        }
        Ok(plans)
    }

    /// 复制 Plan (用于 fork 的 session)
    pub async fn fork_plan(&self, original_slug: &str, new_slug: &str) -> Result<(), String> {
        let content = self.load_plan(original_slug).await?;
        self.save_plan(new_slug, &content, None).await?;
        Ok(())
    }
}

// ========================================================================
// [4] 结构化错误注入 + Memory Correction Hint
// 移植自: messages.ts:176-221
// ========================================================================

/// Memory Correction Hint — 在错误后注入到 tool_result
pub const MEMORY_CORRECTION_HINT: &str = "\n\nNote: The user's next message may contain a correction or preference. Pay close attention — if they explain what went wrong or how they'd prefer you to work, consider saving that to memory for future sessions.";

/// 拒绝消息 — 用户拒绝工具执行时使用
pub const REJECT_MESSAGE: &str = "The user doesn't want to proceed with this tool use. STOP what you are doing and wait for the user to tell you how to proceed.";

/// 取消消息 — 用户取消操作时使用 
pub const CANCEL_MESSAGE: &str = "The user doesn't want to take this action right now. STOP what you are doing and wait for the user to tell you how to proceed.";

/// 工具执行恢复消息 — 输出截断时
pub const OUTPUT_RECOVERY_MESSAGE: &str = "Output limit hit. Resume directly — no apology, no recap. Pick up mid-thought. Break remaining work into smaller pieces.";

/// 注入 Memory Correction Hint
pub fn with_memory_hint(message: &str) -> String {
    format!("{}{}", message, MEMORY_CORRECTION_HINT)
}

/// 构建拒绝错误消息
pub fn build_rejection_message(has_alternative: bool) -> String {
    if has_alternative {
        format!(
            "{}\n\nIf you believe there's an alternative approach that would be acceptable, you may suggest it.",
            REJECT_MESSAGE
        )
    } else {
        REJECT_MESSAGE.to_string()
    }
}

/// 三段式错误消息: 事实 + 指令 + 方案
pub fn structured_error(fact: &str, instruction: &str, alternative: Option<&str>) -> String {
    let mut msg = format!("[Error] {}\n\n{}", fact, instruction);
    if let Some(alt) = alternative {
        msg.push_str(&format!("\n\nAlternative: {}", alt));
    }
    msg
}

// ========================================================================
// [5] Auto-Retry via Hook
// 移植自: toolExecution.ts:1073-1101
// ========================================================================

/// 重试决策
#[derive(Debug, Clone)]
pub enum RetryDecision {
    /// 不重试
    NoRetry,
    /// 在消息中注入 "you may retry" 提示
    RetryAllowed { reason: String },
    /// 自动重试 (带冷却)
    AutoRetry { delay_ms: u64, max_retries: u32 },
}

/// 重试 Hook
pub struct RetryHook {
    max_retries: u32,
    cooldown_ms: u64,
}

impl RetryHook {
    pub fn new(max_retries: u32, cooldown_ms: u64) -> Self {
        Self { max_retries, cooldown_ms }
    }

    /// 根据错误类型决定重试策略
    pub fn decide(&self, error: &str, attempt: u32) -> RetryDecision {
        if attempt >= self.max_retries {
            return RetryDecision::NoRetry;
        }

        let error_lower = error.to_lowercase();
        if error_lower.contains("timeout") || error_lower.contains("rate_limit") {
            // 限流/超时 → 自动重试 (带冷却)
            RetryDecision::AutoRetry {
                delay_ms: self.cooldown_ms * (1 << attempt), // 指数退避
                max_retries: self.max_retries,
            }
        } else if error_lower.contains("permission") || error_lower.contains("denied") {
            // 权限拒绝 → 等待用户批准 + 重试提示
            RetryDecision::RetryAllowed {
                reason: "Permissions may have been granted after the initial attempt. You may retry.".to_string(),
            }
        } else if error_lower.contains("network") || error_lower.contains("connection") {
            // 网络错误 → 自动重试 (短延迟)
            RetryDecision::AutoRetry {
                delay_ms: 1000,
                max_retries: 3,
            }
        } else {
            RetryDecision::NoRetry
        }
    }

    /// 执行自动重试 (带延迟)
    pub async fn auto_retry<F, T>(&self, attempt: u32, f: F) -> Result<T, String>
    where
        F: Fn() -> futures::future::BoxFuture<'_, Result<T, String>>,
    {
        let mut last_error = String::new();
        for i in 0..self.max_retries {
            match f().await {
                Ok(result) => return Ok(result),
                Err(e) => {
                    last_error = e.clone();
                    match self.decide(&e, i) {
                        RetryDecision::AutoRetry { delay_ms, .. } => {
                            sleep(Duration::from_millis(delay_ms)).await;
                        }
                        RetryDecision::RetryAllowed { .. } => {
                            // 返回可重试信号
                            return Err(format!("RETRY_ALLOWED:{}", e));
                        }
                        RetryDecision::NoRetry => {
                            return Err(e);
                        }
                    }
                }
            }
        }
        Err(last_error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // [1] 并发安全分区测试
    #[test]
    fn test_partition_tool_calls() {
        let tools = vec![
            ToolCallInfo { name: "read".into(), input: serde_json::json!({}), safety: ConcurrencySafety::ReadOnly },
            ToolCallInfo { name: "search".into(), input: serde_json::json!({}), safety: ConcurrencySafety::ReadOnly },
            ToolCallInfo { name: "edit".into(), input: serde_json::json!({}), safety: ConcurrencySafety::WriteExclusive },
            ToolCallInfo { name: "read".into(), input: serde_json::json!({}), safety: ConcurrencySafety::ReadOnly },
        ];
        let batches = partition_tool_calls(tools);
        assert_eq!(batches.len(), 3);
        assert!(batches[0].can_concur);   // Read+Search 并行
        assert!(!batches[1].can_concur);  // Edit 独占
        assert!(batches[2].can_concur);   // Read 并行
    }

    #[test]
    fn test_tool_is_readonly() {
        assert!(tool_is_readonly("read"));
        assert!(tool_is_readonly("search"));
        assert!(!tool_is_readonly("edit"));
        assert!(!tool_is_readonly("write"));
    }

    #[test]
    fn test_can_execute_concurrently() {
        let read_tool = ToolCallInfo { name: "read".into(), input: serde_json::json!({}), safety: ConcurrencySafety::ReadOnly };
        let write_tool = ToolCallInfo { name: "edit".into(), input: serde_json::json!({}), safety: ConcurrencySafety::WriteExclusive };

        // 空执行列表 → 可以执行
        assert!(can_execute_concurrently(&read_tool, &[]));
        assert!(can_execute_concurrently(&write_tool, &[]));

        // 只有只读工具在执行 → 只读可并行
        assert!(can_execute_concurrently(&read_tool, &[&read_tool]));

        // 有写入工具在执行 → 只读不可并行
        assert!(!can_execute_concurrently(&read_tool, &[&write_tool]));
    }

    // [2] Plan 管理器测试
    #[tokio::test]
    async fn test_plan_manager() {
        let temp = std::env::temp_dir().join("carpai-plan-test");
        let manager = PlanManager::new(&temp);

        let slug = manager.generate_slug();
        assert!(!slug.is_empty());
        assert!(slug.contains('-'));

        // 保存并读取 Plan
        manager.save_plan(&slug, "# Test Plan\n\n1. Step one\n2. Step two", None).await.unwrap();
        let loaded = manager.load_plan(&slug).await.unwrap();
        assert!(loaded.contains("Step one"));

        // 列表
        let plans = manager.list_plans().await.unwrap();
        assert!(!plans.is_empty());

        // Fork
        let slug2 = manager.generate_slug();
        manager.fork_plan(&slug, &slug2).await.unwrap();
        let forked = manager.load_plan(&slug2).await.unwrap();
        assert!(forked.contains("Test Plan"));

        // 清理
        let _ = std::fs::remove_dir_all(&temp);
    }

    // [4] 结构化错误测试
    #[test]
    fn test_structured_error() {
        let msg = structured_error(
            "File not found",
            "Check the file path and try again",
            Some("Use `find` to locate the file first")
        );
        assert!(msg.contains("File not found"));
        assert!(msg.contains("Check the file path"));
        assert!(msg.contains("Use `find`"));
    }

    #[test]
    fn test_with_memory_hint() {
        let msg = with_memory_hint("Tool execution failed.");
        assert!(msg.contains("Tool execution failed."));
        assert!(msg.contains("Memory Correction Hint"));
    }

    // [5] 重试 Hook 测试
    #[test]
    fn test_retry_hook_timeout() {
        let hook = RetryHook::new(3, 200);
        let decision = hook.decide("Request timed out after 30s", 0);
        assert!(matches!(decision, RetryDecision::AutoRetry { .. }));
    }

    #[test]
    fn test_retry_hook_denied() {
        let hook = RetryHook::new(3, 200);
        let decision = hook.decide("Permission denied: user rejected the action", 0);
        assert!(matches!(decision, RetryDecision::RetryAllowed { .. }));
    }

    #[test]
    fn test_retry_hook_exhausted() {
        let hook = RetryHook::new(3, 200);
        let decision = hook.decide("Some persistent error", 3); // 已达到 max_retries
        assert!(matches!(decision, RetryDecision::NoRetry));
    }
}
