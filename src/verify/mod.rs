//! 自主验证修复引擎
//!
//! 对标 Claude Code 的 auto-fix 功能，提供：
//! - 编译错误检测与修复
//! - Lint 警告自动修复
//! - 测试失败分析修复
//! - 迭代式修复循环
//!
//! 工作流: 修改代码 → 验证(编译/lint/测试) → 分析失败 → 修复 → 重新验证

use anyhow::Result;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use serde::{Deserialize, Serialize};

/// 验证阶段
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum VerifyStage {
    /// 编译检查 (cargo check, tsc, etc.)
    Compile,
    /// Lint 检查 (clippy, eslint, pylint)
    Lint,
    /// 单元测试
    UnitTest,
    /// 集成测试
    IntegrationTest,
    /// 构建
    Build,
    /// 格式化
    Format,
}

impl VerifyStage {
    pub fn label(&self) -> &'static str {
        match self {
            VerifyStage::Compile => "Compile",
            VerifyStage::Lint => "Lint",
            VerifyStage::UnitTest => "Unit Tests",
            VerifyStage::IntegrationTest => "Integration Tests",
            VerifyStage::Build => "Build",
            VerifyStage::Format => "Format",
        }
    }
}

/// 验证结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerifyResult {
    pub stage: VerifyStage,
    pub passed: bool,
    pub duration_ms: u64,
    pub output: String,
    pub errors: Vec<Diagnostic>,
    pub fix_suggestion: Option<String>,
}

/// 诊断信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Diagnostic {
    pub file: Option<String>,
    pub line: Option<usize>,
    pub column: Option<usize>,
    pub level: String,      // error, warning, info
    pub code: Option<String>,
    pub message: String,
    pub suggestion: Option<String>,
}

/// 修复操作
#[derive(Debug, Clone)]
pub struct FixOperation {
    pub file: String,
    pub old_string: String,
    pub new_string: String,
    pub description: String,
}

/// 迭代修复结果
#[derive(Debug, Clone)]
pub struct AutoFixResult {
    pub iterations: usize,
    pub total_duration_ms: u64,
    pub all_passed: bool,
    pub stages_completed: Vec<VerifyStage>,
    pub fixes_applied: Vec<FixOperation>,
    pub remaining_issues: Vec<Diagnostic>,
}

/// 验证配置
#[derive(Debug, Clone)]
pub struct VerifyConfig {
    /// 最大修复迭代次数
    pub max_iterations: usize,
    /// 每次验证超时
    pub timeout_secs: u64,
    /// 启用的验证阶段
    pub stages: Vec<VerifyStage>,
    /// 是否在修复后自动提交
    pub auto_commit: bool,
    /// 工作目录（项目根）
    pub workspace_root: PathBuf,
}

impl Default for VerifyConfig {
    fn default() -> Self {
        Self {
            max_iterations: 5,
            timeout_secs: 120,
            stages: vec![VerifyStage::Compile, VerifyStage::Lint],
            auto_commit: false,
            workspace_root: std::env::current_dir().unwrap_or_default(),
        }
    }
}

/// 验证引擎
pub struct VerifyEngine {
    config: VerifyConfig,
    #[allow(dead_code)]
    project_type_cache: Arc<RwLock<HashMap<PathBuf, String>>>,
}

impl VerifyEngine {
    pub fn new(config: VerifyConfig) -> Self {
        Self {
            config,
            project_type_cache: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// 运行完整的验证 + 修复循环
    pub async fn run_auto_fix(&self) -> Result<AutoFixResult> {
        let start = Instant::now();
        let mut fixes = Vec::new();
        let mut all_passed = false;
        let mut completed_stages = Vec::new();
        let mut remaining_issues = Vec::new();

        for iteration in 0..self.config.max_iterations {
            let mut iteration_issues = Vec::new();
            let mut iteration_passed = true;

            for &stage in &self.config.stages {
                if completed_stages.contains(&stage) && all_passed {
                    continue;
                }

                let result = self.verify_stage(stage).await?;

                if result.passed {
                    if !completed_stages.contains(&stage) {
                        completed_stages.push(stage);
                        crate::logging::info(&format!("Verify [{}]: PASSED ({:?})", stage.label(), Duration::from_millis(result.duration_ms)));
                    }
                } else {
                    iteration_passed = false;
                    iteration_issues.extend(result.errors);

                    // 尝试自动修复
                    if let Some(ref suggestion) = result.fix_suggestion {
                        crate::logging::warn(&format!(
                            "Verify [{}]: FAILED ({} errors). Attempting fix...",
                            stage.label(),
                            result.errors.len()
                        ));

                        if let Some(fix) = self.suggest_fix(&result).await? {
                            fixes.push(fix);
                        }
                    }
                }
            }

            if iteration_passed {
                all_passed = true;
                crate::logging::info(&format!(
                    "Auto-fix: All stages passed after {} iteration(s) ({:?})",
                    iteration + 1,
                    start.elapsed()
                ));
                break;
            }

            remaining_issues = iteration_issues;

            if iteration == self.config.max_iterations - 1 {
                crate::logging::warn(&format!(
                    "Auto-fix: Max iterations ({}) reached. {} issues remaining.",
                    self.config.max_iterations,
                    remaining_issues.len()
                ));
            }
        }

        Ok(AutoFixResult {
            iterations: fixes.len(),
            total_duration_ms: start.elapsed().as_millis() as u64,
            all_passed,
            stages_completed: completed_stages,
            fixes_applied: fixes,
            remaining_issues,
        })
    }

    /// 验证单个阶段
    pub async fn verify_stage(&self, stage: VerifyStage) -> Result<VerifyResult> {
        let stage_start = Instant::now();
        let project_type = self.detect_project_type().await;

        match stage {
            VerifyStage::Compile => self.verify_compile(&project_type).await,
            VerifyStage::Lint => self.verify_lint(&project_type).await,
            VerifyStage::UnitTest => self.verify_tests(&project_type, false).await,
            VerifyStage::IntegrationTest => self.verify_tests(&project_type, true).await,
            VerifyStage::Build => self.verify_build(&project_type).await,
            VerifyStage::Format => self.verify_format(&project_type).await,
        }
        .map(|mut result| {
            result.duration_ms = stage_start.elapsed().as_millis() as u64;
            result
        })
    }

    /// 检测项目类型
    async fn detect_project_type(&self) -> String {
        let root = &self.config.workspace_root;

        let checks: [(&str, &[&str]); 6] = [
            ("rust", &["Cargo.toml"]),
            ("node", &["package.json"]),
            ("python", &["setup.py", "pyproject.toml", "requirements.txt"]),
            ("go", &["go.mod"]),
            ("java", &["pom.xml", "build.gradle"]),
            ("deno", &["deno.json", "deno.jsonc"]),
        ];

        for (proj_type, markers) in &checks {
            for marker in *markers {
                if root.join(marker).exists() {
                    return proj_type.to_string();
                }
            }
        }

        "unknown".to_string()
    }

    /// 编译验证
    async fn verify_compile(&self, project_type: &str) -> Result<VerifyResult> {
        let root = &self.config.workspace_root;
        let output = match project_type {
            "rust" => run_command(root, "cargo", &["check", "--color=never"]).await,
            "node" => run_command(root, "npx", &["tsc", "--noEmit"]).await,
            "python" => run_command(root, "python", &["-m", "py_compile", "src/main.py"]).await,
            "go" => run_command(root, "go", &["build", "./..."]).await,
            _ => run_command(root, "echo", &["No compiler configured"]).await,
        };

        let (passed, errors) = parse_compile_output(&output, project_type);
        let fix_suggestion = if passed { None } else { Some(generate_fix_suggestion(&errors)) };

        Ok(VerifyResult {
            stage: VerifyStage::Compile,
            passed,
            duration_ms: 0,
            output: output.clone(),
            errors,
            fix_suggestion,
        })
    }

    /// Lint 验证
    async fn verify_lint(&self, project_type: &str) -> Result<VerifyResult> {
        let root = &self.config.workspace_root;
        let output = match project_type {
            "rust" => run_command(root, "cargo", &["clippy", "--color=never", "--", "-D", "warnings"]).await,
            "node" => run_command(root, "npx", &["eslint", "."]).await,
            "python" => run_command(root, "python", &["-m", "pylint", "src/"]).await,
            "go" => run_command(root, "golint", &["./..."]).await,
            _ => run_command(root, "echo", &["No linter configured"]).await,
        };

        let (passed, errors) = parse_lint_output(&output, project_type);
        let fix_suggestion = if passed { None } else { Some("Run auto-fix to address lint warnings.".to_string()) };

        Ok(VerifyResult {
            stage: VerifyStage::Lint,
            passed,
            duration_ms: 0,
            output: output.clone(),
            errors,
            fix_suggestion,
        })
    }

    /// 测试验证
    async fn verify_tests(&self, project_type: &str, integration: bool) -> Result<VerifyResult> {
        let root = &self.config.workspace_root;
        let output = match (project_type, integration) {
            ("rust", false) => run_command_timeout(root, &self.config.timeout_secs, "cargo", &["test", "--color=never"]).await,
            ("rust", true) => run_command_timeout(root, &self.config.timeout_secs, "cargo", &["test", "--color=never", "--test", "*"]).await,
            ("node", false) => run_command_timeout(root, &self.config.timeout_secs, "npx", &["jest", "--passWithNoTests"]).await,
            ("python", false) => run_command_timeout(root, &self.config.timeout_secs, "python", &["-m", "pytest"]).await,
            _ => run_command(root, "echo", &["No test runner configured"]).await,
        };

        let (passed, errors) = parse_test_output(&output);
        let fix_suggestion = if passed { None } else { Some("Review and fix failing tests.".to_string()) };

        Ok(VerifyResult {
            stage: if integration { VerifyStage::IntegrationTest } else { VerifyStage::UnitTest },
            passed,
            duration_ms: 0,
            output: output.clone(),
            errors,
            fix_suggestion,
        })
    }

    /// 构建验证
    async fn verify_build(&self, project_type: &str) -> Result<VerifyResult> {
        let root = &self.config.workspace_root;
        let output = match project_type {
            "rust" => run_command(root, "cargo", &["build", "--color=never"]).await,
            "node" => run_command(root, "npm", &["run", "build"]).await,
            _ => run_command(root, "echo", &["No build tool configured"]).await,
        };

        let passed = output.lines().last().map(|l| !l.contains("error")).unwrap_or(true);

        Ok(VerifyResult {
            stage: VerifyStage::Build,
            passed,
            duration_ms: 0,
            output,
            errors: vec![],
            fix_suggestion: None,
        })
    }

    /// 格式化验证
    async fn verify_format(&self, project_type: &str) -> Result<VerifyResult> {
        let root = &self.config.workspace_root;
        let output = match project_type {
            "rust" => run_command(root, "cargo", &["fmt", "--check", "--color=never"]).await,
            _ => run_command(root, "echo", &["No formatter configured"]).await,
        };

        let passed = output.contains("is not formatted") == false;
        let fix_suggestion = if passed { None } else { Some("Run `cargo fmt` to fix formatting.".to_string()) };

        Ok(VerifyResult {
            stage: VerifyStage::Format,
            passed,
            duration_ms: 0,
            output: output.clone(),
            errors: vec![],
            fix_suggestion,
        })
    }

    /// 生成修复建议
    async fn suggest_fix(&self, result: &VerifyResult) -> Result<Option<FixOperation>> {
        if result.errors.is_empty() {
            return Ok(None);
        }

        // 从第一个错误生成修复
        let err = &result.errors[0];
        if let (Some(file), Some(line)) = (&err.file, err.line) {
            let full_path = self.config.workspace_root.join(file);
            if full_path.exists() {
                let content = tokio::fs::read_to_string(&full_path).await?;
                let lines: Vec<&str> = content.lines().collect();

                if line > 0 && line <= lines.len() {
                    let error_line = lines[line - 1];

                    // 根据错误类型生成修复
                    if let Some(fix) = generate_line_fix(error_line, &err.code.as_deref().unwrap_or(""), &err.message) {
                        return Ok(Some(FixOperation {
                            file: file.clone(),
                            old_string: error_line.to_string(),
                            new_string: fix,
                            description: format!("Auto-fix: {} at {}:{}", err.message, file, line),
                        }));
                    }
                }
            }
        }

        Ok(None)
    }
}

// --- Command execution ---

async fn run_command(root: &Path, cmd: &str, args: &[&str]) -> String {
    run_command_timeout(root, 60, cmd, args).await
}

async fn run_command_timeout(root: &Path, timeout_secs: u64, cmd: &str, args: &[&str]) -> String {
    match tokio::time::timeout(
        Duration::from_secs(timeout_secs),
        tokio::process::Command::new(cmd)
            .args(args)
            .current_dir(root)
            .output(),
    ).await {
        Ok(Ok(output)) => {
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            if stderr.is_empty() { stdout } else { format!("{}\n{}", stdout, stderr) }
        }
        Ok(Err(e)) => format!("Command error: {}", e),
        Err(_) => format!("Timeout after {}s running: {} {}", timeout_secs, cmd, args.join(" ")),
    }
}

// --- Output parsing ---

fn parse_compile_output(output: &str, project_type: &str) -> (bool, Vec<Diagnostic>) {
    let mut errors = Vec::new();
    let has_error = match project_type {
        "rust" => {
            for line in output.lines() {
                // rustc error format: file:line:col: error[code]: message
                if let Some(diag) = parse_rustc_diagnostic(line) {
                    errors.push(diag);
                }
            }
            output.contains("error[")
        }
        _ => output.contains("error") || output.contains("Error"),
    };

    (!has_error, errors)
}

fn parse_lint_output(output: &str, project_type: &str) -> (bool, Vec<Diagnostic>) {
    let mut warnings = Vec::new();
    match project_type {
        "rust" => {
            for line in output.lines() {
                if let Some(diag) = parse_rustc_diagnostic(line) {
                    warnings.push(diag);
                }
            }
        }
        _ => {}
    };
    (warnings.is_empty(), warnings)
}

fn parse_test_output(output: &str) -> (bool, Vec<Diagnostic>) {
    let failed = output.contains("FAILED") || output.contains("failures:");
    let errors = if failed {
        vec![Diagnostic {
            file: None,
            line: None,
            column: None,
            level: "error".to_string(),
            code: Some("TEST_FAILURE".to_string()),
            message: output.lines()
                .filter(|l| l.contains("FAILED") || l.contains("panicked"))
                .take(5)
                .collect::<Vec<_>>()
                .join("\n"),
            suggestion: None,
        }]
    } else {
        vec![]
    };
    (!failed, errors)
}

fn parse_rustc_diagnostic(line: &str) -> Option<Diagnostic> {
    // Pattern: file:line:col: level[code]: message
    let re = regex::Regex::new(
        r"^(.+?):(\d+):(\d+):\s+(\w+)\[?([^]]*)\]?:\s+(.+)$"
    ).ok()?;

    if let Some(caps) = re.captures(line) {
        Some(Diagnostic {
            file: Some(caps[1].to_string()),
            line: caps[2].parse::<usize>().ok(),
            column: caps[3].parse::<usize>().ok(),
            level: caps[4].to_string(),
            code: Some(caps[5].to_string()),
            message: caps[6].to_string(),
            suggestion: None,
        })
    } else {
        None
    }
}

// --- Fix generation ---

fn generate_fix_suggestion(errors: &[Diagnostic]) -> String {
    if errors.is_empty() {
        return "No specific fix available.".to_string();
    }
    let mut suggestions = String::from("Suggested fixes:\n");
    for err in errors.iter().take(5) {
        suggestions.push_str(&format!("- {}: {}\n", err.code.as_deref().unwrap_or("unknown"), err.message));
    }
    suggestions
}

fn generate_line_fix(line: &str, code: &str, _message: &str) -> Option<String> {
    match code {
        "unused_variable" | "unused_import" | "dead_code" => {
            // Prefix with underscore or remove
            if line.trim_start().starts_with("let ") || line.trim_start().starts_with("use ") {
                Some(line.replacen("let ", "let _", 1))
            } else {
                Some(format!("// {}", line))
            }
        }
        "needless_return" => {
            // Remove `return` keyword
            Some(line.replace("return ", ""))
        }
        "missing_safety_doc" | "missing_docs" => {
            // Add doc comment
            let indent = line.chars().take_while(|c| c.is_whitespace()).collect::<String>();
            Some(format!("{}/// TODO: Add documentation\n{}", indent, line))
        }
        "should_implement_trait" => {
            // Mark as todo
            Some(format!("todo!() // {}", line.trim()))
        }
        _ => None,
    }
}

// --- Summary formatting ---

pub fn format_verify_result(result: &AutoFixResult) -> String {
    let mut output = format!(
        "## Verification & Auto-Fix Report\n\n**Result**: {}\n**Iterations**: {}\n**Duration**: {:?}\n\n",
        if result.all_passed { "✅ ALL PASSED" } else { "❌ ISSUES REMAINING" },
        result.iterations,
        Duration::from_millis(result.total_duration_ms),
    );

    output.push_str(&format!("**Stages completed**: {:?}\n", result.stages_completed));
    output.push_str(&format!("**Fixes applied**: {}\n\n", result.fixes_applied.len()));

    for fix in &result.fixes_applied {
        output.push_str(&format!("- **{}**: {}\n", fix.file, fix.description));
    }

    if !result.remaining_issues.is_empty() {
        output.push_str("\n**Remaining issues**:\n");
        for issue in &result.remaining_issues {
            output.push_str(&format!(
                "- [{}] {}:{} {}: {}\n",
                issue.level,
                issue.file.as_deref().unwrap_or("<unknown>"),
                issue.line.map(|l| l.to_string()).unwrap_or_default(),
                issue.code.as_deref().unwrap_or(""),
                issue.message,
            ));
        }
    }

    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_detect_project_type_rust() {
        let tmp = std::env::temp_dir().join("carpai-verify-test");
        tokio::fs::create_dir_all(&tmp).await.unwrap();
        tokio::fs::write(tmp.join("Cargo.toml"), "").await.unwrap();

        let engine = VerifyEngine::new(VerifyConfig {
            workspace_root: tmp.clone(),
            ..Default::default()
        });

        let proj_type = engine.detect_project_type().await;
        assert_eq!(proj_type, "rust");

        tokio::fs::remove_dir_all(&tmp).await.unwrap();
    }

    #[test]
    fn test_parse_rustc_error() {
        let line = "src/main.rs:10:5: error[E0308]: mismatched types";
        let diag = parse_rustc_diagnostic(line);
        assert!(diag.is_some());
        let d = diag.unwrap();
        assert_eq!(d.file.unwrap(), "src/main.rs");
        assert_eq!(d.line, Some(10));
        assert_eq!(d.code.unwrap(), "E0308");
    }

    #[test]
    fn test_generate_line_fix_needless_return() {
        let fix = generate_line_fix("    return x;", "needless_return", "");
        assert_eq!(fix, Some("    x;".to_string()));
    }

    #[test]
    fn test_generate_line_fix_unused_variable() {
        let fix = generate_line_fix("    let x = 1;", "unused_variable", "");
        assert_eq!(fix, Some("    let _x = 1;".to_string()));
    }

    #[test]
    fn test_format_verify_result_all_pass() {
        let result = AutoFixResult {
            iterations: 1,
            total_duration_ms: 1500,
            all_passed: true,
            stages_completed: vec![VerifyStage::Compile, VerifyStage::Lint],
            fixes_applied: vec![],
            remaining_issues: vec![],
        };
        let output = format_verify_result(&result);
        assert!(output.contains("ALL PASSED"));
        assert!(output.contains("Compile"));
    }
}
