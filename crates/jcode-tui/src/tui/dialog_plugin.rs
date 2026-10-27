//! Native plugin lifecycle UI. Disk/network work never runs on the input thread.
//! Tab names follow Claude 2.1.310 shipped-source REA evidence
//! ev_01dddda9cb5d5fd60df3fe79003b6d5c81575993d8c64119ac4329d73cad84f8.

#[cfg(test)]
#[path = "dialog_plugin_tests.rs"]
mod tests;
use super::*;
use crate::plugin::{PluginManager, PluginScope};
use crate::tui::plugin_command::PluginCommand;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

#[derive(Default, Clone)]
struct Snapshot {
    installed: Vec<Value>,
    available: Vec<Value>,
    marketplaces: Vec<Value>,
    errors: Vec<String>,
    busy: bool,
    message: String,
}

pub(super) struct PluginState {
    directory: PathBuf,
    snapshot: Arc<Mutex<Snapshot>>,
}

impl PluginState {
    pub(super) fn start(&self, args: Vec<String>) {
        {
            let mut state = self.snapshot.lock().unwrap_or_else(|p| p.into_inner());
            if state.busy {
                return;
            }
            state.busy = true;
            state.message = if args.is_empty() {
                "Loading plugins…".into()
            } else {
                format!("Running {}…", args.join(" "))
            };
        }
        let snapshot = self.snapshot.clone();
        let directory = self.directory.clone();
        std::thread::spawn(move || {
            let mut next = Snapshot::default();
            match PluginManager::for_dir(&directory) {
                Err(error) => next.errors.push(format!("{error:#}")),
                Ok(manager) => {
                    if !args.is_empty() {
                        match execute(&manager, &args) {
                            Ok(()) => {
                                next.message = format!(
                                    "Completed {}. Plugin state saved. Contributions refresh before the next prompt.",
                                    args.join(" ")
                                )
                            }
                            Err(error) => next.errors.push(format!("{error:#}")),
                        }
                    }
                    next.installed = collect(manager.list(), &mut next.errors, "Installed plugins");
                    next.available = collect(manager.discover(), &mut next.errors, "Discovery");
                    next.marketplaces = collect(
                        manager.list_marketplaces(),
                        &mut next.errors,
                        "Marketplaces",
                    );
                    match manager.dependency_diagnostics() {
                        Ok(errors) => next.errors.extend(errors),
                        Err(error) => next.errors.push(format!("Dependencies: {error:#}")),
                    }
                }
            }
            *snapshot.lock().unwrap_or_else(|p| p.into_inner()) = next;
        });
    }
}

fn collect<T: serde::Serialize>(
    result: anyhow::Result<Vec<T>>,
    errors: &mut Vec<String>,
    label: &str,
) -> Vec<Value> {
    match result.and_then(|rows| {
        rows.into_iter()
            .map(|row| serde_json::to_value(row).map_err(Into::into))
            .collect()
    }) {
        Ok(rows) => rows,
        Err(error) => {
            errors.push(format!("{label}: {error:#}"));
            Vec::new()
        }
    }
}

fn execute(manager: &PluginManager, args: &[String]) -> anyhow::Result<()> {
    let operation = args.first().map(String::as_str).unwrap_or("");
    if operation == "marketplace" {
        let target = args.get(2).map(String::as_str).unwrap_or("");
        match args.get(1).map(String::as_str) {
            Some("list") => {}
            Some("add") => {
                manager.add_marketplace(target)?;
            }
            Some("remove") => manager.remove_marketplace(target)?,
            Some("update") => {
                manager.update_marketplace(target)?;
            }
            _ => anyhow::bail!("{}", crate::tui::plugin_command::USAGE),
        }
        return Ok(());
    }
    let target = args
        .get(1)
        .ok_or_else(|| anyhow::anyhow!("Missing plugin name"))?;
    let scope: PluginScope = args.get(3).map(String::as_str).unwrap_or("user").parse()?;
    match operation {
        "install" => {
            manager.install(target, scope)?;
        }
        "enable" => manager.enable(target, scope)?,
        "disable" => manager.disable(target, scope)?,
        "uninstall" => manager.uninstall(target, scope)?,
        "update" => {
            manager.update(target, scope)?;
        }
        _ => anyhow::bail!("{}", crate::tui::plugin_command::USAGE),
    }
    Ok(())
}

fn text<'a>(row: &'a Value, field: &str) -> &'a str {
    row[field].as_str().unwrap_or("")
}
fn plugin_spec(row: &Value) -> String {
    let name = text(row, "name");
    let marketplace = text(row, "marketplace");
    if marketplace.is_empty() {
        name.into()
    } else {
        format!("{name}@{marketplace}")
    }
}
fn item(label: impl Into<String>, action: Option<ItemAction>) -> Item {
    Item {
        group: None,
        icon: None,
        label: label.into(),
        meta: Vec::new(),
        search_text: String::new(),
        action,
    }
}
fn operation(label: &str, args: Vec<String>, busy: bool, confirm: bool) -> Item {
    item(
        label,
        (!busy).then(|| {
            if confirm {
                ItemAction::Open(Route::PluginConfirm(args))
            } else {
                ItemAction::Plugin(args)
            }
        }),
    )
}

impl Dialog {
    pub(super) fn handle_plugin_key(&mut self, code: KeyCode) -> bool {
        let Some(view) = self.stack.last() else {
            return false;
        };
        if let Route::Plugin(tab) = &view.route {
            if matches!(code, KeyCode::Left | KeyCode::Right) {
                let tabs = ["discover", "installed", "marketplaces", "errors"];
                let index = tabs.iter().position(|key| key == tab).unwrap_or(0);
                let index = (index + if code == KeyCode::Left { 3 } else { 1 }) % 4;
                self.stack = vec![self.plugin_view(&Route::Plugin(tabs[index].into()))];
                return true;
            }
        }
        if view.route == Route::PluginAddMarketplace && code == KeyCode::Enter {
            let source = view.filter.trim().to_string();
            if !source.is_empty() {
                self.run(Some(ItemAction::Plugin(vec![
                    "marketplace".into(),
                    "add".into(),
                    source,
                ])));
            }
            return true;
        }
        false
    }

    pub(crate) fn plugins(directory: PathBuf, command: PluginCommand) -> Self {
        let (tab, args) = match command {
            PluginCommand::Browse(tab) => (tab, Vec::new()),
            PluginCommand::Run(args) => (
                if args.first().is_some_and(|s| s == "marketplace") {
                    "marketplaces"
                } else {
                    "installed"
                }
                .into(),
                args,
            ),
        };
        let plugin = PluginState {
            directory,
            snapshot: Arc::new(Mutex::new(Snapshot::default())),
        };
        plugin.start(args);
        let mut dialog = Self {
            stack: Vec::new(),
            mcp: None,
            skills: None,
            plugin: Some(plugin),
            rewind: None,
            pending_rewind: false,
            status: None,
            pending_mcp: None,
        };
        dialog.stack.push(dialog.plugin_view(&Route::Plugin(tab)));
        dialog
    }

    pub(super) fn live_plugin_view(&self) -> Option<View> {
        self.plugin.as_ref()?;
        let old = self.stack.last()?;
        let mut fresh = self.plugin_view(&old.route);
        fresh.filter = old.filter.clone();
        if old.route == Route::PluginAddMarketplace {
            fresh.body.push(format!("Source: {}", old.filter));
        }
        fresh.selected = old.selected.min(fresh.visible().len().saturating_sub(1));
        Some(fresh)
    }

    pub(super) fn plugin_view(&self, route: &Route) -> View {
        let mut view = View::new(route.clone(), "Plugins");
        let Some(plugin) = &self.plugin else {
            return view;
        };
        let state = plugin
            .snapshot
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        let active = match route {
            Route::Plugin(tab) | Route::PluginDetail(tab, ..) => tab.as_str(),
            Route::PluginAddMarketplace => "marketplaces",
            _ => "",
        };
        view.subtitle = [
            ("discover", "Discover"),
            ("installed", "Installed"),
            ("marketplaces", "Marketplaces"),
            ("errors", "Errors"),
        ]
        .into_iter()
        .map(|(key, label)| {
            let label = if key == "errors" {
                format!("Errors ({})", state.errors.len())
            } else {
                label.into()
            };
            if active == key {
                format!("[{label}]")
            } else {
                label
            }
        })
        .collect::<Vec<_>>()
        .join(" · ");
        if !state.message.is_empty() {
            view.body.push(state.message.clone());
        }
        if !state.errors.is_empty() && !matches!(route, Route::Plugin(tab) if tab == "errors") {
            view.body.push(format!(
                "{} error(s). Open Errors for details.",
                state.errors.len()
            ));
        }
        view.notes.push(
            "Plugin language servers require installed executables. Stdio only; queried on demand."
                .into(),
        );
        if state.marketplaces.is_empty() && !state.busy {
            view.notes
                .push("Suggested official source: anthropics/claude-plugins-official".into());
        }
        match route {
            Route::PluginAddMarketplace => {
                view.title = "Add marketplace".into();
                view.searchable = true;
                view.body
                    .push("Enter a Git URL, owner/repository, or local marketplace path.".into());
            }
            Route::Plugin(tab) => {
                view.items
                    .push(operation("Refresh", Vec::new(), state.busy, false));
                if state.marketplaces.is_empty()
                    && matches!(tab.as_str(), "discover" | "marketplaces")
                {
                    view.items.push(operation(
                        "Add official marketplace",
                        vec![
                            "marketplace".into(),
                            "add".into(),
                            "anthropics/claude-plugins-official".into(),
                        ],
                        state.busy,
                        false,
                    ));
                    if tab == "discover" {
                        view.items.push(item(
                            "Add another marketplace…",
                            Some(ItemAction::Open(Route::PluginAddMarketplace)),
                        ));
                    }
                }
                view.searchable = true;
                match tab.as_str() {
                    "errors" => {
                        view.body.extend(state.errors.iter().cloned());
                        if state.errors.is_empty() && !state.busy {
                            view.body.push("No plugin registry errors reported.".into());
                        }
                    }
                    "marketplaces" => {
                        view.items.push(item(
                            "Add marketplace",
                            Some(ItemAction::Open(Route::PluginAddMarketplace)),
                        ));
                        view.notes
                            .push("Add a source: /plugin marketplace add <source>".into());
                        for row in &state.marketplaces {
                            let name = text(row, "name");
                            view.items.push(item(
                                name,
                                Some(ItemAction::Open(Route::PluginDetail(
                                    "marketplaces".into(),
                                    name.into(),
                                    String::new(),
                                ))),
                            ));
                        }
                        if state.marketplaces.is_empty() && !state.busy {
                            view.body.push("No marketplaces configured.".into());
                        }
                    }
                    tab => {
                        let rows = if tab == "installed" {
                            &state.installed
                        } else {
                            &state.available
                        };
                        for row in rows {
                            let spec = plugin_spec(row);
                            let mut entry = item(
                                spec.clone(),
                                Some(ItemAction::Open(Route::PluginDetail(
                                    tab.into(),
                                    spec,
                                    text(row, "scope").into(),
                                ))),
                            );
                            entry.meta.push(text(row, "description").into());
                            if tab == "installed" {
                                entry.meta.push(text(row, "scope").into());
                                entry.meta.push(
                                    if row["enabled"].as_bool().unwrap_or(false) {
                                        "enabled"
                                    } else {
                                        "disabled"
                                    }
                                    .into(),
                                );
                            }
                            view.items.push(entry);
                        }
                        if rows.is_empty() && !state.busy {
                            view.body.push(
                                if tab == "installed" {
                                    "No installed plugins."
                                } else {
                                    "No plugins discovered. Add a marketplace first."
                                }
                                .into(),
                            );
                        }
                    }
                }
            }
            Route::PluginDetail(tab, spec, selected_scope) => {
                view.title = spec.clone();
                if tab == "marketplaces" {
                    view.items.push(operation(
                        "Update marketplace",
                        vec!["marketplace".into(), "update".into(), spec.clone()],
                        state.busy,
                        false,
                    ));
                    view.items.push(operation(
                        "Remove marketplace",
                        vec!["marketplace".into(), "remove".into(), spec.clone()],
                        state.busy,
                        true,
                    ));
                } else if tab == "discover" {
                    for scope in ["user", "project", "local"] {
                        view.items.push(operation(
                            &format!("Install ({scope})"),
                            vec![
                                "install".into(),
                                spec.clone(),
                                "--scope".into(),
                                scope.into(),
                            ],
                            state.busy,
                            false,
                        ));
                    }
                } else if let Some(row) = state
                    .installed
                    .iter()
                    .find(|row| plugin_spec(row) == *spec && text(row, "scope") == selected_scope)
                {
                    let scope = text(row, "scope");
                    let scope = if scope.is_empty() { "user" } else { scope };
                    view.info.push(("Scope".into(), scope.into()));
                    let toggle = if row["enabled"].as_bool().unwrap_or(false) {
                        "disable"
                    } else {
                        "enable"
                    };
                    for action in [toggle, "update", "uninstall"] {
                        view.items.push(operation(
                            action,
                            vec![action.into(), spec.clone(), "--scope".into(), scope.into()],
                            state.busy,
                            action == "uninstall",
                        ));
                    }
                }
            }
            Route::PluginConfirm(args) => {
                view.title = "Confirm plugin operation".into();
                view.body.push(args.join(" "));
                view.items.push(item("Cancel", Some(ItemAction::Back)));
                view.items
                    .push(operation("Confirm", args.clone(), state.busy, false));
                view.on_escape = Some(ItemAction::Back);
            }
            _ => {}
        }
        view
    }
}
