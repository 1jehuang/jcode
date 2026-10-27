use super::*;
use serde_json::json;

fn dialog(state: Snapshot, tab: &str) -> Dialog {
    let plugin = PluginState {
        directory: PathBuf::from("/unused-test-project"),
        snapshot: Arc::new(Mutex::new(state)),
    };
    let mut dialog = Dialog {
        stack: Vec::new(),
        mcp: None,
        skills: None,
        plugin: Some(plugin),
        rewind: None,
        pending_rewind: false,
        status: None,
        pending_mcp: None,
    };
    dialog
        .stack
        .push(dialog.plugin_view(&Route::Plugin(tab.into())));
    dialog
}

#[test]
fn plugin_tabs_and_search_are_keyboard_accessible() {
    let mut dialog = dialog(
        Snapshot {
            available: vec![
                json!({"name":"lint", "marketplace":"tools", "description":"Check source"}),
            ],
            ..Snapshot::default()
        },
        "discover",
    );
    assert!(!dialog.is_mcp());
    assert!(dialog.finish().is_none());
    assert!(
        dialog
            .item_summaries()
            .iter()
            .any(|s| s.contains("lint@tools"))
    );
    dialog.handle_key(KeyCode::Char('l'), KeyModifiers::empty());
    let view = dialog.stack.last().unwrap();
    assert!(
        view.visible()
            .iter()
            .any(|&i| view.items[i].label == "lint@tools")
    );
    dialog.handle_key(KeyCode::Right, KeyModifiers::empty());
    assert_eq!(
        dialog.stack.last().unwrap().route,
        Route::Plugin("installed".into())
    );
    dialog.handle_key(KeyCode::Left, KeyModifiers::empty());
    assert_eq!(
        dialog.stack.last().unwrap().route,
        Route::Plugin("discover".into())
    );
    assert_eq!(
        dialog.stack.len(),
        1,
        "tab changes must not accumulate back-stack entries"
    );
}

#[test]
fn installed_actions_keep_scope_and_require_uninstall_confirmation() {
    let dialog = dialog(
        Snapshot {
            installed: vec![
                json!({"name":"lint", "marketplace":"tools", "scope":"user", "enabled":true}),
                json!({"name":"lint", "marketplace":"tools", "scope":"project", "enabled":false}),
            ],
            ..Snapshot::default()
        },
        "installed",
    );
    let view = dialog.plugin_view(&Route::PluginDetail(
        "installed".into(),
        "lint@tools".into(),
        "project".into(),
    ));
    assert!(
        matches!(&view.items[0].action, Some(ItemAction::Plugin(args)) if args == &["enable", "lint@tools", "--scope", "project"])
    );
    assert!(
        matches!(&view.items[2].action, Some(ItemAction::Open(Route::PluginConfirm(args))) if args == &["uninstall", "lint@tools", "--scope", "project"])
    );
    let confirm = dialog.plugin_view(&Route::PluginConfirm(vec![
        "uninstall".into(),
        "lint@tools".into(),
    ]));
    assert_eq!(
        confirm.items[0].label, "Cancel",
        "destructive default must be safe"
    );
}

#[test]
fn busy_plugins_disable_mutations_and_errors_remain_visible() {
    let dialog = dialog(
        Snapshot {
            busy: true,
            errors: vec!["Registry is corrupt".into()],
            available: vec![json!({"name":"lint", "marketplace":"tools"})],
            ..Snapshot::default()
        },
        "discover",
    );
    let detail = dialog.plugin_view(&Route::PluginDetail(
        "discover".into(),
        "lint@tools".into(),
        String::new(),
    ));
    assert!(detail.items.iter().all(|i| i.action.is_none()));
    let errors = dialog.plugin_view(&Route::Plugin("errors".into()));
    assert!(
        errors
            .body
            .iter()
            .any(|s| s.contains("Registry is corrupt"))
    );
    assert!(
        !errors
            .body
            .iter()
            .any(|s| s.contains("No plugin registry errors"))
    );
    assert!(
        errors
            .notes
            .iter()
            .any(|s| s.contains("installed executables") && s.contains("Stdio only"))
    );
}

#[test]
fn live_snapshot_is_rendered_without_waiting_for_a_key() {
    let dialog = dialog(Snapshot::default(), "discover");
    dialog
        .plugin
        .as_ref()
        .unwrap()
        .snapshot
        .lock()
        .unwrap()
        .available
        .push(json!({"name":"fresh", "marketplace":"tools"}));
    let view = dialog.live_plugin_view().unwrap();
    assert!(view.items.iter().any(|i| i.label == "fresh@tools"));
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(110, 30)).unwrap();
    terminal
        .draw(|frame| dialog.render(frame, frame.area()))
        .unwrap();
    let buffer = terminal.backend().buffer();
    let rendered: String = (0..30)
        .flat_map(|y| (0..110).map(move |x| buffer[(x, y)].symbol().to_string()))
        .collect();
    assert!(rendered.contains("fresh@tools"));
}

#[test]
fn marketplace_input_preserves_source_text_and_escape_returns() {
    let mut dialog = dialog(Snapshot::default(), "marketplaces");
    dialog.run(Some(ItemAction::Open(Route::PluginAddMarketplace)));
    for ch in "/local/marketplace".chars() {
        dialog.handle_key(KeyCode::Char(ch), KeyModifiers::empty());
    }
    let view = dialog.live_plugin_view().unwrap();
    assert_eq!(view.filter, "/local/marketplace");
    assert!(view.body.iter().any(|s| s == "Source: /local/marketplace"));
    dialog.handle_key(KeyCode::Esc, KeyModifiers::empty());
    assert!(dialog.stack.last().unwrap().filter.is_empty());
    dialog.handle_key(KeyCode::Esc, KeyModifiers::empty());
    assert_eq!(
        dialog.stack.last().unwrap().route,
        Route::Plugin("marketplaces".into())
    );
}

#[test]
fn empty_discovery_offers_explicit_marketplace_actions_without_starting_io() {
    let dialog = dialog(Snapshot::default(), "discover");
    let view = dialog.live_plugin_view().unwrap();
    assert!(view.subtitle.contains("[Discover]"));
    assert!(view.subtitle.contains("Errors (0)"));
    assert!(view.items.iter().any(|item| matches!(&item.action, Some(ItemAction::Plugin(args)) if args == &["marketplace", "add", "anthropics/claude-plugins-official"])));
    assert!(view.items.iter().any(|item| matches!(
        item.action,
        Some(ItemAction::Open(Route::PluginAddMarketplace))
    )));
    assert!(
        !dialog
            .plugin
            .as_ref()
            .unwrap()
            .snapshot
            .lock()
            .unwrap()
            .busy,
        "building and rendering must never fetch a marketplace"
    );
}

#[test]
fn background_loading_reports_errors_without_writing_or_blocking_input() {
    const FLAG: &str = "JCODE_PLUGIN_UI_WORKER_TEST";
    if std::env::var_os(FLAG).is_none() {
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(home.path().join("plugins")).unwrap();
        std::fs::write(home.path().join("plugins/registry.json"), "broken registry").unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .arg("background_loading_reports_errors_without_writing_or_blocking_input")
            .arg("--test-threads=1")
            .env(FLAG, "1")
            .env("JCODE_HOME", home.path())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            std::fs::read_to_string(home.path().join("plugins/registry.json")).unwrap(),
            "broken registry"
        );
        return;
    }
    let mut dialog = Dialog::plugins(
        std::env::current_dir().unwrap(),
        PluginCommand::Browse("discover".into()),
    );
    dialog.handle_key(KeyCode::Right, KeyModifiers::empty());
    assert_eq!(
        dialog.stack.last().unwrap().route,
        Route::Plugin("installed".into())
    );
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let snapshot = dialog
            .plugin
            .as_ref()
            .unwrap()
            .snapshot
            .lock()
            .unwrap()
            .clone();
        if !snapshot.busy {
            assert!(!snapshot.errors.is_empty());
            assert!(
                !dialog
                    .plugin_view(&Route::Plugin("errors".into()))
                    .body
                    .is_empty()
            );
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "plugin worker did not finish"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

#[test]
fn slash_operations_execute_against_real_scoped_registry() {
    let root = tempfile::tempdir().unwrap();
    let marketplace = root.path().join("market");
    std::fs::create_dir_all(marketplace.join(".claude-plugin")).unwrap();
    std::fs::create_dir_all(marketplace.join("demo/.claude-plugin")).unwrap();
    std::fs::write(
        marketplace.join(".claude-plugin/marketplace.json"),
        r#"{"name":"fixture","plugins":[{"name":"demo","source":"./demo"}]}"#,
    )
    .unwrap();
    std::fs::write(
        marketplace.join("demo/.claude-plugin/plugin.json"),
        r#"{"name":"demo","version":"1.0.0"}"#,
    )
    .unwrap();
    let manager = PluginManager::new(root.path().join("home"), root.path().join("project"));
    let command = |args: &[&str]| {
        execute(
            &manager,
            &args.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
        )
        .unwrap()
    };
    command(&["marketplace", "add", marketplace.to_str().unwrap()]);
    assert_eq!(manager.discover().unwrap().len(), 1);
    command(&["install", "demo@fixture", "--scope", "project"]);
    assert_eq!(manager.list().unwrap()[0].scope, PluginScope::Project);
    command(&["disable", "demo@fixture", "--scope", "project"]);
    assert!(!manager.list().unwrap()[0].enabled);
    command(&["enable", "demo@fixture", "--scope", "project"]);
    assert!(manager.list().unwrap()[0].enabled);
    command(&["marketplace", "update", "fixture"]);
    command(&["update", "demo@fixture", "--scope", "project"]);
    command(&["uninstall", "demo@fixture", "--scope", "project"]);
    assert!(manager.list().unwrap().is_empty());
    command(&["marketplace", "remove", "fixture"]);
    assert!(manager.list_marketplaces().unwrap().is_empty());
}

#[test]
fn dependency_errors_appear_in_background_plugin_snapshot() {
    const FLAG: &str = "JCODE_PLUGIN_DEPENDENCY_UI_TEST";
    if std::env::var_os(FLAG).is_none() {
        let root = tempfile::tempdir().unwrap();
        let market = root.path().join("market");
        let project = root.path().join("project");
        let home = root.path().join("home");
        std::fs::create_dir_all(&project).unwrap();
        let write = |path: PathBuf, value: Value| {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
        };
        write(
            market.join(".claude-plugin/marketplace.json"),
            json!({
                "name":"dependency-ui", "plugins":[
                    {"name":"parent", "source":"./parent"},
                    {"name":"child", "source":"./child"}
                ]
            }),
        );
        write(
            market.join("parent/.claude-plugin/plugin.json"),
            json!({"name":"parent", "dependencies":["child"]}),
        );
        write(
            market.join("child/.claude-plugin/plugin.json"),
            json!({"name":"child"}),
        );
        let manager = PluginManager::new(home.clone(), project.clone());
        manager.add_marketplace(market.to_str().unwrap()).unwrap();
        manager
            .install("parent@dependency-ui", PluginScope::User)
            .unwrap();
        manager
            .disable("child@dependency-ui", PluginScope::User)
            .unwrap();
        let registry = home.join("plugins/registry.json");
        let before = std::fs::read(&registry).unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .arg("dependency_errors_appear_in_background_plugin_snapshot")
            .arg("--test-threads=1")
            .env(FLAG, "1")
            .env("JCODE_HOME", &home)
            .env("JCODE_DEPENDENCY_UI_PROJECT", &project)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            std::fs::read(registry).unwrap(),
            before,
            "listing dependency errors must not change installed state"
        );
        return;
    }
    let dialog = Dialog::plugins(
        PathBuf::from(std::env::var_os("JCODE_DEPENDENCY_UI_PROJECT").unwrap()),
        PluginCommand::Browse("errors".into()),
    );
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let state = dialog
            .plugin
            .as_ref()
            .unwrap()
            .snapshot
            .lock()
            .unwrap()
            .clone();
        if !state.busy {
            assert!(
                state
                    .errors
                    .iter()
                    .any(|error| error.contains("parent") && error.contains("child")),
                "missing dependency reason: {:?}",
                state.errors
            );
            let view = dialog.plugin_view(&Route::Plugin("errors".into()));
            assert!(view.body.iter().any(|line| line.contains("child")));
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "dependency snapshot timed out"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}
