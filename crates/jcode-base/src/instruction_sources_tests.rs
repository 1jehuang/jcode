use super::*;

fn write(path: impl AsRef<Path>, text: &str) {
    let path = path.as_ref();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn id_for(working_dir: &Path, path: &Path) -> String {
    let canonical = std::fs::canonicalize(path).unwrap();
    choices(Some(working_dir), None, &[])
        .into_iter()
        .find(|source| source.path.as_deref() == Some(canonical.as_path()))
        .unwrap()
        .id
}

#[test]
fn profile_source_defaults_migrate_old_snapshots_to_isolation() {
    let profile: crate::agent_profile::AgentProfile = serde_json::from_value(serde_json::json!({
        "name": "writer", "instructions": "profile"
    }))
    .unwrap();
    assert!(profile.instruction_sources.is_empty());
    assert_eq!(
        serde_json::to_value(profile).unwrap(),
        serde_json::json!({
            "name": "writer", "instructions": "profile"
        })
    );
}

#[test]
fn profile_source_empty_allowlist_excludes_disk_files_but_keeps_harness() {
    let _home = crate::auth::test_sandbox::AuthTestSandbox::new().unwrap();
    let project = tempfile::tempdir().unwrap();
    let global = crate::storage::jcode_dir().unwrap();
    let home_agents = crate::storage::user_home_path("AGENTS.md").unwrap();
    write(&home_agents, "GLOBAL_AGENTS_SOURCE_SENTINEL");
    write(
        project.path().join("AGENTS.md"),
        "PROJECT_AGENTS_SOURCE_SENTINEL",
    );
    for (name, marker) in [
        ("system-prompt.md", "SYSTEM_SOURCE_SENTINEL"),
        ("prompt-overlay.md", "OVERLAY_SOURCE_SENTINEL"),
        ("preferred-tools.md", "TOOLS_SOURCE_SENTINEL"),
        ("swarm-prompt.md", "SWARM_SOURCE_SENTINEL"),
    ] {
        write(
            project.path().join(".jcode").join(name),
            &format!("PROJECT_{marker}"),
        );
        write(global.join(name), &format!("GLOBAL_{marker}"));
    }
    let normal =
        crate::prompt::build_system_prompt_split(None, &[], false, None, Some(project.path())).0;
    assert!(
        normal
            .static_part
            .contains("PROJECT_SYSTEM_SOURCE_SENTINEL")
    );
    assert!(normal.static_part.contains("GLOBAL_AGENTS_SOURCE_SENTINEL"));
    assert!(
        normal
            .static_part
            .contains("GLOBAL_OVERLAY_SOURCE_SENTINEL")
    );
    let (isolated, info) = crate::prompt::build_system_prompt_split_with_instruction_sources(
        &[],
        false,
        Some("MEMORY_CONTEXT_RETAINED"),
        Some(project.path()),
        &[],
    );
    assert!(isolated.static_part.contains("You are Jcode."));
    assert!(!isolated.static_part.contains("SOURCE_SENTINEL"));
    assert!(isolated.dynamic_part.contains("MEMORY_CONTEXT_RETAINED"));
    assert!(!info.has_project_agents_md && !info.has_global_agents_md);
}

#[test]
fn profile_sources_select_individual_files_and_freeze_content_across_cwd_changes() {
    let _home = crate::auth::test_sandbox::AuthTestSandbox::new().unwrap();
    let project = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    let local = project.path().join(".jcode/prompt-overlay.md");
    let global = crate::storage::jcode_dir()
        .unwrap()
        .join("prompt-overlay.md");
    write(&local, "LOCAL_UNSELECTED");
    write(&global, "GLOBAL_SELECTED");
    let id = id_for(project.path(), &global);
    let sources = select(
        Some(project.path()),
        None,
        &[],
        &serde_json::to_string(&[&id]).unwrap(),
    )
    .unwrap();
    assert_eq!(sources.len(), 1);
    write(&global, "GLOBAL_CHANGED_AFTER_SELECTION");
    write(
        other.path().join(".jcode/prompt-overlay.md"),
        "NEW_CWD_UNSELECTED",
    );
    let prompt = crate::prompt::build_system_prompt_split_with_instruction_sources(
        &[],
        false,
        None,
        Some(other.path()),
        &sources,
    )
    .0;
    assert!(prompt.static_part.contains("GLOBAL_SELECTED"));
    for excluded in [
        "LOCAL_UNSELECTED",
        "GLOBAL_CHANGED_AFTER_SELECTION",
        "NEW_CWD_UNSELECTED",
    ] {
        assert!(!prompt.static_part.contains(excluded));
    }
    let refreshed = select(
        Some(project.path()),
        None,
        &sources,
        &serde_json::to_string(&[id]).unwrap(),
    )
    .unwrap();
    assert_eq!(refreshed[0].content, "GLOBAL_CHANGED_AFTER_SELECTION");
}

#[test]
fn profile_sources_reject_malformed_unknown_duplicate_and_unavailable_choices() {
    let _home = crate::auth::test_sandbox::AuthTestSandbox::new().unwrap();
    let project = tempfile::tempdir().unwrap();
    let path = project.path().join("AGENTS.md");
    write(&path, "VALID");
    let id = id_for(project.path(), &path);
    for encoded in [
        "not-json",
        "{}",
        "null",
        "[1]",
        "[\"file:/arbitrary-secret\"]",
    ] {
        assert!(
            select(Some(project.path()), None, &[], encoded).is_err(),
            "{encoded}"
        );
    }
    let repeated = serde_json::to_string(&[&id, &id]).unwrap();
    assert!(select(Some(project.path()), None, &[], &repeated).is_err());
    let missing = choices(Some(project.path()), None, &[])
        .into_iter()
        .find(|source| !source.available)
        .unwrap();
    assert!(
        select(
            Some(project.path()),
            None,
            &[],
            &serde_json::to_string(&[missing.id]).unwrap()
        )
        .is_err()
    );
    let selected = select(
        Some(project.path()),
        None,
        &[],
        &serde_json::to_string(&[&id]).unwrap(),
    )
    .unwrap();
    std::fs::remove_file(path).unwrap();
    let retained = select(
        Some(project.path()),
        None,
        &selected,
        &serde_json::to_string(&[id]).unwrap(),
    )
    .unwrap();
    assert_eq!(retained, selected);
    assert!(
        choices(Some(project.path()), None, &retained)
            .iter()
            .any(|source| source.included && !source.available)
    );
    assert!(
        select(Some(project.path()), None, &retained, "[]")
            .unwrap()
            .is_empty()
    );
}

#[test]
fn profile_sources_keep_agents_context_accounting_and_do_not_scan_ancestors() {
    let _home = crate::auth::test_sandbox::AuthTestSandbox::new().unwrap();
    let project = tempfile::tempdir().unwrap();
    let cwd = project.path().join("child");
    std::fs::create_dir_all(&cwd).unwrap();
    write(project.path().join("AGENTS.md"), "ANCESTOR_NOT_DISCOVERED");
    let local = cwd.join("AGENTS.md");
    write(&local, "PROJECT_CHOSEN");
    let global = crate::storage::user_home_path("AGENTS.md").unwrap();
    write(&global, "GLOBAL_CHOSEN");
    let choices = choices(Some(&cwd), None, &[]);
    assert!(
        !choices.iter().any(
            |source| source.path.as_deref() == Some(project.path().join("AGENTS.md").as_path())
        )
    );
    let ids: Vec<_> = choices
        .iter()
        .filter(|source| source.kind == InstructionSourceKind::AgentsMd && source.available)
        .map(|source| source.id.clone())
        .collect();
    let selected = select(Some(&cwd), None, &[], &serde_json::to_string(&ids).unwrap()).unwrap();
    let (prompt, info) = crate::prompt::build_system_prompt_split_with_instruction_sources(
        &[],
        false,
        None,
        Some(&cwd),
        &selected,
    );
    assert!(prompt.static_part.contains("PROJECT_CHOSEN"));
    assert!(prompt.static_part.contains("GLOBAL_CHOSEN"));
    assert!(!prompt.static_part.contains("ANCESTOR_NOT_DISCOVERED"));
    assert!(info.has_project_agents_md && info.has_global_agents_md);
    assert_eq!(info.project_agents_md_chars, "PROJECT_CHOSEN".len());
    assert_eq!(info.global_agents_md_chars, "GLOBAL_CHOSEN".len());
}

#[cfg(unix)]
#[test]
fn profile_sources_canonical_alias_dedup_and_retained_replacement_precedence() {
    let _home = crate::auth::test_sandbox::AuthTestSandbox::new().unwrap();
    let project = tempfile::tempdir().unwrap();
    let target = project.path().join("external-project-source.md");
    let local = project.path().join(".jcode/system-prompt.md");
    let global = crate::storage::jcode_dir()
        .unwrap()
        .join("system-prompt.md");
    write(&target, "PROJECT_REPLACEMENT");
    write(&global, "GLOBAL_REPLACEMENT");
    std::fs::create_dir_all(local.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&target, &local).unwrap();
    let ids = vec![
        id_for(project.path(), &global),
        id_for(project.path(), &local),
    ];
    let selected = select(
        Some(project.path()),
        None,
        &[],
        &serde_json::to_string(&ids).unwrap(),
    )
    .unwrap();
    assert_eq!(
        first_content(&selected, InstructionSourceKind::SystemPrompt),
        Some("PROJECT_REPLACEMENT")
    );
    std::fs::remove_file(&local).unwrap();
    std::os::unix::fs::symlink(&global, &local).unwrap();
    let retained = select(
        Some(project.path()),
        None,
        &selected,
        &serde_json::to_string(&ids).unwrap(),
    )
    .unwrap();
    assert_eq!(
        first_content(&retained, InstructionSourceKind::SystemPrompt),
        Some("PROJECT_REPLACEMENT")
    );
    let now = choices(Some(project.path()), None, &retained);
    assert_eq!(
        now.iter()
            .filter(|source| source.kind == InstructionSourceKind::SystemPrompt && source.available)
            .count(),
        1
    );
    assert!(
        now.iter()
            .any(|source| source.included && !source.available)
    );
}

#[test]
fn profile_sources_preserve_explicit_empty_session_prompt() {
    let _home = crate::auth::test_sandbox::AuthTestSandbox::new().unwrap();
    let selected = select(None, Some(""), &[], "[\"session:system_prompt\"]").unwrap();
    assert_eq!(selected.len(), 1);
    assert_eq!(selected[0].content, "");
    assert!(
        choices(None, Some(""), &selected)
            .iter()
            .any(|source| source.id == "session:system_prompt"
                && source.available
                && source.included)
    );
}
