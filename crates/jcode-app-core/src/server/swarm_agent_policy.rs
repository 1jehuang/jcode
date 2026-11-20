//! Model, effort and tool policy from an agent definition.
//!
//! The agent chooses only *which* definition. `model`, `effort` and `tools` in
//! a definition are operator configuration, because a person wrote the file
//! (Claude Code and Codex treat them the same way), so they do not conflict
//! with the rule that the swarm tool never lets an agent pick effort.
//!
//! Order, highest first:
//!
//! 1. `[agents] force_swarm_defaults = true` makes the operator's own
//!    `swarm_model`, `swarm_effort` and tool policy win outright
//! 2. the definition's `model`, `effort`, `tools`
//! 3. `[agents] swarm_model` / `swarm_effort`
//! 4. the coordinator
//!
//! A per-spawn `model` still needs `swarm_model_selection` and the allowlist,
//! and loses to a definition's `model`. Project definitions (including
//! `.claude/agents` in a project) only apply their policy when the `agents`
//! scope is trusted. Tools only narrow: the effective set is the global
//! policy intersected with `tools`, minus `disallowedTools`.

use crate::agent_def::{
    AgentDefinition, DefinitionOrigin, DefinitionPolicy, agents_digest, project_policy_definitions,
};
use crate::config::AgentsConfig;
use crate::project_trust::{self, Scope};
#[cfg(test)]
use std::collections::HashSet;
use std::path::Path;

/// Effort levels jcode accepts.
const EFFORTS: [&str; 7] = ["none", "minimal", "low", "medium", "high", "xhigh", "max"];

/// What a definition contributes to a spawn after trust and force rules.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(in crate::server) struct AppliedPolicy {
    /// Concrete model or route to request. `None` keeps the default chain.
    pub model: Option<String>,
    /// `true` when the definition said `inherit`: use the coordinator's model
    /// even if `swarm_model` pins another one.
    pub inherit_model: bool,
    pub effort: Option<String>,
    pub tools: Option<Vec<String>>,
    pub disallowed_tools: Vec<String>,
    /// Why part of the definition was not applied, for one log line.
    pub notice: Option<String>,
}

/// Whether the policy of `applied` is covered by an approval. The approved set
/// is rebuilt with `applied` substituted for its on-disk counterpart, so a file
/// edited after the definition was resolved yields a different digest and is
/// refused: what is approved is exactly what gets applied.
fn project_policy_trusted(applied: &AgentDefinition, working_dir: Option<&Path>) -> bool {
    let Some(dir) = working_dir else {
        return false;
    };
    let mut definitions = project_policy_definitions(Some(dir));
    definitions.retain(|d| d.name != applied.name);
    definitions.push(applied.clone());
    let refs: Vec<&AgentDefinition> = definitions.iter().collect();
    project_trust::is_trusted(dir, Scope::Agents, &agents_digest(&refs))
}

/// Normalise a definition effort to a level jcode knows, else `None`.
fn valid_effort(effort: &str) -> Option<String> {
    let effort = effort.trim().to_ascii_lowercase();
    EFFORTS.contains(&effort.as_str()).then_some(effort)
}

/// Apply trust and force rules to `definition`'s policy.
pub(in crate::server) fn applied_policy(
    definition: &AgentDefinition,
    agents: &AgentsConfig,
    working_dir: Option<&Path>,
) -> AppliedPolicy {
    let policy: &DefinitionPolicy = &definition.policy;
    if policy.is_empty() {
        return AppliedPolicy::default();
    }
    if agents.force_swarm_defaults {
        return AppliedPolicy {
            notice: Some(format!(
                "agent '{}': force_swarm_defaults is on, so its model, effort and tools are ignored",
                definition.name
            )),
            ..AppliedPolicy::default()
        };
    }
    if definition.origin == DefinitionOrigin::Project
        && !project_policy_trusted(definition, working_dir)
    {
        return AppliedPolicy {
            notice: Some(format!(
                "agent '{}' is a project definition whose model, effort and tools are not trusted; run `jcode trust grant --scope agents` to apply them (persona only for now)",
                definition.name
            )),
            ..AppliedPolicy::default()
        };
    }
    let mut applied = AppliedPolicy::default();
    match policy.model.as_deref().map(str::trim) {
        Some(model) if model.eq_ignore_ascii_case("inherit") => applied.inherit_model = true,
        Some(model) if !model.is_empty() => applied.model = Some(model.to_string()),
        _ => {}
    }
    if let Some(effort) = policy.effort.as_deref() {
        match valid_effort(effort) {
            Some(effort) => applied.effort = Some(effort),
            None => crate::logging::warn(&format!(
                "agent '{}': ignoring unknown effort '{effort}'",
                definition.name
            )),
        }
    }
    applied.tools = policy.tools.clone();
    applied.disallowed_tools = policy.disallowed_tools.clone();
    applied
}

/// Headed workers attach from a separate process and a session record has no
/// tool restriction, so a definition's `tools` cannot narrow them. Model and
/// effort do apply. Log it once per spawn so the gap is visible.
pub(in crate::server) fn warn_visible_tools_unsupported(policy: &AppliedPolicy) {
    if policy.tools.is_some() || !policy.disallowed_tools.is_empty() {
        crate::logging::warn(
            "agent definition tools/disallowedTools are not applied to visible (headed) workers; use headless or inline spawn mode to enforce them",
        );
    }
}

/// The model and effort requests for a spawn after the definition's policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::server) struct SpawnRequest {
    pub model: Option<String>,
    pub effort: Option<String>,
}

/// Fold a definition's applied policy into the per-spawn request.
///
/// A definition's `model` replaces the per-spawn model (which the caller has
/// already validated against `swarm_model_selection` and the allowlist), and
/// `inherit` selects the coordinator's model past a `swarm_model` pin by using
/// the `inherit` sentinel. The definition's `effort` replaces the request's
/// effort, which the swarm tool never fills for agents. With no applied
/// policy the inputs pass through unchanged.
pub(in crate::server) fn spawn_request(
    requested_model: Option<String>,
    requested_effort: Option<String>,
    applied: &AppliedPolicy,
) -> SpawnRequest {
    let model = if applied.inherit_model {
        Some("inherit".to_string())
    } else {
        applied.model.clone().or(requested_model)
    };
    SpawnRequest {
        model,
        effort: applied.effort.clone().or(requested_effort),
    }
}

/// Resolve a spawn's policy in one step: the applied definition policy and the
/// resulting model and effort request. Logs the trust, force and tool-surface
/// notices once. `headed` is true for visible workers, which cannot be
/// narrowed.
pub(in crate::server) fn for_spawn(
    definition: Option<&AgentDefinition>,
    agents: &AgentsConfig,
    working_dir: Option<&str>,
    requested_model: Option<String>,
    requested_effort: Option<String>,
    headed: bool,
) -> (AppliedPolicy, SpawnRequest) {
    let applied = match definition {
        Some(d) => applied_policy(d, agents, working_dir.map(Path::new)),
        None => AppliedPolicy::default(),
    };
    if let Some(notice) = applied.notice.as_deref() {
        crate::logging::warn(notice);
    }
    if headed {
        warn_visible_tools_unsupported(&applied);
    }
    let request = spawn_request(requested_model, requested_effort, &applied);
    (applied, request)
}

/// Effective worker tool names: `base` (the global policy already applied)
/// intersected with the definition's allowlist, minus its denylist. Never
/// widens `base`. Used to preview the surface in tests and logs; the live
/// agent applies the same rule in `Agent::narrow_tools`.
#[cfg(test)]
pub(in crate::server) fn narrow_tools(base: &mut HashSet<String>, policy: &AppliedPolicy) {
    if let Some(allow) = &policy.tools {
        let allow: HashSet<&str> = allow.iter().map(String::as_str).collect();
        base.retain(|tool| allow.contains(tool.as_str()));
    }
    for denied in &policy.disallowed_tools {
        base.remove(denied);
    }
}

#[cfg(test)]
#[path = "swarm_agent_policy_tests.rs"]
mod tests;
