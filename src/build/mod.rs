//! # Build 模式 — 计划 → 执行 → 验证 三阶段流水线
//!
//! ## 核心流程
//! ```text
//! 用户输入 "添加用户登录功能"
//!     ↓
//! [Phase 1: 规划]  LLM 分析需求 → 生成 Plan (多个 PlanStep)
//!     ↓
//! [Phase 2: 审批]  用户审批/调整 PlanSteps (可选 auto-approve)
//!     ↓
//! [Phase 3: 执行]  AI 逐步骤执行 (支持 auto-retry)
//!     ↓
//! [Phase 4: 验证]  micro-ci 检查 → 失败则 auto-fix → 重复
//!     ↓
//! [Phase 5: 报告]  生成 build 报告
//! ```
//!
//! ## 关键特性
//! - **计划先行**: BuildTurnStrategy 先规划再执行，避免随机修改
//! - **进度可视化**: 实时显示步骤状态 (⏳✅❌🔄)
//! - **自动重试**: 失败步骤自动修复重试 (max 3 次)
//! - **后置验证**: 自动触发 micro-ci 验证变更质量
//! - **增量执行**: 仅变更修改过的步骤
//! - **构建摘要**: 每个 Build 生成结构化 JSON 报告

// Re-export from jcode_build_support (replaces old src/build.rs)
pub use jcode_build_support::*;

use std::time::Instant;

// Re-export plan types
pub use jcode_plan::{Plan, PlanMode, PlanStep, StepStatus, PlanStats};

mod turn_strategy;
pub use turn_strategy::BuildTurnStrategy;

/// Build 引擎 — 编排 build 全流程
pub struct BuildEngine {
    config: BuildConfig,
}

/// Build 配置
#[derive(Debug, Clone)]
pub struct BuildConfig {
    /// 自动审批所有计划步骤 (默认 true — 追求极致体验)
    pub auto_approve: bool,
    /// 失败步骤最大重试次数
    pub max_retries: u32,
    /// 执行完成后是否自动运行 micro-ci 验证
    pub run_ci_after_build: bool,
    /// 构建结果输出路径 (None = stdout)
    pub report_path: Option<String>,
}

impl Default for BuildConfig {
    fn default() -> Self {
        Self {
            auto_approve: true,
            max_retries: 3,
            run_ci_after_build: true,
            report_path: None,
        }
    }
}

impl BuildEngine {
    pub fn new(config: BuildConfig) -> Self {
        Self { config }
    }

    /// 运行完整 Build 流水线
    pub async fn run(
        &self,
        _user_request: &str,
        _plan: &mut Plan,
    ) -> anyhow::Result<BuildReport> {
        let start = Instant::now();
        let mut report = BuildReport::new(_user_request);

        // Phase 1: 使用 LLM 生成计划 (由 agent 在 BuildTurnStrategy 中完成)
        // 这里记录计划信息
        let _total_steps = _plan.steps.len();
        report.plan_steps = _plan.steps.len() as u32;
        report.planning_time_ms = start.elapsed().as_millis() as u64;

        // Phase 2: 审批 (auto_approve 或等待用户)
        if self.config.auto_approve {
            _plan.approve_all();
            report.auto_approved = true;
        }

        // Phase 3-4: 执行 + 验证在 Agent / BuildTurnStrategy 中完成
        // 这里记录最终统计
        report.completed_steps = _plan.stats.completed_count;
        report.failed_steps = _plan.stats.failed_count;
        report.total_time_ms = start.elapsed().as_millis() as u64;

        Ok(report)
    }

    /// 获取 BuildTurnStrategy 实例
    pub fn strategy(&self) -> BuildTurnStrategy {
        BuildTurnStrategy::new(self.config.max_retries, self.config.run_ci_after_build)
    }
}

/// Build 报告
#[derive(Debug, Clone)]
pub struct BuildReport {
    pub request: String,
    pub status: BuildStatus,
    pub plan_steps: u32,
    pub completed_steps: u32,
    pub failed_steps: u32,
    pub planning_time_ms: u64,
    pub execution_time_ms: u64,
    pub verification_time_ms: u64,
    pub total_time_ms: u64,
    pub auto_approved: bool,
    pub retry_count: u32,
    pub ci_passed: bool,
}

impl BuildReport {
    pub fn new(request: &str) -> Self {
        Self {
            request: request.to_string(),
            status: BuildStatus::Planning,
            plan_steps: 0,
            completed_steps: 0,
            failed_steps: 0,
            planning_time_ms: 0,
            execution_time_ms: 0,
            verification_time_ms: 0,
            total_time_ms: 0,
            auto_approved: false,
            retry_count: 0,
            ci_passed: false,
        }
    }

    /// 生成人类可读报告
    pub fn to_string(&self) -> String {
        let icon = match self.status {
            BuildStatus::Success => "✅",
            BuildStatus::Partial => "⚠️",
            BuildStatus::Failed => "❌",
            _ => "⏳",
        };
        format!(
            r#"{icon} Build Report
═══════════════════════════════════
Request:    {}
Status:    {:?}
Steps:     {}/{} planned
Retries:   {}
Time:      {:.2}s total (plan {:.2}s + exec {:.2}s + verify {:.2}s)
CI:        {}
"#,
            self.request,
            self.status,
            self.completed_steps,
            self.plan_steps,
            self.retry_count,
            self.total_time_ms as f64 / 1000.0,
            self.planning_time_ms as f64 / 1000.0,
            self.execution_time_ms as f64 / 1000.0,
            self.verification_time_ms as f64 / 1000.0,
            if self.ci_passed { "✅ PASS" } else { "⏭️ skip" },
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildStatus {
    Planning,
    Executing,
    Verifying,
    Success,
    Partial,
    Failed,
}

// ── 进度条显示 ──

/// 实时显示 build 进度条
pub struct ProgressBar {
    total: u32,
    current: u32,
    label: String,
    start: Instant,
}

impl ProgressBar {
    pub fn new(total: u32, label: &str) -> Self {
        Self {
            total,
            current: 0,
            label: label.to_string(),
            start: Instant::now(),
        }
    }

    pub fn advance(&mut self, n: u32) {
        self.current = self.current.saturating_add(n);
        self.render();
    }

    pub fn render(&self) {
        let pct = if self.total > 0 {
            (self.current as f64 / self.total as f64) * 100.0
        } else {
            0.0
        };
        let bar_width: usize = 30;
        let filled = (pct / 100.0 * bar_width as f64) as usize;
        let empty = bar_width.saturating_sub(filled);
        let elapsed = self.start.elapsed().as_secs_f64();
        eprint!(
            "\r{} [{}{}] {:.0}% ({}/{}) — {:.1}s",
            self.label,
            "█".repeat(filled),
            "░".repeat(empty),
            pct,
            self.current,
            self.total,
            elapsed,
        );
        use std::io::Write;
        let _ = std::io::stderr().flush();
    }

    pub fn finish(&self) {
        eprintln!();
    }
}
