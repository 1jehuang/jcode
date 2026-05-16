use chrono::{DateTime, Utc, Duration};
use serde::{Serialize, Deserialize};
use std::collections::HashMap;
use std::path::PathBuf;
use uuid::Uuid;

pub struct SessionIntelligenceEngine {
    analyzer: SessionAnalyzer,
    summarizer: SessionSummarizer,
    insight_generator: InsightGenerator,
    pattern_detector: PatternDetector,
    config: IntelligenceConfig,
}

impl SessionIntelligenceEngine {
    pub fn new(config: IntelligenceConfig) -> Self {
        Self {
            analyzer: SessionAnalyzer,
            summarizer: SessionSummarizer,
            insight_generator: InsightGenerator,
            pattern_detector: PatternDetector,
            config,
        }
    }

    pub fn analyze_session(&self, session: &CompletedSession) -> Option<SessionAnalysis> {
        if session.end_time < session.start_time {
            return None;
        }
        let duration = session.end_time - session.start_time;
        if duration < self.config.min_session_duration {
            return None;
        }
        Some(self.analyzer.analyze(session))
    }

    pub async fn generate_summary(&self, analysis: &SessionAnalysis) -> SessionSummary {
        self.summarizer.generate_summary(analysis).await
    }

    pub async fn generate_report(&self, analysis: &SessionAnalysis) -> SessionReport {
        self.summarizer.generate_report(analysis).await
    }

    pub fn generate_insights(&self, analysis: &SessionAnalysis) -> Vec<Insight> {
        self.insight_generator.generate_insights(analysis)
    }

    pub fn detect_anti_patterns(&self, analyses: &[SessionAnalysis]) -> Vec<AntiPatternDetection> {
        self.pattern_detector.detect_anti_patterns(analyses)
    }

    pub fn analyze_habits(&self, analyses: &[SessionAnalysis]) -> UserHabitProfile {
        self.pattern_detector.detect_habits(analyses)
    }
}

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

    fn classify_approach(&self, _session: &CompletedSession) -> ApproachType {
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
        _session: &CompletedSession,
        _steps: &[SolvingStep],
    ) -> Option<Breakthrough> {
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
    pub error_count: usize,
    pub retry_count: usize,
    pub success_rate: f64,
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

pub struct SessionSummarizer;

impl SessionSummarizer {
    pub async fn generate_summary(&self, analysis: &SessionAnalysis) -> SessionSummary {
        let one_liner = format!(
            "Session {}: modified {} files with {:.0}% success rate over {:?}",
            analysis.session_id,
            analysis.metrics.files_modified.len(),
            analysis.metrics.success_rate * 100.0,
            analysis.metrics.total_duration,
        );

        let what_was_done = if !analysis.metrics.files_modified.is_empty() {
            let files: Vec<String> = analysis
                .metrics
                .files_modified
                .iter()
                .map(|f| {
                    f.path
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_default()
                })
                .collect();
            format!("Modified: {}", files.join(", "))
        } else {
            "No file modifications recorded".to_string()
        };

        let how_it_was_done = if let Some(ref ps) = analysis.problem_solving {
            format!("Approach: {:?}", ps.approach_taken)
        } else {
            "Direct implementation".to_string()
        };

        let key_challenges = analysis
            .metrics
            .errors_encountered_in_session()
            .iter()
            .map(|e| format!("{:?}: {}", e.error_type, e.message))
            .collect();

        let lessons_learned = analysis
            .learning_opportunities
            .iter()
            .take(3)
            .map(|lo| format!("{:?}", lo.topic))
            .collect();

        let next_steps = analysis
            .recommendations
            .iter()
            .take(3)
            .map(|r| r.title.clone())
            .collect();

        let mut tags = Vec::new();
        if analysis.metrics.error_count > 0 {
            tags.push("error-recovery".to_string());
        }
        if analysis.metrics.retry_count > 0 {
            tags.push("retries".to_string());
        }
        tags.push(format!("{:.0}pct-success", analysis.metrics.success_rate * 100.0));

        SessionSummary {
            one_liner,
            what_was_done,
            how_it_was_done,
            key_challenges,
            lessons_learned,
            next_steps,
            tags,
        }
    }

    pub async fn generate_report(&self, analysis: &SessionAnalysis) -> SessionReport {
        let summary = self.generate_summary(analysis).await;

        let exec_summary = format!(
            "Session completed in {:?} with {:.1}% overall quality score. \
             {} files modified, {} errors encountered, {:.0}% success rate.",
            analysis.metrics.total_duration,
            analysis.overall_quality_score.overall * 100.0,
            analysis.metrics.files_modified.len(),
            analysis.metrics.error_count,
            analysis.metrics.success_rate * 100.0,
        );

        let metrics_summary = format!(
            "Duration: {:?}\nActive coding: {:?}\nWaiting: {:?}\n\
             Tool calls: {}\nErrors: {}\nRetries: {}\nSuccess rate: {:.0}%",
            analysis.metrics.total_duration,
            analysis.metrics.active_coding_time,
            analysis.metrics.waiting_time,
            analysis.metrics.tool_call_count.values().sum::<usize>(),
            analysis.metrics.error_count,
            analysis.metrics.retry_count,
            analysis.metrics.success_rate * 100.0,
        );

        let file_changes_detail = if analysis.metrics.files_modified.is_empty() {
            "No file changes recorded.".to_string()
        } else {
            analysis
                .metrics
                .files_modified
                .iter()
                .map(|f| {
                    format!(
                        "{}: +{}/-{} edits:{} delta:{:?}",
                        f.path.display(),
                        f.lines_added,
                        f.lines_removed,
                        f.edit_count,
                        f.complexity_delta,
                    )
                })
                .collect::<Vec<_>>()
                .join("\n")
        };

        let command_timeline = if analysis.metrics.commands_executed.is_empty() {
            "No commands executed.".to_string()
        } else {
            analysis
                .metrics
                .commands_executed
                .iter()
                .map(|c| {
                    format!(
                        "[{:?}] {} (exit={:?}, retry={}, cat={:?})",
                        Duration::milliseconds(c.duration_ms as i64),
                        c.command,
                        c.exit_code,
                        c.was_retry,
                        c.category,
                    )
                })
                .collect::<Vec<_>>()
                .join("\n")
        };

        let error_analysis = if analysis.metrics.error_count == 0 {
            "No errors encountered.".to_string()
        } else {
            format!(
                "{} error(s): {} resolved ({:.0}%)",
                analysis.metrics.error_count,
                analysis
                    .metrics
                    .errors_encountered_in_session()
                    .iter()
                    .filter(|e| e.resolved)
                    .count(),
                analysis.metrics.success_rate * 100.0,
            )
        };

        let ps_insights = if let Some(ref ps) = analysis.problem_solving {
            vec![
                format!("Approach: {:?}", ps.approach_taken),
                format!("Efficiency score: {:.2}", ps.efficiency_score),
                format!("Steps taken: {}", ps.steps.len()),
                format!("Dead ends: {}", ps.dead_ends.len()),
            ]
        } else {
            Vec::new()
        };

        let eff_insights = vec![
            format!(
                "Active/total ratio: {:.0}%",
                if analysis.metrics.total_duration.num_milliseconds() > 0 {
                    (analysis.metrics.active_coding_time.num_milliseconds() as f64
                        / analysis.metrics.total_duration.num_milliseconds() as f64)
                        * 100.0
                } else {
                    0.0
                }
            ),
            format!("Retry rate: {}/{}", analysis.metrics.retry_count, analysis.metrics.commands_executed.len().max(1)),
        ];

        let qual_insights = vec![
            format!(
                "Overall quality: {:.1}%",
                analysis.overall_quality_score.overall * 100.0
            ),
            format!(
                "Efficiency: {:.1}%",
                analysis.overall_quality_score.efficiency * 100.0
            ),
            format!(
                "Correctness: {:.1}%",
                analysis.overall_quality_score.correctness * 100.0
            ),
        ];

        let learn_insights = analysis
            .learning_opportunities
            .iter()
            .map(|lo| {
                format!(
                    "{:?} -> {:?} (urgency: {:?}, effort: {:.1}h)",
                    lo.topic, lo.target_proficiency, lo.urgency, lo.estimated_effort_hours
                )
            })
            .collect();

        let raw_metrics = serde_json::json!({
            "duration_ms": analysis.metrics.total_duration.num_milliseconds(),
            "active_ms": analysis.metrics.active_coding_time.num_milliseconds(),
            "error_count": analysis.metrics.error_count,
            "retry_count": analysis.metrics.retry_count,
            "success_rate": analysis.metrics.success_rate,
            "files_modified": analysis.metrics.files_modified.len(),
            "quality_overall": analysis.overall_quality_score.overall,
        });

        let timeline_csv = format!(
            "timestamp,duration_ms,action\n{}",
            analysis
                .metrics
                .commands_executed
                .iter()
                .map(|c| format!(
                    "{},{},{}",
                    Utc::now().format("%Y-%m-%dT%H:%M:%SZ"),
                    c.duration_ms,
                    c.command.replace(',', ";"),
                ))
                .collect::<Vec<_>>()
                .join("\n"),
        );

        SessionReport {
            summary,
            executive_summary: exec_summary,
            technical_details: TechnicalReportSection {
                metrics_summary,
                file_changes_detail,
                command_timeline,
                error_analysis,
            },
            insights_section: InsightsSection {
                problem_solving_insights: ps_insights,
                efficiency_insights: eff_insights,
                quality_insights: qual_insights,
                learning_insights: learn_insights,
            },
            appendices: ReportAppendices {
                raw_metrics,
                tool_usage_breakdown: analysis.metrics.tool_call_count.clone(),
                timeline_csv,
            },
        }
    }

    pub async fn generate_thread(&self, analysis: &SessionAnalysis) -> Vec<String> {
        let mut thread = Vec::new();

        thread.push(format!(
            "🧵 Session Analysis Thread — {:?}",
            analysis.metrics.total_duration
        ));

        thread.push(format!(
            "1/{} Modified {} file(s) | {:.0}% success rate | {} errors",
            4 + analysis.learning_opportunities.len().min(2),
            analysis.metrics.files_modified.len(),
            analysis.metrics.success_rate * 100.0,
            analysis.metrics.error_count,
        ));

        thread.push(format!(
            "2/{} Approach: {:?} | Efficiency: {:.0}%",
            4 + analysis.learning_opportunities.len().min(2),
            analysis
                .problem_solving
                .as_ref()
                .map(|ps| format!("{:?}", ps.approach_taken))
                .unwrap_or_else(|| "N/A".to_string()),
            analysis
                .problem_solving
                .as_ref()
                .map(|ps| ps.efficiency_score * 100.0)
                .unwrap_or(0.0),
        ));

        thread.push(format!(
            "3/{} Quality Score: {:.0} overall | ⚡ {:.0} eff | ✅ {:.0} correct | 📚 {:.0} bp | 📝 {:.0} docs",
            4 + analysis.learning_opportunities.len().min(2),
            analysis.overall_quality_score.overall * 100.0,
            analysis.overall_quality_score.efficiency * 100.0,
            analysis.overall_quality_score.correctness * 100.0,
            analysis.overall_quality_score.best_practices * 100.0,
            analysis.overall_quality_score.documentation * 100.0,
        ));

        for (i, rec) in analysis.recommendations.iter().enumerate().take(2) {
            thread.push(format!(
                "{}/{} 📌 [{}] {} — {}",
                i + 4,
                4 + analysis.learning_opportunities.len().min(2),
                match rec.priority {
                    Priority::Critical => "🔴",
                    Priority::High => "🟠",
                    Priority::Medium => "🟡",
                    Priority::Low => "🟢",
                },
                rec.title,
                rec.description.chars().take(80).collect::<String>(),
            ));
        }

        thread
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSummary {
    pub one_liner: String,
    pub what_was_done: String,
    pub how_it_was_done: String,
    pub key_challenges: Vec<String>,
    pub lessons_learned: Vec<String>,
    pub next_steps: Vec<String>,
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionReport {
    pub summary: SessionSummary,
    pub executive_summary: String,
    pub technical_details: TechnicalReportSection,
    pub insights_section: InsightsSection,
    pub appendices: ReportAppendices,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TechnicalReportSection {
    pub metrics_summary: String,
    pub file_changes_detail: String,
    pub command_timeline: String,
    pub error_analysis: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InsightsSection {
    pub problem_solving_insights: Vec<String>,
    pub efficiency_insights: Vec<String>,
    pub quality_insights: Vec<String>,
    pub learning_insights: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReportAppendices {
    pub raw_metrics: serde_json::Value,
    pub tool_usage_breakdown: HashMap<String, usize>,
    pub timeline_csv: String,
}

pub struct InsightGenerator;

impl InsightGenerator {
    pub fn generate_insights(&self, analysis: &SessionAnalysis) -> Vec<Insight> {
        let mut insights = Vec::new();

        if analysis.metrics.retry_count > 3 {
            insights.push(Insight {
                category: InsightCategory::Efficiency,
                title: "High retry frequency detected".to_string(),
                description: format!(
                    "{} retries observed in a single session suggests \
                     possible issues with pre-validation or environment stability.",
                    analysis.metrics.retry_count
                ),
                evidence: vec![EvidenceItem {
                    location: EvidenceLocation::ConversationTurn { turn: 0 },
                    content: format!("Retry count: {}", analysis.metrics.retry_count),
                    significance: Significance::Key,
                }],
                actionable_advice: Some(ActionableAdvice {
                    short_tip: "Add input validation before executing commands".to_string(),
                    detailed_guidance: Some(
                        "Consider implementing a pre-flight check that validates \
                         inputs, checks dependencies, and verifies environment state \
                         before running build or test commands."
                            .to_string(),
                    ),
                    relevant_resource: None,
                    expected_improvement: ImprovementEstimate {
                        metric: "retry_count".to_string(),
                        current_value: analysis.metrics.retry_count as f64,
                        expected_value: (analysis.metrics.retry_count as f64 * 0.3).max(1.0),
                        unit: "count".to_string(),
                    },
                }),
                confidence: 0.82,
                severity: InsightSeverity::Warning,
            });
        }

        if analysis.metrics.error_count == 0 && analysis.metrics.files_modified.len() > 3 {
            insights.push(Insight {
                category: InsightCategory::BestPractice,
                title: "Clean execution with multiple changes".to_string(),
                description: format!(
                    "Successfully modified {} files without any errors. \
                     This indicates good planning or familiarity with the codebase.",
                    analysis.metrics.files_modified.len()
                ),
                evidence: vec![
                    EvidenceItem {
                        location: EvidenceLocation::FileChange {
                            path: PathBuf::from("session_files"),
                        },
                        content: format!(
                            "{} files changed cleanly",
                            analysis.metrics.files_modified.len()
                        ),
                        significance: Significance::Decisive,
                    },
                ],
                actionable_advice: None,
                confidence: 0.91,
                severity: InsightSeverity::Info,
            });
        }

        if analysis.overall_quality_score.documentation < 0.5 {
            insights.push(Insight {
                category: InsightCategory::WorkflowImprovement,
                title: "Low documentation coverage".to_string(),
                description: "This session shows limited inline documentation. \
                    Adding comments and doc strings improves maintainability."
                    .to_string(),
                evidence: vec![EvidenceItem {
                    location: EvidenceLocation::ConversationTurn { turn: 0 },
                    content: format!(
                        "Documentation score: {:.2}",
                        analysis.overall_quality_score.documentation
                    ),
                    significance: Significance::Supporting,
                }],
                actionable_advice: Some(ActionableAdvice {
                    short_tip: "Add doc comments to public functions".to_string(),
                    detailed_guidance: None,
                    relevant_resource: Some(ResourceRef {
                        title: "Rust Documentation Guidelines".to_string(),
                        url: Some(
                            "https://doc.rust-lang.org/book/ch14-02-publishing-to-crates-io.html"
                                .to_string(),
                        ),
                        resource_type: ResourceType::Documentation,
                        difficulty: ProficiencyLevel::Beginner,
                        estimated_read_time_minutes: 10,
                    }),
                    expected_improvement: ImprovementEstimate {
                        metric: "documentation_score".to_string(),
                        current_value: analysis.overall_quality_score.documentation as f64,
                        expected_value: 0.8,
                        unit: "score".to_string(),
                    },
                }),
                confidence: 0.73,
                severity: InsightSeverity::Suggestion,
            });
        }

        for lo in &analysis.learning_opportunities {
            if matches!(lo.urgency, UrgencyLevel::High | UrgencyLevel::Critical) {
                insights.push(Insight {
                    category: InsightCategory::KnowledgeGap,
                    title: format!("Knowledge gap: {:?}", lo.topic),
                    description: format!(
                        "Current proficiency ({:?}) below target ({:?}) for this area.",
                        lo.current_proficiency, lo.target_proficiency
                    ),
                    evidence: vec![EvidenceItem {
                        location: EvidenceLocation::ConversationTurn { turn: 0 },
                        content: format!("{:?}", lo.topic),
                        significance: Significance::Key,
                    }],
                    actionable_advice: Some(ActionableAdvice {
                        short_tip: format!(
                            "Study: {}",
                            lo.recommended_resources
                                .first()
                                .map(|r| r.title.as_str())
                                .unwrap_or("See recommended resources")
                        ),
                        detailed_guidance: lo.practice_exercise.clone(),
                        relevant_resource: lo.recommended_resources.first().cloned(),
                        expected_improvement: ImprovementEstimate {
                            metric: format!("{:?}", lo.topic),
                            current_value: lo.current_proficiency as u8 as f64,
                            expected_value: lo.target_proficiency as u8 as f64,
                            unit: "proficiency_level".to_string(),
                        },
                    }),
                    confidence: 0.75,
                    severity: if lo.urgency == UrgencyLevel::Critical {
                        InsightSeverity::Warning
                    } else {
                        InsightSeverity::Suggestion
                    },
                });
            }
        }

        insights
    }

    pub fn identify_patterns(&self, analyses: &[SessionAnalysis]) -> Vec<PatternInsight> {
        let mut patterns = Vec::new();

        let avg_success: f64 = analyses
            .iter()
            .map(|a| a.metrics.success_rate)
            .sum::<f64>()
            / analyses.len().max(1) as f64;

        if avg_success < 0.7 {
            patterns.push(PatternInsight {
                pattern_name: "Consistently low success rate".to_string(),
                description: format!(
                    "Average success rate across {} sessions is {:.0}%, \
                     suggesting systemic issues.",
                    analyses.len(),
                    avg_success * 100.0
                ),
                affected_sessions: analyses.iter().map(|a| a.session_id).collect(),
                recommendation:
                    "Review common failure modes and add pre-flight validation.".to_string(),
                potential_improvement: "Target 90%+ success rate through better input validation"
                    .to_string(),
            });
        }

        let high_retry_sessions: Vec<Uuid> = analyses
            .iter()
            .filter(|a| a.metrics.retry_count > 3)
            .map(|a| a.session_id)
            .collect();

        if high_retry_sessions.len() >= analyses.len() / 2 {
            patterns.push(PatternInsight {
                pattern_name: "Chronic retry behavior".to_string(),
                description: format!(
                    "{} of {} sessions show excessive retries (>3).",
                    high_retry_sessions.len(),
                    analyses.len()
                ),
                affected_sessions: high_retry_sessions,
                recommendation: "Investigate root causes of repeated failures."
                    .to_string(),
                potential_improvement: "Reduce retries by 60%+ through proactive validation"
                    .to_string(),
            });
        }

        patterns
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Insight {
    pub category: InsightCategory,
    pub title: String,
    pub description: String,
    pub evidence: Vec<EvidenceItem>,
    pub actionable_advice: Option<ActionableAdvice>,
    pub confidence: f64,
    pub severity: InsightSeverity,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum InsightCategory {
    Efficiency,
    Quality,
    KnowledgeGap,
    WorkflowImprovement,
    AntiPattern,
    BestPractice,
    Security,
    Performance,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum InsightSeverity {
    Info,
    Suggestion,
    Warning,
    Critical,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvidenceItem {
    pub location: EvidenceLocation,
    pub content: String,
    pub significance: Significance,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum EvidenceLocation {
    ToolCall { index: usize },
    ConversationTurn { turn: usize },
    FileChange { path: PathBuf },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum Significance {
    Supporting,
    Key,
    Decisive,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionableAdvice {
    pub short_tip: String,
    pub detailed_guidance: Option<String>,
    pub relevant_resource: Option<ResourceRef>,
    pub expected_improvement: ImprovementEstimate,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImprovementEstimate {
    pub metric: String,
    pub current_value: f64,
    pub expected_value: f64,
    pub unit: String,
}

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
                a.metrics.total_duration.num_milliseconds() > 0
                    && a.metrics.waiting_time.num_milliseconds() as f64
                        / a.metrics.total_duration.num_milliseconds() as f64
                        > 0.5
            })
            .collect();

        if !long_wait_sessions.is_empty() {
            patterns.push(InefficientPattern {
                name: "High idle/wait ratio".to_string(),
                description: format!(
                    "{} sessions spend more than 50% of time waiting/idle",
                    long_wait_sessions.len()
                ),
                frequency: long_wait_sessions.len(),
                time_impact_per_occurrence: Duration::minutes(5),
                suggested_optimization: "Parallelize independent operations where possible"
                    .to_string(),
            });
        }

        patterns
    }

    pub fn detect_habits(&self, analyses: &[SessionAnalysis]) -> UserHabitProfile {
        let preferred_approach = if analyses.is_empty() {
            ApproachType::Incremental
        } else {
            analyses[0]
                .problem_solving
                .as_ref()
                .map(|ps| ps.approach_taken.clone())
                .unwrap_or(ApproachType::Incremental)
        };

        let mut all_tools: HashMap<String, usize> = HashMap::new();
        for a in analyses {
            for (tool, count) in &a.metrics.tool_call_count {
                *all_tools.entry(tool.clone()).or_insert(0) += count;
            }
        }
        let mut common_tool_sequence: Vec<String> =
            all_tools.into_iter().collect::<Vec<_>>();
        common_tool_sequence.sort_by_key(|&(_, c)| std::cmp::Reverse(c));
        common_tool_sequence.truncate(5);
        common_tool_sequence = common_tool_sequence.into_iter().map(|(t, _)| t).collect();

        let avg_dur = if analyses.is_empty() {
            Duration::zero()
        } else {
            let total_ms: i64 = analyses
                .iter()
                .map(|a| a.metrics.total_duration.num_milliseconds())
                .sum();
            Duration::milliseconds(total_ms / analyses.len() as i64)
        };

        let avg_errors = if analyses.is_empty() {
            0.0
        } else {
            analyses.iter().map(|a| a.metrics.error_count).sum::<usize>() as f64
                / analyses.len() as f64
        };

        let recovery_style = if avg_errors < 1.0 {
            ErrorRecoveryStyle::QuickFix
        } else if avg_errors < 3.0 {
            ErrorRecoveryStyle::SystematicDebugging
        } else {
            ErrorRecoveryStyle::TrialAndError
        };

        let learning_velocity = if analyses.len() < 2 {
            LearningVelocity::Unknown
        } else {
            let scores: Vec<f32> = analyses
                .iter()
                .map(|a| a.overall_quality_score.overall)
                .collect();
            let trend = scores[scores.len() - 1] - scores[0];
            if trend > 0.1 {
                LearningVelocity::Fast
            } else if trend > 0.01 {
                LearningVelocity::Moderate
            } else if trend < -0.05 {
                LearningVelocity::Slow
            } else {
                LearningVelocity::Stagnant
            }
        };

        UserHabitProfile {
            preferred_approach,
            common_tool_sequence,
            peak_productivity_hour: None,
            average_session_duration: avg_dur,
            error_recovery_style: recovery_style,
            learning_velocity,
        }
    }

    pub fn detect_skill_progression(
        &self,
        analyses: &[SessionAnalysis],
    ) -> SkillProgressionMap {
        let mut skills = HashMap::new();

        let quality_skill = SkillTrajectory {
            skill_name: "overall_code_quality".to_string(),
            history: analyses
                .iter()
                .enumerate()
                .map(|(i, a)| ProficiencyAtTime {
                    timestamp: a.analyzed_at,
                    level: if a.overall_quality_score.overall >= 0.9 {
                        ProficiencyLevel::Expert
                    } else if a.overall_quality_score.overall >= 0.75 {
                        ProficiencyLevel::Advanced
                    } else if a.overall_quality_score.overall >= 0.55 {
                        ProficiencyLevel::Intermediate
                    } else if a.overall_quality_score.overall >= 0.35 {
                        ProficiencyLevel::Beginner
                    } else {
                        ProficiencyLevel::Novice
                    },
                    confidence: 0.8,
                })
                .collect(),
            trend: if analyses.len() < 2 {
                TrendDirection::Unknown
            } else {
                let first = analyses.first().unwrap().overall_quality_score.overall;
                let last = analyses.last().unwrap().overall_quality_score.overall;
                if last > first + 0.05 {
                    TrendDirection::Improving
                } else if last < first - 0.05 {
                    TrendDirection::Declining
                } else {
                    TrendDirection::Stable
                }
            },
            projected_mastery_date: None,
        };
        skills.insert("overall_code_quality".to_string(), quality_skill);

        let efficiency_skill = SkillTrajectory {
            skill_name: "efficiency".to_string(),
            history: analyses
                .iter()
                .map(|a| ProficiencyAtTime {
                    timestamp: a.analyzed_at,
                    level: if a.overall_quality_score.efficiency >= 0.85 {
                        ProficiencyLevel::Advanced
                    } else if a.overall_quality_score.efficiency >= 0.65 {
                        ProficiencyLevel::Intermediate
                    } else {
                        ProficiencyLevel::Beginner
                    },
                    confidence: 0.75,
                })
                .collect(),
            trend: TrendDirection::Stable,
            projected_mastery_date: None,
        };
        skills.insert("efficiency".to_string(), efficiency_skill);

        SkillProgressionMap { skills }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AntiPatternDetection {
    pub pattern: AntiPatternType,
    pub frequency: usize,
    pub severity: AntiPatternSeverity,
    pub suggested_alternative: String,
    pub examples: Vec<String>,
    pub time_wasted_estimate: Duration,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum AntiPatternType {
    ExcessiveRetry,
    IgnoringErrors,
    PrematureOptimization,
    CopyPasteWithoutUnderstanding,
    OverRelianceOnAI,
    MissingVerification,
    NoErrorHandling,
    HardcodedValues,
    MonolithicEdits,
    SkippingTests,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum AntiPatternSeverity {
    Low,
    Medium,
    High,
    Critical,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InefficientPattern {
    pub name: String,
    pub description: String,
    pub frequency: usize,
    pub time_impact_per_occurrence: Duration,
    pub suggested_optimization: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserHabitProfile {
    pub preferred_approach: ApproachType,
    pub common_tool_sequence: Vec<String>,
    pub peak_productivity_hour: Option<u8>,
    pub average_session_duration: Duration,
    pub error_recovery_style: ErrorRecoveryStyle,
    pub learning_velocity: LearningVelocity,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ErrorRecoveryStyle {
    QuickFix,
    SystematicDebugging,
    AskForHelp,
    TrialAndError,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum LearningVelocity {
    Fast,
    Moderate,
    Slow,
    Stagnant,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillProgressionMap {
    pub skills: HashMap<String, SkillTrajectory>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillTrajectory {
    pub skill_name: String,
    pub history: Vec<ProficiencyAtTime>,
    pub trend: TrendDirection,
    pub projected_mastery_date: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProficiencyAtTime {
    pub timestamp: DateTime<Utc>,
    pub level: ProficiencyLevel,
    pub confidence: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum TrendDirection {
    Improving,
    Stable,
    Declining,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PatternInsight {
    pub pattern_name: String,
    pub description: String,
    pub affected_sessions: Vec<Uuid>,
    pub recommendation: String,
    pub potential_improvement: String,
}

impl TechnicalMetrics {
    pub fn errors_encountered_in_session(&self) -> Vec<ErrorRecord> {
        Vec::new()
    }
}

fn make_test_session(messages: Vec<AnalyzedMessage>) -> CompletedSession {
    CompletedSession {
        id: Uuid::new_v4(),
        project_path: PathBuf::from("/test/project"),
        start_time: Utc::now(),
        end_time: Utc::now() + chrono::Duration::minutes(5),
        messages,
        files_modified: vec![FileModificationRecord {
            path: PathBuf::from("src/main.rs"),
            edit_count: 3,
            lines_added: 15,
            lines_removed: 5,
            complexity_delta: ComplexityDelta::Increased(2),
        }],
        commands_executed: vec![
            CommandExecutionRecord {
                command: "cargo check".to_string(),
                exit_code: Some(0),
                duration_ms: 1200,
                was_retry: false,
                category: CommandCategory::Build,
            },
        ],
        errors_encountered: Vec::new(),
        token_usage: TokenUsageSummary {
            total_input: 5000,
            total_output: 2000,
            cache_read: 1000,
            estimated_cost_usd: Some(0.03),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_message(role: MessageRole, tool_calls: Vec<ToolCallSummary>) -> AnalyzedMessage {
        AnalyzedMessage {
            role,
            content: "Sample message content".to_string(),
            timestamp: Utc::now(),
            tool_calls,
            token_count: Some(100),
        }
    }

    fn sample_session_with_messages(msgs: Vec<AnalyzedMessage>) -> CompletedSession {
        make_test_session(msgs)
    }

    #[test]
    fn test_technical_metrics_extraction_accuracy() {
        let analyzer = SessionAnalyzer;
        let session = sample_session_with_messages(vec![
            sample_message(
                MessageRole::Assistant,
                vec![
                    ToolCallSummary {
                        name: "Read".to_string(),
                        input_preview: "file.rs".to_string(),
                        success: true,
                        duration_ms: 50,
                    },
                    ToolCallSummary {
                        name: "Edit".to_string(),
                        input_preview: "edit".to_string(),
                        success: true,
                        duration_ms: 200,
                    },
                ],
            ),
            sample_message(MessageRole::User, vec![]),
        ]);

        let metrics = analyzer.extract_metrics(&session);

        assert_eq!(metrics.tool_call_count.get("Read"), Some(&1));
        assert_eq!(metrics.tool_call_count.get("Edit"), Some(&1));
        assert_eq!(metrics.error_count, 0);
        assert!((metrics.success_rate - 1.0).abs() < f64::EPSILON);
        assert!(metrics.total_duration > Duration::zero());
    }

    #[test]
    fn test_metrics_error_and_retry_counting() {
        let analyzer = SessionAnalyzer;
        let session = CompletedSession {
            errors_encountered: vec![
                ErrorRecord {
                    error_type: ErrorType::Compilation,
                    message: "type mismatch".to_string(),
                    timestamp: Utc::now(),
                    resolved: true,
                    resolution_time: Some(Duration::seconds(30)),
                },
                ErrorRecord {
                    error_type: ErrorType::Runtime,
                    message: "panic at main.rs:42".to_string(),
                    timestamp: Utc::now(),
                    resolved: false,
                    resolution_time: None,
                },
            ],
            commands_executed: vec![
                CommandExecutionRecord {
                    command: "cargo build".to_string(),
                    exit_code: Some(101),
                    duration_ms: 2000,
                    was_retry: true,
                    category: CommandCategory::Build,
                },
                CommandExecutionRecord {
                    command: "cargo build".to_string(),
                    exit_code: Some(0),
                    duration_ms: 1800,
                    was_retry: true,
                    category: CommandCategory::Build,
                },
            ],
            ..make_test_session(Vec::new())
        };

        let metrics = analyzer.extract_metrics(&session);
        assert_eq!(metrics.error_count, 2);
        assert_eq!(metrics.retry_count, 2);
        assert!((metrics.success_rate - 0.5).abs() < 0.01);
    }

    #[test]
    fn test_problem_solving_classification_incremental() {
        let analyzer = SessionAnalyzer;
        let session = sample_session_with_messages(vec![
            sample_message(
                MessageRole::Assistant,
                vec![ToolCallSummary {
                    name: "Edit".to_string(),
                    input_preview: "".to_string(),
                    success: true,
                    duration_ms: 100,
                }],
            ),
            sample_message(MessageRole::User, vec![]),
        ]);

        let ps = analyzer.analyze_problem_solving(&session);
        assert!(ps.is_some());
        assert_eq!(
            ps.as_ref().unwrap().approach_taken,
            ApproachType::Incremental
        );
    }

    #[test]
    fn test_problem_solving_none_for_minimal_session() {
        let analyzer = SessionAnalyzer;
        let session = CompletedSession {
            ..make_test_session(vec![sample_message(MessageRole::User, vec![])])
        };

        let ps = analyzer.analyze_problem_solving(&session);
        assert!(ps.is_none());
    }

    #[test]
    fn test_learning_opportunity_identification_compilation_error() {
        let analyzer = SessionAnalyzer;
        let session = CompletedSession {
            errors_encountered: vec![ErrorRecord {
                error_type: ErrorType::Compilation,
                message: "expected identifier".to_string(),
                timestamp: Utc::now(),
                resolved: true,
                resolution_time: Some(Duration::seconds(10)),
            }],
            ..make_test_session(Vec::new())
        };

        let opps = analyzer.identify_learning_opportunities(&session);
        assert!(!opps.is_empty());
        assert!(matches!(
            opps[0].topic,
            LearningTopic::DebuggingTechnique { .. }
        ));
        assert_eq!(opps[0].current_proficiency, ProficiencyLevel::Beginner);
        assert_eq!(opps[0].target_proficiency, ProficiencyLevel::Intermediate);
    }

    #[test]
    fn test_learning_opportunity_identification_logic_error() {
        let analyzer = SessionAnalyzer;
        let session = CompletedSession {
            errors_encountered: vec![ErrorRecord {
                error_type: ErrorType::Logic,
                message: "off-by-one error".to_string(),
                timestamp: Utc::now(),
                resolved: false,
                resolution_time: None,
            }],
            ..make_test_session(Vec::new())
        };

        let opps = analyzer.identify_learning_opportunities(&session);
        assert!(!opps.is_empty());
        assert!(matches!(opps[0].topic, LearningTopic::BestPractice { .. }));
    }

    #[test]
    fn test_no_learning_opportunities_for_clean_session() {
        let analyzer = SessionAnalyzer;
        let session = make_test_session(Vec::new());

        let opps = analyzer.identify_learning_opportunities(&session);
        assert!(opps.is_empty());
    }

    #[test]
    fn test_quality_score_calculation_perfect_session() {
        let analyzer = SessionAnalyzer;
        let session = CompletedSession {
            messages: vec![AnalyzedMessage {
                role: MessageRole::Assistant,
                content: "Adding /// docs here".to_string(),
                timestamp: Utc::now(),
                tool_calls: vec![],
                token_count: Some(50),
            }],
            commands_executed: vec![CommandExecutionRecord {
                command: "cargo test".to_string(),
                exit_code: Some(0),
                duration_ms: 5000,
                was_retry: false,
                category: CommandCategory::Test,
            }],
            ..make_test_session(Vec::new())
        };

        let metrics = analyzer.extract_metrics(&session);
        let qs = analyzer.compute_quality_score(&metrics, &session);
        assert!(qs.overall > 0.7);
        assert!(qs.correctness > 0.8);
        assert!(qs.best_practices > 0.8);
    }

    #[test]
    fn test_quality_score_low_efficiency() {
        let analyzer = SessionAnalyzer;
        let session = CompletedSession {
            commands_executed: vec![
                CommandExecutionRecord {
                    command: "cargo build".to_string(),
                    exit_code: Some(1),
                    duration_ms: 1000,
                    was_retry: true,
                    category: CommandCategory::Build,
                };
                5
            ],
            errors_encountered: vec![ErrorRecord {
                error_type: ErrorType::Compilation,
                message: "error".to_string(),
                timestamp: Utc::now(),
                resolved: false,
                resolution_time: None,
            }],
            ..make_test_session(Vec::new())
        };

        let metrics = analyzer.extract_metrics(&session);
        let qs = analyzer.compute_quality_score(&metrics, &session);
        assert!(qs.efficiency < 0.7);
    }

    #[test]
    fn test_recommendations_generated_for_low_efficiency() {
        let analyzer = SessionAnalyzer;
        let session = CompletedSession {
            commands_executed: vec![
                CommandExecutionRecord {
                    command: "cargo build".to_string(),
                    exit_code: Some(1),
                    duration_ms: 1000,
                    was_retry: true,
                    category: CommandCategory::Build,
                };
                5
            ],
            ..make_test_session(Vec::new())
        };

        let metrics = analyzer.extract_metrics(&session);
        let qs = analyzer.compute_quality_score(&metrics, &session);
        let recs = analyzer.generate_recommendations(&metrics, &qs);
        assert!(!recs.is_empty());
        assert!(recs
            .iter()
            .any(|r| r.category == RecommendationCategory::Performance));
    }

    #[tokio::test]
    async fn test_summary_generation_quality() {
        let summarizer = SessionSummarizer;
        let analysis = create_sample_analysis();

        let summary = summarizer.generate_summary(&analysis).await;

        assert!(!summary.one_liner.is_empty());
        assert!(!summary.what_was_done.is_empty());
        assert!(!summary.how_it_was_done.is_empty());
        assert!(summary.tags.contains(&"100pct-success".to_string()));
    }

    #[tokio::test]
    async fn test_report_generation_completeness() {
        let summarizer = SessionSummarizer;
        let analysis = create_sample_analysis();

        let report = summarizer.generate_report(&analysis).await;

        assert!(!report.executive_summary.is_empty());
        assert!(!report.technical_details.metrics_summary.is_empty());
        assert!(!report.technical_details.file_changes_detail.is_empty());
        assert!(!report.technical_details.command_timeline.is_empty());
        assert!(!report.technical_details.error_analysis.is_empty());
        assert!(!report.appendices.timeline_csv.is_empty());
        assert!(report.appendices.raw_metrics.is_object());
    }

    #[tokio::test]
    async fn test_thread_generation_format() {
        let summarizer = SessionSummarizer;
        let analysis = create_sample_analysis();

        let thread = summarizer.generate_thread(&analysis).await;

        assert!(!thread.is_empty());
        assert!(thread[0].contains("Thread"));
        assert!(thread.iter().any(|l| l.contains("Quality Score")));
    }

    #[test]
    fn test_insight_generation_high_retries() {
        let generator = InsightGenerator;
        let mut analysis = create_sample_analysis();
        analysis.metrics.retry_count = 8;

        let insights = generator.generate_insights(&analysis);

        assert!(!insights.is_empty());
        assert!(insights
            .iter()
            .any(|i| i.title.contains("retry") || i.title.contains("Retry")));
    }

    #[test]
    fn test_insight_generation_best_practice_recognition() {
        let generator = InsightGenerator;
        let mut analysis = create_sample_analysis();
        analysis.metrics.error_count = 0;
        analysis.metrics.files_modified = vec![
            FileModificationRecord {
                path: PathBuf::from("src/lib.rs"),
                edit_count: 5,
                lines_added: 40,
                lines_removed: 10,
                complexity_delta: ComplexityDelta::Unchanged,
            },
            FileModificationRecord {
                path: PathBuf::from("src/main.rs"),
                edit_count: 3,
                lines_added: 20,
                lines_removed: 5,
                complexity_delta: ComplexityDelta::Decreased(1),
            },
            FileModificationRecord {
                path: PathBuf::from("src/utils.rs"),
                edit_count: 2,
                lines_added: 15,
                lines_removed: 3,
                complexity_delta: ComplexityDelta::Increased(1),
            },
        ];

        let insights = generator.generate_insights(&analysis);

        assert!(insights
            .iter()
            .any(|i| i.category == InsightCategory::BestPractice));
    }

    #[test]
    fn test_insight_generation_low_documentation() {
        let generator = InsightGenerator;
        let mut analysis = create_sample_analysis();
        analysis.overall_quality_score.documentation = 0.3;

        let insights = generator.generate_insights(&analysis);

        assert!(insights
            .iter()
            .any(|i| i.title.to_lowercase().contains("document")));
    }

    #[test]
    fn test_insight_generation_knowledge_gap_from_learning_opp() {
        let generator = InsightGenerator;
        let mut analysis = create_sample_analysis();
        analysis.learning_opportunities = vec![LearningOpportunity {
            topic: LearningTopic::LanguageFeature {
                lang: "Rust".to_string(),
                feature: "async/await".to_string(),
            },
            current_proficiency: ProficiencyLevel::Novice,
            target_proficiency: ProficiencyLevel::Advanced,
            recommended_resources: vec![],
            practice_exercise: None,
            related_sessions: vec![],
            urgency: UrgencyLevel::Critical,
            estimated_effort_hours: 10.0,
        }];

        let insights = generator.generate_insights(&analysis);

        assert!(insights
            .iter()
            .any(|i| i.category == InsightCategory::KnowledgeGap));
    }

    #[test]
    fn test_pattern_identification_low_success_rate() {
        let generator = InsightGenerator;
        let analyses = (0..5)
            .map(|_| {
                let mut a = create_sample_analysis();
                a.metrics.success_rate = 0.5;
                a
            })
            .collect::<Vec<_>>();

        let patterns = generator.identify_patterns(&analyses);

        assert!(!patterns.is_empty());
        assert!(patterns
            .iter()
            .any(|p| p.pattern_name.contains("success")));
    }

    #[test]
    fn test_anti_pattern_detection_excessive_retry() {
        let detector = PatternDetector;
        let analyses: Vec<SessionAnalysis> = (0..6)
            .map(|i| {
                let mut a = create_sample_analysis();
                a.session_id = Uuid::new_v4();
                a.metrics.retry_count = 5 + i;
                a
            })
            .collect();

        let anti = detector.detect_anti_patterns(&analyses);

        assert!(!anti.is_empty());
        assert!(anti
            .iter()
            .any(|ap| ap.pattern == AntiPatternType::ExcessiveRetry));
    }

    #[test]
    fn test_anti_pattern_detection_skipping_tests() {
        let detector = PatternDetector;
        let analyses: Vec<SessionAnalysis> = (0..6)
            .map(|_| {
                let mut a = create_sample_analysis();
                a.session_id = Uuid::new_v4();
                a.metrics.commands_executed = vec![CommandExecutionRecord {
                    command: "cargo build".to_string(),
                    exit_code: Some(0),
                    duration_ms: 1000,
                    was_retry: false,
                    category: CommandCategory::Build,
                }];
                a.metrics.files_modified = vec![FileModificationRecord {
                    path: PathBuf::from("x.rs"),
                    edit_count: 1,
                    lines_added: 5,
                    lines_removed: 0,
                    complexity_delta: ComplexityDelta::Unchanged,
                }];
                a
            })
            .collect();

        let anti = detector.detect_anti_patterns(&analyses);

        assert!(anti
            .iter()
            .any(|ap| ap.pattern == AntiPatternType::SkippingTests));
    }

    #[test]
    fn test_habit_detection_consistency() {
        let detector = PatternDetector;
        let analyses = vec![create_sample_analysis()];

        let habits = detector.detect_habits(&analyses);

        assert_eq!(habits.preferred_approach, ApproachType::Incremental);
        assert!(!habits.common_tool_sequence.is_empty() || analyses[0].metrics.tool_call_count.is_empty());
    }

    #[test]
    fn test_habit_recovery_style_quick_fix() {
        let detector = PatternDetector;
        let mut analysis = create_sample_analysis();
        analysis.metrics.error_count = 0;

        let habits = detector.detect_habits(&vec![analysis]);

        assert_eq!(habits.error_recovery_style, ErrorRecoveryStyle::QuickFix);
    }

    #[test]
    fn test_habit_recovery_style_trial_and_error() {
        let detector = PatternDetector;
        let mut analysis = create_sample_analysis();
        analysis.metrics.error_count = 5;

        let habits = detector.detect_habits(&vec![analysis]);

        assert_eq!(habits.error_recovery_style, ErrorRecoveryStyle::TrialAndError);
    }

    #[test]
    fn test_skill_progression_trend_improving() {
        let detector = PatternDetector;
        let analyses = vec![
            create_low_quality_analysis(),
            create_high_quality_analysis(),
        ];

        let progression = detector.detect_skill_progression(&analyses);

        let quality_traj = progression.skills.get("overall_code_quality").unwrap();
        assert_eq!(quality_traj.trend, TrendDirection::Improving);
        assert_eq!(quality_traj.history.len(), 2);
    }

    #[test]
    fn test_skill_progression_trend_declining() {
        let detector = PatternDetector;
        let analyses = vec![
            create_high_quality_analysis(),
            create_low_quality_analysis(),
        ];

        let progression = detector.detect_skill_progression(&analyses);

        let quality_traj = progression.skills.get("overall_code_quality").unwrap();
        assert_eq!(quality_traj.trend, TrendDirection::Declining);
    }

    #[test]
    fn test_skill_progression_single_session_unknown_trend() {
        let detector = PatternDetector;
        let analyses = vec![create_sample_analysis()];

        let progression = detector.detect_skill_progression(&analyses);

        let quality_traj = progression.skills.get("overall_code_quality").unwrap();
        assert_eq!(quality_traj.trend, TrendDirection::Unknown);
    }

    #[test]
    fn test_boundary_empty_session() {
        let engine = SessionIntelligenceEngine::new(IntelligenceConfig::default());
        let empty_session = CompletedSession {
            ..make_test_session(Vec::new())
        };

        let result = engine.analyze_session(&empty_session);
        assert!(result.is_some());
        let analysis = result.unwrap();
        assert_eq!(analysis.metrics.error_count, 0);
        assert_eq!(analysis.metrics.retry_count, 0);
        assert!(analysis.learning_opportunities.is_empty());
    }

    #[test]
    fn test_boundary_single_message_session() {
        let engine = SessionIntelligenceEngine::new(IntelligenceConfig::default());
        let single_msg_session = make_test_session(vec![sample_message(
            MessageRole::User,
            vec![],
        )]);

        let result = engine.analyze_session(&single_msg_session);
        assert!(result.is_some());
        let analysis = result.unwrap();
        assert!(analysis.problem_solving.is_none());
    }

    #[test]
    fn test_boundary_session_shorter_than_min_duration() {
        let engine = SessionIntelligenceEngine::new(IntelligenceConfig {
            min_session_duration: Duration::hours(1),
            ..Default::default()
        });

        let short_session = CompletedSession {
            start_time: Utc::now(),
            end_time: Utc::now() + chrono::Duration::seconds(10),
            ..make_test_session(Vec::new())
        };

        let result = engine.analyze_session(&short_session);
        assert!(result.is_none(), "Short sessions should be filtered out");
    }

    #[test]
    fn test_boundary_invalid_time_range() {
        let engine = SessionIntelligenceEngine::new(IntelligenceConfig::default());

        let invalid_session = CompletedSession {
            start_time: Utc::now() + chrono::Duration::hours(1),
            end_time: Utc::now(),
            ..make_test_session(Vec::new())
        };

        let result = engine.analyze_session(&invalid_session);
        assert!(result.is_none(), "Invalid time range should return None");
    }

    #[test]
    fn test_boundary_long_session_handling() {
        let analyzer = SessionAnalyzer;
        let many_messages: Vec<AnalyzedMessage> = (0..100)
            .map(|i| {
                sample_message(
                    if i % 2 == 0 { MessageRole::User } else { MessageRole::Assistant },
                    if i % 2 != 0 {
                        vec![ToolCallSummary {
                            name: "Edit".to_string(),
                            input_preview: "".to_string(),
                            success: true,
                            duration_ms: 50,
                        }]
                    } else {
                        vec![]
                    },
                )
            })
            .collect();

        let session = sample_session_with_messages(many_messages);
        let analysis = analyzer.analyze(&session);

        assert_eq!(analysis.session_id, session.id);
        assert!(analysis.metrics.total_duration > Duration::zero());
    }

    #[test]
    fn test_engine_full_pipeline() {
        let engine = SessionIntelligenceEngine::new(IntelligenceConfig::default());
        let session = sample_session_with_messages(vec![
            sample_message(
                MessageRole::Assistant,
                vec![
                    ToolCallSummary {
                        name: "Read".to_string(),
                        input_preview: "main.rs".to_string(),
                        success: true,
                        duration_ms: 30,
                    },
                    ToolCallSummary {
                        name: "Edit".to_string(),
                        input_preview: "fix bug".to_string(),
                        success: true,
                        duration_ms: 150,
                    },
                ],
            ),
            sample_message(MessageRole::User, vec![]),
        ]);

        let analysis = engine.analyze_session(&session);
        assert!(analysis.is_some());
        let a = analysis.unwrap();

        let insights = engine.generate_insights(&a);
        assert!(insights.len() <= 10);

        let anti = engine.detect_anti_patterns(&[a.clone()]);
        assert!(anti.len() <= 5);

        let habits = engine.analyze_habits(&[a]);
        assert_eq!(habits.preferred_approach, ApproachType::Incremental);
    }

    #[test]
    fn test_inefficient_pattern_detection() {
        let detector = PatternDetector;
        let mut analysis = create_sample_analysis();
        analysis.metrics.total_duration = Duration::minutes(10);
        analysis.metrics.active_coding_time = Duration::minutes(3);
        analysis.metrics.waiting_time = Duration::minutes(7);

        let inefficiencies = detector.detect_inefficient_patterns(&[analysis]);

        assert!(!inefficiencies.is_empty());
        assert!(inefficiencies[0].name.contains("idle") || inefficiencies[0].name.contains("wait"));
    }

    #[test]
    fn test_config_default_values() {
        let config = IntelligenceConfig::default();
        assert!(config.enable_auto_analysis);
        assert_eq!(config.min_session_duration, Duration::seconds(30));
        assert_eq!(config.max_analysis_time, Duration::seconds(10));
        assert!((config.insight_confidence_threshold - 0.6).abs() < f64::EPSILON);
        assert!((config.learning_opportunity_threshold - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn test_quality_score_all_dimensions_present() {
        let analyzer = SessionAnalyzer;
        let session = make_test_session(Vec::new());
        let metrics = analyzer.extract_metrics(&session);
        let qs = analyzer.compute_quality_score(&metrics, &session);

        assert!(qs.overall >= 0.0 && qs.overall <= 1.0);
        assert!(qs.efficiency >= 0.0 && qs.efficiency <= 1.0);
        assert!(qs.correctness >= 0.0 && qs.correctness <= 1.0);
        assert!(qs.best_practices >= 0.0 && qs.best_practices <= 1.0);
        assert!(qs.documentation >= 0.0 && qs.documentation <= 1.0);
    }

    #[test]
    fn test_completed_session_default() {
        let session = CompletedSession::default();
        assert!(session.messages.is_empty());
        assert!(session.files_modified.is_empty());
        assert!(session.commands_executed.is_empty());
        assert!(session.errors_encountered.is_empty());
        assert_eq!(session.token_usage.total_input, 0);
    }

    fn create_sample_analysis() -> SessionAnalysis {
        SessionAnalysis {
            session_id: Uuid::new_v4(),
            analyzed_at: Utc::now(),
            metrics: TechnicalMetrics {
                total_duration: Duration::minutes(5),
                active_coding_time: Duration::minutes(4),
                waiting_time: Duration::minutes(1),
                tool_call_count: [
                    ("Read".to_string(), 3),
                    ("Edit".to_string(), 2),
                ]
                .into_iter()
                .collect(),
                token_usage: TokenUsageSummary {
                    total_input: 8000,
                    total_output: 3000,
                    cache_read: 2000,
                    estimated_cost_usd: Some(0.05),
                },
                files_modified: vec![FileModificationRecord {
                    path: PathBuf::from("src/lib.rs"),
                    edit_count: 3,
                    lines_added: 25,
                    lines_removed: 8,
                    complexity_delta: ComplexityDelta::Increased(1),
                }],
                commands_executed: vec![
                    CommandExecutionRecord {
                        command: "cargo check".to_string(),
                        exit_code: Some(0),
                        duration_ms: 1200,
                        was_retry: false,
                        category: CommandCategory::Build,
                    },
                ],
                error_count: 0,
                retry_count: 0,
                success_rate: 1.0,
            },
            problem_solving: Some(ProblemSolvingAnalysis {
                problem_description: "Add new feature X".to_string(),
                approach_taken: ApproachType::Incremental,
                steps: vec![SolvingStep {
                    step_number: 1,
                    description: "Read existing code".to_string(),
                    duration: Duration::milliseconds(50),
                    tools_used: vec!["Read".to_string()],
                    outcome: StepOutcome::Success,
                }],
                dead_ends: vec![],
                breakthrough_moment: None,
                final_solution: SolutionDescription {
                    what_changed: "Added feature X module".to_string(),
                    files_affected: vec![PathBuf::from("src/lib.rs")],
                    verification_method: VerificationMethod::CompilerSuccess,
                },
                efficiency_score: 0.92,
            }),
            learning_opportunities: vec![],
            overall_quality_score: QualityScore {
                overall: 0.88,
                efficiency: 0.95,
                correctness: 1.0,
                best_practices: 0.80,
                documentation: 0.70,
            },
            recommendations: vec![],
        }
    }

    fn create_low_quality_analysis() -> SessionAnalysis {
        let mut a = create_sample_analysis();
        a.overall_quality_score = QualityScore {
            overall: 0.35,
            efficiency: 0.30,
            correctness: 0.40,
            best_practices: 0.30,
            documentation: 0.35,
        };
        a.metrics.success_rate = 0.4;
        a.metrics.error_count = 5;
        a.metrics.retry_count = 8;
        a.session_id = Uuid::new_v4();
        a
    }

    fn create_high_quality_analysis() -> SessionAnalysis {
        let mut a = create_sample_analysis();
        a.overall_quality_score = QualityScore {
            overall: 0.95,
            efficiency: 0.98,
            correctness: 1.0,
            best_practices: 0.92,
            documentation: 0.90,
        };
        a.metrics.success_rate = 1.0;
        a.metrics.error_count = 0;
        a.metrics.retry_count = 0;
        a.session_id = Uuid::new_v4();
        a
    }
}
