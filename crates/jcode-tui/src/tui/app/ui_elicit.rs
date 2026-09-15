//! Key handling for the native elicitation overlay.
//!
//! The overlay is modal: all keyboard input is routed here while active.
//! Escape or Cancel dismisses with `ElicitAction::Cancel`. Enter or Confirm
//! sends the current selection/value back via the oneshot channel.

use super::App;
use crate::tui::elicitation_types::{
    ButtonSpec, ChoiceOption, DateTimeKind, ElicitAction, ElicitOverlayState, ElicitRequest,
    ElicitResponse, FieldSpec, NotesSpec, Urgency,
};
use crossterm::event::{KeyCode, KeyEventKind, KeyModifiers};

impl App {
    /// Handle an incoming elicitation message from the tool-core global channel.
    ///
    /// Converts tool-core types to TUI types, sets up a bridge oneshot channel,
    /// and activates the modal overlay.
    pub(super) fn handle_elicit_msg(&mut self, msg: jcode_tool_core::ElicitMessage) {
        let jcode_tool_core::ElicitMessage {
            request: tc_req,
            response_tx: tc_tx,
        } = msg;

        // Convert field spec from JSON to typed enum
        let field = parse_field_spec(&tc_req.field);

        // Convert notes
        let notes = tc_req.notes.as_ref().and_then(parse_notes_spec);

        // Convert buttons
        let buttons = tc_req.buttons.as_ref().and_then(parse_button_spec);

        // Convert urgency
        let urgency = parse_urgency(&tc_req.urgency);

        let tui_request = ElicitRequest {
            title: tc_req.title,
            field,
            intent: tc_req.intent,
            question: tc_req.question,
            notes,
            buttons,
            request_id: tc_req.request_id,
            timeout_secs: tc_req.timeout_secs,
            urgency,
        };

        // Create a bridge: TUI sends ElicitResponse via bridge_tx, a spawned
        // task converts it to tool-core ElicitResponse and forwards to tc_tx.
        let (bridge_tx, bridge_rx) = tokio::sync::oneshot::channel::<ElicitResponse>();
        tokio::spawn(async move {
            match bridge_rx.await {
                Ok(tui_resp) => {
                    let action_str = match tui_resp.action {
                        ElicitAction::Confirm => "confirm".to_string(),
                        ElicitAction::Cancel => "cancel".to_string(),
                    };
                    let tc_resp = jcode_tool_core::ElicitResponse {
                        action: action_str,
                        value: tui_resp.value,
                        notes: tui_resp.notes,
                    };
                    let _ = tc_tx.send(tc_resp);
                }
                Err(_) => {
                    // Bridge dropped (overlay dismissed without response).
                    let _ = tc_tx.send(jcode_tool_core::ElicitResponse {
                        action: "cancel".to_string(),
                        value: None,
                        notes: None,
                    });
                }
            }
        });

        // Initialize text_buffer with default value if present
        let text_buffer = match &tui_request.field {
            FieldSpec::Text {
                default: Some(d), ..
            }
            | FieldSpec::LongText {
                default: Some(d), ..
            }
            | FieldSpec::DateTime {
                default: Some(d), ..
            } => d.clone(),
            FieldSpec::Boolean {
                default: Some(true),
                ..
            } => "Y".to_string(),
            FieldSpec::Boolean {
                default: Some(false),
                ..
            } => "N".to_string(),
            FieldSpec::Integer {
                default: Some(d), ..
            } => d.to_string(),
            _ => String::new(),
        };

        let selected = match &tui_request.field {
            FieldSpec::Choice {
                default_index: Some(idx),
                options,
                ..
            } => (*idx).min(options.len().saturating_sub(1)),
            _ => 0,
        };

        self.elicit_overlay = Some(ElicitOverlayState {
            request: tui_request,
            cursor: 0,
            selected,
            text_buffer,
            response_tx: Some(bridge_tx),
        });
    }
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

// ---------------------------------------------------------------------------
// Parse helpers: tool-core JSON values → TUI typed structs
// ---------------------------------------------------------------------------

fn parse_field_spec(v: &serde_json::Value) -> FieldSpec {
    let kind = v.get("kind").and_then(|k| k.as_str()).unwrap_or("text");
    let label = v
        .get("label")
        .and_then(|l| l.as_str())
        .unwrap_or("")
        .to_string();

    match kind {
        "text" => FieldSpec::Text {
            label,
            default: v.get("default").and_then(|d| d.as_str()).map(String::from),
            placeholder: v.get("placeholder").and_then(|p| p.as_str()).map(String::from),
            max_length: v.get("max_length").and_then(|m| m.as_u64()).map(|n| n as u32),
            secret: v.get("secret").and_then(|s| s.as_bool()).unwrap_or(false),
        },
        "long_text" => FieldSpec::LongText {
            label,
            default: v.get("default").and_then(|d| d.as_str()).map(String::from),
            max_length: v.get("max_length").and_then(|m| m.as_u64()).map(|n| n as u32),
        },
        "integer" => FieldSpec::Integer {
            label,
            default: v.get("default").and_then(|d| d.as_i64()),
            min: v.get("min").and_then(|m| m.as_i64()),
            max: v.get("max").and_then(|m| m.as_i64()),
        },
        "choice" => {
            let options = v
                .get("options")
                .and_then(|o| o.as_array())
                .map(|arr| {
                    arr.iter()
                        .map(|opt| ChoiceOption {
                            label: opt
                                .get("label")
                                .and_then(|l| l.as_str())
                                .unwrap_or("")
                                .to_string(),
                            value: opt
                                .get("value")
                                .and_then(|val| val.as_str())
                                .unwrap_or("")
                                .to_string(),
                            description: opt
                                .get("description")
                                .and_then(|d| d.as_str())
                                .map(String::from),
                        })
                        .collect()
                })
                .unwrap_or_default();
            FieldSpec::Choice {
                label,
                options,
                default_index: v.get("default_index").and_then(|i| i.as_u64()).map(|n| n as usize),
            }
        }
        "boolean" => FieldSpec::Boolean {
            label,
            default: v.get("default").and_then(|d| d.as_bool()),
        },
        "date_time" => FieldSpec::DateTime {
            label,
            default: v.get("default").and_then(|d| d.as_str()).map(String::from),
            picker_kind: match v
                .get("picker_kind")
                .and_then(|p| p.as_str())
                .unwrap_or("datetime")
            {
                "date" => DateTimeKind::Date,
                "time" => DateTimeKind::Time,
                _ => DateTimeKind::DateTime,
            },
        },
        // Fallback: treat unknown kinds as text
        _ => FieldSpec::Text {
            label,
            default: None,
            placeholder: None,
            max_length: None,
            secret: false,
        },
    }
}

fn parse_notes_spec(v: &serde_json::Value) -> Option<NotesSpec> {
    Some(NotesSpec {
        label: v
            .get("label")
            .and_then(|l| l.as_str())
            .unwrap_or("Notes")
            .to_string(),
        default: v.get("default").and_then(|d| d.as_str()).map(String::from),
        max_length: v.get("max_length").and_then(|m| m.as_u64()).map(|n| n as u32),
        required: v.get("required").and_then(|r| r.as_bool()).unwrap_or(false),
    })
}

fn parse_button_spec(v: &serde_json::Value) -> Option<ButtonSpec> {
    Some(ButtonSpec {
        confirm: v
            .get("confirm")
            .and_then(|c| c.as_str())
            .unwrap_or("OK")
            .to_string(),
        cancel: v
            .get("cancel")
            .and_then(|c| c.as_str())
            .unwrap_or("Cancel")
            .to_string(),
        default_is_cancel: v
            .get("default_is_cancel")
            .and_then(|d| d.as_bool())
            .unwrap_or(false),
    })
}

fn parse_urgency(s: &str) -> Urgency {
    match s {
        "warning" => Urgency::Warning,
        "error" => Urgency::Error,
        "secret" => Urgency::Secret,
        _ => Urgency::Info,
    }
}
