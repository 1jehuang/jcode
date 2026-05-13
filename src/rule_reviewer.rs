use anyhow::Result;
use chrono::{DateTime, Utc};
use futures::StreamExt;
use jcode_message_types::Message;
use jcode_provider_core::{EventStream, Provider};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ReviewStatus {
    Approved,
    NeedsImprovement,
    Rejected,
    Pending,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewIssue {
    pub severity: IssueSeverity,
    pub category: IssueCategory,
    pub description: String,
    pub suggestion: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum IssueSeverity {
    Critical,
    High,
    Medium,
    Low,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum IssueCategory {
    Performance,
    Security,
    Correctness,
    Maintainability,
    Compatibility,
}

pub struct RuleReviewer {
    provider: Option<Arc<dyn Provider>>,
}

impl Default for RuleReviewer {
    fn default() -> Self {
        Self::new()
    }
}

impl RuleReviewer {
    pub fn new() -> Self {
        Self { provider: None }
    }

    pub fn with_provider(mut self, provider: Arc<dyn Provider>) -> Self {
        self.provider = Some(provider);
        self
    }

    pub async fn review_rule(&self, rule: &RuleToReview) -> Result<RuleReview> {
        if let Some(provider) = &self.provider {
            self.review_with_llm(provider.as_ref(), rule).await
        } else {
            self.review_static(rule).await
        }
    }

    async fn review_with_llm(
        &self,
        provider: &dyn Provider,
        rule: &RuleToReview,
    ) -> Result<RuleReview> {
        use jcode_message_types::StreamEvent;
        
        let prompt = self.build_review_prompt(rule);
        let messages = vec![Message::user(&prompt)];
        let mut stream: EventStream = provider.complete(&messages, &[], "", None).await?;
        
        let mut response = String::new();
        while let Some(event) = stream.next().await {
            if let Ok(event) = event
                && let StreamEvent::TextDelta(text) = event { response.push_str(&text) }
        }
        
        self.parse_llm_response(&response, rule).await
    }

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
                suggestion: "Consider using non-greedy quantifiers (.*?) where appropriate".to_string(),
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
            id: self.generate_id(),
            rule_id: rule.id.clone(),
            rule_name: rule.name.clone(),
            rule_pattern: rule.pattern.clone(),
            review_status: status,
            issues,
            suggestions,
            confidence: 0.8,
            reviewed_at: Utc::now(),
            reviewer: "static".to_string(),
        })
    }

    fn build_review_prompt(&self, rule: &RuleToReview) -> String {
        format!(
            r#"You are an expert security rule reviewer. Please review the following classifier rule:

Rule ID: {}
Rule Name: {}
Rule Description: {}
Rule Pattern: {}
Rule Action: {}
Rule Category: {}
Rule Priority: {}
Rule Enabled: {}

Please analyze this rule for:
1. Correctness: Is the pattern valid and does it match what it's supposed to match?
2. Security: Could this pattern be exploited or cause false positives/negatives?
3. Performance: Is the pattern efficient or could it cause performance issues?
4. Maintainability: Is the pattern clear and maintainable?

Provide your review in JSON format:
{{
  "status": "APPROVED" | "NEEDS_IMPROVEMENT" | "REJECTED",
  "confidence": 0.0-1.0,
  "issues": [
    {{
      "severity": "CRITICAL" | "HIGH" | "MEDIUM" | "LOW",
      "category": "PERFORMANCE" | "SECURITY" | "CORRECTNESS" | "MAINTAINABILITY" | "COMPATIBILITY",
      "description": "issue description",
      "suggestion": "fix suggestion"
    }}
  ],
  "suggestions": ["suggestion 1", "suggestion 2"]
}}"#,
            rule.id,
            rule.name,
            rule.description,
            rule.pattern,
            rule.action,
            rule.category,
            rule.priority,
            rule.enabled
        )
    }

    async fn parse_llm_response(&self, response: &str, rule: &RuleToReview) -> Result<RuleReview> {
        let trimmed = response.trim();
        let json_str = if trimmed.starts_with('{') {
            trimmed
        } else if let Some(start) = trimmed.find('{') {
            &trimmed[start..]
        } else {
            return self.review_static(rule).await;
        };

        match serde_json::from_str::<serde_json::Value>(json_str) {
            Ok(result) => {
                let status_str = result["status"]
                    .as_str()
                    .unwrap_or("PENDING")
                    .to_uppercase();

                let status = match status_str.as_str() {
                    "APPROVED" => ReviewStatus::Approved,
                    "NEEDS_IMPROVEMENT" => ReviewStatus::NeedsImprovement,
                    "REJECTED" => ReviewStatus::Rejected,
                    _ => ReviewStatus::Pending,
                };

                let issues: Vec<ReviewIssue> = result["issues"]
                    .as_array()
                    .unwrap_or(&vec![])
                    .iter()
                    .filter_map(|issue| self.parse_issue(issue))
                    .collect();

                let suggestions: Vec<String> = result["suggestions"]
                    .as_array()
                    .unwrap_or(&vec![])
                    .iter()
                    .filter_map(|s| s.as_str().map(|s| s.to_string()))
                    .collect();

                Ok(RuleReview {
                    id: self.generate_id(),
                    rule_id: rule.id.clone(),
                    rule_name: rule.name.clone(),
                    rule_pattern: rule.pattern.clone(),
                    review_status: status,
                    issues,
                    suggestions,
                    confidence: result["confidence"].as_f64().unwrap_or(0.7),
                    reviewed_at: Utc::now(),
                    reviewer: "llm".to_string(),
                })
            }
            Err(_) => Ok(self.review_static(rule).await?),
        }
    }

    fn parse_issue(&self, issue: &serde_json::Value) -> Option<ReviewIssue> {
        let severity_str = issue["severity"].as_str()?;
        let category_str = issue["category"].as_str()?;

        let severity = match severity_str.to_uppercase().as_str() {
            "CRITICAL" => IssueSeverity::Critical,
            "HIGH" => IssueSeverity::High,
            "MEDIUM" => IssueSeverity::Medium,
            "LOW" => IssueSeverity::Low,
            _ => IssueSeverity::Low,
        };

        let category = match category_str.to_uppercase().as_str() {
            "PERFORMANCE" => IssueCategory::Performance,
            "SECURITY" => IssueCategory::Security,
            "CORRECTNESS" => IssueCategory::Correctness,
            "MAINTAINABILITY" => IssueCategory::Maintainability,
            "COMPATIBILITY" => IssueCategory::Compatibility,
            _ => IssueCategory::Maintainability,
        };

        Some(ReviewIssue {
            severity,
            category,
            description: issue["description"].as_str().unwrap_or("").to_string(),
            suggestion: issue["suggestion"].as_str().unwrap_or("").to_string(),
        })
    }

    fn generate_id(&self) -> String {
        let timestamp = Utc::now().timestamp_millis();
        let random: u32 = rand::random();
        format!("review_{}_{:x}", timestamp, random)
    }
}

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

impl From<super::classifier::ClassifierRule> for RuleToReview {
    fn from(rule: super::classifier::ClassifierRule) -> Self {
        Self {
            id: rule.id,
            name: rule.name,
            description: rule.description,
            pattern: rule.pattern,
            action: format!("{:?}", rule.action),
            category: format!("{:?}", rule.category),
            priority: rule.priority,
            enabled: rule.enabled,
        }
    }
}