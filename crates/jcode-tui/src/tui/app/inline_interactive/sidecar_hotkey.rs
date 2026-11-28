use super::super::inline_interactive::{PickerAction, picker_is_runtime_model_picker};
use super::helpers::model_entry_base_name;
use super::{App, DisplayMessage};
use crate::tui::app::inline_interactive::key_char_eq_ignore_ascii_case;
use anyhow::Result;
use crossterm::event::{KeyCode, KeyModifiers};

impl App {
    pub(crate) fn set_selected_model_as_sidecar(&mut self) {
        let Some(model_spec) = (|| {
            let picker = self.inline_interactive_state.as_ref()?;
            if !picker_is_runtime_model_picker(picker) || picker.filtered.is_empty() {
                return None;
            }
            let idx = picker.filtered[picker.selected];
            let entry = picker.entries.get(idx)?;
            if !matches!(entry.action, PickerAction::Model) {
                return None;
            }
            let bare_name = model_entry_base_name(entry);
            let route = entry.options.get(entry.selected_option)?;
            let selection = crate::provider::MultiProvider::default_model_selection_from_route(
                &bare_name,
                &route.api_method,
                &route.provider,
            );
            Some(selection.model_spec)
        })() else {
            return;
        };

        match crate::config::Config::set_memory_model(&model_spec) {
            Ok(()) => {
                self.set_status_notice(format!("Sidecar → {}", model_spec));
                self.push_display_message(DisplayMessage::system(format!(
                    "Set memory sidecar model to {}. Memory extractions will use this model.",
                    model_spec
                )));
            }
            Err(e) => self.set_status_notice(format!("Failed to set sidecar: {}", e)),
        }
    }

    /// When a runtime model-picker preview is visible, route its favorite/default
    /// hotkeys to the focused picker handler. Returns true if the key was consumed.
    pub(in crate::tui::app) fn model_picker_preview_hotkey(
        &mut self,
        code: KeyCode,
        modifiers: KeyModifiers,
    ) -> Result<bool> {
        let active = self
            .inline_interactive_state
            .as_ref()
            .map(|picker| picker.preview && picker_is_runtime_model_picker(picker))
            .unwrap_or(false);
        if !active {
            return Ok(false);
        }
        // Ctrl+O sets the default, Ctrl+N toggles favorite, Alt+S sets the
        // sidecar; Ctrl+S stays free for the swarm model sub-picker.
        let is_default =
            modifiers.contains(KeyModifiers::CONTROL) && key_char_eq_ignore_ascii_case(code, 'o');
        let is_favorite =
            modifiers.contains(KeyModifiers::CONTROL) && key_char_eq_ignore_ascii_case(code, 'n');
        let is_sidecar =
            modifiers.contains(KeyModifiers::ALT) && key_char_eq_ignore_ascii_case(code, 's');
        if is_default || is_favorite || is_sidecar {
            self.handle_inline_interactive_key(code, modifiers)?;
            return Ok(true);
        }
        Ok(false)
    }
}
