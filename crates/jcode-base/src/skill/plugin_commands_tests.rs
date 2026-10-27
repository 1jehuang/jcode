use super::*;
use crate::plugin::PluginScope;
use crate::plugin_components::fixture::{Fixture, write};

#[test]
fn installation_rejects_invalid_named_command_map_before_publication() {
    let f = Fixture::new();
    // add_marketplace acquires an immutable local copy. Installation reads
    // that registered snapshot, not the original marketplace source tree.
    let marketplace = f.manager.list_marketplaces().unwrap().remove(0);
    let source = marketplace.root.join("registry-fixture");
    manifest(
        &source,
        serde_json::json!({"bad":{"source":"commands/plain.md","content":"Both"}}),
    );
    let error = f
        .manager
        .install("registry-fixture", PluginScope::User)
        .unwrap_err();
    assert!(format!("{error:#}").contains("source XOR content"));
    assert!(f.manager.list().unwrap().is_empty());
    assert!(f.manager.enabled_roots().unwrap().is_empty());
}

fn manifest(root: &Path, commands: serde_json::Value) {
    write(
        &root.join(".claude-plugin/plugin.json"),
        &serde_json::json!({"name":"registry-fixture", "commands": commands}).to_string(),
    );
}

#[test]
fn named_inline_and_source_replace_scan_preserve_metadata_and_reload() {
    let f = Fixture::new();
    let installed = f
        .manager
        .install("registry-fixture", PluginScope::User)
        .unwrap();
    write(
        &installed.root.join("body.md"),
        "---\nname: ignored\ndescription: File description\n---\nSource body",
    );
    manifest(
        &installed.root,
        serde_json::json!({
            "about": {"content":"Inline $ARGUMENTS", "description":"Override", "argumentHint":"[file]", "model":"not-executed", "allowedTools":["Read"]},
            "nested:source": {"source":"./body.md"}
        }),
    );
    let mut registry = SkillRegistry::load_for_working_dir(Some(&f.project)).unwrap();
    assert!(!registry.contains("registry-fixture:plain"));
    let inline = registry.get("registry-fixture:about").unwrap();
    assert_eq!(inline.description, "Override");
    assert_eq!(inline.content, "Inline $ARGUMENTS");
    assert_eq!(inline.allowed_tools, Some(vec!["Read".to_owned()]));
    assert_eq!(
        registry
            .get("registry-fixture:nested:source")
            .unwrap()
            .content,
        "Source body"
    );
    manifest(
        &installed.root,
        serde_json::json!({"about":{"content":"Updated", "description":""}}),
    );
    assert!(registry.reload("registry-fixture:about").unwrap());
    assert_eq!(
        registry.get("registry-fixture:about").unwrap().content,
        "Updated"
    );
    assert_eq!(
        registry.get("registry-fixture:about").unwrap().description,
        "Updated"
    );
    assert!(!registry.contains("registry-fixture:nested:source"));
    f.manager
        .disable("registry-fixture", PluginScope::User)
        .unwrap();
    let refreshed = SkillRegistry::effective_for_working_dir(&registry, Some(&f.project));
    assert!(!refreshed.contains("registry-fixture:about"));
}

#[test]
fn named_description_overrides_preserve_empty_fallbacks_and_whitespace() {
    for (body, fallback) in [
        (
            "---\ndescription: YAML description\n---\nBody fallback",
            "YAML description",
        ),
        (
            "---\nname: informational\n---\nBody fallback",
            "Body fallback",
        ),
        ("Body fallback", "Body fallback"),
    ] {
        for from_file in [true, false] {
            let temp = tempfile::tempdir().unwrap();
            write(&temp.path().join("body.md"), body);
            let mut commands = serde_json::Map::new();
            for (name, description) in [
                ("omitted", None),
                ("empty", Some("")),
                ("override", Some("Manifest description")),
                ("whitespace", Some(" \t ")),
            ] {
                let mut declaration = if from_file {
                    serde_json::json!({"source": "./body.md"})
                } else {
                    serde_json::json!({"content": body})
                };
                if let Some(description) = description {
                    declaration["description"] = serde_json::json!(description);
                }
                commands.insert(name.to_owned(), declaration);
            }
            manifest(temp.path(), serde_json::Value::Object(commands));
            let mut registry = SkillRegistry::default();
            registry.load_declared_commands("registry-fixture", temp.path());
            for (name, expected) in [
                ("omitted", fallback),
                ("empty", fallback),
                ("override", "Manifest description"),
                ("whitespace", " \t "),
            ] {
                assert_eq!(
                    registry
                        .get(&format!("registry-fixture:{name}"))
                        .unwrap()
                        .description,
                    expected,
                    "from_file={from_file}, command={name}, body={body:?}"
                );
            }
        }
    }
}

#[test]
fn named_empty_allowed_tools_clears_frontmatter_tools() {
    let body = "---\ndescription: YAML description\nallowed-tools: [Read, Write]\n---\nBody";
    for from_file in [true, false] {
        let temp = tempfile::tempdir().unwrap();
        write(&temp.path().join("body.md"), body);
        let declaration = if from_file {
            serde_json::json!({"source": "./body.md"})
        } else {
            serde_json::json!({"content": body})
        };
        let mut cleared = declaration.clone();
        cleared["allowedTools"] = serde_json::json!([]);
        manifest(
            temp.path(),
            serde_json::json!({"inherited": declaration, "cleared": cleared}),
        );
        let mut registry = SkillRegistry::default();
        registry.load_declared_commands("registry-fixture", temp.path());
        assert_eq!(
            registry
                .get("registry-fixture:inherited")
                .unwrap()
                .allowed_tools,
            Some(vec!["Read".to_owned(), "Write".to_owned()]),
            "from_file={from_file}"
        );
        assert_eq!(
            registry
                .get("registry-fixture:cleared")
                .unwrap()
                .allowed_tools,
            Some(Vec::<String>::new()),
            "from_file={from_file}"
        );
    }
}

#[test]
fn named_map_rejects_invalid_xor_types_and_escaping_sources() {
    let temp = tempfile::tempdir().unwrap();
    write(&temp.path().join("body.md"), "Body");
    for value in [
        serde_json::json!({"bad":{}}),
        serde_json::json!({"bad":{"content":""}}),
        serde_json::json!({"bad":{"source":"body.md","content":"Both"}}),
        serde_json::json!({"bad":{"source":"../secret.md"}}),
        serde_json::json!({"bad":{"source":"/etc/passwd"}}),
        serde_json::json!({"bad":{"content":"Body","allowedTools":[1]}}),
    ] {
        assert!(
            crate::plugin_components::validate_command_map(temp.path(), &value).is_err(),
            "{value}"
        );
    }
}

#[test]
fn named_collisions_are_namespaced_and_real_skills_win() {
    let temp = tempfile::tempdir().unwrap();
    manifest(
        temp.path(),
        serde_json::json!({"shared":{"content":"Command"}}),
    );
    write(
        &temp.path().join("skills/shared/SKILL.md"),
        "---\nname: shared\ndescription: Real skill\n---\nSkill body",
    );
    let mut registry = SkillRegistry::default();
    registry.load_enabled_plugin("one", temp.path());
    registry.load_enabled_plugin("two", temp.path());
    assert_eq!(registry.get("one:shared").unwrap().content, "Skill body");
    assert_eq!(registry.get("two:shared").unwrap().content, "Skill body");
    assert!(!registry.command_origins.contains_key("one:shared"));
}

#[cfg(unix)]
#[test]
fn named_source_symlink_escape_is_rejected_without_execution() {
    let temp = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    write(&outside.path().join("secret.md"), "Secret");
    std::os::unix::fs::symlink(
        outside.path().join("secret.md"),
        temp.path().join("escape.md"),
    )
    .unwrap();
    let value = serde_json::json!({"leak":{"source":"escape.md"}});
    assert!(crate::plugin_components::validate_command_map(temp.path(), &value).is_err());
    manifest(
        temp.path(),
        serde_json::json!({"inert":{"content":"!`touch should-not-exist`"}}),
    );
    let mut registry = SkillRegistry::default();
    registry.load_enabled_plugin("inert", temp.path());
    assert_eq!(
        registry.get("inert:inert").unwrap().content,
        "!`touch should-not-exist`"
    );
    assert!(!temp.path().join("should-not-exist").exists());
}
