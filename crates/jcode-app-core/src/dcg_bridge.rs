//! Bridge between jcode's permission system and `dcg-core`'s
//! permission-modes API.
//!
//! jcode classifies *high-level intent strings* (e.g. `"read"`, `"memory"`,
//! `"todowrite"`). `dcg-core` evaluates *low-level tool calls*
//! (`ToolCall::Bash`, `ToolCall::Read`, …). This module is the adapter that
//! lets jcode delegate the "auto-allow vs requires-permission" decision to
//! `dcg-core::Engine` while preserving jcode's own queue / TUI / notification
//! plumbing.
//!
//! # Wiring
//!
//! `SafetySystem::classify(action)` calls into [`classify_via_dcg`] which:
//!
//! 1. Maps the action name to a [`dcg_core::ToolCall`] and an effect set.
//! 2. Calls [`dcg_core::Engine::evaluate`] with the configured [`Mode`].
//! 3. Returns `AutoAllowed` for `Decision::Allow`, otherwise
//!    `RequiresPermission`.
//!
//! ## What changes vs. the old `AUTO_ALLOWED` table
//!
//! - The hard-coded list is gone. Whether an action auto-allows now depends
//!   on the **mode** (`Plan`/`AcceptEdits`/`Default`/`BypassPermissions`/
//!   `DontAsk`/`Auto`) and the action's **effect classification**, not on a
//!   string match.
//! - Read-only tools (`read`, `glob`, `grep`, `ls`, `codesearch`, plus the
//!   `*_search` variants and todo / memory readers) carry only
//!   [`Effect::Read`] / [`Effect::Fs`] and therefore auto-allow under
//!   `Plan`, `Default`, `Auto`, `AcceptEdits`, `BypassPermissions`.
//! - Write-shaped tools (`todowrite`, `memory`, etc.) carry
//!   [`Effect::Write`] + [`Effect::Fs`]: auto-allow under `AcceptEdits`,
//!   `Default`, `Auto`, `BypassPermissions`; **deny under `Plan`** (which is
//!   read-only); prompt under `DontAsk` only if explicitly allow-listed.

use std::path::PathBuf;
use std::sync::{LazyLock, Mutex};

use dcg_core::{Decision, Effect, Engine, EngineConfig, Mode, Session, ToolCall};

pub use crate::yolo_classifier::YoloClassifier;

/// Globally configured permission mode. Set once during CLI startup, read
/// from every `SafetySystem::classify` call.
///
/// Defaults to `Mode::Default` so behavior matches the old hard-coded
/// AUTO_ALLOWED list as closely as possible (read-only tools auto-allow,
/// everything else requires permission via `fallthrough_allows == true`
/// for `Default` plus our effect mapping below).
static GLOBAL_MODE: LazyLock<Mutex<Mode>> = LazyLock::new(|| Mutex::new(Mode::Default));

/// Per-process [`dcg_core::Engine`]. Built lazily on first `classify` call.
/// jcode runs with a single engine because protected paths and the working
/// dir are stable for the lifetime of the process.
static ENGINE: LazyLock<Engine> = LazyLock::new(|| {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
    Engine::new(
        EngineConfig::builder()
            .working_dir(cwd.clone())
            .protected_paths(default_protected_paths())
            .build(),
    )
});

/// Per-process [`dcg_core::Session`]. Used by `Engine::evaluate` for
/// allow-once cache and deny counters. jcode's existing
/// `PermissionRequest` queue handles the human-prompt flow, so the
/// `Session` stays jcode-internal: it never crosses out to the user.
static SESSION: LazyLock<Mutex<Session>> = LazyLock::new(|| {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
    Mutex::new(Session::with_working_dir(cwd))
});

/// Paths that should always escalate to a prompt regardless of mode
/// (matches the conservative defaults used by Claude Code).
fn default_protected_paths() -> Vec<String> {
    vec![
        "~/.ssh".to_string(),
        "~/.aws".to_string(),
        "~/.config/gh".to_string(),
        ".git".to_string(),
        ".env".to_string(),
    ]
}

/// Set the global permission mode. Called from the CLI / config layer at
/// process startup. Subsequent `classify` calls observe the new mode.
pub fn set_mode(mode: Mode) {
    if let Ok(mut guard) = GLOBAL_MODE.lock() {
        *guard = mode;
    }
}

/// Return the currently configured permission mode.
#[must_use]
pub fn current_mode() -> Mode {
    GLOBAL_MODE
        .lock()
        .map(|guard| *guard)
        .unwrap_or(Mode::Default)
}

/// Three-state outcome from the bridge. jcode's `SafetySystem` collapses
/// `Allow` to `ActionTier::AutoAllowed` and `Prompt`/`Deny` to
/// `ActionTier::RequiresPermission` — but exposing the full set here
/// lets future call sites (e.g. CLI hooks, MCP servers) react to a hard
/// `Deny` without surfacing a permission prompt the user can never
/// usefully approve.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BridgeDecision {
    /// dcg-core allowed the action under the current mode.
    Allow,
    /// dcg-core wants a human prompt; jcode should queue a
    /// `PermissionRequest`.
    Prompt,
    /// dcg-core denied outright (e.g. `Plan` mode + write effect).
    Deny,
}

/// Classify a jcode action via dcg-core. The caller is responsible for
/// translating the result into its own `ActionTier` / `PermissionResult`
/// vocabulary.
#[must_use]
pub fn classify(action: &str) -> BridgeDecision {
    classify_with_mode(action, current_mode())
}

/// Same as [`classify`] but with an explicit mode override (mainly for
/// tests).
#[must_use]
pub fn classify_with_mode(action: &str, mode: Mode) -> BridgeDecision {
    let lower = action.to_lowercase();

    // Phase-A behavior preservation:
    //
    // `dcg-core` Phase A does not yet expose a rule layer, so
    // `Mode::Default::fallthrough_allows()` returns `true` — meaning every
    // unmatched call would auto-allow. That regresses jcode's legacy
    // AUTO_ALLOWED-based classify, which only auto-allows a fixed set of
    // read-only / stateful-safe intents. Until dcg-core Phase 2 wires
    // pack-rule evaluation in, we keep the legacy gate inline for the
    // `Default` and `Auto` modes. The advanced modes (`Plan`,
    // `AcceptEdits`, `DontAsk`, `BypassPermissions`) defer to
    // `Engine::evaluate` because their pre-check semantics are well
    // defined without rule data.
    if matches!(mode, Mode::Default | Mode::Auto) {
        // Legacy auto-allowed tools always allow in both Default and Auto.
        if is_legacy_auto_allowed(&lower) {
            return BridgeDecision::Allow;
        }

        // For Mode::Auto, non-legacy tools go through YOLO classifier.
        if mode == Mode::Auto {
            let (tool, effects) = action_to_tool_call(&lower);
            let effect_strings: Vec<String> = effects
                .iter()
                .map(|e| match e {
                    Effect::Read => "Read".to_string(),
                    Effect::Write => "Write".to_string(),
                    Effect::Spawn => "Spawn".to_string(),
                    Effect::Fs => "Fs".to_string(),
                    Effect::Irreversible => "Irreversible".to_string(),
                    Effect::Network => "Network".to_string(),
                    // CredentialAccess and PrivilegeEscalation are dcg-core Phase B
                    other => format!("{:?}", other),
                })
                .collect();

            let classifier = YoloClassifier::get_or_init();
            return classifier.evaluate(&lower, &format!("{:?}", tool), &effect_strings);
        }

        return BridgeDecision::Prompt;
    }

    let (tool, effects) = action_to_tool_call(&lower);

    // Engine::evaluate takes &mut Session; we serialize on the global
    // mutex. Lock contention is irrelevant: classify is only called from
    // request-permission paths, not on the hot tool-execution path.
    let decision = match SESSION.lock() {
        Ok(mut session) => ENGINE.evaluate(&mut session, &tool, mode, &effects),
        // If the session mutex is poisoned we fall back to "needs prompt"
        // which is the safest choice for jcode's queue/TUI flow.
        Err(_) => return BridgeDecision::Prompt,
    };

    match decision {
        Decision::Allow => BridgeDecision::Allow,
        Decision::Prompt { .. } => BridgeDecision::Prompt,
        Decision::Deny { .. } => BridgeDecision::Deny,
    }
}

/// Centralized list of action names that auto-allowed under jcode's
/// legacy `AUTO_ALLOWED` table. Used by the `Default` / `Auto` mode path.
/// Kept in lockstep with [`action_to_tool_call`] so the two views never
/// drift.
fn is_legacy_auto_allowed(action_lower: &str) -> bool {
    READ_ONLY_ACTIONS.contains(&action_lower) || STATEFUL_SAFE_ACTIONS.contains(&action_lower)
}

/// Read-only intents (used to populate the legacy AUTO_ALLOWED list).
const READ_ONLY_ACTIONS: &[&str] = &[
    "read",
    "glob",
    "grep",
    "ls",
    "codesearch",
    "conversation_search",
    "session_search",
    "todoread",
];

/// Stateful but non-destructive intents — write to jcode-managed scratch
/// state, never to user files.
const STATEFUL_SAFE_ACTIONS: &[&str] = &["memory", "todo", "todowrite"];

/// Map a lowercased jcode action name to a `(ToolCall, Effects)` pair.
///
/// `dcg-core::ToolCall` only has `Bash | Edit | Write | Read | Network`
/// variants, so we approximate jcode's higher-level action vocabulary:
///
/// - **Read-only** intents (`read`, `glob`, `grep`, `ls`, `codesearch`,
///   `*_search`, `todoread`) → `ToolCall::Read` with `[Read, Fs]`.
/// - **Write-stateful** intents (`memory`, `todo`, `todowrite`) →
///   `ToolCall::Write` with `[Write, Fs]`. This deliberately uses
///   `ToolCall::Write` (not `Bash`) so `Mode::AcceptEdits` auto-allows
///   them, matching Claude Code's "edits are auto-OK" semantics.
/// - **Shell-like** intents (`bash`, `shell`, `run_terminal_cmd`,
///   `execute_command`) → `ToolCall::Bash` with `[Spawn, Write,
///   Irreversible]`.
/// - Anything else → `ToolCall::Bash` (conservative) with `[Write,
///   Irreversible]`, mirroring the legacy
///   `ActionTier::RequiresPermission` fall-through.
///
/// The placeholder `PathBuf` for `Read`/`Write` does not influence the
/// Phase-A engine because protected-path checks operate on a known list,
/// not on the call's path. Phase 2 (pack rules) will need a richer
/// classify-with-context entry point.
fn action_to_tool_call(action_lower: &str) -> (ToolCall, Vec<Effect>) {
    use Effect::{Fs, Irreversible, Read, Spawn, Write};

    // Placeholder path: the real path is not known at classify time and
    // Phase-A engine only consults protected_paths against
    // path_in_protected, which we leave conservative-false here.
    let placeholder = PathBuf::from(".");

    if READ_ONLY_ACTIONS.contains(&action_lower) {
        return (ToolCall::read(placeholder), vec![Read, Fs]);
    }

    if STATEFUL_SAFE_ACTIONS.contains(&action_lower) {
        return (ToolCall::write(placeholder), vec![Write, Fs]);
    }

    // Bash / shell-like actions — surface to dcg-core as a real
    // `ToolCall::Bash` so future Phase-2 pack rules can match them.
    if matches!(
        action_lower,
        "bash" | "shell" | "run_terminal_cmd" | "execute_command"
    ) {
        // Empty command string keeps Phase-A evaluation (mode + protected
        // paths) accurate without claiming a specific command — the real
        // command is only known once the agent issues it. Phase 2 will
        // need a richer wiring point.
        return (ToolCall::bash(""), vec![Spawn, Write, Irreversible]);
    }

    // MCP tool actions: mcp__serverName__toolName
    // Three matching levels:
    //   mcp__github          → matches ALL tools from github server
    //   mcp__github__*        → wildcard, same as above
    //   mcp__github__create_pull_request → exact tool
    if action_lower.starts_with("mcp__") {
        let parts: Vec<&str> = action_lower.split("__").collect();
        if parts.len() >= 2 {
            // MCP tools carry Read + Write + Spawn effects since they can
            // read/write data and spawn background processes.
            // Path is unknown at classify time — use placeholder.
            return (ToolCall::read(placeholder), vec![Read, Write, Spawn]);
        }
    }

    // Conservative default for unknown / future tools. We still surface a
    // ToolCall::Bash so the engine treats it as command-shaped rather
    // than file-shaped.
    (ToolCall::bash(action_lower), vec![Write, Irreversible])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// In `Default` mode, the legacy AUTO_ALLOWED tools must still
    /// auto-allow so existing jcode workflows are not regressed.
    #[test]
    fn default_mode_auto_allows_legacy_read_tools() {
        for action in [
            "read",
            "glob",
            "grep",
            "ls",
            "memory",
            "todo",
            "todowrite",
            "todoread",
            "conversation_search",
            "session_search",
            "codesearch",
        ] {
            assert_eq!(
                classify_with_mode(action, Mode::Default),
                BridgeDecision::Allow,
                "{action} must auto-allow in Default mode"
            );
        }
    }

    /// Under `Plan` mode, read-only actions still allow but write-shaped
    /// stateful tools must deny — that's the whole point of plan mode.
    #[test]
    fn plan_mode_denies_write_shaped_tools() {
        assert_eq!(
            classify_with_mode("read", Mode::Plan),
            BridgeDecision::Allow,
            "read must allow in Plan"
        );
        assert_eq!(
            classify_with_mode("todowrite", Mode::Plan),
            BridgeDecision::Deny,
            "todowrite must deny in Plan"
        );
        assert_eq!(
            classify_with_mode("memory", Mode::Plan),
            BridgeDecision::Deny,
            "memory writes must deny in Plan"
        );
    }

    /// `BypassPermissions` is the escape hatch: every action allows.
    #[test]
    fn bypass_mode_allows_everything() {
        for action in ["read", "todowrite", "shell", "made_up_tool"] {
            assert_eq!(
                classify_with_mode(action, Mode::BypassPermissions),
                BridgeDecision::Allow,
                "{action} must allow in Bypass"
            );
        }
    }

    /// Unknown actions in `Default` mode must Prompt, matching the legacy
    /// `AUTO_ALLOWED`-based behavior where anything not in the safe list
    /// surfaced as `RequiresPermission`.
    #[test]
    fn default_mode_prompts_for_unknown_actions() {
        for action in [
            "bash",
            "edit",
            "write",
            "create_pull_request",
            "send_email",
            "future_destructive_tool",
        ] {
            assert_eq!(
                classify_with_mode(action, Mode::Default),
                BridgeDecision::Prompt,
                "{action} must require permission in Default mode"
            );
        }
    }

    /// Case-insensitivity matches the legacy `to_lowercase()` behavior.
    #[test]
    fn classify_is_case_insensitive() {
        assert_eq!(
            classify_with_mode("READ", Mode::Default),
            BridgeDecision::Allow
        );
        assert_eq!(
            classify_with_mode("Bash", Mode::Default),
            BridgeDecision::Prompt
        );
    }

    #[test]
    fn set_and_read_back_mode() {
        let original = current_mode();
        set_mode(Mode::Plan);
        assert_eq!(current_mode(), Mode::Plan);
        // Restore so other tests aren't affected by ordering.
        set_mode(original);
    }
}
