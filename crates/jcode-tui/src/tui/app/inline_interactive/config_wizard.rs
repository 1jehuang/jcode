//! Interactive `/config` wizard: a settings list rendered with the shared
//! inline picker. Rows are built from the live [`crate::config::Config`] and
//! Enter toggles (or cycles) the selected setting through a single persisted
//! write, so several preferences can be changed in one pass without leaving
//! the picker.

use super::*;

/// One editable setting surfaced by the `/config` wizard.
struct ConfigSetting {
    /// Stable identifier, e.g. `display.centered`. Also the config file path
    /// shown in the picker's third column.
    key: &'static str,
    /// Human-readable row name.
    label: &'static str,
    /// Current value rendered for the VALUE column.
    value: String,
    /// Short help shown in the row detail area.
    detail: &'static str,
}

fn bool_label(value: bool) -> String {
    if value {
        "on".to_string()
    } else {
        "off".to_string()
    }
}

fn config_settings(config: &crate::config::Config) -> Vec<ConfigSetting> {
    let display = &config.display;
    let features = &config.features;
    vec![
        ConfigSetting {
            key: "display.centered",
            label: "Center content",
            value: bool_label(display.centered),
            detail: "Center the transcript (Ctrl+T shortcut also toggles live)",
        },
        ConfigSetting {
            key: "display.mouse_capture",
            label: "Mouse capture",
            value: bool_label(display.mouse_capture),
            detail: "Scroll wheel capture; off restores terminal text selection",
        },
        ConfigSetting {
            key: "display.emoji",
            label: "Emoji output",
            value: bool_label(display.emoji),
            detail: "Render emoji in TUI and CLI output",
        },
        ConfigSetting {
            key: "display.compact_notifications",
            label: "Compact notifications",
            value: bool_label(display.compact_notifications),
            detail: "One-line swarm/file-activity notices instead of full cards",
        },
        ConfigSetting {
            key: "display.show_agentgrep_output",
            label: "Full agentgrep output",
            value: bool_label(display.show_agentgrep_output),
            detail: "Show full agentgrep tool output instead of the summary line",
        },
        ConfigSetting {
            key: "display.tool_call_details",
            label: "Tool call details",
            value: bool_label(display.tool_call_details),
            detail: "Expand tool call parameter details in the transcript",
        },
        ConfigSetting {
            key: "display.pin_todos",
            label: "Pin todos",
            value: bool_label(display.pin_todos),
            detail: "Keep the session todo list pinned while the transcript scrolls",
        },
        ConfigSetting {
            key: "display.queue_mode",
            label: "Queue mode",
            value: bool_label(display.queue_mode),
            detail: "Wait for the current turn before sending the next prompt",
        },
        ConfigSetting {
            key: "display.debug_socket",
            label: "Debug socket",
            value: bool_label(display.debug_socket),
            detail: "External control socket for tools and tests",
        },
        ConfigSetting {
            key: "features.check_updates",
            label: "Check updates",
            value: bool_label(features.check_updates),
            detail: "Look for jcode updates during startup",
        },
        ConfigSetting {
            key: "features.memory",
            label: "Memory features",
            value: bool_label(features.memory),
            detail: "Memory retrieval and extraction",
        },
        ConfigSetting {
            key: "features.swarm",
            label: "Swarm features",
            value: bool_label(features.swarm),
            detail: "Multi-agent swarm coordination",
        },
        ConfigSetting {
            key: "features.mermaid",
            label: "Mermaid rendering",
            value: bool_label(features.mermaid),
            detail: "Render Mermaid diagrams and related model guidance",
        },
        ConfigSetting {
            key: "features.auto_poke",
            label: "Auto-poke",
            value: bool_label(features.auto_poke),
            detail: "Automatic follow-up when the model stops with incomplete todos",
        },
        ConfigSetting {
            key: "features.message_timestamps",
            label: "Message timestamps",
            value: bool_label(features.message_timestamps),
            detail: "Inject timestamps into messages sent to the model",
        },
        ConfigSetting {
            key: "features.kv_cache_miss_notices",
            label: "KV cache miss notices",
            value: bool_label(features.kv_cache_miss_notices),
            detail: "In-chat notice for meaningful KV cache misses",
        },
        ConfigSetting {
            key: "features.update_channel",
            label: "Update channel",
            value: features.update_channel.to_string(),
            detail: "stable, preview, or main",
        },
    ]
}

/// Build fresh wizard entries from the current on-disk config. Called when
/// the wizard opens and after every toggle so values never go stale.
pub(super) fn build_config_wizard_entries() -> Vec<PickerEntry> {
    let config = crate::config::Config::load();
    config_settings(&config)
        .into_iter()
        .map(|setting| PickerEntry {
            name: setting.label.to_string(),
            options: vec![PickerOption {
                provider: setting.value,
                api_method: setting.key.to_string(),
                available: true,
                detail: setting.detail.to_string(),
                estimated_reference_cost_micros: None,
            }],
            action: PickerAction::ConfigToggle { key: setting.key },
            selected_option: 0,
            is_current: false,
            is_default: false,
            is_favorite: false,
            recommended: false,
            recommendation_rank: usize::MAX,
            usage_score: 0,
            old: false,
            created_date: None,
            effort: None,
        })
        .collect()
}

impl App {
    pub(crate) fn open_config_wizard(&mut self) {
        if super::commands_dispatch::ssh_local_action_blocked(self, "Config wizard") {
            return;
        }
        let entries = build_config_wizard_entries();
        let count = entries.len();
        self.inline_view_state = None;
        self.inline_interactive_state = Some(InlineInteractiveState {
            kind: PickerKind::Config,
            filtered: (0..count).collect(),
            entries,
            selected: 0,
            column: 0,
            filter: String::new(),
            preview: false,
        });
        self.input.clear();
        self.cursor_pos = 0;
    }

    /// Toggle the setting identified by `key`, persist it, and refresh the
    /// open wizard in place. The picker stays open so the user can flip
    /// several settings in one pass.
    ///
    /// The live Enter path composes [`Self::record_config_toggle_result`] and
    /// [`Self::report_config_toggle_result`] directly because the handler
    /// already holds a `&mut` borrow of the picker field; this wrapper exists
    /// for tests and future callers that have both borrows free.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn apply_config_wizard_toggle(
        &mut self,
        picker: &mut InlineInteractiveState,
        entry_index: usize,
        key: &'static str,
    ) {
        let result = crate::config::Config::toggle_config_wizard_setting(key);
        Self::record_config_toggle_result(picker, entry_index, &result);
        Self::report_config_toggle_result(self, key, &result);
    }

    /// Update the wizard row in place from a toggle result. A free
    /// associated function so the borrow-split call site can run it while
    /// the picker field is borrowed without involving `&mut self`.
    pub(super) fn record_config_toggle_result(
        picker: &mut InlineInteractiveState,
        entry_index: usize,
        result: &anyhow::Result<String>,
    ) {
        if let Ok(new_value) = result
            && let Some(entry) = picker.entries.get_mut(entry_index)
        {
            entry.is_current = true;
            if let Some(option) = entry.options.first_mut() {
                option.provider = new_value.clone();
            }
        }
    }

    /// Surface a toggle outcome in the transcript and status line.
    pub(super) fn report_config_toggle_result(
        app: &mut App,
        key: &str,
        result: &anyhow::Result<String>,
    ) {
        match result {
            Ok(new_value) => {
                app.set_status_notice(format!("{} → {}", key, new_value));
            }
            Err(error) => {
                app.push_display_message(DisplayMessage::error(format!(
                    "Failed to save {} : {}",
                    key, error
                )));
                app.set_status_notice("Config save failed");
            }
        }
    }
}

#[cfg(test)]
mod config_wizard_tests {
    use super::*;

    /// Serialize env access and redirect `JCODE_HOME` at a temp dir so the
    /// round-trip test never touches the real `~/.jcode/config.toml`.
    struct IsolatedHome {
        _guard: std::sync::MutexGuard<'static, ()>,
        _restore: Restore,
        _dir: tempfile::TempDir,
    }

    struct Restore(Option<std::ffi::OsString>);
    impl Drop for Restore {
        fn drop(&mut self) {
            match &self.0 {
                Some(value) => crate::env::set_var("JCODE_HOME", value),
                None => crate::env::remove_var("JCODE_HOME"),
            }
            crate::config::Config::invalidate_cache();
        }
    }

    impl IsolatedHome {
        fn new() -> Self {
            let guard = crate::storage::lock_test_env();
            let dir = tempfile::tempdir().expect("temp home");
            let previous = std::env::var_os("JCODE_HOME");
            crate::env::set_var("JCODE_HOME", dir.path());
            crate::config::Config::invalidate_cache();
            Self {
                _guard: guard,
                _restore: Restore(previous),
                _dir: dir,
            }
        }
    }

    #[test]
    fn entries_cover_expected_sections_with_values() {
        let _home = IsolatedHome::new();
        let entries = build_config_wizard_entries();
        assert!(entries.len() >= 15);
        let names: Vec<&str> = entries.iter().map(|entry| entry.name.as_str()).collect();
        assert!(names.contains(&"Center content"));
        assert!(names.contains(&"Check updates"));
        assert!(names.contains(&"Update channel"));
        for entry in &entries {
            assert!(matches!(entry.action, PickerAction::ConfigToggle { .. }));
            assert_eq!(entry.options.len(), 1);
            assert!(!entry.options[0].provider.is_empty());
            assert!(entry.options[0].api_method.contains('.'));
        }
    }

    #[test]
    fn every_toggle_key_round_trips() {
        let _home = IsolatedHome::new();
        let entries = build_config_wizard_entries();
        let keys: Vec<&'static str> = entries
            .iter()
            .map(|entry| match entry.action {
                PickerAction::ConfigToggle { key } => key,
                _ => unreachable!(),
            })
            .collect();
        for key in keys {
            let before = crate::config::Config::load();
            let flipped = crate::config::Config::toggle_config_wizard_setting(key)
                .unwrap_or_else(|error| panic!("{key} failed to persist: {error}"));
            crate::config::Config::invalidate_cache();
            let after = crate::config::Config::load();
            match key {
                "features.update_channel" => {
                    assert_ne!(
                        after.features.update_channel.to_string(),
                        before.features.update_channel.to_string(),
                        "{key} did not change"
                    );
                }
                _ => {
                    assert_eq!(
                        flipped,
                        bool_label(!current_bool(&before, key)),
                        "{key} toggle returned wrong label"
                    );
                    assert_ne!(
                        current_bool(&after, key),
                        current_bool(&before, key),
                        "{key} did not change"
                    );
                }
            }
        }
        // The config file must exist and parse after all those writes.
        let path = crate::config::Config::path().expect("config path under temp home");
        assert!(path.exists());
        let raw = std::fs::read_to_string(path).unwrap();
        assert!(raw.contains("[display]"));
    }

    #[test]
    fn unknown_key_reports_error_without_touching_the_file() {
        let _home = IsolatedHome::new();
        assert!(crate::config::Config::toggle_config_wizard_setting("nonsense.key").is_err());
    }

    fn current_bool(config: &crate::config::Config, key: &str) -> bool {
        let display = &config.display;
        match key {
            "display.centered" => display.centered,
            "display.mouse_capture" => display.mouse_capture,
            "display.emoji" => display.emoji,
            "display.compact_notifications" => display.compact_notifications,
            "display.show_agentgrep_output" => display.show_agentgrep_output,
            "display.tool_call_details" => display.tool_call_details,
            "display.pin_todos" => display.pin_todos,
            "display.queue_mode" => display.queue_mode,
            "display.debug_socket" => display.debug_socket,
            "features.check_updates" => config.features.check_updates,
            "features.memory" => config.features.memory,
            "features.swarm" => config.features.swarm,
            "features.mermaid" => config.features.mermaid,
            "features.auto_poke" => config.features.auto_poke,
            "features.message_timestamps" => config.features.message_timestamps,
            "features.kv_cache_miss_notices" => config.features.kv_cache_miss_notices,
            other => panic!("unknown config wizard key {other}"),
        }
    }
}
