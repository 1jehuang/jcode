//! Server-authoritative additive agent profile controls.
use super::{App, DisplayMessage};
use crate::tui::backend::RemoteConnection;
use crate::tui::{InlineInteractiveState, PickerAction, PickerEntry, PickerKind, PickerOption};

pub(super) fn profile_entry(
    name: String,
    action: PickerAction,
    current: bool,
    available: bool,
    detail: &str,
) -> PickerEntry {
    PickerEntry {
        name,
        options: vec![PickerOption {
            provider: if current { "current" } else { "instructions" }.into(),
            api_method: "session only".into(),
            available,
            detail: detail.into(),
            estimated_reference_cost_micros: None,
        }],
        action,
        selected_option: 0,
        is_current: current,
        is_default: false,
        is_favorite: false,
        recommended: false,
        recommendation_rank: usize::MAX,
        usage_score: 0,
        old: false,
        created_date: None,
        effort: None,
    }
}

impl App {
    pub(super) fn fail_agent_profile_list(&mut self, error: &str) {
        if !self.showing_agent_profiles() {
            return;
        }
        let unsupported = error.contains("unknown variant")
            || error
                .to_ascii_lowercase()
                .contains("unknown runtime command")
            || error.contains("incompatible instruction-profile list");
        let recovery = if unsupported {
            "Server upgrade required for /agent support; reopen /agents after upgrading the live server"
        } else {
            "Reopen /agents to retry when the server is ready"
        };
        self.set_agent_profile_entries(vec![profile_entry(
            "Instruction profiles unavailable".into(),
            PickerAction::AgentProfileChoice(None),
            false,
            false,
            &format!("{recovery} · Esc to close · no instructions changed · Server: {error}"),
        )]);
    }

    pub(super) fn handle_agent_profile_list_error(&mut self, id: u64, message: &str) -> bool {
        let Some(request_id) = self.agent_profile_picker_request_id else {
            return false;
        };
        // Older daemons reject the unknown runtime-command variant before they
        // can recover the request ID. Do not consume unrelated id-0 turn errors.
        let legacy_unsupported = id == 0
            && message.contains("unknown variant")
            && (message.contains("`agent`") || message.contains("'agent'"));
        if id != request_id && !legacy_unsupported {
            return false;
        }
        self.agent_profile_picker_request_id = None;
        self.agent_profile_request_ids.remove(&request_id);
        self.fail_agent_profile_list(message);
        self.push_display_message(DisplayMessage::error(message.to_owned()));
        true
    }

    pub(super) fn showing_agent_profiles(&self) -> bool {
        self.inline_interactive_state
            .as_ref()
            .is_some_and(|picker| {
                !picker.entries.is_empty()
                    && picker
                        .entries
                        .iter()
                        .all(|entry| matches!(entry.action, PickerAction::AgentProfileChoice(_)))
            })
    }

    fn set_agent_profile_entries(&mut self, entries: Vec<PickerEntry>) {
        let selected = entries
            .iter()
            .position(|entry| entry.is_current)
            .unwrap_or(0);
        self.inline_view_state = None;
        self.inline_interactive_state = Some(InlineInteractiveState {
            kind: PickerKind::Model,
            filtered: (0..entries.len()).collect(),
            entries,
            // Focus the authoritative current row, or None, never a default filename.
            selected,
            column: 0,
            filter: String::new(),
            preview: false,
            scoped_route_restore: Vec::new(),
            effort_step: None,
        });
    }

    pub(super) fn apply_agent_profile_list(&mut self, data: &serde_json::Value) {
        if !self.showing_agent_profiles() {
            return;
        }
        if !data
            .get("current")
            .is_some_and(|value| value.is_null() || value.is_string())
            || !data
                .get("available")
                .and_then(serde_json::Value::as_array)
                .is_some_and(|names| names.iter().all(serde_json::Value::is_string))
        {
            self.fail_agent_profile_list(
                "The server returned an incompatible instruction-profile list; no instructions changed.",
            );
            return;
        }
        let current = data.get("current").and_then(serde_json::Value::as_str);
        let mut entries = vec![profile_entry(
            "None".into(),
            PickerAction::AgentProfileChoice(None),
            current.is_none(),
            true,
            "No additive profile · base instructions unchanged · current session only",
        )];
        let mut names: Vec<String> = data
            .get("available")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(serde_json::Value::as_str)
            .map(str::to_owned)
            .collect();
        if let Some(name) = current
            && !names.iter().any(|item| item == name)
        {
            names.push(name.into());
        }
        names.sort();
        names.dedup();
        for name in names {
            let is_current = current == Some(name.as_str());
            entries.push(profile_entry(
                name.clone(),
                PickerAction::AgentProfileChoice(Some(name)),
                is_current,
                true,
                "Instruction body only · model, tools and permissions unchanged · this session only",
            ));
        }
        self.set_agent_profile_entries(entries);
    }
}

pub(super) async fn open_agent_profiles(app: &mut App, remote: &mut RemoteConnection) {
    app.set_agent_profile_entries(vec![profile_entry(
        "Loading instruction profiles…".into(),
        PickerAction::AgentProfileChoice(None),
        false,
        false,
        "Fetching current and available profiles from this session's server",
    )]);
    match remote.runtime_command("agent", vec![]).await {
        Ok(id) => {
            app.agent_profile_request_ids.insert(id);
            app.agent_profile_picker_request_id = Some(id);
        }
        Err(error) => {
            app.agent_profile_picker_request_id = None;
            app.fail_agent_profile_list(&format!("Failed to list instruction profiles: {error}"));
            app.push_display_message(DisplayMessage::error(format!(
                "Failed to list instruction profiles: {error}"
            )));
        }
    }
}

/// Use token boundaries so `/agents` keeps its existing role-model picker.
pub(super) fn agent_profile_args(input: &str) -> Option<Vec<String>> {
    let mut words = input.split_whitespace();
    (words.next()? == "/agent").then(|| words.map(str::to_string).collect())
}

pub(super) async fn send_agent_profile_command(
    app: &mut App,
    remote: &mut RemoteConnection,
    args: Vec<String>,
) {
    match remote.runtime_command("agent", args).await {
        Ok(id) => {
            app.agent_profile_request_ids.insert(id);
        }
        Err(error) => app.push_display_message(DisplayMessage::error(format!(
            "Failed to reach the jcode server for /agent: {error}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyModifiers};

    fn loading_app() -> App {
        let mut app = crate::tui::app::tests::create_test_app();
        app.set_agent_profile_entries(vec![profile_entry(
            "Loading instruction profiles…".into(),
            PickerAction::AgentProfileChoice(None),
            false,
            false,
            "Fetching profiles",
        )]);
        app.agent_profile_picker_request_id = Some(42);
        app.agent_profile_request_ids.insert(42);
        app
    }

    fn assert_unavailable(app: &App) {
        assert!(app.showing_agent_profiles());
        let picker = app.inline_interactive_state.as_ref().unwrap();
        assert_eq!(picker.entries.len(), 1);
        let entry = &picker.entries[0];
        assert_eq!(entry.name, "Instruction profiles unavailable");
        let option = entry.active_option().unwrap();
        assert!(!option.available);
        assert!(option.detail.contains("no instructions changed"));
        assert!(!entry.is_current);
        assert!(!entry.is_default);
    }

    #[test]
    fn unsupported_profile_discovery_stays_visible_and_escape_closes() {
        let mut app = loading_app();
        assert!(app.handle_agent_profile_list_error(42, "Unknown runtime command: agent"));
        assert_unavailable(&app);
        assert!(
            app.inline_interactive_state.as_ref().unwrap().entries[0]
                .active_option()
                .unwrap()
                .detail
                .starts_with("Server upgrade required")
        );
        assert!(app.agent_profile_picker_request_id.is_none());
        assert!(!app.agent_profile_request_ids.contains(&42));
        app.handle_inline_interactive_key(KeyCode::Enter, KeyModifiers::NONE)
            .unwrap();
        assert_unavailable(&app);
        app.handle_inline_interactive_key(KeyCode::Esc, KeyModifiers::NONE)
            .unwrap();
        assert!(app.inline_interactive_state.is_none());
    }

    #[test]
    fn malformed_profile_list_stays_in_picker() {
        let mut app = loading_app();
        app.apply_agent_profile_list(&serde_json::json!({"available": []}));
        assert_unavailable(&app);
    }

    #[test]
    fn busy_profile_discovery_offers_retry_without_requiring_upgrade() {
        let mut app = loading_app();
        assert!(app.handle_agent_profile_list_error(42, "Agent is busy; try again"));
        assert_unavailable(&app);
        let detail = &app.inline_interactive_state.as_ref().unwrap().entries[0]
            .active_option()
            .unwrap()
            .detail;
        assert!(detail.contains("Agent is busy; try again"));
        assert!(detail.contains("Reopen /agents to retry"));
        assert!(!detail.contains("Server upgrade required"));
    }

    #[test]
    fn unrelated_errors_do_not_settle_profile_discovery() {
        let mut app = loading_app();
        assert!(!app.handle_agent_profile_list_error(0, "Rate limit exceeded"));
        assert!(!app.handle_agent_profile_list_error(43, "Unknown runtime command: agent"));
        assert_eq!(app.agent_profile_picker_request_id, Some(42));
        assert!(app.agent_profile_request_ids.contains(&42));
        assert!(app.handle_agent_profile_list_error(0, "unknown variant `agent`, expected `mcp`"));
        assert_unavailable(&app);
    }

    #[test]
    fn late_discovery_error_does_not_reopen_dismissed_picker() {
        let mut app = loading_app();
        app.handle_inline_interactive_key(KeyCode::Esc, KeyModifiers::NONE)
            .unwrap();
        assert!(app.handle_agent_profile_list_error(42, "Unknown runtime command: agent"));
        assert!(app.inline_interactive_state.is_none());
    }
}
