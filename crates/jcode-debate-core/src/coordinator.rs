//! # Coordinator Module
//!
//! Orchestrates the multi-perspective debate flow between perspectives.
//! Coordinates API calls, manages rate limits, and synthesizes results.

use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{mpsc, RwLock};
use tokio::time::sleep;

use jcode_message_types::{ContentBlock, Message, Role};
use tracing::{debug, info};

use crate::debate_session::{
    DebateConfig, DebatePhase, DebateSession, DebateVerdict, PerspectiveResponse,
};
use crate::perspectives::{DebateTopic, Perspective, PerspectiveType};
use crate::rate_limiter::RateLimiter;
use crate::DebateResult;

use super::DebateError;

/// Provider trait for making LLM calls
#[async_trait::async_trait]
pub trait LlmProvider: Send + Sync {
    /// Complete a prompt and return the response text
    async fn complete(
        &self,
        messages: &[Message],
        system: &str,
        max_tokens: u32,
        temperature: f32,
    ) -> DebateResult<String>;
}

/// The coordinator manages the debate flow and orchestrates perspectives
pub struct Coordinator {
    /// Session being coordinated
    session: Arc<RwLock<DebateSession>>,
    /// Rate limiter
    rate_limiter: Arc<RateLimiter>,
    /// LLM provider
    llm_provider: Arc<dyn LlmProvider>,
    /// Event sender for real-time updates
    event_sender: Option<mpsc::UnboundedSender<DebateEvent>>,
}

/// Events emitted by the coordinator
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DebateEvent {
    /// A new round has started
    RoundStarted { round: u32 },
    /// A perspective has started speaking
    PerspectiveStarted { perspective: PerspectiveType },
    /// A perspective has finished speaking
    PerspectiveFinished {
        perspective: PerspectiveType,
        response: PerspectiveResponse,
    },
    /// A perspective failed to respond
    PerspectiveFailed {
        perspective: PerspectiveType,
        error: String,
    },
    /// The debate has completed
    DebateCompleted { verdict: DebateVerdict },
    /// An error occurred
    Error { message: String },
    /// Rate limited, waiting
    RateLimited {
        perspective: PerspectiveType,
        wait_secs: u64,
    },
}

impl Coordinator {
    /// Create a new coordinator
    pub fn new(config: DebateConfig, llm_provider: Arc<dyn LlmProvider>) -> Self {
        let session = Arc::new(RwLock::new(DebateSession::new(config.clone())));

        Self {
            session,
            rate_limiter: Arc::new(RateLimiter::new(config.rate_limit_interval_secs)),
            llm_provider,
            event_sender: None,
        }
    }

    /// Create with a custom event sender
    pub fn with_events(
        config: DebateConfig,
        llm_provider: Arc<dyn LlmProvider>,
        event_sender: mpsc::UnboundedSender<DebateEvent>,
    ) -> Self {
        Self {
            session: Arc::new(RwLock::new(DebateSession::new(config.clone()))),
            rate_limiter: Arc::new(RateLimiter::new(config.rate_limit_interval_secs)),
            llm_provider,
            event_sender: Some(event_sender),
        }
    }

    /// Set the debate topic
    pub async fn set_topic(&self, topic: DebateTopic) {
        let mut session = self.session.write().await;
        session.set_topic(topic);
        self.emit(DebateEvent::RoundStarted { round: 0 });
    }

    /// Run the full debate
    pub async fn run_debate(&self) -> DebateResult<DebateVerdict> {
        info!("Starting debate");

        {
            let session = self.session.read().await;
            if session.topic().is_none() {
                return Err(DebateError::InvalidState(
                    "No topic set. Call set_topic() first.".to_string(),
                ));
            }
        }

        // Run advocate-critic rounds
        for round in 1..=self.config().rounds {
            info!("Starting round {}", round);
            self.emit(DebateEvent::RoundStarted { round });

            // Advocate speaks
            self.run_perspective(PerspectiveType::Advocate, round)
                .await?;

            // Wait for rate limit clearance for critic
            self.wait_for_rate_limit(PerspectiveType::Critic).await?;

            // Critic speaks
            self.run_perspective(PerspectiveType::Critic, round).await?;
        }

        // Synthesizer provides final verdict
        info!("Running synthesizer for final verdict");
        let verdict = self.run_synthesizer().await?;

        self.emit(DebateEvent::DebateCompleted {
            verdict: verdict.clone(),
        });

        info!("Debate completed with verdict: {}", verdict.recommendation);
        Ok(verdict)
    }

    /// Run a single perspective
    async fn run_perspective(
        &self,
        perspective_type: PerspectiveType,
        round: u32,
    ) -> DebateResult<PerspectiveResponse> {
        info!("Running {} for round {}", perspective_type, round);
        self.emit(DebateEvent::PerspectiveStarted {
            perspective: perspective_type,
        });

        let start = Instant::now();

        // Build the prompt
        let (system_prompt, user_prompt) = self.build_perspective_prompt(perspective_type).await?;

        // Make the API call
        let call_result = self
            .llm_provider
            .complete(
                &[Message {
                    role: Role::User,
                    content: vec![ContentBlock::Text {
                        text: user_prompt,
                        cache_control: None,
                    }],
                    timestamp: Some(chrono::Utc::now()),
                    tool_duration_ms: None,
                }],
                &system_prompt,
                self.config().max_tokens,
                self.config().temperature,
            )
            .await;

        let duration = start.elapsed();

        // Process the result
        match call_result {
            Ok(text) => {
                let mut resp = PerspectiveResponse::new(perspective_type, text, round);
                resp.duration_ms = Some(duration.as_millis() as u64);

                // Record the turn
                {
                    let mut session = self.session.write().await;
                    session.record_turn(resp.clone());
                    session.advance_phase();
                }

                self.emit(DebateEvent::PerspectiveFinished {
                    perspective: perspective_type,
                    response: resp.clone(),
                });

                self.rate_limiter.mark_call(perspective_type).await;

                // Wait if we need to avoid rate limits
                if let Some(wait) = self.rate_limiter.wait_for_next(perspective_type).await {
                    debug!("Rate limiting: waiting {}ms", wait.as_millis());
                    sleep(wait).await;
                }

                Ok(resp)
            }
            Err(e) => {
                let error_msg = e.to_string();

                {
                    let mut session = self.session.write().await;
                    session.record_error(perspective_type, error_msg.clone());
                }

                self.emit(DebateEvent::PerspectiveFailed {
                    perspective: perspective_type,
                    error: error_msg,
                });

                Err(e)
            }
        }
    }

    /// Run the synthesizer to produce final verdict
    async fn run_synthesizer(&self) -> DebateResult<DebateVerdict> {
        {
            let mut session = self.session.write().await;
            session.advance_phase();
        }

        self.emit(DebateEvent::PerspectiveStarted {
            perspective: PerspectiveType::Synthesizer,
        });

        let start = Instant::now();

        // Build synthesis prompt with all previous responses
        let (system_prompt, user_prompt) = self.build_synthesis_prompt().await?;

        let result = self
            .llm_provider
            .complete(
                &[Message {
                    role: Role::User,
                    content: vec![ContentBlock::Text {
                        text: user_prompt,
                        cache_control: None,
                    }],
                    timestamp: Some(chrono::Utc::now()),
                    tool_duration_ms: None,
                }],
                &system_prompt,
                self.config().max_tokens * 2, // Synthesizer needs more space
                0.5,                          // Lower temperature for synthesis
            )
            .await;

        let duration = start.elapsed();

        match result {
            Ok(text) => {
                // Extract agreements and disagreements from previous turns
                let (agreements, disagreements) = self.extract_agreement_disagreement().await;

                let response = PerspectiveResponse::new(
                    PerspectiveType::Synthesizer,
                    text.clone(),
                    self.session.read().await.round(),
                );
                let response = response.with_duration(duration.as_millis() as u64);

                let verdict = DebateVerdict::from_response(&response, agreements, disagreements);

                {
                    let mut session = self.session.write().await;
                    session.record_turn(response.clone());
                    session.set_verdict(verdict.clone());
                }

                self.emit(DebateEvent::PerspectiveFinished {
                    perspective: PerspectiveType::Synthesizer,
                    response,
                });

                Ok(verdict)
            }
            Err(e) => {
                let error_msg = e.to_string();
                {
                    let mut session = self.session.write().await;
                    session.record_error(PerspectiveType::Synthesizer, error_msg.clone());
                }

                self.emit(DebateEvent::PerspectiveFailed {
                    perspective: PerspectiveType::Synthesizer,
                    error: error_msg,
                });

                Err(e)
            }
        }
    }

    /// Build the prompt for a perspective
    async fn build_perspective_prompt(
        &self,
        perspective_type: PerspectiveType,
    ) -> DebateResult<(String, String)> {
        let session = self.session.read().await;
        let topic = session
            .topic()
            .ok_or_else(|| DebateError::InvalidState("No topic set".to_string()))?;

        let perspective = match perspective_type {
            PerspectiveType::Advocate => Perspective::advocate(),
            PerspectiveType::Critic => Perspective::critic(),
            PerspectiveType::Synthesizer => {
                return Err(DebateError::InvalidState(
                    "Use build_synthesis_prompt for synthesizer".to_string(),
                ))
            }
        };

        let system_prompt = perspective.build_system_prompt(topic);

        let round = session.round();
        let history = self.build_history_summary(&session, perspective_type);

        let user_prompt = if round == 1 {
            format!(
                "{}\n\nRound {}: Present your initial {} perspective on this proposal.",
                history, round, perspective_type
            )
        } else {
            format!(
                "{}\n\nRound {}: Continue your {} perspective, building upon or countering what has been said.",
                history,
                round,
                perspective_type
            )
        };

        Ok((system_prompt, user_prompt))
    }

    /// Build the prompt for synthesis
    async fn build_synthesis_prompt(&self) -> DebateResult<(String, String)> {
        let session = self.session.read().await;
        let topic = session
            .topic()
            .ok_or_else(|| DebateError::InvalidState("No topic set".to_string()))?;

        let synthesizer = Perspective::synthesizer();
        let system_prompt = synthesizer.build_system_prompt(topic);

        let mut history_lines = vec![format!("## Proposal: {}", topic.question)];
        if let Some(context) = &topic.context {
            history_lines.push(format!("\n## Context: {}", context));
        }

        history_lines.push("\n## Debate History:".to_string());

        for turn in session.turns() {
            if turn.response.text.is_empty() {
                continue;
            }
            let role = match turn.response.perspective_type {
                PerspectiveType::Advocate => "ADVOCATE",
                PerspectiveType::Critic => "CRITIC",
                PerspectiveType::Synthesizer => "SYNTHESIS",
            };
            history_lines.push(format!("\n[{} - Round {}]", role, turn.round));
            history_lines.push(turn.response.text.clone());
        }

        let user_prompt = format!(
            "{}\n\n## Your Task\nAnalyze the above debate and produce a comprehensive synthesis with:\n1. Summary of each perspective's main arguments\n2. Key areas of agreement and disagreement\n3. Evaluation against the criteria\n4. Clear RECOMMENDATION with confidence level (High/Medium/Low)",
            history_lines.join("\n")
        );

        Ok((system_prompt, user_prompt))
    }

    /// Build a summary of debate history for prompting
    fn build_history_summary(
        &self,
        session: &DebateSession,
        perspective_type: PerspectiveType,
    ) -> String {
        let mut lines = vec![String::new()];

        lines.push("## Previous Debate Responses:".to_string());

        for turn in session.turns() {
            if turn.response.text.is_empty() {
                continue;
            }

            let is_same_perspective = turn.response.perspective_type == perspective_type;
            let role = match turn.response.perspective_type {
                PerspectiveType::Advocate => "ADVOCATE",
                PerspectiveType::Critic => "CRITIC",
                PerspectiveType::Synthesizer => "SYNTHESIZER",
            };

            if is_same_perspective {
                lines.push(format!(
                    "\n[Your previous {} response in Round {}]",
                    role, turn.round
                ));
            } else {
                lines.push(format!("\n[{} response in Round {}]", role, turn.round));
            }
            lines.push(turn.response.text.clone());
        }

        lines.join("\n")
    }

    /// Extract areas of agreement and disagreement from debate
    async fn extract_agreement_disagreement(&self) -> (Vec<String>, Vec<String>) {
        let session = self.session.read().await;

        let mut agreements = Vec::new();
        let disagreements = Vec::new();

        // Find common positive and negative words in advocate/critic responses
        let advocate_text: String = session
            .responses_for(PerspectiveType::Advocate)
            .iter()
            .map(|r| r.text.to_lowercase())
            .collect();

        let critic_text: String = session
            .responses_for(PerspectiveType::Critic)
            .iter()
            .map(|r| r.text.to_lowercase())
            .collect();

        // Simple heuristic: check for overlapping concerns
        let topics = [
            "performance",
            "maintainability",
            "safety",
            "cost",
            "complexity",
            "risk",
        ];

        for topic in topics {
            let in_advocate = advocate_text.contains(topic);
            let in_critic = critic_text.contains(topic);

            if in_advocate && in_critic {
                agreements.push(format!("Both agree on {} implications", topic));
            } else if in_advocate && !critic_text.contains(&format!("not {}", topic)) {
                // Advocate mentions it positively
            } else if in_critic && !advocate_text.contains(&format!("not {}", topic)) {
                // Critic mentions concerns
            }
        }

        (agreements, disagreements)
    }

    /// Wait for rate limit to clear
    async fn wait_for_rate_limit(&self, perspective_type: PerspectiveType) -> DebateResult<()> {
        if let Some(wait) = self.rate_limiter.wait_for_next(perspective_type).await {
            self.emit(DebateEvent::RateLimited {
                perspective: perspective_type,
                wait_secs: wait.as_secs(),
            });
            sleep(wait).await;
        }
        Ok(())
    }

    /// Get current config
    fn config(&self) -> DebateConfig {
        // We need to clone, so we use a blocking read
        // In async context this is fine
        futures::executor::block_on(async { self.session.read().await.config.clone() })
    }

    /// Get session stats
    pub async fn stats(&self) -> crate::debate_session::DebateSessionStats {
        self.session.read().await.stats()
    }

    /// Get current phase
    pub async fn phase(&self) -> DebatePhase {
        self.session.read().await.phase
    }

    /// Emit an event if we have a sender
    fn emit(&self, event: DebateEvent) {
        if let Some(sender) = &self.event_sender {
            let _ = sender.send(event);
        }
    }

    /// Get the underlying session (for advanced use)
    pub async fn session(&self) -> Arc<RwLock<DebateSession>> {
        self.session.clone()
    }
}

/// Mock LLM provider for testing
#[cfg(test)]
pub mod mock {
    use super::*;

    pub struct MockLlmProvider {
        responses: std::collections::HashMap<PerspectiveType, String>,
        delay_ms: u64,
    }

    impl MockLlmProvider {
        pub fn new() -> Self {
            Self {
                responses: std::collections::HashMap::new(),
                delay_ms: 0,
            }
        }

        pub fn with_response(mut self, perspective: PerspectiveType, response: &str) -> Self {
            self.responses.insert(perspective, response.to_string());
            self
        }

        pub fn with_delay(mut self, delay_ms: u64) -> Self {
            self.delay_ms = delay_ms;
            self
        }
    }

    #[async_trait::async_trait]
    impl LlmProvider for MockLlmProvider {
        async fn complete(
            &self,
            _messages: &[Message],
            _system: &str,
            _max_tokens: u32,
            _temperature: f32,
        ) -> DebateResult<String> {
            if self.delay_ms > 0 {
                sleep(std::time::Duration::from_millis(self.delay_ms)).await;
            }

            // Try to find a response in order: Synthesizer, Critic, Advocate
            // This simulates the coordinator calling perspectives in sequence
            if let Some(response) = self.responses.get(&PerspectiveType::Synthesizer) {
                return Ok(response.clone());
            }
            if let Some(response) = self.responses.get(&PerspectiveType::Critic) {
                return Ok(response.clone());
            }
            if let Some(response) = self.responses.get(&PerspectiveType::Advocate) {
                return Ok(response.clone());
            }

            Ok("Mock response".to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::mock::*;
    use super::*;

    #[tokio::test]
    async fn coordinator_creation() {
        let provider = Arc::new(
            MockLlmProvider::new()
                .with_response(PerspectiveType::Advocate, "I strongly advocate for this"),
        );
        let coordinator = Coordinator::new(DebateConfig::default(), provider);
        assert_eq!(coordinator.config().rounds, 2);
    }

    #[tokio::test]
    async fn coordinator_set_topic() {
        let provider = Arc::new(MockLlmProvider::new());
        let coordinator = Coordinator::new(DebateConfig::default(), provider);

        let topic = DebateTopic::new("Should we adopt Rust?");
        coordinator.set_topic(topic.clone()).await;

        let session_lock = coordinator.session().await;
        let session = session_lock.read().await;
        assert!(session.topic().is_some());
    }

    #[tokio::test]
    async fn coordinator_run_debate() {
        // Use responses that will produce "high" confidence
        let provider = Arc::new(
            MockLlmProvider::new()
                .with_response(
                    PerspectiveType::Advocate,
                    "Advocate argues strongly for this approach",
                )
                .with_response(PerspectiveType::Critic, "Critic identifies potential risks")
                .with_response(
                    PerspectiveType::Synthesizer,
                    "I STRONGLY RECOMMEND this approach. HIGH CONFIDENCE based on the evidence.",
                ),
        );

        let coordinator = Coordinator::new(DebateConfig::default(), provider);

        let topic = DebateTopic::new("Should we adopt Rust?");
        coordinator.set_topic(topic).await;

        let verdict = coordinator.run_debate().await.unwrap();
        // The verdict confidence should be parsed from the text
        assert!(verdict.confidence == "high" || verdict.confidence == "medium");
    }

    #[tokio::test]
    async fn coordinator_without_topic_fails() {
        let provider = Arc::new(MockLlmProvider::new());
        let coordinator = Coordinator::new(DebateConfig::default(), provider);

        let result = coordinator.run_debate().await;
        assert!(result.is_err());
    }
}
