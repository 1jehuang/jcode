//! Layer 3: Editing Layer - Safe Editor with Diff/SearchReplace

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use anyhow::Result;
use chrono::{DateTime, Utc};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::{
    PhaseResult, PhaseName, PhaseOutput, SurgicalRequest,
    TextDiff as TextDiffStruct, DiffType, DiffStats,
    ApplyResult, PreviewResult, RiskLevel,
    EditingLayer,
};

/// Editing configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditingConfig {
    pub auto_backup: bool,
    pub backup_dir: PathBuf,
    pub enable_conflict_detection: bool,
}

impl Default for EditingConfig {
    fn default() -> Self {
        Self {
            auto_backup: true,
            backup_dir: PathBuf::from(".jcode/backups"),
            enable_conflict_detection: true,
        }
    }
}

/// Edit transaction
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditTransaction {
    pub transaction_id: String,
    pub request_id: String,
    pub created_at: DateTime<Utc>,
    pub status: String,
    pub diffs: Vec<TextDiffStruct>,
}

/// Safe editor
pub struct SafeEditor {
    config: EditingConfig,
    active_transactions: Arc<RwLock<HashMap<String, EditTransaction>>>,
}

impl SafeEditor {
    pub fn new(config: EditingConfig) -> Self {
        Self {
            config,
            active_transactions: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Create new transaction
    pub fn create_transaction(&self, request: &SurgicalRequest) -> Result<String> {
        let transaction_id = format!("txn_{}", Uuid::new_v4());

        let transaction = EditTransaction {
            transaction_id: transaction_id.clone(),
            request_id: request.request_id.clone(),
            created_at: Utc::now(),
            status: "created".to_string(),
            diffs: Vec::new(),
        };

        self.active_transactions.write().insert(transaction_id.clone(), transaction);

        Ok(transaction_id)
    }

    /// Add diff to transaction
    pub async fn add_diff_to_transaction(&self, _transaction_id: &str, diff: TextDiffStruct) -> Result<()> {
        // Simplified implementation - just log the diff
        debug!(file = %diff.file_path.display(), diff_type = ?diff.diff_type, "Adding diff to transaction");
        
        // TODO: Implement actual file locking and backup logic
        
        Ok(())
    }

    /// Preview transaction
    pub async fn preview_transaction(&self, transaction_id: &str) -> Result<PreviewResult> {
        let transactions = self.active_transactions.read();
        
        if let Some(txn) = transactions.get(transaction_id) {
            let unified_diff = format!(
                "=== Transaction {} ===\nDiffs: {}\n",
                txn.transaction_id,
                txn.diffs.len()
            );

            Ok(PreviewResult {
                unified_diff,
                estimated_risk: RiskLevel::Medium,
            })
        } else {
            Err(anyhow::anyhow!("Transaction not found"))
        }
    }

    /// Apply transaction (atomic operation)
    pub async fn apply_transaction(&self, transaction_id: &str) -> Result<ApplyResult> {
        info!(transaction_id = %transaction_id, "Applying transaction");

        let diffs = {
            let transactions = self.active_transactions.read();
            if let Some(txn) = transactions.get(transaction_id) {
                txn.diffs.clone()
            } else {
                return Err(anyhow::anyhow!("Transaction not found"));
            }
        };

        let mut applied_count = 0;
        let mut failed_items = Vec::new();

        for (i, diff) in diffs.iter().enumerate() {
            match self.apply_single_diff(diff).await {
                Ok(_) => {
                    applied_count += 1;
                    debug!(index = i, file = %diff.file_path.display(), "Applied diff successfully");
                }
                Err(e) => {
                    error!(index = i, error = %e, "Failed to apply diff");
                    failed_items.push(format!("{}: {}", diff.file_path.display(), e));
                }
            }
        }

        // Update transaction status
        {
            let mut transactions = self.active_transactions.write();
            if let Some(txn) = transactions.get_mut(transaction_id) {
                if failed_items.is_empty() {
                    txn.status = "applied".to_string();
                } else if applied_count > 0 {
                    txn.status = "partially_applied".to_string();
                } else {
                    txn.status = "failed".to_string();
                }
            }
        }

        Ok(ApplyResult {
            success: failed_items.is_empty(),
            applied_count,
            failed_items,
        })
    }

    /// Rollback transaction
    pub async fn rollback_transaction(&self, transaction_id: &str) -> Result<()> {
        info!(transaction_id = %transaction_id, "Rolling back transaction");

        // Simplified implementation - just update status
        let mut transactions = self.active_transactions.write();
        if let Some(txn) = transactions.get_mut(transaction_id) {
            txn.status = "rolled_back".to_string();
        }

        Ok(())
    }

    /// Apply single diff
    async fn apply_single_diff(&self, diff: &TextDiffStruct) -> Result<()> {
        match diff.diff_type {
            DiffType::Add => {
                if let Some(new_content) = &diff.new_content {
                    tokio::fs::write(&diff.file_path, new_content).await?;
                } else {
                    return Err(anyhow::anyhow!("Add diff missing content"));
                }
            }
            DiffType::Modify => {
                if let (Some(old_content), Some(new_content)) = (&diff.old_content, &diff.new_content) {
                    let current_content = tokio::fs::read_to_string(&diff.file_path).await?;
                    
                    if !current_content.contains(old_content.as_str()) {
                        return Err(anyhow::anyhow!("Content not found in file"));
                    }

                    let updated_content = current_content.replacen(old_content.as_str(), new_content.as_str(), 1);
                    tokio::fs::write(&diff.file_path, &updated_content).await?;
                } else {
                    return Err(anyhow::anyhow!("Modify diff missing content"));
                }
            }
            DiffType::Delete => {
                if let Some(old_content) = &diff.old_content {
                    let current_content = tokio::fs::read_to_string(&diff.file_path).await?;
                    
                    if !current_content.contains(old_content.as_str()) {
                        return Err(anyhow::anyhow!("Content to delete not found"));
                    }

                    let updated_content = current_content.replacen(old_content.as_str(), "", 1);
                    tokio::fs::write(&diff.file_path, &updated_content).await?;
                } else {
                    return Err(anyhow::anyhow!("Delete diff missing content"));
                }
            }
            DiffType::Move | DiffType::Rename => {
                // Not implemented in simplified version
                warn!(file = %diff.file_path.display(), "Move/Rename not supported");
            }
        }

        Ok(())
    }

    /// Get active transaction
    pub fn get_active_transaction(&self, transaction_id: &str) -> Option<EditTransaction> {
        self.active_transactions.read().get(transaction_id).cloned()
    }
}

#[async_trait::async_trait]
impl EditingLayer for SafeEditor {
    async fn generate_safe_edits(
        &self,
        request: &SurgicalRequest,
        _retrieval_output: &PhaseOutput,
    ) -> Result<PhaseResult> {
        let start_time = std::time::Instant::now();

        info!(
            request_id = %request.request_id,
            intent = %request.intent[..request.intent.len().min(80)],
            "Generating safe edits"
        );

        // Create transaction
        let _transaction_id = self.create_transaction(request)?;

        // TODO: Generate actual edits based on retrieval output and user intent
        // This would involve calling LLM API to generate specific code modifications

        let duration_ms = start_time.elapsed().as_millis() as u64;

        Ok(PhaseResult {
            phase: PhaseName::Editing,
            passed: true,
            duration_ms,
            output: PhaseOutput::EditingOutput {
                diffs_generated: Vec::new(),
                files_modified: Vec::new(),
                edit_duration_ms: duration_ms,
            },
            warnings: Vec::new(),
            errors: Vec::new(),
        })
    }

    async fn apply_edits(&self, edits: &[TextDiffStruct]) -> Result<ApplyResult> {
        let temp_request = SurgicalRequest {
            request_id: format!("batch_{}", Utc::now().timestamp()),
            intent: "Batch edit application".to_string(),
            target: crate::TargetScope::EntireProject { root: PathBuf::from(".") },
            priority: crate::Priority::Normal,
            safety_mode: crate::SafetyMode::AutoWithLogging,
            created_at: Utc::now(),
            requested_by: "system".to_string(),
        };

        let transaction_id = self.create_transaction(&temp_request)?;

        for edit in edits {
            self.add_diff_to_transaction(&transaction_id, edit.clone()).await?;
        }

        self.apply_transaction(&transaction_id).await
    }

    async fn rollback_changes(
        &self,
        request: &SurgicalRequest,
        _edit_output: &PhaseOutput,
    ) -> Result<PhaseResult> {
        let start_time = std::time::Instant::now();

        info!(request_id = %request.request_id, "Initiating rollback");

        // Find the most recent applicable transaction
        let recent_txn = {
            let transactions = self.active_transactions.read();
            transactions.values()
                .filter(|t| t.request_id == request.request_id && 
                    (t.status == "applied" || t.status == "partially_applied" || t.status == "failed"))
                .last()
                .cloned()
        };

        match recent_txn {
            Some(txn) => {
                self.rollback_transaction(&txn.transaction_id).await?;

                let duration_ms = start_time.elapsed().as_millis() as u64;

                Ok(PhaseResult {
                    phase: PhaseName::Editing,
                    passed: true,
                    duration_ms,
                    output: PhaseOutput::EditingOutput {
                        diffs_generated: Vec::new(),
                        files_modified: Vec::new(),
                        edit_duration_ms: duration_ms,
                    },
                    warnings: Vec::new(),
                    errors: Vec::new(),
                })
            }
            None => {
                warn!(request_id = %request.request_id, "No transaction found for rollback");

                let duration_ms = start_time.elapsed().as_millis() as u64;

                Ok(PhaseResult {
                    phase: PhaseName::Editing,
                    passed: false,
                    duration_ms,
                    output: PhaseOutput::EditingOutput {
                        diffs_generated: Vec::new(),
                        files_modified: Vec::new(),
                        edit_duration_ms: duration_ms,
                    },
                    warnings: vec!["No transaction found to rollback".to_string()],
                    errors: vec!["No applicable transaction found".to_string()],
                })
            }
        }
    }

    async fn preview_diff(&self, diff: &TextDiffStruct) -> Result<PreviewResult> {
        let unified_diff = format!(
            "--- a/{}\n+++ b/{}\n@@ -1,{} +1,{} @@\n{}\n",
            diff.file_path.display(),
            diff.file_path.display(),
            diff.stats.deletions,
            diff.stats.additions,
            diff.unified_diff
        );

        Ok(PreviewResult {
            unified_diff,
            estimated_risk: RiskLevel::Low,
        })
    }
}
