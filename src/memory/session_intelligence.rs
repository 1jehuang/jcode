use super::analysis::{IntelligenceConfig, SessionAnalyzer, CompletedSession, SessionAnalysis};
use super::insights::{SessionSummarizer, InsightGenerator, Insight};
use super::patterns::{PatternDetector, AntiPatternDetection, UserHabitProfile};

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

    pub async fn generate_summary(&self, analysis: &SessionAnalysis) -> super::insights::SessionSummary {
        self.summarizer.generate_summary(analysis).await
    }

    pub async fn generate_report(&self, analysis: &SessionAnalysis) -> super::insights::SessionReport {
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