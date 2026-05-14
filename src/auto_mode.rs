use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutoModeConfig {
    pub enabled: bool,
    pub approval_threshold: f64,
    pub auto_accept_safe: bool,
    pub max_auto_actions: usize,
    pub require_confirmation_for: Vec<String>,
    pub learned_patterns: Vec<LearnedPattern>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LearnedPattern {
    pub pattern: String,
    pub action_type: ActionType,
    pub confidence: f64,
    pub times_accepted: usize,
    pub times_rejected: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ActionType {
    FileEdit,
    FileCreate,
    CommandExecution,
    GitOperation,
    PluginInstall,
    SshCommand,
    Other(String),
}

impl Default for AutoModeConfig {
    fn default() -> Self {
        AutoModeConfig {
            enabled: false,
            approval_threshold: 0.85,
            auto_accept_safe: true,
            max_auto_actions: 50,
            require_confirmation_for: vec![
                "delete".to_string(),
                "rm".to_string(),
                "force".to_string(),
                "push".to_string(),
                "deploy".to_string(),
            ],
            learned_patterns: vec![],
        }
    }
}

pub struct AutoModeEngine {
    config: AutoModeConfig,
    action_history: Vec<ActionRecord>,
}

#[derive(Debug, Clone)]
struct ActionRecord {
    action_type: ActionType,
    description: String,
    auto_approved: bool,
    timestamp: chrono::DateTime<chrono::Utc>,
}

impl AutoModeEngine {
    pub fn new(config: AutoModeConfig) -> Self {
        AutoModeEngine {
            config,
            action_history: vec![],
        }
    }

    pub fn is_enabled(&self) -> bool { self.config.enabled }

    pub fn toggle(&mut self) -> bool {
        self.config.enabled = !self.config.enabled;
        self.config.enabled
    }

    pub fn should_auto_approve(&mut self, action_type: &ActionType, description: &str) -> AutoApprovalDecision {
        if !self.config.enabled {
            return AutoApprovalDecision::ManualReview;
        }

        let requires_confirm = self.config.require_confirmation_for.iter().any(|keyword| {
            description.to_lowercase().contains(&keyword.to_lowercase())
        });

        if requires_confirm {
            return AutoApprovalDecision::RequiresConfirmation("Action contains sensitive keyword".to_string());
        }

        let pattern_match = self.find_matching_pattern(description);

        match pattern_match {
            Some(pattern) if pattern.confidence >= self.config.approval_threshold => {
                AutoApprovalDecision::AutoApprove(format!(
                    "High confidence ({:.1}%) based on learned pattern",
                    pattern.confidence * 100.0
                ))
            }
            Some(pattern) => {
                AutoApprovalDecision::SuggestApprove(format!(
                    "Medium confidence ({:.1}%) - review recommended",
                    pattern.confidence * 100.0
                ))
            }
            None if self.config.auto_accept_safe && Self::is_safe_action(action_type) => {
                AutoApprovalDecision::AutoApprove("Safe operation in auto-accept mode".to_string())
            }
            None => AutoApprovalDecision::ManualReview,
        }
    }

    pub fn record_decision(&mut self, action_type: ActionType, description: &str, approved: bool) {
        let action_clone = action_type.clone();
        self.action_history.push(ActionRecord {
            action_type,
            description: description.to_string(),
            auto_approved: approved,
            timestamp: chrono::Utc::now(),
        });
        self.update_pattern(action_clone, description, approved);
    }

    fn find_matching_pattern(&self, description: &str) -> Option<&LearnedPattern> {
        self.config.learned_patterns.iter().find(|p| {
            description.to_lowercase().contains(&p.pattern.to_lowercase())
        })
    }

    fn update_pattern(&mut self, action_type: ActionType, description: &str, approved: bool) {
        let pattern_str = Self::extract_pattern(description);
        let existing = self.config.learned_patterns.iter_mut().find(|p| p.pattern == pattern_str);

        match existing {
            Some(pattern) => {
                if approved {
                    pattern.times_accepted += 1;
                } else {
                    pattern.times_rejected += 1;
                }
                let total = pattern.times_accepted + pattern.times_rejected;
                pattern.confidence = pattern.times_accepted as f64 / total as f64;
            }
            None => {
                if self.config.learned_patterns.len() < 100 {
                    self.config.learned_patterns.push(LearnedPattern {
                        pattern: pattern_str,
                        action_type,
                        confidence: if approved { 1.0 } else { 0.0 },
                        times_accepted: if approved { 1 } else { 0 },
                        times_rejected: if approved { 0 } else { 1 },
                    });
                }
            }
        }
    }

    fn extract_pattern(description: &str) -> String {
        let words: Vec<&str> = description.split_whitespace().collect();
        if words.len() <= 3 {
            description.to_string()
        } else {
            format!("{} {}", words[0], words[1])
        }
    }

    fn is_safe_action(action_type: &ActionType) -> bool {
        matches!(action_type, ActionType::FileEdit | ActionType::FileCreate)
    }

    pub fn get_stats(&self) -> AutoModeStats {
        let auto_count = self.action_history.iter().filter(|a| a.auto_approved).count();
        AutoModeStats {
            total_actions: self.action_history.len(),
            auto_approved: auto_count,
            manual_review: self.action_history.len() - auto_count,
            patterns_learned: self.config.learned_patterns.len(),
        }
    }

    pub fn get_config(&self) -> &AutoModeConfig { &self.config }

    pub fn set_approval_threshold(&mut self, threshold: f64) {
        self.config.approval_threshold = threshold.clamp(0.0, 1.0);
    }
}

#[derive(Debug)]
pub enum AutoApprovalDecision {
    AutoApprove(String),
    SuggestApprove(String),
    RequiresConfirmation(String),
    ManualReview,
}

#[derive(Debug)]
pub struct AutoModeStats {
    pub total_actions: usize,
    pub auto_approved: usize,
    pub manual_review: usize,
    pub patterns_learned: usize,
}
