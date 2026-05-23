//! # Perspectives Module
//!
//! Defines the three debate perspectives used in the multi-perspective debate system:
//! - **Advocate**: Argues in favor of the proposal
//! - **Critic**: Challenges and scrutinizes the proposal
//! - **Synthesizer**: Integrates perspectives into a coherent decision

use chrono::{DateTime, Utc};
use jcode_message_types::{ContentBlock, Message, Role};
use serde::{Deserialize, Serialize};

/// Type of debate perspective
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PerspectiveType {
    /// Argues in favor of the proposal
    Advocate,
    /// Challenges and scrutinizes the proposal
    Critic,
    /// Integrates perspectives into a coherent decision
    Synthesizer,
}

impl std::fmt::Display for PerspectiveType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PerspectiveType::Advocate => write!(f, "advocate"),
            PerspectiveType::Critic => write!(f, "critic"),
            PerspectiveType::Synthesizer => write!(f, "synthesizer"),
        }
    }
}

/// A debate topic or question to be debated
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DebateTopic {
    /// The main question or proposal
    pub question: String,
    /// Optional context/background information
    pub context: Option<String>,
    /// Constraints or criteria for evaluation
    pub constraints: Vec<String>,
    /// Tags for categorization
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
}

impl DebateTopic {
    /// Create a new debate topic from a question
    pub fn new(question: impl Into<String>) -> Self {
        Self {
            question: question.into(),
            context: None,
            constraints: Vec::new(),
            tags: Vec::new(),
        }
    }

    /// Add context to the topic
    pub fn with_context(mut self, context: impl Into<String>) -> Self {
        self.context = Some(context.into());
        self
    }

    /// Add constraints to the topic
    pub fn with_constraints(mut self, constraints: Vec<String>) -> Self {
        self.constraints = constraints;
        self
    }
}

/// System prompt template for the Advocate perspective
const ADVOCATE_SYSTEM_PROMPT: &str = r#"You are a skilled Advocate in a multi-perspective debate. Your role is to argue STRONGLY in favor of the proposal.

## Your Mission
- Present compelling arguments FOR the proposal
- Identify and articulate the benefits and opportunities
- Address potential objections preemptively with strong counter-arguments
- Provide concrete examples and evidence supporting the proposal
- Maintain intellectual honesty while making the strongest possible case

## Debate Guidelines
1. Start with your strongest argument
2. Use specific examples and evidence
3. Acknowledge legitimate concerns briefly, then explain why they are outweighed
4. Be passionate but rational
5. Challenge the Critic's objections directly and respectfully

## Response Format
Your response should be:
- Clear and persuasive
- Structured with clear argument points
- Around 300-500 words
- Directly addressing the proposal and counter-arguments

Begin your advocacy now."#;

/// System prompt template for the Critic perspective
const CRITIC_SYSTEM_PROMPT: &str = r#"You are a critical Analyst in a multi-perspective debate. Your role is to identify weaknesses, risks, and unintended consequences of the proposal.

## Your Mission
- Challenge assumptions underlying the proposal
- Identify potential failure modes and risks
- Surface hidden costs and trade-offs
- Question the feasibility and implementation challenges
- Be skeptical of over-optimistic claims

## Debate Guidelines
1. Start with your most significant objection
2. Use specific examples of how things could go wrong
3. Quantify risks where possible
4. Consider second and third-order effects
5. Be constructive in your criticism - explain WHY something is problematic and suggest alternatives

## Response Format
Your response should be:
- Analytical and precise
- Structured with clear risk points
- Around 300-500 words
- Providing actionable feedback

Begin your critical analysis now."#;

/// System prompt template for the Synthesizer perspective
const SYNTHESIZER_SYSTEM_PROMPT: &str = r#"You are a Synthesis Expert in a multi-perspective debate. Your role is to integrate opposing viewpoints into a nuanced, actionable decision.

## Your Mission
- Listen carefully to both Advocate and Critic perspectives
- Identify areas of genuine agreement vs. disagreement
- Find middle ground and creative solutions
- Evaluate arguments based on evidence and logic, not just advocacy
- Produce a clear verdict with reasoning

## Debate Guidelines
1. Acknowledge the valid points from BOTH sides
2. Apply rigorous evaluation criteria
3. Consider the context and constraints
4. Produce a balanced assessment with clear reasoning
5. Include caveats and conditions where appropriate

## Response Format
Your response should be:
- Balanced and fair to all perspectives
- Structured as: Summary of positions, Key points of agreement/disagreement, Evaluation, Final verdict
- Around 400-600 words
- Include a clear RECOMMENDATION with confidence level (High/Medium/Low)

Produce your synthesis and recommendation now."#;

/// A perspective in the debate with its configuration and state
#[derive(Debug, Clone)]
pub struct Perspective {
    /// Type of perspective
    pub perspective_type: PerspectiveType,
    /// Display name
    pub name: String,
    /// System prompt for this perspective
    pub system_prompt: String,
    /// Configuration specific to this perspective
    pub config: PerspectiveConfig,
    /// Message history for this perspective
    messages: Vec<Message>,
    /// Last response timestamp
    last_response: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerspectiveConfig {
    /// Maximum tokens for response
    pub max_tokens: u32,
    /// Temperature for sampling (0.0-1.0)
    pub temperature: f32,
    /// Model identifier
    pub model: Option<String>,
    /// Provider identifier
    pub provider: Option<String>,
    /// Priority for rate limiting (higher = more urgent)
    pub priority: u8,
}

impl Default for PerspectiveConfig {
    fn default() -> Self {
        Self {
            max_tokens: 1024,
            temperature: 0.7,
            model: None,
            provider: None,
            priority: 50,
        }
    }
}

impl Perspective {
    /// Create a new perspective of the given type
    pub fn new(perspective_type: PerspectiveType) -> Self {
        let (name, system_prompt) = match perspective_type {
            PerspectiveType::Advocate => {
                ("Advocate".to_string(), ADVOCATE_SYSTEM_PROMPT.to_string())
            }
            PerspectiveType::Critic => ("Critic".to_string(), CRITIC_SYSTEM_PROMPT.to_string()),
            PerspectiveType::Synthesizer => (
                "Synthesizer".to_string(),
                SYNTHESIZER_SYSTEM_PROMPT.to_string(),
            ),
        };

        Self {
            perspective_type,
            name,
            system_prompt,
            config: PerspectiveConfig::default(),
            messages: Vec::new(),
            last_response: None,
        }
    }

    /// Create an Advocate perspective
    pub fn advocate() -> Self {
        Self::new(PerspectiveType::Advocate)
    }

    /// Create a Critic perspective
    pub fn critic() -> Self {
        Self::new(PerspectiveType::Critic)
    }

    /// Create a Synthesizer perspective
    pub fn synthesizer() -> Self {
        Self::new(PerspectiveType::Synthesizer)
    }

    /// Create all three standard perspectives
    pub fn all_three() -> Vec<Self> {
        vec![Self::advocate(), Self::critic(), Self::synthesizer()]
    }

    /// Add a message to this perspective's history
    pub fn add_message(&mut self, role: Role, content: impl Into<String>) {
        self.messages.push(Message {
            role,
            content: vec![ContentBlock::Text {
                text: content.into(),
                cache_control: None,
            }],
            timestamp: Some(Utc::now()),
            tool_duration_ms: None,
        });
    }

    /// Get the message history
    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    /// Update last response timestamp
    pub fn set_last_response(&mut self) {
        self.last_response = Some(Utc::now());
    }

    /// Get last response time
    pub fn last_response(&self) -> Option<DateTime<Utc>> {
        self.last_response
    }

    /// Check if this perspective can proceed (rate limit check)
    pub fn can_proceed(&self, rate_limiter: &RateLimiterState) -> bool {
        rate_limiter.can_call(self.perspective_type)
    }

    /// Get the effective system prompt with topic context
    pub fn build_system_prompt(&self, topic: &DebateTopic) -> String {
        let mut prompt = self.system_prompt.clone();

        if let Some(context) = &topic.context {
            prompt.push_str("\n\n## Context\n");
            prompt.push_str(context);
        }

        if !topic.constraints.is_empty() {
            prompt.push_str("\n\n## Evaluation Criteria\n");
            for constraint in &topic.constraints {
                prompt.push_str("- ");
                prompt.push_str(constraint);
                prompt.push_str("\n");
            }
        }

        prompt.push_str("\n\n## The Proposal\n");
        prompt.push_str(&topic.question);

        prompt
    }

    /// Get the effective model for this perspective
    pub fn effective_model(&self) -> String {
        self.config
            .model
            .clone()
            .unwrap_or_else(|| "claude-sonnet-4-7".to_string())
    }

    /// Get the effective provider for this perspective
    pub fn effective_provider(&self) -> String {
        self.config
            .provider
            .clone()
            .unwrap_or_else(|| "anthropic".to_string())
    }

    /// Get the request priority for rate limiting
    pub fn priority(&self) -> u8 {
        self.config.priority
    }

    /// Clear message history
    pub fn clear_history(&mut self) {
        self.messages.clear();
        self.last_response = None;
    }
}

/// Minimal rate limiter state for perspective eligibility checks
#[derive(Debug, Clone)]
pub struct RateLimiterState {
    last_calls: std::collections::HashMap<PerspectiveType, DateTime<Utc>>,
    min_interval_secs: u64,
}

impl RateLimiterState {
    /// Create new rate limiter state
    pub fn new(min_interval_secs: u64) -> Self {
        Self {
            last_calls: std::collections::HashMap::new(),
            min_interval_secs,
        }
    }

    /// Check if a perspective can make a call
    pub fn can_call(&self, perspective: PerspectiveType) -> bool {
        match self.last_calls.get(&perspective) {
            Some(last) => {
                let elapsed = Utc::now().signed_duration_since(*last);
                elapsed.num_seconds() >= self.min_interval_secs as i64
            }
            None => true,
        }
    }

    /// Record a call for a perspective
    pub fn record_call(&mut self, perspective: PerspectiveType) {
        self.last_calls.insert(perspective, Utc::now());
    }
}

impl Default for RateLimiterState {
    fn default() -> Self {
        Self::new(1) // 1 second minimum between calls to same perspective
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn perspective_types_display() {
        assert_eq!(PerspectiveType::Advocate.to_string(), "advocate");
        assert_eq!(PerspectiveType::Critic.to_string(), "critic");
        assert_eq!(PerspectiveType::Synthesizer.to_string(), "synthesizer");
    }

    #[test]
    fn debate_topic_creation() {
        let topic = DebateTopic::new("Should we use Rust?");
        assert_eq!(topic.question, "Should we use Rust?");
        assert!(topic.context.is_none());
        assert!(topic.constraints.is_empty());

        let topic = DebateTopic::new("Should we use Rust?")
            .with_context("We're building a web service")
            .with_constraints(vec!["Performance".to_string(), "Safety".to_string()]);

        assert!(topic.context.is_some());
        assert_eq!(topic.constraints.len(), 2);
    }

    #[test]
    fn perspective_creation() {
        let advocate = Perspective::advocate();
        assert_eq!(advocate.perspective_type, PerspectiveType::Advocate);
        assert_eq!(advocate.name, "Advocate");
        // Check for "Advocate" in the system prompt (case insensitive)
        assert!(advocate.system_prompt.to_lowercase().contains("advocate"));

        let critic = Perspective::critic();
        assert_eq!(critic.perspective_type, PerspectiveType::Critic);

        let synthesizer = Perspective::synthesizer();
        assert_eq!(synthesizer.perspective_type, PerspectiveType::Synthesizer);
    }

    #[test]
    fn all_three_perspectives() {
        let perspectives = Perspective::all_three();
        assert_eq!(perspectives.len(), 3);

        let types: Vec<_> = perspectives.iter().map(|p| p.perspective_type).collect();
        assert!(types.contains(&PerspectiveType::Advocate));
        assert!(types.contains(&PerspectiveType::Critic));
        assert!(types.contains(&PerspectiveType::Synthesizer));
    }

    #[test]
    fn perspective_add_message() {
        let mut perspective = Perspective::advocate();
        perspective.add_message(Role::User, "The proposal is to adopt Rust");
        perspective.add_message(Role::Assistant, "I strongly agree because...");

        assert_eq!(perspective.messages().len(), 2);
        assert_eq!(perspective.messages()[0].role, Role::User);
        assert_eq!(perspective.messages()[1].role, Role::Assistant);
    }

    #[test]
    fn perspective_system_prompt_with_topic() {
        let perspective = Perspective::advocate();
        let topic = DebateTopic::new("Should we adopt microservices?")
            .with_context("Our monolith is becoming hard to maintain")
            .with_constraints(vec!["Maintainability".to_string()]);

        let prompt = perspective.build_system_prompt(&topic);
        assert!(prompt.contains("microservices"));
        assert!(prompt.contains("monolith"));
        assert!(prompt.contains("Maintainability"));
    }

    #[test]
    fn rate_limiter_state() {
        let state = RateLimiterState::new(5);

        // Initially can call any perspective
        assert!(state.can_call(PerspectiveType::Advocate));
        assert!(state.can_call(PerspectiveType::Critic));

        // Record calls
        let mut state = RateLimiterState::new(5);
        state.record_call(PerspectiveType::Advocate);

        // Should be able to call other perspectives
        assert!(state.can_call(PerspectiveType::Critic));
    }

    #[test]
    fn perspective_config_defaults() {
        let config = PerspectiveConfig::default();
        assert_eq!(config.max_tokens, 1024);
        assert_eq!(config.temperature, 0.7);
        assert_eq!(config.priority, 50);
    }
}
