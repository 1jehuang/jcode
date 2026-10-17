//! Learned-keybinding nudges.
//!
//! jcode tracks, per action, how often the user reaches a result via its
//! configured keyboard shortcut (the *fast* path) versus the slow way (typing a
//! slash command). From those two counters we infer which keybindings the user
//! has *learned* and which they keep paying a "tax" on, and we occasionally
//! surface a single, distinctly-colored nudge teaching the highest-tax binding.
//!
//! Design goals:
//! - **Two signals.** [`record_fast`] is called from the key dispatcher when a
//!   binding matches; [`record_slow`] is called from the slash-command / menu
//!   handlers. A binding with `fast_uses >= LEARNED_FAST_THRESHOLD` is treated
//!   as *learned* and never nudged again.
//! - **Prioritize by tax.** We never nudge after every long-path use. The pure
//!   [`pick_action_to_hint`] selects the unlearned, bound action with the most
//!   slow-path uses, subject to a per-action show cap and cooldown.
//! - **Don't nag.** At most one nudge per session, gated behind the
//!   `display.keybinding_hints` config flag, shown in its own color so the user
//!   understands the system noticed they aren't using a shortcut.
//!
//! The decision logic is pure and unit-tested; only persistence and label
//! resolution touch the environment.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use super::App;

/// Used the keybinding this many times => considered learned; stop nudging.
const LEARNED_FAST_THRESHOLD: u32 = 3;
/// Require at least this many slow-path uses before the first nudge.
const MIN_SLOW_BEFORE_HINT: u32 = 2;
/// Never nudge a single action more than this many times total.
const MAX_HINTS_PER_ACTION: u32 = 3;
/// Minimum wall-clock gap between two nudges for the *same* action.
const HINT_COOLDOWN_SECS: u64 = 6 * 60 * 60;

const STATE_FILE: &str = "keybinding_proficiency.json";

/// An action that can be performed both via a configured shortcut and the slow
/// way (slash command). Each variant has a stable id used as the persistence
/// key, a phrase describing the slow action, and a label loader for the binding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum LearnableAction {
    Resume,
    ModelSwitch,
    EffortCycle,
    Alignment,
}

impl LearnableAction {
    /// Every action the system knows about, in a stable order.
    pub(crate) const ALL: [LearnableAction; 4] = [
        LearnableAction::Resume,
        LearnableAction::ModelSwitch,
        LearnableAction::EffortCycle,
        LearnableAction::Alignment,
    ];

    /// Stable persistence id. Never change these once shipped.
    pub(crate) fn id(self) -> &'static str {
        match self {
            LearnableAction::Resume => "resume",
            LearnableAction::ModelSwitch => "model_switch",
            LearnableAction::EffortCycle => "effort_cycle",
            LearnableAction::Alignment => "alignment",
        }
    }

    fn from_id(id: &str) -> Option<LearnableAction> {
        LearnableAction::ALL.into_iter().find(|a| a.id() == id)
    }

    /// Short phrase describing what the user just did the slow way, e.g.
    /// `"open the session picker"`. Rendered as `press {key} to {phrase}`.
    fn phrase(self) -> &'static str {
        match self {
            LearnableAction::Resume => "open the session picker",
            LearnableAction::ModelSwitch => "switch models",
            LearnableAction::EffortCycle => "change reasoning effort",
            LearnableAction::Alignment => "toggle centered layout",
        }
    }

    /// Resolve the display label of the configured shortcut, or `None` when the
    /// binding is disabled/unbound (in which case the action is never nudged).
    fn shortcut_label(self) -> Option<String> {
        use crate::tui::keybind;
        match self {
            LearnableAction::Resume => keybind::load_open_resume_key().label,
            LearnableAction::ModelSwitch => keybind::model_switch_next_label(),
            LearnableAction::EffortCycle => keybind::effort_increase_label(),
            LearnableAction::Alignment => keybind::centered_toggle_label(),
        }
    }
}

/// Per-action proficiency counters.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct ActionStat {
    /// Times the user used the configured shortcut directly.
    #[serde(default)]
    fast_uses: u32,
    /// Times the user reached the same result the slow way.
    #[serde(default)]
    slow_uses: u32,
    /// Times we have surfaced a learn-hint for this action.
    #[serde(default)]
    hints_shown: u32,
    /// Unix seconds of the last hint, for cooldown throttling.
    #[serde(default)]
    last_hint_unix: u64,
}

impl ActionStat {
    fn is_learned(&self) -> bool {
        self.fast_uses >= LEARNED_FAST_THRESHOLD
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct ProficiencyState {
    #[serde(default)]
    version: u8,
    #[serde(default)]
    actions: HashMap<String, ActionStat>,
}

fn state_path() -> Option<std::path::PathBuf> {
    crate::storage::app_config_dir()
        .ok()
        .map(|dir| dir.join(STATE_FILE))
}

fn load_state() -> ProficiencyState {
    let Some(path) = state_path() else {
        return ProficiencyState::default();
    };
    crate::storage::read_json::<ProficiencyState>(&path).unwrap_or_default()
}

fn save_state(state: &ProficiencyState) {
    let Some(path) = state_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Err(error) = crate::storage::write_json(&path, state) {
        crate::logging::info(&format!(
            "Failed to persist keybinding-proficiency state {}: {}",
            path.display(),
            error
        ));
    }
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Record that the user performed `action` via its keyboard shortcut.
pub(crate) fn record_fast(action: LearnableAction) {
    let mut state = load_state();
    let stat = state.actions.entry(action.id().to_string()).or_default();
    stat.fast_uses = stat.fast_uses.saturating_add(1);
    save_state(&state);
}

/// Record that the user performed `action` the slow way (slash command/menu).
pub(crate) fn record_slow(action: LearnableAction) {
    let mut state = load_state();
    let stat = state.actions.entry(action.id().to_string()).or_default();
    stat.slow_uses = stat.slow_uses.saturating_add(1);
    save_state(&state);
}

/// Pure ranked pick: among `candidates` (action id + stat + whether currently
/// bound), choose the best action to nudge, or `None` if none qualify.
///
/// A candidate qualifies when it is bound, not yet learned, has been used the
/// slow way enough times, still has hint budget, and is past its cooldown. The
/// winner is the one with the highest slow-path tax (ties broken by id for
/// determinism).
fn pick_action_id<'a>(
    candidates: &[(&'a str, &ActionStat, bool)],
    now_unix: u64,
) -> Option<&'a str> {
    candidates
        .iter()
        .filter(|(_, stat, bound)| {
            *bound
                && !stat.is_learned()
                && stat.slow_uses >= MIN_SLOW_BEFORE_HINT
                && stat.hints_shown < MAX_HINTS_PER_ACTION
                && now_unix.saturating_sub(stat.last_hint_unix) >= HINT_COOLDOWN_SECS
        })
        .max_by(|a, b| a.1.slow_uses.cmp(&b.1.slow_uses).then_with(|| b.0.cmp(a.0)))
        .map(|(id, _, _)| *id)
}

/// Resolve the next learn-hint to show, if any, recording the show.
///
/// Returns `(message, label)` where `message` is the fully-formed nudge text.
/// Pure-ish: reads config + bindings, mutates persisted hint counters.
fn next_learn_hint() -> Option<String> {
    let mut state = load_state();
    let now = now_unix();

    // Resolve which actions are currently bound (have a shortcut label).
    let labels: HashMap<&'static str, String> = LearnableAction::ALL
        .into_iter()
        .filter_map(|a| a.shortcut_label().map(|l| (a.id(), l)))
        .collect();

    let stats: HashMap<&'static str, ActionStat> = LearnableAction::ALL
        .into_iter()
        .map(|a| {
            let stat = state.actions.get(a.id()).cloned().unwrap_or_default();
            (a.id(), stat)
        })
        .collect();

    let candidates: Vec<(&'static str, &ActionStat, bool)> = LearnableAction::ALL
        .into_iter()
        .filter_map(|a| {
            // `stats` is built from the same `ALL` list above, so this always
            // resolves; filter_map keeps the invariant panic-free anyway.
            let stat = stats.get(a.id())?;
            Some((a.id(), stat, labels.contains_key(a.id())))
        })
        .collect();

    let chosen_id = pick_action_id(&candidates, now)?;
    let action = LearnableAction::from_id(chosen_id)?;
    let label = labels.get(chosen_id)?.clone();

    // Record the show.
    let stat = state.actions.entry(chosen_id.to_string()).or_default();
    stat.hints_shown = stat.hints_shown.saturating_add(1);
    stat.last_hint_unix = now;
    save_state(&state);

    Some(format!(
        "⌨ You usually {} the slow way \u{2014} press {} next time",
        action.phrase(),
        label
    ))
}

impl App {
    /// Record a fast (shortcut) use of `action`.
    pub(crate) fn record_keybinding_fast(&self, action: LearnableAction) {
        if !crate::config::config().display.keybinding_hints {
            return;
        }
        record_fast(action);
    }

    /// Record a slow (slash command / menu) use of `action`, then maybe surface
    /// a single learned-keybinding nudge for the highest-tax unlearned binding.
    pub(crate) fn record_keybinding_slow(&mut self, action: LearnableAction) {
        if !crate::config::config().display.keybinding_hints {
            return;
        }
        record_slow(action);
        self.maybe_surface_learn_hint();
    }

    /// Surface at most one learn-hint per session, in the distinct learn-hint
    /// color slot. No-op when hints are disabled or already shown this session.
    fn maybe_surface_learn_hint(&mut self) {
        if self.learn_hint_shown_this_session {
            return;
        }
        if let Some(message) = next_learn_hint() {
            self.learn_hint = Some((message, std::time::Instant::now()));
            self.learn_hint_shown_this_session = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stat(fast: u32, slow: u32, hints: u32, last: u64) -> ActionStat {
        ActionStat {
            fast_uses: fast,
            slow_uses: slow,
            hints_shown: hints,
            last_hint_unix: last,
        }
    }

    #[test]
    fn picks_highest_slow_tax_unlearned_bound_action() {
        let a = stat(0, 5, 0, 0);
        let b = stat(0, 2, 0, 0);
        let cands = [("model_switch", &a, true), ("resume", &b, true)];
        assert_eq!(pick_action_id(&cands, 10_000_000), Some("model_switch"));
    }

    #[test]
    fn skips_learned_bindings() {
        let learned = stat(LEARNED_FAST_THRESHOLD, 9, 0, 0);
        let cands = [("resume", &learned, true)];
        assert_eq!(pick_action_id(&cands, 10_000_000), None);
    }

    #[test]
    fn skips_unbound_actions() {
        let s = stat(0, 9, 0, 0);
        let cands = [("resume", &s, false)];
        assert_eq!(pick_action_id(&cands, 10_000_000), None);
    }

    #[test]
    fn requires_minimum_slow_uses() {
        let s = stat(0, MIN_SLOW_BEFORE_HINT - 1, 0, 0);
        let cands = [("resume", &s, true)];
        assert_eq!(pick_action_id(&cands, 10_000_000), None);
    }

    #[test]
    fn respects_per_action_hint_cap() {
        let s = stat(0, 9, MAX_HINTS_PER_ACTION, 0);
        let cands = [("resume", &s, true)];
        assert_eq!(pick_action_id(&cands, 10_000_000), None);
    }

    #[test]
    fn respects_cooldown() {
        let now = 10_000_000u64;
        let recent = stat(0, 9, 1, now - 1);
        let cands = [("resume", &recent, true)];
        assert_eq!(pick_action_id(&cands, now), None);

        let old = stat(0, 9, 1, now - HINT_COOLDOWN_SECS);
        let cands = [("resume", &old, true)];
        assert_eq!(pick_action_id(&cands, now), Some("resume"));
    }

    #[test]
    fn fast_and_slow_counters_persist_independently() {
        let _guard = crate::storage::lock_test_env();
        let temp = tempfile::tempdir().expect("tempdir");
        let prev = std::env::var_os("KCODE_HOME");
        crate::env::set_var("KCODE_HOME", temp.path());

        record_slow(LearnableAction::Resume);
        record_slow(LearnableAction::Resume);
        record_fast(LearnableAction::Resume);

        let state = load_state();
        let stat = state.actions.get("resume").expect("stat present");
        assert_eq!(stat.slow_uses, 2);
        assert_eq!(stat.fast_uses, 1);

        if let Some(prev) = prev {
            crate::env::set_var("KCODE_HOME", prev);
        } else {
            crate::env::remove_var("KCODE_HOME");
        }
    }
}
