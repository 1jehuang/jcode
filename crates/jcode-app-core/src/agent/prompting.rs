use super::Agent;
use crate::logging;
use crate::message::{Message, ToolDefinition};

impl Agent {
    pub(crate) fn runtime_skills_snapshot(&self) -> serde_json::Value {
        let registry = self.current_skills_snapshot();
        let mut loaded = registry.list();
        loaded.sort_by(|a, b| a.name.cmp(&b.name));
        let skills: Vec<_> = loaded
            .iter()
            .map(|skill| {
                serde_json::json!({
                    "name": skill.name, "description": skill.description,
                    "path": skill.path, "allowed_tools": skill.allowed_tools,
                })
            })
            .collect();
        let endorsed: Vec<_> = crate::skill::endorsed_skills().iter().map(|skill| serde_json::json!({
            "name": skill.name, "description": skill.description, "category": skill.category,
            "source": skill.source, "install": skill.install, "installed": registry.contains(skill.name),
        })).collect();
        serde_json::json!({"skills": skills, "endorsed": endorsed, "active_skill": self.active_skill})
    }

    pub(crate) fn runtime_agent_model_overrides(&self) -> serde_json::Value {
        serde_json::json!(self.session.agent_model_overrides)
    }

    pub(crate) async fn runtime_context_snapshot(&self) -> serde_json::Value {
        let mut context = self.debug_context().await;
        context["messages"] = serde_json::json!(self.session.visible_conversation_messages());
        context["context_window"] = serde_json::json!(self.provider_context_window());
        context
    }

    /// Explicitly prepare/freeze the same tool surface used by provider turns.
    /// Unlike `debug_context`, this may update the tool cache. It never calls a provider.
    pub async fn prepare_debug_context(&mut self) -> serde_json::Value {
        let prepared_tools = self.tool_definitions().await;
        let mut context = self.debug_context().await;
        context["prepared_tools"] = serde_json::json!(prepared_tools);
        context
    }

    /// Inspect the next request's static context without inference, prewarming,
    /// or locking a new tool snapshot. Pending memory is deliberately not consumed.
    pub async fn debug_context(&self) -> serde_json::Value {
        let prompt = self.build_system_prompt_split(None);
        let current_tools = self.tool_definitions_for_debug().await;
        let effective_tools = self.locked_tools.as_ref().unwrap_or(&current_tools);
        let locked_tool_names = self.locked_tools.as_ref().map(|tools| {
            tools
                .iter()
                .map(|tool| tool.name.as_str())
                .collect::<Vec<_>>()
        });
        serde_json::json!({
            "session_id": self.session.id,
            "working_dir": self.session.working_dir,
            "mode": if self.is_desktop_selfdev() { "desktop" }
                else if self.session.is_canary { "cli" } else { "regular" },
            "is_canary": self.session.is_canary,
            "system_prompt": {
                "static": prompt.static_part,
                "dynamic": prompt.dynamic_part,
                "pending_memory_included": false,
            },
            "tools_locked": self.locked_tools.is_some(),
            "locked_tool_names": locked_tool_names,
            "effective_tools": effective_tools,
            "current_tools": current_tools,
        })
    }

    pub(super) fn log_prompt_prefix_accounting(
        &self,
        split: &crate::prompt::SplitSystemPrompt,
        tools: &[ToolDefinition],
    ) {
        let system_tokens = split.estimated_tokens();
        let tool_tokens = ToolDefinition::aggregate_prompt_token_estimate(tools);
        let prefix_tokens = system_tokens + tool_tokens;
        logging::info(&format!(
            "Prompt prefix estimate: total={} tokens (system={} tools={})",
            prefix_tokens, system_tokens, tool_tokens
        ));
    }

    pub(super) fn build_memory_prompt_nonblocking_shared(
        &self,
        messages: std::sync::Arc<[Message]>,
        _memory_event_tx: Option<crate::memory::MemoryEventSink>,
    ) -> Option<crate::memory::PendingMemory> {
        if !self.memory_enabled {
            return None;
        }

        let session_id = &self.session.id;

        let fresh_user_turn = crate::message::ends_with_fresh_user_turn(&messages);
        let pending = if fresh_user_turn {
            crate::memory::take_pending_memory_for_project(
                session_id,
                self.session.working_dir.as_deref(),
            )
        } else {
            None
        };

        // Use the persistent memory-agent pipeline as the single source of truth.
        // Running both this and the legacy MemoryManager background retrieval path
        // can prepare overlapping pending prompts for the same turn, which makes
        // memory injection feel overly aggressive.
        // Relevance results are consumed only at the start of a fresh user turn.
        // Enqueuing again after every tool result runs the local embedding model
        // for each provider continuation without creating an additional injection
        // opportunity. One update per user turn keeps memory current while avoiding
        // redundant 512-token inference during tool-heavy agent loops.
        if fresh_user_turn {
            crate::memory_agent::update_context_sync_with_dir(
                session_id,
                messages,
                self.session.working_dir.clone(),
            );
        }

        pending
    }

    /// Build split system prompt for better caching
    /// Returns static (cacheable) and dynamic (not cached) parts separately
    pub(super) fn build_system_prompt_split(
        &self,
        memory_prompt: Option<&str>,
    ) -> crate::prompt::SplitSystemPrompt {
        if let Some(ref override_prompt) = self.session.system_prompt {
            return crate::prompt::SplitSystemPrompt {
                static_part: override_prompt.clone(),
                dynamic_part: String::new(),
            };
        }

        // The active skill prompt is announced once in the transcript when the
        // skill is activated (see `announce_active_skill`). Keeping it out of
        // the system prompt means switching skills never invalidates the cache.
        let skill_prompt: Option<String> = None;

        // Frozen per session so skill installs never rewrite the cached
        // system prefix. Later installs are announced in the transcript.
        let available_skills = &self.prompt_skills_snapshot;

        let working_dir = self
            .session
            .working_dir
            .as_ref()
            .map(std::path::PathBuf::from);

        let (mut split, _context_info) = crate::prompt::build_system_prompt_split_with_agents_md(
            skill_prompt.as_deref(),
            available_skills,
            self.session.is_canary,
            memory_prompt,
            working_dir.as_deref(),
            self.agents_md_snapshot.clone(),
        );

        // The tool list is locked for cache stability, so this inventory is
        // constant for the session. It goes in the static part: the dynamic
        // part is sent after the latest user prompt, so putting it there would
        // move an earlier input item on every turn and reset the OpenAI
        // websocket prefix chain.
        let available_tools = self.locked_tools.as_deref().unwrap_or(&[]);
        append_tool_capability_reminder(&mut split, available_tools);

        split
    }

    /// Non-blocking memory prompt - takes pending result and spawns check for next turn
    #[cfg(test)]
    pub(super) fn build_memory_prompt_nonblocking(
        &self,
        messages: &[Message],
        _memory_event_tx: Option<crate::memory::MemoryEventSink>,
    ) -> Option<crate::memory::PendingMemory> {
        self.build_memory_prompt_nonblocking_shared(messages.to_vec().into(), _memory_event_tx)
    }
}

/// Compact inventory of active tools so the model does not forget capabilities
/// that are easy to miss when MCP definitions sit behind `mcp_search`/`mcp_call`.
fn append_tool_capability_reminder(
    split: &mut crate::prompt::SplitSystemPrompt,
    tools: &[crate::message::ToolDefinition],
) {
    if tools.is_empty() {
        return;
    }

    let mut mcp = Vec::new();
    let mut orchestration = Vec::new();
    let mut other = Vec::new();

    for tool in tools {
        let name = tool.name.as_str();
        if name.starts_with("mcp__") || matches!(name, "mcp_search" | "mcp_call") {
            mcp.push(name);
        } else if matches!(name, "swarm" | "communicate" | "spawn") {
            orchestration.push(name);
        } else {
            other.push(name);
        }
    }

    let mut lines = vec![
        "# Available capabilities".to_string(),
        "The following capabilities are active in this session. Use the exact tool names in the tool definitions; this list is a compact reminder and updates with the current tool surface.".to_string(),
    ];
    append_capability_line(&mut lines, "MCP", &mcp);
    if mcp.contains(&"mcp_search") {
        lines.push("- An empty MCP search does not mean the service is unavailable: use `mcp` list to find configured but disconnected servers, then connect by server name before considering browser fallback.".to_string());
    }
    append_capability_line(&mut lines, "Subagents and orchestration", &orchestration);
    if orchestration.contains(&"swarm") {
        lines.push(
            "- Provider routes: use `swarm` with `list_models` to see authenticated models and alternate-provider routes before spawning; do not assume the coordinator route is the only one available.".to_string(),
        );
    }
    if !other.is_empty() {
        lines.push(format!("- Other tools available: {}", other.join(", ")));
    }
    let installed = installed_cli_names(std::env::var_os("PATH").as_deref());
    if !installed.is_empty() {
        lines.push(format!("- Installed CLIs on PATH: {}. Use `gws` for Google Workspace; `playwright` is available for local testing, not as a substitute for a connected service's native integration.", installed.join(", ")));
    }

    if !split.static_part.is_empty() {
        split.static_part.push_str("\n\n");
    }
    split.static_part.push_str(&lines.join("\n"));
}

fn installed_cli_names(path: Option<&std::ffi::OsStr>) -> Vec<&'static str> {
    let Some(path) = path else { return Vec::new() };
    ["gws", "playwright"]
        .into_iter()
        .filter(|name| {
            std::env::split_paths(path).any(|dir| {
                let candidate = dir.join(name);
                std::fs::metadata(candidate).is_ok_and(|metadata| {
                    if !metadata.is_file() {
                        return false;
                    }
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        metadata.permissions().mode() & 0o111 != 0
                    }
                    #[cfg(not(unix))]
                    {
                        true
                    }
                })
            })
        })
        .collect()
}

fn append_capability_line(lines: &mut Vec<String>, label: &str, names: &[&str]) {
    if !names.is_empty() {
        lines.push(format!("- {} tools available: {}", label, names.join(", ")));
    }
}
