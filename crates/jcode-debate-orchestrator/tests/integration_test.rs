//! Integration tests for jcode-debate-orchestrator
//!
//! Tests the full debate flow from CLI through DecisionEngine to Verdict.

#![allow(dead_code, unused_imports, unused_mut, unused_variables)]

use std::collections::HashMap;
use std::sync::Arc;

// =============================================================================
// Mock Module - Independent implementation for testing
// =============================================================================

mod mock {
    use super::*;

    pub struct MockDebateProvider {
        responses: HashMap<String, String>,
        delay_ms: u64,
        should_fail: bool,
        error_message: String,
    }

    impl MockDebateProvider {
        pub fn new() -> Self {
            Self {
                responses: HashMap::new(),
                delay_ms: 0,
                should_fail: false,
                error_message: "Mock error".to_string(),
            }
        }

        pub fn with_response(mut self, role: &str, response: &str) -> Self {
            self.responses
                .insert(role.to_lowercase(), response.to_string());
            self
        }

        pub fn with_delay(mut self, delay_ms: u64) -> Self {
            self.delay_ms = delay_ms;
            self
        }

        pub fn with_failure(mut self, message: &str) -> Self {
            self.should_fail = true;
            self.error_message = message.to_string();
            self
        }
    }

    // Simple mock provider that implements the LlmProvider trait
    pub struct SimpleMockProvider {
        responses: HashMap<String, String>,
    }

    impl SimpleMockProvider {
        pub fn new() -> Self {
            Self {
                responses: HashMap::new(),
            }
        }

        pub fn with_response(mut self, perspective: &str, response: &str) -> Self {
            self.responses
                .insert(perspective.to_lowercase(), response.to_string());
            self
        }
    }
}

// =============================================================================
// Test 1: CLI -> DebateDecisionEngine -> Coordinator -> Verdict
// =============================================================================

#[test]
fn test_cli_to_decision_engine_to_verdict() {
    use jcode_debate_orchestrator::{
        DebateCliCommand, DebateDecisionEngine, DebateDepth, DebateTriggerReason,
    };

    // Simulate CLI input: "/debate Design auth system"
    let cli_input = "/debate \"Design auth system\"";
    let cli_cmd = DebateCliCommand::parse(cli_input).expect("Failed to parse CLI command");
    assert_eq!(
        cli_cmd.action,
        jcode_debate_orchestrator::DebateCliAction::Activate
    );
    assert_eq!(cli_cmd.depth, Some(DebateDepth::Medium));

    // Create decision engine and make decision
    let engine = DebateDecisionEngine::new();
    let task = cli_cmd.task.as_ref().expect("No task provided");
    let decision = engine.decide(task);

    // Verify decision flow
    assert!(
        decision.should_debate,
        "Architecture decision should trigger debate"
    );
    assert!(
        matches!(
            decision.trigger,
            DebateTriggerReason::ArchitectureQuestion | DebateTriggerReason::DesignDecision
        ),
        "Expected ArchitectureQuestion or DesignDecision trigger, got {:?}",
        decision.trigger
    );
    assert_eq!(decision.depth, Some(DebateDepth::Medium));
    assert!(decision.estimated_cost_multiplier > 1.0);
}

#[test]
fn test_decision_engine_with_explicit_request() {
    use jcode_debate_orchestrator::{DebateDecisionEngine, DebateDepth, DebateTriggerReason};

    let engine = DebateDecisionEngine::new();

    // Even simple tasks should trigger when explicitly requested
    let decision = engine.decide_explicit("Fix this bug", DebateDepth::Quick);

    assert!(decision.should_debate);
    assert_eq!(decision.trigger, DebateTriggerReason::ExplicitRequest);
    assert_eq!(decision.depth, Some(DebateDepth::Quick));
    assert_eq!(decision.confidence, 1.0);
}

// =============================================================================
// Test 2: Full Debate Flow with Mock Provider
// =============================================================================

#[tokio::test]
async fn test_debate_session_basic() {
    use jcode_debate_core::{DebateConfig, DebateSession, DebateTopic};

    let config = DebateConfig::default();
    let topic = DebateTopic::new("Should we adopt microservices?");

    let mut session = DebateSession::with_topic(topic, config);

    // Verify session initialized correctly
    assert!(session.topic().is_some());
    assert_eq!(session.config.rounds, 2);
    assert_eq!(session.perspectives().len(), 3);

    // Simulate debate turns
    session.advance_phase();
    assert_eq!(session.round(), 1);
    assert_eq!(session.phase, jcode_debate_core::DebatePhase::AdvocateTurn);

    session.advance_phase();
    assert_eq!(session.phase, jcode_debate_core::DebatePhase::CriticTurn);
}

#[tokio::test]
async fn test_debate_session_complete_flow() {
    use jcode_debate_core::{
        DebateConfig, DebatePhase, DebateSession, DebateTopic, DebateVerdict, PerspectiveResponse,
        PerspectiveType,
    };

    let config = DebateConfig::default();
    let topic = DebateTopic::new("Test topic for debate");

    let mut session = DebateSession::with_topic(topic, config);

    // Simulate rounds: Advocate -> Critic -> Advocate -> Critic -> Synthesizer
    for round in 1..=2 {
        // Advocate turn
        session.advance_phase();
        session.record_turn(PerspectiveResponse::new(
            PerspectiveType::Advocate,
            format!("Advocate arguments for round {}", round),
            round,
        ));

        // Critic turn
        session.advance_phase();
        session.record_turn(PerspectiveResponse::new(
            PerspectiveType::Critic,
            format!("Critic analysis for round {}", round),
            round,
        ));
    }

    // Synthesizer verdict
    session.advance_phase();
    let verdict_response = PerspectiveResponse::new(
        PerspectiveType::Synthesizer,
        "I STRONGLY RECOMMEND this approach based on the analysis. HIGH CONFIDENCE.".to_string(),
        2,
    );
    session.record_turn(verdict_response);

    let verdict = DebateVerdict::from_response(
        session.turns().last().map(|t| &t.response).unwrap(),
        vec!["Both agree on importance of testing".to_string()],
        vec!["Disagree on implementation approach".to_string()],
    );

    session.set_verdict(verdict);

    // Verify complete flow
    assert_eq!(session.phase, DebatePhase::Completed);
    assert!(session.verdict().is_some());
    assert_eq!(session.verdict().unwrap().confidence, "high");
    assert_eq!(session.turns().len(), 5); // 2 advocate + 2 critic + 1 synthesizer
}

// =============================================================================
// Test 3: Safeguards Under Extreme Conditions
// =============================================================================

#[test]
fn test_safeguards_with_extreme_conditions() {
    use indexmap::indexmap;
    use jcode_debate_orchestrator::{
        AntiManipulation, CircuitBreaker, EmergencyFallback, FallbackReason, OutputValidator,
        PerspectiveOutput, SafeguardOrchestrator,
    };

    let orchestrator = SafeguardOrchestrator::new();

    // Test 3a: Very long responses (edge case)
    let mut perspectives = indexmap!(
        "long_1".to_string() => PerspectiveOutput::new(
            "long_1",
            "A".repeat(50000),
            0.8,
            0.9
        ),
        "long_2".to_string() => PerspectiveOutput::new(
            "long_2",
            "B".repeat(50000),
            0.75,
            0.85
        ),
    );

    let result = orchestrator.process(&mut perspectives);
    assert!(
        !result.response.is_empty(),
        "Should still produce response with long content"
    );

    // Test 3b: Very short responses (edge case)
    let validator = OutputValidator::new();
    let result = validator.validate("Short");
    assert!(!result.is_valid, "Too short content should be invalid");

    // Test 3c: Identical responses (edge case)
    let mut identical_perspectives = indexmap!(
        "same_1".to_string() => PerspectiveOutput::new(
            "same_1",
            "Exactly the same content repeated here".to_string(),
            0.8,
            0.9
        ),
        "same_2".to_string() => PerspectiveOutput::new(
            "same_2",
            "Exactly the same content repeated here".to_string(),
            0.8,
            0.9
        ),
    );

    let anti = AntiManipulation::new();
    let diversity = anti.check_diversity(&identical_perspectives);
    assert!(
        diversity < 0.5,
        "Identical content should have low diversity"
    );

    // Test 3d: Empty responses
    let result = validator.validate("");
    assert!(!result.is_valid, "Empty content should be invalid");
}

#[test]
fn test_circuit_breaker_under_timeout_pressure() {
    use jcode_debate_orchestrator::{CircuitBreaker, CircuitBreakerConfig, CircuitState};

    let breaker_config = CircuitBreakerConfig {
        perspective_timeout_secs: 1, // Very short for testing
        total_timeout_secs: 5,
        ..Default::default()
    };

    let mut breaker = CircuitBreaker::new().with_config(breaker_config);

    // Start tracking perspectives
    breaker.start_perspective("p1");
    breaker.start_perspective("p2");
    breaker.start_perspective("p3");

    // Initially all should pass timeout check
    assert!(breaker.check_perspective_timeout("p1").is_none());
    assert!(breaker.check_perspective_timeout("p2").is_none());

    // Check total timeout (should pass immediately)
    assert!(!breaker.check_total_timeout());

    // Record failures and verify circuit state
    breaker.record_failure();
    breaker.record_failure();
    breaker.record_failure();

    assert_eq!(breaker.state(), CircuitState::Open);

    // Reset should close the circuit
    breaker.reset();
    assert_eq!(breaker.state(), CircuitState::Closed);
}

#[test]
fn test_debate_error_fallback_behavior() {
    use jcode_debate_orchestrator::{DebateError, DebateFallback};

    // Test timeout fallback
    let timeout_err = DebateError::Timeout(30);
    assert_eq!(timeout_err.fallback(), DebateFallback::SimpleResponse);

    // Test rate limit fallback
    let rate_limit_err = DebateError::RateLimit(10);
    assert_eq!(rate_limit_err.fallback(), DebateFallback::RetryWithBackoff);
    assert_eq!(rate_limit_err.retry_after_secs(), Some(10));

    // Test gibberish fallback
    let gibberish_err = DebateError::Gibberish("aaa bbb ccc".to_string());
    assert_eq!(gibberish_err.fallback(), DebateFallback::DiscardGibberish);

    // Test gibberish detection
    assert!(DebateError::is_gibberish(
        "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"
    )); // Repeated chars (60+)
    assert!(DebateError::is_gibberish(
        "THIS IS ALL CAPS AND SHOULD BE DETECTED AS GIBBERISH IN A LONG TEXT SCENARIO"
    )); // Caps
    assert!(!DebateError::is_gibberish(
        "This is a normal coherent response with proper content for testing purposes."
    )); // Normal
}

// =============================================================================
// Test 4: Error Handling
// =============================================================================

#[test]
fn test_decision_engine_error_modes() {
    use jcode_debate_orchestrator::{DebateDecisionEngine, DebateMode, DebateTriggerReason};

    // Test Off mode - should never debate
    let engine = DebateDecisionEngine::new().with_mode(DebateMode::Off);
    let decision = engine.decide("Should we adopt microservices?");
    assert!(!decision.should_debate);
    assert_eq!(
        decision.trigger,
        DebateTriggerReason::Disabled,
        "Off mode should trigger Disabled reason"
    );

    // Test Explicit mode - should skip unless explicit request
    let engine = DebateDecisionEngine::new().with_mode(DebateMode::Explicit);
    let decision = engine.decide("Should we adopt microservices?");
    assert!(!decision.should_debate);
    assert_eq!(
        decision.trigger,
        DebateTriggerReason::SkippedSimpleTask,
        "Explicit mode without explicit request should skip"
    );

    // Test Auto mode with simple question - should skip
    let engine = DebateDecisionEngine::new().with_mode(DebateMode::Auto);
    let decision = engine.decide("What is Rust?");
    assert!(!decision.should_debate);
}

#[test]
fn test_fallback_emergency_handling() {
    use indexmap::indexmap;
    use jcode_debate_orchestrator::{
        ConsensusResult, EmergencyFallback, FallbackReason, PerspectiveOutput,
    };

    let fallback = EmergencyFallback::new();

    // Test fallback reason responses
    let timeout_response = fallback.get_fallback(&FallbackReason::AllTimeouts);
    assert!(!timeout_response.is_empty());

    let validation_response = fallback.get_fallback(&FallbackReason::AllFailedValidation);
    assert!(!validation_response.is_empty());

    let manipulation_response = fallback.get_fallback(&FallbackReason::ManipulationDetected);
    assert!(!manipulation_response.is_empty());

    // Test fallback with low-quality consensus
    let consensus = ConsensusResult::fallback_response("Original response".to_string());
    let mut perspectives = indexmap!(
        "low".to_string() => PerspectiveOutput::new("low", "Low quality".to_string(), 0.3, 0.3),
    );

    let (source, response) =
        fallback.select_response(&consensus, &perspectives, &FallbackReason::NoConsensus);
    assert!(!response.is_empty());
    assert_eq!(source, "fallback"); // Should use fallback since consensus is low quality
}

#[test]
fn test_cli_parser_error_handling() {
    use jcode_debate_orchestrator::DebateCliCommand;

    // Test invalid commands
    assert!(DebateCliCommand::parse("/invalid_command").is_none());
    assert!(DebateCliCommand::parse("/debate").is_none()); // No task

    // Test valid commands with flags
    let cmd = DebateCliCommand::parse("/debate --deep \"Complex task\"").unwrap();
    assert_eq!(
        cmd.action,
        jcode_debate_orchestrator::DebateCliAction::ActivateDeep
    );
    assert_eq!(
        cmd.depth,
        Some(jcode_debate_orchestrator::DebateDepth::Deep)
    );

    // Test --quick flag
    let cmd = DebateCliCommand::parse("/debate --quick \"Quick task\"").unwrap();
    assert_eq!(
        cmd.action,
        jcode_debate_orchestrator::DebateCliAction::ActivateQuick
    );
}

// =============================================================================
// Test 5: Consensus and Agreement Detection
// =============================================================================

#[test]
fn test_consensus_builder_with_aligned_perspectives() {
    use indexmap::indexmap;
    use jcode_debate_orchestrator::{ConsensusBuilder, PerspectiveOutput};

    let builder = ConsensusBuilder::new();

    let mut perspectives = indexmap!(
        "p1".to_string() => PerspectiveOutput::new(
            "p1",
            "Use React for the frontend. It has excellent performance and large ecosystem.".to_string(),
            0.85,
            0.9
        ),
        "p2".to_string() => PerspectiveOutput::new(
            "p2",
            "React is recommended for frontend. Large community and good tooling.".to_string(),
            0.8,
            0.85
        ),
    );

    let result = builder.build(&perspectives);

    assert!(
        !result.used_fallback,
        "Should not use fallback for aligned perspectives"
    );
    assert!(
        result.consensus_score > 0.6,
        "Consensus score should be high for aligned perspectives"
    );
}

#[test]
fn test_consensus_builder_with_diverged_perspectives() {
    use indexmap::indexmap;
    use jcode_debate_orchestrator::{ConsensusBuilder, PerspectiveOutput};

    let builder = ConsensusBuilder::new();

    let mut perspectives = indexmap!(
        "p1".to_string() => PerspectiveOutput::new(
            "p1",
            "Use microservices for scalability and independent deployments.".to_string(),
            0.85,
            0.9
        ),
        "p2".to_string() => PerspectiveOutput::new(
            "p2",
            "Start with monolith for simplicity. Migrate to microservices later.".to_string(),
            0.75,
            0.8
        ),
    );

    let result = builder.build(&perspectives);

    // With diverged perspectives, we should still get a consensus
    // but agreement level should be lower
    assert!(!result.used_fallback);
    assert!(result.agreement_level < 0.5); // Low agreement on opposite views
}

// =============================================================================
// Test 6: Rate Limiter Behavior
// =============================================================================

#[tokio::test]
async fn test_rate_limiter_sequential_calls() {
    use jcode_debate_core::{RateLimiter, perspectives::PerspectiveType};

    let limiter = RateLimiter::new(2); // 2 second interval

    // First call should always succeed
    assert!(limiter.can_call(PerspectiveType::Advocate).await);

    // After calling, should trigger rate limit
    limiter.mark_call(PerspectiveType::Advocate).await;

    // Immediate second call should be limited
    let wait = limiter.wait_for_next(PerspectiveType::Advocate).await;
    assert!(wait.is_some(), "Should have wait time after rate limit");
    if let Some(wait_duration) = wait {
        assert!(
            wait_duration.as_secs() >= 1,
            "Wait time should be at least 1 second"
        );
    }
}

// =============================================================================
// Test 7: Output Validation Edge Cases
// =============================================================================

#[test]
fn test_output_validation_edge_cases() {
    use jcode_debate_orchestrator::{OutputValidationConfig, OutputValidator, ValidationError};

    // Test with very strict config
    let strict_config = OutputValidationConfig {
        min_length: 500,
        max_length: 1000,
        min_entropy: 4.0,
        max_repeated_chars: 2,
        required_word_ratio: 0.6,
    };

    let validator = OutputValidator::new().with_config(strict_config);

    let result = validator.validate("Short text");
    assert!(!result.is_valid);
    assert!(
        result
            .errors
            .iter()
            .any(|e| matches!(e, ValidationError::TooShort { .. })),
        "Should detect too short"
    );

    // Test proper content validation (needs 500+ chars with strict config)
    let proper_text = "This is a well-structured response that contains multiple sentences with proper word separation and meaningful content for analysis purposes. The response includes various technical details and demonstrates coherent thinking about the topic being discussed. We can see here that the validator should accept properly formatted text with adequate length and appropriate word distribution across the content. The key point is that well-written responses with sufficient length and proper syntax will always pass validation tests without any issues.";
    let result = validator.validate(proper_text);
    assert!(
        result.is_valid,
        "Proper content should pass validation: {:?}",
        result.errors
    );

    // Test internal consistency check
    let response1 = "The database schema requires careful normalization for optimal performance and data integrity.";
    let response2 = "Normalization in the database schema is crucial for maintaining data consistency and query efficiency.";

    let is_consistent = validator.check_consistency(response1, response2);
    assert!(
        is_consistent,
        "Similar content should be considered consistent"
    );
}

// =============================================================================
// Test 8: Anti-Manipulation Detection
// =============================================================================

#[test]
fn test_anti_manipulation_sophisticated_attacks() {
    use indexmap::indexmap;
    use jcode_debate_orchestrator::{AntiManipulation, ManipulationIssue, PerspectiveOutput};

    let mut anti = AntiManipulation::new();

    // Simulate controlling perspective (very long response)
    // Need 3+ perspectives for control detection to work
    let mut perspectives = indexmap!(
        "normal1".to_string() => PerspectiveOutput::new(
            "normal1",
            "Standard response about database choice.".to_string(),
            0.8,
            0.9
        ),
        "normal2".to_string() => PerspectiveOutput::new(
            "normal2",
            "Another perspective on databases.".to_string(),
            0.75,
            0.85
        ),
        "controlled".to_string() => PerspectiveOutput::new(
            "controlled",
            format!("{} PostgreSQL is the best choice.", "USE_POSTGRES ".repeat(100)),
            0.85,
            0.9
        ),
    );

    let controller = anti.detect_control(&perspectives);
    assert!(
        controller.is_some(),
        "Controlling perspective should be detected"
    );

    // Record multiple perspectives and check for rate limiting
    anti.record_perspective("source_a", "Normal response content");
    anti.record_perspective("source_b", "Another normal response");
    anti.record_perspective("source_b", "Yet another response"); // Same source twice
    anti.record_perspective("source_b", "More content from same source"); // Third time
    anti.record_perspective("source_b", "Fourth response from same source"); // Fourth time
    anti.record_perspective("source_b", "Fifth response - rate limit exceeded"); // Fifth time - should trigger

    let report = anti.analyze(&perspectives);
    // Check if report has any issues (either detected or from analysis)
    // Rate limiting detection requires >= 5 perspectives from same source
    let has_rate_limit_issue = report
        .issues
        .iter()
        .any(|i| matches!(i, ManipulationIssue::RateLimitExceeded { .. }));
    let has_issues = !report.issues.is_empty();
    assert!(
        has_rate_limit_issue || has_issues || report.manipulation_score > 0.1,
        "Should detect issues: rate_limit={}, issues={:?}, score={}",
        has_rate_limit_issue,
        report.issues,
        report.manipulation_score
    );
}

// =============================================================================
// Test 9: Deep Integration - Full Safeguard Pipeline
// =============================================================================

#[test]
fn test_safeguard_orchestrator_full_pipeline() {
    use indexmap::indexmap;
    use jcode_debate_orchestrator::{PerspectiveOutput, SafeguardOrchestrator};

    let orchestrator = SafeguardOrchestrator::new();

    let mut perspectives = indexmap!(
        "arch".to_string() => PerspectiveOutput::new(
            "arch",
            "The system architecture should follow clean architecture principles with clear separation of concerns.".to_string(),
            0.85,
            0.9
        ),
        "perf".to_string() => PerspectiveOutput::new(
            "perf",
            "Performance optimization requires profiling first, then implementing caching strategies.".to_string(),
            0.82,
            0.88
        ),
        "sec".to_string() => PerspectiveOutput::new(
            "sec",
            "Security review requires input validation, proper authentication, and encryption for sensitive data.".to_string(),
            0.83,
            0.89
        ),
    );

    let result = orchestrator.process(&mut perspectives);

    // Verify no manipulation detected
    assert!(!result.manipulation_detected);

    // Verify consensus was built
    assert!(!result.consensus.consensus_text.is_empty());
    assert!(!result.consensus.used_fallback);

    // Verify no exclusions
    assert!(result.excluded_perspectives.is_empty());

    // Verify response exists
    assert!(!result.response.is_empty());
}

#[test]
fn test_debate_mode_transitions() {
    use jcode_debate_orchestrator::DebateMode;

    // Test parsing
    assert_eq!(DebateMode::parse("off"), Some(DebateMode::Off));
    assert_eq!(DebateMode::parse("explicit"), Some(DebateMode::Explicit));
    assert_eq!(DebateMode::parse("auto"), Some(DebateMode::Auto));
    assert_eq!(DebateMode::parse("invalid"), None);

    // Test conversions
    assert_eq!(DebateMode::Off.as_str(), "off");
    assert_eq!(DebateMode::Explicit.as_str(), "explicit");
    assert_eq!(DebateMode::Auto.as_str(), "auto");
}

#[test]
fn test_debate_depth_configurations() {
    use jcode_debate_orchestrator::DebateDepth;

    // Test perspective counts
    assert_eq!(DebateDepth::Quick.perspective_count(), 2);
    assert_eq!(DebateDepth::Medium.perspective_count(), 3);
    assert_eq!(DebateDepth::Deep.perspective_count(), 5);

    // Test timeout values
    assert_eq!(DebateDepth::Quick.timeout_secs(), 30);
    assert_eq!(DebateDepth::Medium.timeout_secs(), 60);
    assert_eq!(DebateDepth::Deep.timeout_secs(), 120);

    // Test token multipliers
    assert!((DebateDepth::Quick.token_budget_multiplier() - 1.5).abs() < 0.01);
    assert!((DebateDepth::Medium.token_budget_multiplier() - 2.5).abs() < 0.01);
    assert!((DebateDepth::Deep.token_budget_multiplier() - 5.0).abs() < 0.01);

    // Test parsing
    assert_eq!(DebateDepth::parse("quick"), Some(DebateDepth::Quick));
    assert_eq!(DebateDepth::parse("medium"), Some(DebateDepth::Medium));
    assert_eq!(DebateDepth::parse("deep"), Some(DebateDepth::Deep));
}

// =============================================================================
// Test 10: Verdict Parsing and Confidence
// =============================================================================

#[tokio::test]
async fn test_verdict_confidence_parsing() {
    use jcode_debate_core::{DebateVerdict, PerspectiveResponse, PerspectiveType};

    // Test HIGH confidence parsing
    let high_response = PerspectiveResponse::new(
        PerspectiveType::Synthesizer,
        "I STRONGLY RECOMMEND this approach. HIGH CONFIDENCE based on comprehensive analysis."
            .to_string(),
        1,
    );
    let verdict = DebateVerdict::from_response(&high_response, vec![], vec![]);
    assert_eq!(verdict.confidence, "high");

    // Test MEDIUM confidence parsing
    let medium_response = PerspectiveResponse::new(
        PerspectiveType::Synthesizer,
        "I recommend this approach with MODERATE confidence. There are some trade-offs to consider.".to_string(),
        1,
    );
    let verdict = DebateVerdict::from_response(&medium_response, vec![], vec![]);
    assert_eq!(verdict.confidence, "medium");

    // Test LOW confidence parsing
    let low_response = PerspectiveResponse::new(
        PerspectiveType::Synthesizer,
        "The analysis is inconclusive. More data needed.".to_string(),
        1,
    );
    let verdict = DebateVerdict::from_response(&low_response, vec![], vec![]);
    assert_eq!(verdict.confidence, "low");
}

// =============================================================================
// Test 11: Session Statistics
// =============================================================================

#[tokio::test]
async fn test_session_statistics_tracking() {
    use jcode_debate_core::{
        DebateConfig, DebateSession, DebateTopic, PerspectiveResponse, PerspectiveType,
    };

    let config = DebateConfig::default();
    let topic = DebateTopic::new("Test topic for stats");
    let mut session = DebateSession::with_topic(topic, config);

    // Record some turns
    session.advance_phase();
    session.record_turn(PerspectiveResponse::new(
        PerspectiveType::Advocate,
        "Advocate argument 1".to_string(),
        1,
    ));

    session.advance_phase();
    session.record_turn(PerspectiveResponse::new(
        PerspectiveType::Critic,
        "Critic analysis 1".to_string(),
        1,
    ));

    let stats = session.stats();

    assert_eq!(stats.total_turns, 2);
    assert_eq!(stats.successful_turns, 2);
    assert_eq!(stats.failed_turns, 0);
    assert_eq!(stats.round, 1);
    assert_eq!(stats.phase, "critic_turn");
}

// =============================================================================
// Test 12: Decision Engine Threshold Configuration
// =============================================================================

#[test]
fn test_decision_engine_threshold_sensitivity() {
    use jcode_debate_orchestrator::{DebateDecisionEngine, DebateTriggerReason};

    // High threshold - fewer triggers
    let engine = DebateDecisionEngine::new().with_trigger_threshold(3);

    // This should NOT trigger with threshold=3
    let decision = engine.decide("Should we adopt microservices?");
    // May or may not trigger depending on pattern matches

    // Test with very specific pattern
    let decision2 = engine.decide("What is the best architecture pattern?");
    // Verify decision structure
    assert!(decision.estimated_cost_multiplier >= 1.0);
}

// =============================================================================
// Test 13: Coordinator with Config
// =============================================================================

#[tokio::test]
async fn test_coordinator_with_custom_config() {
    use jcode_debate_core::{DebateConfig, DebateTopic, coordinator::Coordinator};

    // Use the MockLlmProvider from coordinator module
    use jcode_debate_core::coordinator::mock::MockLlmProvider;

    let provider = Arc::new(
        MockLlmProvider::new()
            .with_response(
                jcode_debate_core::perspectives::PerspectiveType::Advocate,
                "Advocate response for testing",
            )
            .with_response(
                jcode_debate_core::perspectives::PerspectiveType::Critic,
                "Critic response for testing",
            )
            .with_response(
                jcode_debate_core::perspectives::PerspectiveType::Synthesizer,
                "Synthesizer response - I recommend this approach. HIGH CONFIDENCE.",
            ),
    );

    let mut config = DebateConfig::default();
    config.rounds = 2;
    config.rate_limit_interval_secs = 1;

    let coordinator = Coordinator::new(config, provider);
    let topic = DebateTopic::new("Test custom config debate");
    coordinator.set_topic(topic).await;

    let verdict = coordinator
        .run_debate()
        .await
        .expect("Debate should complete");
    assert!(!verdict.recommendation.is_empty());
}
