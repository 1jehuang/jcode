use crate::types::{ClassificationReport, ClassifiedDiagnostic, CodeValueCategory};
use crate::parser::ParsedDiagnostic;
use regex::Regex;
use std::collections::HashMap;

pub struct Classifier {
    reserved_keywords: Vec<Regex>,
    legacy_keywords: Vec<Regex>,
    reserved_paths: Vec<String>,
    legacy_paths: Vec<String>,
}

impl Classifier {
    pub fn new() -> Self {
        Classifier {
            reserved_keywords: vec![
                Regex::new(r"(?i)\b(planning|future|reserved|placeholder|stub|wip|pending)\b")
                    .unwrap(),
                Regex::new(r"(?i)\b(workspace_manager|tool_registry|plugin_system)\b").unwrap(),
                Regex::new(r"(?i)\b(build_engine|turn_strategy|overnight|swarm)\b").unwrap(),
            ],
            legacy_keywords: vec![
                Regex::new(r"(?i)\b(legacy|deprecated|obsolete|old_|previous_)\b").unwrap(),
                Regex::new(r"(?i)\b(create_desktop_shortcut|setup_hints|windows_setup)\b")
                    .unwrap(),
            ],
            reserved_paths: vec![
                "agent/".to_string(),
                "build/".to_string(),
                "bridge/".to_string(),
                "swarm/".to_string(),
                "overnight/".to_string(),
            ],
            legacy_paths: vec![
                "setup_hints".to_string(),
                "windows_setup".to_string(),
            ],
        }
    }

    pub fn classify(&self, diagnostics: Vec<ParsedDiagnostic>) -> ClassificationReport {
        tracing::info!("开始分类 {} 个诊断项", diagnostics.len());

        let mut classified: Vec<ClassifiedDiagnostic> = diagnostics
            .into_iter()
            .map(|d| self.classify_single(d))
            .collect();

        self.detect_duplicates(&mut classified);

        ClassificationReport::new(classified)
    }

    fn classify_single(&self, diag: ParsedDiagnostic) -> ClassifiedDiagnostic {
        let (category, confidence, rationale) = self.determine_category(&diag);

        let item_name = self.extract_item_name(&diag);

        ClassifiedDiagnostic {
            file_path: diag.file_path,
            line: diag.line,
            column: diag.column,
            lint_code: diag.lint_code,
            message: diag.message,
            category,
            confidence,
            rationale,
            item_name,
        }
    }

    fn determine_category(&self, diag: &ParsedDiagnostic) -> (CodeValueCategory, f64, String) {
        match diag.lint_code.as_str() {
            "dead_code" => self.classify_dead_code(diag),
            "unused_imports" => self.classify_unused_imports(diag),
            "unused_variables" => self.classify_unused_variables(diag),
            "unused_mut" => (
                CodeValueCategory::Redundant,
                0.95,
                "未使用的 mut 声明，属于冗余代码".to_string(),
            ),
            "unreachable_code" => self.classify_unreachable_code(diag),
            "unnecessary_cast" => (
                CodeValueCategory::Redundant,
                0.90,
                "不必要的类型转换，属于冗余代码".to_string(),
            ),
            "while_true" => (
                CodeValueCategory::Redundant,
                0.90,
                "可用 loop 替代的 while true，属于冗余写法".to_string(),
            ),
            "unconditional_recursion" => (
                CodeValueCategory::Invalid,
                0.98,
                "无条件递归将导致栈溢出，属于无效代码".to_string(),
            ),
            "unused_must_use" => (
                CodeValueCategory::Redundant,
                0.85,
                "未使用 must_use 类型的返回值，属于冗余忽略".to_string(),
            ),
            "unused_results" => (
                CodeValueCategory::Redundant,
                0.85,
                "未使用 Result 返回值，可能遗漏错误处理".to_string(),
            ),
            _ => {
                if diag.level == "error" {
                    (
                        CodeValueCategory::Invalid,
                        0.70,
                        format!("编译错误 ({}): {}", diag.lint_code, diag.message),
                    )
                } else {
                    (
                        CodeValueCategory::Redundant,
                        0.60,
                        format!("未分类警告 ({}): {}", diag.lint_code, diag.message),
                    )
                }
            }
        }
    }

    fn classify_dead_code(&self, diag: &ParsedDiagnostic) -> (CodeValueCategory, f64, String) {
        let mut scores: HashMap<CodeValueCategory, f64> = HashMap::new();

        if self.is_in_reserved_path(&diag.file_path) {
            *scores.entry(CodeValueCategory::Reserved).or_insert(0.0) += 0.4;
        }

        if self.matches_reserved_keywords(&diag.message) {
            *scores.entry(CodeValueCategory::Reserved).or_insert(0.0) += 0.5;
        }

        if self.matches_legacy_keywords(&diag.message) {
            *scores.entry(CodeValueCategory::Legacy).or_insert(0.0) += 0.6;
        }

        if self.is_in_legacy_path(&diag.file_path) {
            *scores.entry(CodeValueCategory::Legacy).or_insert(0.0) += 0.5;
        }

        if let Some(ref snippet) = diag.source_snippet {
            if snippet.contains("TODO") || snippet.contains("FIXME") || snippet.contains("HACK") {
                *scores.entry(CodeValueCategory::Reserved).or_insert(0.0) += 0.3;
            }
            if snippet.contains("#[allow(dead_code)]") {
                *scores.entry(CodeValueCategory::Reserved).or_insert(0.0) += 0.3;
            }
        }

        if let Some(ref name) = self.extract_item_name(diag) {
            if name.starts_with('_') {
                *scores.entry(CodeValueCategory::Reserved).or_insert(0.0) += 0.3;
            }
            if is_uppercase(name) {
                *scores.entry(CodeValueCategory::MissingFeature).or_insert(0.0) += 0.4;
            }
        }

        if diag.message.contains("field")
            && !diag.message.contains("function")
            && !diag.message.contains("method")
        {
            *scores.entry(CodeValueCategory::Reserved).or_insert(0.0) += 0.3;
        }

        if diag.file_path.contains("test") || diag.file_path.contains("_test") {
            *scores.entry(CodeValueCategory::Redundant).or_insert(0.0) += 0.4;
        }

        self.pick_best_category(scores, CodeValueCategory::Reserved, 0.55)
    }

    fn classify_unused_imports(
        &self,
        diag: &ParsedDiagnostic,
    ) -> (CodeValueCategory, f64, String) {
        if self.is_in_reserved_path(&diag.file_path) {
            return (
                CodeValueCategory::MissingFeature,
                0.45,
                "预留给规划功能的导入，功能完成后将使用".to_string(),
            );
        }

        (
            CodeValueCategory::Redundant,
            0.90,
            "未使用的导入，属于冗余代码".to_string(),
        )
    }

    fn classify_unused_variables(
        &self,
        diag: &ParsedDiagnostic,
    ) -> (CodeValueCategory, f64, String) {
        if let Some(ref name) = self.extract_item_name(diag)
            && name.starts_with('_')
        {
            return (
                CodeValueCategory::Reserved,
                0.85,
                format!("以下划线前缀标记的预留变量 '{}'", name),
            );
        }

        if let Some(ref snippet) = diag.source_snippet
            && (snippet.contains("TODO") || snippet.contains("FIXME"))
        {
            return (
                CodeValueCategory::MissingFeature,
                0.55,
                "关联 TODO/FIXME 标记的变量，功能未完成".to_string(),
            );
        }

        (
            CodeValueCategory::Redundant,
            0.80,
            "未使用的局部变量，属于冗余代码".to_string(),
        )
    }

    fn classify_unreachable_code(
        &self,
        diag: &ParsedDiagnostic,
    ) -> (CodeValueCategory, f64, String) {
        if let Some(ref snippet) = diag.source_snippet
            && (snippet.contains("TODO")
                || snippet.contains("FIXME")
                || snippet.contains("WIP"))
        {
            return (
                CodeValueCategory::MissingFeature,
                0.50,
                "不可达代码区域标记了 TODO/WIP，功能待实现".to_string(),
            );
        }

        (
            CodeValueCategory::Invalid,
            0.95,
            "永远无法执行的代码路径，属于无效代码".to_string(),
        )
    }

    fn detect_duplicates(&self, classified: &mut [ClassifiedDiagnostic]) {
        let mut name_locations: HashMap<String, Vec<usize>> = HashMap::new();

        for (i, d) in classified.iter().enumerate() {
            if let Some(ref name) = d.item_name
                && !name.is_empty()
                && name.len() > 3
            {
                name_locations.entry(name.clone()).or_default().push(i);
            }
        }

        let duplicate_threshold = 2;
        for (name, indices) in &name_locations {
            if indices.len() >= duplicate_threshold {
                let is_dup = indices.iter().any(|&i| {
                    let diag = &classified[i];
                    diag.category == CodeValueCategory::Reserved
                        || diag.category == CodeValueCategory::Legacy
                });

                if is_dup {
                    for &i in indices {
                        let diag = &mut classified[i];
                        diag.category = CodeValueCategory::Duplicate;
                        diag.confidence = 0.65;
                        diag.rationale = format!(
                            "发现重复定义: 函数/结构体 '{}' 在多处定义 (共 {} 处)",
                            name,
                            indices.len()
                        );
                    }
                }
            }
        }
    }

    fn is_in_reserved_path(&self, file_path: &str) -> bool {
        self.reserved_paths
            .iter()
            .any(|p| file_path.contains(p))
    }

    fn is_in_legacy_path(&self, file_path: &str) -> bool {
        self.legacy_paths
            .iter()
            .any(|p| file_path.contains(p))
    }

    fn matches_reserved_keywords(&self, text: &str) -> bool {
        self.reserved_keywords
            .iter()
            .any(|re| re.is_match(text))
    }

    fn matches_legacy_keywords(&self, text: &str) -> bool {
        self.legacy_keywords.iter().any(|re| re.is_match(text))
    }

    fn extract_item_name(&self, diag: &ParsedDiagnostic) -> Option<String> {
        let re = Regex::new(
            r"(?:struct|fn|const|static|enum|trait|type|mod|constant|variable)\b[\s:]+`?(\w+)`?",
        )
        .unwrap();
        if let Some(caps) = re.captures(&diag.message) {
            return Some(caps[1].to_string());
        }

        let re_field = Regex::new(r"field\s+`?(\w+)`?").unwrap();
        if let Some(caps) = re_field.captures(&diag.message) {
            return Some(caps[1].to_string());
        }

        let re_import = Regex::new(r"unused import[s]?:?\s*`?(\w+)`?").unwrap();
        if let Some(caps) = re_import.captures(&diag.message) {
            return Some(caps[1].to_string());
        }

        None
    }

    fn pick_best_category(
        &self,
        scores: HashMap<CodeValueCategory, f64>,
        default: CodeValueCategory,
        min_confidence: f64,
    ) -> (CodeValueCategory, f64, String) {
        if scores.is_empty() {
            return (
                default,
                min_confidence,
                format!("默认分类为「{}」: {}", default.display_name(), default.action()),
            );
        }

        let best = scores
            .iter()
            .max_by(|a, b| {
                a.1.partial_cmp(b.1)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .unwrap();

        let score = *best.1;
        let confidence = score.min(0.95).max(min_confidence);
        let category = *best.0;

        (
            category,
            confidence,
            format!(
                "基于启发式规则分类为「{}」 (得分: {:.2}): {}",
                category.display_name(),
                score,
                category.action()
            ),
        )
    }
}

impl Default for Classifier {
    fn default() -> Self {
        Self::new()
    }
}

fn is_uppercase(s: &str) -> bool {
    s.chars()
        .filter(|c| c.is_alphabetic())
        .all(|c| c.is_uppercase())
        && s.len() > 1
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_diag(
        lint_code: &str,
        message: &str,
        file_path: &str,
        snippet: Option<&str>,
    ) -> ParsedDiagnostic {
        ParsedDiagnostic {
            file_path: file_path.to_string(),
            line: 10,
            column: 5,
            lint_code: lint_code.to_string(),
            message: message.to_string(),
            level: "warning".to_string(),
            rendered: None,
            crate_name: "test-crate".to_string(),
            source_snippet: snippet.map(|s| s.to_string()),
        }
    }

    #[test]
    fn test_dead_code_field_is_reserved() {
        let classifier = Classifier::new();
        let diag = make_diag(
            "dead_code",
            "field `workspace_manager` is never used",
            "src/agent/mod.rs",
            Some("    workspace_manager: Option<WorkspaceManager>,"),
        );
        let (cat, _, _) = classifier.determine_category(&diag);
        assert_eq!(cat, CodeValueCategory::Reserved);
    }

    #[test]
    fn test_unused_imports_is_redundant() {
        let classifier = Classifier::new();
        let diag = make_diag(
            "unused_imports",
            "unused import: `std::collections::HashMap`",
            "src/main.rs",
            None,
        );
        let (cat, _, _) = classifier.determine_category(&diag);
        assert_eq!(cat, CodeValueCategory::Redundant);
    }

    #[test]
    fn test_underscore_variable_is_reserved() {
        let classifier = Classifier::new();
        let diag = make_diag(
            "unused_variables",
            "unused variable: `_reserved_var`",
            "src/lib.rs",
            Some("let _reserved_var = compute();"),
        );
        let (cat, _, _) = classifier.determine_category(&diag);
        assert_eq!(cat, CodeValueCategory::Reserved);
    }

    #[test]
    fn test_unreachable_code_is_invalid() {
        let classifier = Classifier::new();
        let diag = make_diag(
            "unreachable_code",
            "unreachable statement",
            "src/cli/dispatch.rs",
            Some("    Ok(());"),
        );
        let (cat, _, _) = classifier.determine_category(&diag);
        assert_eq!(cat, CodeValueCategory::Invalid);
    }

    #[test]
    fn test_legacy_function_in_legacy_path() {
        let classifier = Classifier::new();
        let diag = make_diag(
            "dead_code",
            "function `create_desktop_shortcut` is never used",
            "src/setup_hints.rs",
            Some("fn create_desktop_shortcut() {"),
        );
        let (cat, _, _) = classifier.determine_category(&diag);
        assert_eq!(cat, CodeValueCategory::Legacy);
    }

    #[test]
    fn test_dead_code_const_is_missing_feature() {
        let classifier = Classifier::new();
        let diag = make_diag(
            "dead_code",
            "constant `RELOAD_HANDOFF_EVENT_POLL_MS` is never used",
            "src/reload_state.rs",
            Some("const RELOAD_HANDOFF_EVENT_POLL_MS: u64 = 500;"),
        );
        let (cat, _, _) = classifier.determine_category(&diag);
        assert_eq!(cat, CodeValueCategory::MissingFeature);
    }
}