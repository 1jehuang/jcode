//! # 工具结果预算管理
//!
//! 源自 Claude Code 的 `toolResultStorage.ts`，提供更智能的工具结果上下文预算管理：
//!
//! - **全局聚合预算**：按消息聚合管理工具结果，替换超过预算的最大结果 -> 文件路径
//! - **结果持久化**：大型结果写入磁盘，注入文件路径引用而非全文
//! - **ContentReplacementState**：用于提示缓存的稳定状态
//! - **空结果处理**：注入占位文本防止模型停止序列匹配

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

/// 单个工具结果的预算条目
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResultEntry {
    /// 工具调用 ID
    pub tool_use_id: String,
    /// 工具名称
    pub tool_name: String,
    /// 结果大小（字符数）
    pub size_chars: usize,
    /// 结果是否已被持久化到磁盘
    pub persisted: bool,
    /// 持久化后的文件路径（persisted=true 时有效）
    pub persisted_path: Option<String>,
}

/// 内容替换状态 — 用于提示缓存稳定性
/// 源自 Claude Code 的 `ContentReplacementState`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContentReplacementState {
    /// 已替换的 tool_use_id -> 替换信息
    pub replaced: HashMap<String, ReplacedResult>,
    /// 创建时间戳
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// 已替换的结果信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplacedResult {
    /// 原始大小（字符数）
    pub original_size_chars: usize,
    /// 替换后的文件路径
    pub file_path: String,
    /// 替换原因
    pub reason: ReplacementReason,
}

/// 替换原因
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ReplacementReason {
    /// 超过单条大小限制
    ExceededSizeLimit,
    /// 聚合预算不足
    BudgetExceeded,
    /// 主动持久化（工具标记为可持久化）
    ExplicitPersist,
}

/// 工具结果预算配置
#[derive(Debug, Clone)]
pub struct ToolResultBudgetConfig {
    /// 单条工具结果的最大字符数（超过则持久化到磁盘）
    pub max_result_size_chars: usize,
    /// 全局聚合预算字符数
    pub aggregate_budget_chars: usize,
    /// 持久化目录（如果未设置则不持久化）
    pub persist_dir: Option<PathBuf>,
    /// 启用空结果注入
    pub inject_empty_result: bool,
}

impl Default for ToolResultBudgetConfig {
    fn default() -> Self {
        Self {
            max_result_size_chars: 20_000,
            aggregate_budget_chars: 100_000,
            persist_dir: None,
            inject_empty_result: true,
        }
    }
}

/// 工具结果预算管理器
/// 源自 Claude Code 的 `enforceToolResultBudget()` + `persistToolResult()`
pub struct ToolResultBudgetManager {
    config: ToolResultBudgetConfig,
    /// 当前消息的工具结果条目（累积）
    entries: Arc<RwLock<Vec<ToolResultEntry>>>,
    /// 内容替换状态
    replacement_state: Arc<RwLock<ContentReplacementState>>,
}

impl ToolResultBudgetManager {
    pub fn new(config: ToolResultBudgetConfig) -> Self {
        Self {
            entries: Arc::new(RwLock::new(Vec::new())),
            replacement_state: Arc::new(RwLock::new(ContentReplacementState {
                replaced: HashMap::new(),
                created_at: chrono::Utc::now(),
            })),
            config,
        }
    }

    /// 检查工具结果是否超出预算，并返回（可能被截断的）结果
    /// 源自 Claude Code 的 `enforceToolResultBudget()`
    pub async fn enforce_budget(
        &self,
        tool_use_id: &str,
        tool_name: &str,
        result: &str,
    ) -> ToolResultDecision {
        let result_len = result.len();

        // 检查单条大小限制
        if result_len > self.config.max_result_size_chars {
            let file_path = self.persist_result(tool_use_id, tool_name, result).await;
            return ToolResultDecision::PersistToFile { file_path };
        }

        // 检查聚合预算
        let entries = self.entries.read().await;
        let current_total: usize = entries.iter().map(|e| e.size_chars).sum();
        let projected = current_total + result_len;
        let budget = self.config.aggregate_budget_chars;
        let threshold = (budget as f64 * 0.80) as usize; // 80% 阈值

        drop(entries);

        if projected > threshold {
            // 需要替换最大的已有结果
            let file_path = self.persist_result(tool_use_id, tool_name, result).await;
            return ToolResultDecision::PersistToFile { file_path };
        }

        // 记录条目
        let mut entries = self.entries.write().await;
        entries.push(ToolResultEntry {
            tool_use_id: tool_use_id.to_string(),
            tool_name: tool_name.to_string(),
            size_chars: result_len,
            persisted: false,
            persisted_path: None,
        });

        ToolResultDecision::Keep
    }

    /// 将工具结果持久化到磁盘
    /// 源自 Claude Code 的 `persistToolResult()`
    async fn persist_result(&self, tool_use_id: &str, tool_name: &str, result: &str) -> String {
        let persist_dir = match &self.config.persist_dir {
            Some(d) => d.clone(),
            None => {
                // 无持久化目录，直接截断
                return String::new();
            }
        };

        // 确保目录存在
        let _ = tokio::fs::create_dir_all(&persist_dir).await;

        let file_name = format!("{}_{}.txt", tool_name, tool_use_id);
        let file_path = persist_dir.join(&file_name);

        // 写入文件
        match tokio::fs::write(&file_path, result).await {
            Ok(_) => {
                // 更新条目
                let mut entries = self.entries.write().await;
                entries.push(ToolResultEntry {
                    tool_use_id: tool_use_id.to_string(),
                    tool_name: tool_name.to_string(),
                    size_chars: result.len(),
                    persisted: true,
                    persisted_path: Some(file_path.to_string_lossy().to_string()),
                });

                // 更新替换状态
                let mut state = self.replacement_state.write().await;
                state.replaced.insert(
                    tool_use_id.to_string(),
                    ReplacedResult {
                        original_size_chars: result.len(),
                        file_path: file_path.to_string_lossy().to_string(),
                        reason: ReplacementReason::BudgetExceeded,
                    },
                );

                file_path.to_string_lossy().to_string()
            }
            Err(e) => {
                tracing::warn!("[Budget] Failed to persist result: {} — using inline truncation", e);
                String::new()
            }
        }
    }

    /// 清除当前消息的条目（消息完成后调用）
    pub async fn reset(&self) {
        let mut entries = self.entries.write().await;
        entries.clear();
    }

    /// 获取内容替换状态（用于提示缓存）
    pub async fn get_replacement_state(&self) -> ContentReplacementState {
        self.replacement_state.read().await.clone()
    }

    /// 重建内容替换状态（从会话记录恢复时）
    pub async fn reconstruct_from(
        &self,
        state: ContentReplacementState,
    ) {
        let mut current = self.replacement_state.write().await;
        *current = state;
    }

    /// 处理空工具结果 — 注入占位文本
    /// 源自 Claude Code 的 empty result 处理
    pub fn handle_empty_result(tool_name: &str) -> String {
        format!("({} completed with no output)", tool_name)
    }
}

/// 预算决策
#[derive(Debug, Clone)]
pub enum ToolResultDecision {
    /// 保留内联结果
    Keep,
    /// 持久化到文件，注入文件路径引用
    PersistToFile {
        /// 持久化后的文件路径，空字符串表示失败
        file_path: String,
    },
}

impl ToolResultDecision {
    /// 获取展示给模型的结果文本
    pub fn display_text(&self, original_result: &str) -> String {
        match self {
            ToolResultDecision::Keep => original_result.to_string(),
            ToolResultDecision::PersistToFile { file_path } => {
                if file_path.is_empty() {
                    // 持久化失败，截断结果
                    let max = 5000;
                    if original_result.len() > max {
                        let kept = &original_result[..original_result.floor_char_boundary(max - 200)];
                        format!(
                            "{}\n\n⚠️ RESULT TRUNCATED: Original was {} chars. \
                             Only first ~{} chars shown. \
                             Use more targeted queries for full content.",
                            kept,
                            original_result.len(),
                            max
                        )
                    } else {
                        original_result.to_string()
                    }
                } else {
                    format!(
                        "[Tool result saved to {} ({} chars). \
                         The full content is available at this path.]",
                        file_path,
                        original_result.len(),
                    )
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_keep_small_result() {
        let manager = ToolResultBudgetManager::new(ToolResultBudgetConfig {
            max_result_size_chars: 1000,
            aggregate_budget_chars: 5000,
            ..Default::default()
        });

        let decision = manager.enforce_budget("call-1", "read", "small result").await;
        assert!(matches!(decision, ToolResultDecision::Keep));
    }

    #[tokio::test]
    async fn test_persist_large_result() {
        let temp_dir = std::env::temp_dir().join("jcode-test-budget");
        let manager = ToolResultBudgetManager::new(ToolResultBudgetConfig {
            max_result_size_chars: 10,
            aggregate_budget_chars: 5000,
            persist_dir: Some(temp_dir.clone()),
            ..Default::default()
        });

        let decision = manager.enforce_budget("call-1", "read", "this is a very long result that exceeds the limit").await;
        assert!(matches!(decision, ToolResultDecision::PersistToFile { .. }));

        // Cleanup
        let _ = tokio::fs::remove_dir_all(&temp_dir).await;
    }

    #[test]
    fn test_empty_result_handling() {
        let result = ToolResultBudgetManager::handle_empty_result("Bash");
        assert_eq!(result, "(Bash completed with no output)");
    }

    #[test]
    fn test_decision_display_text_keep() {
        let decision = ToolResultDecision::Keep;
        assert_eq!(decision.display_text("hello"), "hello");
    }

    #[test]
    fn test_decision_display_text_persisted() {
        let decision = ToolResultDecision::PersistToFile {
            file_path: "/tmp/result.txt".to_string(),
        };
        let text = decision.display_text("original content here");
        assert!(text.contains("/tmp/result.txt"));
        assert!(text.contains("original content here".len().to_string().as_str()));
    }

    #[test]
    fn test_decision_display_text_truncated() {
        let decision = ToolResultDecision::PersistToFile {
            file_path: "".to_string(),
        };
        let long = "x".repeat(10000);
        let text = decision.display_text(&long);
        assert!(text.contains("TRUNCATED"));
        assert!(text.len() < long.len());
    }
}
