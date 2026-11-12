//! Lifecycle hook configuration.

use serde::{Deserialize, Serialize};

/// Lifecycle hooks: external commands jcode runs at well-defined points.
///
/// Hook commands are parsed shell-style (quotes work) but executed directly,
/// with `JCODE_HOOK_*` env vars describing the event (`JCODE_HOOK_EVENT`,
/// `JCODE_HOOK_SESSION_ID`, `JCODE_HOOK_CWD`, event-specific fields, and a
/// `JCODE_HOOK_PAYLOAD` JSON mirror). Hook processes get
/// `JCODE_HOOKS_DISABLED=1` so nested jcode invocations don't recurse.
///
/// All hooks except `pre_tool` are observers: detached, fire-and-forget,
/// failures only logged. `pre_tool` is a gate: jcode waits for it and exit
/// code 2 blocks the tool call (stderr becomes the error shown to the model);
/// exit 0 allows; anything else fails open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookCommands(Vec<String>);

impl HookCommands {
    pub fn one(command: impl Into<String>) -> Self {
        Self(vec![command.into()])
    }

    pub fn many(commands: Vec<String>) -> Self {
        Self(commands)
    }

    pub fn iter(&self) -> impl Iterator<Item = &str> {
        self.0.iter().map(String::as_str)
    }

    pub fn first(&self) -> Option<&str> {
        self.0.first().map(String::as_str)
    }
}

impl Serialize for HookCommands {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        if let [command] = self.0.as_slice() {
            command.serialize(serializer)
        } else {
            self.0.serialize(serializer)
        }
    }
}

impl<'de> Deserialize<'de> for HookCommands {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum OneOrMany {
            One(String),
            Many(Vec<String>),
        }

        Ok(match OneOrMany::deserialize(deserializer)? {
            OneOrMany::One(command) => Self::one(command),
            OneOrMany::Many(commands) => Self::many(commands),
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct HooksConfig {
    /// Synchronous input transformers run before each tool call. Each receives
    /// the tool input JSON on stdin and may print replacement JSON on stdout.
    /// Empty stdout or any failure leaves the input unchanged.
    pub pre_tool_transform: Option<HookCommands>,
    /// Max milliseconds to wait for each input transformer (default: 500).
    pub pre_tool_transform_timeout_ms: u64,
    /// Runs when an agent turn begins (after the user message is added and
    /// before the model starts generating). Fires before the first `pre_tool`,
    /// so integrations can detect that the agent is actively working even while
    /// it is only thinking/streaming text. Fields: MODEL, SOURCE
    /// ("chat"/"resume"/"ambient"). Env override: JCODE_HOOK_TURN_START.
    pub turn_start: Option<HookCommands>,
    /// Runs when an agent turn completes.
    /// Fields: STATUS ("ok"/"error"), DURATION_MS, MODEL, LAST_ASSISTANT_TEXT.
    /// Env override: JCODE_HOOK_TURN_END.
    pub turn_end: Option<HookCommands>,
    /// Runs when a session becomes active (created or resumed).
    /// Fields: SOURCE ("create"/"resume").
    /// Env override: JCODE_HOOK_SESSION_START.
    pub session_start: Option<HookCommands>,
    /// Runs when a session closes normally.
    /// Env override: JCODE_HOOK_SESSION_END.
    pub session_end: Option<HookCommands>,
    /// Gate hook before each tool call. Receives TOOL_NAME and the tool input
    /// JSON on stdin (also truncated in TOOL_INPUT). Exit 0 allows, exit 2
    /// blocks (stderr is fed back to the model), anything else fails open.
    /// Env override: JCODE_HOOK_PRE_TOOL.
    pub pre_tool: Option<HookCommands>,
    /// Runs after each tool call completes.
    /// Fields: TOOL_NAME, TOOL_CALL_ID, TOOL_INPUT, TOUCHED_PATHS, STATUS
    /// ("ok"/"error"), DURATION_MS, OUTPUT_BYTES, ERROR.
    /// Env override: JCODE_HOOK_POST_TOOL.
    pub post_tool: Option<HookCommands>,
    /// Synchronous hook after each successful tool call. Same fields as
    /// post_tool, tool input JSON on stdin. Its stdout is appended to the tool
    /// result the model sees (e.g. linter output for edited files).
    /// Env override: JCODE_HOOK_POST_TOOL_FEEDBACK.
    pub post_tool_feedback: Option<HookCommands>,
    /// Max milliseconds to wait for each post_tool_feedback hook (default:
    /// 10000). Env override: JCODE_HOOK_POST_TOOL_FEEDBACK_TIMEOUT_MS.
    pub post_tool_feedback_timeout_ms: u64,
    /// Max milliseconds to wait for the pre_tool gate before failing open
    /// (default: 5000). Env override: JCODE_HOOK_PRE_TOOL_TIMEOUT_MS.
    pub pre_tool_timeout_ms: u64,
}

impl Default for HooksConfig {
    fn default() -> Self {
        Self {
            pre_tool_transform: None,
            pre_tool_transform_timeout_ms: 500,
            turn_start: None,
            turn_end: None,
            session_start: None,
            session_end: None,
            pre_tool: None,
            post_tool: None,
            post_tool_feedback: None,
            post_tool_feedback_timeout_ms: 10_000,
            pre_tool_timeout_ms: 5000,
        }
    }
}
