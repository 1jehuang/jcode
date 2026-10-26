//! Runtime-backed slash command execution. No command becomes an agent prompt.
use crate::agent::Agent;
use anyhow::{Result, bail};
use jcode_harness_api::RuntimeCommand;
use serde_json::{Value, json};

pub(super) fn boolean_action(
    args: &[String],
    current: bool,
    toggle_empty: bool,
) -> Result<Option<bool>> {
    match args {
        [] if toggle_empty => Ok(Some(!current)),
        [] => Ok(None),
        [arg] => match arg.to_ascii_lowercase().as_str() {
            "status" | "show" => Ok(None),
            "on" | "enable" | "enabled" => Ok(Some(true)),
            "off" | "disable" | "disabled" => Ok(Some(false)),
            "toggle" => Ok(Some(!current)),
            _ => bail!("Expected on, off, toggle, or status"),
        },
        _ => bail!("Expected at most one argument"),
    }
}

fn status_only(args: &[String]) -> Result<()> {
    match args {
        [] => Ok(()),
        [arg] if matches!(arg.as_str(), "status" | "show" | "stats") => Ok(()),
        _ => bail!("This command accepts only status, show, or stats"),
    }
}

pub(super) async fn execute(
    agent: &mut Agent,
    command: RuntimeCommand,
    args: &[String],
    swarm_enabled: bool,
    swarm_data: Value,
) -> Result<(Value, String)> {
    let config = crate::config::Config::load();
    let data = match command {
        RuntimeCommand::Review | RuntimeCommand::Judge | RuntimeCommand::Subagent => {
            let role = match command {
                RuntimeCommand::Review => "review",
                RuntimeCommand::Judge => "judge",
                _ => "swarm",
            };
            let (explicit_model, label, requested) = role_args(args, role)?;
            let prompt = match command {
                RuntimeCommand::Review => format!(
                    "Independently review the current working tree changes. Inspect the diff, correctness, regressions, security and tests. Report concrete findings with file references. Do not edit files. User focus: {requested}"
                ),
                RuntimeCommand::Judge => format!(
                    "Independently judge the current implementation against the user request. Inspect actual changes and verification evidence, identify unmet acceptance criteria and missing tests. Do not edit files. User focus: {requested}"
                ),
                _ if requested.trim().is_empty() => bail!("Usage: /subagent <task>"),
                _ => requested,
            };
            let global = match command {
                RuntimeCommand::Review => config.autoreview.model.clone(),
                RuntimeCommand::Judge => config.autojudge.model.clone(),
                _ => agent.subagent_model().or(config.agents.swarm_model.clone()),
            };
            let model = explicit_model.or_else(|| {
                agent
                    .session_for_split()
                    .effective_agent_model(role, global)
            });
            let output = agent
                .registry()
                .execute(
                    "swarm",
                    json!({"action": "spawn", "label": label, "prompt": prompt, "model": model}),
                    crate::tool::ToolContext {
                        session_id: agent.session_id().into(),
                        message_id: crate::id::new_id("msg"),
                        tool_call_id: crate::id::new_id("call"),
                        working_dir: agent.working_dir().map(std::path::PathBuf::from),
                        stdin_request_tx: None,
                        graceful_shutdown_signal: None,
                        execution_mode: crate::tool::ToolExecutionMode::Direct,
                    },
                )
                .await?;
            let metadata = output
                .metadata
                .ok_or_else(|| anyhow::anyhow!("Spawn returned no structured session metadata"))?;
            return Ok((
                json!({"spawned_session_id": metadata["session_id"], "role": role, "model": model}),
                output.output,
            ));
        }
        RuntimeCommand::ProviderTestCoverage => {
            let report = if args.is_empty() {
                let (coverage, path) = crate::live_tests::load_coverage(None)?;
                let summary = crate::live_tests::strict_live_provider_model_coverage_summary(
                    &coverage,
                    path.display().to_string(),
                );
                crate::live_tests::format_strict_live_provider_model_coverage_summary(&summary, 0)
            } else {
                let model = if args.len() > 1 {
                    args[1..].join(" ")
                } else {
                    agent.provider_model()
                };
                crate::live_tests::format_provider_test_coverage_report(&args[0], &model, None)
            };
            return Ok((
                json!({"report": report, "source": "persisted_verification"}),
                report,
            ));
        }
        RuntimeCommand::RefreshModelList
        | RuntimeCommand::Continue
        | RuntimeCommand::ResumeAll
        | RuntimeCommand::ServerReload => {
            bail!("Command must be routed through the server control handler")
        }
        RuntimeCommand::Skills => {
            status_only(args)?;
            agent.registry().skills().write().await.reload_global()?;
            let data = agent.runtime_skills_snapshot();
            return Ok((data.clone(), skills_report(&data)));
        }
        RuntimeCommand::Context => {
            status_only(args)?;
            agent.runtime_context_snapshot().await
        }
        RuntimeCommand::Info => {
            status_only(args)?;
            json!({"session_id": agent.session_id(), "working_dir": agent.working_dir(),
                "provider": agent.provider_name(), "model": agent.provider_model(),
                "reasoning_effort": agent.provider_reasoning_effort(),
                "context_window": agent.provider_context_window(),
                "last_usage": agent.last_usage(), "token_usage_totals": agent.token_usage_totals(),
                "memory_enabled": agent.memory_enabled(), "swarm_enabled": swarm_enabled,
                "autoreview_enabled": agent.autoreview_enabled().unwrap_or(config.autoreview.enabled),
                "autojudge_enabled": agent.autojudge_enabled().unwrap_or(config.autojudge.enabled),
                "subagent_model": agent.subagent_model(), "transport": agent.provider_handle().transport(),
                "service_tier": agent.provider_handle().service_tier()})
        }
        RuntimeCommand::Cache => {
            match args {
                [] => {}
                [arg] if matches!(arg.as_str(), "stats" | "status" | "show") => {}
                [arg] if matches!(arg.as_str(), "extend" | "1h" | "5m") => {
                    crate::config::Config::set_anthropic_cache_ttl_1h(arg != "5m")?;
                }
                _ => bail!(
                    "Usage: /cache [stats|status|extend|1h|5m]. Cache eviction/prewarming is not available through this runtime API."
                ),
            }
            json!({"anthropic_cache_ttl": if crate::provider::anthropic::is_cache_ttl_1h() { "1h" } else { "5m" },
                "setting_scope": "Anthropic, subsequent requests only", "provider": agent.provider_name(),
                "last_usage": agent.last_usage(), "token_usage_totals": agent.token_usage_totals(),
                "expiry": null, "expiry_available": false})
        }
        RuntimeCommand::Memory => {
            if args == ["extract"] {
                if !agent.memory_enabled() {
                    bail!("Memory is disabled for this session");
                }
                let transcript = agent.build_transcript_for_extraction();
                if transcript.len() < 200 {
                    bail!("Not enough transcript for memory extraction");
                }
                crate::memory_agent::trigger_final_extraction_with_dir(
                    transcript,
                    agent.session_id().into(),
                    agent.working_dir().map(str::to_string),
                );
                return Ok((
                    json!({"scheduled": true}),
                    "Memory extraction scheduled, not yet completed.".into(),
                ));
            }
            if let Some(enabled) = boolean_action(args, agent.memory_enabled(), true)? {
                agent.set_memory_enabled(enabled);
                if !enabled {
                    crate::memory::clear_pending_memory(agent.session_id());
                }
            }
            json!({"enabled": agent.memory_enabled(), "activity": agent.debug_memory_profile(), "pipeline_active": crate::memory_agent::is_active()})
        }
        RuntimeCommand::Swarm => {
            status_only(args)?;
            json!({"enabled": swarm_enabled, "default_enabled": config.features.swarm, "members": swarm_data})
        }
        RuntimeCommand::Agents => {
            let (global, args) = match args.split_first() {
                Some((scope, rest)) if matches!(scope.as_str(), "global" | "default") => {
                    (true, rest)
                }
                _ => (false, args),
            };
            match args {
                [] => {}
                [arg]
                    if matches!(
                        arg.as_str(),
                        "show" | "status" | "swarm" | "review" | "judge" | "memory" | "ambient"
                    ) => {}
                [target, model] => {
                    let model = if model == "default" {
                        None
                    } else {
                        Some(model.clone())
                    };
                    if global {
                        crate::config::Config::set_agent_model_default(target, model.as_deref())?;
                    } else {
                        agent.set_agent_model_override(target, model)?;
                    }
                }
                _ => bail!(
                    "Usage: /agents [global] [swarm|review|judge|memory|ambient] [model|inherit]."
                ),
            }
            let config = crate::config::Config::load();
            json!({"scope": if global { "global" } else { "session" }, "defaults": {"swarm": config.agents.swarm_model, "memory": config.agents.memory_model, "review": config.autoreview.model, "judge": config.autojudge.model, "ambient": config.ambient.model}, "overrides": agent.runtime_agent_model_overrides(), "models": agent.available_models_for_switching()})
        }
        RuntimeCommand::SubagentModel => {
            match args {
                [] => {}
                [arg] if matches!(arg.as_str(), "status" | "show") => {}
                [model] => agent.set_subagent_model(if model == "default" {
                    None
                } else {
                    Some(model.clone())
                })?,
                _ => bail!("Usage: /subagent-model [show|inherit|model]"),
            }
            json!({"model": agent.subagent_model(), "configured_default": config.agents.swarm_model, "inherit_model": agent.provider_model()})
        }
        RuntimeCommand::Autoreview => {
            let current = agent
                .autoreview_enabled()
                .unwrap_or(config.autoreview.enabled);
            if let Some(enabled) = boolean_action(args, current, false)? {
                agent.set_autoreview_enabled(enabled)?;
            }
            json!({"enabled": agent.autoreview_enabled().unwrap_or(config.autoreview.enabled), "default_enabled": config.autoreview.enabled})
        }
        RuntimeCommand::Autojudge => {
            let current = agent
                .autojudge_enabled()
                .unwrap_or(config.autojudge.enabled);
            if let Some(enabled) = boolean_action(args, current, false)? {
                agent.set_autojudge_enabled(enabled)?;
            }
            json!({"enabled": agent.autojudge_enabled().unwrap_or(config.autojudge.enabled), "default_enabled": config.autojudge.enabled})
        }
        RuntimeCommand::Transport => {
            let provider = agent.provider_handle();
            match args {
                [] => {}
                [arg] if matches!(arg.as_str(), "status" | "show") => {}
                [transport] if matches!(transport.as_str(), "auto" | "https" | "websocket") => {
                    provider.set_transport(transport)?
                }
                _ => bail!("Usage: /transport [status|auto|https|websocket]"),
            }
            json!({"transport": provider.transport(), "provider": provider.display_name()})
        }
        RuntimeCommand::Fast => {
            let provider = agent.provider_handle();
            let current = provider.service_tier();
            let action = match args {
                [] => Some(if current.as_deref() == Some("priority") {
                    "off"
                } else {
                    "priority"
                }),
                [arg] if matches!(arg.as_str(), "status" | "show") => None,
                [arg] if matches!(arg.as_str(), "on" | "fast" | "priority") => Some("priority"),
                [arg] if matches!(arg.as_str(), "ultra" | "ultrafast" | "ultra-fast") => {
                    Some("ultrafast")
                }
                [arg] if matches!(arg.as_str(), "off" | "flex" | "auto") => Some(arg.as_str()),
                _ => bail!("Usage: /fast [on|ultra|off|priority|flex|auto|status]"),
            };
            if let Some(tier) = action {
                provider.set_service_tier(tier)?;
            }
            json!({"service_tier": provider.service_tier(), "provider": provider.display_name()})
        }
        RuntimeCommand::Telemetry => {
            use crate::telemetry::TelemetryCategory;
            let usage = "Usage: /telemetry [all|none] or /telemetry <content|usage|crashes|features|cost|accounts> on|off";
            let parse_on = |v: &str| match v {
                "on" | "yes" | "true" | "1" => Some(true),
                "off" | "no" | "false" | "0" => Some(false),
                _ => None,
            };
            let mut checked = crate::telemetry::category_settings();
            let changed = match args {
                [] => false,
                [arg] if matches!(arg.as_str(), "show" | "status") => false,
                [arg] if matches!(arg.as_str(), "everything" | "all" | "on") => {
                    checked = [true; 6];
                    true
                }
                [arg] if matches!(arg.as_str(), "nothing" | "none" | "off") => {
                    checked = [false; 6];
                    true
                }
                [key, value] => {
                    let (Some(category), Some(on)) =
                        (TelemetryCategory::from_key(key), parse_on(value))
                    else {
                        bail!("{usage}");
                    };
                    if let Some(i) = TelemetryCategory::ALL.iter().position(|c| *c == category) {
                        checked[i] = on;
                    }
                    true
                }
                _ => bail!("{usage}"),
            };
            if changed {
                if checked.iter().any(|on| *on) && crate::telemetry::opt_out_forced_by_env() {
                    bail!("Telemetry is forced off by the runtime environment");
                }
                crate::telemetry::set_category_settings(checked);
            }
            let status = crate::telemetry::status();
            let categories: serde_json::Map<String, serde_json::Value> = TelemetryCategory::ALL
                .into_iter()
                .map(|c| (c.key().to_string(), json!(c.enabled())))
                .collect();
            json!({"enabled": status.enabled, "content_sharing_enabled": status.content_sharing_enabled, "categories": categories, "opt_out_source": status.opt_out_source.map(|source| source.as_str())})
        }
        RuntimeCommand::Subscription => {
            status_only(args)?;
            let configured = crate::subscription_catalog::has_credentials();
            if configured {
                let me = crate::subscription_api::fetch_subscription_me().await?;
                json!({"configured": true, "source": "live", "account": me})
            } else {
                json!({"configured": false, "source": "local", "cached_tier": crate::subscription_catalog::cached_tier().map(|tier| tier.as_str()), "message": "Log in to Jcode to fetch live hosted usage and spending limit."})
            }
        }
        RuntimeCommand::Overnight
        | RuntimeCommand::Productivity
        | RuntimeCommand::Initiatives
        | RuntimeCommand::Premium
        | RuntimeCommand::Remote
        | RuntimeCommand::Auth
        | RuntimeCommand::Logout => {
            return super::runtime_special_commands::execute(agent, command, args).await;
        }
    };
    let message = serde_json::to_string_pretty(&data)?;
    Ok((data, message))
}

fn role_args(args: &[String], default_label: &str) -> Result<(Option<String>, String, String)> {
    let mut model = None;
    let mut label = default_label.to_string();
    let mut index = 0;
    if let Some(first) = args.first()
        && let Some((provider, _)) = first.split_once(':')
        && matches!(
            provider,
            "openai"
                | "openai-api"
                | "claude"
                | "anthropic"
                | "openrouter"
                | "gemini"
                | "copilot"
                | "cursor"
                | "bedrock"
                | "azure"
                | "grok"
        )
    {
        model = Some(first.clone());
        index = 1;
    }
    while let Some(arg) = args.get(index) {
        match arg.as_str() {
            "--model" | "--type" => {
                let value = args
                    .get(index + 1)
                    .ok_or_else(|| anyhow::anyhow!("Missing value for {arg}"))?;
                if arg == "--model" {
                    model = Some(value.clone());
                } else {
                    label = value.clone();
                }
                index += 2;
            }
            "--continue" => bail!(
                "Resuming an existing subagent is unavailable here. Use the swarm resume command for that session."
            ),
            flag if flag.starts_with("--") => bail!("Unknown role launch option {flag}"),
            _ => break,
        }
    }
    Ok((model, label, args[index..].join(" ")))
}

fn skills_report(data: &Value) -> String {
    let mut report = String::from("## Loaded skills\n\n");
    let loaded = data["skills"].as_array().map(Vec::as_slice).unwrap_or(&[]);
    if loaded.is_empty() {
        report.push_str("No skills loaded. Add skills under `~/.jcode/skills/<name>/SKILL.md` or the workspace `.jcode/skills`.\n");
    }
    for skill in loaded {
        let name = skill["name"].as_str().unwrap_or("");
        let active = if data["active_skill"].as_str() == Some(name) {
            " (active)"
        } else {
            ""
        };
        report.push_str(&format!(
            "- **/{name}{active}**: {}\n  Path: `{}`\n",
            skill["description"].as_str().unwrap_or(""),
            skill["path"].as_str().unwrap_or("")
        ));
    }
    report.push_str("\n## Endorsed skills (recommended by Jcode)\n\n");
    for skill in data["endorsed"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or(&[])
    {
        report.push_str(&format!(
            "- **/{}** [{}]: {}\n  Category: {}. Source: {}\n",
            skill["name"].as_str().unwrap_or(""),
            if skill["installed"] == true {
                "installed"
            } else {
                "not installed"
            },
            skill["description"].as_str().unwrap_or(""),
            skill["category"].as_str().unwrap_or(""),
            skill["source"].as_str().unwrap_or("")
        ));
        if skill["installed"] != true
            && let Some(install) = skill["install"].as_str()
        {
            report.push_str(&format!("  Install: `{install}`\n"));
        }
    }
    report.push_str("\nActivate a loaded skill using its slash command. Manage skills with the `skill_manage` tool.\n");
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[derive(Default)]
    struct NoInferenceProvider {
        tier: std::sync::Mutex<Option<String>>,
    }
    #[async_trait::async_trait]
    impl crate::provider::Provider for NoInferenceProvider {
        async fn complete(
            &self,
            _: &[crate::message::Message],
            _: &[crate::message::ToolDefinition],
            _: &str,
            _: Option<&str>,
        ) -> Result<crate::provider::EventStream> {
            panic!("Runtime status commands must never invoke a provider");
        }
        fn name(&self) -> &str {
            "test"
        }
        fn fork(&self) -> Arc<dyn crate::provider::Provider> {
            Arc::new(Self::default())
        }
        fn service_tier(&self) -> Option<String> {
            self.tier.lock().unwrap().clone()
        }
        fn set_service_tier(&self, tier: &str) -> Result<()> {
            *self.tier.lock().unwrap() = if tier == "off" {
                None
            } else {
                Some(tier.into())
            };
            Ok(())
        }
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn runtime_commands_read_actual_state_without_inference() {
        let _lock = crate::storage::lock_test_env();
        let home = tempfile::TempDir::new().unwrap();
        let old = std::env::var_os("JCODE_HOME");
        crate::env::set_var("JCODE_HOME", home.path());
        struct Restore(Option<std::ffi::OsString>);
        impl Drop for Restore {
            fn drop(&mut self) {
                if let Some(value) = &self.0 {
                    crate::env::set_var("JCODE_HOME", value);
                } else {
                    crate::env::remove_var("JCODE_HOME");
                }
            }
        }
        let _restore = Restore(old);
        let provider: Arc<dyn crate::provider::Provider> = Arc::new(NoInferenceProvider::default());
        let registry = crate::tool::Registry::new(provider.clone()).await;
        let mut agent =
            Agent::new_with_initial_working_dir(provider, registry, home.path().to_str());
        for command in [
            RuntimeCommand::Skills,
            RuntimeCommand::Context,
            RuntimeCommand::Info,
            RuntimeCommand::Cache,
            RuntimeCommand::Memory,
            RuntimeCommand::Agents,
            RuntimeCommand::SubagentModel,
            RuntimeCommand::Autoreview,
            RuntimeCommand::Autojudge,
            RuntimeCommand::Transport,
            RuntimeCommand::Fast,
            RuntimeCommand::Telemetry,
        ] {
            let (data, message) =
                execute(&mut agent, command, &["status".into()], true, Value::Null)
                    .await
                    .unwrap();
            assert!(data.is_object(), "{command:?}");
            assert!(!message.is_empty(), "{command:?}");
            if command == RuntimeCommand::Skills {
                assert!(
                    data["endorsed"]
                        .as_array()
                        .is_some_and(|skills| !skills.is_empty())
                );
                assert!(message.contains("Endorsed skills"));
            }
            if command == RuntimeCommand::Context {
                assert!(data.get("messages").is_some());
                assert!(data["system_prompt"]["pending_memory_included"] == false);
            }
        }
        execute(
            &mut agent,
            RuntimeCommand::Autoreview,
            &["on".into()],
            true,
            Value::Null,
        )
        .await
        .unwrap();
        assert_eq!(agent.autoreview_enabled(), Some(true));
        execute(
            &mut agent,
            RuntimeCommand::Autojudge,
            &["off".into()],
            true,
            Value::Null,
        )
        .await
        .unwrap();
        assert_eq!(agent.autojudge_enabled(), Some(false));
        execute(
            &mut agent,
            RuntimeCommand::Agents,
            &["review".into(), "inherit".into()],
            true,
            Value::Null,
        )
        .await
        .unwrap();
        assert_eq!(agent.runtime_agent_model_overrides()["review"], "inherit");
        for alias in ["ultra", "ultrafast", "ultra-fast"] {
            let (data, _) = execute(
                &mut agent,
                RuntimeCommand::Fast,
                &[alias.into()],
                true,
                Value::Null,
            )
            .await
            .unwrap();
            assert_eq!(data["service_tier"], "ultrafast");
            assert_eq!(
                agent.provider_handle().service_tier().as_deref(),
                Some("ultrafast")
            );
        }
        agent.provider_handle().set_service_tier("off").unwrap();
        execute(&mut agent, RuntimeCommand::Fast, &[], true, Value::Null)
            .await
            .unwrap();
        assert_eq!(
            agent.provider_handle().service_tier().as_deref(),
            Some("priority")
        );
        execute(&mut agent, RuntimeCommand::Fast, &[], true, Value::Null)
            .await
            .unwrap();
        assert_eq!(agent.provider_handle().service_tier(), None);
        assert!(
            execute(&mut agent, RuntimeCommand::Subagent, &[], true, Value::Null)
                .await
                .is_err()
        );
        assert!(
            execute(
                &mut agent,
                RuntimeCommand::Transport,
                &["typo".into()],
                true,
                Value::Null
            )
            .await
            .is_err()
        );
    }
    #[test]
    fn toggle_and_status_are_distinct() {
        assert_eq!(boolean_action(&[], true, true).unwrap(), Some(false));
        assert_eq!(
            boolean_action(&["status".into()], true, true).unwrap(),
            None
        );
        assert_eq!(boolean_action(&[], false, false).unwrap(), None);
        assert!(boolean_action(&["typo".into()], false, false).is_err());
        assert!(boolean_action(&["on".into(), "off".into()], false, false).is_err());
    }
    #[test]
    fn role_arguments_preserve_model_and_task_and_reject_unsupported_resume() {
        let args = [
            "--model",
            "openai:gpt-5",
            "--type",
            "research",
            "inspect",
            "code",
        ]
        .map(str::to_string);
        assert_eq!(
            role_args(&args, "swarm").unwrap(),
            (
                Some("openai:gpt-5".into()),
                "research".into(),
                "inspect code".into()
            )
        );
        let args = ["openai:gpt-5", "inspect", "code"].map(str::to_string);
        assert_eq!(
            role_args(&args, "review").unwrap().0.as_deref(),
            Some("openai:gpt-5")
        );
        assert!(role_args(&["--model".into()], "swarm").is_err());
        assert!(role_args(&["--continue".into(), "session".into()], "swarm").is_err());
    }
}
