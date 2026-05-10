use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
pub enum CodeValueCategory {
    #[serde(rename = "预留")]
    Reserved,
    #[serde(rename = "遗留")]
    Legacy,
    #[serde(rename = "缺失功能")]
    MissingFeature,
    #[serde(rename = "无效")]
    Invalid,
    #[serde(rename = "重复")]
    Duplicate,
    #[serde(rename = "冗余")]
    Redundant,
}

impl CodeValueCategory {
    pub fn display_name(&self) -> &'static str {
        match self {
            CodeValueCategory::Reserved => "预留",
            CodeValueCategory::Legacy => "遗留",
            CodeValueCategory::MissingFeature => "缺失功能",
            CodeValueCategory::Invalid => "无效",
            CodeValueCategory::Duplicate => "重复",
            CodeValueCategory::Redundant => "冗余",
        }
    }

    pub fn severity(&self) -> Severity {
        match self {
            CodeValueCategory::Reserved => Severity::Info,
            CodeValueCategory::Legacy => Severity::Low,
            CodeValueCategory::MissingFeature => Severity::Medium,
            CodeValueCategory::Invalid => Severity::High,
            CodeValueCategory::Duplicate => Severity::Medium,
            CodeValueCategory::Redundant => Severity::Low,
        }
    }

    pub fn action(&self) -> &'static str {
        match self {
            CodeValueCategory::Reserved => "保留 — 为规划中的功能预留的结构/字段",
            CodeValueCategory::Legacy => "建议迁移或清理 — 旧版本残留代码",
            CodeValueCategory::MissingFeature => "建议补全 — 声明但未完整实现的功能",
            CodeValueCategory::Invalid => "必须修复 — 永远无法执行的死代码",
            CodeValueCategory::Duplicate => "建议去重 — 多份重复的逻辑实现",
            CodeValueCategory::Redundant => "建议删除 — 无实际作用的冗余代码",
        }
    }
}

impl fmt::Display for CodeValueCategory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.display_name())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Severity {
    Info,
    Low,
    Medium,
    High,
    Critical,
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Severity::Info => write!(f, "INFO"),
            Severity::Low => write!(f, "LOW"),
            Severity::Medium => write!(f, "MEDIUM"),
            Severity::High => write!(f, "HIGH"),
            Severity::Critical => write!(f, "CRITICAL"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClassifiedDiagnostic {
    pub file_path: String,
    pub line: usize,
    pub column: usize,
    pub lint_code: String,
    pub message: String,
    pub category: CodeValueCategory,
    pub confidence: f64,
    pub rationale: String,
    pub item_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClassificationReport {
    pub generated_at: String,
    pub total_diagnostics: usize,
    pub by_category: Vec<CategorySummary>,
    pub diagnostics: Vec<ClassifiedDiagnostic>,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CategorySummary {
    pub category: CodeValueCategory,
    pub count: usize,
    pub severity: Severity,
    pub action: String,
}

impl ClassificationReport {
    pub fn new(diagnostics: Vec<ClassifiedDiagnostic>) -> Self {
        let total = diagnostics.len();
        let mut by_category: std::collections::BTreeMap<CodeValueCategory, usize> =
            std::collections::BTreeMap::new();

        for d in &diagnostics {
            *by_category.entry(d.category).or_insert(0) += 1;
        }

        let summaries: Vec<CategorySummary> = by_category
            .into_iter()
            .map(|(cat, count)| CategorySummary {
                category: cat,
                count,
                severity: cat.severity(),
                action: cat.action().to_string(),
            })
            .collect();

        let summary = Self::generate_summary(&summaries, total);

        ClassificationReport {
            generated_at: chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
            total_diagnostics: total,
            by_category: summaries,
            diagnostics,
            summary,
        }
    }

    fn generate_summary(summaries: &[CategorySummary], total: usize) -> String {
        if total == 0 {
            return "代码价值评估完成：未发现需要分类的诊断项，代码质量良好。".to_string();
        }

        let mut parts: Vec<String> = vec![format!("共分析 {} 个诊断项：", total)];

        for s in summaries {
            let pct = if total > 0 {
                (s.count as f64 / total as f64) * 100.0
            } else {
                0.0
            };
            parts.push(format!(
                "  {}({}): {} 项 ({:.1}%) — {}",
                s.category.display_name(),
                s.severity,
                s.count,
                pct,
                s.action
            ));
        }

        let high_count: usize = summaries
            .iter()
            .filter(|s| s.category.severity() >= Severity::High)
            .map(|s| s.count)
            .sum();

        if high_count > 0 {
            parts.push(format!(
                "\n⚠ 发现 {} 项高严重度问题（无效代码），建议优先处理。",
                high_count
            ));
        }

        parts.join("\n")
    }
}