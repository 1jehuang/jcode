//! Integration test: load the bundled sample agents in
//! `<project>/.jcode/agents/` and assert the registry behaves as
//! documented.
//!
//! Lives in `tests/` so it exercises the public API the way real callers
//! will (the `jcode` binary, the future `cli/agents` module, etc.).
//!
//! If a future PR moves the sample agents elsewhere, update `SAMPLES_DIR`.

use std::path::PathBuf;

use jcode_agent_runtime::{
    AgentRegistry, ModelTier, OutputMode, ReasoningEffort, SourceKind,
};

/// Path to the project-root sample agents directory, relative to the
/// crate manifest. Deliberately constructed via `CARGO_MANIFEST_DIR` so
/// `cargo test --workspace` works regardless of the cwd the runner
/// chooses.
fn samples_dir() -> PathBuf {
    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    // crates/jcode-agent-runtime → ../../ .jcode/agents
    crate_dir.parent().unwrap().parent().unwrap().join(".jcode/agents")
}

#[test]
fn loads_bundled_sample_agents() {
    let dir = samples_dir();
    assert!(
        dir.exists(),
        "sample agents directory missing: {}",
        dir.display(),
    );

    let mut reg = AgentRegistry::new();
    let n = reg
        .load_directory(&dir, SourceKind::ProjectLocal)
        .expect("load_directory");
    assert!(n >= 2, "expected at least 2 sample agents, got {n}");
    assert!(reg.load_errors().is_empty(), "load errors: {:?}", reg.load_errors());
}

#[test]
fn file_picker_sample_has_expected_shape() {
    let dir = samples_dir();
    let mut reg = AgentRegistry::new();
    reg.load_directory(&dir, SourceKind::ProjectLocal)
        .expect("load_directory");

    let agent = reg
        .get("file-picker")
        .expect("file-picker registered")
        .definition
        .clone();

    assert_eq!(agent.display_name, "Fletcher the File Fetcher");
    assert_eq!(agent.prefer_tier, Some(ModelTier::Routine));
    assert_eq!(agent.reasoning, Some(ReasoningEffort::Minimal));
    assert!(!agent.include_message_history, "file picker uses clean slate");
    assert!(!agent.inherit_parent_system_prompt);
    assert_eq!(agent.output_mode, OutputMode::LastMessage);
    assert!(agent.tool_names.iter().any(|t| t == "read"));
    assert!(agent.spawnable_agents.is_empty(), "leaf agent");

    // Resolve model with no env vars set should fall back to the
    // session's current model.
    let resolved = agent.resolve_model("session-model");
    assert_eq!(
        resolved, "session-model",
        "no JCODE_ROUTING_ROUTINE → session default"
    );
}

#[test]
fn code_reviewer_uses_inherit_parent_system_prompt_for_cache_hit() {
    let dir = samples_dir();
    let mut reg = AgentRegistry::new();
    reg.load_directory(&dir, SourceKind::ProjectLocal)
        .expect("load_directory");

    let agent = &reg
        .get("code-reviewer")
        .expect("code-reviewer registered")
        .definition;

    assert!(
        agent.inherit_parent_system_prompt,
        "reviewer must inherit parent system prompt for prompt-cache hits"
    );
    assert!(
        agent.system_prompt.is_empty(),
        "system_prompt must be empty when inheriting (enforced by validation)"
    );
    assert!(
        agent.include_message_history,
        "reviewer needs context of the change it's reviewing"
    );
    assert_eq!(agent.prefer_tier, Some(ModelTier::Thinking));
}

#[test]
fn sample_agents_validate_cleanly() {
    let dir = samples_dir();
    let mut reg = AgentRegistry::new();
    reg.load_directory(&dir, SourceKind::ProjectLocal)
        .expect("load_directory");

    for loaded in reg.iter() {
        loaded
            .definition
            .validate()
            .unwrap_or_else(|err| panic!("{} failed validation: {err}", loaded.definition.id));
    }
}

#[test]
fn basher_sample_has_expected_shape() {
    let dir = samples_dir();
    let mut reg = AgentRegistry::new();
    reg.load_directory(&dir, SourceKind::ProjectLocal)
        .expect("load_directory");

    let agent = reg
        .get("basher")
        .expect("basher registered")
        .definition
        .clone();

    assert_eq!(agent.id, "basher");
    assert_eq!(agent.display_name, "Basher");
    assert_eq!(agent.prefer_tier, Some(ModelTier::Routine));
    assert_eq!(agent.reasoning, Some(ReasoningEffort::Minimal));
    assert!(
        !agent.include_message_history,
        "basher uses a clean slate per command"
    );
    assert!(
        !agent.inherit_parent_system_prompt,
        "basher has its own short system prompt"
    );
    assert_eq!(agent.output_mode, OutputMode::LastMessage);
    assert_eq!(agent.tool_names, vec!["bash"]);
    assert!(agent.spawnable_agents.is_empty(), "leaf agent");

    // No tier env var set → resolve falls back to the session model.
    let resolved = agent.resolve_model("session-model");
    assert_eq!(
        resolved, "session-model",
        "no JCODE_ROUTING_ROUTINE → session default"
    );
}

#[test]
fn editor_sample_has_expected_shape() {
    let dir = samples_dir();
    let mut reg = AgentRegistry::new();
    reg.load_directory(&dir, SourceKind::ProjectLocal)
        .expect("load_directory");

    let agent = reg
        .get("editor")
        .expect("editor registered")
        .definition
        .clone();

    assert_eq!(agent.id, "editor");
    assert_eq!(agent.display_name, "Code Editor");
    assert_eq!(agent.prefer_tier, Some(ModelTier::Thinking));
    assert_eq!(agent.reasoning, Some(ReasoningEffort::Medium));
    assert!(
        agent.include_message_history,
        "editor needs to see what the user asked for"
    );
    assert!(
        agent.inherit_parent_system_prompt,
        "editor must inherit parent system prompt for prompt-cache hits"
    );
    assert!(
        agent.system_prompt.is_empty(),
        "system_prompt must be empty when inheriting (enforced by validation)"
    );
    assert_eq!(agent.output_mode, OutputMode::AllMessages);
    for expected in [
        "read",
        "str_replace",
        "write",
        "edit",
        "multiedit",
        "apply_patch",
        "hashline_edit",
        "patch",
    ] {
        assert!(
            agent.tool_names.iter().any(|t| t == expected),
            "editor tool_names missing `{expected}`: {:?}",
            agent.tool_names,
        );
    }
    assert!(agent.spawnable_agents.is_empty(), "leaf agent");
}
