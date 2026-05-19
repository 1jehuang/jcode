use chrono::{DateTime, Utc, Duration};
use serde::{Serialize, Deserialize};
use std::collections::HashMap;
use std::path::PathBuf;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct IntelligenceConfig {
    pub enable_auto_analysis: bool,
    pub min_session_duration: Duration,
    pub max_analysis_time: Duration,
    pub insight_confidence_threshold: f64,
    pub learning_opportunity_threshold: f64,
}

impl Default for IntelligenceConfig {
    fn default() -> Self {
        Self {
            enable_auto_analysis: true,
            min_session_duration: Duration::seconds(30),
            max_analysis_time: Duration::seconds(10),
            insight_confidence_threshold: 0.6,
            learning_opportunity_threshold: 0.5,
        }
    }
}

pub struct SessionAnalyzer;

impl SessionAnalyzer {
    pub fn analyze(&self, session: &CompletedSession) -> SessionAnalysis {
        let metrics = self.extract_metrics(session);
        let problem_solving = self.analyze_problem_solving(session);
        let learning_opps = self.identify_learning_opportunities(session);
        let quality = self.compute_quality_score(&metrics, session);
        let recommendations = self.generate_recommendations(&metrics, &quality);

        SessionAnalysis {
            session_id: session.id,
            analyzed_at: Utc::now(),
            metrics,
            problem_solving,
            learning_opportunities: learning_opps,
            overall_quality_score: quality,
            recommendations,
        }
    }

    fn extract_metrics(&self, session: &CompletedSession) -> TechnicalMetrics {
        let total_duration = session.end_time - session.start_time;
        let mut tool_call_count: HashMap<String, usize> = HashMap::new();
        for msg in &session.messages {
            for tc in &msg.tool_calls {
                *tool_call_count.entry(tc.name.clone()).or_insert(0) += 1;
            }
        }
        let retry_count = session
            .commands_executed
            .iter()
            .filter(|c| c.was_retry)
            .count();
        let error_count = session.errors_encountered.len();
        let success_rate = if error_count == 0 {
            1.0
        } else {
            let resolved = session
                .errors_encountered
                .iter()
                .filter(|e| e.resolved)
                .count() as f64;
            resolved / error_count as f64
        };
        let active_coding_time = self.estimate_active_coding_time(session);
        let waiting_time = total_duration - active_coding_time;

        TechnicalMetrics {
            total_duration,
            active_coding_time,
            waiting_time,
            tool_call_count,
            token_usage: session.token_usage.clone(),
            files_modified: session.files_modified.clone(),
            commands_executed: session.commands_executed.clone(),
            errors_encountered: session.errors_encountered.clone(),
            error_count,
            retry_count,
            success_rate,
        }
    }

    fn estimate_active_coding_time(&self, session: &CompletedSession) -> Duration {
        if session.messages.is_empty() {
            return Duration::zero();
        }
        let first_msg = session.messages.first().unwrap().timestamp;
        let last_msg = session.messages.last().unwrap().timestamp;
        if last_msg > first_msg {
            last_msg - first_msg
        } else {
            Duration::zero()
        }
    }

    fn analyze_problem_solving(
        &self,
        session: &CompletedSession,
    ) -> Option<ProblemSolvingAnalysis> {
        if session.messages.len() < 2 || session.files_modified.is_empty() {
            return None;
        }
        let approach = self.classify_approach(session);
        let steps = self.extract_steps(session);
        let dead_ends = self.find_dead_ends(session);
        let breakthrough = self.identify_breakthrough(session, &steps);
        let solution = self.describe_solution(session);
        let efficiency = self.compute_efficiency_score(
            steps.len(),
            dead_ends.len(),
            &session.errors_encountered,
        );

        Some(ProblemSolvingAnalysis {
            problem_description: self.infer_problem_description(session),
            approach_taken: approach,
            steps,
            dead_ends,
            breakthrough_moment: breakthrough,
            final_solution: solution,
            efficiency_score: efficiency,
        })
    }

    fn classify_approach(&self, session: &CompletedSession) -> ApproachType {
        let edit_count: usize = session.files_modified.iter().map(|f| f.edit_count).sum();
        let error_count = session.errors_encountered.len();
        
        if session.messages.iter().any(|m| m.content.to_lowercase().contains("refactor")) {
            return ApproachType::Refactoring;
        }
        
        if session.messages.iter().any(|m| m.content.to_lowercase().contains("research") || 
            m.content.to_lowercase().contains("learn") || 
            m.content.to_lowercase().contains("what is")) {
            return ApproachType::ResearchBased;
        }
        
        if session.messages.iter().any(|m| m.content.to_lowercase().contains("help") || 
            m.content.to_lowercase().contains("please") || 
            m.content.to_lowercase().contains("can you")) {
            return ApproachType::AskForHelp;
        }
        
        if edit_count > 50 && session.files_modified.len() > 3 {
            return ApproachType::DivideAndConquer;
        }
        
        if error_count > edit_count / 2 {
            return ApproachType::TrialAndError;
        }
        
        ApproachType::Incremental
    }

    fn extract_steps(&self, session: &CompletedSession) -> Vec<SolvingStep> {
        session
            .messages
            .iter()
            .filter(|m| m.role == MessageRole::Assistant && !m.tool_calls.is_empty())
            .enumerate()
            .map(|(i, m)| SolvingStep {
                step_number: i + 1,
                description: format!(
                    "Executed {} tool call(s)",
                    m.tool_calls.len()
                ),
                duration: Duration::milliseconds(
                    m.tool_calls.iter().map(|t| t.duration_ms).sum::<u64>() as i64,
                ),
                tools_used: m.tool_calls.iter().map(|t| t.name.clone()).collect(),
                outcome: if m.tool_calls.iter().all(|t| t.success) {
                    StepOutcome::Success
                } else if m.tool_calls.iter().any(|t| t.success) {
                    StepOutcome::PartialProgress
                } else {
                    StepOutcome::DeadEnd {
                        reason: "All tool calls failed".to_string(),
                    }
                },
            })
            .collect()
    }

    fn find_dead_ends(&self, session: &CompletedSession) -> Vec<DeadEndPath> {
        session
            .errors_encountered
            .iter()
            .filter(|e| !e.resolved)
            .enumerate()
            .map(|(i, e)| DeadEndPath {
                attempt_number: i + 1,
                description: format!("{:?} error: {}", e.error_type, e.message),
                time_spent: e.resolution_time.unwrap_or(Duration::seconds(5)),
                why_failed: "Error not resolved within session".to_string(),
            })
            .collect()
    }

    fn identify_breakthrough(
        &self,
        session: &CompletedSession,
        steps: &[SolvingStep],
    ) -> Option<Breakthrough> {
        if steps.len() < 3 {
            return None;
        }
        
        for (i, step) in steps.iter().enumerate() {
            if i == 0 {
                continue;
            }
            
            let prev_step = &steps[i - 1];
            
            if matches!(prev_step.outcome, StepOutcome::DeadEnd { .. }) && 
               matches!(step.outcome, StepOutcome::Success) {
                let trigger = if session.messages[i].content.contains("realize") || 
                    session.messages[i].content.contains("understand") || 
                    session.messages[i].content.contains("figured out") {
                    BreakthroughTrigger::PatternRecognition
                } else if session.messages[i].content.contains("from") || 
                    session.messages[i].content.contains("based on") {
                    BreakthroughTrigger::NewInformation
                } else if session.messages[i].content.contains("thanks") || 
                    session.messages[i].content.contains("help") {
                    BreakthroughTrigger::ExternalHelp
                } else {
                    BreakthroughTrigger::Reframing
                };
                
                return Some(Breakthrough {
                    description: format!("Breakthrough after {} failed attempts", i),
                    trigger,
                    time_to_breakthrough: step.duration + prev_step.duration,
                });
            }
        }
        
        None
    }

    fn describe_solution(&self, session: &CompletedSession) -> SolutionDescription {
        SolutionDescription {
            what_changed: format!(
                "Modified {} file(s)",
                session.files_modified.len()
            ),
            files_affected: session
                .files_modified
                .iter()
                .map(|f| f.path.clone())
                .collect(),
            verification_method: VerificationMethod::ManualVerification,
        }
    }

    fn compute_efficiency_score(
        &self,
        steps: usize,
        dead_ends: usize,
        errors: &[ErrorRecord],
    ) -> f64 {
        let base = 1.0;
        let step_penalty = (steps as f64).min(20.0) * 0.02;
        let dead_end_penalty = (dead_ends as f64).min(5.0) * 0.1;
        let error_penalty = (errors.len() as f64).min(10.0) * 0.03;
        (base - step_penalty - dead_end_penalty - error_penalty).max(0.0).min(1.0)
    }

    fn infer_problem_description(&self, _session: &CompletedSession) -> String {
        "Code modification task".to_string()
    }

    fn identify_learning_opportunities(
        &self,
        session: &CompletedSession,
    ) -> Vec<LearningOpportunity> {
        let mut opportunities = Vec::new();

        for error in &session.errors_encountered {
            match error.error_type {
                ErrorType::Compilation => {
                    opportunities.push(LearningOpportunity {
                        topic: LearningTopic::DebuggingTechnique {
                            technique: "Compilation error resolution".to_string(),
                        },
                        current_proficiency: ProficiencyLevel::Beginner,
                        target_proficiency: ProficiencyLevel::Intermediate,
                        recommended_resources: vec![ResourceRef {
                            title: "Rust Compiler Error Index".to_string(),
                            url: Some("https://doc.rust-lang.org/error-index.html".to_string()),
                            resource_type: ResourceType::Documentation,
                            difficulty: ProficiencyLevel::Beginner,
                            estimated_read_time_minutes: 15,
                        }],
                        practice_exercise: Some(
                            "Intentionally introduce and fix compilation errors in a test crate"
                                .to_string(),
                        ),
                        related_sessions: vec![session.id],
                        urgency: UrgencyLevel::Medium,
                        estimated_effort_hours: 2.0,
                    });
                }
                ErrorType::Logic => {
                    opportunities.push(LearningOpportunity {
                        topic: LearningTopic::BestPractice {
                            area: "Unit testing logic bugs".to_string(),
                        },
                        current_proficiency: ProficiencyLevel::Intermediate,
                        target_proficiency: ProficiencyLevel::Advanced,
                        recommended_resources: vec![ResourceRef {
                            title: "Test-Driven Development by Example".to_string(),
                            url: None,
                            resource_type: ResourceType::Book,
                            difficulty: ProficiencyLevel::Intermediate,
                            estimated_read_time_minutes: 300,
                        }],
                        practice_exercise: Some(
                            "Write failing tests before implementing features".to_string(),
                        ),
                        related_sessions: vec![session.id],
                        urgency: UrgencyLevel::Low,
                        estimated_effort_hours: 8.0,
                    });
                }
                _ => {}
            }
        }

        opportunities
    }

    fn compute_quality_score(
        &self,
        metrics: &TechnicalMetrics,
        session: &CompletedSession,
    ) -> QualityScore {
        let efficiency = (metrics.success_rate * 0.6
            + (1.0 - (metrics.retry_count as f64 / (metrics.commands_executed.len().max(1)) as f64)).min(1.0) * 0.4)
            as f32;

        let correctness = (if metrics.error_count == 0 { 1.0 } else { 0.7 }) as f32;

        let best_practices = (if session.files_modified.is_empty() {
            1.0
        } else {
            let has_tests = session
                .commands_executed
                .iter()
                .any(|c| matches!(c.category, CommandCategory::Test));
            if has_tests { 0.9 } else { 0.6 }
        }) as f32;

        let documentation = (if session
            .messages
            .iter()
            .any(|m| m.content.contains("///") || m.content.contains("// "))
        {
            0.85
        } else {
            0.5
        }) as f32;

        let overall = (efficiency as f64 * 0.3
            + correctness as f64 * 0.3
            + best_practices as f64 * 0.25
            + documentation as f64 * 0.15) as f32;

        QualityScore {
            overall,
            efficiency,
            correctness,
            best_practices,
            documentation,
        }
    }

    fn generate_recommendations(
        &self,
        metrics: &TechnicalMetrics,
        quality: &QualityScore,
    ) -> Vec<Recommendation> {
        let mut recs = Vec::new();

        if quality.efficiency < 0.7 {
            recs.push(Recommendation {
                category: RecommendationCategory::Performance,
                title: "Reduce retry cycles".to_string(),
                description: format!(
                    "{} retries detected. Consider validating inputs before execution.",
                    metrics.retry_count
                ),
                priority: Priority::Medium,
                effort: EffortLevel::Small,
                impact: ImpactLevel::Medium,
            });
        }

        if quality.correctness < 0.8 {
            recs.push(Recommendation {
                category: RecommendationCategory::Reliability,
                title: "Increase test coverage".to_string(),
                description: "Add unit tests to catch errors earlier.".to_string(),
                priority: Priority::High,
                effort: EffortLevel::Medium,
                impact: ImpactLevel::High,
            });
        }

        recs
    }
}

#[derive(Debug, Clone)]
pub struct CompletedSession {
    pub id: Uuid,
    pub project_path: PathBuf,
    pub start_time: DateTime<Utc>,
    pub end_time: DateTime<Utc>,
    pub messages: Vec<AnalyzedMessage>,
    pub files_modified: Vec<FileModificationRecord>,
    pub commands_executed: Vec<CommandExecutionRecord>,
    pub errors_encountered: Vec<ErrorRecord>,
    pub token_usage: TokenUsageSummary,
}

impl Default for CompletedSession {
    fn default() -> Self {
        Self {
            id: Uuid::new_v4(),
            project_path: PathBuf::from("/tmp/project"),
            start_time: Utc::now(),
            end_time: Utc::now() + chrono::Duration::minutes(5),
            messages: Vec::new(),
            files_modified: Vec::new(),
            commands_executed: Vec::new(),
            errors_encountered: Vec::new(),
            token_usage: TokenUsageSummary::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalyzedMessage {
    pub role: MessageRole,
    pub content: String,
    pub timestamp: DateTime<Utc>,
    pub tool_calls: Vec<ToolCallSummary>,
    pub token_count: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum MessageRole {
    User,
    Assistant,
    System,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallSummary {
    pub name: String,
    pub input_preview: String,
    pub success: bool,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileModificationRecord {
    pub path: PathBuf,
    pub edit_count: usize,
    pub lines_added: usize,
    pub lines_removed: usize,
    pub complexity_delta: ComplexityDelta,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ComplexityDelta {
    Increased(u32),
    Decreased(u32),
    Unchanged,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandExecutionRecord {
    pub command: String,
    pub exit_code: Option<i32>,
    pub duration_ms: u64,
    pub was_retry: bool,
    pub category: CommandCategory,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum CommandCategory {
    Git,
    Build,
    Test,
    Run,
    Docker,
    Npm,
    Cargo,
    Other,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorRecord {
    pub error_type: ErrorType,
    pub message: String,
    pub timestamp: DateTime<Utc>,
    pub resolved: bool,
    pub resolution_time: Option<Duration>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ErrorType {
    Compilation,
    Runtime,
    Network,
    Permission,
    Logic,
    Timeout,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenUsageSummary {
    pub total_input: u64,
    pub total_output: u64,
    pub cache_read: u64,
    pub estimated_cost_usd: Option<f64>,
}

impl Default for TokenUsageSummary {
    fn default() -> Self {
        Self {
            total_input: 0,
            total_output: 0,
            cache_read: 0,
            estimated_cost_usd: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TechnicalMetrics {
    pub total_duration: Duration,
    pub active_coding_time: Duration,
    pub waiting_time: Duration,
    pub tool_call_count: HashMap<String, usize>,
    pub token_usage: TokenUsageSummary,
    pub files_modified: Vec<FileModificationRecord>,
    pub commands_executed: Vec<CommandExecutionRecord>,
    pub errors_encountered: Vec<ErrorRecord>,
    pub error_count: usize,
    pub retry_count: usize,
    pub success_rate: f64,
}

impl TechnicalMetrics {
    pub fn errors_encountered_in_session(&self) -> &[ErrorRecord] {
        &self.errors_encountered
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProblemSolvingAnalysis {
    pub problem_description: String,
    pub approach_taken: ApproachType,
    pub steps: Vec<SolvingStep>,
    pub dead_ends: Vec<DeadEndPath>,
    pub breakthrough_moment: Option<Breakthrough>,
    pub final_solution: SolutionDescription,
    pub efficiency_score: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ApproachType {
    Incremental,
    Refactoring,
    ResearchBased,
    AskForHelp,
    DivideAndConquer,
    TrialAndError,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SolvingStep {
    pub step_number: usize,
    pub description: String,
    pub duration: Duration,
    pub tools_used: Vec<String>,
    pub outcome: StepOutcome,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum StepOutcome {
    Success,
    PartialProgress,
    DeadEnd { reason: String },
    InsightGained { insight: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeadEndPath {
    pub attempt_number: usize,
    pub description: String,
    pub time_spent: Duration,
    pub why_failed: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Breakthrough {
    pub description: String,
    pub trigger: BreakthroughTrigger,
    pub time_to_breakthrough: Duration,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum BreakthroughTrigger {
    NewInformation,
    PatternRecognition,
    ExternalHelp,
    Reframing,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SolutionDescription {
    pub what_changed: String,
    pub files_affected: Vec<PathBuf>,
    pub verification_method: VerificationMethod,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum VerificationMethod {
    TestsPassed,
    ManualVerification,
    CompilerSuccess,
    RuntimeSuccess,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LearningOpportunity {
    pub topic: LearningTopic,
    pub current_proficiency: ProficiencyLevel,
    pub target_proficiency: ProficiencyLevel,
    pub recommended_resources: Vec<ResourceRef>,
    pub practice_exercise: Option<String>,
    pub related_sessions: Vec<Uuid>,
    pub urgency: UrgencyLevel,
    pub estimated_effort_hours: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum LearningTopic {
    LanguageFeature { lang: String, feature: String },
    ToolMastery { tool: String, aspect: ToolAspect },
    DomainKnowledge { domain: String, concept: String },
    DebuggingTechnique { technique: String },
    ArchitecturePattern { pattern: String },
    BestPractice { area: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ToolAspect {
    BasicUsage,
    AdvancedFeatures,
    Configuration,
    Troubleshooting,
    Integration,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, PartialOrd, Ord, Eq)]
pub enum ProficiencyLevel {
    Novice,
    Beginner,
    Intermediate,
    Advanced,
    Expert,
    Master,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceRef {
    pub title: String,
    pub url: Option<String>,
    pub resource_type: ResourceType,
    pub difficulty: ProficiencyLevel,
    pub estimated_read_time_minutes: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ResourceType {
    Documentation,
    Tutorial,
    Video,
    Book,
    Course,
    BlogPost,
    StackOverflow,
    RFC,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum UrgencyLevel {
    Low,
    Medium,
    High,
    Critical,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionAnalysis {
    pub session_id: Uuid,
    pub analyzed_at: DateTime<Utc>,
    pub metrics: TechnicalMetrics,
    pub problem_solving: Option<ProblemSolvingAnalysis>,
    pub learning_opportunities: Vec<LearningOpportunity>,
    pub overall_quality_score: QualityScore,
    pub recommendations: Vec<Recommendation>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QualityScore {
    pub overall: f32,
    pub efficiency: f32,
    pub correctness: f32,
    pub best_practices: f32,
    pub documentation: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Recommendation {
    pub category: RecommendationCategory,
    pub title: String,
    pub description: String,
    pub priority: Priority,
    pub effort: EffortLevel,
    pub impact: ImpactLevel,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum RecommendationCategory {
    Performance,
    Reliability,
    Maintainability,
    Security,
    Learning,
    Tooling,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum Priority {
    Low,
    Medium,
    High,
    Critical,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum EffortLevel {
    QuickWin,
    Small,
    Medium,
    Large,
    Major,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ImpactLevel {
    Low,
    Medium,
    High,
    Transformative,
}