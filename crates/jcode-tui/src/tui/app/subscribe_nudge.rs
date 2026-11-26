//! Hosted-model nudge: two tasteful, rate-limited prompts to try metered access.
//!
//! Surfaces (never a blocking screen):
//!   - Trigger A (value prop): the provider just rate-limited the user. The
//!     rate-limit system line gains a one-line "get more tokens" suffix.
//!   - Trigger B (goodwill): a todo group the session worked on for 1+ hour
//!     just fully completed with every item passing the todo quality gate.
//!
//! Delivery rules:
//!   - At most once per week across all sessions (persisted timestamp), and
//!     at most once per session.
//!   - Never while onboarding is active, never for users who already hold
//!     jcode account credentials, never in replay/test runtimes.
//!
//! `/subscribe` remains a compatibility alias for the hosted-model pitch.

use super::{App, AppRuntimeMode, DisplayMessage};
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// Minimum gap between two nudges, across sessions.
const NUDGE_INTERVAL: Duration = Duration::from_secs(7 * 24 * 60 * 60);
/// A todo group must have been in flight at least this long to count as a
/// "long task" for the goodwill trigger.
const LONG_TASK_MIN_ELAPSED: Duration = Duration::from_secs(60 * 60);

/// Minimum duration (in seconds) for a `cargo` build/test/check command to
/// count as a slow Rust build for the justrust nudge.
const SLOW_CARGO_BUILD_MIN_SECS: f32 = 45.0;

/// Copy appended to the rate-limit system message (trigger A).
pub(super) const RATE_LIMIT_NUDGE_LINE: &str =
    "Avoid provider rate limits with Jcode hosted models: /hosted";
/// Status-line notice for rate-limit / token exhaustion.
pub(super) const RATE_LIMIT_NUDGE_NOTICE: &str =
    "Hit rate limit? Keep going with Jcode Inference: /subscribe";
/// Status-line notice for turn interrupted by sleep or disconnect.
pub(super) const CLOUD_DISCONNECT_NUDGE_NOTICE: &str =
    "Turn interrupted? Keep sessions running in Jcode Cloud: /cloud";
/// Status-line notice for slow or OOM Rust compiles.
pub(super) const SLOW_RUST_BUILD_NUDGE_NOTICE: &str =
    "Slow Rust build? Offload builds with justrust in Jcode Sub: /subscribe";
/// Status-line tail shared by trigger B.
const SUPPORT_NUDGE_NOTICE: &str = "Try Jcode hosted models: /hosted";

/// Why a nudge fired. Used to build the message and recorded in the state file
/// so we can tune trigger mix later.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SubscribeNudgeTrigger {
    /// The provider rate-limited the user mid-turn.
    RateLimited,
    /// A 1h+ todo group completed with all items passing the quality gate.
    LongTaskCompleted,
    /// An in-flight turn was interrupted by a network drop or laptop sleep.
    TurnInterrupted,
    /// A local `cargo` build/test/check took a long time or ran out of memory.
    SlowRustBuild,
}

impl SubscribeNudgeTrigger {
    fn as_str(self) -> &'static str {
        match self {
            SubscribeNudgeTrigger::RateLimited => "rate_limited",
            SubscribeNudgeTrigger::LongTaskCompleted => "long_task_completed",
            SubscribeNudgeTrigger::TurnInterrupted => "turn_interrupted",
            SubscribeNudgeTrigger::SlowRustBuild => "slow_rust_build",
        }
    }
}

/// Persisted nudge bookkeeping (one small JSON file under the jcode dir).
#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
struct NudgeState {
    /// Unix seconds when a nudge was last shown, across all sessions.
    #[serde(default)]
    last_shown_unix: u64,
    /// Which trigger produced the last nudge (diagnostic only).
    #[serde(default)]
    last_trigger: String,
}

fn state_path() -> Option<PathBuf> {
    crate::storage::jcode_dir()
        .ok()
        .map(|dir| dir.join("subscribe-nudge.json"))
}

fn load_state() -> NudgeState {
    let Some(path) = state_path() else {
        return NudgeState::default();
    };
    std::fs::read_to_string(path)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

fn store_state(state: &NudgeState) {
    let Some(path) = state_path() else {
        return;
    };
    if let Ok(raw) = serde_json::to_string_pretty(state) {
        let _ = std::fs::write(path, raw);
    }
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|dur| dur.as_secs())
        .unwrap_or(0)
}

/// Whether the persisted weekly gate allows another nudge at `now_unix`.
fn weekly_gate_allows(last_shown_unix: u64, now_unix: u64) -> bool {
    now_unix.saturating_sub(last_shown_unix) >= NUDGE_INTERVAL.as_secs()
}

/// Whether a fully-loaded todo list qualifies for the long-task trigger:
/// non-empty, every item completed, and every item's final confidence at or
/// above the todo quality-gate threshold ("passed all quality gates").
fn long_task_todos_qualify(todos: &[crate::todo::TodoItem]) -> bool {
    if todos.is_empty() {
        return false;
    }
    todos.iter().all(|todo| {
        todo.status == "completed"
            && crate::todo::completion_confidence_passes(
                todo.completion_confidence.or(todo.confidence),
            )
    })
}

/// "1h 23m" style rendering for the goodwill message.
fn format_elapsed(elapsed: Duration) -> String {
    let total_minutes = elapsed.as_secs() / 60;
    let hours = total_minutes / 60;
    let minutes = total_minutes % 60;
    if hours == 0 {
        format!("{minutes}m")
    } else if minutes == 0 {
        format!("{hours}h")
    } else {
        format!("{hours}h {minutes}m")
    }
}

/// The goodwill message for a completed long task.
fn long_task_message(elapsed: Duration) -> String {
    format!(
        "✦ jcode just worked {} for you. {}",
        format_elapsed(elapsed),
        SUPPORT_NUDGE_NOTICE
    )
}

/// The full hosted-model pitch. Reuses the live curated catalog so models
/// never drift from the account status surface.
pub(super) fn subscribe_pitch_markdown() -> String {
    let mut message = String::from("Jcode hosted models\n\n");
    message.push_str("No subscription. Set a monthly spending limit and pay only for usage:\n\n");
    message.push_str("  - You control the monthly limit and can change it from your account\n");
    message.push_str("  - Usage milestones send warnings without slowing your requests\n");
    let model_names: Vec<&str> = crate::subscription_catalog::curated_models()
        .iter()
        .map(|model| model.display_name)
        .collect();
    if !model_names.is_empty() {
        message.push_str(&format!(
            "  - Curated catalog: {}\n",
            model_names.join(", ")
        ));
    }
    message.push_str("  - Sign in once: Jcode saves the hosted API key and routes the rest\n");
    message.push_str("  - Automatic failover routing when a provider has a bad day\n");
    message
        .push_str("  - Billing starts at $20 of usage, then uses progressively larger tranches\n");
    message.push_str("  - Any remaining usage is billed at your limit or at month end\n");
    message.push_str("  - Funds Jcode development while the software stays open source\n");

    message.push_str("\nStart: /login jcode (browser approval, no key pasted into the terminal)\n");
    message.push_str("Usage anytime: /usage");
    message
}

/// Contextual pitch tailored to the pain point the user just hit, with clear
/// subscribe-and-use steps. Falls back to [`subscribe_pitch_markdown`] when no
/// contextual trigger fired in this session.
pub(super) fn subscribe_pitch_for_trigger(trigger: Option<SubscribeNudgeTrigger>) -> String {
    let pricing = crate::subscription_catalog::JCODE_PRICING_URL;
    let lead = match trigger {
        Some(SubscribeNudgeTrigger::RateLimited) => Some(format!(
            "Jcode Inference (keep going past provider rate limits)\n\n\
             1. Sign in via the browser window we open (or run `/login jcode`) to link your account automatically.\n\
             2. Choose a plan at {pricing}.\n\
             3. Run `/model` to switch to Jcode hosted models (or let failover route automatically).\n\n---\n\n"
        )),
        Some(SubscribeNudgeTrigger::TurnInterrupted) => Some(format!(
            "Jcode Cloud (keep sessions running through sleep & disconnects)\n\n\
             1. Sign in via the browser window we open (or run `/login jcode`) to link your account automatically.\n\
             2. Choose a plan at {pricing}.\n\
             3. Run `/cloud` to move this session to your managed cloud VM.\n\n---\n\n"
        )),
        Some(SubscribeNudgeTrigger::SlowRustBuild) => Some(format!(
            "justrust Remote Builds (included with Jcode Subscription)\n\n\
             1. Sign in via the browser window we open (or run `/login jcode`) to link your account automatically.\n\
             2. Choose a plan at {pricing}.\n\
             3. Run `justrust check`, `justrust test`, or `justrust build` to offload Rust compiles automatically.\n\n---\n\n"
        )),
        Some(SubscribeNudgeTrigger::LongTaskCompleted) | None => None,
    };
    match lead {
        Some(prefix) => format!("{prefix}{}", subscribe_pitch_markdown()),
        None => subscribe_pitch_markdown(),
    }
}

/// Whether a completed tool call was a slow or OOM local `cargo` build/test/check
/// (and not already using `justrust`).
pub(super) fn is_slow_or_oom_cargo_build(
    tool_call: &crate::message::ToolCall,
    duration_secs: Option<f32>,
    output: &str,
    is_error: bool,
) -> bool {
    let name = tool_call.name.to_ascii_lowercase();
    if !matches!(name.as_str(), "bash" | "shell" | "shell_exec" | "run_shell") {
        return false;
    }
    let Some(command) = tool_call.input.get("command").and_then(|v| v.as_str()) else {
        return false;
    };
    let lower_cmd = command.to_ascii_lowercase();
    if lower_cmd.contains("justrust") {
        return false;
    }
    let is_cargo_compile = [
        "cargo build",
        "cargo test",
        "cargo check",
        "cargo clippy",
        "cargo run",
    ]
    .iter()
    .any(|needle| lower_cmd.contains(needle));
    if !is_cargo_compile {
        return false;
    }
    if duration_secs.is_some_and(|secs| secs >= SLOW_CARGO_BUILD_MIN_SECS) {
        return true;
    }
    if is_error {
        let lower_out = output.to_ascii_lowercase();
        if lower_out.contains("signal: 9")
            || lower_out.contains("sigkill")
            || lower_out.contains("out of memory")
            || lower_out.contains("cannot allocate memory")
        {
            return true;
        }
    }
    false
}

/// Per-session state for the subscribe nudge.
#[derive(Default)]
pub(super) struct SubscribeNudgeState {
    /// Whether the nudge already fired this session.
    shown_this_session: bool,
    /// Last trigger that fired in this session, used to tailor `/subscribe`.
    last_trigger: Option<SubscribeNudgeTrigger>,
    /// When this session's todo list first showed incomplete work, for the
    /// 1h+ long-task nudge.
    todo_started: Option<std::time::Instant>,
}

impl App {
    /// Central gate for both triggers. Returns true when the nudge may be
    /// shown, and records the claim (session flag + weekly file) so a `true`
    /// must be followed by actually showing it.
    pub(super) fn claim_subscribe_nudge(&mut self, trigger: SubscribeNudgeTrigger) -> bool {
        // Deterministic playback and unit-test harnesses must not grow
        // marketing lines (golden transcripts assert exact output).
        if cfg!(test) || !matches!(self.runtime_mode, AppRuntimeMode::RemoteClient) {
            return false;
        }
        if self.subscribe_nudge.shown_this_session {
            return false;
        }
        // Never pitch during onboarding: the user has gotten zero value yet.
        if self.onboarding_flow_active() {
            return false;
        }
        // Never pitch existing jcode account holders.
        if crate::subscription_catalog::has_credentials() {
            return false;
        }
        let now = now_unix();
        let mut state = load_state();
        if !weekly_gate_allows(state.last_shown_unix, now) {
            return false;
        }
        state.last_shown_unix = now;
        state.last_trigger = trigger.as_str().to_string();
        store_state(&state);
        self.subscribe_nudge.shown_this_session = true;
        self.subscribe_nudge.last_trigger = Some(trigger);
        true
    }

    /// Trigger B driver, called whenever this session's todo list changes
    /// (todo tool completed locally or remotely). Arms a timer when incomplete
    /// todos first appear; fires the goodwill nudge when the list later
    /// reaches all-completed with quality gates passed after 1+ hour.
    pub(super) fn note_todo_update_for_subscribe_nudge(&mut self, session_id: &str) {
        // Cheap early-outs before touching disk: replay/test runtimes never
        // nudge, and a session nudges at most once.
        if cfg!(test)
            || !matches!(self.runtime_mode, AppRuntimeMode::RemoteClient)
            || self.subscribe_nudge.shown_this_session
        {
            return;
        }
        let todos = crate::todo::load_todos(session_id).unwrap_or_default();
        if todos.is_empty() {
            return;
        }
        if todos.iter().any(|todo| todo.status != "completed") {
            // Work in flight: arm (or keep) the long-task timer.
            self.subscribe_nudge
                .todo_started
                .get_or_insert_with(Instant::now);
            return;
        }
        // Completion edge: evaluate exactly once per armed run. Taking the
        // timer here means a short batch can never leak its start time into a
        // later batch and inflate that batch's apparent duration.
        let Some(started) = self.subscribe_nudge.todo_started.take() else {
            return;
        };
        let elapsed = started.elapsed();
        if elapsed < LONG_TASK_MIN_ELAPSED || !long_task_todos_qualify(&todos) {
            return;
        }
        if !self.claim_subscribe_nudge(SubscribeNudgeTrigger::LongTaskCompleted) {
            return;
        }
        self.push_display_message(DisplayMessage::system(long_task_message(elapsed)));
        self.set_status_notice(SUPPORT_NUDGE_NOTICE);
    }

    /// Render the `/subscribe` pitch into the transcript and immediately kick
    /// off browser sign-in / account linking (or open pricing if already signed in)
    /// so the user can subscribe and start using the feature in one step.
    pub(super) fn show_subscribe_pitch(&mut self) {
        let trigger = self.subscribe_nudge.last_trigger;
        self.push_display_message(DisplayMessage::system(subscribe_pitch_for_trigger(trigger)));
        if cfg!(test) {
            self.set_status_notice("Hosted models: /login jcode to start");
            return;
        }
        if crate::subscription_catalog::has_credentials() {
            let opened = App::open_auth_browser(crate::subscription_catalog::JCODE_PRICING_URL);
            if opened {
                self.set_status_notice("Opened jcode.sh/pricing in your browser");
            } else {
                self.set_status_notice("Manage plan: /account jcode manage");
            }
        } else {
            self.start_jcode_login();
        }
    }

    /// Trigger for slow or OOM local `cargo` builds: suggests `justrust` in the
    /// status line when the weekly/session gate allows.
    pub(super) fn note_tool_completed_for_subscribe_nudge(
        &mut self,
        tool_call: &crate::message::ToolCall,
        duration_secs: Option<f32>,
        output: &str,
        is_error: bool,
    ) {
        if !is_slow_or_oom_cargo_build(tool_call, duration_secs, output, is_error) {
            return;
        }
        if self.claim_subscribe_nudge(SubscribeNudgeTrigger::SlowRustBuild) {
            self.set_status_notice(SLOW_RUST_BUILD_NUDGE_NOTICE);
        }
    }

    /// Trigger for an in-flight turn interrupted by disconnect or laptop sleep:
    /// suggests Jcode Cloud in the status line when not already on a remote host.
    pub(super) fn note_turn_interrupted_for_subscribe_nudge(&mut self) {
        if crate::tui::is_ssh_remote() {
            return;
        }
        if self.claim_subscribe_nudge(SubscribeNudgeTrigger::TurnInterrupted) {
            self.set_status_notice(CLOUD_DISCONNECT_NUDGE_NOTICE);
        }
    }
}

impl App {
    /// Rate-limit notice line, with the weekly-gated subscribe nudge appended
    /// when the gate allows (user is blocked on tokens right now).
    /// Returns `(message, nudged)` so the caller can preserve the status notice.
    pub(super) fn rate_limit_notice_with_nudge(&mut self, reset_secs: u64) -> (String, bool) {
        let mut line = format!(
            "⏳ Rate limit hit. Will auto-retry in {} seconds...",
            reset_secs
        );
        let nudged = self.claim_subscribe_nudge(SubscribeNudgeTrigger::RateLimited);
        if nudged {
            line.push_str(&format!("\n{}", RATE_LIMIT_NUDGE_LINE));
            self.set_status_notice(RATE_LIMIT_NUDGE_NOTICE);
        }
        (line, nudged)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn todo(
        status: &str,
        completion_confidence: Option<crate::todo::ConfidenceState>,
    ) -> crate::todo::TodoItem {
        crate::todo::TodoItem {
            content: "task".to_string(),
            status: status.to_string(),
            priority: "high".to_string(),
            id: "t1".to_string(),
            completion_confidence,
            ..Default::default()
        }
    }

    #[test]
    fn weekly_gate_blocks_within_a_week_and_allows_after() {
        let week = NUDGE_INTERVAL.as_secs();
        // Fresh state (never shown, last_shown 0) allows immediately.
        assert!(weekly_gate_allows(0, week));
        assert!(weekly_gate_allows(0, u64::MAX));
        assert!(weekly_gate_allows(1_000, 1_000 + week));
        assert!(!weekly_gate_allows(1_000, 1_000 + week - 1));
        // `now` before `last_shown` must saturate rather than wrap into
        // "a week has passed" (clock skew / restored backup).
        assert!(!weekly_gate_allows(1_000, 0));
    }

    #[test]
    fn long_task_qualification_requires_all_completed_and_gated_confidence() {
        let gate = crate::todo::ConfidenceState::Validated;
        // Empty list never qualifies.
        assert!(!long_task_todos_qualify(&[]));
        // Incomplete item disqualifies.
        assert!(!long_task_todos_qualify(&[
            todo("completed", Some(gate)),
            todo("in_progress", Some(gate)),
        ]));
        // Low completion confidence disqualifies ("quality gate failed").
        assert!(!long_task_todos_qualify(&[todo(
            "completed",
            Some(crate::todo::ConfidenceState::Plausible)
        )]));
        // Missing confidence disqualifies.
        assert!(!long_task_todos_qualify(&[todo("completed", None)]));
        // All completed at/above the gate qualifies.
        assert!(long_task_todos_qualify(&[
            todo("completed", Some(gate)),
            todo("completed", Some(crate::todo::ConfidenceState::Verified)),
        ]));
    }

    #[test]
    fn long_task_message_reads_support_wording_with_elapsed() {
        let message = long_task_message(Duration::from_secs(60 * 83));
        assert_eq!(
            message,
            "✦ jcode just worked 1h 23m for you. Try Jcode hosted models: /hosted"
        );
        assert!(long_task_message(Duration::from_secs(3600)).contains("worked 1h for you"));
        assert!(long_task_message(Duration::from_secs(59 * 60)).contains("worked 59m for you"));
    }

    #[test]
    fn rate_limit_copy_leads_with_the_hosted_model_value_prop() {
        assert!(RATE_LIMIT_NUDGE_LINE.starts_with("Avoid provider rate limits"));
        assert!(RATE_LIMIT_NUDGE_LINE.contains("/hosted"));
    }

    #[test]
    fn pitch_explains_limits_warnings_tranches_and_next_step() {
        let pitch = subscribe_pitch_markdown();
        assert!(pitch.contains("No subscription"));
        assert!(pitch.contains("monthly spending limit"));
        assert!(pitch.contains("warnings without slowing"));
        assert!(pitch.contains("$20"));
        assert!(pitch.contains("progressively larger tranches"));
        assert!(pitch.contains("month end"));
        assert!(pitch.contains("open source"));
        assert!(pitch.contains("/login jcode"));
        assert!(pitch.contains("/usage"));
        for model in crate::subscription_catalog::curated_models() {
            assert!(pitch.contains(model.display_name));
        }
    }

    #[test]
    fn elapsed_formatting_covers_minute_and_hour_shapes() {
        assert_eq!(format_elapsed(Duration::from_secs(60 * 61)), "1h 1m");
        assert_eq!(format_elapsed(Duration::from_secs(7200)), "2h");
        assert_eq!(format_elapsed(Duration::from_secs(60 * 45)), "45m");
    }

    #[test]
    fn slow_or_oom_cargo_detection_matches_only_local_cargo_compiles() {
        let bash_call = |cmd: &str| crate::message::ToolCall {
            id: "1".to_string(),
            name: "bash".to_string(),
            input: serde_json::json!({ "command": cmd }),
            intent: None,
            thought_signature: None,
        };
        assert!(is_slow_or_oom_cargo_build(
            &bash_call("cargo build --release"),
            Some(50.0),
            "",
            false
        ));
        assert!(!is_slow_or_oom_cargo_build(
            &bash_call("cargo build"),
            Some(12.0),
            "",
            false
        ));
        assert!(is_slow_or_oom_cargo_build(
            &bash_call("cargo test -p jcode"),
            Some(10.0),
            "error: process didn't exit successfully (signal: 9, SIGKILL: kill)",
            true
        ));
        assert!(!is_slow_or_oom_cargo_build(
            &bash_call("justrust test -p jcode"),
            Some(90.0),
            "",
            false
        ));
        assert!(!is_slow_or_oom_cargo_build(
            &bash_call("ls -la"),
            Some(60.0),
            "",
            false
        ));
    }

    #[test]
    fn contextual_subscribe_pitch_explains_subscribe_and_use_steps() {
        let pricing = crate::subscription_catalog::JCODE_PRICING_URL;

        let inference = subscribe_pitch_for_trigger(Some(SubscribeNudgeTrigger::RateLimited));
        assert!(inference.contains("Jcode Inference"));
        assert!(inference.contains(pricing));
        assert!(inference.contains("/model"));

        let cloud = subscribe_pitch_for_trigger(Some(SubscribeNudgeTrigger::TurnInterrupted));
        assert!(cloud.contains("Jcode Cloud"));
        assert!(cloud.contains(pricing));
        assert!(cloud.contains("/cloud"));

        let justrust = subscribe_pitch_for_trigger(Some(SubscribeNudgeTrigger::SlowRustBuild));
        assert!(justrust.contains("justrust"));
        assert!(justrust.contains(pricing));
        assert!(justrust.contains("justrust check"));

        assert_eq!(
            subscribe_pitch_for_trigger(None),
            subscribe_pitch_markdown()
        );
    }

    #[test]
    fn contextual_status_notices_are_single_line_and_compact() {
        for notice in [
            RATE_LIMIT_NUDGE_NOTICE,
            CLOUD_DISCONNECT_NUDGE_NOTICE,
            SLOW_RUST_BUILD_NUDGE_NOTICE,
        ] {
            assert!(
                !notice.contains('\n'),
                "notice must be single-line: {notice}"
            );
            assert!(
                notice.len() <= 75,
                "notice too wide ({} chars): {notice}",
                notice.len()
            );
        }
    }
}
