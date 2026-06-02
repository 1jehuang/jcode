//! Issue #113: programmatic orchestration API.
//!
//! Tiny request → execution shim that lets external harnesses
//! (jcode-server bindings, external code review bots, CI runners,
//! testing tooling) submit a "spawn an agent task" request without
//! going through the TUI or the WebSocket gateway.
//!
//! Compared to the gateway:
//!   - In-process only (no transport)
//!   - No streaming output (request-response only)
//!   - No multi-turn (single turn per request)
//!   - No swarm coordination
//!
//! Useful for harnesses that want to drive jcode programmatically
//! and only need the "send prompt → get final result" surface.
//!
//! ## Quick start
//!
//! ```rust,no_run
//! use jcode::orchestration_api::{OrchestrationRequest, OrchestrationResponse};
//!
//! let req = OrchestrationRequest::new("Refactor the foo module")
//!     .with_session_id("ci-12345")
//!     .with_max_turns(5);
//!
//! // Caller wires up the actual driver — this module only defines
//! // the request/response shape and validation.
//! let validated = req.validate().unwrap();
//! ```
//!
//! ## Out of scope (#113 follow-ups)
//!
//! - Driver implementation (spawning an agent + streaming the
//!   result back). Will land once the swarm refactor #54 lands so
//!   we can hook into the same orchestrator.
//! - Permission gating (which prompts a harness can run)
//! - Cost limits per-request
//! - Multi-turn sessions

use serde::{Deserialize, Serialize};

/// What the harness wants the agent to do.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrchestrationRequest {
    /// User-facing prompt text. Must not be empty.
    pub prompt: String,

    /// Optional session id. If `None`, a fresh ephemeral session
    /// is created. If `Some`, the agent attaches to / resumes that
    /// session (driver-dependent).
    #[serde(default)]
    pub session_id: Option<String>,

    /// Cap on agent turns for this request. Default = 10.
    /// Hard maximum = 100 (enforced by validate()).
    #[serde(default = "default_max_turns")]
    pub max_turns: u32,

    /// Working directory for tool calls. If `None`, the driver
    /// chooses (typically the harness's CWD).
    #[serde(default)]
    pub working_dir: Option<String>,

    /// Optional list of tool names to allow-list. `None` =
    /// driver's default tool set. Empty = no tools (chat-only).
    #[serde(default)]
    pub allowed_tools: Option<Vec<String>>,
}

fn default_max_turns() -> u32 {
    10
}

impl OrchestrationRequest {
    pub fn new(prompt: impl Into<String>) -> Self {
        Self {
            prompt: prompt.into(),
            session_id: None,
            max_turns: default_max_turns(),
            working_dir: None,
            allowed_tools: None,
        }
    }

    pub fn with_session_id(mut self, id: impl Into<String>) -> Self {
        self.session_id = Some(id.into());
        self
    }

    pub fn with_max_turns(mut self, n: u32) -> Self {
        self.max_turns = n;
        self
    }

    pub fn with_working_dir(mut self, dir: impl Into<String>) -> Self {
        self.working_dir = Some(dir.into());
        self
    }

    pub fn with_allowed_tools(mut self, tools: Vec<String>) -> Self {
        self.allowed_tools = Some(tools);
        self
    }

    /// Reject malformed requests up-front so drivers don't have to.
    pub fn validate(&self) -> Result<&Self, OrchestrationError> {
        if self.prompt.trim().is_empty() {
            return Err(OrchestrationError::EmptyPrompt);
        }
        if self.max_turns == 0 {
            return Err(OrchestrationError::ZeroMaxTurns);
        }
        if self.max_turns > 100 {
            return Err(OrchestrationError::MaxTurnsExceedsCap {
                requested: self.max_turns,
                cap: 100,
            });
        }
        if let Some(id) = &self.session_id
            && id.trim().is_empty()
        {
            return Err(OrchestrationError::EmptySessionId);
        }
        Ok(self)
    }
}

/// Outcome of a completed orchestration request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrchestrationResponse {
    /// Session id the request ran in (echoed back even if caller
    /// didn't supply one — useful for follow-up requests).
    pub session_id: String,
    /// Final assistant text. Empty when the agent failed before
    /// emitting any text.
    pub final_text: String,
    /// Number of agent turns consumed. Always <= max_turns.
    pub turns_used: u32,
    /// Outcome category for cheap scriptable checks.
    pub status: OrchestrationStatus,
    /// Optional human-readable error message when status != Success.
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrchestrationStatus {
    Success,
    MaxTurnsReached,
    ToolError,
    ProviderError,
    Cancelled,
}

#[derive(Debug, thiserror::Error)]
pub enum OrchestrationError {
    #[error("orchestration prompt is empty")]
    EmptyPrompt,
    #[error("max_turns must be > 0")]
    ZeroMaxTurns,
    #[error("max_turns ({requested}) exceeds hard cap ({cap})")]
    MaxTurnsExceedsCap { requested: u32, cap: u32 },
    #[error("session_id is empty (omit instead of passing empty string)")]
    EmptySessionId,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_sets_defaults() {
        let req = OrchestrationRequest::new("hello");
        assert_eq!(req.prompt, "hello");
        assert_eq!(req.session_id, None);
        assert_eq!(req.max_turns, 10);
        assert_eq!(req.working_dir, None);
        assert_eq!(req.allowed_tools, None);
    }

    #[test]
    fn builder_methods_chain() {
        let req = OrchestrationRequest::new("hi")
            .with_session_id("abc")
            .with_max_turns(3)
            .with_working_dir("/tmp/wd")
            .with_allowed_tools(vec!["read".to_string(), "edit".to_string()]);
        assert_eq!(req.session_id.as_deref(), Some("abc"));
        assert_eq!(req.max_turns, 3);
        assert_eq!(req.working_dir.as_deref(), Some("/tmp/wd"));
        assert_eq!(req.allowed_tools.unwrap(), vec!["read", "edit"]);
    }

    #[test]
    fn validate_accepts_valid_request() {
        let req = OrchestrationRequest::new("hi");
        assert!(req.validate().is_ok());
    }

    #[test]
    fn validate_rejects_empty_prompt() {
        let req = OrchestrationRequest::new("");
        let err = req.validate().unwrap_err();
        assert!(matches!(err, OrchestrationError::EmptyPrompt));
    }

    #[test]
    fn validate_rejects_whitespace_only_prompt() {
        let req = OrchestrationRequest::new("   \n\t");
        assert!(matches!(
            req.validate().unwrap_err(),
            OrchestrationError::EmptyPrompt
        ));
    }

    #[test]
    fn validate_rejects_zero_max_turns() {
        let req = OrchestrationRequest::new("hi").with_max_turns(0);
        assert!(matches!(
            req.validate().unwrap_err(),
            OrchestrationError::ZeroMaxTurns
        ));
    }

    #[test]
    fn validate_caps_max_turns() {
        let req = OrchestrationRequest::new("hi").with_max_turns(101);
        match req.validate().unwrap_err() {
            OrchestrationError::MaxTurnsExceedsCap { requested, cap } => {
                assert_eq!(requested, 101);
                assert_eq!(cap, 100);
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn validate_rejects_empty_session_id() {
        let req = OrchestrationRequest::new("hi").with_session_id("");
        assert!(matches!(
            req.validate().unwrap_err(),
            OrchestrationError::EmptySessionId
        ));
    }

    #[test]
    fn json_round_trip() {
        let req = OrchestrationRequest::new("test")
            .with_session_id("sid-1")
            .with_max_turns(7);
        let s = serde_json::to_string(&req).unwrap();
        let back: OrchestrationRequest = serde_json::from_str(&s).unwrap();
        assert_eq!(back.prompt, "test");
        assert_eq!(back.session_id.as_deref(), Some("sid-1"));
        assert_eq!(back.max_turns, 7);
    }

    #[test]
    fn response_json_round_trip() {
        let resp = OrchestrationResponse {
            session_id: "sid".to_string(),
            final_text: "done".to_string(),
            turns_used: 3,
            status: OrchestrationStatus::Success,
            error: None,
        };
        let s = serde_json::to_string(&resp).unwrap();
        assert!(s.contains("\"status\":\"success\""));
        let back: OrchestrationResponse = serde_json::from_str(&s).unwrap();
        assert_eq!(back.status, OrchestrationStatus::Success);
        assert_eq!(back.turns_used, 3);
    }

    #[test]
    fn json_default_max_turns_when_omitted() {
        let s = r#"{"prompt":"hi"}"#;
        let req: OrchestrationRequest = serde_json::from_str(s).unwrap();
        assert_eq!(req.max_turns, 10);
    }
}
