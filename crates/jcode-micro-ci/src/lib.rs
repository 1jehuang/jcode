//! # jcode-micro-ci
//! 微型 CI — 在代码离开开发者电脑之前完成三层检查。
//!
//! ## 增强说明
//! - `run()` 现在实际执行所有三层检查 (AST / Type / AI Logic)
//! - 支持并行执行：使用 `tokio::join!` 同时运行各阶段
//! - 支持自动修复：修复尾随空格、缺少换行符、Tab 字符等问题
//! - 更多 AST 检查：命名规范、TODO/FIXME 检测、魔法数字、导入排序

mod phases;
mod reporter;
mod hook;
mod auto_fix;

pub use reporter::{CiReport, Issue};
pub use hook::{GitHook, HookConfig, HookType};
pub use auto_fix::{AutoFixer, FixAction};

use phases::{
    AstCheck, TypeCheck, AiLogicCheck,
    RegexAstCheck, CargoTypeCheck, RuleBasedAiCheck,
};
use std::time::Instant;

#[derive(Debug, Clone)]
pub struct CiConfig {
    pub workspace_root: String,
    pub fail_on_warning: bool,
    pub max_ai_check_time_ms: u64,
    pub parallel: bool,
    pub incremental: bool,
    pub git_diff_mode: bool,
    pub output_format: String,
    /// 启用自动修复可修复的问题
    pub auto_fix: bool,
}

impl Default for CiConfig {
    fn default() -> Self {
        Self {
            workspace_root: ".".to_string(),
            fail_on_warning: false,
            max_ai_check_time_ms: 5000,
            parallel: true,
            incremental: true,
            git_diff_mode: false,
            output_format: "text".to_string(),
            auto_fix: false,
        }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct CiStats {
    pub total_time_ms: u64,
    pub ast_time_ms: u64,
    pub type_time_ms: u64,
    pub ai_time_ms: u64,
    pub total_issues: usize,
    pub errors: usize,
    pub warnings: usize,
    /// 自动修复的问题数
    pub auto_fixed: usize,
}

impl Default for CiStats {
    fn default() -> Self {
        Self {
            total_time_ms: 0,
            ast_time_ms: 0,
            type_time_ms: 0,
            ai_time_ms: 0,
            total_issues: 0,
            errors: 0,
            warnings: 0,
            auto_fixed: 0,
        }
    }
}

pub struct MicroCi {
    config: CiConfig,
    ast_checker: RegexAstCheck,
    type_checker: CargoTypeCheck,
    ai_checker: RuleBasedAiCheck,
    auto_fixer: AutoFixer,
}

impl MicroCi {
    pub fn new(config: CiConfig) -> Self {
        Self {
            auto_fixer: AutoFixer::new(config.auto_fix),
            ast_checker: RegexAstCheck::new(),
            type_checker: CargoTypeCheck::new(),
            ai_checker: RuleBasedAiCheck::new(),
            config,
        }
    }

    /// 执行完整的 Micro-CI 检查流水线
    pub async fn run(&self) -> CiReport {
        let start = Instant::now();
        let mut all_issues = Vec::new();
        let mut stats = CiStats::default();

        let root = &self.config.workspace_root;

        // 选择并行或串行执行
        let phases: Vec<(&str, tokio::task::JoinHandle<anyhow::Result<Vec<Issue>>>)> = if self.config.parallel {
            // 并行执行所有阶段
            let ast_handle = {
                let root = root.to_string();
                let checker = RegexAstCheck::new();
                tokio::spawn(async move { checker.check(&root).await })
            };
            let type_handle = {
                let root = root.to_string();
                let checker = CargoTypeCheck::new();
                tokio::spawn(async move { checker.check(&root).await })
            };
            let ai_handle = {
                let root = root.to_string();
                let checker = RuleBasedAiCheck::new();
                tokio::spawn(async move { checker.check(&root).await })
            };
            vec![
                ("ast", ast_handle),
                ("type", type_handle),
                ("ai", ai_handle),
            ]
        } else {
            // 串行执行
            vec![]
        };

        if self.config.parallel {
            let ast_start = Instant::now();
            let type_start = Instant::now();
            let ai_start = Instant::now();

            // 等待所有并行任务完成
            let mut ast_issues = Vec::new();
            let mut type_issues = Vec::new();
            let mut ai_issues = Vec::new();

            for (phase, handle) in phases {
                match handle.await {
                    Ok(Ok(issues)) => {
                        match phase {
                            "ast" => {
                                stats.ast_time_ms = ast_start.elapsed().as_millis() as u64;
                                ast_issues = issues;
                            }
                            "type" => {
                                stats.type_time_ms = type_start.elapsed().as_millis() as u64;
                                type_issues = issues;
                            }
                            "ai" => {
                                stats.ai_time_ms = ai_start.elapsed().as_millis() as u64;
                                ai_issues = issues;
                            }
                            _ => {}
                        }
                    }
                    Ok(Err(e)) => {
                        tracing::warn!("Phase {} failed: {}", phase, e);
                        match phase {
                            "ast" => ast_issues.push(Issue::error("ast", &format!("AST check error: {}", e))),
                            "type" => type_issues.push(Issue::error("type", &format!("Type check error: {}", e))),
                            "ai" => ai_issues.push(Issue::error("ai", &format!("AI check error: {}", e))),
                            _ => {}
                        }
                    }
                    Err(e) => {
                        tracing::warn!("Phase {} panicked: {}", phase, e);
                    }
                }
            }

            all_issues.extend(ast_issues);
            all_issues.extend(type_issues);
            all_issues.extend(ai_issues);
        } else {
            // 串行执行
            // Phase 1: AST check
            let ast_start = Instant::now();
            match self.ast_checker.check(root).await {
                Ok(issues) => {
                    stats.ast_time_ms = ast_start.elapsed().as_millis() as u64;
                    all_issues.extend(issues);
                }
                Err(e) => {
                    stats.ast_time_ms = ast_start.elapsed().as_millis() as u64;
                    all_issues.push(Issue::error("ast", &format!("AST check error: {}", e)));
                }
            }

            // Phase 2: Type check
            let type_start = Instant::now();
            match self.type_checker.check(root).await {
                Ok(issues) => {
                    stats.type_time_ms = type_start.elapsed().as_millis() as u64;
                    all_issues.extend(issues);
                }
                Err(e) => {
                    stats.type_time_ms = type_start.elapsed().as_millis() as u64;
                    all_issues.push(Issue::error("type", &format!("Type check error: {}", e)));
                }
            }

            // Phase 3: AI logic check
            let ai_start = Instant::now();
            match self.ai_checker.check(root).await {
                Ok(issues) => {
                    stats.ai_time_ms = ai_start.elapsed().as_millis() as u64;
                    all_issues.extend(issues);
                }
                Err(e) => {
                    stats.ai_time_ms = ai_start.elapsed().as_millis() as u64;
                    all_issues.push(Issue::error("ai", &format!("AI check error: {}", e)));
                }
            }
        }

        // 自动修复
        let fix_count = self.auto_fixer.apply_fixes(&mut all_issues, root).await;

        stats.auto_fixed = fix_count;
        stats.total_time_ms = start.elapsed().as_millis() as u64;
        stats.total_issues = all_issues.len();
        stats.errors = all_issues.iter().filter(|i| i.severity == "error").count();
        stats.warnings = all_issues.iter().filter(|i| i.severity == "warning").count();

        let passed = if self.config.fail_on_warning {
            stats.errors == 0 && stats.warnings == 0
        } else {
            stats.errors == 0
        };

        CiReport { issues: all_issues, stats, passed }
    }

    pub async fn run_and_print(&self) {
        let report = self.run().await;
        match self.config.output_format.as_str() {
            "json" => println!("{}", report.to_json()),
            "markdown" | "md" => println!("{}", report.to_markdown()),
            _ => println!("{}", report.to_string()),
        }
    }
}
