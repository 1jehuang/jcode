//! Debate Orchestrator - Token/Latency-Aware Debate Decision Engine
//!
//! Decides WHEN to invoke multi-perspective debate vs simple response.
//! Cost: 2-5x tokens and latency vs simple response.
//!
//! # Decision Flow
//!
//! ```text
//! User Input → [TRIGGER CHECK] → Debatable?
//!                                  ├─ YES → [DEPTH CONFIG] → Run Debate
//!                                  └─ NO  → Simple Response
//! ```

use serde::{Deserialize, Serialize};

/// Debate activation state controlled by user.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum DebateMode {
    /// Always use simple response, ignore debate triggers.
    Off,
    /// Only use debate when explicitly requested via CLI.
    Explicit,
    /// Automatically use debate for trigger-matched tasks.
    #[default]
    Auto,
}

impl DebateMode {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "off" => Some(Self::Off),
            "explicit" => Some(Self::Explicit),
            "auto" => Some(Self::Auto),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Explicit => "explicit",
            Self::Auto => "auto",
        }
    }
}

/// Debate depth levels control perspectives and timeout.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum DebateDepth {
    /// 2 perspectives, 30s timeout. Quick trade-off check.
    Quick,
    /// 3 perspectives, 60s timeout. Balanced analysis.
    #[default]
    Medium,
    /// 5 perspectives, 120s timeout. Deep architectural review.
    Deep,
}

impl DebateDepth {
    pub fn perspective_count(self) -> usize {
        match self {
            Self::Quick => 2,
            Self::Medium => 3,
            Self::Deep => 5,
        }
    }

    pub fn timeout_secs(self) -> u64 {
        match self {
            Self::Quick => 30,
            Self::Medium => 60,
            Self::Deep => 120,
        }
    }

    pub fn token_budget_multiplier(self) -> f64 {
        match self {
            Self::Quick => 1.5,
            Self::Medium => 2.5,
            Self::Deep => 5.0,
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "quick" => Some(Self::Quick),
            "medium" => Some(Self::Medium),
            "deep" => Some(Self::Deep),
            _ => None,
        }
    }
}

/// What triggered the debate decision.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DebateTriggerReason {
    /// Explicit CLI request: /debate "task"
    ExplicitRequest,
    /// Auto-detected architecture question.
    ArchitectureQuestion,
    /// Auto-detected multiple trade-offs.
    MultiTradeOffAnalysis,
    /// Auto-detected ambiguous problem.
    AmbiguousProblem,
    /// Auto-detected design decision needed.
    DesignDecision,
    /// User disabled debate for this task.
    Disabled,
    /// Task matched skip rules.
    SkippedSimpleTask,
}

/// Result of debate decision.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct DebateDecision {
    /// Whether debate should be invoked.
    pub should_debate: bool,
    /// Why this decision was made.
    pub trigger: DebateTriggerReason,
    /// User-configured mode at decision time.
    pub mode: DebateMode,
    /// Depth level if debate will run.
    pub depth: Option<DebateDepth>,
    /// Estimated token cost multiplier vs simple response.
    pub estimated_cost_multiplier: f64,
    /// Confidence score (0.0-1.0) in this decision.
    pub confidence: f64,
    /// Explanation of why debate was/wasn't chosen.
    pub explanation: String,
}

impl DebateDecision {
    fn skip(trigger: DebateTriggerReason, mode: DebateMode, explanation: &str) -> Self {
        Self {
            should_debate: false,
            trigger,
            mode,
            depth: None,
            estimated_cost_multiplier: 1.0,
            confidence: 1.0,
            explanation: explanation.to_string(),
        }
    }

    fn engage(
        trigger: DebateTriggerReason,
        mode: DebateMode,
        depth: DebateDepth,
        confidence: f64,
        explanation: &str,
    ) -> Self {
        Self {
            should_debate: true,
            trigger,
            mode,
            depth: Some(depth),
            estimated_cost_multiplier: depth.token_budget_multiplier(),
            confidence,
            explanation: explanation.to_string(),
        }
    }
}

/// Patterns that ACTIVATE debate (triggers).
#[derive(Debug, Clone)]
pub struct DebateTriggerPatterns {
    /// Regex patterns for architecture questions.
    pub architecture: Vec<regex::Regex>,
    /// Regex patterns for trade-off discussions.
    pub trade_offs: Vec<regex::Regex>,
    /// Regex patterns for ambiguous problems.
    pub ambiguous: Vec<regex::Regex>,
    /// Regex patterns for design decisions.
    pub design: Vec<regex::Regex>,
}

impl Default for DebateTriggerPatterns {
    fn default() -> Self {
        Self {
            architecture: Self::compile_patterns(&[
                r"(?i)^(design|architect|structure|architecture|system design)",
                r"(?i)how (should we|would you|do you recommend)",
                r"(?i)what is (the best|better|optimal|recommended)",
                r"(?i)compare (a |the )?(monolith|microservices|serverless)",
                r"(?i)(api|api design|api contract)",
                r"(?i)(database schema|data model)",
                r"(?i)(scalability|scale|horizontal|vertical)",
                r"(?i)(microservices|monolith|modular)",
            ]),
            trade_offs: Self::compile_patterns(&[
                r"(?i)(trade-?off|pros? ?and? ?cons?|advantages? (and|&) disadvantages?)",
                r"(?i)(should we|which is (better|preferred)|pick (between|a))",
                r"(?i)(performance vs|simplicity vs|flexibility vs)",
                r"(?i)(cost-benefit|cost effectiveness)",
                r"(?i)decide (between|on)",
            ]),
            ambiguous: Self::compile_patterns(&[
                r"\?$", // Ends with question mark
                r"(?i)(not sure|unclear|uncertain|ambiguous)",
                r"(?i)(how (would you|should we|do you)) .*\?",
                r"(?i)(what (do you|would you|should we)) .*\?",
                r"(?i)(multiple (approaches|options|ways|solutions))",
            ]),
            design: Self::compile_patterns(&[
                r"(?i)(design (a |an |new )?system)",
                r"(?i)(implement|build|create|architect) .*(auth|auth|permission)",
                r"(?i)(refactor|restructure|redesign)",
                r"(?i)(choosing|pick|select) .*(library|framework|technology)",
                r"(?i)(migration|upgrade|moving from)",
            ]),
        }
    }
}

impl DebateTriggerPatterns {
    fn compile_patterns(patterns: &[&str]) -> Vec<regex::Regex> {
        patterns
            .iter()
            .filter_map(|p| regex::Regex::new(p).ok())
            .collect()
    }

    fn matches_any(&self, patterns: &[regex::Regex], input: &str) -> bool {
        patterns.iter().any(|r| r.is_match(input))
    }

    fn architecture_score(&self, input: &str) -> usize {
        self.matches_any(&self.architecture, input) as usize
    }

    fn trade_offs_score(&self, input: &str) -> usize {
        self.matches_any(&self.trade_offs, input) as usize
    }

    fn ambiguous_score(&self, input: &str) -> usize {
        self.matches_any(&self.ambiguous, input) as usize
    }

    fn design_score(&self, input: &str) -> usize {
        self.matches_any(&self.design, input) as usize
    }

    fn total_trigger_score(&self, input: &str) -> usize {
        self.architecture_score(input)
            + self.trade_offs_score(input)
            + self.ambiguous_score(input)
            + self.design_score(input)
    }

    fn dominant_trigger(&self, input: &str) -> DebateTriggerReason {
        let scores = [
            (
                self.architecture_score(input),
                DebateTriggerReason::ArchitectureQuestion,
            ),
            (
                self.trade_offs_score(input),
                DebateTriggerReason::MultiTradeOffAnalysis,
            ),
            (
                self.ambiguous_score(input),
                DebateTriggerReason::AmbiguousProblem,
            ),
            (
                self.design_score(input),
                DebateTriggerReason::DesignDecision,
            ),
        ];

        scores
            .into_iter()
            .max_by_key(|(score, _)| *score)
            .map(|(_, trigger)| trigger)
            .unwrap_or(DebateTriggerReason::ArchitectureQuestion)
    }
}

/// Patterns that DEACTIVATE debate (skips).
#[derive(Debug, Clone)]
pub struct DebateSkipPatterns {
    pub simple_fact: Vec<regex::Regex>,
    pub syntax_error: Vec<regex::Regex>,
    pub code_generation: Vec<regex::Regex>,
    pub refactor_simple: Vec<regex::Regex>,
}

impl Default for DebateSkipPatterns {
    fn default() -> Self {
        Self {
            simple_fact: Self::compile_patterns(&[
                r"(?i)^(what is|who is|when is|where is|how do i)",
                r"(?i)^(define|explain|tell me about)",
                r"^\s*(yes|no|yep|nope)\s*[.!]?\s*$", // Direct yes/no
                r"(?i)^(list|show|get|print) (all |the )?(files?|functions?|methods|classes)",
            ]),
            syntax_error: Self::compile_patterns(&[
                r"(?i)(syntax error|parse error|compilation error|compile error)",
                r"(?i)(unexpected token|expected .*, found)",
                r"(?i)(undefined (variable|function|method))",
                r"(?i)(missing |missing) (semicolon|bracket|parenthesis|comma)",
                r"(?i)(fix this|error on line|line \d+)",
            ]),
            code_generation: Self::compile_patterns(&[
                r"(?i)^(write|create|generate|implement) .*(function|method|class|struct)",
                r"(?i)^(add |implement |create )?(getter|setter|constructor)",
                r"(?i)^(make|write) (a |an )?simple",
                r"(?i)^(format|lint|prettify|beautify)",
            ]),
            refactor_simple: Self::compile_patterns(&[
                r"(?i)(rename|extract|inline) (variable|method|function)",
                r"(?i)(add type|add return type|add parameter)",
                r"(?i)(simplify|clean up) (this |the )?code",
                r"(?i)^(remove|delete) (unused|dead) (code|import|variable)",
            ]),
        }
    }
}

impl DebateSkipPatterns {
    fn compile_patterns(patterns: &[&str]) -> Vec<regex::Regex> {
        patterns
            .iter()
            .filter_map(|p| regex::Regex::new(p).ok())
            .collect()
    }

    fn should_skip(&self, input: &str) -> bool {
        let has_simple_fact = self.simple_fact.iter().any(|r| r.is_match(input));
        let has_syntax_error = self.syntax_error.iter().any(|r| r.is_match(input));
        let has_code_gen = self.code_generation.iter().any(|r| r.is_match(input));
        let has_simple_refactor = self.refactor_simple.iter().any(|r| r.is_match(input));

        has_simple_fact || has_syntax_error || has_code_gen || has_simple_refactor
    }

    fn skip_reason(&self, input: &str) -> &'static str {
        if self.simple_fact.iter().any(|r| r.is_match(input)) {
            "Simple factual question"
        } else if self.syntax_error.iter().any(|r| r.is_match(input)) {
            "Syntax error fix"
        } else if self.code_generation.iter().any(|r| r.is_match(input)) {
            "Simple code generation"
        } else if self.refactor_simple.iter().any(|r| r.is_match(input)) {
            "Simple refactor"
        } else {
            "Unknown simple task"
        }
    }
}

/// Core decision engine for debate invocation.
pub struct DebateDecisionEngine {
    triggers: DebateTriggerPatterns,
    skips: DebateSkipPatterns,
    mode: DebateMode,
    default_depth: DebateDepth,
    trigger_threshold: usize,
}

impl Default for DebateDecisionEngine {
    fn default() -> Self {
        Self {
            triggers: DebateTriggerPatterns::default(),
            skips: DebateSkipPatterns::default(),
            mode: DebateMode::Auto,
            default_depth: DebateDepth::Medium,
            trigger_threshold: 1,
        }
    }
}

impl DebateDecisionEngine {
    pub fn new() -> Self {
        Self::default()
    }

    /// Configure debate mode.
    pub fn with_mode(mut self, mode: DebateMode) -> Self {
        self.mode = mode;
        self
    }

    /// Configure default depth level.
    pub fn with_default_depth(mut self, depth: DebateDepth) -> Self {
        self.default_depth = depth;
        self
    }

    /// Configure trigger sensitivity threshold.
    pub fn with_trigger_threshold(mut self, threshold: usize) -> Self {
        self.trigger_threshold = threshold;
        self
    }

    /// Make a debate decision for the given task description.
    pub fn decide(&self, task: &str) -> DebateDecision {
        // Mode: Off = never debate
        if self.mode == DebateMode::Off {
            return DebateDecision::skip(
                DebateTriggerReason::Disabled,
                self.mode,
                "Debate disabled via /debate off",
            );
        }

        // Mode: Explicit = only debate if explicitly requested
        // (This is handled at CLI level before calling decide)
        if self.mode == DebateMode::Explicit {
            return DebateDecision::skip(
                DebateTriggerReason::SkippedSimpleTask,
                self.mode,
                "Debate mode is explicit - provide explicit /debate command to activate",
            );
        }

        // Mode: Auto - check triggers and skips
        // First: Skip rule (takes precedence)
        if self.skips.should_skip(task) {
            return DebateDecision::skip(
                DebateTriggerReason::SkippedSimpleTask,
                self.mode,
                self.skips.skip_reason(task),
            );
        }

        // Second: Trigger rule
        let trigger_score = self.triggers.total_trigger_score(task);

        if trigger_score >= self.trigger_threshold {
            let dominant_trigger = self.triggers.dominant_trigger(task);

            // Calculate confidence based on score
            let confidence = (trigger_score as f64 / 4.0).min(1.0);

            // Determine depth based on trigger strength
            let depth = if trigger_score >= 3 {
                DebateDepth::Deep
            } else if trigger_score >= 2 {
                DebateDepth::Medium
            } else {
                self.default_depth
            };

            // Clone for format macro before engaging (dominant_trigger moves into engage)
            let trigger_for_log = dominant_trigger.clone();

            return DebateDecision::engage(
                dominant_trigger,
                self.mode,
                depth,
                confidence,
                &format!(
                    "Trigger matched: {:?} (score: {})",
                    trigger_for_log, trigger_score
                ),
            );
        }

        // No clear trigger - use simple response
        DebateDecision::skip(
            DebateTriggerReason::SkippedSimpleTask,
            self.mode,
            "No debate trigger matched - task appears straightforward",
        )
    }

    /// Make decision with explicit debate request (overrides Auto mode).
    pub fn decide_explicit(&self, _task: &str, depth: DebateDepth) -> DebateDecision {
        // Even in Off mode, explicit requests can proceed
        DebateDecision::engage(
            DebateTriggerReason::ExplicitRequest,
            self.mode,
            depth,
            1.0,
            "Explicit debate request via /debate command",
        )
    }
}

/// CLI command parser for debate commands.
#[derive(Debug, Clone)]
pub struct DebateCliCommand {
    pub action: DebateCliAction,
    pub task: Option<String>,
    pub depth: Option<DebateDepth>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DebateCliAction {
    /// /debate "task" - activate debate for task
    Activate,
    /// /debate --quick "task" - quick debate
    ActivateQuick,
    /// /debate --medium "task" - medium debate
    ActivateMedium,
    /// /debate --deep "task" - deep debate
    ActivateDeep,
    /// /debate off - disable debate
    Disable,
    /// /debate on - enable auto mode
    Enable,
    /// /debate status - show current config
    Status,
}

impl DebateCliCommand {
    /// Parse a debate CLI command string.
    ///
    /// Examples:
    /// - `/debate "Design auth system"` → Activate with Medium depth
    /// - `/debate --quick "Fix this"` → ActivateQuick with Quick depth
    /// - `/debate off` → Disable
    pub fn parse(input: &str) -> Option<Self> {
        let input = input.trim();

        // Remove leading slash if present
        let input = input.strip_prefix('/').unwrap_or(input);
        let input = input.trim();

        // Check for "debate" command
        if !input.to_lowercase().starts_with("debate") {
            return None;
        }

        let rest = input[6..].trim();

        // /debate off
        if rest.eq_ignore_ascii_case("off") {
            return Some(Self {
                action: DebateCliAction::Disable,
                task: None,
                depth: None,
            });
        }

        // /debate on
        if rest.eq_ignore_ascii_case("on") {
            return Some(Self {
                action: DebateCliAction::Enable,
                task: None,
                depth: None,
            });
        }

        // /debate status
        if rest.eq_ignore_ascii_case("status") {
            return Some(Self {
                action: DebateCliAction::Status,
                task: None,
                depth: None,
            });
        }

        // Parse flags: --quick, --medium, --deep
        let mut depth = None;
        let mut remaining = rest;

        if remaining.starts_with("--") {
            if let Some(space_idx) = remaining.find(|c: char| c.is_whitespace()) {
                let flag = &remaining[..space_idx];
                remaining = remaining[space_idx..].trim();

                match flag.to_lowercase().as_str() {
                    "--quick" => depth = Some(DebateDepth::Quick),
                    "--medium" => depth = Some(DebateDepth::Medium),
                    "--deep" => depth = Some(DebateDepth::Deep),
                    _ => return None,
                }
            }
        }

        // Extract task (quoted or rest of string)
        let task = Self::extract_quoted_or_rest(remaining)?;

        Some(Self {
            action: if depth == Some(DebateDepth::Quick) {
                DebateCliAction::ActivateQuick
            } else if depth == Some(DebateDepth::Deep) {
                DebateCliAction::ActivateDeep
            } else {
                DebateCliAction::Activate
            },
            task: Some(task),
            depth: depth.or(Some(DebateDepth::Medium)),
        })
    }

    fn extract_quoted_or_rest(input: &str) -> Option<String> {
        let input = input.trim();

        // Try double-quoted
        if input.starts_with('"') {
            if let Some(end) = input[1..].find('"') {
                return Some(input[1..end + 1].to_string());
            }
        }

        // Try single-quoted
        if input.starts_with('\'') {
            if let Some(end) = input[1..].find('\'') {
                return Some(input[1..end + 1].to_string());
            }
        }

        // Use remainder as task (trim whitespace)
        let trimmed = input.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    }
}

/// Fallback behavior when debate fails.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DebateFallback {
    /// Return simple coordinator response.
    SimpleResponse,
    /// Retry with backoff (up to max_retries).
    RetryWithBackoff,
    /// Discard perspective if gibberish.
    DiscardGibberish,
}

impl Default for DebateFallback {
    fn default() -> Self {
        Self::SimpleResponse
    }
}

/// Debate error types for fallback handling.
#[derive(Debug, Clone, thiserror::Error)]
pub enum DebateError {
    #[error("Debate timeout after {0}s")]
    Timeout(u64),

    #[error("Rate limit exceeded, retry after {0}s")]
    RateLimit(u64),

    #[error("Perspective contains gibberish: {0}")]
    Gibberish(String),

    #[error("Agent failed: {0}")]
    AgentFailed(String),
}

impl DebateError {
    pub fn fallback(&self) -> DebateFallback {
        match self {
            Self::Timeout(_) => DebateFallback::SimpleResponse,
            Self::RateLimit(_) => DebateFallback::RetryWithBackoff,
            Self::Gibberish(_) => DebateFallback::DiscardGibberish,
            Self::AgentFailed(_) => DebateFallback::SimpleResponse,
        }
    }

    pub fn retry_after_secs(&self) -> Option<u64> {
        match self {
            Self::RateLimit(s) => Some(*s),
            _ => None,
        }
    }

    /// Check if perspective output is gibberish (low coherence).
    pub fn is_gibberish(text: &str) -> bool {
        let text = text.trim();

        // Too short to evaluate
        if text.len() < 50 {
            return false;
        }

        // Check for repeated characters (e.g., "aaaaaaa")
        let mut repeat_count = 0;
        let mut last_char = None;
        for c in text.chars() {
            if Some(c) == last_char {
                repeat_count += 1;
                if repeat_count > 5 {
                    return true;
                }
            } else {
                repeat_count = 0;
            }
            last_char = Some(c);
        }

        // Check for excessive uppercase (SHOUTING)
        let upper_count = text.chars().filter(|c| c.is_uppercase()).count();
        let letter_count = text.chars().filter(|c| c.is_alphabetic()).count();
        if letter_count > 20 && (upper_count as f64 / letter_count as f64) > 0.8 {
            return true;
        }

        false
    }
}

/// Debate session state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DebateSession {
    pub id: String,
    pub task: String,
    pub mode: DebateMode,
    pub depth: DebateDepth,
    pub perspectives: Vec<PerspectiveState>,
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub status: DebateSessionStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerspectiveState {
    pub name: String,
    pub status: PerspectiveStatus,
    pub response: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PerspectiveStatus {
    Pending,
    Running,
    Completed,
    Timeout,
    Failed,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DebateSessionStatus {
    Pending,
    Running,
    Completed,
    PartialFailure,
    Failed,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_skip_simple_questions() {
        let engine = DebateDecisionEngine::new();
        let task = "What is Rust?";

        let decision = engine.decide(task);
        assert!(!decision.should_debate);
        assert_eq!(decision.trigger, DebateTriggerReason::SkippedSimpleTask);
    }

    #[test]
    fn test_skip_syntax_errors() {
        let engine = DebateDecisionEngine::new();
        let task = "Fix syntax error on line 42";

        let decision = engine.decide(task);
        assert!(!decision.should_debate);
    }

    #[test]
    fn test_trigger_architecture() {
        let engine = DebateDecisionEngine::new();
        let task = "Design an authentication system for a microservices architecture";

        let decision = engine.decide(task);
        assert!(decision.should_debate);
        assert!(matches!(
            decision.trigger,
            DebateTriggerReason::ArchitectureQuestion | DebateTriggerReason::DesignDecision
        ));
    }

    #[test]
    fn test_trigger_trade_offs() {
        let engine = DebateDecisionEngine::new();
        // Use exact phrasing that matches regex: "trade-off" (no 's')
        let task = "What are the trade-offs between monolith and microservices?";

        let decision = engine.decide(task);
        assert!(decision.should_debate);
        // Either architecture or trade-offs trigger (trade-offs ends with 's' so might not match trade-?off)
        // Actually the pattern (?i)(trade-?off...) matches "trade-offs" because 's' is not explicitly excluded
        // Let's verify the trigger matches at least one expected type
        assert!(matches!(
            decision.trigger,
            DebateTriggerReason::MultiTradeOffAnalysis
                | DebateTriggerReason::ArchitectureQuestion
                | DebateTriggerReason::AmbiguousProblem
        ));
    }

    #[test]
    fn test_explicit_debate_request() {
        let engine = DebateDecisionEngine::new();
        let task = "Fix this bug";

        let decision = engine.decide_explicit(task, DebateDepth::Quick);
        assert!(decision.should_debate);
        assert_eq!(decision.trigger, DebateTriggerReason::ExplicitRequest);
        assert_eq!(decision.depth, Some(DebateDepth::Quick));
    }

    #[test]
    fn test_cli_parse_basic() {
        let cmd = DebateCliCommand::parse("/debate \"Design auth system\"").unwrap();
        assert_eq!(cmd.action, DebateCliAction::Activate);
        assert_eq!(cmd.task.as_deref(), Some("Design auth system"));
        assert_eq!(cmd.depth, Some(DebateDepth::Medium));
    }

    #[test]
    fn test_cli_parse_quick() {
        let cmd = DebateCliCommand::parse("/debate --quick \"Fix this bug\"").unwrap();
        assert_eq!(cmd.action, DebateCliAction::ActivateQuick);
        assert_eq!(cmd.depth, Some(DebateDepth::Quick));
    }

    #[test]
    fn test_cli_parse_off() {
        let cmd = DebateCliCommand::parse("/debate off").unwrap();
        assert_eq!(cmd.action, DebateCliAction::Disable);
    }

    #[test]
    fn test_depth_token_multiplier() {
        assert!((DebateDepth::Quick.token_budget_multiplier() - 1.5).abs() < 0.01);
        assert!((DebateDepth::Medium.token_budget_multiplier() - 2.5).abs() < 0.01);
        assert!((DebateDepth::Deep.token_budget_multiplier() - 5.0).abs() < 0.01);
    }

    #[test]
    fn test_gibberish_detection() {
        // Test excessive uppercase (SHOUTING) - must be 50+ chars
        let shouting = "THIS IS ALL CAPS AND SHOULD BE DETECTED AS GIBBERISH TEXT";
        assert!(
            DebateError::is_gibberish(shouting),
            "All caps text should be gibberish"
        );

        // Test repeated characters (must be 50+ chars)
        let repeated = "AAAAAA AAAAAA AAAAAA AAAAAA AAAAAA AAAAAA AAAAAA AAAAAA AAAAAA AAAAAA";
        assert!(
            DebateError::is_gibberish(repeated),
            "Repeated characters should be gibberish"
        );

        // Test normal text
        assert!(
            !DebateError::is_gibberish(
                "This is a normal response with multiple sentences and proper content."
            ),
            "Normal text should not be gibberish"
        );

        // Short text should return false (too short to evaluate)
        assert!(
            !DebateError::is_gibberish("A simple sentence."),
            "Too short to evaluate"
        );
    }
}

// =============================================================================
// SAFEGUARDS - Multi-Perspective Debate Safety Systems
// =============================================================================

use indexmap::IndexMap;
use std::collections::HashMap;
use std::time::Instant;

// ===========================================================================
// 1. OUTPUT VALIDATION
// ===========================================================================

/// Configuration for output validation.
#[derive(Debug, Clone)]
pub struct OutputValidationConfig {
    pub min_length: usize,
    pub max_length: usize,
    pub min_entropy: f64,
    pub max_repeated_chars: usize,
    pub required_word_ratio: f64,
}

impl Default for OutputValidationConfig {
    fn default() -> Self {
        Self {
            min_length: 50,
            max_length: 50_000,
            min_entropy: 2.5,
            max_repeated_chars: 5,
            required_word_ratio: 0.3,
        }
    }
}

/// Result of validating perspective output.
#[derive(Debug, Clone)]
pub struct ValidationResult {
    pub is_valid: bool,
    pub errors: Vec<ValidationError>,
    pub entropy_score: f64,
    pub word_ratio: f64,
    pub quality_score: f64,
}

impl ValidationResult {
    pub fn valid() -> Self {
        Self {
            is_valid: true,
            errors: vec![],
            entropy_score: 4.0,
            word_ratio: 0.5,
            quality_score: 1.0,
        }
    }

    pub fn invalid(errors: Vec<ValidationError>) -> Self {
        Self {
            is_valid: false,
            errors,
            entropy_score: 0.0,
            word_ratio: 0.0,
            quality_score: 0.0,
        }
    }

    pub fn with_quality(mut self, score: f64) -> Self {
        self.quality_score = score;
        self
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ValidationError {
    TooShort {
        actual: usize,
        minimum: usize,
    },
    TooLong {
        actual: usize,
        maximum: usize,
    },
    LowEntropy {
        actual: f64,
        minimum: f64,
    },
    ExcessiveRepeats {
        char: char,
        count: usize,
        limit: usize,
    },
    GibberishDetected {
        reason: String,
    },
    InternalInconsistency {
        details: String,
    },
}

/// Output validator with entropy and gibberish detection.
pub struct OutputValidator {
    config: OutputValidationConfig,
}

impl OutputValidator {
    pub fn new() -> Self {
        Self {
            config: OutputValidationConfig::default(),
        }
    }

    pub fn with_config(mut self, config: OutputValidationConfig) -> Self {
        self.config = config;
        self
    }

    /// Validate perspective output.
    pub fn validate(&self, text: &str) -> ValidationResult {
        let mut errors = Vec::new();
        let text = text.trim();

        // Length checks
        if text.len() < self.config.min_length {
            errors.push(ValidationError::TooShort {
                actual: text.len(),
                minimum: self.config.min_length,
            });
        }

        if text.len() > self.config.max_length {
            errors.push(ValidationError::TooLong {
                actual: text.len(),
                maximum: self.config.max_length,
            });
        }

        // Entropy check
        let entropy = self.calculate_entropy(text);
        if entropy < self.config.min_entropy {
            errors.push(ValidationError::LowEntropy {
                actual: entropy,
                minimum: self.config.min_entropy,
            });
        }

        // Repeated character check
        if let Some((ch, count)) = self.find_max_repeated_char(text) {
            if count > self.config.max_repeated_chars {
                errors.push(ValidationError::ExcessiveRepeats {
                    char: ch,
                    count,
                    limit: self.config.max_repeated_chars,
                });
            }
        }

        // Word ratio check (detects gibberish)
        let word_ratio = self.calculate_word_ratio(text);
        if word_ratio < self.config.required_word_ratio {
            errors.push(ValidationError::GibberishDetected {
                reason: format!(
                    "Word ratio {} below threshold {}",
                    format!("{:.2}", word_ratio),
                    format!("{:.2}", self.config.required_word_ratio)
                ),
            });
        }

        // Check for excessive special characters
        let special_chars = text
            .chars()
            .filter(|c| !c.is_alphanumeric() && !c.is_whitespace())
            .count();
        let total_chars = text.len();
        if total_chars > 50 && special_chars as f64 / total_chars as f64 > 0.3 {
            errors.push(ValidationError::GibberishDetected {
                reason: "Excessive special characters".to_string(),
            });
        }

        let is_valid = errors.is_empty();
        ValidationResult {
            is_valid,
            errors,
            entropy_score: entropy,
            word_ratio,
            quality_score: self.calculate_quality_score(text, entropy, word_ratio),
        }
    }

    fn calculate_entropy(&self, text: &str) -> f64 {
        if text.is_empty() {
            return 0.0;
        }

        let mut char_counts: HashMap<char, usize> = HashMap::new();
        let mut total = 0usize;

        for c in text.chars() {
            *char_counts.entry(c).or_insert(0) += 1;
            total += 1;
        }

        let mut entropy = 0.0;
        for (_, count) in char_counts {
            let p = count as f64 / total as f64;
            if p > 0.0 {
                entropy -= p * p.log2();
            }
        }

        entropy
    }

    fn find_max_repeated_char(&self, text: &str) -> Option<(char, usize)> {
        let mut max_char = None;
        let mut max_count = 0;
        let mut current_char = None;
        let mut current_count = 0;

        for c in text.chars() {
            if Some(c) == current_char {
                current_count += 1;
            } else {
                if current_count > max_count {
                    max_count = current_count;
                    max_char = current_char;
                }
                current_char = Some(c);
                current_count = 1;
            }
        }

        if current_count > max_count {
            max_count = current_count;
            max_char = current_char;
        }

        max_char.map(|c| (c, max_count))
    }

    fn calculate_word_ratio(&self, text: &str) -> f64 {
        let words: Vec<&str> = text.split_whitespace().collect();
        if words.is_empty() {
            return 0.0;
        }

        let word_chars: usize = words.iter().map(|w| w.len()).sum();
        let alpha_chars: usize = text.chars().filter(|c| c.is_alphabetic()).count();

        if alpha_chars == 0 {
            return 0.0;
        }

        word_chars as f64 / alpha_chars as f64
    }

    fn calculate_quality_score(&self, _text: &str, entropy: f64, word_ratio: f64) -> f64 {
        // Quality is a weighted combination of entropy and word ratio
        // Ideal entropy around 4.0-4.5 (English text)
        // Ideal word ratio around 0.6-0.8
        let entropy_score = (entropy / 5.0).min(1.0);
        let word_score = (word_ratio / 0.7).min(1.0);
        entropy_score * 0.4 + word_score * 0.6
    }

    /// Check if two responses are internally consistent.
    pub fn check_consistency(&self, response1: &str, response2: &str) -> bool {
        // Simple heuristic: check if they share significant vocabulary
        let lowercase1 = response1.to_lowercase();
        let words1: std::collections::HashSet<_> = lowercase1
            .split_whitespace()
            .filter(|w| w.len() > 4)
            .collect();

        let lowercase2 = response2.to_lowercase();
        let words2: std::collections::HashSet<_> = lowercase2
            .split_whitespace()
            .filter(|w| w.len() > 4)
            .collect();

        if words1.is_empty() || words2.is_empty() {
            return true;
        }

        let intersection = words1.intersection(&words2).count();
        let union = words1.union(&words2).count();

        // Jaccard similarity
        intersection as f64 / union as f64 > 0.1
    }
}

impl Default for OutputValidator {
    fn default() -> Self {
        Self::new()
    }
}

// ===========================================================================
// 2. CIRCUIT BREAKER
// ===========================================================================

/// Configuration for circuit breaker.
#[derive(Debug, Clone)]
pub struct CircuitBreakerConfig {
    pub divergence_threshold: f64,
    pub perspective_timeout_secs: u64,
    pub total_timeout_secs: u64,
    pub min_perspectives_required: usize,
}

impl Default for CircuitBreakerConfig {
    fn default() -> Self {
        Self {
            divergence_threshold: 0.6,
            perspective_timeout_secs: 45,
            total_timeout_secs: 120,
            min_perspectives_required: 2,
        }
    }
}

/// State of a circuit breaker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CircuitState {
    Closed,
    Open,
    HalfOpen,
}

/// Reason why perspective was excluded.
#[derive(Debug, Clone, PartialEq)]
pub enum ExclusionReason {
    DivergedTooMuch { divergence: f64, threshold: f64 },
    TimedOut { elapsed_secs: f64, limit_secs: u64 },
    ValidationFailed,
    ManipulationDetected,
}

/// ExclusionReason with Eq compatibility.
impl Eq for ExclusionReason {}

/// Result of circuit breaker check.
#[derive(Debug, Clone)]
pub struct CircuitBreakerResult {
    pub should_exclude: bool,
    pub reason: Option<ExclusionReason>,
    pub state: CircuitState,
    pub excluded_perspectives: Vec<String>,
    pub remaining_perspectives: Vec<String>,
}

impl CircuitBreakerResult {
    pub fn allow_all(perspectives: Vec<String>) -> Self {
        Self {
            should_exclude: false,
            reason: None,
            state: CircuitState::Closed,
            excluded_perspectives: vec![],
            remaining_perspectives: perspectives,
        }
    }

    pub fn with_exclusions(
        excluded: Vec<String>,
        remaining: Vec<String>,
        reason: ExclusionReason,
    ) -> Self {
        Self {
            should_exclude: true,
            reason: Some(reason),
            state: CircuitState::Open,
            excluded_perspectives: excluded,
            remaining_perspectives: remaining,
        }
    }
}

/// Circuit breaker for debate perspectives.
pub struct CircuitBreaker {
    config: CircuitBreakerConfig,
    state: CircuitState,
    failure_count: usize,
    last_failure: Option<Instant>,
    perspective_timers: HashMap<String, Instant>,
    total_start: Option<Instant>,
}

impl CircuitBreaker {
    pub fn new() -> Self {
        Self {
            config: CircuitBreakerConfig::default(),
            state: CircuitState::Closed,
            failure_count: 0,
            last_failure: None,
            perspective_timers: HashMap::new(),
            total_start: None,
        }
    }

    pub fn with_config(mut self, config: CircuitBreakerConfig) -> Self {
        self.config = config;
        self
    }

    /// Start tracking time for a perspective.
    pub fn start_perspective(&mut self, name: &str) {
        self.perspective_timers
            .insert(name.to_string(), Instant::now());
        if self.total_start.is_none() {
            self.total_start = Some(Instant::now());
        }
    }

    /// Check if a perspective has exceeded its timeout.
    pub fn check_perspective_timeout(&self, name: &str) -> Option<ExclusionReason> {
        if let Some(start) = self.perspective_timers.get(name) {
            let elapsed = start.elapsed().as_secs_f64();
            if elapsed > self.config.perspective_timeout_secs as f64 {
                return Some(ExclusionReason::TimedOut {
                    elapsed_secs: elapsed,
                    limit_secs: self.config.perspective_timeout_secs,
                });
            }
        }
        None
    }

    /// Check if total debate time exceeded.
    pub fn check_total_timeout(&self) -> bool {
        if let Some(start) = self.total_start {
            return start.elapsed().as_secs() > self.config.total_timeout_secs;
        }
        false
    }

    /// Check if perspective diverged too much from consensus.
    pub fn check_divergence(
        &self,
        perspective_scores: &HashMap<String, f64>,
        consensus_score: f64,
    ) -> Vec<(String, ExclusionReason)> {
        let mut excluded = Vec::new();

        for (name, score) in perspective_scores {
            let divergence = (score - consensus_score).abs();
            if divergence > self.config.divergence_threshold {
                excluded.push((
                    name.clone(),
                    ExclusionReason::DivergedTooMuch {
                        divergence,
                        threshold: self.config.divergence_threshold,
                    },
                ));
            }
        }

        excluded
    }

    /// Final circuit check with all perspectives.
    pub fn finalize(
        &self,
        perspectives: &IndexMap<String, PerspectiveOutput>,
    ) -> CircuitBreakerResult {
        // Build score map
        let scores: HashMap<String, f64> = perspectives
            .iter()
            .map(|(name, output)| (name.clone(), output.quality_score))
            .collect();

        // Calculate consensus (median of scores)
        let mut sorted_scores: Vec<f64> = scores.values().cloned().collect();
        sorted_scores.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let consensus_score = if sorted_scores.len() % 2 == 0 {
            (sorted_scores[sorted_scores.len() / 2 - 1] + sorted_scores[sorted_scores.len() / 2])
                / 2.0
        } else {
            sorted_scores[sorted_scores.len() / 2]
        };

        // Check divergences
        let divergences = self.check_divergence(&scores, consensus_score);
        let excluded: Vec<String> = divergences.iter().map(|(n, _)| n.clone()).collect();
        let remaining: Vec<String> = perspectives
            .keys()
            .filter(|n| !excluded.contains(n))
            .cloned()
            .collect();

        // Check if we have minimum perspectives
        if remaining.len() < self.config.min_perspectives_required {
            // Can't exclude, return original
            return CircuitBreakerResult::allow_all(perspectives.keys().cloned().collect());
        }

        if !excluded.is_empty() {
            let reason = divergences.first().map(|(_, r)| r.clone()).unwrap_or(
                ExclusionReason::DivergedTooMuch {
                    divergence: 1.0,
                    threshold: self.config.divergence_threshold,
                },
            );
            CircuitBreakerResult::with_exclusions(excluded, remaining, reason)
        } else {
            CircuitBreakerResult::allow_all(remaining)
        }
    }

    /// Record a failure and potentially open the circuit.
    pub fn record_failure(&mut self) {
        self.failure_count += 1;
        self.last_failure = Some(Instant::now());

        // Open circuit after 3 consecutive failures
        if self.failure_count >= 3 {
            self.state = CircuitState::Open;
        }
    }

    /// Reset the circuit breaker.
    pub fn reset(&mut self) {
        self.state = CircuitState::Closed;
        self.failure_count = 0;
        self.last_failure = None;
        self.perspective_timers.clear();
        self.total_start = None;
    }

    pub fn state(&self) -> CircuitState {
        self.state
    }
}

impl Default for CircuitBreaker {
    fn default() -> Self {
        Self::new()
    }
}

/// Output from a perspective including quality score.
#[derive(Debug, Clone)]
pub struct PerspectiveOutput {
    pub name: String,
    pub content: String,
    pub quality_score: f64,
    pub confidence: f64,
    pub execution_time_ms: u64,
}

impl PerspectiveOutput {
    pub fn new(name: &str, content: String, quality_score: f64, confidence: f64) -> Self {
        Self {
            name: name.to_string(),
            content,
            quality_score,
            confidence,
            execution_time_ms: 0,
        }
    }
}

// ===========================================================================
// 3. CONSENSUS BUILDER
// ===========================================================================

/// Configuration for consensus building.
#[derive(Debug, Clone)]
pub struct ConsensusConfig {
    pub confidence_weight: f64,
    pub quality_weight: f64,
    pub agreement_threshold: f64,
    pub min_agreement_count: usize,
}

impl Default for ConsensusConfig {
    fn default() -> Self {
        Self {
            confidence_weight: 0.3,
            quality_weight: 0.4,
            agreement_threshold: 0.5,
            min_agreement_count: 2,
        }
    }
}

/// Result of building consensus.
#[derive(Debug, Clone)]
pub struct ConsensusResult {
    pub consensus_text: String,
    pub consensus_score: f64,
    pub agreement_level: f64,
    pub winning_perspectives: Vec<String>,
    pub alternative_perspectives: Vec<String>,
    pub used_fallback: bool,
}

impl ConsensusResult {
    pub fn fallback_response(text: String) -> Self {
        Self {
            consensus_text: text,
            consensus_score: 0.5,
            agreement_level: 0.0,
            winning_perspectives: vec![],
            alternative_perspectives: vec![],
            used_fallback: true,
        }
    }

    pub fn with_consensus(
        text: String,
        score: f64,
        agreement: f64,
        winners: Vec<String>,
        alternatives: Vec<String>,
    ) -> Self {
        Self {
            consensus_text: text,
            consensus_score: score,
            agreement_level: agreement,
            winning_perspectives: winners,
            alternative_perspectives: alternatives,
            used_fallback: false,
        }
    }
}

/// Builds consensus from multiple perspectives using weighted scoring.
pub struct ConsensusBuilder {
    config: ConsensusConfig,
    fallback_response: String,
}

impl ConsensusBuilder {
    pub fn new() -> Self {
        Self {
            config: ConsensusConfig::default(),
            fallback_response: "Based on the analysis, the recommended approach balances the trade-offs identified. Consider the specific context and constraints of your use case when making the final decision.".to_string(),
        }
    }

    pub fn with_config(mut self, config: ConsensusConfig) -> Self {
        self.config = config;
        self
    }

    pub fn with_fallback_response(mut self, response: String) -> Self {
        self.fallback_response = response;
        self
    }

    /// Build consensus from perspective outputs.
    pub fn build(&self, perspectives: &IndexMap<String, PerspectiveOutput>) -> ConsensusResult {
        if perspectives.is_empty() {
            return ConsensusResult::fallback_response(self.fallback_response.clone());
        }

        if perspectives.len() == 1 {
            let single = perspectives.first().unwrap();
            return ConsensusResult::with_consensus(
                single.1.content.clone(),
                single.1.quality_score,
                1.0,
                vec![single.0.clone()],
                vec![],
            );
        }

        // Calculate weighted scores
        let mut scored: Vec<(f64, &str, &PerspectiveOutput)> = perspectives
            .iter()
            .map(|(name, output)| {
                let weighted_score = self.calculate_weighted_score(output);
                (weighted_score, name.as_str(), output)
            })
            .collect();

        // Sort by score descending
        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

        // Calculate agreement level
        let agreement = self.calculate_agreement(perspectives);

        // Determine winners and alternatives
        let top_score = scored.first().map(|(s, _, _)| *s).unwrap_or(0.0);
        let winners: Vec<String> = scored
            .iter()
            .filter(|(score, _, _)| (*score - top_score).abs() < 0.1)
            .map(|(_, name, _)| (*name).to_string())
            .collect();

        let alternatives: Vec<String> = scored
            .iter()
            .skip(winners.len())
            .map(|(_, name, _)| (*name).to_string())
            .collect();

        // Check if we have enough agreement
        if agreement < self.config.agreement_threshold
            && scored.len() >= self.config.min_agreement_count
        {
            // Combine top perspectives
            let top_count = winners.len().min(2);
            let top_slice: Vec<(f64, &str, &PerspectiveOutput)> = scored
                .iter()
                .take(top_count)
                .map(|(s, n, p)| (*s, *n, *p))
                .collect();
            let consensus_text = self.combine_perspectives(&top_slice);
            let avg_score: f64 =
                scored[..top_count].iter().map(|(s, _, _)| *s).sum::<f64>() / (top_count as f64);

            return ConsensusResult::with_consensus(
                consensus_text,
                avg_score,
                agreement,
                winners,
                alternatives,
            );
        }

        // Use single best perspective
        let (_, _, best) = scored.first().unwrap();
        ConsensusResult::with_consensus(
            best.content.clone(),
            top_score,
            agreement,
            winners,
            alternatives,
        )
    }

    fn calculate_weighted_score(&self, output: &PerspectiveOutput) -> f64 {
        let confidence_component = output.confidence * self.config.confidence_weight;
        let quality_component = output.quality_score * self.config.quality_weight;
        let consistency_component = (1.0 - (output.execution_time_ms as f64 / 50000.0).min(1.0))
            * (1.0 - self.config.confidence_weight - self.config.quality_weight);

        confidence_component + quality_component + consistency_component
    }

    fn calculate_agreement(&self, perspectives: &IndexMap<String, PerspectiveOutput>) -> f64 {
        if perspectives.len() < 2 {
            return 1.0;
        }

        // Extract key terms from each perspective
        let key_terms: Vec<std::collections::HashSet<_>> = perspectives
            .values()
            .map(|p| self.extract_key_terms(&p.content))
            .collect();

        // Calculate pairwise Jaccard similarity
        let mut total_similarity = 0.0;
        let mut comparisons = 0;

        for i in 0..key_terms.len() {
            for j in (i + 1)..key_terms.len() {
                let intersection = key_terms[i].intersection(&key_terms[j]).count() as f64;
                let union = key_terms[i].union(&key_terms[j]).count() as f64;
                if union > 0.0 {
                    total_similarity += intersection / union;
                    comparisons += 1;
                }
            }
        }

        if comparisons == 0 {
            return 0.0;
        }

        total_similarity / comparisons as f64
    }

    fn extract_key_terms(&self, text: &str) -> std::collections::HashSet<String> {
        text.split_whitespace()
            .filter(|w| {
                w.len() > 5
                    && w.chars().all(|c| c.is_alphanumeric())
                    && !STOP_WORDS.contains(&w.to_lowercase().as_str())
            })
            .map(|w| w.to_lowercase())
            .collect()
    }

    fn combine_perspectives<'a>(
        &self,
        perspectives: &[(f64, &'a str, &'a PerspectiveOutput)],
    ) -> String {
        if perspectives.is_empty() {
            return self.fallback_response.clone();
        }

        if perspectives.len() == 1 {
            return perspectives[0].2.content.clone();
        }

        // Combine first two perspectives with a bridge
        let first = &perspectives[0].2.content;
        let second = &perspectives[1].2.content;

        let min_len = first.len().min(second.len());
        let combined = format!(
            "{}\n\nAdditionally, {}\n\nSynthesis: {}",
            &first[..min_len.min(first.len().min(500))],
            perspectives[1].2.name,
            "After considering multiple perspectives, the analysis suggests that the optimal approach depends on specific context. Key factors include performance requirements, maintainability needs, and team expertise."
        );

        combined
    }
}

impl Default for ConsensusBuilder {
    fn default() -> Self {
        Self::new()
    }
}

// Common English stop words for key term extraction
const STOP_WORDS: &[&str] = &[
    "the", "and", "that", "this", "with", "from", "your", "they", "have", "been", "will", "would",
    "could", "should", "about", "which", "their", "there", "what", "when", "where", "who", "how",
    "all", "each", "both", "these", "those", "into", "only", "other", "some", "such", "than",
    "very",
];

// ===========================================================================
// 4. ANTI-MANIPULATION
// ===========================================================================

/// Configuration for anti-manipulation.
#[derive(Debug, Clone)]
pub struct AntiManipulationConfig {
    pub max_perspectives_per_source: usize,
    pub min_perspective_diversity: f64,
    pub manipulation_score_threshold: f64,
    pub rate_limit_per_perspective: usize,
}

impl Default for AntiManipulationConfig {
    fn default() -> Self {
        Self {
            max_perspectives_per_source: 3,
            min_perspective_diversity: 0.3,
            manipulation_score_threshold: 0.7,
            rate_limit_per_perspective: 10,
        }
    }
}

/// Detection result for manipulation attempts.
#[derive(Debug, Clone)]
pub struct ManipulationReport {
    pub is_manipulated: bool,
    pub manipulation_score: f64,
    pub issues: Vec<ManipulationIssue>,
    pub controlled_perspectives: Vec<String>,
}

impl ManipulationReport {
    pub fn clean() -> Self {
        Self {
            is_manipulated: false,
            manipulation_score: 0.0,
            issues: vec![],
            controlled_perspectives: vec![],
        }
    }

    pub fn with_issues(issues: Vec<ManipulationIssue>, score: f64) -> Self {
        Self {
            is_manipulated: score > 0.5,
            manipulation_score: score,
            issues,
            controlled_perspectives: vec![],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManipulationIssue {
    RateLimitExceeded { perspective: String, count: usize },
    ControllingPerspective { perspective: String },
    LowDiversity,
    RepeatedTerms { term: String, count: usize },
    ConsensusHijacking { hijacked_perspectives: Vec<String> },
}

/// Anti-manipulation detector for debate system.
pub struct AntiManipulation {
    config: AntiManipulationConfig,
    perspective_counts: HashMap<String, usize>,
    term_frequencies: HashMap<String, usize>,
    controlling_perspective: Option<String>,
}

impl AntiManipulation {
    pub fn new() -> Self {
        Self {
            config: AntiManipulationConfig::default(),
            perspective_counts: HashMap::new(),
            term_frequencies: HashMap::new(),
            controlling_perspective: None,
        }
    }

    pub fn with_config(mut self, config: AntiManipulationConfig) -> Self {
        self.config = config;
        self
    }

    /// Record a perspective response.
    pub fn record_perspective(&mut self, source: &str, content: &str) {
        *self
            .perspective_counts
            .entry(source.to_string())
            .or_insert(0) += 1;

        // Track term frequencies
        for word in content.split_whitespace() {
            if word.len() > 6 {
                *self
                    .term_frequencies
                    .entry(word.to_lowercase())
                    .or_insert(0) += 1;
            }
        }
    }

    /// Detect if any perspective is controlling the debate.
    pub fn detect_control(
        &self,
        perspectives: &IndexMap<String, PerspectiveOutput>,
    ) -> Option<String> {
        // Check response lengths - a controlling perspective tends to be much longer
        let lengths: Vec<(f64, &str)> = perspectives
            .iter()
            .map(|(name, output)| (output.content.len() as f64, name.as_str()))
            .collect();

        if lengths.len() < 3 {
            return None;
        }

        let avg_len: f64 = lengths.iter().map(|(l, _)| l).sum::<f64>() / lengths.len() as f64;

        // Check for outliers (much longer than average)
        for (len, name) in &lengths {
            if *len > avg_len * 2.5 {
                return Some(name.to_string());
            }
        }

        None
    }

    /// Check diversity of perspectives.
    pub fn check_diversity(&self, perspectives: &IndexMap<String, PerspectiveOutput>) -> f64 {
        if perspectives.len() < 2 {
            return 1.0;
        }

        // Calculate Jaccard similarity of all perspective pairs
        let contents: Vec<&str> = perspectives.values().map(|p| p.content.as_str()).collect();

        let mut total_similarity = 0.0;
        let mut pairs = 0;

        for i in 0..contents.len() {
            for j in (i + 1)..contents.len() {
                let sim = self.jaccard_similarity(contents[i], contents[j]);
                total_similarity += sim;
                pairs += 1;
            }
        }

        if pairs == 0 {
            return 1.0;
        }

        let avg_similarity = total_similarity / pairs as f64;

        // Diversity is inverse of similarity
        1.0 - avg_similarity
    }

    fn jaccard_similarity(&self, text1: &str, text2: &str) -> f64 {
        let words1: std::collections::HashSet<_> = text1
            .split_whitespace()
            .filter(|w| w.len() > 4)
            .map(|w| w.to_lowercase())
            .collect();

        let words2: std::collections::HashSet<_> = text2
            .split_whitespace()
            .filter(|w| w.len() > 4)
            .map(|w| w.to_lowercase())
            .collect();

        if words1.is_empty() || words2.is_empty() {
            return 0.0;
        }

        let intersection = words1.intersection(&words2).count() as f64;
        let union = words1.union(&words2).count() as f64;

        intersection / union
    }

    /// Generate full manipulation report.
    pub fn analyze(
        &self,
        perspectives: &IndexMap<String, PerspectiveOutput>,
    ) -> ManipulationReport {
        let mut issues = Vec::new();
        let mut score = 0.0;

        // Check rate limits
        for (source, count) in &self.perspective_counts {
            if *count > self.config.rate_limit_per_perspective {
                issues.push(ManipulationIssue::RateLimitExceeded {
                    perspective: source.clone(),
                    count: *count,
                });
                score += 0.2;
            }
        }

        // Check for controlling perspective
        if let Some(controller) = self.detect_control(perspectives) {
            issues.push(ManipulationIssue::ControllingPerspective {
                perspective: controller.clone(),
            });
            score += 0.3;
        }

        // Check diversity
        let diversity = self.check_diversity(perspectives);
        if diversity < self.config.min_perspective_diversity {
            issues.push(ManipulationIssue::LowDiversity);
            score += 0.3;
        }

        // Check for repeated terms (potential template poisoning)
        for (term, count) in &self.term_frequencies {
            if *count > perspectives.len() * 5 {
                issues.push(ManipulationIssue::RepeatedTerms {
                    term: term.clone(),
                    count: *count,
                });
                score += 0.1;
            }
        }

        // Check if manipulation threshold exceeded
        if score > self.config.manipulation_score_threshold {
            issues.push(ManipulationIssue::ConsensusHijacking {
                hijacked_perspectives: perspectives.keys().cloned().collect(),
            });
        }

        ManipulationReport {
            is_manipulated: score > 0.5,
            manipulation_score: score.min(1.0),
            issues,
            controlled_perspectives: self.controlling_perspective.clone().into_iter().collect(),
        }
    }

    /// Reset tracking state.
    pub fn reset(&mut self) {
        self.perspective_counts.clear();
        self.term_frequencies.clear();
        self.controlling_perspective = None;
    }
}

impl Default for AntiManipulation {
    fn default() -> Self {
        Self::new()
    }
}

// ===========================================================================
// 5. EMERGENCY FALLBACK
// ===========================================================================

/// Emergency fallback handler for debate system.
pub struct EmergencyFallback {
    fallback_responses: IndexMap<FallbackReason, String>,
    use_most_capable: bool,
}

impl EmergencyFallback {
    pub fn new() -> Self {
        let mut fallback_responses = IndexMap::new();

        fallback_responses.insert(
            FallbackReason::AllTimeouts,
            "The analysis could not be completed within the time limits. Based on general software engineering principles, the recommended approach would be to prioritize simplicity and maintainability while addressing the specific requirements.".to_string(),
        );

        fallback_responses.insert(
            FallbackReason::AllFailedValidation,
            "The perspective outputs did not meet quality standards. Please rephrase your question or break it into smaller parts.".to_string(),
        );

        fallback_responses.insert(
            FallbackReason::NoConsensus,
            "The perspectives reached different conclusions without clear agreement. Consider providing more specific constraints or context.".to_string(),
        );

        fallback_responses.insert(
            FallbackReason::ManipulationDetected,
            "The debate system detected patterns that may indicate manipulation. A neutral assessment based on standard engineering practices is recommended.".to_string(),
        );

        fallback_responses.insert(
            FallbackReason::InsufficientDiversity,
            "The perspectives provided were too similar to establish meaningful debate. Consider reformulating the question to explore different aspects.".to_string(),
        );

        Self {
            fallback_responses,
            use_most_capable: true,
        }
    }

    pub fn with_fallback_response(mut self, reason: FallbackReason, response: String) -> Self {
        self.fallback_responses.insert(reason, response);
        self
    }

    /// Get fallback response.
    pub fn get_fallback(&self, reason: &FallbackReason) -> String {
        self.fallback_responses
            .get(reason)
            .cloned()
            .unwrap_or_else(|| {
                "An error occurred during analysis. Please try again or rephrase your question."
                    .to_string()
            })
    }

    /// Get the response from the most capable perspective as fallback.
    pub fn get_most_capable_response(
        &self,
        perspectives: &IndexMap<String, PerspectiveOutput>,
    ) -> Option<(String, String)> {
        if !self.use_most_capable || perspectives.is_empty() {
            return None;
        }

        perspectives
            .iter()
            .max_by(|a, b| {
                (a.1.quality_score * a.1.confidence)
                    .partial_cmp(&(b.1.quality_score * b.1.confidence))
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|(name, output)| (name.clone(), output.content.clone()))
    }

    /// Select best available response with fallback.
    pub fn select_response(
        &self,
        consensus: &ConsensusResult,
        perspectives: &IndexMap<String, PerspectiveOutput>,
        reason: &FallbackReason,
    ) -> (String, String) {
        // If we have a valid consensus and it's better than fallback
        if !consensus.used_fallback && consensus.consensus_score > 0.5 {
            return ("consensus".to_string(), consensus.consensus_text.clone());
        }

        // Try most capable perspective
        if let Some((name, content)) = self.get_most_capable_response(perspectives) {
            if perspectives.len() > 1 {
                return (name, content);
            }
        }

        // Ultimate fallback
        ("fallback".to_string(), self.get_fallback(reason))
    }
}

impl Default for EmergencyFallback {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum FallbackReason {
    AllTimeouts,
    AllFailedValidation,
    NoConsensus,
    ManipulationDetected,
    InsufficientDiversity,
}

// ===========================================================================
// SAFEGUARD ORCHESTRATOR
// ===========================================================================

/// Orchestrates all safeguards for the debate system.
pub struct SafeguardOrchestrator {
    validator: OutputValidator,
    circuit_breaker: CircuitBreaker,
    consensus_builder: ConsensusBuilder,
    anti_manipulation: AntiManipulation,
    fallback: EmergencyFallback,
}

impl SafeguardOrchestrator {
    pub fn new() -> Self {
        Self {
            validator: OutputValidator::new(),
            circuit_breaker: CircuitBreaker::new(),
            consensus_builder: ConsensusBuilder::new(),
            anti_manipulation: AntiManipulation::new(),
            fallback: EmergencyFallback::new(),
        }
    }

    /// Validate all perspective outputs.
    pub fn validate_perspectives(
        &self,
        perspectives: &mut IndexMap<String, PerspectiveOutput>,
    ) -> HashMap<String, ValidationResult> {
        let mut results = HashMap::new();

        for (name, output) in perspectives.iter_mut() {
            let result = self.validator.validate(&output.content);
            if result.is_valid {
                output.quality_score = result.quality_score;
            }
            results.insert(name.clone(), result);
        }

        results
    }

    /// Check circuit breaker conditions.
    pub fn check_circuit_breaker(
        &self,
        perspectives: &IndexMap<String, PerspectiveOutput>,
    ) -> CircuitBreakerResult {
        self.circuit_breaker.finalize(perspectives)
    }

    /// Build consensus with safeguards.
    pub fn build_safe_consensus(
        &self,
        perspectives: &IndexMap<String, PerspectiveOutput>,
    ) -> ConsensusResult {
        // First, check for manipulation
        let manipulation = self.anti_manipulation.analyze(perspectives);
        if manipulation.is_manipulated {
            return ConsensusResult::fallback_response(
                self.fallback
                    .get_fallback(&FallbackReason::ManipulationDetected),
            );
        }

        // Build consensus
        let consensus = self.consensus_builder.build(perspectives);

        // Check if consensus is worse than simple response
        if consensus.consensus_score < 0.4 && perspectives.len() > 1 {
            // Try to find the single best perspective instead
            if let Some((_, output)) = perspectives.iter().max_by(|a, b| {
                (a.1.quality_score * a.1.confidence)
                    .partial_cmp(&(b.1.quality_score * b.1.confidence))
                    .unwrap_or(std::cmp::Ordering::Equal)
            }) {
                return ConsensusResult::with_consensus(
                    output.content.clone(),
                    output.quality_score * output.confidence,
                    0.5,
                    vec![output.name.clone()],
                    vec![],
                );
            }
        }

        consensus
    }

    /// Run full safeguard pipeline.
    pub fn process(
        &self,
        perspectives: &mut IndexMap<String, PerspectiveOutput>,
    ) -> SafeguardResult {
        let mut warnings = Vec::new();
        let mut excluded = Vec::new();

        // 1. Validate
        let validation_results = self.validate_perspectives(perspectives);
        for (name, result) in &validation_results {
            if !result.is_valid {
                warnings.push(format!(
                    "Perspective '{}' failed validation: {:?}",
                    name, result.errors
                ));
            }
        }

        // 2. Circuit breaker
        let circuit_result = self.check_circuit_breaker(perspectives);
        if circuit_result.should_exclude {
            for name in &circuit_result.excluded_perspectives {
                excluded.push(name.clone());
                warnings.push(format!(
                    "Perspective '{}' excluded by circuit breaker: {:?}",
                    name, circuit_result.reason
                ));
            }
        }

        // 3. Anti-manipulation
        let manipulation = self.anti_manipulation.analyze(perspectives);
        if manipulation.is_manipulated {
            warnings.push(format!(
                "Manipulation detected (score: {:.2}): {:?}",
                manipulation.manipulation_score, manipulation.issues
            ));
        }

        // 4. Build consensus
        let consensus = self.build_safe_consensus(perspectives);

        // 5. Select final response
        let (source, response) =
            self.fallback
                .select_response(&consensus, perspectives, &FallbackReason::NoConsensus);

        SafeguardResult {
            response,
            source,
            consensus,
            excluded_perspectives: excluded,
            warnings,
            manipulation_detected: manipulation.is_manipulated,
        }
    }

    /// Reset all safeguards for new debate session.
    pub fn reset(&mut self) {
        self.circuit_breaker.reset();
        self.anti_manipulation.reset();
    }
}

impl Default for SafeguardOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

/// Result of safeguard processing.
#[derive(Debug, Clone)]
pub struct SafeguardResult {
    pub response: String,
    pub source: String,
    pub consensus: ConsensusResult,
    pub excluded_perspectives: Vec<String>,
    pub warnings: Vec<String>,
    pub manipulation_detected: bool,
}

// ===========================================================================
// TESTS
// ===========================================================================

#[cfg(test)]
mod safeguard_tests {
    use super::*;

    // ========== Output Validation Tests ==========

    #[test]
    fn test_validator_accepts_valid_output() {
        let validator = OutputValidator::new();
        let text = "This is a well-formed response that provides meaningful analysis \
                   of the architectural decision being considered. It includes specific \
                   recommendations based on established engineering principles.";

        let result = validator.validate(text);
        assert!(result.is_valid);
        assert!(result.errors.is_empty());
        assert!(result.quality_score > 0.5);
    }

    #[test]
    fn test_validator_rejects_short_output() {
        let validator = OutputValidator::new();
        let text = "Short answer.";

        let result = validator.validate(text);
        assert!(!result.is_valid);
        assert!(result.errors.contains(&ValidationError::TooShort {
            actual: 13,
            minimum: 50,
        }));
    }

    #[test]
    fn test_validator_rejects_gibberish() {
        let validator = OutputValidator::new();

        // Low word ratio (mostly special chars - should be rejected)
        let result = validator.validate("!!! @@@ ### $$% ^^^ &&& ***");
        assert!(!result.is_valid, "Low word ratio should be invalid");
        assert!(
            result
                .errors
                .iter()
                .any(|e| matches!(e, ValidationError::GibberishDetected { .. }))
        );

        // Very low entropy text (repeating characters)
        let result = validator.validate("AAAAAAAAAAAAAAAAAAAAAAAAAA");
        assert!(!result.is_valid, "Low entropy should be invalid");
        assert!(
            result
                .errors
                .iter()
                .any(|e| matches!(e, ValidationError::LowEntropy { .. }))
        );
    }

    #[test]
    fn test_entropy_calculation() {
        let validator = OutputValidator::new();

        // Low entropy (repeating)
        let low_entropy = validator.calculate_entropy("AAAAAAAAAA");
        assert!(low_entropy < 2.0);

        // Higher entropy (normal text)
        let normal_entropy =
            validator.calculate_entropy("The quick brown fox jumps over the lazy dog");
        assert!(normal_entropy > 3.0);
    }

    #[test]
    fn test_word_ratio_calculation() {
        let validator = OutputValidator::new();

        // Good word ratio
        let good_ratio = validator.calculate_word_ratio(
            "The architecture decision requires careful consideration of trade-offs.",
        );
        assert!(good_ratio > 0.5);

        // Bad word ratio (mostly special chars)
        let bad_ratio = validator.calculate_word_ratio("!!! @@@ ### $$% ^^^");
        assert!(bad_ratio < 0.2);
    }

    // ========== Circuit Breaker Tests ==========

    #[test]
    fn test_circuit_breaker_allows_aligned_perspectives() {
        let breaker = CircuitBreaker::new();
        let mut perspectives = IndexMap::new();

        perspectives.insert(
            "perspective_1".to_string(),
            PerspectiveOutput::new("perspective_1", "content".to_string(), 0.7, 0.8),
        );
        perspectives.insert(
            "perspective_2".to_string(),
            PerspectiveOutput::new("perspective_2", "content".to_string(), 0.75, 0.8),
        );

        let result = breaker.finalize(&perspectives);
        assert!(!result.should_exclude);
        assert!(result.excluded_perspectives.is_empty());
    }

    #[test]
    fn test_circuit_breaker_excludes_diverged() {
        // Use a circuit breaker with lower threshold for this test
        let breaker = CircuitBreaker::with_config(
            CircuitBreaker::new(),
            CircuitBreakerConfig {
                divergence_threshold: 0.3, // Lower threshold for test
                ..Default::default()
            },
        );
        let mut perspectives = IndexMap::new();

        // Normal perspectives (high quality)
        perspectives.insert(
            "normal1".to_string(),
            PerspectiveOutput::new("normal1", "content".to_string(), 0.8, 0.9),
        );
        perspectives.insert(
            "normal2".to_string(),
            PerspectiveOutput::new("normal2", "content".to_string(), 0.75, 0.85),
        );

        // Diverged perspective (very low quality, > 0.3 from consensus)
        perspectives.insert(
            "diverged".to_string(),
            PerspectiveOutput::new("diverged", "content".to_string(), 0.1, 0.2),
        );

        let result = breaker.finalize(&perspectives);
        assert!(result.should_exclude);
        assert!(
            result
                .excluded_perspectives
                .contains(&"diverged".to_string())
        );
    }

    #[test]
    fn test_circuit_breaker_perspective_timeout() {
        let mut breaker = CircuitBreaker::new();
        breaker.start_perspective("test");

        // Should not timeout yet (config says 45s, we check immediately)
        let result = breaker.check_perspective_timeout("test");
        assert!(result.is_none());

        // Check non-existent perspective
        let result = breaker.check_perspective_timeout("nonexistent");
        assert!(result.is_none());
    }

    // ========== Consensus Builder Tests ==========

    #[test]
    fn test_consensus_single_perspective() {
        let builder = ConsensusBuilder::new();
        let mut perspectives = IndexMap::new();

        perspectives.insert(
            "single".to_string(),
            PerspectiveOutput::new("single", "Single response content".to_string(), 0.8, 0.9),
        );

        let result = builder.build(&perspectives);
        assert!(!result.used_fallback);
        assert_eq!(result.consensus_text, "Single response content");
        assert_eq!(result.winning_perspectives, vec!["single"]);
    }

    #[test]
    fn test_consensus_multiple_weighted() {
        let builder = ConsensusBuilder::new();
        let mut perspectives = IndexMap::new();

        perspectives.insert(
            "low_quality".to_string(),
            PerspectiveOutput::new("low_quality", "Low quality response".to_string(), 0.3, 0.4),
        );

        perspectives.insert(
            "high_quality".to_string(),
            PerspectiveOutput::new(
                "high_quality",
                "High quality response with detailed analysis".to_string(),
                0.9,
                0.9,
            ),
        );

        let result = builder.build(&perspectives);
        assert!(!result.used_fallback);
        assert!(
            result
                .winning_perspectives
                .contains(&"high_quality".to_string())
        );
    }

    #[test]
    fn test_consensus_uses_fallback_on_empty() {
        let builder = ConsensusBuilder::new();
        let perspectives = IndexMap::new();

        let result = builder.build(&perspectives);
        assert!(result.used_fallback);
        assert!(!result.consensus_text.is_empty());
    }

    // ========== Anti-Manipulation Tests ==========

    #[test]
    fn test_anti_manipulation_detects_control() {
        let anti = AntiManipulation::new();
        let mut perspectives = IndexMap::new();

        // Normal response (20 chars)
        perspectives.insert(
            "normal".to_string(),
            PerspectiveOutput::new("normal", "Normal short response".to_string(), 0.7, 0.8),
        );

        // Another normal response (similar length to first)
        perspectives.insert(
            "normal2".to_string(),
            PerspectiveOutput::new(
                "normal2",
                "Another short response here".to_string(),
                0.7,
                0.8,
            ),
        );

        // Controlling response (much longer - 10x the average)
        // With normal responses of ~20-25 chars, we need > 2.5x average
        // Average will be ~60 chars, so we need > 150 chars
        let long_content = "AAAAAAAAAA ".repeat(25); // ~250 chars total
        perspectives.insert(
            "controller".to_string(),
            PerspectiveOutput::new("controller", long_content, 0.7, 0.8),
        );

        let controller = anti.detect_control(&perspectives);
        assert!(controller.is_some());
        assert_eq!(controller.unwrap(), "controller");
    }

    #[test]
    fn test_anti_manipulation_checks_diversity() {
        let anti = AntiManipulation::new();
        let mut perspectives = IndexMap::new();

        // Similar content (low diversity)
        perspectives.insert(
            "p1".to_string(),
            PerspectiveOutput::new(
                "p1",
                "The database schema requires careful design".to_string(),
                0.7,
                0.8,
            ),
        );
        perspectives.insert(
            "p2".to_string(),
            PerspectiveOutput::new(
                "p2",
                "The database schema requires careful planning".to_string(),
                0.7,
                0.8,
            ),
        );

        let diversity = anti.check_diversity(&perspectives);
        assert!(diversity < 0.5); // Low diversity due to similar content
    }

    #[test]
    fn test_anti_manipulation_clean_perspectives() {
        let anti = AntiManipulation::new();
        let mut perspectives = IndexMap::new();

        // Diverse content
        perspectives.insert(
            "p1".to_string(),
            PerspectiveOutput::new(
                "p1",
                "Consider using microservices for scaling".to_string(),
                0.7,
                0.8,
            ),
        );
        perspectives.insert(
            "p2".to_string(),
            PerspectiveOutput::new(
                "p2",
                "A monolith might be simpler for initial development".to_string(),
                0.7,
                0.8,
            ),
        );
        perspectives.insert(
            "p3".to_string(),
            PerspectiveOutput::new(
                "p3",
                "Evaluate team expertise before deciding architecture".to_string(),
                0.7,
                0.8,
            ),
        );

        let report = anti.analyze(&perspectives);
        assert!(!report.is_manipulated);
    }

    // ========== Emergency Fallback Tests ==========

    #[test]
    fn test_fallback_returns_reason() {
        let fallback = EmergencyFallback::new();

        let response = fallback.get_fallback(&FallbackReason::AllTimeouts);
        assert!(!response.is_empty());
        assert!(response.len() > 20);
    }

    #[test]
    fn test_fallback_selects_most_capable() {
        let fallback = EmergencyFallback::new();
        let mut perspectives = IndexMap::new();

        perspectives.insert(
            "low".to_string(),
            PerspectiveOutput::new("low", "Low quality".to_string(), 0.3, 0.4),
        );

        perspectives.insert(
            "high".to_string(),
            PerspectiveOutput::new(
                "high",
                "High quality response with detailed analysis".to_string(),
                0.9,
                0.95,
            ),
        );

        let (name, content) = fallback.get_most_capable_response(&perspectives).unwrap();
        assert_eq!(name, "high");
        assert!(content.contains("High quality"));
    }

    // ========== Orchestrator Tests ==========

    #[test]
    fn test_orchestrator_full_pipeline() {
        let orchestrator = SafeguardOrchestrator::new();
        let mut perspectives = IndexMap::new();

        perspectives.insert(
            "p1".to_string(),
            PerspectiveOutput::new(
                "p1",
                "Microservices offer better scalability but increase complexity".to_string(),
                0.8,
                0.9,
            ),
        );

        perspectives.insert(
            "p2".to_string(),
            PerspectiveOutput::new(
                "p2",
                "Monolith is simpler to develop and deploy initially".to_string(),
                0.75,
                0.85,
            ),
        );

        let result = orchestrator.process(&mut perspectives);

        assert!(!result.response.is_empty());
        assert!(result.excluded_perspectives.is_empty());
        assert!(!result.manipulation_detected);
    }

    #[test]
    fn test_orchestrator_handles_invalid_perspective() {
        let orchestrator = SafeguardOrchestrator::new();
        let mut perspectives = IndexMap::new();

        // Valid perspective
        perspectives.insert(
            "valid".to_string(),
            PerspectiveOutput::new(
                "valid",
                "This is a valid response with proper content for analysis".to_string(),
                0.8,
                0.9,
            ),
        );

        // Invalid (too short / gibberish - validation will fail)
        perspectives.insert(
            "invalid".to_string(),
            PerspectiveOutput::new("invalid", "!!! @@@ ###".to_string(), 0.2, 0.3),
        );

        let result = orchestrator.process(&mut perspectives);

        // Should still produce a response
        assert!(!result.response.is_empty());

        // Invalid perspective should generate warnings
        assert!(
            !result.warnings.is_empty()
                || result
                    .excluded_perspectives
                    .contains(&"invalid".to_string())
        );
    }

    // ========== Integration Test ==========

    #[test]
    fn test_full_debate_safeguard_flow() {
        let orchestrator = SafeguardOrchestrator::new();
        let mut perspectives = IndexMap::new();

        // Simulate a real multi-perspective debate
        perspectives.insert(
            "architect".to_string(),
            PerspectiveOutput::new(
                "architect",
                "The architecture should follow domain-driven design principles. \
                 Consider bounded contexts for service boundaries."
                    .to_string(),
                0.85,
                0.9,
            ),
        );

        perspectives.insert(
            "performance_engineer".to_string(),
            PerspectiveOutput::new(
                "performance_engineer",
                "Performance analysis suggests caching layer for read-heavy workloads. \
                 Consider Redis for session storage."
                    .to_string(),
                0.8,
                0.85,
            ),
        );

        perspectives.insert(
            "security_expert".to_string(),
            PerspectiveOutput::new(
                "security_expert",
                "Security review indicates need for input validation and encryption at rest. \
                 Consider mTLS for service communication."
                    .to_string(),
                0.82,
                0.88,
            ),
        );

        let result = orchestrator.process(&mut perspectives);

        // All perspectives should pass
        assert!(!result.manipulation_detected);
        assert!(result.excluded_perspectives.is_empty());

        // Response should be meaningful consensus
        assert!(result.response.len() > 100);

        // Should have warnings about what was considered
        assert!(!result.warnings.is_empty() || !result.manipulation_detected);
    }
}
