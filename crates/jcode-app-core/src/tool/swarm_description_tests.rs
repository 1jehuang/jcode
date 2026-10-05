//! Tests for swarm-prompt embedding in the tool description.

use super::*;

/// The shared tool holds only the base description: the swarm prompt is
/// per-session data and the registry is built before any session's working
/// directory is known (P2.2).
#[test]
fn the_shared_tool_carries_only_the_base_description() {
    let tool = CommunicateTool::new();
    let description = tool.description();
    assert!(
        description.starts_with("Coordinate agents"),
        "description should lead with the short coordination summary"
    );
    assert!(
        !description.contains("Swarm prompt"),
        "the shared tool must not bake in any project's swarm prompt: {description:?}"
    );
}

/// [`CommunicateTool::description_for`] is what embeds the prompt, and it
/// resolves against the session's working directory.
#[test]
fn description_for_a_session_embeds_that_sessions_swarm_prompt() {
    let project = tempfile::tempdir().unwrap();
    let prompt_dir = project.path().join(".jcode");
    std::fs::create_dir_all(&prompt_dir).unwrap();
    std::fs::write(prompt_dir.join("swarm-prompt.md"), "route reviews to opus").unwrap();

    let description = CommunicateTool::description_for(Some(project.path()));
    assert!(
        description.starts_with("Coordinate agents"),
        "description should lead with the short coordination summary"
    );
    assert!(
        description.contains("Swarm prompt") && description.contains("route reviews to opus"),
        "the session's own swarm prompt should be embedded: {description:?}"
    );
}

/// The swarm prompt is resolved when a session's tool definitions are built,
/// not when the shared tool is constructed (P2.2). An already-built
/// description therefore keeps the version it was built with, and rebuilding
/// picks up an edit without a daemon restart.
#[test]
fn existing_description_keeps_prompt_while_a_rebuild_loads_edit() {
    let project = tempfile::tempdir().unwrap();
    let prompt_dir = project.path().join(".jcode");
    std::fs::create_dir_all(&prompt_dir).unwrap();
    let prompt_path = prompt_dir.join("swarm-prompt.md");
    std::fs::write(&prompt_path, "first routing version").unwrap();

    let existing = CommunicateTool::description_for(Some(project.path()));
    std::fs::write(&prompt_path, "second routing version").unwrap();
    let rebuilt = CommunicateTool::description_for(Some(project.path()));

    assert!(existing.contains("first routing version"));
    assert!(!existing.contains("second routing version"));
    assert!(rebuilt.contains("second routing version"));
}

/// A shared `CommunicateTool` holds only the base description. The project
/// prompt is per-session data, so a registry shared across projects must not
/// bake one project's prompt into every session's definitions.
#[test]
fn shared_tool_description_carries_no_project_prompt() {
    let tool = CommunicateTool::new();
    let description = tool.description();
    assert!(description.starts_with("Coordinate agents"));
    assert!(
        !description.contains("Swarm prompt"),
        "the shared tool description must not embed a project swarm prompt: {:?}",
        description
    );
}

/// P2.2: with no working directory the description resolves no project
/// prompt, even when the process cwd is a repo that has one.
#[test]
fn description_with_no_working_dir_ignores_the_daemon_cwd_prompt() {
    let project = tempfile::tempdir().unwrap();
    let prompt_dir = project.path().join(".jcode");
    std::fs::create_dir_all(&prompt_dir).unwrap();
    std::fs::write(
        prompt_dir.join("swarm-prompt.md"),
        "daemon started project routing",
    )
    .unwrap();

    let prev_cwd = std::env::current_dir().unwrap();
    std::env::set_current_dir(project.path()).unwrap();
    let without_working_dir = CommunicateTool::description_for(None);
    let with_working_dir = CommunicateTool::description_for(Some(project.path()));
    std::env::set_current_dir(prev_cwd).unwrap();

    assert!(
        !without_working_dir.contains("daemon started project routing"),
        "a session with no working dir picked up the launching project's prompt: {:?}",
        without_working_dir
    );
    // Positive control: the prompt is readable and does show up when the
    // session names that project, so the assertion above is not vacuous.
    assert!(
        with_working_dir.contains("daemon started project routing"),
        "a session rooted at the project must see that project's prompt"
    );
}
