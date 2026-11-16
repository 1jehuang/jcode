use super::*;

impl App {
    /// Re-parse keybinding snapshots when the config cache has reloaded.
    ///
    /// The parsed bindings are cached on `App` for cheap per-keystroke lookup,
    /// so without this poll a config.toml keybinding edit would only take
    /// effect after a restart. Called from the idle tick in both local and
    /// remote run loops, and again immediately before dispatching a key press
    /// so an edit lands on the very next keystroke even when the run loop is
    /// sitting at the 5s deep-idle cadence. The generation check makes the
    /// no-change path a single atomic load. Returns true when bindings were
    /// re-parsed.
    pub(in crate::tui::app) fn refresh_keybindings_if_config_reloaded(&mut self) -> bool {
        // config() performs the throttled file-fingerprint staleness check and
        // bumps the reload generation when config.toml changed on disk.
        crate::config::config();
        let generation = crate::config::config_reload_generation();
        if generation == self.keybindings_config_generation {
            return false;
        }
        self.keybindings_config_generation = generation;
        self.model_switch_keys = keybind::load_model_switch_keys();
        self.effort_switch_keys = keybind::load_effort_switch_keys();
        self.speed_switch_keys = keybind::load_speed_switch_keys();
        self.centered_toggle_keys = keybind::load_centered_toggle_key();
        self.toggle_keys = keybind::load_toggle_keys();
        self.workspace_navigation_keys = keybind::load_workspace_navigation_keys();
        self.dictation_key = keybind::load_dictation_key();
        self.new_terminal_key = keybind::load_new_terminal_key();
        self.open_resume_key = keybind::load_open_resume_key();
        self.voice_input_key = keybind::load_voice_input_key();
        self.fallback_switch_key = keybind::load_fallback_switch_key();
        self.scroll_keys = keybind::load_scroll_keys();
        crate::logging::info("KEYBINDINGS: reloaded from config change");
        // Confirm the pickup to the user. Without this, an edit that is
        // already live is indistinguishable from one that silently did
        // nothing, which is the main source of "did that actually apply?".
        self.set_status_notice("Config reloaded from disk");
        true
    }
}
