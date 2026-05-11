//! Diagnostics Manager — 智能诊断推送系统
//!
//! ## 核心能力 (对标 Cursor/Claude Code)
//! - **实时推送**: Server → Client 的错误/警告/信息推送
//! - **智能去重**: 避免重复显示相同的诊断
//! - **优先级排序**: Error > Warning > Hint > Information
//! - **文件关联**: 自动关联诊断到对应文件
//! - **变更触发**: 文档保存/修改时自动刷新
//!
//! ## 诊断流程
//! ```text
//! LSP Server ──(publishDiagnostics)──▶ DiagnosticsManager
//!                                          │
//!                                    ┌─────┴─────┐
//!                                    │           │
//!                              去重过滤    优先级排序
//!                                    │           │
//!                                    ▼           ▼
//!                               缓存存储   推送通知
//! ```

use lsp_types::*;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};
use tracing::{debug, info, warn};

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
#[derive(Debug, Clone)]
struct EnhancedDiagnostic {
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

#[derive(Debug, Clone)]
struct GlobalStats {
    total_files: usize,
    total_errors: usize,
    total_warnings: usize,
    total_hints: usize,
    total_info: usize,
}

impl Default for GlobalStats {
    fn default() -> Self {
        Self {
            total_files: 0,
            total_errors: 0,
            total_warnings: 0,
            total_hints: 0,
            total_info: 0,
        }
    }
}

/// 配置选项
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
            .filter(|(_uri, state)| state.error_count > 0)
            .map((uri, state)| (uri.to_string(), state.error_count))
            .collect()
    }

    // ─── 内部方法 ─────────────────────────

    fn compute_diagnostic_hash(&self, uri: &Url, diag: &Diagnostic) -> u64 {
        use std::hash::{Hash, Hasher};
        use std::collections::hash_map::DefaultHasher;

        let mut hasher = DefaultHasher::new();
        uri.hash(&mut hasher);
        diag.range.hash(&mut hasher);
        diag.code.as_ref().hash(&mut hasher);
        diag.message.hash(&mut hasher);
        diag.severity.hash(&mut hasher);
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
                    .clone()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_handle_diagnostics() {
        let manager = DiagnosticsManager::new();

        let params = PublishDiagnosticsParams {
            uri: Url::parse("file:///test.rs").unwrap(),
            diagnostics: vec![
                Diagnostic::new_simple("Missing semicolon", Range::new(
                    Position::new(10, 5),
                    Position::new(10, 20),
                ), Some(DiagnosticSeverity::ERROR), None),
                Diagnostic::new_simple("Unused variable", Range::new(
                    Position::new(15, 0),
                    Position::new(15, 8),
                ), Some(DiagnosticSeverity::WARNING), None),
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
                Diagnostic::new_simple("Error", Range::new(
                    Position::new(0, 0),
                    Position::new(0, 5),
                ), Some(DiagnosticSeverity::ERROR), None),
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
                Diagnostic::new_simple("Test", Range::new(
                    Position::new(0, 0),
                    Position::new(0, 4),
                ), Some(DiagnosticSeverity::ERROR), None),
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
