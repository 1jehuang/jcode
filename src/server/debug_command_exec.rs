use super::ServerIdentity;
use super::debug_jobs::{DebugJob, maybe_start_async_debug_job};
use crate::agent::Agent;
use crate::build;
use crate::mcp::McpConfig;
use anyhow::Result;
use jcode_agent_runtime::SoftInterruptSource;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, RwLock};

pub(super) async fn resolve_debug_session(
    sessions: &Arc<RwLock<HashMap<String, Arc<Mutex<Agent>>>>>,
    session_id: &Arc<RwLock<String>>,
    requested: Option<String>,
) -> Result<(String, Arc<Mutex<Agent>>)> {
    let mut target = requested;
    if target.is_none() {
        let current = session_id.read().await.clone();
        if !current.is_empty() {
            target = Some(current);
        }
    }

    let sessions_guard = sessions.read().await;
    if let Some(id) = target {
        let agent = sessions_guard
            .get(&id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("Unknown session_id '{}'", id))?;
        return Ok((id, agent));
    }

    if sessions_guard.len() == 1 {
        let (id, agent) = sessions_guard.iter().next().expect("len==1 checked above");
        return Ok((id.clone(), Arc::clone(agent)));
    }

    Err(anyhow::anyhow!(
        "No active session found. Connect a client or provide session_id."
    ))
}

pub(super) fn debug_message_timeout_secs() -> Option<u64> {
    let raw = std::env::var("JCODE_DEBUG_MESSAGE_TIMEOUT_SECS").ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let secs = trimmed.parse::<u64>().ok()?;
    if secs == 0 { None } else { Some(secs) }
}

pub(super) async fn run_debug_message_with_timeout(
    agent: Arc<Mutex<Agent>>,
    msg: &str,
    timeout_secs: u64,
) -> Result<String> {
    let msg = msg.to_string();
    let mut handle = tokio::spawn(async move {
        let mut agent = agent.lock().await;
        agent.run_once_capture(&msg).await
    });

    tokio::select! {
        join_result = &mut handle => {
            match join_result {
                Ok(result) => result,
                Err(e) => Err(anyhow::anyhow!("debug message task failed: {}", e)),
            }
        }
        _ = tokio::time::sleep(Duration::from_secs(timeout_secs)) => {
            handle.abort();
            Err(anyhow::anyhow!(
                "debug message timed out after {}s",
                timeout_secs
            ))
        }
    }
}

pub(super) async fn execute_debug_command(
    agent: Arc<Mutex<Agent>>,
    command: &str,
    debug_jobs: Arc<RwLock<HashMap<String, DebugJob>>>,
    server_identity: Option<&ServerIdentity>,
) -> Result<String> {
    let trimmed = command.trim();

    if let Some(output) =
        maybe_start_async_debug_job(Arc::clone(&agent), trimmed, Arc::clone(&debug_jobs)).await?
    {
        return Ok(output);
    }

    if trimmed.starts_with("swarm_message:") {
        let msg = trimmed.strip_prefix("swarm_message:").unwrap_or("").trim();
        if msg.is_empty() {
            return Err(anyhow::anyhow!("swarm_message: requires content"));
        }

        let final_text = super::run_swarm_message(agent.clone(), msg).await?;
        return Ok(final_text);
    }

    if trimmed.starts_with("message:") {
        let msg = trimmed.strip_prefix("message:").unwrap_or("").trim();
        if let Some(timeout_secs) = debug_message_timeout_secs() {
            return run_debug_message_with_timeout(agent, msg, timeout_secs).await;
        }
        let mut agent = agent.lock().await;
        let output = agent.run_once_capture(msg).await?;
        return Ok(output);
    }

    if trimmed.starts_with("queue_interrupt:") {
        let content = trimmed
            .strip_prefix("queue_interrupt:")
            .unwrap_or("")
            .trim();
        if content.is_empty() {
            return Err(anyhow::anyhow!("queue_interrupt: requires content"));
        }
        let agent = agent.lock().await;
        agent.queue_soft_interrupt(content.to_string(), false, SoftInterruptSource::User);
        return Ok("queued".to_string());
    }

    if trimmed.starts_with("queue_interrupt_urgent:") {
        let content = trimmed
            .strip_prefix("queue_interrupt_urgent:")
            .unwrap_or("")
            .trim();
        if content.is_empty() {
            return Err(anyhow::anyhow!("queue_interrupt_urgent: requires content"));
        }
        let agent = agent.lock().await;
        agent.queue_soft_interrupt(content.to_string(), true, SoftInterruptSource::User);
        return Ok("queued (urgent)".to_string());
    }

    if trimmed.starts_with("tool:") {
        let raw = trimmed.strip_prefix("tool:").unwrap_or("").trim();
        if raw.is_empty() {
            return Err(anyhow::anyhow!("tool: requires a tool name"));
        }
        let mut parts = raw.splitn(2, |c: char| c.is_whitespace());
        let name = parts.next().unwrap_or("").trim();
        let input_raw = parts.next().unwrap_or("").trim();
        let input = if input_raw.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::from_str::<serde_json::Value>(input_raw)?
        };
        let agent = agent.lock().await;
        let output = agent.execute_tool(name, input).await?;
        let payload = serde_json::json!({
            "output": output.output,
            "title": output.title,
            "metadata": output.metadata,
        });
        return Ok(serde_json::to_string_pretty(&payload).unwrap_or_else(|_| "{}".to_string()));
    }

    if trimmed == "history" {
        let agent = agent.lock().await;
        let history = agent.get_history();
        return Ok(serde_json::to_string_pretty(&history).unwrap_or_else(|_| "[]".to_string()));
    }

    if trimmed == "tools" {
        let agent = agent.lock().await;
        let tools = agent.tool_names().await;
        return Ok(serde_json::to_string_pretty(&tools).unwrap_or_else(|_| "[]".to_string()));
    }

    if trimmed == "tools:full" {
        let agent = agent.lock().await;
        let definitions = agent.tool_definitions_for_debug().await;
        return Ok(serde_json::to_string_pretty(&definitions).unwrap_or_else(|_| "[]".to_string()));
    }

    if trimmed == "mcp" || trimmed == "mcp:servers" {
        let agent = agent.lock().await;
        let tool_names = agent.tool_names().await;
        let mut connected: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for name in tool_names {
            if let Some(rest) = name.strip_prefix("mcp__") {
                let mut parts = rest.splitn(2, "__");
                if let (Some(server), Some(tool)) = (parts.next(), parts.next()) {
                    connected
                        .entry(server.to_string())
                        .or_default()
                        .push(tool.to_string());
                }
            }
        }
        for tools in connected.values_mut() {
            tools.sort();
        }
        let connected_servers: Vec<String> = connected.keys().cloned().collect();

        let config = McpConfig::load();
        let config_path = if let Ok(jcode_dir) = crate::storage::jcode_dir() {
            let path = jcode_dir.join("mcp.json");
            if path.exists() {
                Some(path.to_string_lossy().to_string())
            } else {
                None
            }
        } else {
            None
        };
        let mut configured_servers: Vec<String> = config.servers.keys().cloned().collect();
        configured_servers.sort();

        return Ok(serde_json::to_string_pretty(&serde_json::json!({
            "config_path": config_path,
            "configured_servers": configured_servers,
            "connected_servers": connected_servers,
            "connected_tools": connected,
        }))
        .unwrap_or_else(|_| "{}".to_string()));
    }

    if trimmed == "mcp:tools" {
        let agent = agent.lock().await;
        let tool_names = agent.tool_names().await;
        let mcp_tools: Vec<&str> = tool_names
            .iter()
            .filter(|name| name.starts_with("mcp__"))
            .map(|name| name.as_str())
            .collect();
        return Ok(serde_json::to_string_pretty(&mcp_tools).unwrap_or_else(|_| "[]".to_string()));
    }

    if let Some(rest) = trimmed.strip_prefix("mcp:connect:") {
        let (server_name, config_json) = match rest.find(' ') {
            Some(idx) => (rest[..idx].trim(), &rest[idx + 1..]),
            None => {
                return Err(anyhow::anyhow!(
                    "Usage: mcp:connect:<server> {{\"command\":\"...\",\"args\":[...]}}"
                ));
            }
        };
        let mut input: serde_json::Value = serde_json::from_str(config_json)
            .map_err(|e| anyhow::anyhow!("Invalid JSON: {}", e))?;
        input["action"] = serde_json::json!("connect");
        input["server"] = serde_json::json!(server_name);
        let agent = agent.lock().await;
        let result = agent.execute_tool("mcp", input).await?;
        return Ok(result.output);
    }

    if let Some(server_name) = trimmed.strip_prefix("mcp:disconnect:") {
        let server_name = server_name.trim();
        let input = serde_json::json!({"action": "disconnect", "server": server_name});
        let agent = agent.lock().await;
        let result = agent.execute_tool("mcp", input).await?;
        return Ok(result.output);
    }

    if trimmed == "mcp:reload" {
        let input = serde_json::json!({"action": "reload"});
        let mut agent = agent.lock().await;
        let result = agent.execute_tool("mcp", input).await?;
        agent.unlock_tools();
        return Ok(result.output);
    }

    if let Some(rest) = trimmed.strip_prefix("mcp:call:") {
        let (tool_path, args_json) = match rest.find(' ') {
            Some(idx) => (rest[..idx].trim(), rest[idx + 1..].trim()),
            None => (rest.trim(), "{}"),
        };
        let mut parts = tool_path.splitn(2, ':');
        let server = parts.next().unwrap_or("");
        let tool = parts
            .next()
            .ok_or_else(|| anyhow::anyhow!("Usage: mcp:call:<server>:<tool> <json>"))?;
        let tool_name = format!("mcp__{}__{}", server, tool);
        let input: serde_json::Value =
            serde_json::from_str(args_json).map_err(|e| anyhow::anyhow!("Invalid JSON: {}", e))?;
        let agent = agent.lock().await;
        let result = agent.execute_tool(&tool_name, input).await?;
        return Ok(result.output);
    }

    if trimmed == "cancel" {
        let agent = agent.lock().await;
        agent.queue_soft_interrupt(
            "[CANCELLED] Generation cancelled via debug socket".to_string(),
            true,
            SoftInterruptSource::User,
        );
        return Ok(serde_json::json!({
            "status": "cancel_queued",
            "message": "Urgent interrupt queued - will cancel at next tool boundary"
        })
        .to_string());
    }

    if trimmed == "clear" || trimmed == "clear_history" {
        let mut agent = agent.lock().await;
        agent.clear();
        return Ok(serde_json::json!({
            "status": "cleared",
            "message": "Conversation history cleared"
        })
        .to_string());
    }

    if trimmed == "agent:info" {
        let agent = agent.lock().await;
        let info = agent.debug_info();
        return Ok(serde_json::to_string_pretty(&info).unwrap_or_else(|_| "{}".to_string()));
    }

    if trimmed == "last_response" {
        let agent = agent.lock().await;
        return Ok(agent
            .last_assistant_text()
            .unwrap_or_else(|| "last_response: none".to_string()));
    }

    if trimmed == "state" {
        let agent = agent.lock().await;
        let mut payload = serde_json::json!({
            "session_id": agent.session_id(),
            "messages": agent.message_count(),
            "is_canary": agent.is_canary(),
            "provider": agent.provider_name(),
            "model": agent.provider_model(),
            "upstream_provider": agent.last_upstream_provider(),
        });
        if let Some(identity) = server_identity {
            payload["server_name"] = serde_json::json!(identity.name);
            payload["server_icon"] = serde_json::json!(identity.icon);
            payload["server_version"] = serde_json::json!(identity.version);
        }
        return Ok(serde_json::to_string_pretty(&payload).unwrap_or_else(|_| "{}".to_string()));
    }

    if trimmed == "usage" {
        let agent = agent.lock().await;
        let usage = agent.last_usage();
        return Ok(serde_json::to_string_pretty(&usage).unwrap_or_else(|_| "{}".to_string()));
    }

    if trimmed == "help" {
        return Ok(
            "debug commands: state, usage, history, tools, tools:full, mcp:servers, mcp:tools, mcp:connect:<server> <json>, mcp:disconnect:<server>, mcp:reload, mcp:call:<server>:<tool> <json>, last_response, message:<text>, message_async:<text>, swarm_message:<text>, swarm_message_async:<text>, tool:<name> <json>, queue_interrupt:<content>, queue_interrupt_urgent:<content>, jobs, job_status:<id>, job_wait:<id>, sessions, create_session, create_session:<path>, create_session:selfdev:<path>, set_model:<model>, set_provider:<name>, trigger_extraction, available_models, reload, help".to_string()
        );
    }

    if trimmed.starts_with("set_model:") {
        let model = trimmed.strip_prefix("set_model:").unwrap_or("").trim();
        if model.is_empty() {
            return Err(anyhow::anyhow!("set_model: requires a model name"));
        }
        let mut agent = agent.lock().await;
        agent.set_model(model)?;
        let payload = serde_json::json!({
            "model": agent.provider_model(),
            "provider": agent.provider_name(),
        });
        return Ok(serde_json::to_string_pretty(&payload).unwrap_or_else(|_| "{}".to_string()));
    }

    if trimmed.starts_with("set_provider:") {
        let provider = trimmed
            .strip_prefix("set_provider:")
            .unwrap_or("")
            .trim()
            .to_lowercase();
        let claude_usage = crate::usage::get_sync();
        let claude_usage_exhausted =
            claude_usage.five_hour >= 0.99 && claude_usage.seven_day >= 0.99;
        let default_model = match provider.as_str() {
            "claude" | "anthropic" => {
                if claude_usage_exhausted {
                    "claude-sonnet-4-6"
                } else {
                    "claude-opus-4-6"
                }
            }
            "openai" | "codex" => "gpt-5.4",
            "openrouter" => "anthropic/claude-sonnet-4",
            "cursor" => "gpt-5",
            "copilot" => "copilot:claude-sonnet-4",
            "gemini" => "gemini-2.5-pro",
            "antigravity" => "default",
            _ => {
                return Err(anyhow::anyhow!(
                    "Unknown provider '{}'. Use: claude, openai, openrouter, cursor, copilot, gemini, antigravity",
                    provider
                ));
            }
        };
        let mut agent = agent.lock().await;
        agent.set_model(default_model)?;
        crate::telemetry::record_provider_switch();
        let payload = serde_json::json!({
            "model": agent.provider_model(),
            "provider": agent.provider_name(),
        });
        return Ok(serde_json::to_string_pretty(&payload).unwrap_or_else(|_| "{}".to_string()));
    }

    if trimmed == "trigger_extraction" {
        let agent = agent.lock().await;
        let count = agent.extract_session_memories().await;
        let payload = serde_json::json!({
            "extracted": count,
            "message_count": agent.message_count(),
        });
        return Ok(serde_json::to_string_pretty(&payload).unwrap_or_else(|_| "{}".to_string()));
    }

    if trimmed == "available_models" {
        let agent = agent.lock().await;
        let models = agent.available_models_display();
        return Ok(serde_json::to_string_pretty(&models).unwrap_or_else(|_| "[]".to_string()));
    }

    if trimmed == "reload" {
        let repo_dir = crate::build::get_repo_dir()
            .ok_or_else(|| anyhow::anyhow!("Could not find jcode repository directory"))?;

        let target_binary = crate::build::find_dev_binary(&repo_dir)
            .unwrap_or_else(|| build::release_binary_path(&repo_dir));
        if !target_binary.exists() {
            return Err(anyhow::anyhow!(format!(
                "No binary found at {}. Run 'jcode self-dev --build' first, or build with 'scripts/dev_cargo.sh build --release --bin jcode' and publish current.",
                target_binary.display()
            )));
        }

        let hash = crate::build::current_git_hash(&repo_dir)?;

        crate::build::publish_local_current_build(&repo_dir)?;
        crate::build::install_version(&repo_dir, &hash)?;
        crate::build::update_canary_symlink(&hash)?;

        let mut manifest = crate::build::BuildManifest::load()?;
        manifest.canary = Some(hash.clone());
        manifest.canary_status = Some(crate::build::CanaryStatus::Testing);
        manifest.save()?;

        let jcode_dir = crate::storage::jcode_dir()?;
        let info_path = jcode_dir.join("reload-info");
        std::fs::write(&info_path, format!("reload:{}", hash))?;

        let _request_id = super::send_reload_signal(hash.clone(), None, false);

        return Ok(format!(
            "Reload signal sent for build {}. Server will restart.",
            hash
        ));
    }

    Err(anyhow::anyhow!("Unknown debug command '{}'", trimmed))
}

#[cfg(test)]
mod tests {
    use super::execute_debug_command;
    use crate::agent::Agent;
    use crate::provider::{EventStream, Provider};
    use crate::tool::Registry;
    use anyhow::Result;
    use async_trait::async_trait;
    use std::collections::HashMap;
    use std::ffi::OsString;
    use std::sync::{Arc, Mutex, OnceLock};
    use std::time::{Duration, Instant};
    use tokio::sync::{Mutex as AsyncMutex, RwLock};

    static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

    fn lock_env() -> std::sync::MutexGuard<'static, ()> {
        ENV_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap()
    }

    struct EnvGuard {
        key: &'static str,
        original: Option<OsString>,
    }

    impl EnvGuard {
        fn set(key: &'static str, value: &str) -> Self {
            let original = std::env::var_os(key);
            crate::env::set_var(key, value);
            Self { key, original }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            if let Some(value) = &self.original {
                crate::env::set_var(self.key, value);
            } else {
                crate::env::remove_var(self.key);
            }
        }
    }

    struct TestProvider;

    #[async_trait]
    impl Provider for TestProvider {
        async fn complete(
            &self,
            _messages: &[crate::message::Message],
            _tools: &[crate::message::ToolDefinition],
            _system: &str,
            _resume_session_id: Option<&str>,
        ) -> Result<EventStream> {
            unimplemented!("not needed for debug command test")
        }

        fn name(&self) -> &str {
            "test"
        }

        fn fork(&self) -> Arc<dyn Provider> {
            Arc::new(Self)
        }
    }

    #[tokio::test]
    async fn debug_tool_selfdev_reload_returns_promptly_for_direct_execution() {
        let _env_lock = lock_env();
        let _test_session = EnvGuard::set("JCODE_TEST_SESSION", "1");
        let _debug_control = EnvGuard::set("JCODE_DEBUG_CONTROL", "1");

        let mut reload_rx = crate::server::subscribe_reload_signal_for_tests();

        let provider: Arc<dyn Provider> = Arc::new(TestProvider);
        let registry = Registry::new(provider.clone()).await;
        registry.register_selfdev_tools().await;

        let mut agent = Agent::new(provider, registry);
        agent.set_canary("self-dev");
        let agent = Arc::new(AsyncMutex::new(agent));

        let debug_jobs = Arc::new(RwLock::new(HashMap::new()));
        let started = Instant::now();
        let ack_task = tokio::spawn(async move {
            loop {
                if let Some(signal) = reload_rx.borrow_and_update().clone() {
                    crate::server::acknowledge_reload_signal(&signal);
                    return;
                }
                reload_rx
                    .changed()
                    .await
                    .expect("reload signal channel should remain open");
            }
        });
        let output = tokio::time::timeout(
            Duration::from_secs(2),
            execute_debug_command(
                agent,
                r#"tool:selfdev {"action":"reload"}"#,
                debug_jobs,
                None,
            ),
        )
        .await
        .expect("debug selfdev reload should not hang")
        .expect("debug selfdev reload should succeed");
        ack_task.await.expect("reload ack task should complete");

        assert!(
            started.elapsed() < Duration::from_secs(2),
            "debug selfdev reload took too long"
        );
        assert!(
            output.contains("Reload acknowledged") || output.contains("Server is restarting now"),
            "expected reload acknowledgement output, got: {}",
            output
        );
    }
}
