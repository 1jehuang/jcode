//! Server-authoritative, session-only instruction profile controls.
use super::{App, DisplayMessage};
use crate::tui::backend::RemoteConnection;
use crate::tui::{InlineInteractiveState, PickerAction, PickerEntry, PickerKind, PickerOption};
use crossterm::event::{KeyCode, KeyModifiers};

#[derive(Clone, serde::Deserialize)]
struct InstructionSource {
    id: String,
    kind: String,
    label: String,
    path: Option<String>,
    available: bool,
    included: bool,
    #[serde(skip)]
    saved_snapshot: bool,
}

#[derive(Default)]
pub(super) struct AgentProfileUi {
    sources: Option<Vec<InstructionSource>>,
    current: Option<String>,
    notice: String,
    draft: Option<ProfileDraft>,
    apply_request_id: Option<u64>,
}

struct ProfileDraft {
    name: String,
    editing_current: bool,
    sources: Vec<InstructionSource>,
    parent: InlineInteractiveState,
    error: Option<String>,
}

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
    pub(super) fn invalidate_agent_profile_ui(&mut self) {
        self.agent_profile_picker_request_id = None;
        if self.showing_agent_profiles() || self.showing_agent_profile_sources() {
            self.inline_interactive_state = None;
        }
        // Keep outstanding IDs in the control-traffic set so delayed replies
        // cannot settle a turn or a newer profile selection after switching.
        self.agent_profile_ui = Default::default();
    }

    pub(super) fn showing_agent_profile_sources(&self) -> bool {
        self.inline_interactive_state
            .as_ref()
            .is_some_and(InlineInteractiveState::is_agent_profile_sources)
    }

    pub(super) fn open_agent_profile_sources(&mut self, name: String) {
        let Some(mut sources) = self.agent_profile_ui.sources.clone() else {
            self.fail_agent_profile_list("The server returned an incompatible instruction-profile list; source-selection support requires a server upgrade.");
            return;
        };
        let editing_current = self.agent_profile_ui.current.as_deref() == Some(name.as_str());
        if !editing_current {
            for source in &mut sources {
                source.included = false;
            }
        } else {
            for source in &mut sources {
                source.saved_snapshot = source.included;
            }
        }
        let Some(parent) = self.inline_interactive_state.take() else {
            return;
        };
        self.agent_profile_ui.draft = Some(ProfileDraft {
            name,
            editing_current,
            sources,
            parent,
            error: None,
        });
        self.refresh_agent_profile_sources(None);
    }

    fn refresh_agent_profile_sources(&mut self, selected: Option<usize>) {
        let Some(draft) = &self.agent_profile_ui.draft else {
            return;
        };
        let mut entries = Vec::new();
        for source in &draft.sources {
            entries.push(profile_entry(
                format!(
                    "[{}] {}",
                    if source.included { "x" } else { " " },
                    source.label
                ),
                PickerAction::AgentProfileSource(source.id.clone()),
                false,
                source.available || source.saved_snapshot,
                &format!(
                    "{} · {}{}",
                    source.path.as_deref().unwrap_or(&source.kind),
                    if source.included { "ON" } else { "OFF" },
                    if !source.available && source.included {
                        " · saved snapshot, file unavailable"
                    } else if !source.available {
                        " · unavailable"
                    } else {
                        ""
                    }
                ),
            ));
        }
        let apply_index = entries.len();
        entries.push(profile_entry(
            "Apply".into(),
            PickerAction::AgentProfileApply(draft.name.clone()),
            false,
            true,
            draft.error.as_deref().unwrap_or(""),
        ));
        entries[apply_index].options[0].provider = self.agent_profile_ui.notice.clone();
        entries.push(profile_entry(
            "Cancel".into(),
            PickerAction::AgentProfileCancel,
            false,
            true,
            "Discard this draft; no instructions changed",
        ));
        self.set_agent_profile_entries(entries);
        if let Some(picker) = self.inline_interactive_state.as_mut() {
            picker.selected = selected
                .unwrap_or(apply_index)
                .min(picker.entries.len() - 1);
        }
    }

    fn cancel_agent_profile_sources(&mut self) {
        if let Some(draft) = self.agent_profile_ui.draft.take() {
            self.inline_interactive_state = Some(draft.parent);
        }
    }

    fn agent_profile_sources_error(&mut self, message: String) {
        self.agent_profile_ui.apply_request_id = None;
        if let Some(draft) = self.agent_profile_ui.draft.as_mut() {
            draft.error = Some(message);
            let selected = self
                .inline_interactive_state
                .as_ref()
                .map(|picker| picker.selected);
            self.refresh_agent_profile_sources(selected);
        }
    }

    pub(super) fn settle_agent_profile_apply(&mut self, id: u64, error: Option<&str>) -> bool {
        if self.agent_profile_ui.apply_request_id != Some(id) {
            return false;
        }
        self.agent_profile_request_ids.remove(&id);
        if let Some(error) = error {
            self.agent_profile_sources_error(error.to_owned());
        } else {
            self.agent_profile_ui = Default::default();
            self.inline_interactive_state = None;
        }
        true
    }

    pub(super) async fn handle_agent_profile_sources_key(
        &mut self,
        code: KeyCode,
        modifiers: KeyModifiers,
        remote: &mut RemoteConnection,
    ) -> anyhow::Result<()> {
        if self.agent_profile_ui.apply_request_id.is_some() {
            return Ok(());
        }
        let action = self
            .inline_interactive_state
            .as_ref()
            .and_then(|picker| picker.selected_entry())
            .map(|entry| entry.action.clone());
        match code {
            KeyCode::Esc => self.cancel_agent_profile_sources(),
            KeyCode::Enter | KeyCode::Char(' ') => match action {
                Some(PickerAction::AgentProfileCancel) if code == KeyCode::Enter => {
                    self.cancel_agent_profile_sources();
                }
                Some(PickerAction::AgentProfileSource(id)) => {
                    if let Some(source) =
                        self.agent_profile_ui.draft.as_mut().and_then(|draft| {
                            draft.sources.iter_mut().find(|source| source.id == id)
                        })
                        && (source.available || source.saved_snapshot)
                    {
                        source.included = !source.included;
                    }
                    let selected = self
                        .inline_interactive_state
                        .as_ref()
                        .map(|picker| picker.selected);
                    self.refresh_agent_profile_sources(selected);
                }
                Some(PickerAction::AgentProfileApply(_)) if code == KeyCode::Enter => {
                    let Some(draft) = self.agent_profile_ui.draft.as_ref() else {
                        return Ok(());
                    };
                    let ids: Vec<&str> = draft
                        .sources
                        .iter()
                        .filter(|source| source.included)
                        .map(|source| source.id.as_str())
                        .collect();
                    let json = serde_json::to_string(&ids)?;
                    let args = if draft.editing_current {
                        vec!["--sources".into(), draft.name.clone(), json]
                    } else {
                        vec!["--select".into(), draft.name.clone(), json]
                    };
                    match remote.runtime_command("agent", args).await {
                        Ok(id) => {
                            self.agent_profile_request_ids.insert(id);
                            self.agent_profile_ui.apply_request_id = Some(id);
                            if let Some(draft) = self.agent_profile_ui.draft.as_mut() {
                                draft.error = Some("Applying instruction sources…".into());
                            }
                            let selected = self
                                .inline_interactive_state
                                .as_ref()
                                .map(|picker| picker.selected);
                            self.refresh_agent_profile_sources(selected);
                        }
                        Err(error) => self.agent_profile_sources_error(format!(
                            "Failed to reach the jcode server: {error}"
                        )),
                    }
                }
                _ => {}
            },
            KeyCode::Tab
            | KeyCode::BackTab
            | KeyCode::Up
            | KeyCode::Down
            | KeyCode::Home
            | KeyCode::End
            | KeyCode::PageUp
            | KeyCode::PageDown => {
                let navigation = match code {
                    KeyCode::Tab => KeyCode::Down,
                    KeyCode::BackTab => KeyCode::Up,
                    code => code,
                };
                self.handle_inline_interactive_key(navigation, KeyModifiers::empty())?;
            }
            _ if modifiers.contains(KeyModifiers::CONTROL) => {
                self.set_status_notice(
                    "Instruction sources are session-only; Apply or Cancel this draft",
                );
            }
            _ => {}
        }
        Ok(())
    }

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
        let sources = data
            .get("instruction_sources")
            .cloned()
            .and_then(|value| serde_json::from_value::<Vec<InstructionSource>>(value).ok());
        let included = data
            .get("included_sources")
            .and_then(serde_json::Value::as_array);
        let valid_sources = sources.as_ref().is_some_and(|sources| {
            let mut ids = std::collections::HashSet::new();
            sources.iter().all(|source| {
                !source.id.is_empty() && !source.label.is_empty() && ids.insert(source.id.as_str())
            }) && included.is_some_and(|included| {
                included.iter().all(|id| {
                    id.as_str().is_some_and(|id| {
                        sources
                            .iter()
                            .any(|source| source.id == id && source.included)
                    })
                }) && sources
                    .iter()
                    .filter(|source| source.included)
                    .all(|source| {
                        included
                            .iter()
                            .any(|id| id.as_str() == Some(source.id.as_str()))
                    })
            })
        });
        if !valid_sources {
            self.agent_profile_ui.sources = None;
            self.fail_agent_profile_list("The server returned an incompatible instruction-profile list; upgrade the live server for isolated profiles and optional instruction sources.");
            return;
        }
        self.agent_profile_ui.current = current.map(str::to_owned);
        self.agent_profile_ui.sources = sources;
        self.agent_profile_ui.notice = data
            .get("instruction_source_notice")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("Provider-native instructions may still apply outside Jcode control.")
            .to_owned();
        let mut entries = vec![profile_entry(
            "None".into(),
            PickerAction::AgentProfileChoice(None),
            current.is_none(),
            true,
            "Remove the profile and restore normal instructions · this session only",
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
                "Enter to choose optional Jcode-loaded sources · profile alone by default · this session only",
            ));
        }
        self.set_agent_profile_entries(entries);
    }
}

pub(super) async fn open_agent_profiles(app: &mut App, remote: &mut RemoteConnection) {
    app.agent_profile_ui = Default::default();
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
