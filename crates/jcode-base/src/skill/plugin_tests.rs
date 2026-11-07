use super::*;
use crate::plugin::PluginScope;
use crate::plugin_components::fixture::{Fixture, write};

#[test]
fn plugin_registry_root_skill_fallback_when_skills_directory_absent() {
    let f = Fixture::new();
    let installed = f
        .manager
        .install("registry-fixture", PluginScope::User)
        .unwrap();
    write(
        &installed.root.join("SKILL.md"),
        "---\nname: root-fallback\ndescription: Root skill\n---\nRoot body",
    );
    let registry = SkillRegistry::load_for_working_dir(Some(&f.project)).unwrap();
    assert!(!registry.contains("registry-fixture:root-fallback"));
    std::fs::remove_dir_all(installed.root.join("skills")).unwrap();
    let registry = SkillRegistry::effective_for_working_dir(&registry, Some(&f.project));
    assert!(registry.contains("registry-fixture:root-fallback"));
    assert!(registry.contains("registry-fixture:custom"));
}

#[test]
fn plugin_registry_explicit_commands_replace_defaults_but_skills_extend() {
    let f = Fixture::new();
    let installed = f
        .manager
        .install("registry-fixture", PluginScope::User)
        .unwrap();
    write(
        &installed.root.join(".claude-plugin/plugin.json"),
        r#"{"name":"registry-fixture","commands":"./extra-commands","agents":"./extra-agents","skills":"./extra-skills"}"#,
    );
    let registry = SkillRegistry::load_for_working_dir(Some(&f.project)).unwrap();
    assert!(registry.contains("registry-fixture:default"));
    assert!(registry.contains("registry-fixture:custom"));
    assert!(registry.contains("registry-fixture:nested:check"));
    assert!(!registry.contains("registry-fixture:plain"));
    let profiles = crate::agent_profile::list(&f.project).unwrap();
    assert!(profiles.contains(&"registry-fixture:custom".to_owned()));
    assert!(!profiles.contains(&"registry-fixture:reviewer".to_owned()));
    // Without declarations, commands and agents use the conventional directories.
    write(
        &installed.root.join(".claude-plugin/plugin.json"),
        r#"{"name":"registry-fixture"}"#,
    );
    let registry = SkillRegistry::effective_for_working_dir(&registry, Some(&f.project));
    assert!(registry.contains("registry-fixture:plain"));
    assert!(!registry.contains("registry-fixture:nested:check"));
    assert!(
        crate::agent_profile::list(&f.project)
            .unwrap()
            .contains(&"registry-fixture:reviewer".to_owned())
    );
}

#[test]
fn plugin_registry_activates_default_and_manifest_skills_and_commands() {
    let f = Fixture::new();
    let installed = f
        .manager
        .install("registry-fixture", PluginScope::User)
        .unwrap();
    let mut registry = SkillRegistry::load_for_working_dir(Some(&f.project)).unwrap();
    for name in ["default", "custom", "plain", "nested:check"] {
        assert!(
            registry.contains(&format!("registry-fixture:{name}")),
            "missing {name}"
        );
    }
    let command = registry.get("registry-fixture:plain").unwrap();
    let invoked = command.invocation_text("hello");
    assert!(invoked.contains("Run hello in"));
    assert!(invoked.contains(installed.root.to_str().unwrap()));
    assert!(!invoked.contains("${CLAUDE_PLUGIN_ROOT}"));
    assert!(registry.reload("registry-fixture:plain").unwrap());
    assert!(registry.reload("registry-fixture:nested:check").unwrap());
    assert_eq!(
        registry.get("registry-fixture:nested:check").unwrap().name,
        "registry-fixture:nested:check"
    );
}

#[test]
fn plugin_registry_disable_uninstall_and_scope_remove_cached_contributions() {
    let f = Fixture::new();
    f.manager
        .install("registry-fixture", PluginScope::Project)
        .unwrap();
    let initial = SkillRegistry::load_for_working_dir(Some(&f.project)).unwrap();
    assert!(initial.contains("registry-fixture:default"));
    let elsewhere = SkillRegistry::effective_for_working_dir(&initial, Some(&f.other));
    assert!(!elsewhere.contains("registry-fixture:default"));
    f.manager
        .disable("registry-fixture", PluginScope::Project)
        .unwrap();
    let disabled = SkillRegistry::effective_for_working_dir(&initial, Some(&f.project));
    assert!(!disabled.contains("registry-fixture:default"));
    assert!(!disabled.contains("registry-fixture:plain"));
    f.manager
        .enable("registry-fixture", PluginScope::Project)
        .unwrap();
    assert!(
        SkillRegistry::effective_for_working_dir(&disabled, Some(&f.project))
            .contains("registry-fixture:custom")
    );
    f.manager
        .uninstall("registry-fixture", PluginScope::Project)
        .unwrap();
    assert!(
        !SkillRegistry::effective_for_working_dir(&initial, Some(&f.project))
            .contains("registry-fixture:plain")
    );
}

#[test]
fn plugin_registry_explicit_override_survives_reload_provenance() {
    let f = Fixture::new();
    let installed = f
        .manager
        .install("registry-fixture", PluginScope::User)
        .unwrap();
    let explicit = f.temp.path().join("explicit");
    write(
        &explicit.join("override/SKILL.md"),
        "---\nname: registry-fixture:default\ndescription: Explicit override\n---\nUser body",
    );
    let mut registry = SkillRegistry::default();
    registry.load_enabled_plugin("registry-fixture", &installed.root);
    // Same loader used by reload_global for explicit global directories.
    registry.load_from_dir_count(&explicit).unwrap();
    assert!(!registry.plugin_keys.contains("registry-fixture:default"));
    let refreshed = SkillRegistry::effective_for_working_dir(&registry, Some(&f.project));
    assert_eq!(
        refreshed.get("registry-fixture:default").unwrap().content,
        "User body"
    );
}

#[cfg(unix)]
#[test]
fn plugin_registry_rejects_manifest_and_skill_symlink_escapes() {
    let f = Fixture::new();
    let installed = f
        .manager
        .install("registry-fixture", PluginScope::User)
        .unwrap();
    let secret = f.temp.path().join("secret.md");
    write(
        &secret,
        "---\nname: leaked\ndescription: secret\n---\nPrivate",
    );
    let escaped = installed.root.join("skills/escape");
    std::fs::create_dir_all(&escaped).unwrap();
    std::os::unix::fs::symlink(&secret, escaped.join("SKILL.md")).unwrap();
    std::os::unix::fs::symlink(&secret, installed.root.join("commands/leak.md")).unwrap();
    let manifest_path = installed.root.join(".claude-plugin/plugin.json");
    let valid_manifest = std::fs::read_to_string(&manifest_path).unwrap();
    write(
        &manifest_path,
        &format!(
            r#"{{"name":"registry-fixture","commands":["{}"]}}"#,
            secret.display()
        ),
    );
    let registry = SkillRegistry::load_for_working_dir(Some(&f.project)).unwrap();
    assert!(!registry.contains("registry-fixture:leaked"));
    assert!(!registry.contains("registry-fixture:secret"));
    assert!(!registry.contains("registry-fixture:leak"));
    // The backend rejects an invalid manifest as a whole, not just its bad
    // component. No contribution from that plugin may survive validation.
    assert!(f.manager.enabled_roots().unwrap().is_empty());
    assert!(!registry.contains("registry-fixture:default"));

    // With a valid manifest, the component loader must still reject individual
    // escaping symlinks without dropping unrelated, contained skill files.
    write(&manifest_path, &valid_manifest);
    assert_eq!(f.manager.enabled_roots().unwrap().len(), 1);
    let registry = SkillRegistry::effective_for_working_dir(&registry, Some(&f.project));
    assert!(!registry.contains("registry-fixture:leaked"));
    assert!(!registry.contains("registry-fixture:secret"));
    assert!(!registry.contains("registry-fixture:leak"));
    assert!(registry.contains("registry-fixture:default"));
}
