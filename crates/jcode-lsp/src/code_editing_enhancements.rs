// code_editing_enhancements.rs
// ════════════════════════════════════════════════════════════════
// 代码编辑能力完善 — QuickFix / Review / FormatCode
//
// ## 核心能力对标 Claude Code/Cursor
// 1. **QuickFix**: 自动修复编译错误、lint 警告
// 2. **Review**: 安全审查 + 性能审查 + 最佳实践检查
// 3. **FormatCode**: 智能格式化（多语言支持）
//
// ## 架构设计
// ┌─────────────────────────────────────────────┐
// │         Code Editing Engine              │
// │  ┌──────────┐ ┌──────────┐ ┌─────────┐│
// │  │ QuickFix │ │  Review  │ │ Format  ││
// │  │ Engine   │ │ Engine   │ │ Engine  ││
// │  └────┬─────┘ └────┬─────┘ └────┬────┘│
// │       │            │            │      │
// │       ▼            ▼            ▼      │
// │  ┌─────────────────────────────────┐   │
// │  │     LSP Integration Layer      │   │
// │  │  (textDocument/codeAction)    │   │
// │  └─────────────────────────────────┘   │
// └─────────────────────────────────────┘

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use lsp_types::*;

// ════════════════════════════════════════════════════════════════
// 1. QuickFix Engine — 自动修复编译错误和 lint 警告
// ════════════════════════════════════════════════════════════════

/// QuickFix 结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuickFixResult {
    /// 是否有可用的修复
    pub has_fixes: bool,
    /// 修复建议列表
    pub fixes: Vec<FixSuggestion>,
    /// 应用的修复数量
    pub applied_count: usize,
    /// 是否全部成功
    pub all_success: bool,
}

/// 单个修复建议
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FixSuggestion {
    /// 修复类型
    pub fix_type: FixCategory,
    /// 问题描述
    pub title: String,
    /// 详细描述
    pub description: String,
    /// 文件路径
    pub file_path: String,
    /// 行号（可选）
    pub line: Option<u32>,
    /// 列号（可选）
    pub character: Option<u32>,
    /// 原始代码（可选）
    pub original_code: Option<String>,
    /// 修复后的代码
    pub fixed_code: String,
    /// 置信度 (0.0 - 1.0)
    pub confidence: f64,
    /// 是否自动应用
    pub auto_applicable: bool,
}

/// 修复类别
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FixCategory {
    CompilationError,
    LintWarning,
    SecurityVulnerability,
    PerformanceIssue,
    StyleViolation,
    BestPractice,
}

impl std::fmt::Display for FixCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FixCategory::CompilationError => write!(f, "🔴 Compilation Error"),
            FixCategory::LintWarning => write!(f, "⚠️ Lint Warning"),
            FixCategory::SecurityVulnerability => write!(f, "🔒 Security Vulnerability"),
            FixCategory::PerformanceIssue => write!(f, "⚡ Performance Issue"),
            FixCategory::StyleViolation => write!(f, "🎨 Style Violation"),
            FixCategory::BestPractice => write!(f, "💡 Best Practice"),
        }
    }
}

/// QuickFix 引擎
pub struct QuickFixEngine {
    config: QuickFixConfig,
    fix_patterns: Arc<RwLock<Vec<FixPattern>>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuickFixConfig {
    /// 是否启用自动修复
    pub auto_apply: bool,
    /// 最大修复数量
    pub max_fixes_per_file: usize,
    /// 最低置信度阈值
    pub min_confidence: f64,
    /// 支持的语言列表
    pub supported_languages: Vec<String>,
}

impl Default for QuickFixConfig {
    fn default() -> Self {
        Self {
            auto_apply: false,
            max_fixes_per_file: 10,
            min_confidence: 0.7,
            supported_languages: vec![
                "rust".to_string(),
                "python".to_string(),
                "javascript".to_string(),
                "typescript".to_string(),
                "go".to_string(),
                "java".to_string(),
            ],
        }
    }
}

/// 修复模式
struct FixPattern {
    category: FixCategory,
    pattern: regex::Regex,
    fix_template: String,
    confidence: f64,
    description: String,
}

impl QuickFixEngine {
    /// 创建新的 QuickFix 引擎
    pub fn new() -> Self {
        Self::with_config(QuickFixConfig::default())
    }

    /// 使用配置创建
    pub fn with_config(config: QuickFixConfig) -> Self {
        let mut engine = Self {
            config,
            fix_patterns: Arc::new(RwLock::new(Vec::new())),
        };
        
        // 注册内置的修复模式
        engine.register_builtin_patterns();
        
        engine
    }

    /// 注册内置的修复模式
    fn register_builtin_patterns(&mut self) {
        let patterns = vec![
            // Rust 未使用变量
            FixPattern {
                category: FixCategory::LintWarning,
                pattern: regex::Regex::new(r"warning:\[unused_variables\]\s*:\s*(\w+)").unwrap(),
                fix_template: "${var}_unused".to_string(),
                confidence: 0.95,
                description: "Add underscore prefix to unused variable".to_string(),
            },
            // Rust 缺少分号
            FixPattern {
                category: FixCategory::CompilationError,
                pattern: regex::Regex::new(r"error\[E0425\].*expected one of").unwrap(),
                fix_template: ";".to_string(),
                confidence: 0.9,
                description: "Add semicolon at end of statement".to_string(),
            },
            // Rust 类型不匹配
            FixPattern {
                category: FixCategory::CompilationError,
                pattern: regex::Regex::new(r"error\[E0308\].*mismatched types").unwrap(),
                fix_template: "".to_string(), // 需要更复杂的处理
                confidence: 0.6,
                description: "Type mismatch - requires manual review".to_string(),
            },
            // Python IndentationError
            FixPattern {
                category: FixCategory::StyleViolation,
                pattern: regex::Regex::new(r"IndentationError.*expected an indented block").unwrap(),
                fix_template: "    ".to_string(),
                confidence: 0.85,
                description: "Add indentation to block".to_string(),
            },
            // Python UndefinedVariable
            FixPattern {
                category: FixCategory::CompilationError,
                pattern: regex::Regex::new(r"NameError.*name '(\w+)' is not defined").unwrap(),
                fix_template: "# TODO: Define ${var}".to_string(),
                confidence: 0.75,
                description: "Define the undefined variable".to_string(),
            },
        ];
        
        *self.fix_patterns.write().unwrap() = patterns;
    }

    /// 分析并生成修复建议
    ///
    /// # Arguments
    /// * `error_output` - 编译器/ linter 的错误输出
    /// * `file_path` - 文件路径
    /// * `language` - 编程语言
    ///
    /// # Returns
    /// 返回 QuickFixResult，包含所有匹配的修复建议
    pub async fn analyze_and_suggest(
        &self,
        error_output: &str,
        file_path: &str,
        language: &str,
    ) -> QuickFixResult {
        debug!("Analyzing errors for quick fix suggestions...");
        
        let mut fixes = Vec::new();
        let patterns = self.fix_patterns.read().await;
        
        for pattern in patterns.iter() {
            if let Some(caps) = pattern.pattern.captures(error_output).next() {
                let var_name = caps.get(1)
                    .map(|m| m.as_str().to_string())
                    .unwrap_or_default();
                
                let fixed_code = pattern.fix_template
                    .replace("${var}", &var_name);
                
                // 提取行号信息（如果有的话）
                let line = self.extract_line_number(error_output);
                
                fixes.push(FixSuggestion {
                    fix_type: pattern.category,
                    title: format!("{}: {}", pattern.description, var_name),
                    description: format!(
                        "Auto-fix suggestion for {} in {}",
                        pattern.description, file_path
                    ),
                    file_path: file_path.to_string(),
                    line,
                    character: None,
                    original_code: None,
                    fixed_code,
                    confidence: pattern.confidence,
                    auto_applicable: pattern.confidence >= self.config.min_confidence,
                });
            }
        }
        
        // 按置信度和类别排序
        fixes.sort_by(|a, b| {
            b.confidence.partial_cmp(&a.confidence)
                .then_with(|| a.fix_type.cmp(&b.fix_type))
        });
        
        // 限制数量
        if fixes.len() > self.config.max_fixes_per_file {
            fixes.truncate(self.config.max_fixes_per_file);
        }
        
        QuickFixResult {
            has_fixes: !fixes.is_empty(),
            fixes,
            applied_count: 0,
            all_success: false,
        }
    }

    /// 应用单个修复
    pub fn apply_fix(
        &self,
        content: &str,
        fix: &FixSuggestion,
    ) -> Result<String, String> {
        if !fix.auto_applicable {
            return Err("Fix not auto-applicable (confidence too low)".to_string());
        }
        
        // 如果有行号，尝试在该行应用修复
        if let Some(line_num) = fix.line {
            let lines: Vec<&str> = content.lines().collect();
            
            if line_num > 0 && (line_num as usize) <= lines.len() {
                let target_line_idx = (line_num - 1) as usize;
                let original_line = lines[target_line_idx];
                
                // 根据修复类型应用不同的策略
                let new_line = match fix.fix_type {
                    FixCategory::CompilationError => {
                        // 对于编译错误，通常需要替换整行或添加内容
                        if !fix.fixed_code.is_empty() {
                            fix.fixed_code.clone()
                        } else {
                            original_line.to_string()
                        }
                    }
                    FixCategory::LintWarning => {
                        // 对于 lint 警告，可能只需要修改部分内容
                        original_line.to_string() // 保持原样，让用户决定
                    }
                    _ => original_line.to_string(),
                };
                
                // 构建新内容
                let mut result = lines[..target_line_idx].join("\n");
                result.push_str("\n");
                result.push_str(&new_line);
                result.push_str("\n");
                result.push_str(&lines[(target_line_idx + 1)..].join("\n"));
                
                return Ok(result);
            }
        }
        
        Err("Cannot apply fix: invalid line number or content".to_string())
    }

    /// 批量应用所有修复
    pub async fn apply_all_fixes(
        &self,
        content: &str,
        fixes: &[FixSuggestion],
    ) -> Result<(String, Vec<usize>), String> {
        let mut current_content = content.to_string();
        let mut applied_indices = Vec::new();
        
        for (idx, fix) in fixes.iter().enumerate() {
            match self.apply_fix(&current_content, fix) {
                Ok(new_content) => {
                    current_content = new_content;
                    applied_indices.push(idx);
                }
                Err(e) => {
                    warn!("Failed to apply fix {}: {}", idx, e);
                }
            }
        }
        
        if applied_indices.is_empty() {
            Err("No fixes could be applied".to_string())
        } else {
            Ok((current_content, applied_indices))
        }
    }

    /// 从错误输出中提取行号
    fn extract_line_number(&self, output: &str) -> Option<u32> {
        // 匹配常见的错误格式:
        // --> file.rs:line:col
        let line_re = regex::Regex::new(r"-->\s*.+?:(\d+):\d+").unwrap();
        
        line_re.captures(output)
            .and_then(|caps| caps.get(1))
            .and_then(|m| m.as_str().parse::<u32>().ok())
    }
}


// ════════════════════════════════════════════════════════════════
// 2. Review Engine — 安全审查 + 性能审查
// ════════════════════════════════════════════════════════════════

/// 审查结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeReviewResult {
    /// 审查的文件
    pub file_path: String,
    /// 总体评分 (0-100)
    pub overall_score: u8,
    /// 安全问题
    pub security_issues: Vec<SecurityIssue>,
    /// 性能问题
    pub performance_issues: Vec<PerformanceIssue>,
    /// 代码风格问题
    pub style_issues: Vec<StyleIssue>,
    /// 最佳实践建议
    pub best_practices: Vec<BestPracticeSuggestion>,
    /// 统计摘要
    pub summary: ReviewSummary,
}

/// 安全问题
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityIssue {
    /// 严重级别 (Critical/High/Medium/Low)
    pub severity: SeverityLevel,
    /// 规则 ID (如 CWE-xxx)
    pub rule_id: String,
    /// 标题
    pub title: String,
    /// 描述
    pub description: String,
    /// 文件路径
    pub file_path: String,
    /// 行号
    pub line: Option<u32>,
    /// 列号
    pub character: Option<u32>,
    /// 受影响的代码
    pub affected_code: Option<String>,
    /// 建议修复
    pub recommendation: String,
    /// 参考链接
    pub reference_url: Option<String>,
}

/// 性能问题
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerformanceIssue {
    /// 严重级别
    pub severity: SeverityLevel,
    /// 类别 (Memory/CPU/I/O/Algorithm)
    pub perf_category: PerfCategory,
    /// 标题
    pub title: String,
    /// 描述
    pub description: String,
    /// 文件路径
    pub file_path: String,
    /// 行号
    pub line: Option<u32>,
    /// 当前实现
    pub current_implementation: Option<String>,
    /// 建议优化
    pub suggested_optimization: String,
    /// 预期改进 (百分比)
    pub expected_improvement_percent: Option<f64>,
}

/// 严重级别
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum SeverityLevel {
    Critical,
    High,
    Medium,
    Low,
    Info,
}

impl std::fmt::Display for SeverityLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SeverityLevel::Critical => write!(f, "🔴 CRITICAL"),
            SeverityLevel::High => write!(f, "🟠 HIGH"),
            SeverityLevel::Medium => write!(f, "🟡 MEDIUM"),
            SeverityLevel::Low => write!(f, "🟢 LOW"),
            SeverityLevel::Info => write!(f, "ℹ️ INFO"),
        }
    }
}

/// 性能类别
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PerfCategory {
    Memory,
    CPU,
    IO,
    Algorithm,
    Concurrency,
}

impl std::fmt::Display for PerfCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PerfCategory::Memory => write!(f, "💾 Memory"),
            PerfCategory::CPU => write!(f, "⚡ CPU"),
            PerfCategory::IO => write!(f, "💽 I/O"),
            PerfCategory::Algorithm => write!(f, "🧮 Algorithm"),
            PerfCategory::Concurrency => write!(f, "🔀 Concurrency"),
        }
    }
}

/// 代码风格问题
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StyleIssue {
    /// 规则名称
    pub rule_name: String,
    /// 描述
    pub description: String,
    /// 文件路径
    pub file_path: String,
    /// 行号
    pub line: Option<u32>,
    /// 当前代码
    pub current_code: Option<String>,
    /// 建议修改
    pub suggested_change: Option<String>,
}

/// 最佳实践建议
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BestPracticeSuggestion {
    /// 类别
    pub category: PracticeCategory,
    /// 标题
    pub title: String,
    /// 描述
    pub description: String,
    /// 当前实现
    pub current_implementation: Option<String>,
    /// 推荐做法
    pub recommended_approach: String,
    /// 参考文档
    pub reference: Option<String>,
}

/// 实践类别
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PracticeCategory {
    ErrorHandling,
    ResourceManagement,
    API Design,
    Testing,
    Documentation,
    Maintainability,
}

/// 审查摘要
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewSummary {
    /// 总体评分 (0-100)
    pub score: u8,
    /// 安全评分 (0-100)
    pub security_score: u8,
    /// 性能评分 (0-100)
    pub performance_score: u8,
    /// 风格评分 (0-100)
    pub style_score: u8,
    /// 问题统计
    pub issue_counts: IssueCounts,
}

/// 问题计数
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct IssueCounts {
    pub critical_security: usize,
    pub high_security: usize,
    pub medium_security: usize,
    pub low_security: usize,
    pub critical_perf: usize,
    pub high_perf: usize,
    pub medium_perf: usize,
    pub style_violations: usize,
}

/// 代码审查引擎
pub struct CodeReviewEngine {
    config: ReviewConfig,
    security_rules: Arc<RwLock<Vec<SecurityRule>>>,
    performance_rules: Arc<RwLock<Vec<PerformanceRule>>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewConfig {
    /// 启用安全审查
    pub enable_security_review: bool,
    /// 启用性能审查
    pub enable_performance_review: bool,
    /// 启用风格检查
    pub enable_style_check: bool,
    /// 安全阈值 (低于此分数视为不通过)
    pub security_threshold: u8,
    /// 性能阈值
    pub performance_threshold: u8,
    /// 审查的语言
    pub languages: Vec<String>,
}

impl Default for ReviewConfig {
    fn default() -> Self {
        Self {
            enable_security_review: true,
            enable_performance_review: true,
            enable_style_check: true,
            security_threshold: 70,
            performance_threshold: 70,
            languages: vec!["rust".to_string(), "python".to_string()],
        }
    }
}

/// 安全规则
struct SecurityRule {
    id: &'static str,
    name: &'static str,
    severity: SeverityLevel,
    category: &'static str,
    pattern: regex::Regex,
    description: &'static str,
    recommendation: &'static str,
}

/// 性能规则
struct PerformanceRule {
    id: &'static str,
    name: &'static str,
    severity: SeverityLevel,
    category: PerfCategory,
    pattern: regex::Regex,
    description: &'static str,
    optimization: &'static str,
    expected_improvement: Option<f64>,
}

impl CodeReviewEngine {
    /// 创建新的代码审查引擎
    pub fn new() -> Self {
        Self::with_config(ReviewConfig::default())
    }

    /// 使用配置创建
    pub fn with_config(config: ReviewConfig) -> Self {
        let mut engine = Self {
            config,
            security_rules: Arc::new(RwLock::new(Vec::new())),
            performance_rules: Arc::new(RwLock::new(Vec::new())),
        };
        
        engine.register_builtin_rules();
        engine
    }

    /// 注册内置规则
    fn register_builtin_rules(&mut self) {
        // 安全规则
        let sec_rules = vec![
            SecurityRule {
                id: "CWE-79",
                name: "Buffer Overflow",
                severity: SeverityLevel::Critical,
                category: "Memory Safety",
                pattern: regex::Regex::new(r"(unsafe\s*\{[^}]*\}|\bmemcpy\b|\bstrcpy\b)").unwrap(),
                description: "Potential buffer overflow detected",
                recommendation: "Use safe string handling functions or bounds checking",
            },
            SecurityRule {
                id: "CWE-89",
                name: "SQL Injection",
                severity: SeverityLevel::Critical,
                category: "Injection",
                pattern: regex::Regex::new(r"(format!\s*\(.*?\{.*?\}|execute_sql\s*\()").unwrap(),
                description: "Potential SQL injection vulnerability",
                recommendation: "Use parameterized queries or prepared statements",
            },
            SecurityRule {
                id: "CWE-20",
                name: "Improper Input Validation",
                severity: SeverityLevel::High,
                category: "Input Validation",
                pattern: regex::Regex::new(r"\.unwrap\(\)\s*[^(?!)]").unwrap(),
                description: "Unwrapping user input without validation",
                recommendation: "Validate input before unwrapping or use expect() with message",
            },
            SecurityRule {
                id: "CWE-295",
                name: "Improper Certificate Validation",
                severity: SeverityLevel::High,
                category: "Cryptographic Issues",
                pattern: regex::Regex::new(r"(tls_accept_invalid_certs|DANGEROUS_CERTIFICATE_ACCEPTED)").unwrap(),
                description: "Invalid certificate accepted",
                recommendation: "Implement proper certificate validation and pinning",
            },
        ];
        
        // 性能规则
        let perf_rules = vec![
            PerformanceRule {
                id: "PERF-001",
                name: "Inefficient Loop",
                severity: SeverityLevel::Medium,
                category: PerfCategory::CPU,
                pattern: regex::Regex::new(r"for\s+\w+\s+in\s+.+\.iter\(\)\s*\{").unwrap(),
                description: "Using iter() in a loop may be inefficient",
                optimization: "Consider using iterators directly or pre-computing values",
                expected_improvement: Some(15.0),
            },
            PerformanceRule {
                id: "PERF-002",
                name: "Unnecessary Clones",
                severity: SeverityLevel::Medium,
                category: PerfCategory::Memory,
                pattern: regex::Regex::new(r"\.clone\(\)\s*$").unwrap(),
                description: "Cloning large data structures unnecessarily",
                optimization: "Use references (&) instead of cloning when possible",
                expected_improvement: Some(30.0),
            },
            PerformanceRule {
                id: "PERF-003",
                name: "Blocking I/O in Async Context",
                severity: SeverityLevel::High,
                category: PerfCategory::IO,
                pattern: regex::Regex::new(r"(std::fs::read_to_string|tokio::fs::read)\.await").unwrap(),
                description: "Synchronous file read in async context blocks the executor",
                optimization: "Use tokio::fs::read or spawn_blocking_task for CPU-intensive operations",
                expected_improvement: Some(50.0),
            },
            PerformanceRule {
                id: "PERF-004",
                name: "O(n²) Algorithm",
                severity: SeverityLevel::Medium,
                category: PerfCategory::Algorithm,
                pattern: regex::Regex::new(r"#\[warn\(perf::slow\)\]").unwrap(),
                description: "Known slow algorithm detected",
                optimization: "Consider using HashMap, BTreeMap, or more efficient algorithm",
                expected_improvement: Some(40.0),
            },
        ];
        
        *self.security_rules.write().unwrap() = sec_rules;
        *self.performance_rules.write().unwrap() = perf_rules;
    }

    /// 执行完整的代码审查
    ///
    /// # Arguments
    /// * `code` - 要审查的代码
    /// * `file_path` - 文件路径
    /// * `language` - 编程语言
    ///
    /// # Returns
    /// 返回详细的审查结果
    pub async fn review_code(
        &self,
        code: &str,
        file_path: &str,
        language: &str,
    ) -> CodeReviewResult {
        info!("Starting code review for {}", file_path);
        
        let mut security_issues = Vec::new();
        let mut performance_issues = Vec::new();
        let mut style_issues = Vec::new();
        let mut best_practices = Vec::new();
        
        // 1. 安全审查
        if self.config.enable_security_review {
            security_issues = self.perform_security_review(code, file_path, language).await;
        }
        
        // 2. 性能审查
        if self.config.enable_performance_review {
            performance_issues = self.perform_performance_review(code, file_path, language).await;
        }
        
        // 3. 风格检查
        if self.config.enable_style_check {
            style_issues = self.perform_style_check(code, file_path, language).await;
        }
        
        // 4. 最佳实践检查
        best_practices = self.check_best_practices(code, file_path, language).await;
        
        // 计算总体评分
        let summary = self.calculate_summary(
            &security_issues,
            &performance_issues,
            &style_issues,
        );
        
        CodeReviewResult {
            file_path: file_path.to_string(),
            overall_score: summary.score,
            security_issues,
            performance_issues,
            style_issues,
            best_practices,
            summary,
        }
    }

    /// 执行安全审查
    async fn perform_security_review(
        &self,
        code: &str,
        file_path: &str,
        _language: &str,
    ) -> Vec<SecurityIssue> {
        let rules = self.security_rules.read().await;
        let mut issues = Vec::new();
        
        for rule in rules.iter() {
            if let Some(caps) = rule.pattern.find(code) {
                let start = caps.start();
                let end = caps.end();
                let affected_code = code[start..end.min(start + 80)].to_string();
                
                // 计算行号
                let line = code[..start].matches('\n').count() as u32 + 1;
                
                issues.push(SecurityIssue {
                    severity: rule.severity,
                    rule_id: rule.id.to_string(),
                    title: format!("{}: {}", rule.name, rule.description),
                    description: rule.description.to_string(),
                    file_path: file_path.to_string(),
                    line: Some(line),
                    character: None,
                    affected_code: Some(affected_code),
                    recommendation: rule.recommendation.to_string(),
                    reference_url: Some(format!("https://cwe.mitre.org/data/definitions/{}", rule.id)),
                });
            }
        }
        
        issues
    }

    /// 执行性能审查
    async fn perform_performance_review(
        &self,
        code: &str,
        file_path: &str,
        _language: &str,
    ) -> Vec<PerformanceIssue> {
        let rules = self.performance_rules.read().await;
        let mut issues = Vec::new();
        
        for rule in rules.iter() {
            if let Some(caps) = rule.pattern.find(code) {
                let start = caps.start();
                let end = caps.end();
                let affected_code = code[start..end.min(start + 80)].to_string();
                
                let line = code[..start].matches('\n').count() as u32 + 1;
                
                issues.push(PerformanceIssue {
                    severity: rule.severity,
                    perf_category: rule.category,
                    title: format!("{}: {}", rule.name, rule.description),
                    description: rule.description.to_string(),
                    file_path: file_path.to_string(),
                    line: Some(line),
                    current_implementation: Some(affected_code),
                    suggested_optimization: rule.optimization.to_string(),
                    expected_improvement: rule.expected_improvement,
                });
            }
        }
        
        issues
    }

    /// 执行风格检查
    async fn perform_style_check(
        &self,
        code: &str,
        file_path: &str,
        _language: &str,
    ) -> Vec<StyleIssue> {
        let mut issues = Vec::new();
        
        // 检查行长度
        for (idx, line) in code.lines().enumerate() {
            if line.len() > 100 {
                issues.push(StyleIssue {
                    rule_name: "line_length".to_string(),
                    description: format!("Line too long ({} chars > 100)", line.len()),
                    file_path: file_path.to_string(),
                    line: Some((idx + 1) as u32),
                    current_code: Some(line.to_string()),
                    suggested_change: Some("Break into multiple lines".to_string()),
                });
            }
        }
        
        // 检查尾随空格
        for (idx, line) in code.lines().enumerate() {
            if line.ends_with(' ') && !line.trim().is_empty() {
                issues.push(StyleIssue {
                    rule_name: "trailing_whitespace".to_string(),
                    description: "Trailing whitespace detected".to_string(),
                    file_path: file_path.to_string(),
                    line: Some((idx + 1) as u32),
                    current_code: Some(line.to_string()),
                    suggested_change: Some(line.trim_end().to_string()),
                });
            }
        }
        
        // 检查缺少空行（函数之间应该有空行）
        let lines: Vec<&str> = code.lines().collect();
        for idx in 1..lines.len().saturating_sub(1) {
            let prev_line = lines[idx - 1];
            let curr_line = lines[idx];
            
            if prev_line.ends_with('}') && !curr_line.is_empty() && !curr_line.starts_with("}") {
                issues.push(StyleIssue {
                    rule_name: "blank_line_after_function".to_string(),
                    description: "Missing blank line after function/block end".to_string(),
                    file_path: file_path.to_string(),
                    line: Some((idx + 1) as u32),
                    current_code: Some(curr_line.to_string()),
                    suggested_change: Some(format!("\n{}", curr_line)),
                });
            }
        }
        
        issues
    }

    /// 检查最佳实践
    async fn check_best_practices(
        &self,
        code: &str,
        file_path: &str,
        language: &str,
    ) -> Vec<BestPracticeSuggestion> {
        let mut suggestions = Vec::new();
        
        // Rust 特定检查
        if language == "rust" {
            // 检查是否使用了 unwrap()
            if code.contains(".unwrap()") || code.contains(".expect(\"") == false {
                suggestions.push(BestPracticeSuggestion {
                    category: PracticeCategory::ErrorHandling,
                    title: "Prefer expect() over unwrap()".to_string(),
                    description: "Using unwrap() can cause panics; prefer expect() with descriptive messages".to_string(),
                    current_implementation: None,
                    recommended_approach: "Replace .unwrap() with .expect(\"Descriptive message\")".to_string(),
                    reference: Some("https://doc.rust-lang.org/book/ch09-error-handling.html".to_string()),
                });
            }
            
            // 检查是否有测试
            if !code.contains("#[cfg(test)]") && !code.contains("#[test]") {
                suggestions.push(BestPracticeSuggestion {
                    category: PracticeCategory::Testing,
                    title: "Add unit tests".to_string(),
                    description: "No tests found in this file".to_string(),
                    current_implementation: None,
                    recommended_approach: "Add #[cfg(test)] mod tests { ... } with unit and integration tests".to_string(),
                    reference: Some("https://doc.rust-lang.org/book/ch11-testing.html".to_string()),
                });
            }
            
            // 检查是否有文档注释
            if !code.contains("/// ") && !code.contains("//!") {
                suggestions.push(BestPracticeSuggestion {
                    category: PracticeCategory::Documentation,
                    title: "Add documentation comments".to_string(),
                    description: "Public items should have doc comments".to_string(),
                    current_implementation: None,
                    recommended_approach: "Add /// comments for public functions, structs, and enums".to_string(),
                    reference: Some("https://doc.rust-lang.org/book/ch14-doc-comments.html".to_string()),
                });
            }
        }
        
        // Python 特定检查
        if language == "python" {
            // 检查是否有 type hints
            if !code.contains(": ") && code.contains("def ") {
                suggestions.push(BestPracticeSuggestion {
                    category: PracticeCategory::API Design,
                    title: "Add type hints".to_string(),
                    description: "Functions should have type annotations for better IDE support".to_string(),
                    current_implementation: None,
                    recommended_approach: "Add type hints to function parameters and return values".to_string(),
                    reference: Some("https://peps.python.org/pep-0484/".to_string()),
                });
            }
            
            // 检查是否有 docstrings
            if !code.contains("\"\"\"") && code.contains("def ") {
                suggestions.push(BestPracticeSuggestion {
                    category: PracticeCategory::Documentation,
                    title: "Add docstrings".to_string(),
                    description: "Public functions should have docstrings".to_string(),
                    current_implementation: None,
                    recommended_approach: 'Add """docstring""" to all public functions'.to_string(),
                    reference: Some("https://www.python.org/dev/peps/pep-0257/".to_string()),
                });
            }
        }
        
        suggestions
    }

    /// 计算总体评分
    fn calculate_summary(
        &self,
        security_issues: &[SecurityIssue],
        performance_issues: &[PerformanceIssue],
        style_issues: &[StyleIssue],
    ) -> ReviewSummary {
        let issue_counts = IssueCounts {
            critical_security: security_issues.iter().filter(|i| i.severity == SeverityLevel::Critical).count(),
            high_security: security_issues.iter().filter(|i| i.severity == SeverityLevel::High).count(),
            medium_security: security_issues.iter().filter(|i| i.severity == SeverityLevel::Medium).count(),
            low_security: security_issues.iter().filter(|i| i.severity == SeverityLevel::Low).count(),
            critical_perf: performance_issues.iter().filter(|i| i.severity == SeverityLevel::Critical).count(),
            high_perf: performance_issues.iter().filter(|i| i.severity == SeverityLevel::High).count(),
            medium_perf: performance_issues.iter().filter(|i| i.severity == SeverityLevel::Medium).count(),
            style_violations: style_issues.len(),
        };
        
        // 计算各项评分 (基础 100 分，扣分制)
        let security_score = 100u8.saturating_sub(
            (issue_counts.critical_security * 25 +
             issue_counts.high_security * 10 +
             issue_counts.medium_security * 5 +
             issue_counts.low_security * 1) as u8
        );
        
        let performance_score = 100u8.saturating_sub(
            (issue_counts.critical_perf * 20 +
             issue_counts.high_perf * 10 +
             issue_counts.medium_perf * 5) as u8
        );
        
        let style_score = 100u8.saturating_sub(issue_counts.style_violations.min(20) * 5);
        
        // 总体评分 (加权平均)
        let overall_score = ((security_score as u32 * 35 +
                           performance_score as u32 * 35 +
                           style_score as u32 * 30) / 100) as u8;
        
        ReviewSummary {
            score: overall_score,
            security_score,
            performance_score,
            style_score,
            issue_counts,
        }
    }


// ════════════════════════════════════════════════════════════════
// 3. FormatCode Engine — 智能代码格式化
// ════════════════════════════════════════════════════════════════

/// 格式化结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FormatResult {
    /// 是否成功
    pub success: bool,
    /// 格式化后的代码
    pub formatted_code: String,
    /// 使用的工具
    pub tool_used: String,
    /// 统计信息
    pub stats: FormatStats,
}

/// 格式化统计
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FormatStats {
    /// 格式化的文件数
    pub files_formatted: usize,
    /// 总行数变化
    pub total_lines_changed: isize,
    /// 耗时（毫秒）
    pub duration_ms: u64,
}

/// 代码格式化引擎
pub struct FormatCodeEngine {
    config: FormatConfig,
    formatters: Arc<RwLock<HashMap<String, FormatterConfig>>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FormatConfig {
    /// 是否启用自动格式化
    pub auto_format_on_save: bool,
    /// 默认缩进宽度
    pub indent_width: usize,
    /// 使用 tab 还是 spaces
    pub use_tabs: bool,
    /// 行宽限制
    pub max_line_length: usize,
    /// 格式化工具路径
    pub formatter_paths: HashMap<String, String>, // language -> formatter binary path
}

impl Default for FormatConfig {
    fn default() -> Self {
        let mut paths = HashMap::new();
        paths.insert("rust".to_string(), "rustfmt".to_string());
        paths.insert("python".to_string(), "black".to_string());
        paths.insert("javascript".to_string(), "prettier".to_string());
        paths.insert("typescript".to_string(), "prettier".to_string());
        paths.insert("go".to_string(), "gofmt".to_string());
        paths.insert("java".to_string(), "google-java-format".to_string());
        paths.insert("cpp".to_string(), "clang-format".to_string());
        paths.insert("c".to_string(), "clang-format".to_string());
        
        Self {
            auto_format_on_save: true,
            indent_width: 4,
            use_tabs: false,
            max_line_length: 100,
            formatter_paths: paths,
        }
    }
}

/// 格式化器配置
struct FormatterConfig {
    command: String,
    args: Vec<String>,
    env: HashMap<String, String>,
}

impl FormatCodeEngine {
    /// 创建新的格式化引擎
    pub fn new() -> Self {
        Self::with_config(FormatConfig::default())
    }

    /// 使用配置创建
    pub fn with_config(config: FormatConfig) -> Self {
        let mut formatters = HashMap::new();
        
        // 配置各种语言的格式化器
        formatters.insert("rust".to_string(), FormatterConfig {
            command: config.formatter_paths.get("rust").cloned().unwrap_or_else(|| "rustfmt".to_string()),
            args: vec![],
            env: HashMap::new(),
        });
        
        formatters.insert("python".to_string(), FormatterConfig {
            command: config.formatter_paths.get("python").cloned().unwrap_or_else(|| "black".to_string()),
            args: vec![],
            env: HashMap::new(),
        });
        
        formatters.insert("javascript".to_string(), FormatterConfig {
            command: config.formatter_paths.get("javascript").cloned().unwrap_or_else(|| "prettier".to_string()),
            args: vec![],
            env: HashMap::new(),
        });
        
        formatters.insert("typescript".to_string(), FormatterConfig {
            command: config.formatter_paths.get("typescript").cloned().unwrap_or_else(|| "prettier".to_string()),
            args: vec![],
            env: HashMap::new(),
        });
        
        formatters.insert("go".to_string(), FormatterConfig {
            command: config.formatter_paths.get("go").cloned().unwrap_or_else(|| "gofmt".to_string()),
            args: vec!["-w".to_string()],
            env: HashMap::new(),
        });
        
        formatters.insert("java".to_string(), FormatterConfig {
            command: config.formatter_paths.get("java").cloned().unwrap_or_else(|| "google-java-format".to_string()),
            args: vec![],
            env: HashMap::new(),
        });
        
        formatters.insert("cpp".to_string(), FormatterConfig {
            command: config.formatter_paths.get("cpp").cloned().unwrap_or_else(|| "clang-format".to_string()),
            args: vec![],
            env: HashMap::new(),
        });
        
        formatters.insert("c".to_string(), FormatterConfig {
            command: config.formatter_paths.get("c").cloned().unwrap_or_else(|| "clang-format".to_string()),
            args: vec![],
            env: HashMap::new(),
        });
        
        Self {
            config,
            formatters: Arc::new(RwLock::new(formatters)),
        }
    }

    /// 格式化代码
    ///
    /// # Arguments
    /// * `code` - 原始代码
    /// * `file_path` - 文件路径
    /// * `language` - 编程语言
    ///
    /// # Returns
    /// 返回格式化结果，包含格式化后的代码和统计信息
    pub async fn format_code(
        &self,
        code: &str,
        file_path: &str,
        language: &str,
    ) -> FormatResult {
        info!("Formatting {} ({})", file_path, language);
        
        let start_time = std::time::Instant::now();
        
        // 获取该语言的格式化器
        let formatter = self.formatters.read().await.get(language).cloned();
        
        match formatter {
            Some(formatter_config) => {
                // 执行外部格式化命令
                match self.run_external_formatter(code, &formatter_config).await {
                    Ok(formatted_code) => {
                        let duration = start_time.elapsed().as_millis() as u64;
                        
                        // 计算行数变化
                        let lines_before = code.lines().count();
                        let lines_after = formatted_code.lines().count();
                        let lines_diff = lines_after as isize - lines_before as isize;
                        
                        FormatResult {
                            success: true,
                            formatted_code,
                            tool_used: formatter_config.command.clone(),
                            stats: FormatStats {
                                files_formatted: 1,
                                total_lines_changed: lines_diff,
                                duration_ms: duration,
                            },
                        }
                    }
                    Err(e) => {
                        warn!("External formatter failed: {}, falling back to basic formatting", e);
                        // 回退到基本格式化
                        self.basic_format(code, language)
                    }
                }
            }
            None => {
                warn!("No formatter configured for language: {}, using basic formatting", language);
                self.basic_format(code, language)
            }
        }
    }

    /// 运行外部格式化工具
    async fn run_external_formatter(
        &self,
        code: &str,
        formatter: &FormatterConfig,
    ) -> Result<String, Box<dyn std::error::Error>> {
        // 写入临时文件
        let temp_dir = std::env::temp_dir();
        let temp_file_path = temp_dir.join("jcode_fmt_temp");
        
        std::fs::write(&temp_file_path, code)?;
        
        // 执行格式化命令
        let output = tokio::process::Command::new(&formatter.command)
            .args(&formatter.args)
            .current_dir(&temp_dir)
            .output()
            .await?;
        
        // 读取格式化后的结果
        let formatted_code = std::fs::read_to_string(&temp_file_path)?;
        
        // 清理临时文件
        let _ = std::fs::remove_file(&temp_file_path);
        
        if output.status.success() {
            Ok(formatted_code)
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(format!("Formatter failed:\n{}", stderr).into())
        }
    }

    /// 基本格式化（回退方案）
    fn basic_format(&self, code: &str, language: &str) -> FormatResult {
        let start_time = std::time::Instant::now();
        let mut formatted_lines: Vec<String> = Vec::new();
        
        // 简单的格式化：统一缩进、去除多余空行等
        let mut blank_line_count = 0;
        
        for line in code.lines() {
            let trimmed = line.trim();
            
            if trimmed.is_empty() {
                blank_line_count += 1;
                if blank_line_count <= 1 {
                    formatted_lines.push(String::new());
                }
                continue;
            }
            
            blank_line_count = 0;
            
            // 处理缩进
            let indented = if self.config.use_tabs {
                let depth = (line.len() - line.trim_start().len()) / self.config.indent_width;
                "\t".repeat(depth) + trimmed
            } else {
                let depth = (line.len() - line.trim_start().len()) / self.config.indent_width;
                " ".repeat(depth * self.config.indent_width) + trimmed
            };
            
            formatted_lines.push(indented);
        }
        
        let formatted_code = formatted_lines.join("\n");
        let duration = start_time.elapsed().as_millis() as u64;
        
        let lines_before = code.lines().count();
        let lines_after = formatted_code.lines().count();
        
        FormatResult {
            success: true,
            formatted_code,
            tool_used: format!("basic_formatter_{}", language),
            stats: FormatStats {
                files_formatted: 1,
                total_lines_changed: (lines_after - lines_before) as isize,
                duration_ms: duration,
            },
        }
    }

    /// 批量格式化多个文件
    pub async fn format_files(
        &self,
        files: &[(String, String)], // (file_path, content, language)
    ) -> Vec<(String, FormatResult)> {
        let mut results = Vec::new();
        
        for (file_path, code, language) in files {
            let result = self.format_code(code, file_path, language).await;
            results.push((file_path.clone(), result));
        }
        
        results
    }
}
