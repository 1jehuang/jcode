//! Specialist runtime workflows for non-terminal clients.
//! Never mutate a client's filesystem or replace a workflow with a model prompt.
use crate::agent::Agent;
use anyhow::{Context, Result, bail};
use jcode_harness_api::RuntimeCommand;
use serde_json::{Value, json};

fn status_args(args: &[String]) -> Result<()> {
    match args {
        [] => Ok(()),
        [arg] if matches!(arg.as_str(), "status" | "show") => Ok(()),
        _ => bail!("This command accepts only status or show"),
    }
}

fn premium_arg(
    args: &[String],
    current: crate::provider::copilot::PremiumMode,
) -> Result<Option<crate::provider::copilot::PremiumMode>> {
    use crate::provider::copilot::PremiumMode;
    match args {
        [] => Ok(None),
        [arg] => match arg.as_str() {
            "status" | "show" => Ok(None),
            "normal" => Ok(Some(PremiumMode::Normal)),
            "one" => Ok(Some(PremiumMode::OnePerSession)),
            "zero" => Ok(Some(PremiumMode::Zero)),
            "toggle-one" => Ok(Some(if current == PremiumMode::OnePerSession {
                PremiumMode::Normal
            } else {
                PremiumMode::OnePerSession
            })),
            "toggle-zero" => Ok(Some(if current == PremiumMode::Zero {
                PremiumMode::Normal
            } else {
                PremiumMode::Zero
            })),
            _ => bail!("Usage: premium [normal|one|zero|status]"),
        },
        _ => bail!("Usage: premium [normal|one|zero|status]"),
    }
}

/// Find the attached session's latest run, rather than the host's latest run.
fn session_manifest(session_id: &str) -> Result<Option<crate::overnight::OvernightManifest>> {
    let root = crate::overnight::runs_dir()?;
    if !root.exists() {
        return Ok(None);
    }
    let mut latest = None;
    for entry in std::fs::read_dir(root)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let path = entry.path().join("manifest.json");
        let Ok(manifest) = crate::storage::read_json::<crate::overnight::OvernightManifest>(&path)
        else {
            continue;
        };
        if manifest.parent_session_id != session_id && manifest.coordinator_session_id != session_id
        {
            continue;
        }
        if latest
            .as_ref()
            .is_none_or(|old: &crate::overnight::OvernightManifest| {
                old.started_at < manifest.started_at
            })
        {
            latest = Some(manifest);
        }
    }
    Ok(latest)
}

pub(super) async fn execute(
    agent: &mut Agent,
    command: RuntimeCommand,
    args: &[String],
) -> Result<(Value, String)> {
    match command {
        RuntimeCommand::Auth => {
            status_args(args)?;
            let status = tokio::task::spawn_blocking(crate::auth::AuthStatus::check_fast).await?;
            let providers: Vec<_> = crate::provider_catalog::auth_status_login_providers()
                .into_iter()
                .map(|provider| {
                    let assessment = status.assessment_for_provider(provider);
                    json!({"id": provider.id, "display_name": provider.display_name,
                    "state": assessment.state, "readiness": assessment.readiness,
                    "credential_source": assessment.credential_source,
                    "refresh_support": assessment.refresh_support})
                })
                .collect();
            let data = json!({"scope": "runtime host", "providers": providers,
                "note": "Credential readiness is not a live provider request. Secrets and raw credential files are never returned."});
            Ok((
                data.clone(),
                format!(
                    "# Runtime authentication\n\n```json\n{}\n```",
                    serde_json::to_string_pretty(&data)?
                ),
            ))
        }
        RuntimeCommand::Logout => logout(args),
        RuntimeCommand::Remote => remote(args),
        RuntimeCommand::Overnight => overnight(agent, args).await,
        RuntimeCommand::Productivity => {
            status_args(args)?;
            let output = tokio::task::spawn_blocking(jcode_productivity_core::generate).await??;
            use base64::Engine;
            let png = base64::engine::general_purpose::STANDARD.encode(&output.png);
            Ok((
                json!({"report": output.report, "png_path": output.png_path,
                "image": {"media_type": "image/png", "base64": png}, "scope": "runtime host"}),
                output.markdown,
            ))
        }
        RuntimeCommand::Initiatives => {
            status_args(args)?;
            // The obsolete initiative tool was intentionally unregistered upstream.
            // Do not silently re-enable it, or pretend /goals starts tracking again.
            Ok((json!({"retired": true, "replacement": "session todo goals"}),
                "Legacy initiatives are retired. The initiative tool is not registered, and this command does not re-enable it. Use the session todo list and its goal assessments for current tracked work. Existing initiative records are preserved.".into()))
        }
        RuntimeCommand::Premium => {
            let requested = premium_arg(args, agent.premium_mode())?;
            let provider = agent.provider_name();
            if !provider.to_ascii_lowercase().contains("copilot") {
                bail!(
                    "Premium modes apply only to GitHub Copilot sessions. The current provider is {provider}; nothing was changed."
                );
            }
            if let Some(mode) = requested {
                let config = match mode {
                    crate::provider::copilot::PremiumMode::Normal => None,
                    crate::provider::copilot::PremiumMode::OnePerSession => Some("one"),
                    crate::provider::copilot::PremiumMode::Zero => Some("zero"),
                };
                crate::config::Config::set_copilot_premium(config)?;
                agent.set_premium_mode(mode);
            }
            let mode = match agent.premium_mode() {
                crate::provider::copilot::PremiumMode::Normal => "normal",
                crate::provider::copilot::PremiumMode::OnePerSession => "one",
                crate::provider::copilot::PremiumMode::Zero => "zero",
            };
            Ok((
                json!({"provider": provider, "mode": mode}),
                format!("Copilot premium mode: `{mode}`."),
            ))
        }
        _ => bail!("Not a specialist runtime command"),
    }
}

fn logout(args: &[String]) -> Result<(Value, String)> {
    use crate::provider_catalog::{LoginProviderTarget, resolve_login_provider};
    let (name, confirmed) = match args {
        [] => return Ok((json!({"confirmation_required": true, "scope": "runtime host"}),
            "Logout requires an explicit provider and confirmation. Use `/auth` to inspect runtime-host credentials, then `/logout <provider> confirm`. For `claude` and `openai`, this removes all Jcode-managed OAuth accounts plus the saved API key for that provider. API-specific ids remove only that saved API key. No credential was removed.".into())),
        [name] => (name.as_str(), false),
        [name, confirmation] if confirmation == "confirm" => (name.as_str(), true),
        _ => bail!("Usage: `/logout <provider> confirm`. Nothing was removed."),
    };
    let provider = resolve_login_provider(name)
        .with_context(|| format!("Unknown provider `{name}`. Nothing was removed."))?;
    let supported = matches!(
        provider.target,
        LoginProviderTarget::Claude
            | LoginProviderTarget::ClaudeApiKey
            | LoginProviderTarget::OpenAi
            | LoginProviderTarget::OpenAiApiKey
            | LoginProviderTarget::OpenRouter
            | LoginProviderTarget::Bedrock
            | LoginProviderTarget::Azure
            | LoginProviderTarget::OpenAiCompatible(_)
            | LoginProviderTarget::Cursor
    );
    if !supported {
        bail!(
            "Logout for {} is not supported by the shared runtime credential-removal API. Use that provider's owning account/login application. No credential was removed.",
            provider.display_name
        );
    }
    if !confirmed {
        return Ok((
            json!({"confirmation_required": true, "provider": provider.id, "scope": "runtime host"}),
            format!(
                "To remove {} credentials stored by Jcode on the attached runtime host, run `/logout {} confirm`. For Claude/OpenAI this removes every Jcode-managed OAuth account and the saved API key for that provider. Other apps' credentials and externally supplied environment keys are not revoked. No credential was removed.",
                provider.display_name, provider.id
            ),
        ));
    }
    let mut removed_oauth_accounts = 0;
    let mut cleared_api_key = false;
    let mut errors = Vec::new();
    let mut clear_key =
        |env_key: &str, env_file: &str| match crate::provider_catalog::save_env_value_to_env_file(
            env_key, env_file, None,
        ) {
            Ok(()) => cleared_api_key = true,
            Err(error) => errors.push(format!("Saved API-key removal failed: {error}")),
        };
    match provider.target {
        LoginProviderTarget::Claude => {
            clear_key("ANTHROPIC_API_KEY", "anthropic.env");
            match crate::auth::claude::clear_accounts() {
                Ok(count) => removed_oauth_accounts = count,
                Err(error) => errors.push(format!("OAuth account removal failed: {error}")),
            }
        }
        LoginProviderTarget::OpenAi => {
            clear_key("OPENAI_API_KEY", "openai.env");
            match crate::auth::codex::clear_accounts() {
                Ok(count) => removed_oauth_accounts = count,
                Err(error) => errors.push(format!("OAuth account removal failed: {error}")),
            }
        }
        LoginProviderTarget::ClaudeApiKey => clear_key("ANTHROPIC_API_KEY", "anthropic.env"),
        LoginProviderTarget::OpenAiApiKey => clear_key("OPENAI_API_KEY", "openai.env"),
        LoginProviderTarget::OpenRouter => clear_key("OPENROUTER_API_KEY", "openrouter.env"),
        LoginProviderTarget::Bedrock => clear_key(
            crate::provider::bedrock::API_KEY_ENV,
            crate::provider::bedrock::ENV_FILE,
        ),
        LoginProviderTarget::Azure => clear_key(
            crate::auth::azure::API_KEY_ENV,
            crate::auth::azure::ENV_FILE,
        ),
        LoginProviderTarget::OpenAiCompatible(profile) if profile.id == "muse-subscription" => {
            let had_login = crate::auth::muse::credentials_path()
                .map(|path| path.exists())
                .unwrap_or(false);
            match crate::auth::muse::clear_credentials() {
                Ok(()) => {
                    cleared_api_key = had_login;
                    removed_oauth_accounts = usize::from(had_login);
                }
                Err(error) => errors.push(format!("Muse device-login removal failed: {error}")),
            }
        }
        LoginProviderTarget::OpenAiCompatible(profile) => {
            let profile = crate::provider_catalog::resolve_openai_compatible_profile(profile);
            clear_key(&profile.api_key_env, &profile.env_file);
        }
        LoginProviderTarget::Cursor => match crate::auth::cursor::clear_api_key() {
            Ok(()) => cleared_api_key = true,
            Err(error) => errors.push(format!("Cursor API-key removal failed: {error}")),
        },
        _ => unreachable!("unsupported provider rejected before confirmation"),
    }
    crate::auth::AuthStatus::invalidate_cached_status();
    let data = json!({"provider": provider.id, "scope": "runtime host", "removed_oauth_accounts": removed_oauth_accounts,
        "saved_api_key_cleared": cleared_api_key, "errors": errors, "live_provider_reloaded": false,
        "external_credentials_revoked": false});
    let mut message = format!(
        "{} stored credential cleanup: removed {removed_oauth_accounts} Jcode-managed OAuth account(s); saved API key cleared: {cleared_api_key}. Existing provider instances may still hold in-memory credentials until reconnect/reinitialization. External credentials and upstream tokens were not revoked.",
        provider.display_name
    );
    if !errors.is_empty() {
        message.push_str(&format!(
            "\n\nCleanup was only partially successful:\n{}",
            errors.join("\n")
        ));
    }
    Ok((data, message))
}

fn remote(args: &[String]) -> Result<(Value, String)> {
    use crate::gateway::control::{self, RemoteCommand, RemoteStatus, ToggleOutcome};
    let input = format!("/remote {}", args.join(" "));
    let command = control::parse_remote_command(&input)
        .context("Invalid remote command")?
        .map_err(anyhow::Error::msg)?;
    match command {
        RemoteCommand::Cloud => bail!("Jcode Cloud activation belongs to the client's native account/cloud dialog. No local gateway setting was changed. Use Desktop's cloud setup UI."),
        RemoteCommand::Help => Ok((json!({"scope": "runtime host"}),
            "Usage: `/remote status`, `/remote on`, `/remote off`, `/remote pair`, `/remote revoke <device-id-or-name>`. These commands manage the attached runtime host's gateway. Gateway enable/disable settings take effect after a coordinated server restart; no server is restarted automatically. `/remote cloud` uses the client's Cloud setup dialog.".into())),
        RemoteCommand::Status => {
            let status = RemoteStatus::load();
            let devices: Vec<_> = status.devices.iter().map(|device| json!({"id": device.id, "name": device.name, "paired_at": device.paired_at, "last_seen": device.last_seen})).collect();
            Ok((json!({"enabled": status.enabled, "bind_addr": status.bind_addr, "port": status.port, "dial_address": status.dial_address(), "devices": devices, "scope": "runtime host", "listener_health_checked": false}),
                format!("{}\nThis is the runtime host's gateway configuration snapshot, not a live listener health check. Changed settings require a coordinated server restart.", status.to_markdown())))
        }
        RemoteCommand::On | RemoteCommand::Off => {
            let enabled = command == RemoteCommand::On;
            let outcome = control::set_gateway_enabled(enabled)?;
            let changed = matches!(outcome, ToggleOutcome::Changed { .. });
            Ok((json!({"enabled": enabled, "changed": changed, "restart_required": changed, "scope": "runtime host"}),
                if changed { format!("Gateway {} saved on the runtime host. A coordinated server restart is required before this change takes effect. No server was restarted.", if enabled { "enabled" } else { "disabled" }) }
                else { format!("Gateway is already configured {}. No setting was changed or server restarted.", if enabled { "on" } else { "off" }) }))
        }
        RemoteCommand::Pair => {
            let invite = control::create_pairing_invite()?;
            let message = format!("Pairing code: `{}`\n\nDial: `{}`\n\nPairing URI: `{}`\n\nThis code expires in five minutes. Share it only with the device you intend to authorize.{}",
                invite.spaced_code(), invite.dial_address, invite.uri,
                if invite.gateway_enabled { "" } else { " The gateway is not enabled. Run `/remote on` and coordinate a server restart before pairing." });
            Ok((json!({"code": invite.code, "dial_address": invite.dial_address, "uri": invite.uri, "gateway_enabled": invite.gateway_enabled, "expires_in_seconds": 300, "scope": "runtime host"}), message))
        }
        RemoteCommand::Revoke(target) => {
            let removed = control::revoke_device(&target)?;
            let message = if removed.is_empty() { format!("No paired device matched `{target}`. Nothing was revoked.") }
                else { format!("Revoked paired device(s): {}.", removed.join(", ")) };
            Ok((json!({"target": target, "removed": removed, "scope": "runtime host"}), message))
        }
    }
}

async fn overnight(agent: &mut Agent, args: &[String]) -> Result<(Value, String)> {
    use crate::overnight::{self, OvernightCommand, OvernightRunStatus};
    let input = format!("/overnight {}", args.join(" "));
    let command = overnight::parse_overnight_command(&input)
        .context("Invalid overnight command")?
        .map_err(anyhow::Error::msg)?;
    if command == OvernightCommand::Help {
        return Ok((
            json!({"usage": overnight::overnight_usage()}),
            overnight::overnight_usage().into(),
        ));
    }
    let session_id = agent.session_id().to_owned();
    if let OvernightCommand::Start { duration, mission } = command {
        if let Some(existing) = session_manifest(&session_id)?
            && matches!(
                existing.status,
                OvernightRunStatus::Running | OvernightRunStatus::CancelRequested
            )
        {
            bail!(
                "This session already has an active overnight run {}. Inspect status or cancel it before starting another.",
                existing.run_id
            );
        }
        let launch = overnight::start_overnight_run(overnight::OvernightStartOptions {
            duration,
            mission,
            parent_session: agent.session_for_split().clone(),
            provider: agent.provider_fork(),
            registry: agent.registry(),
            working_dir: agent.working_dir().map(std::path::PathBuf::from),
            use_current_session: false,
        })?;
        let message = format!(
            "Started supervised overnight coordinator `{}` in session `{}`. Target wake: {}. Use `/overnight status`, `/overnight log`, `/overnight review`, or `/overnight cancel`.",
            launch.manifest.run_id,
            launch.manifest.coordinator_session_id,
            launch.manifest.target_wake_at.to_rfc3339()
        );
        return Ok((serde_json::to_value(launch.manifest)?, message));
    }
    let Some(mut manifest) = session_manifest(&session_id)? else {
        return Ok((
            json!({"run": null}),
            "No overnight runs belong to this session.".into(),
        ));
    };
    match command {
        OvernightCommand::Status => {
            let message = overnight::format_status_markdown(&manifest);
            Ok((serde_json::to_value(manifest)?, message))
        }
        OvernightCommand::Log => {
            let message = overnight::format_log_markdown(&manifest, 100);
            Ok((
                json!({"run_id": manifest.run_id, "events": overnight::read_events(&manifest)?}),
                message,
            ))
        }
        OvernightCommand::Review => {
            // Return the actual review document, not an opener on the remote host.
            overnight::render_review_html(&manifest)?;
            let html = std::fs::read_to_string(&manifest.review_path)?;
            let cards = overnight::read_task_cards(&manifest)?;
            let mut message = format!(
                "# Overnight review: {}\n\n{}\n\n## Task outcomes\n",
                manifest.run_id,
                overnight::format_status_markdown(&manifest)
            );
            if cards.is_empty() {
                message.push_str("\nNo task cards have been recorded yet.\n");
            }
            for card in &cards {
                message.push_str(&format!(
                    "\n### {}\n\nStatus: {}\n",
                    card.title, card.status
                ));
                if let Some(outcome) = &card.outcome {
                    message.push_str(&format!("\n{outcome}\n"));
                }
                if let Some(result) = &card.validation.result {
                    message.push_str(&format!("\nValidation: {result}\n"));
                }
                for evidence in &card.validation.evidence {
                    message.push_str(&format!("- Evidence: {evidence}\n"));
                }
                for followup in &card.followups {
                    message.push_str(&format!("- Follow-up: {followup}\n"));
                }
            }
            Ok((
                json!({"run_id": manifest.run_id, "html": html, "path": manifest.review_path, "tasks": cards}),
                message,
            ))
        }
        OvernightCommand::Cancel => {
            if !matches!(
                manifest.status,
                OvernightRunStatus::Completed | OvernightRunStatus::Failed
            ) {
                manifest.status = OvernightRunStatus::CancelRequested;
                manifest.cancel_requested_at = Some(chrono::Utc::now());
                overnight::save_manifest(&manifest)?;
                overnight::record_event(
                    &manifest,
                    "cancel_requested",
                    "User requested overnight cancellation".into(),
                    json!({}),
                    true,
                )?;
                overnight::render_review_html(&manifest)?;
            }
            let message = overnight::format_status_markdown(&manifest);
            Ok((serde_json::to_value(manifest)?, message))
        }
        OvernightCommand::Start { .. } | OvernightCommand::Help => unreachable!(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn premium_modes_are_exact_and_invalid_input_never_defaults() {
        use crate::provider::copilot::PremiumMode;
        let parse = |args: &[String]| premium_arg(args, PremiumMode::Normal);
        assert_eq!(parse(&[]).unwrap(), None);
        assert_eq!(parse(&["status".into()]).unwrap(), None);
        assert_eq!(
            parse(&["normal".into()]).unwrap(),
            Some(PremiumMode::Normal)
        );
        assert_eq!(
            parse(&["one".into()]).unwrap(),
            Some(PremiumMode::OnePerSession)
        );
        assert_eq!(parse(&["zero".into()]).unwrap(), Some(PremiumMode::Zero));
        assert_eq!(
            premium_arg(&["toggle-one".into()], PremiumMode::OnePerSession).unwrap(),
            Some(PremiumMode::Normal)
        );
        assert_eq!(
            premium_arg(&["toggle-zero".into()], PremiumMode::Normal).unwrap(),
            Some(PremiumMode::Zero)
        );
        assert_eq!(
            premium_arg(&["toggle-zero".into()], PremiumMode::Zero).unwrap(),
            Some(PremiumMode::Normal)
        );
        assert!(parse(&["invalid".into()]).is_err());
        assert!(parse(&["zero".into(), "extra".into()]).is_err());
    }
    #[test]
    fn logout_requires_explicit_supported_provider_and_confirmation() {
        let (data, _) = logout(&[]).unwrap();
        assert_eq!(data["confirmation_required"], true);
        let (data, message) = logout(&["openai".into()]).unwrap();
        assert_eq!(data["confirmation_required"], true);
        assert!(message.contains("No credential was removed"));
        assert!(logout(&["openai".into(), "yes".into()]).is_err());
        assert!(logout(&["invalid-provider".into(), "confirm".into()]).is_err());
        assert!(logout(&["copilot".into(), "confirm".into()]).is_err());
    }

    #[test]
    fn remote_help_and_cloud_are_side_effect_free() {
        let (data, message) = remote(&["help".into()]).unwrap();
        assert_eq!(data["scope"], "runtime host");
        assert!(message.contains("No") || message.contains("no server"));
        assert!(remote(&["cloud".into()]).is_err());
        assert!(remote(&["on".into(), "extra".into()]).is_err());
        assert!(remote(&["revoke".into()]).is_err());
    }

    #[test]
    fn read_only_commands_reject_mutation_arguments() {
        assert!(status_args(&[]).is_ok());
        assert!(status_args(&["status".into()]).is_ok());
        assert!(status_args(&["resume".into()]).is_err());
        assert!(status_args(&["delete".into()]).is_err());
    }
}
