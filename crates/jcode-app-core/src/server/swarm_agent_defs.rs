//! Agent definitions at swarm spawn.
//!
//! `spawn` can name a definition with `agent`. The definition is resolved from
//! the coordinator's working directory, and its persona is delivered once to
//! the worker as a hidden message at the start of its transcript, so the system
//! prompt (and prompt caching) stays untouched.

use crate::agent_def::{AgentDefinition, find_agent_definition, load_agent_definitions};
use crate::server::SessionAgents;
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

/// Apply a definition to a freshly created headless worker: deliver the
/// persona and narrow the tool surface to the definition's policy.
pub(in crate::server) async fn apply_to_headless_worker(
    sessions: &SessionAgents,
    session_id: &str,
    definition: Option<&AgentDefinition>,
    policy: &super::swarm_agent_policy::AppliedPolicy,
) {
    let Some(definition) = definition else {
        return;
    };
    let agent = sessions.read().await.get(session_id).cloned();
    let Some(agent) = agent else {
        return;
    };
    let mut agent = agent.lock().await;
    agent.add_persona_message(&persona_text(definition));
    if policy.tools.is_some() || !policy.disallowed_tools.is_empty() {
        agent
            .narrow_tools(policy.tools.as_deref(), &policy.disallowed_tools)
            .await;
    }
}

/// Write the persona into a prepared (not yet launched) visible session, so
/// the headed client finds it at the start of the transcript on attach.
pub(in crate::server) fn persist_persona(session_id: &str, definition: &AgentDefinition) {
    let result = crate::session::Session::load(session_id).and_then(|mut session| {
        // Attaching skips the opening context once the transcript is nonempty,
        // so create it first, as the headless path does, and keep it ahead of
        // the persona.
        session.ensure_initial_session_context_message();
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

#[cfg(test)]
mod persist_tests {
    use super::*;

    #[test]
    fn visible_persona_follows_the_opening_session_context() {
        let _guard = crate::storage::lock_test_env();
        let home = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let previous = std::env::var_os("JCODE_HOME");
        crate::env::set_var("JCODE_HOME", home.path());
        let mut session = crate::session::Session::create(None, None);
        session.working_dir = Some(project.path().display().to_string());
        session.save_prepared().unwrap();
        let id = session.id.clone();
        std::fs::create_dir_all(project.path().join(".git")).unwrap();
        std::fs::create_dir_all(project.path().join(".jcode/agents")).unwrap();
        std::fs::write(
            project.path().join(".jcode/agents/reviewer.md"),
            "---\nname: reviewer\ndescription: Reviews code\n---\nBe strict.\n",
        )
        .unwrap();
        let cwd = project.path().display().to_string();
        let definition = resolve(Some("reviewer"), Some(&cwd)).unwrap().unwrap();
        persist_persona(&id, &definition);
        let mut loaded = crate::session::Session::load(&id).unwrap();
        // What the headed client does on attach: must not add a second one.
        let added_on_attach = loaded.ensure_initial_session_context_message();
        match previous {
            Some(value) => crate::env::set_var("JCODE_HOME", value),
            None => crate::env::remove_var("JCODE_HOME"),
        }
        let texts: Vec<String> = loaded
            .messages
            .iter()
            .flat_map(|message| message.content.iter())
            .filter_map(|block| match block {
                crate::message::ContentBlock::Text { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect();
        assert!(!added_on_attach);
        assert_eq!(texts.len(), 2, "{texts:?}");
        assert!(texts[0].contains("# Session Context"), "{texts:?}");
        assert!(texts[1].contains("# Agent persona: reviewer"), "{texts:?}");
    }
}
