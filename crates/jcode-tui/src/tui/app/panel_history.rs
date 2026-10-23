//! Session-scoped metadata picker. Archived bodies stay in the store until reopen.
use super::{App, DisplayMessage};
use crate::side_panel::{SidePanelPageSource, SidePanelPageStatus, SidePanelSnapshot};
use crate::tui::{InlineInteractiveState, PickerAction, PickerEntry, PickerKind, PickerOption};

pub(super) fn active_session_id(app: &App) -> String {
    if app.is_remote {
        app.remote_session_id
            .clone()
            .unwrap_or_else(|| app.session.id.clone())
    } else {
        app.session.id.clone()
    }
}

fn timestamp(value: u64) -> String {
    i64::try_from(value)
        .ok()
        .filter(|v| *v > 0)
        .and_then(chrono::DateTime::<chrono::Utc>::from_timestamp_millis)
        .map(|time| time.format("%Y-%m-%d %H:%M UTC").to_string())
        .unwrap_or_else(|| "unknown".into())
}

fn entry(
    session: &str,
    id: &str,
    title: &str,
    path: &str,
    source: SidePanelPageSource,
    status: &SidePanelPageStatus,
    state: &str,
    current: bool,
) -> PickerEntry {
    PickerEntry {
        name: title.replace(['\n', '\r', '\t'], " "),
        options: vec![PickerOption {
            provider: state.into(),
            api_method: source.as_str().into(),
            available: true,
            detail: format!(
                "{} · {} · opened {} · updated {}{}",
                source.as_str(),
                path.replace(['\n', '\r', '\t'], " "),
                timestamp(status.last_opened_at_ms),
                timestamp(status.last_successful_update_ms),
                status
                    .stale_reason
                    .as_ref()
                    .map(|reason| format!(
                        " · Enter retries source · {}",
                        reason.replace(['\n', '\r', '\t'], " ")
                    ))
                    .unwrap_or_default()
            ),
            estimated_reference_cost_micros: None,
        }],
        action: PickerAction::Panel {
            session_id: session.into(),
            panel_id: id.into(),
        },
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

fn history_entries(snapshot: &SidePanelSnapshot, session: &str, hidden: bool) -> Vec<PickerEntry> {
    // A delayed inventory from a previous session must never become searchable.
    if snapshot
        .session_id
        .as_deref()
        .is_some_and(|id| id != session)
    {
        return Vec::new();
    }
    let mut entries = Vec::new();
    for page in &snapshot.pages {
        if page.source == SidePanelPageSource::Ephemeral {
            continue;
        }
        let unavailable = page.status.stale_reason.is_some() || snapshot.unavailable.is_some();
        let state = if unavailable {
            "unavailable · open"
        } else if hidden {
            "hidden"
        } else {
            "open"
        };
        entries.push((
            page.status.last_opened_at_ms,
            entry(
                session,
                &page.id,
                &page.title,
                &page.file_path,
                page.source,
                &page.status,
                state,
                snapshot.focused_page_id.as_deref() == Some(&page.id),
            ),
        ));
    }
    for page in &snapshot.archived {
        if page.source == SidePanelPageSource::Ephemeral
            || snapshot.pages.iter().any(|open| open.id == page.id)
        {
            continue;
        }
        let state = if page.status.stale_reason.is_some() {
            "unavailable · archived"
        } else {
            "archived"
        };
        entries.push((
            page.status.last_opened_at_ms,
            entry(
                session,
                &page.id,
                &page.title,
                &page.file_path,
                page.source,
                &page.status,
                state,
                false,
            ),
        ));
    }
    entries.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.name.cmp(&b.1.name)));
    entries.into_iter().map(|(_, entry)| entry).collect()
}

pub(super) fn handle_panels_command(app: &mut App, trimmed: &str) -> bool {
    let rest = if trimmed == "/panels" {
        ""
    } else if let Some(rest) = trimmed.strip_prefix("/panels ") {
        rest
    } else {
        return false;
    };
    let session = active_session_id(app);
    let entries = history_entries(&app.side_panel, &session, app.side_panel_user_hidden);
    app.inline_view_state = None;
    let mut picker = InlineInteractiveState {
        kind: PickerKind::Panels,
        filtered: (0..entries.len()).collect(),
        entries,
        selected: 0,
        column: 0,
        filter: rest.trim().into(),
        preview: false,
    };
    App::apply_inline_interactive_filter(&mut picker);
    if picker.entries.is_empty() {
        app.set_status_notice("No recent panels in this session");
    }
    app.inline_interactive_state = Some(picker);
    app.input.clear();
    app.cursor_pos = 0;
    true
}

impl App {
    pub(super) fn reopen_panel_from_history(&mut self, session: &str, id: &str) {
        if active_session_id(self) != session {
            self.set_status_notice("Panel belongs to another session");
            return;
        }
        self.remember_side_panel_viewport();
        if self.is_remote {
            self.pending_panel_action = Some((session.into(), "reopen".into(), id.into()));
            self.set_status_notice("Reopening panel...");
            return;
        }
        match crate::side_panel::reopen_page(session, id) {
            Ok(snapshot) => {
                self.set_side_panel_snapshot(snapshot);
                self.side_panel.focused_page_id = Some(id.into());
                self.last_side_panel_focus_id = Some(id.into());
                self.side_panel_user_hidden = false;
                self.side_panel_explicit_hidden = false;
                self.side_panel_fullscreen = false;
                self.restore_side_panel_viewport();
                self.set_diff_pane_focus(true);
                self.sync_diagram_fit_context();
                self.set_status_notice("Panel reopened");
            }
            Err(error) => self.push_display_message(DisplayMessage::error(format!(
                "Couldn't reopen panel: {error}"
            ))),
        }
    }
}

#[cfg(test)]
#[path = "panel_history_tests.rs"]
mod tests;
