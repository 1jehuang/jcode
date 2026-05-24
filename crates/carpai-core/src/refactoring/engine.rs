//! # RefactorEngine — 统一重构入口
//!
//! 串联 PreciseEditEngine + CheckpointManager + AtomicEditCoordinator，
//! 让所有编辑 Tool (Edit/MultiEdit/ApplyPatch/BatchEdit) 经此执行。
//!
//! ## 核心流程
//! ```text
//! EditTool.execute()
//!     -> RefactorEngine.execute()
//!         -> CheckpointManager.track_edit() (备份)
//!         -> AtomicEditCoordinator (事务)
//!             -> PreciseEditEngine (精确编辑)
//!         -> 验证
//!     -> 如果失败: CheckpointManager.rewind() (回滚)
//! ```

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use tracing::{info, warn};

use super::atomic_edit::{
    AtomicEditCoordinator, TransactionStatus,
};
use super::precise_edit::{EditOperation, EditResult};

/// 重构操作结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefactorResult {
    /// 操作是否成功
    pub success: bool,
    /// 修改的文件数
    pub files_modified: usize,
    /// 各文件编辑结果
    pub edit_results: Vec<EditResult>,
    /// 事务 ID (用于回滚)
    pub transaction_id: Option<String>,
    /// 是否创建了快照
    pub snapshot_created: bool,
    /// 耗时 (ms)
    pub duration_ms: u64,
    /// 错误信息
    pub error: Option<String>,
}

/// 重构引擎配置
#[derive(Debug, Clone)]
pub struct RefactorConfig {
    /// 是否启用 Checkpoint 备份
    pub enable_checkpoints: bool,
    /// 是否启用两阶段提交
    pub enable_two_phase_commit: bool,
    /// 是否自动回滚失败的操作
    pub auto_rollback: bool,
    /// 临时文件目录
    pub temp_dir: PathBuf,
}

impl Default for RefactorConfig {
    fn default() -> Self {
        Self {
            enable_checkpoints: true,
            enable_two_phase_commit: true,
            auto_rollback: true,
            temp_dir: std::env::temp_dir(),
        }
    }
}

/// 统一重构引擎 — 串联所有编辑基础设施
pub struct RefactorEngine {
    coordinator: AtomicEditCoordinator,
    config: RefactorConfig,
}

impl RefactorEngine {
    /// 创建新的重构引擎
    pub fn new(_session_id: impl Into<String>) -> Self {
        let config = RefactorConfig::default();
        Self {
            coordinator: AtomicEditCoordinator::new(config.temp_dir.clone()),
            config,
        }
    }

    /// 使用自定义配置创建
    pub fn with_config(_session_id: impl Into<String>, config: RefactorConfig) -> Self {
        Self {
            coordinator: AtomicEditCoordinator::new(config.temp_dir.clone()),
            config,
        }
    }

    /// 执行单个编辑操作 (最常用的入口)
    pub async fn execute_edit(&mut self, op: EditOperation) -> RefactorResult {
        self.execute_edits(vec![op]).await
    }

    /// 执行多个编辑操作 (原子事务)
    pub async fn execute_edits(&mut self, ops: Vec<EditOperation>) -> RefactorResult {
        let start = std::time::Instant::now();

        if ops.is_empty() {
            return RefactorResult {
                success: true, files_modified: 0, edit_results: vec![],
                transaction_id: None, snapshot_created: false,
                duration_ms: start.elapsed().as_millis() as u64, error: None,
            };
        }

        // Step 1: Skip checkpoint backup (checkpoint module not yet implemented)
        let snapshot_created = false;

        // Step 2: 如果启用两阶段提交，使用临时文件
        if self.config.enable_two_phase_commit {
            self.execute_with_two_phase_commit(ops, start, snapshot_created).await
        } else {
            self.execute_direct(ops, start, snapshot_created).await
        }
    }

    /// 直接执行 (无两阶段提交)
    async fn execute_direct(
        &mut self,
        ops: Vec<EditOperation>,
        start: std::time::Instant,
        snapshot_created: bool,
    ) -> RefactorResult {
        // Begin transaction
        let tx_id = match self.coordinator.begin_transaction(ops.clone()) {
            Ok(id) => id,
            Err(e) => {
                return RefactorResult {
                    success: false, files_modified: 0, edit_results: vec![],
                    transaction_id: None, snapshot_created,
                    duration_ms: start.elapsed().as_millis() as u64,
                    error: Some(format!("Failed to begin transaction: {}", e)),
                };
            }
        };

        // Commit
        let result = match self.coordinator.commit(&tx_id).await {
            Ok(coord_result) => {
                if coord_result.status == TransactionStatus::Committed {
                    RefactorResult {
                        success: true,
                        files_modified: coord_result.files_modified,
                        edit_results: coord_result.results,
                        transaction_id: Some(tx_id),
                        snapshot_created,
                        duration_ms: start.elapsed().as_millis() as u64,
                        error: None,
                    }
                } else {
                    // Partial failure — auto rollback if configured
                    if self.config.auto_rollback {
                        if let Err(e) = self.coordinator.rollback(&tx_id).await {
                            warn!("Auto-rollback failed: {}", e);
                        }
                    }
                    RefactorResult {
                        success: false,
                        files_modified: 0,
                        edit_results: coord_result.results,
                        transaction_id: Some(tx_id),
                        snapshot_created,
                        duration_ms: start.elapsed().as_millis() as u64,
                        error: coord_result.error,
                    }
                }
            }
            Err(e) => {
                if self.config.auto_rollback {
                    let _ = self.coordinator.rollback(&tx_id).await;
                }
                RefactorResult {
                    success: false, files_modified: 0, edit_results: vec![],
                    transaction_id: Some(tx_id), snapshot_created,
                    duration_ms: start.elapsed().as_millis() as u64,
                    error: Some(format!("Commit failed: {}", e)),
                }
            }
        };

        result
    }

    /// 两阶段提交执行
    async fn execute_with_two_phase_commit(
        &mut self,
        ops: Vec<EditOperation>,
        start: std::time::Instant,
        snapshot_created: bool,
    ) -> RefactorResult {
        // Phase 1: 写入临时文件
        let mut temp_files: HashMap<PathBuf, PathBuf> = HashMap::new(); // target -> temp
        let mut phase1_success = true;
        let mut phase1_error = String::new();

        for op in &ops {
            let target = &op.file_path;
            if !target.exists() {
                continue;
            }

            // Create temp file path
            let file_name = target.file_name().unwrap_or_default().to_string_lossy();
            let temp_path = self.config.temp_dir.join(format!(
                ".{}.tmp.{}",
                file_name,
                std::process::id()
            ));

            // Copy to temp
            match std::fs::copy(target, &temp_path) {
                Ok(_) => {
                    temp_files.insert(target.clone(), temp_path);
                }
                Err(e) => {
                    phase1_success = false;
                    phase1_error = format!("Phase 1 failed: cannot backup {:?}: {}", target, e);
                    break;
                }
            }
        }

        if !phase1_success {
            // Cleanup temp files
            for (_, temp) in &temp_files {
                let _ = std::fs::remove_file(temp);
            }
            return RefactorResult {
                success: false, files_modified: 0, edit_results: vec![],
                transaction_id: None, snapshot_created,
                duration_ms: start.elapsed().as_millis() as u64,
                error: Some(phase1_error),
            };
        }

        // Execute edits (direct write to target files)
        let edit_result = self.execute_direct(ops, start, snapshot_created).await;

        if !edit_result.success {
            // Phase 2 failed — restore from temp files (atomic rename)
            for (target, temp) in &temp_files {
                match std::fs::rename(temp, target) {
                    Ok(_) => info!("Restored {:?} from temp backup", target),
                    Err(e) => warn!("Failed to restore {:?}: {}", target, e),
                }
            }
            return edit_result;
        }

        // Phase 2 succeeded — cleanup temp files
        for (_, temp) in &temp_files {
            let _ = std::fs::remove_file(temp);
        }

        edit_result
    }

    /// 回滚到指定事务
    pub async fn rollback(&mut self, transaction_id: &str) -> Result<usize> {
        self.coordinator.rollback(transaction_id).await
            .map_err(|e| anyhow::anyhow!("Rollback failed: {}", e))
    }

    #[allow(dead_code)]
    pub async fn rewind_to_message(&self, _message_id: &str) -> Result<Vec<String>> {
        Err(anyhow::anyhow!("Checkpoint not yet implemented"))
    }

    #[allow(dead_code)]
    pub async fn create_snapshot(&self, _message_id: &str) -> Result<()> {
        Err(anyhow::anyhow!("Checkpoint not yet implemented"))
    }
}
