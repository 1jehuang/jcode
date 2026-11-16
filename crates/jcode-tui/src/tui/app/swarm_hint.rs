//! One-time TUI nudge teaching that the swarm config is a prompt file.
//!
//! Swarms are complicated, dynamic systems, so their routing policy (which
//! model/effort each spawned worker gets) is passed to the model as a prompt
//! (`swarm-prompt.md`) rather than as options in a standard config file. Users
//! do not discover this on their own, so the first few times the root session
//! actually invokes the `swarm` tool we surface a short hint pointing at the
//! editable prompt file.
//!
//! Follows the same shape as `shortcut_hints`: at most one show per session,
//! a small lifetime show cap persisted to disk, rendered in the learn-hint
//! pop-out slot so it is visually distinct from tool output.

use serde::{Deserialize, Serialize};

use super::App;

/// Never show the swarm-config hint more than this many times, ever.
const MAX_SHOWS: u32 = 3;

const STATE_FILE: &str = "swarm_config_hint.json";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct SwarmHintState {
    #[serde(default)]
    shows: u32,
}

fn state_path() -> Option<std::path::PathBuf> {
    crate::storage::app_config_dir()
        .ok()
        .map(|dir| dir.join(STATE_FILE))
}

fn load_state() -> SwarmHintState {
    let Some(path) = state_path() else {
        return SwarmHintState::default();
    };
    crate::storage::read_json::<SwarmHintState>(&path).unwrap_or_default()
}

fn save_state(state: &SwarmHintState) {
    let Some(path) = state_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Err(error) = crate::storage::write_json(&path, state) {
        crate::logging::info(&format!(
            "Failed to persist swarm-config-hint state {}: {}",
            path.display(),
            error
        ));
    }
}

/// Pure decision: should the hint be shown given the persisted show count and
/// whether it was already shown this session?
pub(super) fn should_show(shows: u32, shown_this_session: bool) -> bool {
    !shown_this_session && shows < MAX_SHOWS
}

/// The hint text pointing at the editable swarm prompt/config file.
pub(super) fn hint_message() -> String {
    "\u{2699} Swarm routing (models, effort) is configured by a prompt, not a config file. Edit ~/.kcode/swarm-prompt.md (or ./.jcode/swarm-prompt.md) to tune it".to_string()
}

impl App {
    /// Surface the swarm-config hint the first few times the user's session
    /// invokes the `swarm` tool. No-op after the lifetime cap or once shown
    /// this session. Rendered in the learn-hint pop-out slot.
    pub(in crate::tui::app) fn maybe_surface_swarm_config_hint(&mut self) {
        if !should_show(load_state().shows, self.swarm_hint_shown_this_session) {
            return;
        }
        let mut state = load_state();
        state.shows = state.shows.saturating_add(1);
        save_state(&state);
        self.swarm_hint_shown_this_session = true;
        self.learn_hint = Some((hint_message(), std::time::Instant::now()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shows_until_lifetime_cap() {
        assert!(should_show(0, false));
        assert!(should_show(MAX_SHOWS - 1, false));
        assert!(!should_show(MAX_SHOWS, false));
        assert!(!should_show(MAX_SHOWS + 5, false));
    }

    #[test]
    fn shows_at_most_once_per_session() {
        assert!(!should_show(0, true));
    }

    #[test]
    fn hint_mentions_the_prompt_file_and_that_it_is_the_config() {
        let message = hint_message();
        assert!(message.contains("swarm-prompt.md"));
        assert!(message.contains("prompt, not a config file"));
    }

    #[test]
    fn state_persists_show_count() {
        let _guard = crate::storage::lock_test_env();
        let temp = tempfile::tempdir().expect("tempdir");
        let prev = std::env::var_os("JCODE_HOME");
        crate::env::set_var("JCODE_HOME", temp.path());

        let mut state = load_state();
        assert_eq!(state.shows, 0);
        state.shows = 2;
        save_state(&state);
        assert_eq!(load_state().shows, 2);

        if let Some(prev) = prev {
            crate::env::set_var("JCODE_HOME", prev);
        } else {
            crate::env::remove_var("JCODE_HOME");
        }
    }
}
