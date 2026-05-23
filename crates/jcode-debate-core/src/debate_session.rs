//! # Debate Session Module
//!
//! Manages the state and history of a multi-perspective debate session.

use chrono::{DateTime, Utc};
use jcode_message_types::{ContentBlock, Message, Role};
use serde::{Deserialize, Serialize};

use crate::perspectives::{DebateTopic, Perspective, PerspectiveType};
use crate::rate_limiter::RateLimiter;

/// Configuration for debate behavior
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DebateConfig {
    /// Number of rounds before synthesis (default: 2)
    pub rounds: u32,
    /// Maximum tokens per response
    pub max_tokens: u32,
    /// Temperature for sampling
    pub temperature: f32,
    /// Default model to use
    pub model: String,
    /// Provider to use
    pub provider: String,
    /// Minimum seconds between calls to same perspective
    pub rate_limit_interval_secs: u64,
    /// Enable parallel perspective calls
    pub parallel_calls: bool,
    /// Include reasoning in responses
    pub include_reasoning: bool,
    /// Session timeout in seconds
    pub timeout_secs: u64,
}

impl Default for DebateConfig {
    fn default() -> Self {
        Self {
            rounds: 2,
            max_tokens: 1024,
            temperature: 0.7,
            model: "claude-sonnet-4-7".to_string(),
            provider: "anthropic".to_string(),
            rate_limit_interval_secs: 2,
            parallel_calls: true,
            include_reasoning: true,
            timeout_secs: 120,
        }
    }
}

/// Phase of the debate
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DebatePhase {
    /// Initial state, no topic set
    Initial,
    /// Topic has been set, preparing for debate
    Preparing,
    /// Advocate is presenting their case
    AdvocateTurn,
    /// Critic is presenting their analysis
    CriticTurn,
    /// Synthesizer is integrating perspectives
    Synthesizing,
    /// Debate has concluded
    Completed,
    /// Debate was cancelled or failed
    Failed,
}

impl Default for DebatePhase {
    fn default() -> Self {
        Self::Initial
    }
}

impl std::fmt::Display for DebatePhase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DebatePhase::Initial => write!(f, "initial"),
            DebatePhase::Preparing => write!(f, "preparing"),
            DebatePhase::AdvocateTurn => write!(f, "advocate_turn"),
            DebatePhase::CriticTurn => write!(f, "critic_turn"),
            DebatePhase::Synthesizing => write!(f, "synthesizing"),
            DebatePhase::Completed => write!(f, "completed"),
            DebatePhase::Failed => write!(f, "failed"),
        }
    }
}

/// Response from a perspective
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerspectiveResponse {
    /// The perspective that generated this response
    pub perspective_type: PerspectiveType,
    /// The generated text response
    pub text: String,
    /// Token usage if available
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    /// When this response was generated
    pub timestamp: DateTime<Utc>,
    /// Round number this response belongs to
    pub round: u32,
    /// Execution duration in milliseconds
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
}

impl PerspectiveResponse {
    /// Create a new perspective response
    pub fn new(perspective_type: PerspectiveType, text: String, round: u32) -> Self {
        Self {
            perspective_type,
            text,
            input_tokens: None,
            output_tokens: None,
            timestamp: Utc::now(),
            round,
            duration_ms: None,
        }
    }

    /// Set token usage
    pub fn with_tokens(mut self, input: u64, output: u64) -> Self {
        self.input_tokens = Some(input);
        self.output_tokens = Some(output);
        self
    }

    /// Set duration
    pub fn with_duration(mut self, duration_ms: u64) -> Self {
        self.duration_ms = Some(duration_ms);
        self
    }
}

/// A single turn in the debate
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DebateTurn {
    /// Round number
    pub round: u32,
    /// Phase of this turn
    pub phase: DebatePhase,
    /// Response from the perspective
    pub response: PerspectiveResponse,
    /// Error if the turn failed
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl DebateTurn {
    /// Check if this turn succeeded
    pub fn is_success(&self) -> bool {
        self.error.is_none()
    }
}

/// Final verdict from the debate
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DebateVerdict {
    /// The recommendation
    pub recommendation: String,
    /// Confidence level: high, medium, low
    pub confidence: String,
    /// Summary of key points
    pub summary: String,
    /// Key agreements between perspectives
    pub agreements: Vec<String>,
    /// Key disagreements
    pub disagreements: Vec<String>,
    /// Caveats and conditions
    pub caveats: Vec<String>,
    /// When the verdict was generated
    pub timestamp: DateTime<Utc>,
    /// Rounds completed
    pub rounds_completed: u32,
}

impl DebateVerdict {
    /// Create a verdict from synthesizer response
    pub fn from_response(
        response: &PerspectiveResponse,
        agreements: Vec<String>,
        disagreements: Vec<String>,
    ) -> Self {
        let (recommendation, confidence, summary) = Self::parse_synthesizer_text(&response.text);

        Self {
            recommendation,
            confidence,
            summary,
            agreements,
            disagreements,
            caveats: Vec::new(),
            timestamp: Utc::now(),
            rounds_completed: response.round,
        }
    }

    /// Parse the synthesizer's text to extract recommendation details
    fn parse_synthesizer_text(text: &str) -> (String, String, String) {
        let text_lower = text.to_lowercase();

        let confidence = if text_lower.contains("high confidence")
            || text_lower.contains("strongly recommend")
            || text_lower.contains("clear recommendation")
        {
            "high".to_string()
        } else if text_lower.contains("medium")
            || text_lower.contains("moderate")
            || text_lower.contains("conditional")
        {
            "medium".to_string()
        } else {
            "low".to_string()
        };

        // Extract recommendation - look for "recommend", "verdict", etc.
        let recommendation = text
            .lines()
            .find(|line| {
                line.to_lowercase().contains("recommend")
                    || line.to_lowercase().contains("verdict")
                    || line.to_lowercase().contains("decision")
            })
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|| "See summary for details".to_string());

        // Use first paragraph as summary
        let summary = text
            .split('\n')
            .take(3)
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(500)
            .collect();

        (recommendation, confidence, summary)
    }
}

/// The main debate session state
#[derive(Debug, Clone)]
pub struct DebateSession {
    /// Session identifier
    pub id: String,
    /// Current configuration
    pub config: DebateConfig,
    /// The topic being debated
    topic: Option<DebateTopic>,
    /// Current phase of the debate
    pub phase: DebatePhase,
    /// Current round number (1-indexed)
    round: u32,
    /// All turns in the debate
    turns: Vec<DebateTurn>,
    /// Final verdict (if completed)
    verdict: Option<DebateVerdict>,
    /// Perspectives used in this session
    perspectives: Vec<Perspective>,
    /// Rate limiter
    rate_limiter: RateLimiter,
    /// Session metadata
    pub created_at: DateTime<Utc>,
    /// When the session was last updated
    pub updated_at: DateTime<Utc>,
    /// Error state
    error: Option<String>,
}

impl DebateSession {
    /// Create a new debate session
    pub fn new(config: DebateConfig) -> Self {
        Self {
            id: uuid_v4(),
            config: config.clone(),
            topic: None,
            phase: DebatePhase::Initial,
            round: 0,
            turns: Vec::new(),
            verdict: None,
            perspectives: Perspective::all_three(),
            rate_limiter: RateLimiter::new(config.rate_limit_interval_secs),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            error: None,
        }
    }

    /// Create with default configuration
    pub fn with_topic(topic: DebateTopic, config: DebateConfig) -> Self {
        let mut session = Self::new(config);
        session.set_topic(topic);
        session
    }

    /// Set the debate topic
    pub fn set_topic(&mut self, topic: DebateTopic) {
        self.topic = Some(topic);
        self.phase = DebatePhase::Preparing;
        self.updated_at = Utc::now();
    }

    /// Get the current topic
    pub fn topic(&self) -> Option<&DebateTopic> {
        self.topic.as_ref()
    }

    /// Get current round number (1-indexed)
    pub fn round(&self) -> u32 {
        self.round
    }

    /// Get the perspectives participating in this session
    pub fn perspectives(&self) -> &[Perspective] {
        &self.perspectives
    }

    /// Get the session rate limiter
    pub fn rate_limiter(&self) -> &RateLimiter {
        &self.rate_limiter
    }

    /// Check if debate can proceed to next round
    pub fn can_continue(&self) -> bool {
        self.phase != DebatePhase::Completed
            && self.phase != DebatePhase::Failed
            && self.round < self.config.rounds
    }

    /// Advance to the next phase
    pub fn advance_phase(&mut self) {
        self.phase = match self.phase {
            DebatePhase::Initial => DebatePhase::Preparing,
            DebatePhase::Preparing => {
                self.round += 1;
                DebatePhase::AdvocateTurn
            }
            DebatePhase::AdvocateTurn => DebatePhase::CriticTurn,
            DebatePhase::CriticTurn => {
                if self.round >= self.config.rounds {
                    DebatePhase::Synthesizing
                } else {
                    self.round += 1;
                    DebatePhase::AdvocateTurn
                }
            }
            DebatePhase::Synthesizing => DebatePhase::Completed,
            DebatePhase::Completed | DebatePhase::Failed => return,
        };
        self.updated_at = Utc::now();
    }

    /// Record a turn's response
    pub fn record_turn(&mut self, response: PerspectiveResponse) {
        let phase = self.phase;
        let turn = DebateTurn {
            round: self.round,
            phase,
            response,
            error: None,
        };
        self.turns.push(turn);
        self.updated_at = Utc::now();
    }

    /// Record a turn's error
    pub fn record_error(&mut self, perspective_type: PerspectiveType, error: String) {
        let turn = DebateTurn {
            round: self.round,
            phase: self.phase,
            response: PerspectiveResponse::new(perspective_type, String::new(), self.round),
            error: Some(error.clone()),
        };
        self.turns.push(turn);
        self.error = Some(error);
        self.updated_at = Utc::now();

        if self.phase != DebatePhase::Synthesizing {
            self.phase = DebatePhase::Failed;
        }
    }

    /// Set the final verdict
    pub fn set_verdict(&mut self, verdict: DebateVerdict) {
        self.verdict = Some(verdict);
        self.phase = DebatePhase::Completed;
        self.updated_at = Utc::now();
    }

    /// Get the verdict
    pub fn verdict(&self) -> Option<&DebateVerdict> {
        self.verdict.as_ref()
    }

    /// Get all turns
    pub fn turns(&self) -> &[DebateTurn] {
        &self.turns
    }

    /// Get turns for a specific round
    pub fn turns_for_round(&self, round: u32) -> Vec<&DebateTurn> {
        self.turns.iter().filter(|t| t.round == round).collect()
    }

    /// Get responses for a specific perspective type
    pub fn responses_for(&self, perspective_type: PerspectiveType) -> Vec<&PerspectiveResponse> {
        self.turns
            .iter()
            .filter(|t| t.response.perspective_type == perspective_type && t.is_success())
            .map(|t| &t.response)
            .collect()
    }

    /// Get current error
    pub fn error(&self) -> Option<&String> {
        self.error.as_ref()
    }

    /// Build messages for a perspective including history
    pub fn build_messages(&self, perspective_type: PerspectiveType) -> Vec<Message> {
        let mut messages = Vec::new();

        // Add topic as initial context
        if let Some(topic) = &self.topic {
            messages.push(Message {
                role: Role::User,
                content: vec![ContentBlock::Text {
                    text: format!(
                        "## Debate Topic\n{}\n\n{}",
                        topic.question,
                        topic.context.as_deref().unwrap_or("")
                    ),
                    cache_control: None,
                }],
                timestamp: Some(self.created_at),
                tool_duration_ms: None,
            });
        }

        // Add history from previous turns
        for turn in &self.turns {
            if turn.is_success() {
                let role = match turn.response.perspective_type {
                    PerspectiveType::Advocate => "Advocate",
                    PerspectiveType::Critic => "Critic",
                    PerspectiveType::Synthesizer => "Synthesizer",
                };

                messages.push(Message {
                    role: Role::Assistant,
                    content: vec![ContentBlock::Text {
                        text: format!("[{} - Round {}]\n{}", role, turn.round, turn.response.text),
                        cache_control: None,
                    }],
                    timestamp: Some(turn.response.timestamp),
                    tool_duration_ms: None,
                });
            }
        }

        // Add latest from the other perspective if we're in round > 1
        if self.round > 1 {
            let other_type = match perspective_type {
                PerspectiveType::Advocate => PerspectiveType::Critic,
                PerspectiveType::Critic => PerspectiveType::Advocate,
                PerspectiveType::Synthesizer => return messages,
            };

            if let Some(last_response) = self.responses_for(other_type).last() {
                let role_name = match other_type {
                    PerspectiveType::Advocate => "Advocate",
                    PerspectiveType::Critic => "Critic",
                    PerspectiveType::Synthesizer => "Synthesizer",
                };

                messages.push(Message {
                    role: Role::User,
                    content: vec![ContentBlock::Text {
                        text: format!(
                            "[Previous {} response to consider]\n{}\n\n[Your task: Continue the debate, building upon or countering this perspective.]",
                            role_name,
                            last_response.text
                        ),
                        cache_control: None,
                    }],
                    timestamp: Some(Utc::now()),
                    tool_duration_ms: None,
                });
            }
        }

        messages
    }

    /// Get session statistics
    pub fn stats(&self) -> DebateSessionStats {
        DebateSessionStats {
            id: self.id.clone(),
            phase: self.phase.to_string(),
            round: self.round,
            total_turns: self.turns.len(),
            successful_turns: self.turns.iter().filter(|t| t.is_success()).count(),
            failed_turns: self.turns.iter().filter(|t| !t.is_success()).count(),
            total_input_tokens: self
                .turns
                .iter()
                .filter_map(|t| t.response.input_tokens)
                .sum(),
            total_output_tokens: self
                .turns
                .iter()
                .filter_map(|t| t.response.output_tokens)
                .sum(),
            total_duration_ms: self
                .turns
                .iter()
                .filter_map(|t| t.response.duration_ms)
                .sum(),
            created_at: self.created_at,
            updated_at: self.updated_at,
        }
    }
}

/// Statistics for a debate session
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DebateSessionStats {
    pub id: String,
    pub phase: String,
    pub round: u32,
    pub total_turns: usize,
    pub successful_turns: usize,
    pub failed_turns: usize,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub total_duration_ms: u64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Simple UUID v4 generation (for compatibility without external crate)
fn uuid_v4() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};

    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();

    let random_bytes: [u8; 16] = {
        let mut bytes = [0u8; 16];
        // Use timestamp for some randomness
        let ts_low = timestamp as u64;
        let ts_high = (timestamp >> 64) as u64;
        for (i, byte) in bytes.iter_mut().enumerate() {
            let mix: u8 = ((ts_low >> (i % 8)) ^ (ts_high >> ((i * 7) % 64))) as u8;
            *byte = mix.wrapping_mul(0xAE).wrapping_add(0x12);
        }
        bytes
    };

    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        random_bytes[0], random_bytes[1], random_bytes[2], random_bytes[3],
        random_bytes[4], random_bytes[5], random_bytes[6], random_bytes[7],
        random_bytes[8], random_bytes[9], random_bytes[10], random_bytes[11],
        random_bytes[12], random_bytes[13], random_bytes[14], random_bytes[15]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debate_config_default() {
        let config = DebateConfig::default();
        assert_eq!(config.rounds, 2);
        assert_eq!(config.max_tokens, 1024);
        assert!(config.parallel_calls);
    }

    #[test]
    fn debate_phase_display() {
        assert_eq!(DebatePhase::Initial.to_string(), "initial");
        assert_eq!(DebatePhase::Completed.to_string(), "completed");
    }

    #[test]
    fn debate_session_creation() {
        let session = DebateSession::new(DebateConfig::default());
        assert_eq!(session.phase, DebatePhase::Initial);
        assert!(session.topic().is_none());
        assert_eq!(session.round(), 0);
        assert_eq!(session.perspectives().len(), 3);
    }

    #[tokio::test]
    async fn debate_session_exposes_rate_limiter() {
        let session = DebateSession::new(DebateConfig::default());
        assert!(
            session
                .rate_limiter()
                .can_call(PerspectiveType::Advocate)
                .await
        );
    }

    #[test]
    fn debate_session_with_topic() {
        let topic = DebateTopic::new("Should we adopt microservices?");
        let session = DebateSession::with_topic(topic.clone(), DebateConfig::default());

        assert_eq!(session.phase, DebatePhase::Preparing);
        assert!(session.topic().is_some());
    }

    #[test]
    fn debate_session_advance_phase() {
        let mut session = DebateSession::new(DebateConfig::default());
        let topic = DebateTopic::new("Test question");
        session.set_topic(topic);

        session.advance_phase();
        assert_eq!(session.round(), 1);
        assert_eq!(session.phase, DebatePhase::AdvocateTurn);

        session.advance_phase();
        assert_eq!(session.phase, DebatePhase::CriticTurn);
        assert_eq!(session.round(), 1); // Round unchanged

        session.advance_phase();
        // With 2 rounds config, CriticTurn transitions to AdvocateTurn for next round
        assert_eq!(session.phase, DebatePhase::AdvocateTurn);
        assert_eq!(session.round(), 2);
    }

    #[test]
    fn debate_session_record_turn() {
        let mut session = DebateSession::new(DebateConfig::default());
        let topic = DebateTopic::new("Test");
        session.set_topic(topic);

        session.advance_phase();
        session.record_turn(PerspectiveResponse::new(
            PerspectiveType::Advocate,
            "Advocate argues...".to_string(),
            1,
        ));

        assert_eq!(session.turns().len(), 1);
        assert!(session.turns()[0].is_success());
    }

    #[test]
    fn debate_session_record_error() {
        let mut session = DebateSession::new(DebateConfig::default());
        let topic = DebateTopic::new("Test");
        session.set_topic(topic);

        session.advance_phase();
        session.record_error(PerspectiveType::Advocate, "API timeout".to_string());

        assert_eq!(session.turns().len(), 1);
        assert!(!session.turns()[0].is_success());
        assert_eq!(session.phase, DebatePhase::Failed);
    }

    #[test]
    fn debate_session_responses_for() {
        let mut session = DebateSession::new(DebateConfig::default());
        let topic = DebateTopic::new("Test");
        session.set_topic(topic);

        session.advance_phase();
        session.record_turn(PerspectiveResponse::new(
            PerspectiveType::Advocate,
            "Advocate response 1".to_string(),
            1,
        ));

        session.advance_phase();
        session.record_turn(PerspectiveResponse::new(
            PerspectiveType::Critic,
            "Critic response".to_string(),
            1,
        ));

        let advocate_responses = session.responses_for(PerspectiveType::Advocate);
        assert_eq!(advocate_responses.len(), 1);
        assert!(advocate_responses[0].text.contains("Advocate"));

        let critic_responses = session.responses_for(PerspectiveType::Critic);
        assert_eq!(critic_responses.len(), 1);
    }

    #[test]
    fn debate_session_build_messages() {
        let mut session = DebateSession::new(DebateConfig::default());
        let topic = DebateTopic::new("Should we use Rust?");
        session.set_topic(topic);

        session.advance_phase();
        session.record_turn(PerspectiveResponse::new(
            PerspectiveType::Advocate,
            "Rust is great because...".to_string(),
            1,
        ));

        let messages = session.build_messages(PerspectiveType::Critic);
        // Should have topic message
        assert!(!messages.is_empty());
    }

    #[test]
    fn debate_verdict_parse() {
        let text = "I strongly recommend we adopt microservices. High confidence in this decision.";
        let response = PerspectiveResponse::new(PerspectiveType::Synthesizer, text.to_string(), 1);

        let verdict = DebateVerdict::from_response(&response, vec![], vec![]);
        assert_eq!(verdict.confidence, "high");
    }

    #[test]
    fn session_stats() {
        let session = DebateSession::new(DebateConfig::default());
        let stats = session.stats();

        assert_eq!(stats.total_turns, 0);
        assert_eq!(stats.successful_turns, 0);
    }
}
