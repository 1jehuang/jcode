use chrono::Duration;
use serde::{Serialize, Deserialize};
use std::path::PathBuf;
use uuid::Uuid;
use super::analysis::*;

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
                    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ"),
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
    pub tool_usage_breakdown: std::collections::HashMap<String, usize>,
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
                            current_value: proficiency_to_f64(&lo.current_proficiency),
                            expected_value: proficiency_to_f64(&lo.target_proficiency),
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PatternInsight {
    pub pattern_name: String,
    pub description: String,
    pub affected_sessions: Vec<Uuid>,
    pub recommendation: String,
    pub potential_improvement: String,
}

fn proficiency_to_f64(level: &ProficiencyLevel) -> f64 {
    match level {
        ProficiencyLevel::Novice => 0.0,
        ProficiencyLevel::Beginner => 1.0,
        ProficiencyLevel::Intermediate => 2.0,
        ProficiencyLevel::Advanced => 3.0,
        ProficiencyLevel::Expert => 4.0,
        ProficiencyLevel::Master => 5.0,
    }
}