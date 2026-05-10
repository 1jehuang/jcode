use crate::CiStats;
use serde::Serialize;

/// CI 报告
#[derive(Debug, Clone, Serialize)]
pub struct CiReport {
    pub issues: Vec<Issue>,
    pub stats: CiStats,
    pub passed: bool,
}

/// 单个问题
#[derive(Debug, Clone, Serialize)]
pub struct Issue {
    pub phase: String,
    pub severity: String,
    pub message: String,
    pub line: Option<u32>,
    pub fix_suggestion: Option<String>,
    pub file: Option<String>,
}

impl Issue {
    /// 创建错误问题
    pub fn error(phase: &str, msg: &str) -> Self {
        Self {
            phase: phase.to_string(),
            severity: "error".into(),
            message: msg.into(),
            line: None,
            fix_suggestion: None,
            file: None,
        }
    }

    /// 创建警告问题
    pub fn warning(phase: &str, msg: &str) -> Self {
        Self {
            phase: phase.to_string(),
            severity: "warning".into(),
            message: msg.into(),
            line: None,
            fix_suggestion: None,
            file: None,
        }
    }

    /// 创建信息问题
    pub fn info(phase: &str, msg: &str) -> Self {
        Self {
            phase: phase.to_string(),
            severity: "info".into(),
            message: msg.into(),
            line: None,
            fix_suggestion: None,
            file: None,
        }
    }

    /// 创建带修复建议的错误
    pub fn error_with_fix(phase: &str, msg: &str, fix: &str) -> Self {
        let (file, line) = Self::parse_file_and_line(msg);
        Self {
            phase: phase.to_string(),
            severity: "error".into(),
            message: msg.into(),
            line,
            fix_suggestion: Some(fix.into()),
            file,
        }
    }

    /// 创建带修复建议的警告
    pub fn warning_with_fix(phase: &str, msg: &str, fix: &str) -> Self {
        let (file, line) = Self::parse_file_and_line(msg);
        Self {
            phase: phase.to_string(),
            severity: "warning".into(),
            message: msg.into(),
            line,
            fix_suggestion: Some(fix.into()),
            file,
        }
    }

    /// 从消息中解析文件名和行号（格式: "file:line ..."）
    fn parse_file_and_line(msg: &str) -> (Option<String>, Option<u32>) {
        // 尝试匹配 "path/file.rs:42" 或 "path/file.rs:42:5" 格式
        let re = regex::Regex::new(r"^(.+?):(\d+)(?::\d+)?\s").ok();
        if let Some(re) = re {
            if let Some(caps) = re.captures(msg) {
                let file = caps.get(1).map(|m| m.as_str().to_string());
                let line = caps
                    .get(2)
                    .and_then(|m| m.as_str().parse::<u32>().ok());
                return (file, line);
            }
        }
        (None, None)
    }
}

impl CiReport {
    /// 生成人类可读的报告
    pub fn to_string(&self) -> String {
        let icon = if self.passed { "✅" } else { "❌" };
        let mut out = format!(
            "\n{} Micro CI Report\n{}\n\n",
            icon, "═".repeat(50)
        );

        // 按严重级别分组输出
        let errors: Vec<_> = self.issues.iter().filter(|i| i.severity == "error").collect();
        let warnings: Vec<_> = self.issues.iter().filter(|i| i.severity == "warning").collect();
        let infos: Vec<_> = self.issues.iter().filter(|i| i.severity == "info").collect();

        if !errors.is_empty() {
            out.push_str(&format!("🔴 Errors ({}):\n", errors.len()));
            for issue in &errors {
                out.push_str(&format!("   [{}] {}", issue.phase, issue.message));
                if let Some(fix) = &issue.fix_suggestion {
                    out.push_str(&format!("\n        ⚡ Fix: {}", fix));
                }
                out.push('\n');
            }
            out.push('\n');
        }

        if !warnings.is_empty() {
            out.push_str(&format!("🟡 Warnings ({}):\n", warnings.len()));
            for issue in &warnings {
                out.push_str(&format!("   [{}] {}", issue.phase, issue.message));
                if let Some(fix) = &issue.fix_suggestion {
                    out.push_str(&format!("\n        💡 Suggestion: {}", fix));
                }
                out.push('\n');
            }
            out.push('\n');
        }

        if !infos.is_empty() {
            out.push_str(&format!("🔵 Info ({}):\n", infos.len()));
            for issue in &infos {
                out.push_str(&format!("   [{}] {}\n", issue.phase, issue.message));
            }
            out.push('\n');
        }

        out.push_str(&format!(
            "\nStats:\n  AST:  {}ms\n  Type: {}ms\n  AI:   {}ms\n  Total: {}ms\n",
            self.stats.ast_time_ms, self.stats.type_time_ms,
            self.stats.ai_time_ms, self.stats.total_time_ms
        ));
        out.push_str(&format!(
            "Issues: {} errors, {} warnings\n",
            self.stats.errors, self.stats.warnings
        ));
        out.push_str(&format!("Verdict: {}\n", if self.passed { "✅ PASS" } else { "❌ FAIL" }));
        out
    }

    /// 生成 JSON 格式的报告
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".to_string())
    }

    /// 生成 Markdown 格式的报告
    pub fn to_markdown(&self) -> String {
        let icon = if self.passed { "✅" } else { "❌" };
        let mut out = format!("# {} Micro CI Report\n\n", icon);

        for issue in &self.issues {
            let emoji = match issue.severity.as_str() {
                "error" => "🔴",
                "warning" => "🟡",
                _ => "🔵",
            };
            out.push_str(&format!("- {} `[{}]` {}", emoji, issue.phase, issue.message));
            if let Some(fix) = &issue.fix_suggestion {
                out.push_str(&format!("\n  - *Fix:* {}", fix));
            }
            out.push('\n');
        }

        out.push_str(&format!(
            "\n## Stats\n\n| Phase | Time |\n|-------|------|\n\
             | AST | {}ms |\n| Type | {}ms |\n| AI | {}ms |\n| **Total** | **{}ms** |\n\n\
             **Verdict:** {}\n",
            self.stats.ast_time_ms, self.stats.type_time_ms,
            self.stats.ai_time_ms, self.stats.total_time_ms,
            if self.passed { "✅ PASS" } else { "❌ FAIL" }
        ));
        out
    }
}
