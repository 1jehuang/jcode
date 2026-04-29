use super::*;

/// Verify the default system prompt does NOT identify as "Claude Code"
/// It's fine to say "powered by Claude" but not "Claude Code" (Anthropic's product)
#[test]
fn test_default_system_prompt_no_claude_code_identity() {
    let prompt = DEFAULT_SYSTEM_PROMPT.to_lowercase();

    assert!(
        !prompt.contains("claude code"),
        "DEFAULT_SYSTEM_PROMPT should NOT identify as 'Claude Code'. Found in system.md"
    );
    assert!(
        !prompt.contains("claude-code"),
        "DEFAULT_SYSTEM_PROMPT should NOT contain 'claude-code'. Found in system.md"
    );
}

/// Verify skill prompts don't accidentally introduce "Claude Code" identity
#[test]
fn test_skill_prompt_integration() {
    // Test that a skill prompt is properly appended and doesn't break anything
    let skill_prompt = "You are helping with a debugging task.";
    let prompt = build_system_prompt(Some(skill_prompt), &[]);

    // The prompt should contain our default system prompt
    assert!(prompt.contains("You are the Jcode Agent"));

    // The prompt should contain the skill prompt
    assert!(prompt.contains(skill_prompt));

    // The base prompt parts (excluding user-provided instruction files) should NOT contain
    // "Claude Code". We check DEFAULT_SYSTEM_PROMPT separately since user files may
    // legitimately contain it.
    let default_lower = DEFAULT_SYSTEM_PROMPT.to_lowercase();
    assert!(
        !default_lower.contains("claude code"),
        "DEFAULT_SYSTEM_PROMPT should NOT identify as 'Claude Code'"
    );
}

#[test]
fn test_load_agents_md_files_uses_sandboxed_global_files() {
    let _guard = crate::storage::lock_test_env();
    let prev_home = std::env::var_os("JCODE_HOME");
    let temp = tempfile::TempDir::new().unwrap();
    crate::env::set_var("JCODE_HOME", temp.path());
    std::fs::create_dir_all(temp.path().join("external")).unwrap();

    std::fs::write(
        temp.path().join("external/AGENTS.md"),
        "sandboxed global agents instructions",
    )
    .unwrap();

    let project_dir = tempfile::TempDir::new().unwrap();
    let (content, info) = load_agents_md_files_from_dir(Some(project_dir.path()));

    assert!(info.has_global_agents_md);
    let content = content.expect("global instructions content");
    assert!(content.contains("sandboxed global agents instructions"));

    if let Some(prev_home) = prev_home {
        crate::env::set_var("JCODE_HOME", prev_home);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
}

#[test]
fn test_dynamic_system_prompt_includes_time_and_timezone() {
    let (split, _info) = build_system_prompt_split(None, &[], false, None, None);
    assert!(split.dynamic_part.contains("Time: "));
    assert!(split.dynamic_part.contains("Timezone: UTC"));
}

#[test]
fn test_prompt_overlay_files_are_loaded_from_project_and_global_jcode_dirs() {
    let _guard = crate::storage::lock_test_env();
    let prev_home = std::env::var_os("JCODE_HOME");
    let temp = tempfile::TempDir::new().unwrap();
    crate::env::set_var("JCODE_HOME", temp.path());
    std::fs::create_dir_all(temp.path()).unwrap();
    std::fs::write(
        temp.path().join("prompt-overlay.md"),
        "global prompt overlay instructions",
    )
    .unwrap();

    let project_dir = tempfile::TempDir::new().unwrap();
    std::fs::create_dir_all(project_dir.path().join(".jcode")).unwrap();
    std::fs::write(
        project_dir.path().join(".jcode/prompt-overlay.md"),
        "project prompt overlay instructions",
    )
    .unwrap();

    let direct = load_prompt_overlay_files_from_dir(Some(project_dir.path()));

    assert!(direct.0.is_some(), "expected prompt overlay content");
    let direct_content = direct.0.unwrap();
    assert!(
        direct_content.contains("project prompt overlay instructions"),
        "expected project prompt overlay content"
    );
    assert!(
        direct_content.contains("global prompt overlay instructions"),
        "expected global prompt overlay content"
    );

    let (prompt, info) = build_system_prompt_full(None, &[], false, None, Some(project_dir.path()));
    assert!(prompt.contains("project prompt overlay instructions"));
    assert!(prompt.contains("global prompt overlay instructions"));
    assert!(info.prompt_overlay_chars > 0);

    if let Some(prev_home) = prev_home {
        crate::env::set_var("JCODE_HOME", prev_home);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
}

#[test]
fn test_non_selfdev_prompt_includes_lightweight_selfdev_hint() {
    let prompt = build_system_prompt(None, &[]);
    assert!(prompt.contains("Self-Development Access"));
    assert!(prompt.contains("`selfdev`"));
    assert!(prompt.contains("selfdev enter"));
    assert!(!prompt.contains("You are running in self-dev mode"));
}

#[test]
fn test_selfdev_prompt_uses_full_selfdev_instructions() {
    let prompt = build_system_prompt_with_selfdev(None, &[], true);
    assert!(prompt.contains("You are working on the jcode codebase itself."));
    assert!(!prompt.contains("Self-Development Access"));
}

#[test]
fn test_selfdev_prompt_prefers_publish_flow_for_active_builds() {
    let prompt = build_system_prompt_with_selfdev(None, &[], true);
    assert!(prompt.contains("selfdev build"));
    assert!(prompt.contains("cancel-build"));
    assert!(prompt.contains("selfdev reload"));
    assert!(prompt.contains("fallback when `selfdev build` is not appropriate"));
    assert!(prompt.contains("scripts/dev_cargo.sh build --profile selfdev -p jcode --bin jcode"));
    assert!(prompt.contains("remote build host is configured"));
    assert!(prompt.contains("Do not wait for user input"));
}

#[test]
fn test_selfdev_prompt_template_placeholders_are_resolved() {
    let static_prompt = build_selfdev_prompt_static();
    let dynamic_prompt = build_selfdev_prompt();
    assert!(!static_prompt.contains("__DEBUG_SOCKET_BLOCK__"));
    assert!(!dynamic_prompt.contains("__DEBUG_SOCKET_BLOCK__"));
    assert_eq!(static_prompt, dynamic_prompt);
}

#[test]
fn split_prompt_estimated_tokens_is_positive_when_populated() {
    let (split, _info) = build_system_prompt_split(None, &[], false, None, None);
    assert!(split.chars() > 0);
    assert!(split.estimated_tokens() > 0);
}
