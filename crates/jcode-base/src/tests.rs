//! Tests for `#[path]`-attributed module `tests` of `skill.rs`.
//!
//! Extracted from the parent file: an inline `#[cfg(test)] mod` counts toward
//! the code-size budget, which only exempts `*_tests.rs` files.

use super::Skill;
use std::path::Path;

use super::*;

fn test_skill(name: &str, description: &str, content: &str) -> Skill {
    Skill {
        name: name.to_string(),
        description: description.to_string(),
        allowed_tools: None,
        content: content.to_string(),
        path: PathBuf::from(format!("/tmp/{name}/SKILL.md")),
        search_text: build_skill_search_text(name, description, content),
    }
}

fn write_test_skill(root: &Path, scope: &str, name: &str) {
    let dir = root.join(scope).join("skills").join(name);
    std::fs::create_dir_all(&dir).expect("create skill dir");
    std::fs::write(
        dir.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: Test skill {name}\n---\n\nUse {name}.\n"),
    )
    .expect("write skill");
}

fn write_skill_file(skills_dir: &Path, name: &str, content: &str) -> PathBuf {
    let dir = skills_dir.join(name);
    std::fs::create_dir_all(&dir).expect("create skill dir");
    let path = dir.join("SKILL.md");
    std::fs::write(&path, content).expect("write skill");
    path
}

#[cfg(unix)]
#[test]
fn copy_skills_dir_skips_skill_already_imported_from_linked_source() {
    use std::os::unix::fs::PermissionsExt;

    let temp = tempfile::tempdir().expect("temp dir");
    let claude = temp.path().join("claude-skills");
    let skill = claude.join("s");
    std::fs::create_dir_all(skill.join("sub")).expect("create skill");
    std::fs::write(
        skill.join("SKILL.md"),
        "---\nname: s\ndescription: test skill\n---\nbody\n",
    )
    .expect("write skill");
    let readonly = skill.join("sub").join("obj");
    std::fs::write(&readonly, "x").expect("write obj");
    std::fs::set_permissions(&readonly, std::fs::Permissions::from_mode(0o444)).expect("chmod");
    let codex = temp.path().join("codex-skills");
    std::os::unix::fs::symlink(&claude, &codex).expect("symlink");
    let dest = temp.path().join("jcode-skills");

    let first = SkillRegistry::copy_skills_dir(&claude, &dest);
    assert_eq!((first.copied, first.skipped, first.failed), (1, 0, 0));
    let second = SkillRegistry::copy_skills_dir(&codex, &dest);
    assert_eq!((second.copied, second.skipped, second.failed), (0, 1, 0));
    assert_eq!(
        std::fs::read_to_string(dest.join("s").join("sub").join("obj")).expect("read obj"),
        "x"
    );
}

#[test]
fn allowed_tools_accepts_legacy_string_sequence_and_absence() {
    let temp = tempfile::tempdir().expect("temp dir");
    let cases = [
        (
            "allowed-tools: bash, read, write\n",
            Some(vec!["bash", "read", "write"]),
        ),
        (
            "allowed-tools:\n  - bash\n  - read\n  - write\n",
            Some(vec!["bash", "read", "write"]),
        ),
        ("", None),
    ];

    for (index, (allowed_tools, expected)) in cases.into_iter().enumerate() {
        let path = temp.path().join(format!("skill-{index}.md"));
        std::fs::write(
            &path,
            format!("---\nname: test\ndescription: Test skill\n{allowed_tools}---\n\nBody\n"),
        )
        .expect("write skill");
        let skill = SkillRegistry::parse_skill_inner(&path).expect("parse skill");
        let expected = expected.map(|tools| {
            tools
                .into_iter()
                .map(str::to_string)
                .collect::<Vec<String>>()
        });
        assert_eq!(skill.allowed_tools, expected);
    }
}

#[test]
fn allowed_tools_rejects_non_string_sequence_values() {
    let temp = tempfile::tempdir().expect("temp dir");
    let path = temp.path().join("SKILL.md");
    std::fs::write(
        &path,
        "---\nname: test\ndescription: Test skill\nallowed-tools: [bash, 1]\n---\n\nBody\n",
    )
    .expect("write skill");

    assert!(SkillRegistry::parse_skill_inner(&path).is_err());
}

#[test]
fn parse_invocation_supports_a_trailing_prompt() {
    assert_eq!(
        SkillRegistry::parse_invocation("/frontend-design build a settings page"),
        Some(SkillInvocation {
            name: "frontend-design",
            prompt: Some("build a settings page"),
        })
    );
    assert_eq!(
        SkillRegistry::parse_invocation("  /frontend-design   \"build a settings page\"  "),
        Some(SkillInvocation {
            name: "frontend-design",
            prompt: Some("build a settings page"),
        })
    );
}

#[test]
fn parse_invocation_handles_bare_and_incomplete_quoted_prompts_without_blocking() {
    assert_eq!(
        SkillRegistry::parse_invocation("/optimization"),
        Some(SkillInvocation {
            name: "optimization",
            prompt: None,
        })
    );
    assert_eq!(
        SkillRegistry::parse_invocation("/optimization \"make this faster"),
        Some(SkillInvocation {
            name: "optimization",
            prompt: Some("\"make this faster"),
        })
    );
    assert_eq!(SkillRegistry::parse_invocation("/"), None);
}

#[test]
fn parse_invocation_rejects_terminal_file_drop_paths() {
    for input in [
        "/tmp/screenshot.png",
        "/Users/example/My\\ File.txt inspect this",
        "/home/example/project/file.rs",
        "/.hidden-file",
        "/network\\share\\file.txt",
    ] {
        assert_eq!(
            SkillRegistry::parse_invocation(input),
            None,
            "filesystem path must not parse as a skill invocation: {input}"
        );
    }
}

#[test]
fn list_is_sorted_by_name_regardless_of_insertion_order() {
    // The "Available Skills" system-prompt section is built from `list()`.
    // The backing store is a HashMap (per-instance randomized iteration
    // order), and `current_skills_snapshot` can hand back a *different*
    // HashMap instance via its lock-contended `self.skills.clone()` fallback.
    // If `list()` did not sort, two snapshots of the same skill set could
    // serialize the section in different orders: a same-length but
    // different-bytes system prompt that silently busts the KV cache.
    let names = ["zebra", "alpha", "mango", "beta", "yak"];

    let mut reg_a = SkillRegistry::default();
    for name in names {
        reg_a
            .skills
            .insert(name.to_string(), test_skill(name, "d", "c"));
    }

    // Build a second registry with the reverse insertion order to maximize
    // the chance of a differing HashMap layout.
    let mut reg_b = SkillRegistry::default();
    for name in names.iter().rev() {
        reg_b
            .skills
            .insert(name.to_string(), test_skill(name, "d", "c"));
    }

    let order_a: Vec<&str> = reg_a.list().iter().map(|s| s.name.as_str()).collect();
    let order_b: Vec<&str> = reg_b.list().iter().map(|s| s.name.as_str()).collect();

    assert_eq!(order_a, vec!["alpha", "beta", "mango", "yak", "zebra"]);
    assert_eq!(
        order_a, order_b,
        "list() ordering must be identical across HashMap instances"
    );
}

#[test]
fn skill_as_memory_entry_formats_invocation_and_prompt() {
    let skill = test_skill(
        "firefox-browser",
        "Control Firefox browser sessions and logged-in pages",
        "Use this skill when you need to open websites, click buttons, or interact with browser pages.",
    );

    let entry = skill.as_memory_entry();

    assert_eq!(entry.id, "skill:firefox-browser");
    assert!(matches!(
        entry.category,
        crate::memory::MemoryCategory::Custom(ref name) if name == "Skills"
    ));
    assert!(entry.content.contains("/firefox-browser"));
    assert!(entry.content.contains("# Skill: firefox-browser"));
    assert_eq!(entry.source.as_deref(), Some("skill_registry"));
}

#[test]
fn load_for_working_dir_reads_project_local_jcode_skills() {
    let temp = tempfile::tempdir().expect("tempdir");
    write_test_skill(temp.path(), ".jcode", "wd-only");

    let registry = SkillRegistry::load_for_working_dir(Some(temp.path())).expect("load skills");

    let skill = registry
        .get("wd-only")
        .expect("working-dir local skill should load");
    assert_eq!(skill.description, "Test skill wd-only");
    assert!(skill.path.starts_with(temp.path()));
}

#[test]
fn load_for_working_dir_reads_project_local_agents_skills() {
    let temp = tempfile::tempdir().expect("tempdir");
    write_test_skill(temp.path(), ".agents", "agents-only");

    let registry = SkillRegistry::load_for_working_dir(Some(temp.path())).expect("load skills");

    let skill = registry
        .get("agents-only")
        .expect("project-local .agents skill should load");
    assert_eq!(skill.description, "Test skill agents-only");
    assert!(skill.path.starts_with(temp.path()));
}

#[test]
fn malformed_skill_does_not_block_directory_loaders() {
    let temp = tempfile::tempdir().expect("tempdir");
    let skills_dir = temp.path().join("skills");
    write_skill_file(
        &skills_dir,
        "valid",
        "---\nname: valid\ndescription: Valid skill\n---\n\nUse valid.\n",
    );
    write_skill_file(
        &skills_dir,
        "malformed",
        "---\nname: malformed\ndescription: Triggers: invalid yaml\n---\n\nBroken.\n",
    );

    let mut registry = SkillRegistry::default();
    registry
        .load_from_dir(&skills_dir)
        .expect("load skills while skipping malformed file");
    assert!(registry.contains("valid"));
    assert!(!registry.contains("malformed"));

    let mut counted_registry = SkillRegistry::default();
    let count = counted_registry
        .load_from_dir_count(&skills_dir)
        .expect("count skills while skipping malformed file");
    assert_eq!(count, 1);
    assert!(counted_registry.contains("valid"));
    assert!(!counted_registry.contains("malformed"));
}

/// A session with no workspace root must not pick up the skills of
/// whichever project happens to have started the daemon.
///
/// The process cwd is deliberately pointed at a repository that *does* have
/// `.jcode/skills`, so this fails under the old
/// `unwrap_or(relative_path)` fallback. `Some(...)` is asserted in the same
/// test so the assertion cannot pass vacuously.
#[test]
fn no_working_dir_loads_no_project_skills_even_under_a_repo_cwd() {
    let _env = crate::storage::lock_test_env();
    let repo = tempfile::tempdir().expect("tempdir");
    write_test_skill(repo.path(), ".jcode", "daemon-project-skill");

    let old_cwd = std::env::current_dir().expect("current dir");
    std::env::set_current_dir(repo.path()).expect("set current dir");

    // The cwd fallback under test: relative `.jcode/skills` from the daemon's
    // directory. This is what a `working_dir: None` session must not do.
    assert!(
        repo.path()
            .join(".jcode/skills/daemon-project-skill/SKILL.md")
            .exists(),
        "test must run inside a repo that really has this project skill"
    );

    let overlay = SkillRegistry::load_project_overlay(None).expect("load overlay");
    let registry = SkillRegistry::load_for_working_dir(None).expect("load skills");
    let effective = SkillRegistry::effective_for_working_dir(&SkillRegistry::default(), None);

    let same_dir = SkillRegistry::load_project_overlay(Some(repo.path())).expect("load overlay");

    std::env::set_current_dir(old_cwd).expect("restore current dir");

    assert!(
        overlay.get("daemon-project-skill").is_none(),
        "None working_dir must not read .jcode/skills from the process cwd"
    );
    assert!(
        registry.get("daemon-project-skill").is_none(),
        "None working_dir must not read .jcode/skills from the process cwd"
    );
    assert!(
        effective.get("daemon-project-skill").is_none(),
        "effective skills for a session with no cwd must not include another \
         project's skills"
    );

    // Positive control: the same lookup does find it when the workspace root
    // is named. Without this, the assertions above would also pass if the
    // loader were simply broken.
    assert_eq!(
        same_dir
            .get("daemon-project-skill")
            .map(|skill| skill.path.clone()),
        Some(
            repo.path()
                .join(".jcode/skills/daemon-project-skill/SKILL.md")
        ),
        "naming the workspace root must still load that project's skills"
    );
}

/// `None` means "no project", for every project-local convention, not just
/// `.jcode`.
#[test]
fn no_working_dir_skips_every_project_local_skill_convention() {
    let _env = crate::storage::lock_test_env();
    let repo = tempfile::tempdir().expect("tempdir");
    write_test_skill(repo.path(), ".jcode", "jcode-convention");
    write_test_skill(repo.path(), ".agents", "agents-convention");
    write_test_skill(repo.path(), ".claude", "claude-convention");

    let old_cwd = std::env::current_dir().expect("current dir");
    std::env::set_current_dir(repo.path()).expect("set current dir");
    let overlay = SkillRegistry::load_project_overlay(None).expect("load overlay");
    std::env::set_current_dir(old_cwd).expect("restore current dir");

    for name in ["jcode-convention", "agents-convention", "claude-convention"] {
        assert!(
            overlay.get(name).is_none(),
            "{name} leaked in from the daemon cwd"
        );
    }
}

#[test]
fn project_overlay_is_session_scoped_and_composes_over_globals() {
    let temp = tempfile::tempdir().expect("tempdir");
    write_test_skill(temp.path(), ".jcode", "session-skill");

    // The overlay resolves against the given workspace root, not the
    // process cwd or a shared registry (issue #457).
    let overlay = SkillRegistry::load_project_overlay(Some(temp.path())).expect("load overlay");
    assert!(overlay.get("session-skill").is_some());

    // A different workspace root sees nothing from this project.
    let other = tempfile::tempdir().expect("tempdir");
    let other_overlay =
        SkillRegistry::load_project_overlay(Some(other.path())).expect("load overlay");
    assert!(other_overlay.get("session-skill").is_none());

    // Effective set = base globals + overlay, with overlay winning on name.
    let mut base = SkillRegistry::default();
    base.skills.insert(
        "session-skill".to_string(),
        test_skill("session-skill", "global variant", "global body"),
    );
    let effective = SkillRegistry::effective_for_working_dir(&base, Some(temp.path()));
    let skill = effective
        .get("session-skill")
        .expect("overlay skill should be present");
    assert!(
        skill.path.starts_with(temp.path()),
        "project-local overlay must win over a same-named global skill"
    );
}

#[test]
fn reload_global_excludes_project_local_skills() {
    // chdir is process-global; serialize with other env-sensitive tests.
    let _env_guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("tempdir");
    write_test_skill(temp.path(), ".jcode", "session-skill");

    let prev_cwd = std::env::current_dir().expect("cwd");
    // reload_global must not pick up project-local skills even when the
    // process cwd contains them (daemon startup cwd independence).
    std::env::set_current_dir(temp.path()).expect("chdir");
    let mut registry = SkillRegistry::default();
    let result = registry.reload_global();
    std::env::set_current_dir(prev_cwd).expect("restore cwd");

    result.expect("reload skills");
    assert!(
        registry.get("session-skill").is_none(),
        "shared/global reload must never absorb project-local skills"
    );
}

#[test]
fn endorsed_skills_have_unique_nonempty_metadata() {
    let endorsed = endorsed_skills();
    assert!(!endorsed.is_empty(), "expected at least one endorsed skill");

    let mut seen = std::collections::HashSet::new();
    for skill in endorsed {
        assert!(!skill.name.is_empty(), "endorsed skill name must be set");
        assert!(
            !skill.description.is_empty(),
            "endorsed skill {} needs a description",
            skill.name
        );
        assert!(
            !skill.category.is_empty(),
            "endorsed skill {} needs a category",
            skill.name
        );
        assert!(
            !skill.source.is_empty(),
            "endorsed skill {} needs a source",
            skill.name
        );
        assert!(
            !skill.name.starts_with('/'),
            "endorsed skill name should not include the leading slash"
        );
        if let Some(install) = skill.install {
            assert!(
                install.contains(skill.name),
                "endorsed skill {} install hint should reference its name",
                skill.name
            );
        }
        assert!(
            seen.insert(skill.name),
            "duplicate endorsed skill name: {}",
            skill.name
        );
    }
}

#[test]
fn endorsed_skills_include_nvidia_cuda_x_catalog() {
    let endorsed = endorsed_skills();
    // Spot-check representative NVIDIA CUDA-X skills sourced from the
    // official NVIDIA/skills catalog.
    for expected in [
        "cuopt-numerical-optimization-api-python",
        "cupynumeric-install",
        "accelerated-computing-cudf",
        "cudaq-guide",
        "tilegym-adding-cutile-kernel",
    ] {
        let skill = endorsed
            .iter()
            .find(|s| s.name == expected)
            .unwrap_or_else(|| panic!("expected endorsed NVIDIA skill {expected}"));
        assert_eq!(skill.category, "NVIDIA CUDA-X");
        assert!(
            skill
                .install
                .is_some_and(|cmd| cmd.contains("nvidia/skills")),
            "NVIDIA skill {expected} should have an nvidia/skills install hint"
        );
    }
}

#[test]
fn endorsed_skills_include_anthropic_frontend_design() {
    let skill = endorsed_skills()
        .iter()
        .find(|s| s.name == "frontend-design")
        .expect("expected endorsed Anthropic frontend-design skill");
    assert_eq!(skill.category, "Anthropic Design");
    assert!(
        skill.source.contains("anthropics/skills"),
        "frontend-design should be sourced from anthropics/skills"
    );
    assert!(
        skill
            .install
            .is_some_and(|cmd| cmd.contains("anthropics/skills")),
        "frontend-design should have an anthropics/skills install hint"
    );
}

#[test]
fn registry_contains_reports_loaded_skills() {
    let temp = tempfile::tempdir().expect("tempdir");
    write_test_skill(temp.path(), ".jcode", "present-skill");

    let registry = SkillRegistry::load_for_working_dir(Some(temp.path())).expect("load skills");
    assert!(registry.contains("present-skill"));
    assert!(!registry.contains("missing-skill"));
}

/// Write `SKILL.md` for `name` inside `<plugin_dir>/skills/<name>/`.
fn write_plugin_skill(plugin_dir: &Path, name: &str) {
    write_plugin_skill_with_description(plugin_dir, name, &format!("Plugin skill {name}"));
}

fn write_plugin_skill_with_description(plugin_dir: &Path, name: &str, description: &str) {
    let dir = plugin_dir.join("skills").join(name);
    std::fs::create_dir_all(&dir).expect("create plugin skill dir");
    std::fs::write(
        dir.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: {description}\n---\n\nUse {name}.\n"),
    )
    .expect("write plugin skill");
}

fn write_installed_plugins_manifest(plugins_root: &Path, install_paths: &[&Path]) {
    let plugins: serde_json::Map<String, serde_json::Value> = install_paths
        .iter()
        .enumerate()
        .map(|(i, path)| {
            (
                format!("plugin-{i}@test-marketplace"),
                serde_json::json!([{ "scope": "user", "installPath": path, "version": "1.0.0" }]),
            )
        })
        .collect();
    std::fs::write(
        plugins_root.join("installed_plugins.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "version": 2,
            "plugins": plugins,
        }))
        .expect("serialize manifest"),
    )
    .expect("write manifest");
}

#[test]
fn plugin_skills_load_from_installed_plugins_manifest() {
    let temp = tempfile::tempdir().expect("tempdir");
    let plugins_root = temp.path();

    // Mirror the real Claude Code layout:
    // cache/<marketplace>/<plugin>/<version>/skills/<skill>/SKILL.md
    let install = plugins_root.join("cache/test-marketplace/vercel/0.40.1");
    write_plugin_skill(&install, "ai-gateway");
    // Nested `.claude/skills` variant inside the same install.
    write_plugin_skill(&install.join(".claude"), "benchmark-agents");
    write_installed_plugins_manifest(plugins_root, &[&install]);

    let mut registry = SkillRegistry::default();
    let count = registry.load_plugin_skills_from_root(plugins_root);

    assert_eq!(count, 2);
    assert!(registry.contains("ai-gateway"));
    assert!(registry.contains("benchmark-agents"));
}

#[test]
fn plugin_skills_fall_back_to_cache_scan_without_manifest() {
    let temp = tempfile::tempdir().expect("tempdir");
    let plugins_root = temp.path();

    let install = plugins_root.join("cache/test-marketplace/my-plugin/1.0.0");
    write_plugin_skill(&install, "cache-skill");

    let mut registry = SkillRegistry::default();
    let count = registry.load_plugin_skills_from_root(plugins_root);

    assert_eq!(count, 1);
    assert!(registry.contains("cache-skill"));
}

#[test]
fn plugin_skills_load_from_repos_layout() {
    let temp = tempfile::tempdir().expect("tempdir");
    let plugins_root = temp.path();

    let repo = plugins_root.join("repos/owner/my-plugin");
    write_plugin_skill(&repo, "repo-skill");

    let mut registry = SkillRegistry::default();
    let count = registry.load_plugin_skills_from_root(plugins_root);

    assert_eq!(count, 1);
    assert!(registry.contains("repo-skill"));
}

#[test]
fn plugin_scan_skips_marketplace_catalog_when_manifest_exists() {
    let temp = tempfile::tempdir().expect("tempdir");
    let plugins_root = temp.path();

    // Installed plugin listed in the manifest.
    let install = plugins_root.join("cache/test-marketplace/installed/1.0.0");
    write_plugin_skill(&install, "installed-skill");
    write_installed_plugins_manifest(plugins_root, &[&install]);

    // Marketplace catalog entry the user never installed.
    write_plugin_skill(
        &plugins_root.join("marketplaces/test-marketplace/plugins/uninstalled"),
        "uninstalled-skill",
    );
    // Cache entry not referenced by the manifest (stale install).
    write_plugin_skill(
        &plugins_root.join("cache/test-marketplace/stale/0.1.0"),
        "stale-skill",
    );

    let mut registry = SkillRegistry::default();
    registry.load_plugin_skills_from_root(plugins_root);

    assert!(registry.contains("installed-skill"));
    assert!(
        !registry.contains("uninstalled-skill"),
        "marketplace catalog skills must not load"
    );
    assert!(
        !registry.contains("stale-skill"),
        "cache entries outside the manifest must not load when a manifest exists"
    );
}

#[test]
fn plugin_scan_respects_depth_bound() {
    let temp = tempfile::tempdir().expect("tempdir");
    let plugins_root = temp.path();

    // Deeper than PLUGIN_SCAN_MAX_DEPTH below the cache root.
    let too_deep = plugins_root.join("cache/a/b/c/d/e/f");
    write_plugin_skill(&too_deep, "too-deep-skill");

    let mut registry = SkillRegistry::default();
    let count = registry.load_plugin_skills_from_root(plugins_root);

    assert_eq!(count, 0);
    assert!(!registry.contains("too-deep-skill"));
}

#[test]
fn explicit_jcode_skill_wins_over_plugin_skill_with_same_name() {
    let temp = tempfile::tempdir().expect("tempdir");
    let plugins_root = temp.path().join("plugins");

    let install = plugins_root.join("cache/test-marketplace/my-plugin/1.0.0");
    write_plugin_skill_with_description(&install, "shared-name", "plugin version");

    // Explicit jcode skill with the same name.
    write_test_skill(temp.path(), ".jcode", "shared-name");

    // Mirror load ordering: plugins first, then explicit skill dirs, so
    // the later (explicit) insert wins in the registry map.
    let mut registry = SkillRegistry::default();
    registry.load_plugin_skills_from_root(&plugins_root);
    registry
        .load_from_dir(&temp.path().join(".jcode/skills"))
        .expect("load explicit skills");

    let skill = registry.get("shared-name").expect("skill present");
    assert_eq!(
        skill.description, "Test skill shared-name",
        "explicit jcode skill must override the plugin-provided one"
    );
}

#[test]
fn plugin_skill_dirs_empty_for_missing_root() {
    let temp = tempfile::tempdir().expect("tempdir");
    let missing = temp.path().join("does-not-exist");
    assert!(SkillRegistry::plugin_skill_dirs_under(&missing).is_empty());
}
