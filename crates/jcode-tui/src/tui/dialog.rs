//! Claude Code-style stacked dialogs for `/mcp` and `/skills`: a bordered box
//! with a title, subtitle, info rows and a navigable list. Enter opens the
//! selected entry, Esc steps back one level, typing filters searchable lists.
//!
//! `/mcp`: servers grouped by config file → server (status, command, config
//! location, View tools / Reconnect / Disable) → tools → tool detail.
//! `/skills`: every skill with source and listing cost; Enter/Space cycles
//! on → name-only → user-only → off, saved on close.

use crate::protocol::{RewindCheckpoint, RewindMode};
use crate::skill_overrides::{SkillMode, SkillOverrides};
use crossterm::event::{KeyCode, KeyModifiers};
use jcode_tui_style::theme::{accent_color, dim_color, error_color, success_color, warning_color};

#[path = "dialog_rewind.rs"]
mod rewind;
#[cfg(test)]
#[path = "dialog_rewind_tests.rs"]
mod rewind_tests;
use ratatui::prelude::*;
use ratatui::widgets::{Clear, Paragraph};
use serde_json::Value;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

#[path = "dialog_mcp.rs"]
mod mcp;
#[path = "dialog_plugin.rs"]
mod plugin;
pub use mcp::first_pending_approval;
use mcp::{
    MCP_DOCS_URL, mcp_server_view, mcp_tool_view, mcp_tools_view, scope_of, server_status, str_of,
    tone_prefix,
};

#[derive(Debug, Clone, PartialEq)]
pub enum Route {
    McpList,
    McpServer(String),
    McpTools(String),
    McpTool(String, String),
    Skills,
    Plugin(String),
    PluginDetail(String, String, String),
    PluginConfirm(Vec<String>),
    PluginAddMarketplace,
    RewindList,
    RewindModes(String),
}

#[derive(Debug, Clone, PartialEq)]
enum ItemAction {
    Open(Route),
    Back,
    Close,
    /// Runtime `mcp` command args, e.g. `["reconnect", "github"]`.
    Mcp(Vec<String>),
    CycleSkill(String),
    Plugin(Vec<String>),
    Rewind(String, RewindMode),
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Tone {
    Success,
    Error,
    Warning,
    Off,
}

impl Tone {
    fn glyph(self) -> &'static str {
        match self {
            Tone::Success => "✔",
            Tone::Error => "✘",
            Tone::Warning => "⚠",
            Tone::Off => "◯",
        }
    }

    fn color(self) -> Color {
        match self {
            Tone::Success => success_color(),
            Tone::Error => error_color(),
            Tone::Warning => warning_color(),
            Tone::Off => dim_color(),
        }
    }
}

#[derive(Debug, Clone)]
struct Item {
    group: Option<String>,
    icon: Option<Tone>,
    label: String,
    meta: Vec<String>,
    /// Matched by search but not shown (a skill's description).
    search_text: String,
    action: Option<ItemAction>,
}

#[derive(Debug, Clone)]
struct View {
    route: Route,
    title: String,
    subtitle: String,
    info: Vec<(String, String)>,
    /// Rows Claude Code prints unpadded after the aligned info rows
    /// (`Capabilities: tools`, `Tools: 4 tools`).
    inline_info: Vec<(String, String)>,
    /// Styled tags after the title (a tool's `[read-only]` …).
    title_tags: Vec<Span<'static>>,
    body: Vec<String>,
    items: Vec<Item>,
    notes: Vec<String>,
    empty: String,
    searchable: bool,
    /// Actions shown as `1. …` and runnable by digit (a server's menu).
    numbered: bool,
    /// What Esc does instead of stepping back (the approval prompt's "no").
    on_escape: Option<ItemAction>,
    filter: String,
    /// Index into the filtered items.
    selected: usize,
}

impl View {
    fn new(route: Route, title: impl Into<String>) -> Self {
        Self {
            route,
            title: title.into(),
            subtitle: String::new(),
            info: Vec::new(),
            inline_info: Vec::new(),
            title_tags: Vec::new(),
            body: Vec::new(),
            items: Vec::new(),
            notes: Vec::new(),
            empty: String::new(),
            searchable: false,
            numbered: false,
            on_escape: None,
            filter: String::new(),
            selected: 0,
        }
    }

    fn visible(&self) -> Vec<usize> {
        let needle = self.filter.to_lowercase();
        (0..self.items.len())
            .filter(|&index| {
                let item = &self.items[index];
                if !needle.is_empty()
                    && self.route == Route::RewindList
                    && matches!(item.action, Some(ItemAction::Close))
                {
                    return false;
                }
                needle.is_empty()
                    || item.label.to_lowercase().contains(&needle)
                    || item
                        .meta
                        .iter()
                        .any(|meta| meta.to_lowercase().contains(&needle))
                    || item.search_text.to_lowercase().contains(&needle)
            })
            .collect()
    }
}

/// One skill row for the `/skills` dialog.
#[derive(Debug, Clone)]
pub struct SkillRow {
    pub name: String,
    pub description: String,
    /// `user`, `project` or `plugin`.
    pub source: String,
    /// `disable-model-invocation` in frontmatter: fixed at user-only.
    pub author_user_only: bool,
}

#[derive(Debug, Clone)]
struct SkillsState {
    rows: Vec<SkillRow>,
    saved: SkillOverrides,
    current: SkillOverrides,
    sort_by_tokens: bool,
}

#[derive(Debug, PartialEq)]
pub enum DialogKey {
    Handled,
    Close,
    /// Send `mcp` with these args to the daemon.
    Mcp(Vec<String>),
    Rewind {
        checkpoint_id: String,
        mode: RewindMode,
    },
}

pub struct Dialog {
    stack: Vec<View>,
    mcp: Option<Value>,
    skills: Option<SkillsState>,
    plugin: Option<plugin::PluginState>,
    rewind: Option<Vec<RewindCheckpoint>>,
    pub pending_rewind: bool,
    /// Shown under the subtitle: progress or the last error.
    status: Option<String>,
    /// Args of the `mcp` action awaiting a reply.
    pub pending_mcp: Option<Vec<String>>,
}

impl Dialog {
    /// `/mcp` before the daemon replied.
    pub fn mcp_loading() -> Self {
        Self {
            stack: vec![View::new(Route::McpList, "Manage MCP servers")],
            mcp: None,
            skills: None,
            plugin: None,
            rewind: None,
            pending_rewind: false,
            status: Some("Loading MCP servers…".to_string()),
            pending_mcp: None,
        }
    }

    pub fn skills(rows: Vec<SkillRow>, overrides: SkillOverrides) -> Self {
        let mut dialog = Self {
            stack: Vec::new(),
            mcp: None,
            skills: Some(SkillsState {
                rows,
                saved: overrides.clone(),
                current: overrides,
                sort_by_tokens: false,
            }),
            plugin: None,
            rewind: None,
            pending_rewind: false,
            status: None,
            pending_mcp: None,
        };
        dialog.stack.push(dialog.build(&Route::Skills));
        dialog
    }

    /// Open the `/mcp` dialog on one server's page, e.g. the approval prompt.
    pub fn mcp_at_server(data: Value, server: &str) -> Self {
        let mut dialog = Self::mcp_loading();
        dialog.set_mcp(data);
        dialog.open_server(server);
        dialog
    }

    /// Show `server`'s page on top of the server list.
    pub fn open_server(&mut self, server: &str) {
        self.pop_to_root();
        let view = self.build(&Route::McpServer(server.to_string()));
        if !view.title.is_empty() {
            self.stack.push(view);
        }
    }

    pub fn is_mcp(&self) -> bool {
        self.plugin.is_none() && self.skills.is_none() && self.rewind.is_none()
    }

    pub fn set_status(&mut self, status: Option<String>) {
        self.status = status;
    }

    /// New daemon snapshot: rebuild every open view, dropping views whose
    /// server or tool disappeared, and keep each view's selection and filter.
    pub fn set_mcp(&mut self, data: Value) {
        self.mcp = Some(data);
        self.status = None;
        self.rebuild();
    }

    /// Back to the server list (after enable/disable, like Claude Code).
    pub fn pop_to_root(&mut self) {
        self.stack.truncate(1);
    }

    /// Persist `/skills` changes. Returns the message to show for a `/skills`
    /// dialog: what was saved, a failure, or `No changes`.
    pub fn finish(&self) -> Option<String> {
        let skills = self.skills.as_ref()?;
        let changed = skills
            .rows
            .iter()
            .filter(|row| {
                crate::skill_overrides::mode(&skills.saved, &row.name)
                    != crate::skill_overrides::mode(&skills.current, &row.name)
            })
            .count();
        // Claude Code reports a no-op close too.
        if changed == 0 {
            return Some("No changes".to_string());
        }
        Some(match crate::skill_overrides::save(&skills.current) {
            Ok(()) => format!(
                "Updated {changed} skill override{}",
                if changed == 1 { "" } else { "s" }
            ),
            Err(error) => format!("Failed to save skill overrides: {error}"),
        })
    }

    /// `label · meta · meta` for each item of the top view.
    #[cfg(test)]
    pub fn item_summaries(&self) -> Vec<String> {
        self.stack.last().map_or_else(Vec::new, |view| {
            view.items
                .iter()
                .map(|item| {
                    std::iter::once(item.label.as_str())
                        .chain(
                            item.meta
                                .iter()
                                .map(String::as_str)
                                .filter(|m| !m.is_empty()),
                        )
                        .collect::<Vec<_>>()
                        .join(" · ")
                })
                .collect()
        })
    }

    pub fn handle_key(&mut self, code: KeyCode, modifiers: KeyModifiers) -> DialogKey {
        if self.plugin.is_some() {
            self.rebuild();
            if self.handle_plugin_key(code) {
                return DialogKey::Handled;
            }
        }
        if self.pending_rewind {
            return DialogKey::Handled;
        }
        let Some(view) = self.stack.last_mut() else {
            return DialogKey::Close;
        };
        let visible = view.visible();
        let count = visible.len();
        let typing = !modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
        match code {
            KeyCode::Esc => {
                if !view.filter.is_empty() {
                    view.filter.clear();
                    view.selected = 0;
                    return DialogKey::Handled;
                }
                if let Some(action) = view.on_escape.clone() {
                    return self.run(Some(action));
                }
                self.stack.pop();
                // Claude Code rebuilds the screen below, so its cursor
                // starts back at the top.
                if let Some(parent) = self.stack.last_mut()
                    && parent.route == Route::McpList
                {
                    parent.selected = 0;
                }
                return if self.stack.is_empty() {
                    DialogKey::Close
                } else {
                    DialogKey::Handled
                };
            }
            KeyCode::Char('c' | 'd') if modifiers.contains(KeyModifiers::CONTROL) => {
                return DialogKey::Close;
            }
            KeyCode::Up | KeyCode::BackTab if count > 0 => {
                view.selected = (view.selected + count - 1) % count;
            }
            KeyCode::Down | KeyCode::Tab if count > 0 => {
                view.selected = (view.selected + 1) % count;
            }
            KeyCode::Char('k') if !view.searchable && count > 0 => {
                view.selected = (view.selected + count - 1) % count;
            }
            KeyCode::Char('j') if !view.searchable && count > 0 => {
                view.selected = (view.selected + 1) % count;
            }
            KeyCode::PageUp => view.selected = view.selected.saturating_sub(10),
            KeyCode::PageDown if count > 0 => view.selected = (view.selected + 10).min(count - 1),
            KeyCode::Home => view.selected = 0,
            KeyCode::End => view.selected = count.saturating_sub(1),
            KeyCode::Enter => {
                let action = visible
                    .get(view.selected)
                    .and_then(|&index| view.items[index].action.clone());
                return self.run(action);
            }
            // Claude Code numbers a server's actions; a digit runs one.
            KeyCode::Char(digit @ '1'..='9') if view.numbered => {
                let position = digit as usize - '1' as usize;
                let action = visible
                    .get(position)
                    .and_then(|&index| view.items[index].action.clone());
                return self.run(action);
            }
            KeyCode::Char(' ') if view.route == Route::Skills => {
                let action = visible
                    .get(view.selected)
                    .and_then(|&index| view.items[index].action.clone());
                return self.run(action);
            }
            KeyCode::Backspace if view.searchable => {
                view.filter.pop();
                view.selected = 0;
            }
            KeyCode::Char('t')
                if typing && view.route == Route::Skills && view.filter.is_empty() =>
            {
                if let Some(skills) = self.skills.as_mut() {
                    skills.sort_by_tokens = !skills.sort_by_tokens;
                }
                self.rebuild();
            }
            // `/` only focuses search in Claude Code; typing already does.
            KeyCode::Char('/')
                if view.searchable
                    && view.filter.is_empty()
                    && view.route != Route::PluginAddMarketplace => {}
            KeyCode::Char(ch) if typing && view.searchable => {
                view.filter.push(ch);
                view.selected = 0;
            }
            _ => {}
        }
        DialogKey::Handled
    }

    fn run(&mut self, action: Option<ItemAction>) -> DialogKey {
        match action {
            None => DialogKey::Handled,
            Some(ItemAction::Close) => DialogKey::Close,
            Some(ItemAction::Back) => {
                self.stack.pop();
                if self.stack.is_empty() {
                    DialogKey::Close
                } else {
                    DialogKey::Handled
                }
            }
            Some(ItemAction::Open(route)) => {
                let view = self.build(&route);
                self.stack.push(view);
                DialogKey::Handled
            }
            Some(ItemAction::Mcp(args)) => {
                if self.pending_mcp.is_some() {
                    return DialogKey::Handled;
                }
                let server = args.get(1).map_or("", String::as_str);
                // Claude Code swaps the page for a progress line.
                self.status = Some(match args.first().map(String::as_str) {
                    Some("reconnect") => format!(
                        "Reconnecting to {server} · Restarting MCP server process. This may take a few moments."
                    ),
                    Some("authenticate") => format!(
                        "Authenticating with {server}… A browser window will open for authentication."
                    ),
                    Some("clear-auth") => format!("Clearing authentication for {server}…"),
                    Some("enable") => format!("Enabling {server}…"),
                    Some("approve" | "approve-all") => format!("Starting {server}…"),
                    Some("reject") => format!("Skipping {server}…"),
                    _ => format!("Disabling {server}…"),
                });
                self.pending_mcp = Some(args.clone());
                DialogKey::Mcp(args)
            }
            Some(ItemAction::Rewind(checkpoint_id, mode)) => {
                self.pending_rewind = true;
                self.status = Some("Restoring checkpoint…".to_string());
                DialogKey::Rewind {
                    checkpoint_id,
                    mode,
                }
            }
            Some(ItemAction::Plugin(args)) => {
                if let Some(plugin) = &self.plugin {
                    plugin.start(args);
                }
                self.pop_to_root();
                self.rebuild();
                DialogKey::Handled
            }
            Some(ItemAction::CycleSkill(name)) => {
                if let Some(skills) = self.skills.as_mut() {
                    let next = crate::skill_overrides::mode(&skills.current, &name).next();
                    skills.current.insert(name, next);
                }
                self.rebuild();
                DialogKey::Handled
            }
        }
    }

    fn rebuild(&mut self) {
        let old = std::mem::take(&mut self.stack);
        for view in old {
            let mut fresh = self.build(&view.route);
            if fresh.title.is_empty() {
                break;
            }
            fresh.filter = view.filter;
            fresh.selected = view.selected.min(fresh.visible().len().saturating_sub(1));
            self.stack.push(fresh);
        }
    }

    /// Build a view from current data. An empty title means the route's
    /// server or tool no longer exists.
    fn build(&self, route: &Route) -> View {
        match route {
            Route::Plugin(_)
            | Route::PluginDetail(..)
            | Route::PluginConfirm(_)
            | Route::PluginAddMarketplace => self.plugin_view(route),
            Route::Skills => self.skills_view(),
            Route::RewindList => self.rewind_list_view(),
            Route::RewindModes(id) => self.rewind_modes_view(id),
            Route::McpList => self.mcp_list_view(),
            Route::McpServer(name) => self
                .mcp_server(name)
                .map_or_else(|| View::new(route.clone(), ""), mcp_server_view),
            Route::McpTools(name) => self
                .mcp_server(name)
                .map_or_else(|| View::new(route.clone(), ""), mcp_tools_view),
            Route::McpTool(server, tool) => self
                .mcp_server(server)
                .and_then(|s| {
                    s["tools"]
                        .as_array()?
                        .iter()
                        .find(|t| t["name"] == tool.as_str())
                        .map(|t| mcp_tool_view(server, t))
                })
                .unwrap_or_else(|| View::new(route.clone(), "")),
        }
    }

    fn mcp_servers(&self) -> &[Value] {
        self.mcp
            .as_ref()
            .and_then(|data| data["servers"].as_array())
            .map_or(&[], Vec::as_slice)
    }

    fn mcp_server(&self, name: &str) -> Option<&Value> {
        self.mcp_servers().iter().find(|s| s["name"] == name)
    }

    fn mcp_list_view(&self) -> View {
        let mut view = View::new(Route::McpList, "Manage MCP servers");
        let servers = self.mcp_servers();
        if self.mcp.is_none() {
            return view;
        }
        view.subtitle = format!(
            "{} server{}",
            servers.len(),
            if servers.len() == 1 { "" } else { "s" }
        );
        view.empty = "No MCP servers configured. Add servers to ~/.jcode/mcp.json or the \
                      project's .mcp.json (Claude Code's ~/.claude.json is read too), then \
                      reopen /mcp."
            .to_string();
        // Claude Code's group order, servers sorted by name within each.
        let order = ["project", "local", "user", "dynamic"];
        let mut sorted: Vec<&Value> = servers.iter().collect();
        sorted.sort_by_key(|s| {
            let rank = order
                .iter()
                .position(|scope| *scope == scope_of(s))
                .unwrap_or(order.len());
            (rank, str_of(s, "name").to_string())
        });
        for server in sorted {
            let location = str_of(server, "config_location");
            let group = match scope_of(server) {
                "project" => format!("Project MCPs ({location})"),
                "local" => format!("Local MCPs ({location})"),
                "user" => format!("User MCPs ({location})"),
                _ => "Built-in MCPs (always available)".to_string(),
            };
            let (tone, meta) = server_status(server);
            view.items.push(Item {
                group: Some(group),
                icon: Some(tone),
                label: str_of(server, "name").to_string(),
                meta: vec![meta],
                search_text: String::new(),
                action: Some(ItemAction::Open(Route::McpServer(
                    str_of(server, "name").to_string(),
                ))),
            });
        }
        if servers.iter().any(|s| s["status"] == "failed") {
            view.notes
                .push("※ Run jcode --trace to see error logs".to_string());
        }
        view.notes.push(format!("{MCP_DOCS_URL} for help"));
        view
    }

    fn skills_view(&self) -> View {
        let mut view = View::new(Route::Skills, "Skills");
        let Some(skills) = self.skills.as_ref() else {
            return view;
        };
        view.searchable = true;
        view.empty = "No skills found. Create skills in .jcode/skills/ or ~/.jcode/skills/ \
                      (~/.claude/skills/ and ~/.agents/skills/ are read too)."
            .to_string();
        let mut rows: Vec<&SkillRow> = skills.rows.iter().collect();
        if skills.sort_by_tokens {
            rows.sort_by(|a, b| {
                skill_tokens(b)
                    .cmp(&skill_tokens(a))
                    .then_with(|| a.name.cmp(&b.name))
            });
        } else {
            rows.sort_by(|a, b| a.source.cmp(&b.source).then_with(|| a.name.cmp(&b.name)));
        }
        for row in rows {
            let mode = if row.author_user_only {
                SkillMode::UserInvocableOnly
            } else if row.source == "plugin" {
                SkillMode::On
            } else {
                crate::skill_overrides::mode(&skills.current, &row.name)
            };
            let word = match mode {
                SkillMode::On | SkillMode::Off => "",
                SkillMode::NameOnly => "name-only",
                SkillMode::UserInvocableOnly => "user-only",
            };
            // Claude Code: plugin skills are managed with the plugin, not here.
            let plugin_locked = row.source == "plugin" && !row.author_user_only;
            let mut meta = Vec::new();
            if row.author_user_only {
                meta.push("locked by author".to_string());
            } else if plugin_locked {
                meta.push("locked by plugin".to_string());
            }
            meta.extend(
                [
                    word.to_string(),
                    row.source.clone(),
                    format_tokens(skill_tokens(row)),
                ]
                .into_iter()
                .filter(|m| !m.is_empty()),
            );
            view.items.push(Item {
                group: None,
                icon: Some(if mode == SkillMode::Off {
                    Tone::Off
                } else {
                    Tone::Success
                }),
                label: row.name.clone(),
                meta,
                search_text: row.description.clone(),
                action: (!row.author_user_only && !plugin_locked)
                    .then(|| ItemAction::CycleSkill(row.name.clone())),
            });
        }
        if skills.rows.iter().any(|row| row.source == "plugin") {
            view.notes
                .push("Plugin skills are managed via Claude Code's /plugin".to_string());
        }
        view
    }

    /// Rows the dialog wants when drawn in the bottom pane: its full content
    /// plus the rule and the key guide.
    pub fn desired_height(&self, width: u16) -> u16 {
        let live = self.live_plugin_view();
        let Some(view) = live.as_ref().or_else(|| self.stack.last()) else {
            return 0;
        };
        let body = self.body_lines(
            view,
            usize::from(width.saturating_sub(4)).max(10),
            usize::MAX,
        );
        let chrome = if self.hint(view).is_some() { 3 } else { 1 };
        (body.len() + chrome).min(usize::from(u16::MAX)) as u16
    }

    /// Claude Code's layout: a `▔` rule in the accent color, then the
    /// title, subtitle, content and an italic key guide, indented 3 columns.
    pub fn render(&self, frame: &mut Frame, area: Rect) {
        let live = self.live_plugin_view();
        let Some(view) = live.as_ref().or_else(|| self.stack.last()) else {
            return;
        };
        if area.height < 3 || area.width < 8 {
            return;
        }
        let rule = Line::from(Span::styled(
            "▔".repeat(usize::from(area.width)),
            Style::default().fg(accent_color()),
        ));
        let width = usize::from(area.width.saturating_sub(4)).max(10);
        let hint = self.hint(view);
        // Rows left for the body once the rule and the guide are drawn.
        let chrome = if hint.is_some() { 3 } else { 1 };
        let budget = usize::from(area.height).saturating_sub(chrome);
        let mut lines = vec![rule];
        lines.extend(
            self.body_lines(view, width, budget)
                .into_iter()
                .take(budget)
                .map(|line| indent(line, 3)),
        );
        if let Some(hint) = hint {
            lines.push(Line::from(""));
            lines.push(indent(
                Line::from(Span::styled(
                    hint,
                    Style::default()
                        .fg(dim_color())
                        .add_modifier(Modifier::ITALIC),
                )),
                3,
            ));
        }
        frame.render_widget(Clear, area);
        frame.render_widget(Paragraph::new(lines), area);
    }

    /// Claude Code's key guides, per view. `/skills` has none: its hints
    /// are in the subtitle.
    fn hint(&self, view: &View) -> Option<String> {
        if matches!(view.route, Route::Plugin(_)) {
            return Some(
                "←/→ tabs · Type to search · ↑/↓ navigate · Enter select · Esc back".into(),
            );
        }
        if view.route == Route::PluginAddMarketplace {
            return Some("Type a marketplace source · Enter to add · Esc to cancel".into());
        }
        if view.route == Route::Skills {
            return None;
        }
        if view.on_escape.is_some() {
            return Some("Enter to confirm · Esc to cancel".to_string());
        }
        Some(
            match view.route {
                Route::McpList => "↑/↓ to navigate · Enter to confirm · Esc to cancel",
                Route::McpTool(..) => "Esc to go back",
                _ if view.items.is_empty() => "Esc to go back",
                _ => "↑/↓ to navigate · Enter to select · Esc to back",
            }
            .to_string(),
        )
    }

    /// The dialog body (title through notes), fitted to `budget` rows: a long
    /// list shows a window around the selection with `↑ N more` / `↓ N more`
    /// markers, like Claude Code.
    fn body_lines(&self, view: &View, width: usize, budget: usize) -> Vec<Line<'static>> {
        let dim = Style::default().fg(dim_color());
        let bold = Style::default().add_modifier(Modifier::BOLD);
        let visible = view.visible();
        let mut head: Vec<Line<'static>> = Vec::new();

        // Title and subtitle.
        let title_style = Style::default()
            .fg(accent_color())
            .add_modifier(Modifier::BOLD);
        let mut title_spans = vec![Span::styled(view.title.clone(), title_style)];
        for tag in &view.title_tags {
            title_spans.push(Span::raw(" "));
            title_spans.push(tag.clone());
        }
        head.push(Line::from(title_spans));
        let mut subtitle = view.subtitle.clone();
        if view.route == Route::Skills
            && let Some(skills) = self.skills.as_ref()
        {
            let total = skills.rows.len();
            let plural = if total == 1 { "" } else { "s" };
            subtitle = if view.filter.is_empty() {
                format!("{total} skill{plural}")
            } else {
                format!("{}/{total} skill{plural}", visible.len())
            };
            if skills.sort_by_tokens {
                subtitle.push_str(" · sorted by tokens");
            }
            subtitle.push_str(if visible.is_empty() {
                " · / to search, Esc to close"
            } else {
                " · enter/space to cycle, / to search, t to sort, Esc to close"
            });
        }
        if !subtitle.is_empty() {
            head.push(Line::from(Span::styled(subtitle, dim)));
        }
        if let Some(status) = &self.status {
            for chunk in wrap(status, width) {
                head.push(Line::from(Span::styled(
                    chunk,
                    Style::default().fg(Tone::Warning.color()),
                )));
            }
        }
        if view.searchable {
            let border = Style::default().add_modifier(Modifier::DIM);
            let inner = width.saturating_sub(4).max(4);
            let (text, style) = if view.filter.is_empty() {
                let placeholder = if matches!(view.route, Route::RewindList) {
                    "⌕ Search messages…"
                } else {
                    "⌕ Search skills…"
                };
                (placeholder.to_string(), dim)
            } else {
                (format!("⌕ {}", view.filter), Style::default())
            };
            let pad = inner.saturating_sub(text.width());
            head.push(Line::from(Span::styled(
                format!("╭{}╮", "─".repeat(inner + 2)),
                border,
            )));
            head.push(Line::from(vec![
                Span::styled("│ ", border),
                Span::styled(text, style),
                Span::raw(" ".repeat(pad)),
                Span::styled(" │", border),
            ]));
            head.push(Line::from(Span::styled(
                format!("╰{}╯", "─".repeat(inner + 2)),
                border,
            )));
        }

        // Info rows: values aligned in one column (Claude Code pads labels to
        // the widest one plus two spaces). `inline_info` rows follow, unpadded.
        let mut detail: Vec<Line<'static>> = Vec::new();
        let info_width = view
            .info
            .iter()
            .map(|(label, _)| label.width() + 1)
            .max()
            .unwrap_or(0)
            + 2;
        for (label, value) in &view.info {
            let prefix = format!("{label}:");
            let prefix = format!("{prefix:<info_width$}");
            let value_width = width.saturating_sub(prefix.width()).max(10);
            for (index, chunk) in wrap(value, value_width).into_iter().enumerate() {
                let head_span = if index == 0 {
                    Span::styled(prefix.clone(), bold)
                } else {
                    Span::raw(" ".repeat(prefix.width()))
                };
                let mut spans = vec![head_span];
                match tone_prefix(&chunk).filter(|_| index == 0) {
                    Some(tone) => {
                        let glyph = tone.glyph();
                        spans.push(Span::styled(
                            format!("{glyph} "),
                            Style::default().fg(tone.color()),
                        ));
                        spans.push(Span::raw(chunk[glyph.len()..].trim_start().to_string()));
                    }
                    None if label == "Status" => spans.push(Span::raw(chunk)),
                    None => spans.push(Span::styled(chunk, dim)),
                }
                detail.push(Line::from(spans));
            }
        }
        for (label, value) in &view.inline_info {
            detail.push(Line::from(vec![
                Span::styled(format!("{label}: "), bold),
                Span::styled(
                    value.clone(),
                    if label == "Capabilities" {
                        Style::default()
                    } else {
                        dim
                    },
                ),
            ]));
        }
        for paragraph in &view.body {
            if let Some(heading) = paragraph.strip_prefix("## ") {
                if !detail.is_empty() {
                    detail.push(Line::from(""));
                }
                detail.push(Line::from(Span::styled(heading.to_string(), bold)));
                continue;
            }
            if let Some(param) = paragraph.strip_prefix("  ● ") {
                detail.extend(param_lines(param, width));
                continue;
            }
            for chunk in wrap(paragraph, width) {
                detail.push(Line::from(Span::raw(chunk)));
            }
        }

        let mut footer: Vec<Line<'static>> = Vec::new();
        for note in &view.notes {
            for chunk in wrap(note, width) {
                footer.push(Line::from(Span::styled(chunk, dim)));
            }
        }

        // List rows (group headings, items, spacers), with the line index of
        // each item so the window can follow the selection.
        let mut rows: Vec<Line<'static>> = Vec::new();
        let mut item_rows: Vec<usize> = Vec::new();
        if view.items.is_empty() && !view.empty.is_empty() && self.status.is_none() {
            for chunk in wrap(&view.empty, width) {
                rows.push(Line::from(Span::styled(chunk, dim)));
            }
        } else if visible.is_empty() && !view.filter.is_empty() {
            rows.push(Line::from(Span::styled(
                format!("No matches for \"{}\"", view.filter),
                dim,
            )));
        }
        // Claude Code's row grid: the name column is as wide as the longest
        // name among rows that have facts (capped at 50), across the whole
        // list, then a 3-space gap and the facts, dim.
        let label_width = view
            .items
            .iter()
            .filter(|item| item.meta.iter().any(|m| !m.is_empty()))
            .map(|item| item.label.width())
            .max()
            .unwrap_or(0)
            .min(50)
            .min(width / 2)
            + if view.numbered { 3 } else { 0 };
        let mut last_group: Option<&str> = None;
        for (position, &index) in visible.iter().enumerate() {
            let item = &view.items[index];
            if let Some(group) = item.group.as_deref()
                && last_group != Some(group)
            {
                if last_group.is_some() {
                    rows.push(Line::from(""));
                }
                let mut spans = vec![Span::raw("  ")];
                match group.split_once(" (") {
                    Some((label, path)) => {
                        spans.push(Span::styled(label.to_string(), bold));
                        spans.push(Span::styled(format!(" ({path}"), dim));
                    }
                    None => spans.push(Span::styled(group.to_string(), bold)),
                }
                rows.push(Line::from(spans));
                last_group = Some(group);
            }
            let is_selected = position == view.selected;
            item_rows.push(rows.len());
            let accent = Style::default().fg(accent_color());
            let mut spans = vec![Span::styled(if is_selected { "❯ " } else { "  " }, accent)];
            if let Some(tone) = item.icon {
                spans.push(Span::styled(
                    format!("{} ", tone.glyph()),
                    Style::default().fg(tone.color()),
                ));
            }
            let mut label_used = 0;
            if view.numbered {
                let number = format!("{}. ", position + 1);
                label_used += number.width();
                spans.push(Span::styled(number, dim));
            }
            label_used += item.label.width();
            spans.push(Span::styled(
                item.label.clone(),
                if is_selected {
                    accent
                } else {
                    Style::default()
                },
            ));
            let meta: Vec<&str> = item
                .meta
                .iter()
                .map(String::as_str)
                .filter(|m| !m.is_empty())
                .collect();
            if !meta.is_empty() {
                let pad = label_width.saturating_sub(label_used) + 3;
                spans.push(Span::raw(" ".repeat(pad)));
                spans.push(Span::styled(meta.join(" · "), dim));
            }
            rows.push(Line::from(spans));
        }

        // Assemble with Claude Code's spacing: a blank line between the
        // header, the details, the list and the notes.
        let mut out = head;
        let has_detail = !detail.is_empty();
        if has_detail {
            out.push(Line::from(""));
            out.extend(detail);
        }
        let fixed_after = if footer.is_empty() {
            0
        } else {
            footer.len() + 1
        };
        if !rows.is_empty() {
            // Claude Code's search box sits right above its list.
            if !view.searchable || has_detail {
                out.push(Line::from(""));
            }
            let room = budget.saturating_sub(out.len() + fixed_after);
            let selected_row = item_rows.get(view.selected).copied().unwrap_or(0);
            out.extend(window(rows, selected_row, room, dim));
        }
        if !footer.is_empty() {
            out.push(Line::from(""));
            out.extend(footer);
        }
        out
    }
}

/// Prefix a line with `n` spaces.
fn indent(mut line: Line<'static>, n: usize) -> Line<'static> {
    line.spans.insert(0, Span::raw(" ".repeat(n)));
    line
}

/// `room` rows of `rows` around `selected`, with `↑ N more` / `↓ N more`
/// markers counting hidden rows, like Claude Code's scrolling lists.
fn window(
    rows: Vec<Line<'static>>,
    selected: usize,
    room: usize,
    dim: Style,
) -> Vec<Line<'static>> {
    let total = rows.len();
    if total <= room || room < 3 {
        return rows.into_iter().take(room.max(total.min(room))).collect();
    }
    // Two marker rows may be needed; keep the selection inside the window.
    let inner = room.saturating_sub(2).max(1);
    let start = selected.saturating_sub(inner / 2).min(total - inner);
    let mut start = start;
    let mut end = (start + inner).min(total);
    // Without a top marker the window can use that row, same at the bottom.
    if start == 0 {
        end = (room - 1).min(total);
    } else if end == total {
        start = total.saturating_sub(room - 1);
    }
    let mut out = Vec::with_capacity(room);
    if start > 0 {
        out.push(Line::from(Span::styled(format!("  ↑ {start} more"), dim)));
    }
    out.extend(rows.into_iter().skip(start).take(end - start));
    if end < total {
        out.push(Line::from(Span::styled(
            format!("  ↓ {} more", total - end),
            dim,
        )));
    }
    out
}

/// One parameter of the tool page: `● name (required): type - description`,
/// with the name plain and the rest dim, wrapped under the bullet.
fn param_lines(param: &str, width: usize) -> Vec<Line<'static>> {
    let dim = Style::default().fg(dim_color());
    let (name, rest) = param.split_once('\u{1f}').unwrap_or((param, ""));
    let (required, rest) = match rest.strip_prefix("required\u{1f}") {
        Some(rest) => (true, rest),
        None => (false, rest),
    };
    let lead = format!("  ● {name}");
    let req = if required { " (required)" } else { "" };
    let first_width = width.saturating_sub(lead.width() + req.width() + 2).max(10);
    let wrapped = wrap(rest, first_width);
    let mut lines = Vec::new();
    for (index, chunk) in wrapped.into_iter().enumerate() {
        if index == 0 {
            lines.push(Line::from(vec![
                Span::raw(lead.clone()),
                Span::styled(req.to_string(), dim),
                Span::raw(": "),
                Span::styled(chunk, dim),
            ]));
        } else {
            lines.push(Line::from(Span::styled(format!("    {chunk}"), dim)));
        }
    }
    lines
}

fn skill_tokens(row: &SkillRow) -> usize {
    crate::util::estimate_tokens(&format!("{} {}", row.name, row.description))
}

/// Claude Code's listing-cost label: `< 20 tok` or `~N tok` to the nearest 10.
fn format_tokens(tokens: usize) -> String {
    if tokens < 20 {
        "< 20 tok".to_string()
    } else {
        format!("~{} tok", (tokens + 5) / 10 * 10)
    }
}

fn truncate_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Greedy word wrap by display width; overlong words are split.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut out = Vec::new();
    for raw in text.split('\n') {
        let indent: String = raw.chars().take_while(|c| *c == ' ').collect();
        let mut line = String::new();
        for word in raw.split_whitespace() {
            let needed = if line.is_empty() {
                indent.width()
            } else {
                line.width() + 1
            };
            if !line.is_empty() && needed + word.width() > width {
                out.push(std::mem::take(&mut line));
            }
            if line.is_empty() {
                line.push_str(&indent);
            } else {
                line.push(' ');
            }
            for ch in word.chars() {
                if line.width() + ch.width().unwrap_or(0) > width && !line.trim().is_empty() {
                    out.push(std::mem::take(&mut line));
                }
                line.push(ch);
            }
        }
        out.push(line);
    }
    out
}

#[cfg(test)]
#[path = "dialog_tests.rs"]
mod tests;
