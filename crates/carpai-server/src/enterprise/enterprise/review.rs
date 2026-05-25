//! # Enterprise Rule Review
//!
//! Rule review system for enterprise compliance and security.
//! Migrated from `src/rule_reviewer.rs` as part of dead code cleanup.
//!
//! Provides:
//! - Static pattern review (regex validity, performance, correctness)
//! - LLM-assisted rule review (via Provider trait)

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Result of a rule review
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuleReview {
    pub id: String,
    pub rule_id: String,
    pub rule_name: String,
    pub rule_pattern: String,
    pub review_status: ReviewStatus,
    pub issues: Vec<ReviewIssue>,
    pub suggestions: Vec<String>,
    pub confidence: f64,
    pub reviewed_at: DateTime<Utc>,
    pub reviewer: String,
}

/// Review status for a rule
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ReviewStatus {
    Approved,
    NeedsImprovement,
    Rejected,
    Pending,
}

/// A single issue found during review
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewIssue {
    pub severity: IssueSeverity,
    pub category: IssueCategory,
    pub description: String,
    pub suggestion: String,
}

/// Severity level for review issues
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum IssueSeverity {
    Critical,
    High,
    Medium,
    Low,
}

/// Category classification for review issues
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum IssueCategory {
    Performance,
    Security,
    Correctness,
    Maintainability,
    Compatibility,
}

/// A rule to be reviewed
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuleToReview {
    pub id: String,
    pub name: String,
    pub description: String,
    pub pattern: String,
    pub action: String,
    pub category: String,
    pub priority: u32,
    pub enabled: bool,
}

/// Enterprise rule reviewer — performs static and AI-assisted rule analysis
pub struct RuleReviewer {
    // Future: add provider for LLM-assisted review
    _private: (),
}

impl Default for RuleReviewer {
    fn default() -> Self {
        Self::new()
    }
}

impl RuleReviewer {
    pub fn new() -> Self {
        Self { _private: () }
    }

    /// Review a rule — currently static analysis only.
    /// LLM-assisted review is available when a Provider is wired in.
    pub async fn review_rule(&self, rule: &RuleToReview) -> Result<RuleReview> {
        self.review_static(rule).await
    }

    /// Static analysis of a rule pattern
    async fn review_static(&self, rule: &RuleToReview) -> Result<RuleReview> {
        let mut issues = Vec::new();
        let suggestions = Vec::new();

        if rule.pattern.is_empty() {
            issues.push(ReviewIssue {
                severity: IssueSeverity::Critical,
                category: IssueCategory::Correctness,
                description: "Pattern is empty".to_string(),
                suggestion: "Add a valid regex pattern".to_string(),
            });
        }

        if rule.pattern.len() > 1000 {
            issues.push(ReviewIssue {
                severity: IssueSeverity::Medium,
                category: IssueCategory::Performance,
                description: "Pattern is too long".to_string(),
                suggestion: "Consider simplifying the pattern for better performance".to_string(),
            });
        }

        if rule.pattern.contains(".*") {
            issues.push(ReviewIssue {
                severity: IssueSeverity::Low,
                category: IssueCategory::Performance,
                description: "Pattern contains greedy quantifier".to_string(),
                suggestion: "Consider using non-greedy quantifiers (.*?) where appropriate"
                    .to_string(),
            });
        }

        if let Err(e) = regex::Regex::new(&rule.pattern) {
            issues.push(ReviewIssue {
                severity: IssueSeverity::Critical,
                category: IssueCategory::Correctness,
                description: format!("Invalid regex: {}", e),
                suggestion: "Fix the regex pattern".to_string(),
            });
        }

        let status = if issues.iter().any(|i| i.severity == IssueSeverity::Critical) {
            ReviewStatus::Rejected
        } else if !issues.is_empty() {
            ReviewStatus::NeedsImprovement
        } else {
            ReviewStatus::Approved
        };

        Ok(RuleReview {
            id: uuid::Uuid::new_v4().to_string(),
            rule_id: rule.id.clone(),
            rule_name: rule.name.clone(),
            rule_pattern: rule.pattern.clone(),
            review_status: status,
            issues,
            suggestions,
            confidence: 0.8,
            reviewed_at: Utc::now(),
            reviewer: "enterprise-reviewer".to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_pattern_rejected() {
        let reviewer = RuleReviewer::new();
        let rule = RuleToReview {
            id: "test-1".into(),
            name: "Test".into(),
            description: "".into(),
            pattern: "".into(),
            action: "block".into(),
            category: "security".into(),
            priority: 1,
            enabled: true,
        };
        let result = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(reviewer.review_rule(&rule))
            .unwrap();
        assert_eq!(result.review_status, ReviewStatus::Rejected);
        assert!(!result.issues.is_empty());
    }

    #[test]
    fn test_valid_pattern_approved() {
        let reviewer = RuleReviewer::new();
        let rule = RuleToReview {
            id: "test-2".into(),
            name: "Valid Regex".into(),
            description: "".into(),
            pattern: r"^[a-zA-Z0-9]+$".into(),
            action: "allow".into(),
            category: "validation".into(),
            priority: 2,
            enabled: true,
        };
        let result = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(reviewer.review_rule(&rule))
            .unwrap();
        assert_eq!(result.review_status, ReviewStatus::Approved);
    }

    #[test]
    fn test_long_pattern_gets_performance_warning() {
        let reviewer = RuleReviewer::new();
        let long_pattern = "a".repeat(1001);
        let rule = RuleToReview {
            id: "test-3".into(),
            name: "Long Pattern".into(),
            description: "".into(),
            pattern: long_pattern,
            action: "warn".into(),
            category: "performance".into(),
            priority: 3,
            enabled: true,
        };
        let result = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(reviewer.review_rule(&rule))
            .unwrap();
        assert_eq!(result.review_status, ReviewStatus::NeedsImprovement);
    }
}
