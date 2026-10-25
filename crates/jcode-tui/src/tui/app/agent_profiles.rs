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
            self.open_agents_picker();
            self.push_display_message(DisplayMessage::error(
                "The server returned an incompatible instruction-profile list; no instructions changed.",
            ));
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
                "Apply Markdown instructions on next turn · current session only",
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
            app.open_agents_picker();
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
