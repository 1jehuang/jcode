//! Client-owned document selection and viewport, separate from the shared inventory.
use super::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub(super) struct PanelViewport {
    scroll: usize,
    scroll_x: i32,
    zoom: u8,
    auto_scroll: bool,
}

impl Default for PanelViewport {
    fn default() -> Self {
        Self {
            scroll: 0,
            scroll_x: 0,
            zoom: 100,
            auto_scroll: true,
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct SessionView {
    selected: Option<String>,
    last: Option<String>,
    hidden: bool,
    explicit_hidden: bool,
    fullscreen: bool,
    pages: HashMap<String, PanelViewport>,
    revision: u64,
    focus_revision: u64,
    initialized: bool,
    #[serde(skip)]
    awaiting_inventory: bool,
    #[serde(skip)]
    seen_current_inventory: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub(super) struct PanelSessions {
    active: Option<String>,
    views: HashMap<String, SessionView>,
    #[serde(skip)]
    snapshots: HashMap<String, crate::side_panel::SidePanelSnapshot>,
    #[serde(skip)]
    snapshot_order: Vec<String>,
}

impl PanelSessions {
    fn reload_path(pid: u32) -> Option<std::path::PathBuf> {
        match crate::storage::jcode_dir() {
            Ok(home) => Some(home.join(format!("client-panel-view-{pid}.json"))),
            Err(error) => {
                crate::logging::warn(&format!("Could not locate client panel view: {error}"));
                None
            }
        }
    }

    fn handoff_pid(value: Option<&str>) -> u32 {
        match value.map(str::parse::<u32>) {
            Some(Ok(pid)) => pid,
            Some(Err(error)) => {
                crate::logging::warn(&format!("Invalid client panel handoff PID: {error}"));
                std::process::id()
            }
            None => std::process::id(),
        }
    }

    pub(super) fn restore_for_client() -> Self {
        // Unix exec retains the PID. Windows replacement spawns a child instead.
        #[cfg(windows)]
        let handoff = match std::env::var("JCODE_PANEL_VIEW_RELOAD_PID") {
            Ok(value) => Some(value),
            Err(std::env::VarError::NotPresent) => None,
            Err(error) => {
                crate::logging::warn(&format!(
                    "Invalid client panel handoff environment: {error}"
                ));
                None
            }
        };
        #[cfg(not(windows))]
        let handoff: Option<String> = None;
        #[cfg(windows)]
        crate::env::remove_var("JCODE_PANEL_VIEW_RELOAD_PID");
        Self::restore_for_pid(Self::handoff_pid(handoff.as_deref()))
    }

    fn restore_for_pid(pid: u32) -> Self {
        let Some(path) = Self::reload_path(pid) else {
            return Self::default();
        };
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Self::default(),
            Err(error) => {
                crate::logging::warn(&format!("Could not read client panel view: {error}"));
                return Self::default();
            }
        };
        let mut restored = match serde_json::from_slice::<Self>(&bytes) {
            Ok(restored) => restored,
            Err(error) => {
                crate::logging::warn(&format!("Could not decode client panel view: {error}"));
                return Self::default();
            }
        };
        if let Err(error) = std::fs::remove_file(path) {
            crate::logging::warn(&format!("Could not consume client panel view: {error}"));
        }
        restored.active = None;
        restored
    }

    // Bodies are disposable. Keep view metadata even after evicting an inventory.
    fn trim_snapshot_cache(&mut self) {
        const MAX_INACTIVE: usize = 4;
        const MAX_INACTIVE_BYTES: usize = 64 * 1024 * 1024;
        self.snapshot_order
            .retain(|id| self.snapshots.contains_key(id));
        loop {
            let inactive: Vec<_> = self
                .snapshot_order
                .iter()
                .filter(|id| Some(id.as_str()) != self.active.as_deref())
                .collect();
            let bytes: usize = inactive
                .iter()
                .filter_map(|id| self.snapshots.get(*id))
                .flat_map(|snapshot| &snapshot.pages)
                .map(|page| {
                    page.content.capacity() + page.pdf_data.as_ref().map_or(0, String::capacity)
                })
                .sum();
            if inactive.len() <= MAX_INACTIVE && bytes <= MAX_INACTIVE_BYTES {
                break;
            }
            let Some(oldest) = inactive.first().map(|id| (*id).clone()) else {
                break;
            };
            self.snapshots.remove(&oldest);
            self.snapshot_order.retain(|id| id != &oldest);
        }
    }
}

impl App {
    pub(super) fn save_panel_view_for_reload(&self) {
        let Some(path) = PanelSessions::reload_path(std::process::id()) else {
            return;
        };
        let result = serde_json::to_vec(&self.saved_panel_sessions())
            .map_err(anyhow::Error::from)
            .and_then(|bytes| {
                let temporary = path.with_extension("json.tmp");
                std::fs::write(&temporary, bytes)?;
                std::fs::rename(temporary, path).map_err(anyhow::Error::from)
            });
        match result {
            Ok(()) => {
                #[cfg(windows)]
                crate::env::set_var(
                    "JCODE_PANEL_VIEW_RELOAD_PID",
                    std::process::id().to_string(),
                );
            }
            Err(error) => {
                crate::logging::warn(&format!("Could not save client panel view: {error}"))
            }
        }
    }

    pub(super) fn set_side_panel_snapshot(
        &mut self,
        snapshot: crate::side_panel::SidePanelSnapshot,
    ) {
        self.set_side_panel_snapshot_changed(snapshot);
    }

    pub(super) fn set_side_panel_snapshot_changed(
        &mut self,
        snapshot: crate::side_panel::SidePanelSnapshot,
    ) -> bool {
        let Some(snapshot) = self.retain_panel_session_snapshot(snapshot) else {
            return false;
        };
        self.refresh_split_view_if_needed();
        let focus_split = self.split_view_enabled
            && self.side_panel.focused_page_id.as_deref()
                == Some(super::split_view::SPLIT_VIEW_PAGE_ID);
        let focus_observe = self.observe_mode_enabled
            && self.side_panel.focused_page_id.as_deref() == Some(super::observe::OBSERVE_PAGE_ID);
        let snapshot = if self.split_view_enabled {
            self.decorate_side_panel_with_split_view(snapshot, focus_split)
        } else {
            snapshot
        };
        let focus_todos = self.todos_view_enabled
            && self.side_panel.focused_page_id.as_deref()
                == Some(super::todos_view::TODOS_VIEW_PAGE_ID);
        let snapshot = if self.todos_view_enabled {
            self.decorate_side_panel_with_todos_view(snapshot, focus_todos)
        } else {
            snapshot
        };
        let mut snapshot = if self.observe_mode_enabled {
            self.decorate_side_panel_with_observe(snapshot, focus_observe)
        } else {
            snapshot
        };
        if self.side_panel_user_hidden {
            snapshot.focused_page_id = None;
        }
        let changed = snapshot != self.side_panel;
        self.apply_side_panel_snapshot(snapshot);
        changed
    }

    pub(super) fn apply_side_panel_snapshot(
        &mut self,
        mut snapshot: crate::side_panel::SidePanelSnapshot,
    ) {
        self.remember_side_panel_viewport();
        // Local app panels refresh through this path too. Content updates must
        // not undo a user dismissal or replace the saved selection.
        if self.side_panel_user_hidden {
            snapshot.focused_page_id = None;
        }
        if snapshot != self.side_panel {
            self.wake_idle_hibernation();
        }
        let focused_before = self.side_panel.focused_page_id.clone();
        let focused_after = snapshot.focused_page_id.clone();
        let focused_changed = focused_before != focused_after;
        let focused_title_after = snapshot.focused_page().map(|page| page.title.clone());
        if let Some(focused_after) = focused_after.as_deref() {
            if focused_after != super::observe::OBSERVE_PAGE_ID {
                self.last_side_panel_focus_id = Some(focused_after.to_string());
            }
        } else if snapshot.pages.is_empty() {
            self.last_side_panel_focus_id = None;
        }
        self.last_side_panel_refresh = None;
        self.side_panel = snapshot;
        if self.side_panel.focused_page().is_none() {
            self.side_panel_fullscreen = false;
        }
        self.note_runtime_memory_event("side_panel_updated", "side_panel_snapshot_applied");
        if focused_changed {
            self.restore_side_panel_viewport();
        }
        if focused_changed {
            match (focused_after.as_deref(), focused_title_after.as_deref()) {
                (Some(super::split_view::SPLIT_VIEW_PAGE_ID), _) => {
                    self.set_status_notice("Split view")
                }
                (Some(super::todos_view::TODOS_VIEW_PAGE_ID), _) => self.set_status_notice("Todos"),
                (Some(super::observe::OBSERVE_PAGE_ID), _) => self.set_status_notice("Observe"),
                (Some("goals"), _) => self.set_status_notice("Goals"),
                (Some(id), Some(title)) if id.starts_with("goal.") => self.set_status_notice(title),
                _ => {}
            }
        }
        self.sync_diagram_fit_context();
        self.prewarm_focused_side_panel();
    }

    #[cfg(test)]
    pub(super) fn restore_panel_sessions(&mut self, mut sessions: PanelSessions) {
        sessions.active = None;
        self.panel_sessions = sessions;
    }

    pub(super) fn remember_side_panel_viewport(&mut self) {
        let Some(session) = self.panel_sessions.active.clone() else {
            return;
        };
        let viewport = self.current_panel_viewport();
        let view = self.panel_sessions.views.entry(session).or_default();
        if let Some(id) = &self.side_panel.focused_page_id {
            view.pages.insert(id.clone(), viewport);
        }
    }

    fn current_panel_viewport(&self) -> PanelViewport {
        PanelViewport {
            scroll: self.diff_pane_scroll,
            scroll_x: self.diff_pane_scroll_x,
            zoom: self.side_panel_image_zoom_percent,
            auto_scroll: self.diff_pane_auto_scroll,
        }
    }

    pub(super) fn restore_side_panel_viewport(&mut self) {
        let viewport = self
            .panel_sessions
            .active
            .as_ref()
            .and_then(|session| self.panel_sessions.views.get(session))
            .and_then(|view| {
                self.side_panel
                    .focused_page_id
                    .as_ref()
                    .and_then(|id| view.pages.get(id))
            })
            .cloned()
            .unwrap_or_else(PanelViewport::default);
        self.diff_pane_scroll = viewport.scroll;
        self.diff_pane_scroll_x = viewport.scroll_x;
        self.side_panel_image_zoom_percent = viewport.zoom;
        self.diff_pane_auto_scroll = viewport.auto_scroll;
    }

    pub(super) fn saved_panel_sessions(&self) -> PanelSessions {
        let mut sessions = PanelSessions {
            active: self.panel_sessions.active.clone(),
            views: self.panel_sessions.views.clone(),
            snapshots: HashMap::new(),
            snapshot_order: Vec::new(),
        };
        if let Some(session) = &sessions.active {
            let view = sessions.views.entry(session.clone()).or_default();
            if view.awaiting_inventory {
                return sessions;
            }
            view.selected = self.side_panel.focused_page_id.clone();
            view.last = self.last_side_panel_focus_id.clone();
            view.hidden = self.side_panel_user_hidden;
            view.explicit_hidden = self.side_panel_explicit_hidden;
            view.fullscreen = self.side_panel_fullscreen;
            if let Some(id) = &view.selected {
                view.pages.insert(id.clone(), self.current_panel_viewport());
            }
        }
        sessions
    }

    fn remember_panel_session_view(&mut self) {
        if self
            .panel_sessions
            .active
            .as_ref()
            .and_then(|id| self.panel_sessions.views.get(id))
            .is_some_and(|view| view.awaiting_inventory)
        {
            return;
        }
        self.remember_side_panel_viewport();
        if let Some(session) = &self.panel_sessions.active {
            let view = self
                .panel_sessions
                .views
                .entry(session.clone())
                .or_default();
            view.selected = self.side_panel.focused_page_id.clone();
            view.last = self.last_side_panel_focus_id.clone();
            view.hidden = self.side_panel_user_hidden;
            view.explicit_hidden = self.side_panel_explicit_hidden;
            view.fullscreen = self.side_panel_fullscreen;
        }
    }

    /// Called before decorating a shared inventory with transient client pages.
    pub(super) fn retain_panel_session_snapshot(
        &mut self,
        mut incoming: crate::side_panel::SidePanelSnapshot,
    ) -> Option<crate::side_panel::SidePanelSnapshot> {
        let session = self.active_client_session_id()?.to_string();
        if incoming
            .session_id
            .as_deref()
            .is_some_and(|id| id != session)
        {
            return None;
        }
        if self.panel_sessions.active.is_none() && !self.side_panel.pages.is_empty() {
            self.panel_sessions.active = Some(session.clone());
            self.remember_panel_session_view();
            let view = self
                .panel_sessions
                .views
                .entry(session.clone())
                .or_default();
            view.initialized = true;
            view.focus_revision = self.side_panel.focus_revision;
            view.revision = self.side_panel.revision;
        }
        let switched = self.panel_sessions.active.as_deref() != Some(session.as_str());
        if switched {
            self.remember_panel_session_view();
            self.panel_sessions.active = Some(session.clone());
            self.side_panel = crate::side_panel::SidePanelSnapshot::default();
            let view = self
                .panel_sessions
                .views
                .entry(session.clone())
                .or_default();
            self.last_side_panel_focus_id = view.last.clone();
            self.side_panel_user_hidden = view.hidden;
            self.side_panel_explicit_hidden = view.explicit_hidden;
            self.side_panel_fullscreen = view.fullscreen;
        } else {
            self.remember_panel_session_view();
        }
        self.panel_sessions.trim_snapshot_cache();
        let view = self
            .panel_sessions
            .views
            .entry(session.clone())
            .or_default();
        if incoming.unavailable.is_some() {
            let mut retained = self
                .panel_sessions
                .snapshots
                .get(&session)
                .cloned()
                .unwrap_or_else(|| self.side_panel.clone());
            view.awaiting_inventory = retained.pages.is_empty();
            retained.focused_page_id = view
                .selected
                .as_ref()
                .filter(|id| retained.pages.iter().any(|page| page.id == **id))
                .cloned();
            retained.session_id = Some(session);
            retained.unavailable = incoming.unavailable;
            return Some(retained);
        }
        if incoming.revision < view.revision {
            if view.seen_current_inventory {
                return None;
            }
            for page in &mut incoming.pages {
                if page.status.stale_reason.is_none() {
                    page.status.stale_reason = Some("Restored older inventory snapshot".into());
                }
            }
        }
        if view.awaiting_inventory {
            self.side_panel_fullscreen = view.fullscreen;
            self.last_side_panel_focus_id = view.last.clone();
        }
        view.awaiting_inventory = false;
        let explicit_focus = incoming.focus_revision > view.focus_revision;
        if view.initialized && !explicit_focus {
            incoming.focused_page_id = view
                .selected
                .as_ref()
                .filter(|id| incoming.pages.iter().any(|page| page.id == **id))
                .cloned();
            // A deletion may remove the selected page, but never merge deleted pages back.
            if incoming.focused_page_id.is_none() && view.selected.is_some() && !view.hidden {
                incoming.focused_page_id = incoming.pages.first().map(|page| page.id.clone());
            }
        }
        if explicit_focus {
            self.side_panel_user_hidden = false;
            self.side_panel_explicit_hidden = false;
        }
        view.revision = view.revision.max(incoming.revision);
        view.focus_revision = view.focus_revision.max(incoming.focus_revision);
        view.initialized = true;
        view.seen_current_inventory = true;
        self.panel_sessions
            .snapshots
            .insert(session.clone(), incoming.clone());
        self.panel_sessions
            .snapshot_order
            .retain(|id| id != &session);
        self.panel_sessions.snapshot_order.push(session);
        self.panel_sessions.trim_snapshot_cache();
        Some(incoming)
    }

    /// Typed model catalogs reuse History, but their default panel is not an inventory.
    pub(super) fn apply_history_panel_snapshot(
        &mut self,
        snapshot: crate::side_panel::SidePanelSnapshot,
        authoritative: bool,
    ) {
        if authoritative
            || snapshot.session_id.is_some()
            || snapshot.revision != 0
            || snapshot.unavailable.is_some()
            || !snapshot.pages.is_empty()
            || !snapshot.archived.is_empty()
        {
            self.set_side_panel_snapshot(snapshot);
        }
    }

    pub(super) fn open_registered_markdown_panel(
        &mut self,
        path: &std::path::Path,
        fallback_title: &str,
    ) {
        use std::hash::{Hash, Hasher};
        let Some(session) = self.active_client_session_id().map(str::to_owned) else {
            self.set_status_notice("Wait for the session before opening a Markdown panel");
            return;
        };
        let title = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(fallback_title);
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        path.hash(&mut hash);
        let id = format!("linked-markdown-{:016x}", hash.finish());
        match crate::side_panel::load_markdown_file(&session, &id, Some(title), path, true) {
            Ok(snapshot) => {
                self.set_side_panel_snapshot(snapshot);
                self.set_diff_pane_focus(true);
                self.set_status_notice(format!("Opened Markdown: {title}"));
            }
            Err(error) => self.set_status_notice(format!("Failed to open Markdown panel: {error}")),
        }
    }

    pub(super) fn inherit_recovered_panels(&mut self, old_session: &str) {
        self.remember_panel_session_view();
        if let Some(mut view) = self.panel_sessions.views.get(old_session).cloned() {
            view.revision = 0;
            view.focus_revision = 0;
            view.hidden = false;
            view.explicit_hidden = false;
            self.panel_sessions
                .views
                .insert(self.session.id.clone(), view);
        }
        match crate::side_panel::inherit_for_recovery(old_session, &self.session.id) {
            Ok(snapshot) => self.set_side_panel_snapshot(snapshot),
            Err(error) => {
                self.load_session_panels(&self.session.id.clone());
                self.set_status_notice(format!("Panel recovery unavailable: {error}"));
            }
        }
    }

    pub(super) fn adopt_remote_panel_session(&mut self, session_id: &str) {
        let changed = self.remote_session_id.as_deref() != Some(session_id);
        self.remote_session_id = Some(session_id.to_string());
        if changed {
            self.remote_available_service_tiers = None;
            self.mark_side_panel_unavailable("loading session");
        }
    }

    pub(super) fn mark_side_panel_unavailable(&mut self, reason: &str) {
        self.set_side_panel_snapshot(crate::side_panel::SidePanelSnapshot {
            session_id: self.active_client_session_id().map(str::to_owned),
            unavailable: Some(reason.to_string()),
            ..Default::default()
        });
    }

    pub(super) fn load_session_panels(&mut self, session_id: &str) {
        let snapshot =
            crate::side_panel::snapshot_for_session(session_id).unwrap_or_else(|error| {
                crate::side_panel::SidePanelSnapshot {
                    session_id: Some(session_id.to_string()),
                    unavailable: Some(error.to_string()),
                    ..Default::default()
                }
            });
        self.set_side_panel_snapshot(snapshot);
    }
}

#[cfg(test)]
#[path = "panel_session_tests.rs"]
mod tests;
