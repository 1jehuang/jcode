//! Agent definitions at swarm spawn.
//!
//! `spawn` can name a definition with `agent`. The definition is resolved from
//! the coordinator's working directory, and its persona is delivered once to
//! the worker as a hidden message at the start of its transcript, so the system
//! prompt (and prompt caching) stays untouched.

use crate::agent_def::{AgentDefinition, find_agent_definition, load_agent_definitions};
use anyhow::Result;
use std::path::Path;

/// Resolve the requested definition, or `Ok(None)` when none was requested.
/// An unknown name is an error listing the valid ones.
pub(in crate::server) fn resolve(
    agent: Option<&str>,
    working_dir: Option<&str>,
) -> Result<Option<AgentDefinition>> {
    let Some(name) = agent.map(str::trim).filter(|name| !name.is_empty()) else {
        return Ok(None);
    };
    let dir = working_dir.map(Path::new);
    match find_agent_definition(name, dir) {
        Some(definition) => Ok(Some(definition)),
        None => {
            let known: Vec<String> = load_agent_definitions(dir).into_keys().collect();
            if known.is_empty() {
                anyhow::bail!(
                    "Unknown agent definition '{name}'. No definitions are available: add Markdown files under .jcode/agents or .claude/agents."
                )
            }
            anyhow::bail!(
                "Unknown agent definition '{name}'. Available: {}",
                known.join(", ")
            )
        }
    }
}

/// The hidden persona message for `definition`.
pub(in crate::server) fn persona_text(definition: &AgentDefinition) -> String {
    format!(
        "<system-reminder>\n# Agent persona: {}\n\n{}\n</system-reminder>",
        definition.name,
        definition.persona.trim()
    )
}

pub(in crate::server) type SessionAgents = std::sync::Arc<
    tokio::sync::RwLock<
        std::collections::HashMap<String, std::sync::Arc<tokio::sync::Mutex<crate::agent::Agent>>>,
    >,
>;

/// Add the persona to a freshly created headless worker, if a definition was
/// requested.
pub(in crate::server) async fn deliver_headless_persona(
    sessions: &SessionAgents,
    session_id: &str,
    definition: Option<&AgentDefinition>,
) {
    let Some(definition) = definition else {
        return;
    };
    let agent = sessions.read().await.get(session_id).cloned();
    if let Some(agent) = agent {
        agent
            .lock()
            .await
            .add_persona_message(&persona_text(definition));
    }
}

/// Write the persona into a prepared (not yet launched) visible session, so
/// the headed client finds it at the start of the transcript on attach.
pub(in crate::server) fn persist_persona(session_id: &str, definition: &AgentDefinition) {
    let result = crate::session::Session::load(session_id).and_then(|mut session| {
        session.add_message_with_display_role(
            crate::message::Role::User,
            vec![crate::message::ContentBlock::Text {
                text: persona_text(definition),
                cache_control: None,
            }],
            Some(crate::session::StoredDisplayRole::System),
        );
        session.save_prepared()
    });
    if let Err(error) = result {
        crate::logging::warn(&format!(
            "Failed to persist agent persona '{}' for session {session_id}: {error}",
            definition.name
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_request_resolves_to_none_and_unknown_lists_choices() {
        let _guard = crate::storage::lock_test_env();
        let home = tempfile::tempdir().unwrap();
        let previous = std::env::var_os("JCODE_HOME");
        crate::env::set_var("JCODE_HOME", home.path());
        let project = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(project.path().join(".git")).unwrap();
        std::fs::create_dir_all(project.path().join(".jcode/agents")).unwrap();
        std::fs::write(
            project.path().join(".jcode/agents/reviewer.md"),
            "---\nname: reviewer\ndescription: Reviews code\n---\nBe strict.\n",
        )
        .unwrap();
        let cwd = project.path().display().to_string();
        let none = resolve(None, Some(&cwd)).unwrap();
        let blank = resolve(Some("  "), Some(&cwd)).unwrap();
        let found = resolve(Some("reviewer"), Some(&cwd)).unwrap().unwrap();
        let unknown = resolve(Some("nope"), Some(&cwd)).unwrap_err().to_string();
        let empty_cwd = tempfile::tempdir().unwrap();
        let empty = resolve(Some("nope"), Some(&empty_cwd.path().display().to_string()))
            .unwrap_err()
            .to_string();
        match previous {
            Some(value) => crate::env::set_var("JCODE_HOME", value),
            None => crate::env::remove_var("JCODE_HOME"),
        }
        assert!(none.is_none() && blank.is_none());
        assert_eq!(found.name, "reviewer");
        assert!(unknown.contains("Available: reviewer"), "{unknown}");
        assert!(empty.contains("No definitions are available"), "{empty}");
        let text = persona_text(&found);
        assert!(text.starts_with("<system-reminder>\n# Agent persona: reviewer"));
        assert!(text.contains("Be strict."));
    }
}
