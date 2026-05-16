//! Diagnostics Manager — 智能诊断推送 + QuickFix 自动修复系统
//!
//! ## 核心能力 (对标 Cursor/Claude Code)
//! - **实时推送**: Server -> Client 的错误/警告/信息推送
//! - **智能去重**: 避免重复显示相同的诊断
//! - **优先级排序**: Error > Warning > Hint > Information
//! - **文件关联**: 自动关联诊断到对应文件
//! - **变更触发**: 文档保存/修改时自动刷新
//! - **QuickFix**: 自动修复编译错误和 lint 警告
//!
//! ## 诊断流程
//! ```text
//! LSP Server --(publishDiagnostics)--▶ DiagnosticsManager
//!                                          |
//!                                    +-----+-----+
//!                                    |           |
//!                              去重过滤    优先级排序
//!                                    |           |
//!                                    ▼           ▼
//!                               缓存存储   推送通知
//!                                          |
//!                                    +-----▼-----+
//!                                    |  QuickFix  | <- 自动修复引擎
//!                                    |  Engine    |
//!                                    +-----------+
//! ```

use lsp_types::*;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};
use tracing::{debug, warn};

/// 诊断严重级别（用于排序）
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DiagnosticSeverity {
    Error = 1,
    Warning = 2,
    Information = 3,
    Hint = 4,
}

impl From<lsp_types::DiagnosticSeverity> for DiagnosticSeverity {
    fn from(severity: lsp_types::DiagnosticSeverity) -> Self {
        match severity {
            s if s == lsp_types::DiagnosticSeverity::ERROR => Self::Error,
            s if s == lsp_types::DiagnosticSeverity::WARNING => Self::Warning,
            s if s == lsp_types::DiagnosticSeverity::INFORMATION => Self::Information,
            s if s == lsp_types::DiagnosticSeverity::HINT => Self::Hint,
            _ => Self::Error, // 默认作为 Error 处理
        }
    }
}

/// 增强的诊断信息
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct EnhancedDiagnostic {
    /// 原始诊断
    diagnostic: Diagnostic,
    
    /// 来源文件 URI
    uri: Url,
    
    /// 接收时间戳
    received_at: std::time::Instant,
    
    /// 是否已读
    is_read: bool,
    
    /// 关联的代码操作建议
    quick_fixes: Vec<CodeAction>,
}

/// 文件诊断状态
struct FileDiagnosticsState {
    /// 当前活跃的诊断列表
    diagnostics: Vec<EnhancedDiagnostic>,
    
    /// 诊断哈希集合（用于快速去重）
    diagnostics_hash: HashSet<u64>,
    
    /// 错误计数
    error_count: usize,
    
    /// 警告计数
    warning_count: usize,
    
    /// 最后更新时间
    last_updated: std::time::Instant,
    
    /// 版本号（每次变更 +1）
    version: u64,
}

impl Default for FileDiagnosticsState {
    fn default() -> Self {
        Self {
            diagnostics: vec![],
            diagnostics_hash: HashSet::new(),
            error_count: 0,
            warning_count: 0,
            last_updated: std::time::Instant::now(),
            version: 0,
        }
    }
}

/// 诊断事件类型
#[derive(Debug, Clone)]
pub enum DiagnosticEvent {
    /// 新诊断到达
    DiagnosticsReceived {
        uri: String,
        diagnostics: Vec<EnhancedDiagnostic>,
    },
    
    /// 诊断被清除
    DiagnosticsCleared {
        uri: String,
    },
    
    /// 诊断统计更新
    StatsUpdated {
        total_errors: usize,
        total_warnings: usize,
    },
}

/// 诊断管理器
pub struct DiagnosticsManager {
    /// 每个文件的诊断状态
    file_states: Arc<RwLock<HashMap<Url, FileDiagnosticsState>>>,
    
    /// 事件广播通道 (用于实时推送)
    event_sender: broadcast::Sender<DiagnosticEvent>,
    
    /// 全局统计
    global_stats: Arc<RwLock<GlobalStats>>,
    
    /// 配置选项
    config: DiagnosticsConfig,
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
#[derive(Default)]
struct GlobalStats {
    total_files: usize,
    total_errors: usize,
    total_warnings: usize,
    total_hints: usize,
    total_info: usize,
}


/// 配置选项
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct DiagnosticsConfig {
    /// 最大缓存文件数
    max_cached_files: usize,
    
    /// 诊断过期时间 (None = 不过期)
    ttl: Option<std::time::Duration>,
    
    /// 是否启用广播
    enable_broadcast: bool,
    
    /// 广播通道容量
    broadcast_capacity: usize,
}

impl Default for DiagnosticsConfig {
    fn default() -> Self {
        Self {
            max_cached_files: 100,
            ttl: Some(std::time::Duration::from_secs(300)), // 5 分钟
            enable_broadcast: true,
            broadcast_capacity: 64,
        }
    }
}

impl Default for DiagnosticsManager {
    fn default() -> Self {
        Self::new()
    }
}

impl DiagnosticsManager {
    pub fn new() -> Self {
        Self::with_config(DiagnosticsConfig::default())
    }

    pub fn with_config(config: DiagnosticsConfig) -> Self {
        let (event_sender, _) = broadcast::channel(config.broadcast_capacity);

        Self {
            file_states: Arc::new(RwLock::new(HashMap::new())),
            event_sender,
            global_stats: Arc::new(RwLock::new(GlobalStats::default())),
            config,
        }
    }

    /// 订阅诊断事件
    pub async fn subscribe(&self) -> broadcast::Receiver<DiagnosticEvent> {
        self.event_sender.subscribe()
    }

    /// 处理来自 LSP Server 的 publishDiagnostics 通知
    pub async fn handle_publish_diagnostics(
        &self,
        params: &PublishDiagnosticsParams,
    ) -> Vec<EnhancedDiagnostic> {
        let uri = &params.uri;
        let diagnostics = &params.diagnostics;

        debug!(
            uri = %uri,
            count = diagnostics.len(),
            "Received diagnostics"
        );

        let mut states = self.file_states.write().await;
        
        // 获取或创建文件状态
        let state = states.entry(uri.clone())
            .or_insert_with(FileDiagnosticsState::default);
        
        // 清除旧诊断
        state.diagnostics.clear();
        state.diagnostics_hash.clear();
        state.error_count = 0;
        state.warning_count = 0;

        // 处理新诊断（带去重和增强）
        let mut enhanced_diagnostics = vec![];
        
        for diag in diagnostics {
            let hash = self.compute_diagnostic_hash(uri, diag);
            
            if !state.diagnostics_hash.contains(&hash) {
                state.diagnostics_hash.insert(hash);
                
                let severity = diag.severity
                    .map(DiagnosticSeverity::from)
                    .unwrap_or(DiagnosticSeverity::Error);

                match severity {
                    DiagnosticSeverity::Error => state.error_count += 1,
                    DiagnosticSeverity::Warning => state.warning_count += 1,
                    _ => {}
                }

                let enhanced = EnhancedDiagnostic {
                    diagnostic: diag.clone(),
                    uri: uri.clone(),
                    received_at: std::time::Instant::now(),
                    is_read: false,
                    quick_fixes: vec![],
                };

                enhanced_diagnostics.push(enhanced.clone());
                state.diagnostics.push(enhanced);
            }
        }

        // 更新版本和时间戳
        state.version += 1;
        state.last_updated = std::time::Instant::now();

        drop(states); // 释放写锁

        // 更新全局统计
        self.update_global_stats().await;

        // 发送事件通知
        if self.config.enable_broadcast && !enhanced_diagnostics.is_empty() {
            let _ = self.event_sender.send(DiagnosticEvent::DiagnosticsReceived {
                uri: uri.to_string(),
                diagnostics: enhanced_diagnostics.clone(),
            });
        } else if diagnostics.is_empty() {
            let _ = self.event_sender.send(DiagnosticEvent::DiagnosticsCleared {
                uri: uri.to_string(),
            });
        }

        enhanced_diagnostics
    }

    /// 获取指定文件的诊断
    pub async fn get_file_diagnostics(&self, uri: &str) -> Vec<EnhancedDiagnostic> {
        if let Ok(url) = Url::parse(uri) {
            let states = self.file_states.read().await;
            states.get(&url)
                .map(|s| s.diagnostics.clone())
                .unwrap_or_default()
        } else {
            vec![]
        }
    }

    /// 获取文件诊断摘要
    pub async fn get_file_summary(&self, uri: &str) -> Option<FileDiagnosticSummary> {
        let url = Url::parse(uri).ok()?;
        let states = self.file_states.read().await;
        states.get(&url).map(|s| FileDiagnosticSummary {
            errors: s.error_count,
            warnings: s.warning_count,
            version: s.version,
            last_updated: s.last_updated,
        })
    }

    /// 获取全局诊断统计
    pub async fn get_global_stats(&self) -> GlobalStats {
        self.global_stats.read().await.clone()
    }

    /// 清除指定文件的诊断
    pub async fn clear_file_diagnostics(&self, uri: &str) {
        let url = Url::parse(uri).unwrap();
        let mut states = self.file_states.write().await;
        if let Some(state) = states.get_mut(&url) {
            state.diagnostics.clear();
            state.diagnostics_hash.clear();
            state.error_count = 0;
            state.warning_count = 0;
            state.version += 1;
        }
        drop(states);

        let _ = self.event_sender.send(DiagnosticEvent::DiagnosticsCleared {
            uri: uri.to_string(),
        });

        self.update_global_stats().await;
    }

    /// 清理过期的诊断缓存
    pub async fn cleanup_expired(&self) -> usize {
        if let Some(ttl) = self.config.ttl {
            let mut states = self.file_states.write().await;
            let before = states.len();

            states.retain(|_uri, state| {
                state.last_updated.elapsed() < ttl
            });

            let removed = before - states.len();
            if removed > 0 {
                debug!(removed, "Cleaned up expired diagnostic caches");
            }

            removed
        } else {
            0
        }
    }

    /// 获取所有有错误的文件列表
    pub async fn get_files_with_errors(&self) -> Vec<(String, usize)> {
        let states = self.file_states.read().await;
        states.iter()
            .filter(|(_uri_ref, state_ref)| state_ref.error_count > 0)
            .map(|(uri_ref, state_ref)| (uri_ref.to_string(), state_ref.error_count))
            .collect()
    }

    // --- 内部方法 -------------------------

    fn compute_diagnostic_hash(&self, uri: &Url, diag: &Diagnostic) -> u64 {
        use std::hash::{Hash, Hasher};
        use std::collections::hash_map::DefaultHasher;

        let mut hasher = DefaultHasher::new();
        uri.hash(&mut hasher);
        diag.range.hash(&mut hasher);
        diag.code.as_ref().hash(&mut hasher);
        diag.message.hash(&mut hasher);
        
        // 手动计算 severity 的 hash（因为 DiagnosticSeverity 没有实现 Hash）
        if let Some(severity) = &diag.severity {
            // 使用 match 来获取内部值（避免访问私有字段）
            let severity_value = match *severity {
                lsp_types::DiagnosticSeverity::ERROR => 1,
                lsp_types::DiagnosticSeverity::WARNING => 2,
                lsp_types::DiagnosticSeverity::INFORMATION => 3,
                lsp_types::DiagnosticSeverity::HINT => 4,
                _ => 0, // 其他值
            };
            (severity_value as i64).hash(&mut hasher);
        } else {
            (-1i64).hash(&mut hasher); // None 的情况
        }
        
        hasher.finish()
    }

    async fn update_global_stats(&self) {
        let states = self.file_states.read().await;
        let mut stats = self.global_stats.write().await;

        stats.total_files = states.len();
        stats.total_errors = 0;
        stats.total_warnings = 0;
        stats.total_hints = 0;
        stats.total_info = 0;

        for (_uri, state) in states.iter() {
            stats.total_errors += state.error_count;
            stats.total_warnings += state.warning_count;
            
            for diag in &state.diagnostics {
                match diag.diagnostic.severity
                    .map(DiagnosticSeverity::from)
                    .unwrap_or(DiagnosticSeverity::Error)
                {
                    DiagnosticSeverity::Hint => stats.total_hints += 1,
                    DiagnosticSeverity::Information => stats.total_info += 1,
                    _ => {}
                }
            }
        }

        if self.config.enable_broadcast {
            let _ = self.event_sender.send(DiagnosticEvent::StatsUpdated {
                total_errors: stats.total_errors,
                total_warnings: stats.total_warnings,
            });
        }
    }
}

/// 文件诊断摘要
#[derive(Debug, Clone)]
pub struct FileDiagnosticSummary {
    pub errors: usize,
    pub warnings: usize,
    pub version: u64,
    pub last_updated: std::time::Instant,
}

// ════════════════════════════════════════════════════════════════
// QuickFix Engine — 自动修复编译错误和 lint 警告
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
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

/// QuickFix 引擎配置
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

/// 修复模式（内部使用）
struct FixPattern {
    category: FixCategory,
    pattern: regex::Regex,
    fix_template: String,
    confidence: f64,
    description: String,
}

/// QuickFix 引擎 — 与 DiagnosticsManager 紧密集成
pub struct QuickFixEngine {
    config: QuickFixConfig,
    fix_patterns: Arc<RwLock<Vec<FixPattern>>>,
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
        
        engine.register_builtin_patterns();
        engine
    }

    /// 注册内置的修复模式
    async fn register_builtin_patterns(&mut self) {
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
                fix_template: "".to_string(),
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
        
        *self.fix_patterns.write().await = patterns;
    }

    /// 分析并生成修复建议
    pub async fn analyze_and_suggest(
        &self,
        error_output: &str,
        file_path: &str,
        _language: &str,
    ) -> QuickFixResult {
        debug!("Analyzing errors for quick fix suggestions...");
        
        let mut fixes = Vec::new();
        let patterns = self.fix_patterns.read().await;
        
        for pattern in patterns.iter() {
            if let Some(caps) = pattern.pattern.captures(error_output) {
                let var_name = caps.get(1)
                    .map(|m| m.as_str().to_string())
                    .unwrap_or_default();
                
                let fixed_code = pattern.fix_template
                    .replace("${var}", &var_name);
                
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
            b.confidence
                .partial_cmp(&a.confidence)
                .unwrap_or(std::cmp::Ordering::Equal)
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
        
        if let Some(line_num) = fix.line {
            let lines: Vec<&str> = content.lines().collect();
            
            if line_num > 0 && (line_num as usize) <= lines.len() {
                let target_line_idx = (line_num - 1) as usize;
                let original_line = lines[target_line_idx];
                
                let new_line = match fix.fix_type {
                    FixCategory::CompilationError => {
                        if !fix.fixed_code.is_empty() {
                            fix.fixed_code.clone()
                        } else {
                            original_line.to_string()
                        }
                    }
                    FixCategory::LintWarning => original_line.to_string(),
                    _ => original_line.to_string(),
                };
                
                let mut result = lines[..target_line_idx].join("\n");
                result.push('\n');
                result.push_str(&new_line);
                result.push('\n');
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
        let line_re = regex::Regex::new(r"-->\s*.+?:(\d+):\d+").unwrap();
        
        line_re.captures(output)
            .and_then(|caps| caps.get(1))
            .and_then(|m| m.as_str().parse::<u32>().ok())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_handle_diagnostics() {
        let manager = DiagnosticsManager::new();

        let params = PublishDiagnosticsParams {
            uri: Url::parse("file:///test.rs").unwrap(),
            diagnostics: vec![
                lsp_types::Diagnostic {
                    range: Range::new(
                        Position::new(10, 5),
                        Position::new(10, 20),
                    ),
                    severity: Some(lsp_types::DiagnosticSeverity::ERROR),
                    message: "Missing semicolon".to_string(),
                    ..Default::default()
                },
                lsp_types::Diagnostic {
                    range: Range::new(
                        Position::new(15, 0),
                        Position::new(15, 8),
                    ),
                    severity: Some(lsp_types::DiagnosticSeverity::WARNING),
                    message: "Unused variable".to_string(),
                    ..Default::default()
                },
            ],
            version: None,
        };

        let result = manager.handle_publish_diagnostics(&params).await;
        assert_eq!(result.len(), 2);

        let summary = manager.get_file_summary("file:///test.rs").await;
        assert!(summary.is_some());
        let summary = summary.unwrap();
        assert_eq!(summary.errors, 1);
        assert_eq!(summary.warnings, 1);
    }

    #[tokio::test]
    async fn test_diagnostic_deduplication() {
        let manager = DiagnosticsManager::new();

        let params = PublishDiagnosticsParams {
            uri: Url::parse("file:///test.rs").unwrap(),
            diagnostics: vec![
                lsp_types::Diagnostic {
                    range: Range::new(
                        Position::new(0, 0),
                        Position::new(0, 5),
                    ),
                    severity: Some(lsp_types::DiagnosticSeverity::ERROR),
                    message: "Error".to_string(),
                    ..Default::default()
                },
            ],
            version: None,
        };

        // 第一次接收
        let result1 = manager.handle_publish_diagnostics(&params).await;
        assert_eq!(result1.len(), 1);

        // 第二次接收相同诊断（应该被去重）
        let result2 = manager.handle_publish_diagnostics(&params).await;
        assert_eq!(result2.len(), 1); // 返回的是缓存的

        // 验证总数
        let stats = manager.get_global_stats().await;
        assert_eq!(stats.total_errors, 1); // 不是 2
    }

    #[tokio::test]
    async fn test_subscribe_to_events() {
        let manager = DiagnosticsManager::new();
        let mut receiver = manager.subscribe().await;

        let params = PublishDiagnosticsParams {
            uri: Url::parse("file:///test.rs").unwrap(),
            diagnostics: vec![
                lsp_types::Diagnostic {
                    range: Range::new(
                        Position::new(0, 0),
                        Position::new(0, 4),
                    ),
                    severity: Some(lsp_types::DiagnosticSeverity::ERROR),
                    message: "Test".to_string(),
                    ..Default::default()
                },
            ],
            version: None,
        };

        manager.handle_publish_diagnostics(&params).await;

        // 应该收到事件
        let event = receiver.recv().await.unwrap();
        match event {
            DiagnosticEvent::DiagnosticsReceived { diagnostics, .. } => {
                assert_eq!(diagnostics.len(), 1);
            }
            other => panic!("Unexpected event: {:?}", other),
        }
    }
}
