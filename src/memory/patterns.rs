use chrono::Duration;
use uuid::Uuid;
use super::analysis::*;

pub struct PatternDetector;

impl PatternDetector {
    pub fn detect_anti_patterns(&self, analyses: &[SessionAnalysis]) -> Vec<AntiPatternDetection> {
        let mut detections = Vec::new();

        let mut retry_counts: Vec<(Uuid, usize)> = analyses
            .iter()
            .map(|a| (a.session_id, a.metrics.retry_count))
            .collect();
        retry_counts.sort_by_key(|&(_, c)| std::cmp::Reverse(c));

        let total_retries: usize = retry_counts.iter().map(|&(_, c)| c).sum();
        let retry_sessions = retry_counts.iter().filter(|&&(_, c)| c > 2).count();

        if retry_sessions > analyses.len() / 3 && total_retries > 10 {
            detections.push(AntiPatternDetection {
                pattern: AntiPatternType::ExcessiveRetry,
                frequency: retry_sessions,
                severity: if total_retries > 50 {
                    AntiPatternSeverity::High
                } else {
                    AntiPatternSeverity::Medium
                },
                suggested_alternative: "Implement pre-flight validation before command execution"
                    .to_string(),
                examples: retry_counts
                    .iter()
                    .take(3)
                    .map(|&(id, c)| format!("Session {}: {} retries", id, c))
                    .collect(),
                time_wasted_estimate: Duration::minutes((total_retries * 2) as i64),
            });
        }

        let unresolved_errors: Vec<(Uuid, usize)> = analyses
            .iter()
            .map(|a| {
                let unresolved = a
                    .metrics
                    .errors_encountered_in_session()
                    .iter()
                    .filter(|e| !e.resolved)
                    .count();
                (a.session_id, unresolved)
            })
            .filter(|&(_, c)| c > 0)
            .collect();

        if !unresolved_errors.is_empty() {
            detections.push(AntiPatternDetection {
                pattern: AntiPatternType::IgnoringErrors,
                frequency: unresolved_errors.len(),
                severity: AntiPatternSeverity::Medium,
                suggested_alternative:
                    "Always resolve or document errors before moving on".to_string(),
                examples: unresolved_errors
                    .iter()
                    .take(3)
                    .map(|&(id, c)| format!("Session {}: {} unresolved errors", id, c))
                    .collect(),
                time_wasted_estimate: Duration::minutes(
                    (unresolved_errors.iter().map(|&(_, c)| c).sum::<usize>() * 5) as i64,
                ),
            });
        }

        let no_test_sessions: Vec<Uuid> = analyses
            .iter()
            .filter(|a| {
                !a.metrics
                    .commands_executed
                    .iter()
                    .any(|c| matches!(c.category, CommandCategory::Test))
                    && !a.metrics.files_modified.is_empty()
            })
            .map(|a| a.session_id)
            .collect();

        if no_test_sessions.len() > analyses.len() / 2 {
            detections.push(AntiPatternDetection {
                pattern: AntiPatternType::SkippingTests,
                frequency: no_test_sessions.len(),
                severity: AntiPatternSeverity::Medium,
                suggested_alternative: "Run tests after every meaningful change".to_string(),
                examples: no_test_sessions
                    .iter()
                    .take(3)
                    .map(|id| format!("Session {}: no tests run", id))
                    .collect(),
                time_wasted_estimate: Duration::hours(no_test_sessions.len() as i64),
            });
        }

        detections
    }

    pub fn detect_inefficient_patterns(
        &self,
        analyses: &[SessionAnalysis],
    ) -> Vec<InefficientPattern> {
        let mut patterns = Vec::new();

        let long_wait_sessions: Vec<&SessionAnalysis> = analyses
            .iter()
            .filter(|a| {
                let total_ms = a.metrics.total_duration.num_milliseconds();
                let wait_ms = a.metrics.waiting_time.num_milliseconds();
                total_ms > 0 && (wait_ms as f64 / total_ms as f64) > 0.7
            })
            .collect();

        if !long_wait_sessions.is_empty() {
            patterns.push(InefficientPattern {
                pattern: InefficientPatternType::ExcessiveWaiting,
                affected_sessions: long_wait_sessions.iter().map(|a| a.session_id).collect(),
                description: format!(
                    "{} sessions spent >70% of time waiting",
                    long_wait_sessions.len()
                ),
                average_wait_ratio: long_wait_sessions
                    .iter()
                    .map(|a| {
                        let total_ms = a.metrics.total_duration.num_milliseconds();
                        if total_ms > 0 {
                            a.metrics.waiting_time.num_milliseconds() as f64 / total_ms as f64
                        } else {
                            0.0
                        }
                    })
                    .sum::<f64>()
                    / long_wait_sessions.len() as f64,
            });
        }

        let high_tool_usage_sessions: Vec<&SessionAnalysis> = analyses
            .iter()
            .filter(|a| {
                a.metrics.tool_call_count.values().sum::<usize>() > 50
            })
            .collect();

        if !high_tool_usage_sessions.is_empty() {
            patterns.push(InefficientPattern {
                pattern: InefficientPatternType::OverToolUsage,
                affected_sessions: high_tool_usage_sessions.iter().map(|a| a.session_id).collect(),
                description: format!(
                    "{} sessions used tools excessively (>50 calls)",
                    high_tool_usage_sessions.len()
                ),
                average_wait_ratio: 0.0,
            });
        }

        patterns
    }

    pub fn detect_habits(&self, analyses: &[SessionAnalysis]) -> UserHabitProfile {
        let mut habit_profile = UserHabitProfile {
            session_count: analyses.len(),
            average_session_duration: Duration::zero(),
            most_common_actions: Vec::new(),
            error_prone_patterns: Vec::new(),
            productivity_patterns: Vec::new(),
            learning_opportunities: Vec::new(),
        };

        if analyses.is_empty() {
            return habit_profile;
        }

        let total_duration: i64 = analyses
            .iter()
            .map(|a| a.metrics.total_duration.num_milliseconds())
            .sum();
        habit_profile.average_session_duration = Duration::milliseconds(total_duration / analyses.len() as i64);

        let mut action_counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        for analysis in analyses {
            for (tool_name, count) in &analysis.metrics.tool_call_count {
                *action_counts.entry(tool_name.clone()).or_insert(0) += count;
            }
        }

        let mut sorted_actions: Vec<_> = action_counts.into_iter().collect();
        sorted_actions.sort_by(|a, b| b.1.cmp(&a.1));
        habit_profile.most_common_actions = sorted_actions
            .into_iter()
            .take(5)
            .map(|(name, count)| HabitAction { name, count })
            .collect();

        let error_patterns = self.detect_anti_patterns(analyses);
        habit_profile.error_prone_patterns = error_patterns
            .iter()
            .map(|e| e.pattern.clone())
            .collect();

        let mut efficiency_scores: Vec<f64> = analyses
            .iter()
            .filter_map(|a| a.problem_solving.as_ref().map(|ps| ps.efficiency_score))
            .collect();

        if !efficiency_scores.is_empty() {
            efficiency_scores.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
            let high_efficiency = efficiency_scores.iter().filter(|&&s| s > 0.8).count();
            
            if high_efficiency > analyses.len() / 2 {
                habit_profile.productivity_patterns.push(ProductivityPattern::ConsistentlyEfficient);
            }
        }

        let total_errors: usize = analyses.iter().map(|a| a.metrics.error_count).sum();
        if total_errors == 0 {
            habit_profile.productivity_patterns.push(ProductivityPattern::ErrorFree);
        }

        habit_profile
    }
}

#[derive(Debug, Clone)]
pub struct AntiPatternDetection {
    pub pattern: AntiPatternType,
    pub frequency: usize,
    pub severity: AntiPatternSeverity,
    pub suggested_alternative: String,
    pub examples: Vec<String>,
    pub time_wasted_estimate: Duration,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AntiPatternType {
    ExcessiveRetry,
    IgnoringErrors,
    SkippingTests,
    PoorDocumentation,
    NoVersionControl,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AntiPatternSeverity {
    Low,
    Medium,
    High,
    Critical,
}

#[derive(Debug, Clone)]
pub struct InefficientPattern {
    pub pattern: InefficientPatternType,
    pub affected_sessions: Vec<Uuid>,
    pub description: String,
    pub average_wait_ratio: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum InefficientPatternType {
    ExcessiveWaiting,
    OverToolUsage,
    RepetitiveActions,
    PoorResourceUtilization,
}

#[derive(Debug, Clone)]
pub struct UserHabitProfile {
    pub session_count: usize,
    pub average_session_duration: Duration,
    pub most_common_actions: Vec<HabitAction>,
    pub error_prone_patterns: Vec<AntiPatternType>,
    pub productivity_patterns: Vec<ProductivityPattern>,
    pub learning_opportunities: Vec<LearningOpportunity>,
}

#[derive(Debug, Clone)]
pub struct HabitAction {
    pub name: String,
    pub count: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ProductivityPattern {
    ConsistentlyEfficient,
    ErrorFree,
    RapidIteration,
    ComprehensiveTesting,
    GoodDocumentation,
}

#[derive(Debug, Clone)]
pub struct ErrorPattern {
    pub error_type: ErrorType,
    pub frequency: usize,
    pub common_context: String,
}