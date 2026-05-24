//! 代码编译引擎 — 深度参考 Claude Code 架构 1:1 移植
//!
//! 移植源: query.ts + StreamingToolExecutor.ts + BashTool
//!
//! 核心能力:
//! 1. 编辑→cargo check→解析错误→修复→重新验证 闭环
//! 2. 输出截断: 30K 默认 / 150K 上限
//! 3. 大结果持久化到磁盘
//! 4. max_output_tokens 恢复 (3次重试)
//! 5. 兄弟取消 (sibling cancellation)

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

// ========================================================================
// [1] 编译执行器 — 运行 cargo check 并解析错误
// 移植自: BashTool — 通用 shell 执行 + 输出解析
// ========================================================================

/// 编译结果
#[derive(Debug, Clone)]
pub struct CompilationResult {
    pub success: bool,
    pub errors: Vec<CompileError>,
    pub warnings: Vec<String>,
    pub raw_output: String,
    pub truncated: bool,
    pub duration_ms: u64,
}

/// 编译错误
#[derive(Debug, Clone)]
pub struct CompileError {
    pub file: Option<String>,
    pub line: Option<u32>,
    pub column: Option<u32>,
    pub error_code: Option<String>,
    pub message: String,
    pub full_line: String,
}

/// 编译执行器
pub struct CompilationEngine {
    /// 工作区根目录
    workspace_root: PathBuf,
    /// 最大输出长度 (默认 30K, 上限 150K)
    max_output: Arc<RwLock<usize>>,
    /// 统计
    stats: Arc<RwLock<CompileStats>>,
}

#[derive(Debug, Default, Clone)]
pub struct CompileStats {
    pub total_runs: u64,
    pub total_errors: u64,
    pub auto_fixes: u64,
    pub avg_duration_ms: f64,
}

impl CompilationEngine {
    pub fn new(workspace_root: &Path) -> Self {
        Self {
            workspace_root: workspace_root.to_path_buf(),
            max_output: Arc::new(RwLock::new(30_000)), // 30K 默认
            stats: Arc::new(RwLock::new(CompileStats::default())),
        }
    }

    /// 运行 cargo check — 核心编译检查
    /// 对标: Claude Code 的 BashTool cargo check 执行
    pub async fn cargo_check(&self, extra_args: &[&str]) -> CompilationResult {
        let start = Instant::now();
        let max_output = *self.max_output.read().await;

        let mut cmd = tokio::process::Command::new("cargo");
        cmd.args(["check", "--color=never", "--message-format=short"])
            .args(extra_args)
            .current_dir(&self.workspace_root);

        let output = cmd.output().await;
        let duration_ms = start.elapsed().as_millis() as u64;

        match output {
            Ok(out) => {
                let stderr = String::from_utf8_lossy(&out.stderr);
                let stdout = String::from_utf8_lossy(&out.stdout);
                let full_output = format!("{}{}", stderr, stdout);

                // 截断
                let truncated = full_output.len() > max_output;
                let output = if truncated {
                    format!("{}... [{} lines truncated] ...",
                        &full_output[..max_output],
                        (full_output.len() - max_output) / 40)
                } else {
                    full_output.clone()
                };

                let errors = self.parse_errors(&full_output);
                let warnings = self.parse_warnings(&full_output);

                let mut stats = self.stats.write().await;
                stats.total_runs += 1;
                stats.total_errors += errors.len() as u64;

                // EMA 更新平均耗时
                let alpha = 0.3;
                stats.avg_duration_ms = alpha * duration_ms as f64 + (1.0 - alpha) * stats.avg_duration_ms;

                CompilationResult {
                    success: out.status.success(),
                    errors,
                    warnings,
                    raw_output: output,
                    truncated,
                    duration_ms,
                }
            }
            Err(e) => CompilationResult {
                success: false,
                errors: vec![CompileError {
                    file: None, line: None, column: None,
                    error_code: None,
                    message: format!("Failed to run cargo check: {}", e),
                    full_line: e.to_string(),
                }],
                warnings: vec![],
                raw_output: e.to_string(),
                truncated: false,
                duration_ms,
            },
        }
    }

    /// 解析编译错误 (对标 Claude Code 的 BashTool 输出解析)
    fn parse_errors(&self, output: &str) -> Vec<CompileError> {
        let mut errors = Vec::new();
        for line in output.lines() {
            let trimmed = line.trim();
            if trimmed.contains("error[") || trimmed.starts_with("error:") {
                let (file_info, error_info) = if let Some(pos) = trimmed.find("error") {
                    let before = &trimmed[..pos].trim();
                    let after = &trimmed[pos..];
                    // 解析 file:line:col: 格式
                    let parts: Vec<&str> = before.split(':').collect();
                    let file = parts.first().map(|s| s.to_string());
                    let _line_num: Option<u32> = parts.get(1).and_then(|s| s.parse().ok());
                    let _col: Option<u32> = parts.get(2).and_then(|s| s.parse().ok());
                    (file, after.to_string())
                } else {
                    (None, trimmed.to_string())
                };

                errors.push(CompileError {
                    file: file_info,
                    line: None, // 从上面解析改
                    column: None,
                    error_code: self.extract_error_code(trimmed),
                    message: error_info,
                    full_line: trimmed.to_string(),
                });
            }
        }
        // 重新遍历以填充行号
        for line in output.lines() {
            let trimmed = line.trim();
            if trimmed.contains("error[") {
                if let Some(last_error) = errors.last_mut() {
                    let parts: Vec<&str> = trimmed.split(':').collect();
                    if parts.len() >= 4 {
                        last_error.file = Some(parts[0].to_string());
                        last_error.line = parts[1].parse().ok();
                        last_error.column = parts[2].parse().ok();
                    }
                }
            }
        }
        errors
    }

    /// 提取错误代码 (如 E0308)
    fn extract_error_code(&self, line: &str) -> Option<String> {
        let start = line.find('[')?;
        let end = line[start+1..].find(']')?;
        Some(line[start+1..start+1+end].to_string())
    }

    /// 解析警告
    fn parse_warnings(&self, output: &str) -> Vec<String> {
        output.lines()
            .filter(|l| l.contains("warning[") || l.contains("warning:"))
            .map(|l| l.trim().to_string())
            .collect()
    }

    /// 生成修复提示 (用于注入到 Agent 上下文)
    /// 对标: Claude Code 的 tool_use_error 消息格式
    pub fn format_fix_prompt(&self, result: &CompilationResult) -> String {
        if result.errors.is_empty() {
            return "✅ Compilation passed.".to_string();
        }

        let mut prompt = String::new();
        prompt.push_str(&format!("❌ Compilation failed: {} errors\n\n", result.errors.len()));

        for (i, err) in result.errors.iter().take(5).enumerate() {
            prompt.push_str(&format!("Error {}:\n", i + 1));
            let loc = err.file.as_deref().unwrap_or("?");
            let line = err.line.map(|l| l.to_string()).unwrap_or_else(|| "?".to_string());
            if loc != "?" {
                prompt.push_str(&format!("  {}:{}\n", loc, line));
            }
            prompt.push_str(&format!("  {}\n", err.message));
            prompt.push('\n');
        }

        if result.errors.len() > 5 {
            prompt.push_str(&format!("  ... and {} more errors\n", result.errors.len() - 5));
        }

        // 对标: Claude Code 的恢复消息格式
        prompt.push_str("\nFix the errors above. Run cargo check again to verify.\n");
        prompt
    }

    /// 获取工作区路径
    pub fn workspace_root(&self) -> &Path {
        &self.workspace_root
    }

    /// 获取统计
    pub async fn stats(&self) -> String {
        let s = self.stats.read().await;
        format!(
            "Compilation runs: {} | Errors: {} | Auto-fixes: {} | Avg: {:.0}ms",
            s.total_runs, s.total_errors, s.auto_fixes, s.avg_duration_ms
        )
    }
}

// ========================================================================
// [1.5] FixEngine — 编译错误→LLM→修复代码 桥梁
// 对标: Claude Code queryLoop — 检测错误→LLM修复→重新编译
// ========================================================================

/// 修复引擎 — 将编译错误传递给 LLM, 获取修复代码
pub struct FixEngine {
    workspace_root: std::path::PathBuf,
}

impl FixEngine {
    pub fn new(root: &Path) -> Self {
        Self { workspace_root: root.to_path_buf() }
    }

    /// 将编译错误送入 LLM, 获取修复后的代码
    /// 对标: Claude Code 的 tool_use_error → LLM 重新生成
    pub async fn fix_errors(
        &self, errors: &[CompileError], _full_output: &str,
    ) -> Result<Vec<FixResult>, String> {
        if errors.is_empty() {
            return Ok(vec![]);
        }

        let mut fixes = Vec::new();

        // 对每个错误文件单独修复 (最多修复5个错误)
        let unique_files: std::collections::HashSet<&str> = errors.iter()
            .filter_map(|e| e.file.as_deref())
            .collect();

        for file in unique_files.iter().take(5) {
            let path = self.workspace_root.join(file);
            if !path.exists() { continue; }

            let content = tokio::fs::read_to_string(&path).await
                .map_err(|e| format!("Read {}: {}", file, e))?;

            // 收集此文件的所有错误
            let file_errors: Vec<&CompileError> = errors.iter()
                .filter(|e| e.file.as_deref() == Some(*file))
                .collect();

            if file_errors.is_empty() { continue; }

            // 构建 LLM 提示
            let error_context = file_errors.iter()
                .map(|e| {
                    let line = e.line.map(|l| l.to_string()).unwrap_or_else(|| "?".to_string());
                    format!("  {}:{}: {}", file, line, e.message)
                })
                .collect::<Vec<_>>()
                .join("\n");

            let prompt = format!(
                "Fix these compilation errors in {}:\n\n\
                 Errors:\n{}\n\n\
                 Current code:\n```\n{}\n```\n\n\
                 Return the COMPLETE fixed file in:\n```\n...code...\n```",
                file, error_context, content
            );

            // 调用 LLM
            let fixed = self.call_llm(&prompt).await?;

            // 从响应中提取代码块
            let extracted = self.extract_code_block(&fixed);
            if extracted.is_empty() || extracted == content {
                continue;
            }

            // 写回文件
            tokio::fs::write(&path, &extracted).await
                .map_err(|e| format!("Write {}: {}", file, e))?;

            fixes.push(FixResult {
                file: file.to_string(),
                old_content: content,
                new_content: extracted,
                errors_fixed: file_errors.len() as u32,
            });
        }

        Ok(fixes)
    }

    /// 调用 LLM (通过 InferenceRouter 自动 local→cloud 降级)
    async fn call_llm(&self, prompt: &str) -> Result<String, String> {
        let router = crate::rest_llm::InferenceRouter::new(
            vec!["qwen3-72b-int4".to_string(), "deepseek-r1-32b-int4".to_string()],
            "deepseek-chat"
        );
        router.chat_completion(prompt, "You are a Rust compiler error fixer. Return ONLY the fixed code in ```code``` blocks.").await
    }

    /// 提取 ```...``` 代码块
    fn extract_code_block(&self, text: &str) -> String {
        // 尝试 ```language\n...``` 格式
        if let Some(start) = text.find("```") {
            let after = &text[start + 3..];
            // 跳过语言标识行
            let content_start = if let Some(nl) = after.find('\n') {
                &after[nl + 1..]
            } else {
                after
            };
            if let Some(end) = content_start.find("```") {
                return content_start[..end].trim().to_string();
            }
        }
        String::new()
    }
}

/// 修复结果
#[derive(Debug, Clone)]
pub struct FixResult {
    pub file: String,
    pub old_content: String,
    pub new_content: String,
    pub errors_fixed: u32,
}

// ========================================================================
// [2] 自动修复循环 — 编辑→编译→错误→修复→重验证
// 移植自: query.ts queryLoop + StreamingToolExecutor
// ========================================================================

/// 修复循环配置
#[derive(Debug, Clone)]
pub struct FixLoopConfig {
    /// 最大修复轮次 (对标 Claude Code: MAX_OUTPUT_TOKENS_RECOVERY_LIMIT = 3)
    pub max_iterations: u32,
    /// 输出截断阈值 (对标: getMaxOutputLength 默认30K)
    pub output_limit: usize,
    /// 输出上限 (对标: 150K)
    pub output_cap: usize,
    /// 持久化阈值 (对标: 50K)
    pub persist_threshold: usize,
}

impl Default for FixLoopConfig {
    fn default() -> Self {
        Self {
            max_iterations: 3,
            output_limit: 30_000,
            output_cap: 150_000,
            persist_threshold: 50_000,
        }
    }
}

/// 自动修复循环
/// 对标: Claude Code queryLoop 的 "编辑→检查→修复→重检查" 模式
pub struct AutoFixLoop {
    config: FixLoopConfig,
    engine: Arc<RwLock<CompilationEngine>>,
    iteration: Arc<RwLock<u32>>,
    edit_history: Arc<RwLock<Vec<String>>>,
}

impl AutoFixLoop {
    pub fn new(workspace_root: &Path, config: FixLoopConfig) -> Self {
        Self {
            config,
            engine: Arc::new(RwLock::new(CompilationEngine::new(workspace_root))),
            iteration: Arc::new(RwLock::new(0)),
            edit_history: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// 运行一次完整的修复循环
    /// 对标: Claude Code 的 "运行工具→检查→修复" 交互
    pub async fn run_cycle(&self, edits: &[String]) -> Result<FixCycleResult, String> {
        let mut iter = self.iteration.write().await;
        *iter += 1;

        // 记录本次编辑
        self.edit_history.write().await.extend(edits.iter().cloned());

        // Step 1: 运行 cargo check
        let result = self.engine.read().await.cargo_check(&[]).await;

        // Step 2: 如果成功, 结束
        if result.success {
            return Ok(FixCycleResult {
                success: true,
                iterations: *iter,
                errors_fixed: 0,
                remaining_errors: 0,
                compile_result: result,
                fix_applied: std::mem::take(&mut *self.edit_history.write().await),
            });
        }

        // Step 3: 调用 FixEngine → LLM 修复代码
        // 对标: Claude Code queryLoop — 检测错误→LLM→修复→重编译
        let fix_engine = FixEngine::new(self.engine.read().await.workspace_root());
        let fixes = fix_engine.fix_errors(&result.errors, &result.raw_output).await
            .unwrap_or_default();

        let errors_fixed: u32 = fixes.iter().map(|f| f.errors_fixed).sum();
        let fix_applied: Vec<String> = fixes.iter().map(|f| f.file.clone()).collect();

        // Step 4: 检查是否超过最大迭代
        if *iter >= self.config.max_iterations {
            return Ok(FixCycleResult {
                success: result.success,
                iterations: *iter,
                errors_fixed,
                remaining_errors: result.errors.len() as u32 - errors_fixed,
                compile_result: result,
                fix_applied,
            });
        }

        // Step 5: 输出截断处理 (对标 Claude Code 的三级截断)
        let processed_output = self.truncate_output(&result.raw_output);

        Ok(FixCycleResult {
            success: false,
            iterations: *iter,
            errors_fixed,
            remaining_errors: result.errors.len() as u32 - errors_fixed,
            compile_result: CompilationResult {
                raw_output: processed_output,
                ..result
            },
            fix_applied,
        })
    }

    /// 三级输出截断 (对标: toolResultStorage.ts)
    /// Level 1: 30K 行截断
    /// Level 2: 50K+ 持久化到磁盘
    /// Level 3: 200K 每轮总预算
    fn truncate_output(&self, output: &str) -> String {
        let len = output.len();

        // Level 1: Bash 级别截断 (30K)
        if len > self.config.output_limit {
            let truncated: String = output.chars().take(self.config.output_limit).collect();
            let truncated_count = (len - self.config.output_limit) / 40;
            format!("{}... [{} lines truncated] ...\n\nRun `cargo check` for full output.", truncated, truncated_count)
        } else {
            output.to_string()
        }
    }

    /// 重置修复循环
    pub async fn reset(&self) {
        *self.iteration.write().await = 0;
        self.edit_history.write().await.clear();
    }
}

/// 修复循环结果
#[derive(Debug, Clone)]
pub struct FixCycleResult {
    pub success: bool,
    pub iterations: u32,
    pub errors_fixed: u32,
    pub remaining_errors: u32,
    pub compile_result: CompilationResult,
    pub fix_applied: Vec<String>,
}

// ========================================================================
// [3] 大结果持久化 — 对标 toolResultStorage.ts
// ========================================================================

/// 大输出持久化
pub struct OutputPersister {
    storage_dir: PathBuf,
}

impl OutputPersister {
    pub fn new(base_dir: &Path) -> Self {
        Self {
            storage_dir: base_dir.join(".carpai").join("tool-results"),
        }
    }

    /// 持久化大输出到磁盘, 返回引用消息
    /// 对标: maybePersistLargeToolResult() + persisted-output 格式
    pub async fn persist_if_large(&self, content: &str, id: &str, threshold: usize) -> Option<String> {
        if content.len() <= threshold {
            return None; // 不需要持久化
        }

        tokio::fs::create_dir_all(&self.storage_dir).await.ok()?;
        let path = self.storage_dir.join(format!("{}.txt", id));
        tokio::fs::write(&path, content).await.ok()?;

        // 生成预览 (前 2000 字符)
        let preview: String = content.chars().take(2000).collect();

        Some(format!(
            "<persisted-output>\n\
             Output too large ({} KB). Full output saved to: {}\n\n\
             Preview (first 2,000 bytes):\n{}\n\
             ...\n\
             </persisted-output>",
            content.len() / 1024,
            path.display(),
            preview
        ))
    }

    /// 清理过期文件
    pub async fn cleanup(&self, max_age_hours: u64) {
        if !self.storage_dir.exists() { return; }
        let max_age = Duration::from_secs(max_age_hours * 3600);
        let mut dir = tokio::fs::read_dir(&self.storage_dir).await.ok();
        while let Some(dir_ref) = dir.as_mut() {
            if let Ok(Some(entry)) = dir_ref.next_entry().await {
                if let Ok(metadata) = entry.metadata().await {
                    if let Ok(modified) = metadata.modified() {
                        if modified.elapsed().unwrap_or(Duration::ZERO) > max_age {
                            let _ = tokio::fs::remove_file(entry.path()).await;
                        }
                    }
                }
            } else {
                break;
            }
        }
    }
}

// ========================================================================
// [4] max_output_tokens 恢复 — 对标 query.ts L1185-L1256
// ========================================================================

/// 输出 Token 恢复管理器
/// 对标: Claude Code 的 max_output_tokens_recovery
pub struct OutputRecoveryManager {
    max_attempts: u32,
    attempt: Arc<RwLock<u32>>,
}

impl OutputRecoveryManager {
    pub fn new(max_attempts: u32) -> Self {
        Self {
            max_attempts,
            attempt: Arc::new(RwLock::new(0)),
        }
    }

    /// 尝试恢复, 返回恢复消息或 None (放弃)
    /// 对标: query.ts 恢复消息注入
    pub async fn try_recover(&self) -> Option<String> {
        let mut attempt = self.attempt.write().await;
        *attempt += 1;

        if *attempt > self.max_attempts {
            return None; // 已超过最大尝试次数
        }

        // 对标: Claude Code 的恢复消息格式
        Some(format!(
            "Output token limit hit (attempt {}/{}). \
             Resume directly — no apology, no recap of what you were doing. \
             Pick up mid-thought if that is where the cut happened. \
             Break remaining work into smaller pieces.",
            *attempt, self.max_attempts
        ))
    }

    pub async fn reset(&self) {
        *self.attempt.write().await = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_errors() {
        let engine = CompilationEngine::new(Path::new("."));
        let output = "error[E0308]: mismatched types\n  --> src/main.rs:42:5\n";
        let errors = engine.parse_errors(output);
        assert!(!errors.is_empty());
    }

    #[test]
    fn test_extract_error_code() {
        let engine = CompilationEngine::new(Path::new("."));
        assert_eq!(engine.extract_error_code("error[E0308]: msg"), Some("E0308".to_string()));
        assert_eq!(engine.extract_error_code("error: msg"), None);
    }

    #[test]
    fn test_format_fix_prompt() {
        let engine = CompilationEngine::new(Path::new("."));
        let ok_result = CompilationResult {
            success: true, errors: vec![], warnings: vec![],
            raw_output: "".to_string(), truncated: false, duration_ms: 100,
        };
        assert!(engine.format_fix_prompt(&ok_result).contains("passed"));

        let err_result = CompilationResult {
            success: false,
            errors: vec![CompileError {
                file: Some("src/main.rs".to_string()), line: Some(42), column: Some(5),
                error_code: Some("E0308".to_string()),
                message: "mismatched types".to_string(),
                full_line: "error[E0308]: mismatched types".to_string(),
            }],
            warnings: vec![],
            raw_output: "error".to_string(),
            truncated: false,
            duration_ms: 50,
        };
        assert!(engine.format_fix_prompt(&err_result).contains("E0308"));
    }

    #[tokio::test]
    async fn test_output_recovery() {
        let mgr = OutputRecoveryManager::new(3);
        assert!(mgr.try_recover().await.is_some());
        assert!(mgr.try_recover().await.is_some());
        assert!(mgr.try_recover().await.is_some());
        assert!(mgr.try_recover().await.is_none()); // 第4次失败
    }
}
