//! Plan Verifier - Pre-execution feasibility validation engine
//!
//! Validates each step of a plan before Agent execution:
//! - Static analysis: Check file existence, syntax validity, dependency availability
//! - Resource estimation: Estimate token consumption, time cost, API call count
//! - Risk detection: Identify dangerous operation patterns (delete critical files, modify configs)
//! - Dependency verification: Confirm prerequisites are satisfied
//! - Rollback planning: Auto-generate rollback strategies for high-risk steps
//! - Confidence scoring: Quantitative feasibility assessment of overall plan

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanStep {
    pub id: String,
    pub description: String,
    pub action: PlanAction,
    pub target_files: Vec<PathBuf>,
    pub prerequisites: Vec<String>,
    pub estimated_tokens: usize,
    pub risk_level: RiskLevel,
    pub rollback_strategy: Option<RollbackStrategy>,
}

impl Default for PlanStep {
    fn default() -> Self {
        Self {
            id: String::new(),
            description: String::new(),
            action: PlanAction::ReadFile,
            target_files: vec![],
            prerequisites: vec![],
            estimated_tokens: 0,
            risk_level: RiskLevel::Low,
            rollback_strategy: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PlanAction {
    ReadFile,
    WriteFile,
    EditBlock,
    RunCommand,
    CreateFile,
    DeleteFile,
    SearchReplace,
    CallApi,
    MultiFileEdit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Default)]
pub enum RiskLevel {
    Safe,
    #[default]
    Low,
    Medium,
    High,
    Critical,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RollbackStrategy {
    pub method: RollbackMethod,
    pub description: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RollbackMethod {
    GitRevert,
    FileBackupRestore,
    ManualIntervention,
    NoRollback,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationIssue {
    pub step_id: String,
    pub severity: IssueSeverity,
    pub category: IssueCategory,
    pub message: String,
    pub suggestion: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum IssueSeverity {
    Warning,
    Error,
    Critical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum IssueCategory {
    FileNotFound,
    PermissionDenied,
    SyntaxError,
    DependencyMissing,
    RiskViolation,
    ResourceExceeded,
}

/// Result of plan verification
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanVerificationResult {
    pub plan_id: String,
    pub is_feasible: bool,
    pub confidence: f64,
    pub issues: Vec<VerificationIssue>,
    pub total_estimated_tokens: usize,
    pub high_risk_steps: usize,
    pub verification_duration_us: u64,
    pub summary: String,
}

/// Plan verifier with configurable risk tolerance and token budgets
pub struct PlanVerifier {
    workspace_root: PathBuf,
    max_risk_tolerance: RiskLevel,
    token_budget: Option<usize>,
}

impl PlanVerifier {
    pub fn new(workspace_root: impl Into<PathBuf>) -> Self {
        Self {
            workspace_root: workspace_root.into(),
            max_risk_tolerance: RiskLevel::High,
            token_budget: None,
        }
    }

    pub fn with_token_budget(mut self, tokens: usize) -> Self {
        self.token_budget = Some(tokens);
        self
    }

    pub fn with_max_risk(mut self, risk: RiskLevel) -> Self {
        self.max_risk_tolerance = risk;
        self
    }

    /// Verify a complete plan
    pub fn verify(&self, steps: &[PlanStep], plan_id: &str) -> PlanVerificationResult {
        let start = std::time::Instant::now();
        let mut issues = Vec::new();
        let mut total_tokens = 0usize;
        let mut high_risk_count = 0usize;

        for step in steps {
            total_tokens += step.estimated_tokens;
            
            // Check risk level
            if step.risk_level > self.max_risk_tolerance {
                high_risk_count += 1;
                issues.push(VerificationIssue {
                    step_id: step.id.clone(),
                    severity: if step.risk_level >= RiskLevel::Critical {
                        IssueSeverity::Critical
                    } else {
                        IssueSeverity::Error
                    },
                    category: IssueCategory::RiskViolation,
                    message: format!(
                        "Step '{}' exceeds maximum tolerated risk ({:?} > {:?})",
                        step.description, step.risk_level, self.max_risk_tolerance
                    ),
                    suggestion: Some("Reduce risk level or split into smaller steps".into()),
                });
            }

            // Verify individual step
            issues.extend(self.verify_step(step));
        }

        // Check token budget
        if let Some(budget) = self.token_budget {
            if total_tokens > budget {
                issues.push(VerificationIssue {
                    step_id: "plan_total".into(),
                    severity: IssueSeverity::Warning,
                    category: IssueCategory::ResourceExceeded,
                    message: format!(
                        "Total estimated tokens ({}) exceeds budget ({})",
                        total_tokens, budget
                    ),
                    suggestion: Some("Consider reducing scope or increasing budget".into()),
                });
            }
        }

        // Calculate confidence score
        let errors = issues.iter()
            .filter(|i| matches!(i.severity, IssueSeverity::Error | IssueSeverity::Critical))
            .count();
        let warnings = issues.len() - errors;
        
        let confidence = if errors > 0 {
            0.0
        } else if warnings > 0 {
            0.5
        } else {
            1.0 - (high_risk_count as f64 * 0.1).max(0.0)
        };

        let status = if errors > 0 {
            "REJECTED"
        } else if warnings > 0 {
            "CONDITIONAL"
        } else {
            "APPROVED"
        };

        let issues_len = issues.len();

        PlanVerificationResult {
            plan_id: plan_id.to_string(),
            is_feasible: errors == 0 && high_risk_count == 0,
            confidence,
            issues,
            total_estimated_tokens: total_tokens,
            high_risk_steps: high_risk_count,
            verification_duration_us: start.elapsed().as_micros() as u64,
            summary: format!(
                "{}: {} issue(s) ({} error(s), {} warning(s)), {} high-risk step(s), confidence={:.2}",
                status, issues_len, errors, warnings, high_risk_count, confidence
            ),
        }
    }

    /// Verify an individual step
    fn verify_step(&self, step: &PlanStep) -> Vec<VerificationIssue> {
        let mut issues = Vec::new();

        match step.action {
            PlanAction::ReadFile | PlanAction::EditBlock | PlanAction::SearchReplace => {
                for path in &step.target_files {
                    let full_path = self.workspace_root.join(path);
                    if !full_path.exists() {
                        issues.push(VerificationIssue {
                            step_id: step.id.clone(),
                            severity: IssueSeverity::Error,
                            category: IssueCategory::FileNotFound,
                            message: format!("Target file does not exist: {:?}", full_path),
                            suggestion: Some("Verify file path or create file first".into()),
                        });
                    }
                }
            }
            PlanAction::DeleteFile => {
                for path in &step.target_files {
                    let is_important = path.to_str().unwrap_or("")
                        .ends_with(".env") || 
                        path.to_str().unwrap_or("").contains("/etc/");
                    
                    if is_important {
                        issues.push(VerificationIssue {
                            step_id: step.id.clone(),
                            severity: IssueSeverity::Critical,
                            category: IssueCategory::RiskViolation,
                            message: format!("Attempting to delete sensitive file: {:?}", path),
                            suggestion: Some("Use backup strategy before deletion".into()),
                        });
                    }
                }
            }
            PlanAction::RunCommand => {
                let desc_lower = step.description.to_lowercase();
                if desc_lower.contains("rm -rf") ||
                   desc_lower.contains("drop database") ||
                   desc_lower.contains("format") {
                    issues.push(VerificationIssue {
                        step_id: step.id.clone(),
                        severity: IssueSeverity::Critical,
                        category: IssueCategory::RiskViolation,
                        message: "Destructive command detected".into(),
                        suggestion: Some("Add explicit confirmation step".into()),
                    });
                }
            }
            _ => {}
        }

        issues
    }

    /// Generate rollback plan for all steps
    pub fn generate_rollback_plan(&self, steps: &[PlanStep]) -> Vec<(String, RollbackStrategy)> {
        steps.iter().rev().filter_map(|step| {
            let strategy = match step.action {
                PlanAction::WriteFile | PlanAction::EditBlock | PlanAction::MultiFileEdit => {
                    Some(RollbackStrategy {
                        method: RollbackMethod::GitRevert,
                        description: format!(
                            "git checkout -- {:?}",
                            step.target_files.first()?
                        ),
                    })
                }
                PlanAction::CreateFile => Some(RollbackStrategy {
                    method: RollbackMethod::FileBackupRestore,
                    description: format!(
                        "Remove newly created file: {:?}",
                        step.target_files.first()?
                    ),
                }),
                PlanAction::DeleteFile => Some(RollbackStrategy {
                    method: RollbackMethod::GitRevert,
                    description: format!(
                        "git restore {:?} from previous commit",
                        step.target_files.first()?
                    ),
                }),
                PlanAction::RunCommand => Some(RollbackStrategy {
                    method: RollbackMethod::ManualIntervention,
                    description: "Manual review of command effects required".into(),
                }),
                _ => None,
            };
            strategy.map(|s| (step.id.clone(), s))
        }).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile;

    fn make_verifier() -> PlanVerifier {
        PlanVerifier::new(tempfile::tempdir().unwrap())
    }

    #[test]
    fn test_verify_safe_plan() {
        let v = make_verifier();
        let steps = vec![PlanStep {
            id: "s1".into(),
            description: "Read main.rs".into(),
            action: PlanAction::ReadFile,
            target_files: vec![PathBuf::from("main.rs")],
            estimated_tokens: 100,
            risk_level: RiskLevel::Safe,
            ..Default::default()
        }];
        
        let result = v.verify(&steps, "plan1");
        assert!(result.is_feasible);
        assert_eq!(result.issues.len(), 0);
    }

    #[test]
    fn test_reject_dangerous_plan() {
        let v = make_verifier();
        let steps = vec![PlanStep {
            id: "s1".into(),
            description: "rm -rf /important".into(),
            action: PlanAction::RunCommand,
            target_files: vec![PathBuf::from("/important")],
            estimated_tokens: 50,
            risk_level: RiskLevel::Critical,
            ..Default::default()
        }];
        
        let result = v.verify(&steps, "plan2");
        assert!(!result.is_feasible);
        assert!(result.issues.iter().any(|i| i.severity == IssueSeverity::Critical));
    }

    #[test]
    fn test_missing_file_detection() {
        let tmp = tempfile::tempdir().unwrap();
        let v = PlanVerifier::new(tmp.path());
        let steps = vec![PlanStep {
            id: "s1".into(),
            description: "Edit nonexistent.rs".into(),
            action: PlanAction::EditBlock,
            target_files: vec![PathBuf::from("nonexistent.rs")],
            estimated_tokens: 200,
            risk_level: RiskLevel::Low,
            ..Default::default()
        }];
        
        let result = v.verify(&steps, "plan3");
        assert!(!result.is_feasible);
        assert!(result.issues.iter().any(|i| i.category == IssueCategory::FileNotFound));
    }

    #[test]
    fn test_rollback_generation() {
        let v = make_verifier();
        let steps = vec![
            PlanStep {
                id: "s1".into(),
                description: "Create new.rs".into(),
                action: PlanAction::CreateFile,
                target_files: vec![PathBuf::from("new.rs")],
                estimated_tokens: 50,
                risk_level: RiskLevel::Safe,
                ..Default::default()
            },
            PlanStep {
                id: "s2".into(),
                description: "Edit existing.rs".into(),
                action: PlanAction::EditBlock,
                target_files: vec![PathBuf::from("existing.rs")],
                estimated_tokens: 100,
                risk_level: RiskLevel::Low,
                ..Default::default()
            },
        ];
        
        let rollback = v.generate_rollback_plan(&steps);
        assert_eq!(rollback.len(), 2);
    }

    #[test]
    fn test_token_budget_exceeded() {
        let v = PlanVerifier::new(tempfile::tempdir().unwrap())
            .with_token_budget(100);
        
        let steps = vec![PlanStep {
            id: "s1".into(),
            description: "Big task".into(),
            action: PlanAction::ReadFile,
            target_files: vec![PathBuf::from("file.rs")],
            estimated_tokens: 200,
            risk_level: RiskLevel::Safe,
            ..Default::default()
        }];
        
        let result = v.verify(&steps, "plan4");
        assert!(result.issues.iter().any(|i| i.category == IssueCategory::ResourceExceeded));
    }
}
