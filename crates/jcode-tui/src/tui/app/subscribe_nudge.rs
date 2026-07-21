//! Subscription nudge: two tasteful, rate-limited prompts to subscribe.
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
//! `/subscribe` renders the full pitch and points at `/login jcode`.

use super::{App, AppRuntimeMode, DisplayMessage};
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// Minimum gap between two nudges, across sessions.
const NUDGE_INTERVAL: Duration = Duration::from_secs(7 * 24 * 60 * 60);
/// A todo group must have been in flight at least this long to count as a
/// "long task" for the goodwill trigger.
const LONG_TASK_MIN_ELAPSED: Duration = Duration::from_secs(60 * 60);

/// Copy appended to the rate-limit system message (trigger A).
pub(super) const RATE_LIMIT_NUDGE_LINE: &str =
    "Get more tokens with a jcode subscription: /subscribe";
/// Status-line tail shared by trigger B.
const SUPPORT_NUDGE_NOTICE: &str = "Support jcode: /subscribe";

/// Why a nudge fired. Used to build the message and recorded in the state file
/// so we can tune trigger mix later.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SubscribeNudgeTrigger {
    /// The provider rate-limited the user mid-turn.
    RateLimited,
    /// A 1h+ todo group completed with all items passing the quality gate.
    LongTaskCompleted,
}

impl SubscribeNudgeTrigger {
    fn as_str(self) -> &'static str {
        match self {
            SubscribeNudgeTrigger::RateLimited => "rate_limited",
            SubscribeNudgeTrigger::LongTaskCompleted => "long_task_completed",
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
            && todo
                .completion_confidence
                .or(todo.confidence)
                .unwrap_or(0)
                >= crate::todo::QUALITY_GATE_THRESHOLD
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

/// The full `/subscribe` pitch. Reuses the live curated catalog so the plans
/// and models never drift from `/subscription`.
pub(super) fn subscribe_pitch_markdown() -> String {
    let mut message = String::from("Subscribe to jcode\n\n");
    message.push_str("One subscription, more tokens, zero API keys:\n\n");
    message.push_str(
        "  - Get more tokens: a monthly inference budget on curated frontier models\n",
    );
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
    message.push_str("  - No key management: sign in once in the browser, jcode routes the rest\n");
    message.push_str("  - Automatic failover routing when a provider has a bad day\n");
    message.push_str("  - Funds jcode development - jcode is open source\n");

    message.push_str("\nPlans\n\n");
    for tier in crate::subscription_catalog::JcodeTier::ALL.iter().copied() {
        message.push_str(&format!(
            "  - {} - ${}/mo, about ${:.2} usable inference budget\n",
            tier.display_name(),
            tier.retail_price_usd(),
            tier.usable_budget_usd()
        ));
    }

    message.push_str("\nStart: /login jcode (browser approval, no keys in the terminal)\n");
    message.push_str("Details anytime: /subscription");
    message
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
        if self.subscribe_nudge_shown_this_session {
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
        self.subscribe_nudge_shown_this_session = true;
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
            || self.subscribe_nudge_shown_this_session
        {
            return;
        }
        let todos = crate::todo::load_todos(session_id).unwrap_or_default();
        if todos.is_empty() {
            return;
        }
        if todos.iter().any(|todo| todo.status != "completed") {
            // Work in flight: arm (or keep) the long-task timer.
            self.subscribe_nudge_todo_started
                .get_or_insert_with(Instant::now);
            return;
        }
        // Completion edge: evaluate exactly once per armed run. Taking the
        // timer here means a short batch can never leak its start time into a
        // later batch and inflate that batch's apparent duration.
        let Some(started) = self.subscribe_nudge_todo_started.take() else {
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

    /// Render the `/subscribe` pitch into the transcript.
    pub(super) fn show_subscribe_pitch(&mut self) {
        self.push_display_message(DisplayMessage::system(subscribe_pitch_markdown()));
        self.set_status_notice("Subscribe: /login jcode to start");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn todo(status: &str, completion_confidence: Option<u8>) -> crate::todo::TodoItem {
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
        assert!(weekly_gate_allows(0, week));
        assert!(weekly_gate_allows(1_000, 1_000 + week));
        assert!(!weekly_gate_allows(1_000, 1_000 + week - 1));
        // Fresh state (never shown) allows immediately.
        assert!(weekly_gate_allows(0, 0 + week));
        assert!(weekly_gate_allows(0, u64::MAX));
    }

    #[test]
    fn long_task_qualification_requires_all_completed_and_gated_confidence() {
        let gate = crate::todo::QUALITY_GATE_THRESHOLD;
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
            Some(gate.saturating_sub(1))
        )]));
        // Missing confidence disqualifies.
        assert!(!long_task_todos_qualify(&[todo("completed", None)]));
        // All completed at/above the gate qualifies.
        assert!(long_task_todos_qualify(&[
            todo("completed", Some(gate)),
            todo("completed", Some(100)),
        ]));
    }

    #[test]
    fn long_task_message_reads_support_wording_with_elapsed() {
        let message = long_task_message(Duration::from_secs(60 * 83));
        assert_eq!(
            message,
            "✦ jcode just worked 1h 23m for you. Support jcode: /subscribe"
        );
        assert!(long_task_message(Duration::from_secs(3600)).contains("worked 1h for you"));
        assert!(long_task_message(Duration::from_secs(59 * 60)).contains("worked 59m for you"));
    }

    #[test]
    fn rate_limit_copy_leads_with_the_token_value_prop() {
        assert!(RATE_LIMIT_NUDGE_LINE.starts_with("Get more tokens"));
        assert!(RATE_LIMIT_NUDGE_LINE.contains("/subscribe"));
    }

    #[test]
    fn pitch_lists_reasons_plans_and_next_step() {
        let pitch = subscribe_pitch_markdown();
        assert!(pitch.contains("Get more tokens"));
        assert!(pitch.contains("open source"));
        assert!(pitch.contains("/login jcode"));
        assert!(pitch.contains("/subscription"));
        // Every launched tier appears with its retail price.
        for tier in crate::subscription_catalog::JcodeTier::ALL.iter().copied() {
            assert!(pitch.contains(tier.display_name()));
            assert!(pitch.contains(&format!("${}/mo", tier.retail_price_usd())));
        }
    }

    #[test]
    fn elapsed_formatting_covers_minute_and_hour_shapes() {
        assert_eq!(format_elapsed(Duration::from_secs(60 * 61)), "1h 1m");
        assert_eq!(format_elapsed(Duration::from_secs(7200)), "2h");
        assert_eq!(format_elapsed(Duration::from_secs(60 * 45)), "45m");
    }
}
