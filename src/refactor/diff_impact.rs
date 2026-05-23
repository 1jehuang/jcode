//! # Diff Impact Analysis — 差异影响分析（借鉴 Understand-Anything /understand-diff）
//!
//! 在执行代码修改前，分析本次 diff 会影响哪些模块、函数和文件。
//! 帮助开发者预览和规避"改了A坏了B"的蝴蝶效应。
//!
//! 与 CarpAI 现有 refactor_engine、diff_engine、refactor_verify_pipeline 配合使用。

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// 影响类型
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ImpactType {
    /// 直接修改
    Direct,
    /// 间接依赖
    IndirectDependency,
    /// 接口变更
    ApiChange,
    /// 类型变更
    TypeChange,
    /// 可能破坏
    Breaking,
}

/// 影响范围
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImpactScope {
    /// 直接修改的文件
    pub directly_modified: Vec<String>,
    /// 受影响的依赖文件（引入此文件或被此文件引入）
    pub affected_dependents: Vec<String>,
    /// 可能破坏的 API
    pub breaking_apis: Vec<String>,
    /// 影响详情
    pub details: Vec<ImpactDetail>,
}

/// 单条影响详情
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImpactDetail {
    pub file: String,
    pub impact_type: ImpactType,
    pub symbol: String,
    pub description: String,
    pub severity: Severity,
}

/// 严重程度
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum Severity {
    Low,
    Medium,
    High,
    Critical,
}

/// Diff 分析器
pub struct DiffImpactAnalyzer;

impl DiffImpactAnalyzer {
    /// 分析 diff 的影响范围
    ///
    /// `diff_lines`: 变更的行内容列表
    /// `project_files`: 项目中所有文件的导入依赖映射
    pub fn analyze(
        diff_lines: &[String],
        project_deps: &HashMap<String, Vec<String>>,
    ) -> ImpactScope {
        let mut directly_modified = Vec::new();
        let mut affected_dependents = HashSet::new();
        let mut breaking_apis = Vec::new();
        let mut details = Vec::new();

        // 1. 从 diff 中提取直接修改的文件
        for line in diff_lines {
            if line.starts_with("--- a/") || line.starts_with("+++ b/") {
                let path = line.trim_start_matches("--- a/").trim_start_matches("+++ b/");
                if !directly_modified.contains(&path.to_string()) {
                    directly_modified.push(path.to_string());
                }
            }
        }

        // 2. 追踪受影响的依赖
        for modified in &directly_modified {
            if let Some(dependents) = project_deps.get(modified) {
                for dep in dependents {
                    if dep != modified {
                        affected_dependents.insert(dep.clone());
                        details.push(ImpactDetail {
                            file: dep.clone(),
                            impact_type: ImpactType::IndirectDependency,
                            symbol: modified.clone(),
                            description: format!("{} 的修改可能影响 {}", modified, dep),
                            severity: Severity::Medium,
                        });
                    }
                }
            }
        }

        // 3. 检测 API 变更风险
        for line in diff_lines {
            if line.contains("pub fn") || line.contains("pub struct") || line.contains("pub enum")
                || line.contains("pub trait") || line.contains("pub type")
            {
                breaking_apis.push(line.trim().to_string());
                details.push(ImpactDetail {
                    file: String::new(),
                    impact_type: ImpactType::ApiChange,
                    symbol: line.trim().to_string(),
                    description: format!("公开 API 变更: {}", line.trim()),
                    severity: Severity::High,
                });
            }
        }

        // 4. 检测破坏性变更
        for line in diff_lines {
            if line.starts_with('-') && !line.starts_with("---") {
                // 删除的行可能是 breaking change
                let trimmed = line.trim_start_matches('-').trim();
                if trimmed.starts_with("fn ") || trimmed.starts_with("struct ")
                    || trimmed.starts_with("enum ") || trimmed.starts_with("trait ")
                {
                    breaking_apis.push(format!("REMOVED: {}", trimmed));
                    details.push(ImpactDetail {
                        file: String::new(),
                        impact_type: ImpactType::Breaking,
                        symbol: trimmed.to_string(),
                        description: format!("可能破坏性变更: 移除了 {}", trimmed),
                        severity: Severity::Critical,
                    });
                }
            }
        }

        ImpactScope {
            directly_modified,
            affected_dependents: affected_dependents.into_iter().collect(),
            breaking_apis,
            details,
        }
    }

    /// 生成影响报告（供 LLM 消费）
    pub fn format_report(scope: &ImpactScope) -> String {
        let mut report = String::new();
        report.push_str("# Diff 影响分析报告\n\n");

        report.push_str(&format!("## 直接修改文件 ({} 个)\n", scope.directly_modified.len()));
        for f in &scope.directly_modified {
            report.push_str(&format!("- `{}`\n", f));
        }

        if !scope.affected_dependents.is_empty() {
            report.push_str(&format!("\n## 间接受影响文件 ({} 个)\n", scope.affected_dependents.len()));
            for f in &scope.affected_dependents {
                report.push_str(&format!("- `{}`\n", f));
            }
        }

        if !scope.breaking_apis.is_empty() {
            report.push_str(&format!("\n## ⚠️ API 变更 ({} 处)\n", scope.breaking_apis.len()));
            for api in &scope.breaking_apis {
                report.push_str(&format!("- `{}`\n", api));
            }
        }

        report
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_diff_analysis() {
        let diff = vec![
            "--- a/src/auth.rs".into(),
            "+++ b/src/auth.rs".into(),
            "-pub fn old_auth() {}".into(),
            "+pub fn new_auth() {}".into(),
        ];
        let mut deps = HashMap::new();
        deps.insert("src/auth.rs".into(), vec!["src/server.rs".into(), "src/api.rs".into()]);

        let scope = DiffImpactAnalyzer::analyze(&diff, &deps);
        assert_eq!(scope.directly_modified.len(), 1);
        assert!(scope.affected_dependents.contains(&"src/server.rs".to_string()));
        assert!(!scope.breaking_apis.is_empty());
    }
}
