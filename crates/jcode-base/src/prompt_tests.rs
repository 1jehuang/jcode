use super::*;

/// Verify the default system prompt does NOT identify as "Claude Code"
/// It's fine to say "powered by Claude" but not "Claude Code" (Anthropic's product)
#[test]
fn test_default_system_prompt_no_claude_code_identity() {
    let prompt = DEFAULT_SYSTEM_PROMPT.to_lowercase();

    assert!(
        !prompt.contains("claude code"),
        "DEFAULT_SYSTEM_PROMPT should NOT identify as 'Claude Code'. Found in system_prompt.md"
    );
    assert!(
        !prompt.contains("claude-code"),
        "DEFAULT_SYSTEM_PROMPT should NOT contain 'claude-code'. Found in system_prompt.md"
    );
}

#[test]
fn mermaid_prompt_module_follows_capability() {
    let (enabled, _) = build_system_prompt_split_with_capabilities(
        None,
        &[],
        false,
        None,
        None,
        PromptCapabilities { mermaid: true },
    );
    assert!(enabled.static_part.contains(MERMAID_PROMPT));

    let (disabled, _) = build_system_prompt_split_with_capabilities(
        None,
        &[],
        false,
        None,
        None,
        PromptCapabilities { mermaid: false },
    );
    assert!(!disabled.static_part.contains("Mermaid diagrams"));
    assert!(!disabled.static_part.contains("fenced `mermaid` code block"));
}

/// Verify skill prompts don't accidentally introduce "Claude Code" identity
#[test]
fn test_skill_prompt_integration() {
    // Test that a skill prompt is properly appended and doesn't break anything
    let skill_prompt = "You are helping with a debugging task.";
    let prompt = build_system_prompt(Some(skill_prompt), &[]);

    // The prompt should contain our default system prompt
    assert!(prompt.contains("Your name is Jcode."));

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
    assert!(content.contains("# Global Instructions (~/AGENTS.md)"));
    assert!(!content.contains("~/.AGENTS.md"));
    assert!(content.contains("sandboxed global agents instructions"));

    if let Some(prev_home) = prev_home {
        crate::env::set_var("JCODE_HOME", prev_home);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
}

#[test]
fn test_session_context_includes_time_timezone_and_system_info() {
    let context = build_session_context(None);
    assert!(context.contains("# Session Context"));
    assert!(context.contains("Time: "));
    assert!(context.contains("Timezone: UTC"));
    assert!(context.contains("OS: "));
    assert!(context.contains("Architecture: "));
    assert!(context.contains("Jcode version: "));
    assert!(!context.contains("Working directory: "));
    assert!(!context.contains("Git:"));
}

#[test]
fn test_split_prompt_does_not_inject_session_context_per_turn() {
    let (split, _info) = build_system_prompt_split(None, &[], false, None, None);
    assert!(!split.dynamic_part.contains("# Session Context"));
    assert!(!split.dynamic_part.contains("Time: "));
    assert!(!split.dynamic_part.contains("Timezone: UTC"));
}

#[test]
fn sponsored_discovery_is_not_injected_into_the_system_prompt() {
    let (split, _) = build_system_prompt_split(None, &[], false, None, None);
    assert!(!split.static_part.contains("Discoverable Tools"));
    assert!(!split.static_part.contains("discover_tools"));
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
fn test_preferred_tools_files_are_loaded_from_project_and_global_jcode_dirs() {
    let _guard = crate::storage::lock_test_env();
    let prev_home = std::env::var_os("JCODE_HOME");
    let temp = tempfile::TempDir::new().unwrap();
    crate::env::set_var("JCODE_HOME", temp.path());
    std::fs::create_dir_all(temp.path()).unwrap();
    std::fs::write(
        temp.path().join("preferred-tools.md"),
        "global preferred tools instructions",
    )
    .unwrap();

    let project_dir = tempfile::TempDir::new().unwrap();
    std::fs::create_dir_all(project_dir.path().join(".jcode")).unwrap();
    std::fs::write(
        project_dir.path().join(".jcode/preferred-tools.md"),
        "project preferred tools instructions",
    )
    .unwrap();

    let direct = load_preferred_tools_files_from_dir(Some(project_dir.path()));

    assert!(direct.0.is_some(), "expected preferred tools content");
    let direct_content = direct.0.unwrap();
    assert!(
        direct_content.contains("Project Preferred Tools (.jcode/preferred-tools.md)"),
        "expected project preferred tools section heading"
    );
    assert!(
        direct_content.contains("project preferred tools instructions"),
        "expected project preferred tools content"
    );
    assert!(
        direct_content.contains("Global Preferred Tools (~/.jcode/preferred-tools.md)"),
        "expected global preferred tools section heading"
    );
    assert!(
        direct_content.contains("global preferred tools instructions"),
        "expected global preferred tools content"
    );

    let (prompt, info) = build_system_prompt_full(None, &[], false, None, Some(project_dir.path()));
    assert!(prompt.contains("project preferred tools instructions"));
    assert!(prompt.contains("global preferred tools instructions"));
    assert!(info.preferred_tools_chars > 0);

    let (split, split_info) =
        build_system_prompt_split(None, &[], false, None, Some(project_dir.path()));
    assert!(
        split
            .static_part
            .contains("project preferred tools instructions")
    );
    assert!(
        split
            .static_part
            .contains("global preferred tools instructions")
    );
    assert!(split_info.preferred_tools_chars > 0);

    if let Some(prev_home) = prev_home {
        crate::env::set_var("JCODE_HOME", prev_home);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
}

#[test]
fn test_swarm_prompt_prefers_project_then_global_then_default() {
    let _guard = crate::storage::lock_test_env();
    let prev_home = std::env::var_os("JCODE_HOME");
    let temp = tempfile::TempDir::new().unwrap();
    crate::env::set_var("JCODE_HOME", temp.path());
    std::fs::create_dir_all(temp.path()).unwrap();

    let project_dir = tempfile::TempDir::new().unwrap();

    // No override files: built-in default.
    let prompt = load_swarm_prompt(Some(project_dir.path()));
    assert_eq!(prompt, DEFAULT_SWARM_PROMPT.trim());

    // Global override wins over the default.
    std::fs::write(temp.path().join("swarm-prompt.md"), "global swarm routing").unwrap();
    let prompt = load_swarm_prompt(Some(project_dir.path()));
    assert_eq!(prompt, "global swarm routing");

    // Project override wins over global.
    std::fs::create_dir_all(project_dir.path().join(".jcode")).unwrap();
    std::fs::write(
        project_dir.path().join(".jcode/swarm-prompt.md"),
        "project swarm routing",
    )
    .unwrap();
    let prompt = load_swarm_prompt(Some(project_dir.path()));
    assert_eq!(prompt, "project swarm routing");

    // A blank project file falls through to global instead of going empty.
    std::fs::write(project_dir.path().join(".jcode/swarm-prompt.md"), "   \n").unwrap();
    let prompt = load_swarm_prompt(Some(project_dir.path()));
    assert_eq!(prompt, "global swarm routing");

    if let Some(prev_home) = prev_home {
        crate::env::set_var("JCODE_HOME", prev_home);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
}

#[test]
fn test_default_swarm_prompt_mentions_model_and_list_models() {
    assert!(DEFAULT_SWARM_PROMPT.contains("list_models"));
    assert!(DEFAULT_SWARM_PROMPT.contains("model"));
    assert!(DEFAULT_SWARM_PROMPT.contains("effort"));
}

#[test]
fn test_non_selfdev_prompt_leaves_selfdev_guidance_to_the_tool_schema() {
    let prompt = build_system_prompt(None, &[]);
    assert!(!prompt.contains("Self-Development Access"));
    assert!(!prompt.contains("You have access to the `selfdev` tool in all sessions"));
    assert!(!prompt.contains("You are working on the jcode codebase itself."));
}

#[test]
fn test_selfdev_prompt_uses_full_selfdev_instructions() {
    let prompt = build_system_prompt_with_selfdev(None, &[], true);
    assert!(prompt.contains("You are working on the jcode codebase itself."));
    assert!(prompt.contains("launched from the TUI/root jcode context"));
    assert!(prompt.contains("selfdev build target=tui"));
    assert!(!prompt.contains("Self-Development Access"));
}

#[test]
fn test_selfdev_prompt_uses_desktop_focus_for_desktop_working_dir() {
    let desktop_dir = std::path::Path::new("/tmp/jcode/crates/jcode-desktop/src");
    let (prompt, _info) = build_system_prompt_full(None, &[], true, None, Some(desktop_dir));
    assert!(prompt.contains("launched from the desktop app context"));
    assert!(prompt.contains("selfdev build target=desktop"));
    assert!(!prompt.contains("launched from the TUI/root jcode context"));
}

#[test]
fn test_split_selfdev_prompt_defaults_to_tui_focus_for_repo_root() {
    let repo_dir = std::path::Path::new("/tmp/jcode");
    let (split, _info) = build_system_prompt_split(None, &[], true, None, Some(repo_dir));
    assert!(
        split
            .static_part
            .contains("launched from the TUI/root jcode context")
    );
    assert!(split.static_part.contains("selfdev build target=tui"));
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
    assert!(!static_prompt.contains("__SELFDEV_PRODUCT_FOCUS__"));
    assert!(!dynamic_prompt.contains("__SELFDEV_PRODUCT_FOCUS__"));
    assert_eq!(static_prompt, dynamic_prompt);
}

#[test]
fn split_prompt_estimated_tokens_is_positive_when_populated() {
    let (split, _info) = build_system_prompt_split(None, &[], false, None, None);
    assert!(split.chars() > 0);
    assert!(split.estimated_tokens() > 0);
}

#[test]
fn swarm_effort_directive_is_appended_only_for_swarm_sentinel() {
    assert!(is_swarm_effort("swarm"));
    assert!(is_swarm_effort("  Swarm "));
    assert!(!is_swarm_effort("xhigh"));

    let mut split = SplitSystemPrompt {
        static_part: "base".to_string(),
        dynamic_part: String::new(),
    };
    append_swarm_effort_directive(&mut split, Some("xhigh"));
    assert!(!split.dynamic_part.contains("Swarm Effort"));

    append_swarm_effort_directive(&mut split, Some("swarm"));
    assert!(split.dynamic_part.contains("# Swarm Effort"));
    assert!(split.dynamic_part.contains("swarm` tool"));

    // None / empty effort should not inject.
    let mut other = SplitSystemPrompt::default();
    append_swarm_effort_directive(&mut other, None);
    assert!(other.dynamic_part.is_empty());
}

#[test]
fn swarm_deep_effort_injects_task_graph_directive() {
    use crate::prompt::is_deep_swarm_effort;

    assert!(is_swarm_effort("swarm-deep"));
    assert!(is_deep_swarm_effort("swarm-deep"));
    assert!(is_deep_swarm_effort("  Swarm-Deep "));
    assert!(!is_deep_swarm_effort("swarm"));
    assert!(!is_deep_swarm_effort("xhigh"));

    // Deep sentinel injects the DAG-first task-graph directive, not the light one.
    let mut split = SplitSystemPrompt::default();
    append_swarm_effort_directive(&mut split, Some("swarm-deep"));
    assert!(split.dynamic_part.contains("# Deep Task Graph"));
    assert!(split.dynamic_part.contains("swarm task_graph"));
    assert!(!split.dynamic_part.contains("# Swarm Effort"));

    // Light sentinel still injects the fan-out directive, not the deep one.
    let mut light = SplitSystemPrompt::default();
    append_swarm_effort_directive(&mut light, Some("swarm"));
    assert!(light.dynamic_part.contains("# Swarm Effort"));
    assert!(!light.dynamic_part.contains("# Deep Task Graph"));
}

#[test]
fn classify_effort_distinguishes_reasoning_from_swarm_modes() {
    use crate::prompt::{EffortKind, classify_effort, is_swarm_mode_effort};

    // Plain reasoning levels are not swarm modes.
    for level in ["none", "minimal", "low", "medium", "high", "xhigh", "max"] {
        assert_eq!(classify_effort(level), EffortKind::Reasoning, "{level}");
        assert!(!is_swarm_mode_effort(level), "{level}");
    }

    assert_eq!(classify_effort("swarm"), EffortKind::SwarmLight);
    assert_eq!(classify_effort("swarm-deep"), EffortKind::SwarmDeep);
    assert!(is_swarm_mode_effort("swarm"));
    assert!(is_swarm_mode_effort("  Swarm-Deep "));
    assert!(EffortKind::SwarmLight.is_swarm_mode());
    assert!(EffortKind::SwarmDeep.is_swarm_mode());
    assert!(!EffortKind::Reasoning.is_swarm_mode());
}
