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
    let millis = match i64::try_from(value) {
        Ok(millis) if millis > 0 => millis,
        // Legacy records have no wall-clock timestamp. Out-of-range revisions
        // are not dates either, and must not be displayed as the Unix epoch.
        _ => return "unknown".into(),
    };
    match chrono::DateTime::<chrono::Utc>::from_timestamp_millis(millis) {
        Some(time) => time.format("%Y-%m-%d %H:%M UTC").to_string(),
        None => "unknown".into(),
    }
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
                match &status.stale_reason {
                    Some(reason) => format!(
                        " · /panels refresh {id} retries source · {}",
                        reason.replace(['\n', '\r', '\t'], " ")
                    ),
                    // No warning is normal after a successful refresh.
                    None => String::new(),
                }
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
    let mut args = rest.split_whitespace();
    if args.next() == Some("refresh") {
        match (args.next(), args.next()) {
            (Some(id), None) => app.refresh_panel_from_history(&session, id),
            _ => app.set_status_notice("Usage: /panels refresh <panel_id>"),
        }
        return true;
    }
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
        // Opening a retained page is a client presentation action, including
        // disconnected SSH clients and servers without PanelAction support.
        if self.side_panel.pages.iter().any(|page| page.id == id) {
            self.focus_retained_panel(id);
            return;
        }
        if !self.side_panel.archived.iter().any(|page| page.id == id) {
            self.set_status_notice("Panel not found in this session");
            return;
        }
        if self.is_remote {
            self.pending_panel_action = Some((session.into(), "reopen".into(), id.into()));
            self.set_status_notice("Reopening panel...");
            return;
        }
        match crate::side_panel::reopen_page(session, id) {
            Ok(snapshot) => {
                self.set_side_panel_snapshot(snapshot);
                self.focus_retained_panel(id);
            }
            Err(error) => self.push_display_message(DisplayMessage::error(format!(
                "Couldn't reopen panel: {error}"
            ))),
        }
    }

    fn focus_retained_panel(&mut self, id: &str) {
        if self
            .pending_panel_action
            .as_ref()
            .is_some_and(|(_, action, _)| matches!(action.as_str(), "focus" | "reopen"))
        {
            self.pending_panel_action = None;
        }
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

    pub(super) fn refresh_panel_from_history(&mut self, session: &str, id: &str) {
        if active_session_id(self) != session {
            self.set_status_notice("Panel belongs to another session");
            return;
        }
        if self.is_remote {
            self.pending_panel_action = Some((session.into(), "refresh".into(), id.into()));
            self.set_status_notice("Refreshing panel...");
            return;
        }
        let result = crate::side_panel::snapshot_for_session(session).and_then(|mut snapshot| {
            anyhow::ensure!(
                snapshot.pages.iter().any(|page| page.id == id),
                "Panel not found in this session: {id}"
            );
            crate::side_panel::refresh_linked_page_content(&mut snapshot, Some(id));
            Ok(snapshot)
        });
        match result {
            Ok(snapshot) => {
                self.set_side_panel_snapshot(snapshot);
                self.set_status_notice("Panel refresh checked");
            }
            Err(error) => self.push_display_message(DisplayMessage::error(format!(
                "Couldn't refresh panel: {error}"
            ))),
        }
    }
}

#[cfg(test)]
#[path = "panel_history_tests.rs"]
mod tests;
