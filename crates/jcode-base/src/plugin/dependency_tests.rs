use super::*;
use serde_json::{Value, json};

fn fixture() -> (tempfile::TempDir, PluginManager) {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    fs::create_dir(&project).unwrap();
    let manager = PluginManager::new(temp.path().join("home"), project);
    (temp, manager)
}
fn market(temp: &Path, name: &str, plugins: Value, allowed: &[&str]) -> PathBuf {
    let root = temp.join(name);
    fs::create_dir_all(root.join(".claude-plugin")).unwrap();
    for plugin in plugins.as_array().unwrap() {
        let name = plugin["name"].as_str().unwrap();
        fs::create_dir_all(root.join(name).join(".claude-plugin")).unwrap();
        fs::write(
            root.join(name).join(".claude-plugin/plugin.json"),
            serde_json::to_vec(&json!({"name":name})).unwrap(),
        )
        .unwrap();
        fs::write(root.join(name).join("bytes"), name).unwrap();
    }
    fs::write(
        root.join(".claude-plugin/marketplace.json"),
        serde_json::to_vec(
            &json!({"name":name,"plugins":plugins,"allowCrossMarketplaceDependenciesOn":allowed}),
        )
        .unwrap(),
    )
    .unwrap();
    root
}
fn manifest(root: &Path, name: &str, value: Value) {
    fs::write(
        root.join(name).join(".claude-plugin/plugin.json"),
        serde_json::to_vec(&value).unwrap(),
    )
    .unwrap();
}
fn registry(manager: &PluginManager) -> Vec<u8> {
    fs::read(manager.base().join("registry.json")).unwrap()
}

#[test]
fn identifier_schema_normalizes_objects_and_rejects_ranges_unknowns() {
    assert_eq!(
        dependencies::parse(Some(
            &json!(["a", {"name":"b","marketplace":"other"}, {"name":"c"}, "a"])
        ))
        .unwrap(),
        ["a", "b@other", "c"]
    );
    for value in [
        json!(["a@^1"]),
        json!([{"name":"b","version":"1"}]),
        json!([false]),
        json!(["../escape"]),
        json!("a"),
        json!([{"name":"a","marketplace":false}]),
        json!([{"name":"a@other"}]),
        json!([{"name":"a","marketplace":"b@c"}]),
    ] {
        assert!(
            dependencies::parse(Some(&value)).is_err(),
            "accepted {value}"
        );
    }
}

#[test]
fn graph_depth_and_list_limits_fail_without_registry_change() {
    let (temp, manager) = fixture();
    let plugins: Vec<Value> = (0..66)
        .map(|i| {
            let name = format!("node{i}");
            let mut value = json!({"name":name,"source":format!("./{name}")});
            if i < 65 {
                value["dependencies"] = json!([format!("node{}", i + 1)]);
            }
            value
        })
        .collect();
    let root = market(temp.path(), "fixture", Value::Array(plugins), &[]);
    manager.add_marketplace(root.to_str().unwrap()).unwrap();
    let before = registry(&manager);
    let error = manager
        .install("node0", PluginScope::User)
        .unwrap_err()
        .to_string();
    assert!(error.contains("depth 64"), "{error}");
    assert_eq!(registry(&manager), before);
    assert!(!manager.base().join("cache").exists());
    let too_many = Value::Array((0..257).map(|_| json!("a")).collect());
    assert!(dependencies::parse(Some(&too_many)).is_err());
}

#[test]
fn cached_cycles_fail_runtime_and_update_without_acquiring_new_dependency_bytes() {
    let (temp, manager) = fixture();
    let source = market(
        temp.path(),
        "fixture",
        json!([
            {"name":"root","source":"./root","dependencies":["middle"]},
            {"name":"middle","source":"./middle","dependencies":["leaf"]},
            {"name":"leaf","source":"./leaf"}
        ]),
        &[],
    );
    manager.add_marketplace(source.to_str().unwrap()).unwrap();
    manager.install("root", PluginScope::User).unwrap();
    let leaf = manager
        .list()
        .unwrap()
        .into_iter()
        .find(|p| p.name == "leaf")
        .unwrap();
    fs::write(
        leaf.root.join(".claude-plugin/plugin.json"),
        serde_json::to_vec(&json!({"name":"leaf","dependencies":["root"]})).unwrap(),
    )
    .unwrap();
    let before = registry(&manager);
    assert!(manager.enabled_roots().unwrap().is_empty());
    assert!(
        manager
            .dependency_diagnostics()
            .unwrap()
            .iter()
            .any(|s| s.contains("cycle"))
    );
    let error = manager
        .update("root", PluginScope::User)
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("root@fixture -> middle@fixture -> leaf@fixture -> root@fixture"),
        "{error}"
    );
    assert_eq!(registry(&manager), before);
}

#[test]
fn transitive_manifest_and_catalog_dependencies_stage_in_requested_scope() {
    let (temp, manager) = fixture();
    let root = market(
        temp.path(),
        "fixture",
        json!([
            {"name":"root","source":"./root","dependencies":["catalog"]},
            {"name":"middle","source":"./middle"},
            {"name":"leaf","source":"./leaf","defaultEnabled":false},
            {"name":"catalog","source":"./catalog"}
        ]),
        &[],
    );
    manifest(
        &root,
        "root",
        json!({"name":"root","dependencies":["middle"]}),
    );
    manifest(
        &root,
        "middle",
        json!({"name":"middle","dependencies":[{"name":"leaf"}]}),
    );
    manager.add_marketplace(root.to_str().unwrap()).unwrap();
    manager.install("root", PluginScope::Project).unwrap();
    let list = manager.list().unwrap();
    assert_eq!(list.len(), 4);
    assert!(
        list.iter()
            .all(|p| p.enabled && p.scope == PluginScope::Project)
    );
    assert_eq!(manager.enabled_roots().unwrap().len(), 4);
    assert!(manager.enabled_user_roots().unwrap().is_empty());
    assert!(manager.dependency_diagnostics().unwrap().is_empty());
}

#[test]
fn failed_missing_cycle_and_invalid_payload_leave_registry_and_existing_bytes() {
    let (temp, manager) = fixture();
    let root = market(
        temp.path(),
        "fixture",
        json!([
            {"name":"root","source":"./root"}, {"name":"leaf","source":"./leaf"}
        ]),
        &[],
    );
    manager.add_marketplace(root.to_str().unwrap()).unwrap();
    let installed = manager.install("root", PluginScope::User).unwrap();
    let original_installed = serde_json::to_value(manager.list().unwrap()).unwrap();
    for deps in [json!(["missing"]), json!(["root"]), json!(["leaf"])] {
        manifest(&root, "root", json!({"name":"root","dependencies":deps}));
        manifest(&root, "leaf", json!({"name":"wrong-name"}));
        manager.update_marketplace("fixture").unwrap();
        // Refresh deliberately publishes a new marketplace cache UUID. The
        // failed plugin update must preserve that successful refresh exactly.
        let before = registry(&manager);
        assert!(manager.update("root", PluginScope::User).is_err());
        assert_eq!(registry(&manager), before);
        assert_eq!(
            serde_json::to_value(manager.list().unwrap()).unwrap(),
            original_installed
        );
        assert_eq!(
            fs::read_to_string(installed.root.join("bytes")).unwrap(),
            "root"
        );
        assert_eq!(manager.list().unwrap().len(), 1);
    }
}

#[test]
fn root_only_allowlist_does_not_inherit_transitive_trust() {
    let (temp, manager) = fixture();
    let a = market(
        temp.path(),
        "a",
        json!([{"name":"root","source":"./root","dependencies":["middle@b"]}]),
        &["b"],
    );
    let b = market(
        temp.path(),
        "b",
        json!([{"name":"middle","source":"./middle","dependencies":["leaf@c"]}]),
        &["c"],
    );
    let c = market(
        temp.path(),
        "c",
        json!([{"name":"leaf","source":"./leaf"}]),
        &[],
    );
    for root in [&a, &b, &c] {
        manager.add_marketplace(root.to_str().unwrap()).unwrap();
    }
    let before = registry(&manager);
    let error = manager
        .install("root@a", PluginScope::User)
        .unwrap_err()
        .to_string();
    assert!(error.contains("ROOT"), "{error}");
    assert_eq!(registry(&manager), before);
    assert!(manager.list().unwrap().is_empty());
    let mut catalog: Value =
        serde_json::from_slice(&fs::read(a.join(".claude-plugin/marketplace.json")).unwrap())
            .unwrap();
    catalog["allowCrossMarketplaceDependenciesOn"] = json!(["b", "c"]);
    fs::write(
        a.join(".claude-plugin/marketplace.json"),
        serde_json::to_vec(&catalog).unwrap(),
    )
    .unwrap();
    manager.update_marketplace("a").unwrap();
    manager.install("root@a", PluginScope::User).unwrap();
    assert_eq!(manager.list().unwrap().len(), 3);
}

#[test]
fn installed_cross_market_dependency_is_reused_without_catalog_update() {
    let (temp, manager) = fixture();
    let a = market(
        temp.path(),
        "a",
        json!([{"name":"root","source":"./root","dependencies":["leaf@b"]}]),
        &[],
    );
    let b = market(
        temp.path(),
        "b",
        json!([{"name":"leaf","source":"./leaf"}]),
        &[],
    );
    for root in [&a, &b] {
        manager.add_marketplace(root.to_str().unwrap()).unwrap();
    }
    let leaf = manager.install("leaf@b", PluginScope::User).unwrap();
    manifest(
        &b,
        "leaf",
        json!({"name":"leaf","dependencies":["missing"]}),
    );
    manager.update_marketplace("b").unwrap();
    manager.install("root@a", PluginScope::User).unwrap();
    assert_eq!(
        manager
            .list()
            .unwrap()
            .iter()
            .find(|p| p.name == "leaf")
            .unwrap()
            .root,
        leaf.root
    );
    assert_eq!(manager.enabled_roots().unwrap().len(), 2);
}

#[test]
fn disabled_transitive_cached_dependency_and_scope_masks_are_not_overridden() {
    let (temp, manager) = fixture();
    let root = market(
        temp.path(),
        "fixture",
        json!([
            {"name":"root","source":"./root","dependencies":["middle"]},
            {"name":"middle","source":"./middle","dependencies":["leaf"]},
            {"name":"leaf","source":"./leaf"}
        ]),
        &[],
    );
    manager.add_marketplace(root.to_str().unwrap()).unwrap();
    manager.install("root", PluginScope::Project).unwrap();
    manager.disable("leaf", PluginScope::Project).unwrap();
    let before = registry(&manager);
    let error = manager
        .update("root", PluginScope::Project)
        .unwrap_err()
        .to_string();
    assert!(error.contains("enable dependency first"), "{error}");
    assert_eq!(registry(&manager), before);
    assert!(manager.enabled_roots().unwrap().is_empty());
    manager.enable("leaf", PluginScope::Project).unwrap();
    manager.install("leaf", PluginScope::Local).unwrap();
    manager.disable("leaf", PluginScope::Local).unwrap();
    manager.update("root", PluginScope::Project).unwrap();
    assert!(
        manager
            .list()
            .unwrap()
            .iter()
            .any(|p| p.name == "leaf" && p.scope == PluginScope::Local && !p.enabled)
    );
    assert!(manager.enabled_roots().unwrap().is_empty());
}

#[test]
fn runtime_closure_disables_and_restores_contributions_without_mutating_reads() {
    let (temp, manager) = fixture();
    let root = market(
        temp.path(),
        "fixture",
        json!([
            {"name":"root","source":"./root","dependencies":["leaf"]}, {"name":"leaf","source":"./leaf"}
        ]),
        &[],
    );
    manager.add_marketplace(root.to_str().unwrap()).unwrap();
    manager.install("root", PluginScope::User).unwrap();
    assert_eq!(manager.enabled_roots().unwrap().len(), 2);
    manager.disable("leaf", PluginScope::User).unwrap();
    let before = registry(&manager);
    assert!(manager.enabled_roots().unwrap().is_empty());
    assert!(
        manager
            .dependency_diagnostics()
            .unwrap()
            .iter()
            .any(|s| s.contains("disabled"))
    );
    assert_eq!(registry(&manager), before);
    manager.enable("leaf", PluginScope::User).unwrap();
    assert_eq!(manager.enabled_roots().unwrap().len(), 2);
    manager.uninstall("leaf", PluginScope::User).unwrap();
    assert!(manager.enabled_roots().unwrap().is_empty());
    assert!(
        manager
            .dependency_diagnostics()
            .unwrap()
            .iter()
            .any(|s| s.contains("not installed"))
    );
}

#[test]
fn disabled_default_root_does_not_activate_new_dependency() {
    let (temp, manager) = fixture();
    let root = market(
        temp.path(),
        "fixture",
        json!([
            {"name":"root","source":"./root","defaultEnabled":false,"dependencies":["leaf"]},
            {"name":"leaf","source":"./leaf"}
        ]),
        &[],
    );
    manager.add_marketplace(root.to_str().unwrap()).unwrap();
    let installed = manager.install("root", PluginScope::User).unwrap();
    assert!(!installed.enabled);
    assert!(manager.list().unwrap().iter().all(|p| !p.enabled));
    assert!(manager.enabled_roots().unwrap().is_empty());
    manager.enable("root", PluginScope::User).unwrap();
    assert_eq!(manager.enabled_roots().unwrap().len(), 2);
}

#[test]
fn enabling_an_inactive_root_preserves_explicit_dependency_disables() {
    let (temp, manager) = fixture();
    let root = market(
        temp.path(),
        "fixture",
        json!([
            {"name":"root","source":"./root","defaultEnabled":false,"dependencies":["middle"]},
            {"name":"middle","source":"./middle","dependencies":["leaf"]},
            {"name":"leaf","source":"./leaf"}
        ]),
        &[],
    );
    manager.add_marketplace(root.to_str().unwrap()).unwrap();
    manager.install("root", PluginScope::User).unwrap();
    manager.disable("leaf", PluginScope::User).unwrap();
    let before = registry(&manager);
    assert!(manager.enable("root", PluginScope::User).is_err());
    assert_eq!(registry(&manager), before);
    manager.enable("leaf", PluginScope::User).unwrap();
    manager.enable("root", PluginScope::User).unwrap();
    assert_eq!(manager.enabled_roots().unwrap().len(), 3);
}

#[test]
fn requested_scope_changes_succeed_under_higher_disabled_masks() {
    for with_dependency in [false, true] {
        let (temp, manager) = fixture();
        let mut plugin = json!({"name":"root","source":"./root"});
        if with_dependency {
            plugin["dependencies"] = json!(["leaf"]);
        }
        let root = market(
            temp.path(),
            "fixture",
            json!([
                plugin, {"name":"leaf","source":"./leaf"}
            ]),
            &[],
        );
        manager.add_marketplace(root.to_str().unwrap()).unwrap();
        manager.install("root", PluginScope::Local).unwrap();
        manager.disable("root", PluginScope::Local).unwrap();
        if with_dependency {
            manager.disable("leaf", PluginScope::Local).unwrap();
        }
        manager.install("root", PluginScope::User).unwrap();
        manager.update("root", PluginScope::User).unwrap();
        assert!(manager.enabled_roots().unwrap().is_empty());
        assert!(
            manager
                .list()
                .unwrap()
                .iter()
                .filter(|p| p.scope == PluginScope::Local)
                .all(|p| !p.enabled)
        );
        let other = PluginManager::new(manager.user_root.clone(), temp.path().join("other"));
        assert_eq!(
            other.enabled_roots().unwrap().len(),
            if with_dependency { 2 } else { 1 }
        );
    }
}

#[test]
fn active_root_reuses_automatically_disabled_dependency_without_enabling_other_root() {
    let (temp, manager) = fixture();
    let root = market(
        temp.path(),
        "fixture",
        json!([
            {"name":"inactive","source":"./inactive","defaultEnabled":false,"dependencies":["leaf"]},
            {"name":"active","source":"./active","dependencies":["leaf"]},
            {"name":"leaf","source":"./leaf"}
        ]),
        &[],
    );
    manager.add_marketplace(root.to_str().unwrap()).unwrap();
    manager.install("inactive", PluginScope::User).unwrap();
    manager.update("leaf", PluginScope::User).unwrap();
    manager.update("inactive", PluginScope::User).unwrap();
    manager.install("active", PluginScope::User).unwrap();
    assert_eq!(manager.enabled_roots().unwrap().len(), 2);
    assert!(
        !manager
            .list()
            .unwrap()
            .iter()
            .find(|p| p.name == "inactive")
            .unwrap()
            .enabled
    );
}

#[test]
fn higher_scope_marketplace_conflicts_do_not_block_global_management() {
    let (temp, manager) = fixture();
    for name in ["local", "global"] {
        let source = market(
            temp.path(),
            name,
            json!([
                {"name":"root","source":"./root"}
            ]),
            &[],
        );
        manager.add_marketplace(source.to_str().unwrap()).unwrap();
    }
    manager.install("root@local", PluginScope::Local).unwrap();
    manager.disable("root@local", PluginScope::Local).unwrap();
    manager.install("root@global", PluginScope::User).unwrap();
    manager.update("root@global", PluginScope::User).unwrap();
    assert!(manager.enabled_roots().unwrap().is_empty());
    let other = PluginManager::new(manager.user_root.clone(), temp.path().join("other"));
    let roots = other.enabled_roots().unwrap();
    assert_eq!(roots.len(), 1);
    assert_eq!(roots[0].marketplace, "global");
}

#[test]
fn legacy_disabled_dependency_without_provenance_stays_explicit() {
    let (temp, manager) = fixture();
    let root = market(
        temp.path(),
        "fixture",
        json!([
            {"name":"root","source":"./root","defaultEnabled":false,"dependencies":["leaf"]},
            {"name":"leaf","source":"./leaf"}
        ]),
        &[],
    );
    manager.add_marketplace(root.to_str().unwrap()).unwrap();
    manager.install("root", PluginScope::User).unwrap();
    let mut saved: Value = serde_json::from_slice(&registry(&manager)).unwrap();
    for record in saved["installed"].as_array_mut().unwrap() {
        record
            .as_object_mut()
            .unwrap()
            .remove("automatically_disabled");
    }
    atomic_json(&manager.base().join("registry.json"), &saved).unwrap();
    let before = registry(&manager);
    assert!(manager.enable("root", PluginScope::User).is_err());
    assert_eq!(registry(&manager), before);
    manager.enable("leaf", PluginScope::User).unwrap();
    manager.enable("root", PluginScope::User).unwrap();
    assert_eq!(manager.enabled_roots().unwrap().len(), 2);
}
