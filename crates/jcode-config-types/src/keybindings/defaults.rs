use crate::*;

impl Default for KeybindingsConfig {
    fn default() -> Self {
        // Pull platform-appropriate defaults from the single source of truth in
        // `keybindings.rs`. This is where the macOS vs Windows/Linux split takes
        // effect: each field resolves to its own platform's default binding.
        let p = KeybindingPlatform::current();
        let get = |id: &str, fallback: &'static str| {
            default_binding(id, p).unwrap_or(fallback).to_string()
        };
        Self {
            scroll_up: get("scroll_up", "ctrl+k"),
            scroll_down: get("scroll_down", "ctrl+j"),
            scroll_page_up: get("scroll_page_up", "alt+u"),
            scroll_page_down: get("scroll_page_down", "alt+d"),
            model_switch_next: get("model_switch_next", "ctrl+tab"),
            model_switch_prev: get("model_switch_prev", "ctrl+shift+tab"),
            fallback_switch: get("fallback_switch", "ctrl+y"),
            effort_increase: get("effort_increase", "alt+right"),
            effort_decrease: get("effort_decrease", "alt+left"),
            speed_increase: get("speed_increase", "alt+up"),
            speed_decrease: get("speed_decrease", "alt+down"),
            centered_toggle: get("centered_toggle", "alt+c"),
            scroll_prompt_up: get("scroll_prompt_up", "ctrl+["),
            scroll_prompt_down: get("scroll_prompt_down", "ctrl+]"),
            scroll_bookmark: get("scroll_bookmark", "ctrl+g"),
            scroll_to_bottom: get("scroll_to_bottom", "ctrl+end, alt+q"),
            auto_poke_toggle: get("auto_poke_toggle", "ctrl+p"),
            scroll_up_fallback: get("scroll_up_fallback", ""),
            scroll_down_fallback: get("scroll_down_fallback", ""),
            workspace_left: get("workspace_left", "alt+h"),
            workspace_down: get("workspace_down", "alt+j"),
            workspace_up: get("workspace_up", "alt+k"),
            workspace_right: get("workspace_right", "alt+l"),
            side_panel_toggle: get("side_panel_toggle", "alt+m"),
            copy_selection_toggle: get("copy_selection_toggle", "alt+y"),
            diagram_pane_toggle: get("diagram_pane_toggle", "alt+t"),
            diagram_pane_visibility_toggle: get("diagram_pane_visibility_toggle", "alt+shift+m"),
            typing_scroll_lock_toggle: get("typing_scroll_lock_toggle", "alt+s"),
            diff_mode_cycle: get("diff_mode_cycle", ""),
            commit_log: get("commit_log", "alt+g"),
            info_widget_toggle: get("info_widget_toggle", "alt+i"),
            todo_card_toggle: get("todo_card_toggle", "alt+x"),
            swarm_panel_focus: get("swarm_panel_focus", "alt+n"),
            new_terminal: get("new_terminal", ""),
            open_resume: get(
                "open_resume",
                if cfg!(target_os = "macos") {
                    "cmd+b"
                } else {
                    "alt+r"
                },
            ),
            voice_input: get("voice_input", "ctrl+space"),
            session_picker_enter: SessionPickerResumeAction::CurrentTerminal,
        }
    }
}
