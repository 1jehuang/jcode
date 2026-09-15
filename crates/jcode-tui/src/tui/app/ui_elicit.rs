//! Key handling for the native elicitation overlay.
//!
//! The overlay is modal: all keyboard input is routed here while active.
//! Escape or Cancel dismisses with `ElicitAction::Cancel`. Enter or Confirm
//! sends the current selection/value back via the oneshot channel.

use super::App;
use crate::tui::elicitation_types::{
    ElicitAction, ElicitOverlayState, ElicitResponse, FieldSpec,
};
use crossterm::event::{KeyCode, KeyEventKind, KeyModifiers};

impl App {
    /// Handle a key press while the elicitation overlay is active.
    ///
    /// All keys are consumed; nothing falls through to the normal input path.
    pub(super) fn handle_elicit_key(&mut self, code: KeyCode, modifiers: KeyModifiers) {
        let Some(ref mut state) = self.elicit_overlay else {
            return;
        };

        // We need to take ownership to send the response, so clone the state
        // temporarily. This is fine because the overlay is modal and only one
        // can be active at a time.
        let state_clone = state.request.clone();
        let field = &state_clone.field;

        match code {
            // ---- Global cancel ----
            KeyCode::Esc => {
                self.send_elicit_response(ElicitResponse {
                    action: ElicitAction::Cancel,
                    value: None,
                    notes: None,
                });
                return;
            }

            // ---- Confirm / submit ----
            KeyCode::Enter => {
                match field {
                    FieldSpec::Boolean { .. } => {
                        // For boolean, Enter confirms the current selection
                        // (Y or N must be pressed first, or default is used).
                        let value = self
                            .elicit_overlay
                            .as_ref()
                            .map(|s| {
                                if s.text_buffer == "Y" || s.text_buffer == "y" {
                                    "true".to_string()
                                } else if s.text_buffer == "N" || s.text_buffer == "n" {
                                    "false".to_string()
                                } else {
                                    // Use default or "false"
                                    match &s.request.field {
                                        FieldSpec::Boolean {
                                            default: Some(true),
                                            ..
                                        } => "true".to_string(),
                                        _ => "false".to_string(),
                                    }
                                }
                            })
                            .unwrap_or_else(|| "false".to_string());
                        self.send_elicit_response(ElicitResponse {
                            action: ElicitAction::Confirm,
                            value: Some(value),
                            notes: None,
                        });
                        return;
                    }
                    FieldSpec::Choice {
                        options, ..
                    } => {
                        let selected = self
                            .elicit_overlay
                            .as_ref()
                            .map(|s| s.selected)
                            .unwrap_or(0);
                        let value = options
                            .get(selected)
                            .map(|opt| opt.value.clone())
                            .unwrap_or_default();
                        self.send_elicit_response(ElicitResponse {
                            action: ElicitAction::Confirm,
                            value: Some(value),
                            notes: None,
                        });
                        return;
                    }
                    FieldSpec::Text { .. }
                    | FieldSpec::Integer { .. }
                    | FieldSpec::DateTime { .. } => {
                        let value = self
                            .elicit_overlay
                            .as_ref()
                            .map(|s| s.text_buffer.clone())
                            .unwrap_or_default();
                        self.send_elicit_response(ElicitResponse {
                            action: ElicitAction::Confirm,
                            value: Some(value),
                            notes: None,
                        });
                        return;
                    }
                    FieldSpec::LongText { .. } => {
                        // Ctrl+Enter submits multi-line text; plain Enter inserts newline.
                        if modifiers.contains(KeyModifiers::CONTROL) {
                            let value = self
                                .elicit_overlay
                                .as_ref()
                                .map(|s| s.text_buffer.clone())
                                .unwrap_or_default();
                            self.send_elicit_response(ElicitResponse {
                                action: ElicitAction::Confirm,
                                value: Some(value),
                                notes: None,
                            });
                            return;
                        }
                        // Plain Enter: insert newline in the text buffer.
                        if let Some(ref mut state) = self.elicit_overlay {
                            state.text_buffer.push('\n');
                        }
                        return;
                    }
                }
            }

            // ---- Arrow navigation for choice type ----
            KeyCode::Up => {
                if let Some(ref mut state) = self.elicit_overlay {
                    if let FieldSpec::Choice { options, .. } = &state.request.field {
                        if !options.is_empty() && state.selected > 0 {
                            state.selected -= 1;
                        }
                    }
                }
                return;
            }
            KeyCode::Down => {
                if let Some(ref mut state) = self.elicit_overlay {
                    if let FieldSpec::Choice { options, .. } = &state.request.field {
                        if state.selected + 1 < options.len() {
                            state.selected += 1;
                        }
                    }
                }
                return;
            }

            // ---- Boolean quick keys ----
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                if matches!(&self.elicit_overlay.as_ref().map(|s| &s.request.field), Some(FieldSpec::Boolean { .. })) {
                    self.send_elicit_response(ElicitResponse {
                        action: ElicitAction::Confirm,
                        value: Some("true".to_string()),
                        notes: None,
                    });
                    return;
                }
            }
            KeyCode::Char('n') | KeyCode::Char('N') => {
                if matches!(&self.elicit_overlay.as_ref().map(|s| &s.request.field), Some(FieldSpec::Boolean { .. })) {
                    self.send_elicit_response(ElicitResponse {
                        action: ElicitAction::Confirm,
                        value: Some("false".to_string()),
                        notes: None,
                    });
                    return;
                }
            }

            // ---- Text input for Text/LongText/Integer/DateTime types ----
            KeyCode::Char(ch) => {
                // Only accept character input for text-like fields.
                let is_text_field = matches!(
                    &self.elicit_overlay.as_ref().map(|s| &s.request.field),
                    Some(FieldSpec::Text { .. } | FieldSpec::LongText { .. } | FieldSpec::DateTime { .. })
                );
                let is_int_field = matches!(
                    &self.elicit_overlay.as_ref().map(|s| &s.request.field),
                    Some(FieldSpec::Integer { .. })
                );
                if is_text_field || (is_int_field && ch.is_ascii_digit()) {
                    if let Some(ref mut state) = self.elicit_overlay {
                        // Check max_length constraint.
                        let max_ok = match &state.request.field {
                            FieldSpec::Text { max_length, .. } | FieldSpec::LongText { max_length, .. } => {
                                max_length.map_or(true, |max| state.text_buffer.len() < max as usize)
                            }
                            _ => true,
                        };
                        if max_ok {
                            state.text_buffer.push(ch);
                        }
                    }
                }
                return;
            }

            // ---- Backspace for text fields ----
            KeyCode::Backspace => {
                let is_text_field = matches!(
                    &self.elicit_overlay.as_ref().map(|s| &s.request.field),
                    Some(FieldSpec::Text { .. } | FieldSpec::LongText { .. } | FieldSpec::DateTime { .. } | FieldSpec::Integer { .. })
                );
                if is_text_field {
                    if let Some(ref mut state) = self.elicit_overlay {
                        state.text_buffer.pop();
                    }
                }
                return;
            }

            // ---- Tab: cycle through interactive areas (fields -> notes -> buttons) ----
            KeyCode::Tab => {
                if let Some(ref mut state) = self.elicit_overlay {
                    state.cursor = state.cursor.wrapping_add(1) % 3;
                }
                return;
            }

            _ => {
                // Consume all other keys (no fallthrough).
                return;
            }
        }
    }

    /// Send the elicitation response and clear the overlay.
    fn send_elicit_response(&mut self, response: ElicitResponse) {
        if let Some(mut state) = self.elicit_overlay.take() {
            if let Some(tx) = state.response_tx.take() {
                let _ = tx.send(response);
            }
        }
    }
}
