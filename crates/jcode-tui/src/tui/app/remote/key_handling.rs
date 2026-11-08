#[path = "key_handling/rewind.rs"]
mod rewind;
mod update;
use rewind::*;

use super::*;
use crate::tui::app as app_mod;
use crate::tui::app::PendingRemoteRewindNotice;
use crate::tui::core;

/// Canonical runtime commands never run the similarly named laptop handler.
/// Keep argument validation and destructive-action confirmation on the daemon.
fn ssh_runtime_command(input: &str) -> Option<(&'static str, Vec<String>)> {
    if let Some(command) = super::ssh_host::command(input) {
        return Some(command);
    }
    let mut words = input.split_whitespace();
    let token = words.next()?;
    let command = match token {
        "/poke" => "poke",
        "/improve" => "improve",
        "/refactor" => "refactor",
        "/fix" => "fix",
        "/skills" => "skills",
        "/refresh-model-list" => "refresh_model_list",
        "/context" => "context",
        "/info" => "info",
        "/cache" => "cache",
        "/agents" => "agents",
        "/subagent-model" => "subagent_model",
        "/autoreview" => "autoreview",
        "/autojudge" => "autojudge",
        "/review" => "review",
        "/judge" => "judge",
        "/subagent" => "subagent",
        "/telemetry" => "telemetry",
        "/subscription" => "subscription",
        "/overnight" => "overnight",
        "/productivity" | "/wrapped" => "productivity",
        "/initiatives" | "/goals" => "initiatives",
        "/auth" => "auth",
        "/logout" => "logout",
        "/remote" => "remote",
        "/provider-test-coverage" | "/model-status" => "provider_test_coverage",
        "/z" | "/zz" | "/zzz" | "/zstatus" => "premium",
        _ => return None,
    };
    let mut args = Vec::new();
    match token {
        "/z" => args.push("normal".to_string()),
        "/zz" => args.push("toggle-one".to_string()),
        "/zzz" => args.push("toggle-zero".to_string()),
        "/zstatus" => args.push("status".to_string()),
        _ => {}
    }
    args.extend(words.map(str::to_string));
    Some((command, args))
}

#[cfg(test)]
mod ssh_runtime_routing_tests {
    #[test]
    fn ssh_runtime_aliases_preserve_arguments_and_confirmation() {
        for (input, command, args) in [
            ("/logout openai", "logout", vec!["openai"]),
            (
                "/logout openai confirm",
                "logout",
                vec!["openai", "confirm"],
            ),
            ("/wrapped status", "productivity", vec!["status"]),
            ("/goals", "initiatives", vec![]),
            ("/model-status", "provider_test_coverage", vec![]),
            ("/z", "premium", vec!["normal"]),
            ("/zz", "premium", vec!["toggle-one"]),
            ("/zzz", "premium", vec!["toggle-zero"]),
            ("/zstatus", "premium", vec!["status"]),
        ] {
            let (actual, actual_args) = super::ssh_runtime_command(input).unwrap();
            assert_eq!(actual, command, "{input}");
            assert_eq!(actual_args, args, "{input}");
        }
        for input in [
            "/contextual skill",
            "/home/vm/info",
            "/logout-custom-skill",
            "/improve-custom-skill",
            "/fast-default-custom",
        ] {
            assert!(super::ssh_runtime_command(input).is_none(), "{input}");
        }
    }
}

#[cfg(test)]
pub(in crate::tui::app) use update::reload_stale_remote_server_before_update;
pub(in crate::tui::app) use update::{
    handle_remote_char_input, handle_remote_update_command, send_interleave_now,
};

/// Step the remote session's speed tier (Standard -> Fast -> Ultrafast).
async fn apply_remote_speed_direction(
    app: &mut App,
    remote: &mut RemoteConnection,
    direction: i8,
) -> Result<()> {
    use jcode_provider_core::service_tier::{speed_tier_ladder_with_capabilities, step_speed_tier};
    let (provider_name, provider_model) = app.remote_effort_identity();
    let ladder = speed_tier_ladder_with_capabilities(
        provider_name.as_deref(),
        provider_model.as_deref(),
        app.remote_available_service_tiers.as_deref(),
    );
    let Some((index, next, at_end)) =
        step_speed_tier(&ladder, app.remote_service_tier.as_deref(), direction)
    else {
        app.set_status_notice("Speed tiers not available for this model");
        return Ok(());
    };
    if at_end {
        app.set_status_notice(app_mod::speed_tier_notice(
            next,
            index,
            ladder.len(),
            Some(direction),
        ));
        return Ok(());
    }
    request_remote_service_tier(app, remote, next).await?;
    Ok(())
}

async fn request_remote_service_tier(
    app: &mut App,
    remote: &mut RemoteConnection,
    tier: &str,
) -> Result<()> {
    // A transient notice acknowledges input without claiming configuration or
    // provider acceptance. The server acknowledgement replaces it, so fast
    // round trips do not add a second transcript message.
    app.set_status_notice(format!(
        "Speed: {} requested (waiting for server)",
        app_mod::service_tier_display_label(tier)
    ));
    remote.set_service_tier(tier).await
}

async fn apply_remote_effort_direction(
    app: &mut App,
    remote: &mut RemoteConnection,
    direction: i8,
) -> Result<()> {
    // Use the best-known provider/model identity (server-reported when
    // available, header hints during the pre-History bootstrap window) so
    // effort cycling works immediately after spawn instead of claiming the
    // provider does not support it until the History payload settles.
    let (provider_name, provider_model) = app.remote_effort_identity();
    let efforts =
        app_mod::inferred_reasoning_efforts(provider_name.as_deref(), provider_model.as_deref());
    if efforts.is_empty() {
        app.set_status_notice("Reasoning effort not available for this provider");
        return Ok(());
    }
    let current = app.remote_reasoning_effort_hint();
    let current = current.as_deref();
    let current_index = current
        .and_then(|c| efforts.iter().position(|e| *e == c))
        .unwrap_or(efforts.len() - 1);
    let len = efforts.len();
    let next_index = if direction > 0 {
        if current_index + 1 >= len {
            current_index
        } else {
            current_index + 1
        }
    } else if current_index == 0 {
        0
    } else {
        current_index - 1
    };
    let next_effort = efforts[next_index];
    if Some(next_effort) == current {
        let label = app_mod::effort_display_label(next_effort);
        app.set_status_notice(format!(
            "Effort: {} (already at {})",
            label,
            if direction > 0 { "max" } else { "min" }
        ));
    } else {
        app.remote_reasoning_effort = Some(next_effort.to_string());
        app.invalidate_model_picker_cache();
        app.set_status_notice(format!(
            "Effort: {} (will apply to next request)",
            app_mod::effort_display_label(next_effort)
        ));
        remote.set_reasoning_effort(next_effort).await?;
    }
    Ok(())
}

impl App {
    pub(super) async fn handle_account_picker_command_remote(
        &mut self,
        remote: &mut RemoteConnection,
        command: crate::tui::account_picker::AccountPickerCommand,
    ) -> Result<()> {
        match command {
            crate::tui::account_picker::AccountPickerCommand::OpenAccountCenter {
                provider_filter,
            } => self.open_account_center(provider_filter.as_deref()),
            crate::tui::account_picker::AccountPickerCommand::OpenAddReplaceFlow {
                provider_filter,
            } => self.open_account_add_replace_flow(provider_filter.as_deref()),
            crate::tui::account_picker::AccountPickerCommand::SubmitInput(input) => {
                crate::tui::app::auth::handle_account_command_remote(self, &input, remote).await?;
            }
            crate::tui::account_picker::AccountPickerCommand::PromptValue {
                prompt,
                command_prefix,
                empty_value,
                status_notice,
            } => self.prompt_account_value(prompt, command_prefix, empty_value, status_notice),
            crate::tui::account_picker::AccountPickerCommand::PromptNew { provider } => {
                self.prompt_new_account_label(provider)
            }
            other => {
                if let Some(command) = crate::tui::app::auth::account_command_from_picker(&other) {
                    crate::tui::app::auth::execute_account_command_remote(self, command, remote)
                        .await?;
                }
            }
        }
        Ok(())
    }
}

pub(in crate::tui::app) async fn handle_remote_key(
    app: &mut App,
    code: KeyCode,
    modifiers: KeyModifiers,
    remote: &mut RemoteConnection,
) -> Result<()> {
    handle_remote_key_internal(app, code, modifiers, remote, None).await
}

#[path = "key_handling/key_events.rs"]
mod key_events;
pub(in crate::tui::app) use key_events::handle_remote_key_event;
use key_events::handle_remote_key_internal;
/// Everything a plain Enter does with the composer contents in a remote
/// session: slash commands, shell commands, and prompts. Shared by the key
/// handler and by the turn-end dispatcher for commands queued with Ctrl+Enter.
pub(in crate::tui::app) async fn submit_remote_enter_input(
    app: &mut App,
    remote: &mut RemoteConnection,
) -> Result<()> {
    if app.activate_picker_from_preview() {
        return Ok(());
    }
    if !app.input.is_empty() {
        let command = input::expand_paste_placeholders(app, &app.input)
            .trim()
            .to_string();
        if !app_mod::commands_cloud::check_cloud_handoff_input(app, &command) {
            return Ok(());
        }
        if app.cloud_handoff_blocks_work()
            && app_mod::commands_cloud::parse_cloud_command(app.input.trim()).is_none()
        {
            app.set_status_notice(
                "Cloud handoff pending. Use /cloud cancel before submitting more work.",
            );
            return Ok(());
        }
        let prepared = input::take_prepared_input(app);
        return submit_prepared_remote_enter_input(app, remote, prepared).await;
    }
    Ok(())
}

/// Already-expanded input shares Enter's wire-backed handlers without touching
/// the composer or consulting the laptop's skill catalog in SSH mode.
pub(in crate::tui::app) async fn submit_prepared_remote_enter_input(
    app: &mut App,
    remote: &mut RemoteConnection,
    prepared: input::PreparedInput,
) -> Result<()> {
    {
        if !prepared.images.is_empty()
            && matches!(
                app_mod::commands_cloud::parse_cloud_command(prepared.expanded.trim()),
                Some(
                    app_mod::commands_cloud::CloudCommand::Move { .. }
                        | app_mod::commands_cloud::CloudCommand::MoveSelected { .. }
                        | app_mod::commands_cloud::CloudCommand::Return
                )
            )
        {
            // Alternate slash submissions already consumed the composer, so
            // the regular handoff guard cannot see these attachments anymore.
            super::input_dispatch::restore_prepared_remote_input(app, prepared);
            app.push_display_message(DisplayMessage::error(
                "Cloud handoff not queued: attached images would be lost. Send or save them, then retry. Nothing was removed.",
            ));
            return Ok(());
        }
        if !app_mod::commands_cloud::check_cloud_handoff_input(app, prepared.expanded.trim()) {
            super::input_dispatch::restore_prepared_remote_input(app, prepared);
            return Ok(());
        }
        if app.cloud_handoff_blocks_work()
            && app_mod::commands_cloud::parse_cloud_command(prepared.expanded.trim()).is_none()
        {
            super::input_dispatch::restore_prepared_remote_input(app, prepared);
            app.set_status_notice(
                "Cloud handoff pending. Use /cloud cancel before submitting more work.",
            );
            return Ok(());
        }
        let trimmed = prepared.expanded.trim();

        // Before the SSH gate: `/local` must work from a client attached
        // to the cloud copy, because the return is coordinated locally.
        if app_mod::commands_cloud::parse_cloud_command(trimmed).is_some() {
            let session_id = app_mod::commands::active_session_id(app);
            if crate::tui::is_ssh_remote()
                && matches!(
                    app_mod::commands_cloud::parse_cloud_command(trimmed),
                    Some(
                        app_mod::commands_cloud::CloudCommand::Move { .. }
                            | app_mod::commands_cloud::CloudCommand::MoveSelected { .. }
                    )
                )
            {
                app.push_display_message(DisplayMessage::error(
                            "This session already runs on a remote host. Use /local to bring it back first.".to_string(),
                        ));
                return Ok(());
            }
            if matches!(
                app_mod::commands_cloud::parse_cloud_command(trimmed),
                Some(
                    app_mod::commands_cloud::CloudCommand::Move { .. }
                        | app_mod::commands_cloud::CloudCommand::MoveSelected { .. }
                )
            ) {
                // A fresh session is not on disk until its first message, and
                // `jcode cloud move` loads it from disk. The daemon holds the
                // model, effort and account, so ask it to write the file now.
                if let Err(error) = remote.persist_session().await {
                    app.push_display_message(DisplayMessage::error(format!(
                        "Could not save this session before moving it: {error}. Nothing moved."
                    )));
                    return Ok(());
                }
            }
            app_mod::commands_cloud::handle_cloud_command(app, trimmed, &session_id);
            return Ok(());
        }

        if app_mod::auth::handle_ssh_usage_command_remote(app, trimmed, remote).await? {
            return Ok(());
        }
        if super::ssh_sessions::handle_command(app, remote, trimmed).await? {
            return Ok(());
        }
        if crate::tui::is_ssh_remote()
            && let Some((command, args)) = ssh_runtime_command(trimmed)
        {
            remote.runtime_command(command, args).await?;
            app.set_status_notice(format!("Requested remote {command}"));
            return Ok(());
        }

        if crate::tui::is_ssh_remote() && trimmed == "/client-reload" {
            let Some(session_id) = app
                .remote_session_id
                .clone()
                .or_else(|| app.resume_session_id.clone())
            else {
                app.push_display_message(DisplayMessage::system(
                    "Wait for the remote session to connect before reloading the client."
                        .to_string(),
                ));
                return Ok(());
            };
            app.save_input_for_reload(&session_id);
            app.reload_requested = Some(session_id);
            app.should_quit = true;
            return Ok(());
        }
        if app_mod::commands_dispatch::handle_ssh_presentation_command(app, trimmed) {
            if matches!(trimmed.split_whitespace().next(), Some("/todos" | "/todo")) {
                remote.request_todos().await?;
            }
            return Ok(());
        }

        if app_mod::commands_dispatch::handle_ssh_unsupported_command(app, trimmed) {
            return Ok(());
        }

        if crate::tui::is_ssh_remote() && trimmed.starts_with('/') {
            let token = trimmed.split_whitespace().next();
            if !app_mod::registered_command_names().any(|name| Some(name) == token) {
                // Local prefix parsers are not a remote skill resolver. In
                // particular /plan-custom and /improve-custom must not become
                // client-side loop commands or consult laptop session storage.
                return submit_prepared_remote_input(app, remote, prepared).await;
            }
        }

        if crate::tui::is_ssh_remote() && trimmed.split_whitespace().next() == Some("/git") {
            if matches!(trimmed, "/git" | "/git status") {
                // The daemon resolves its own CWD. Never consult local git or
                // interpolate the remote working directory into a shell string.
                super::input_dispatch::submit_remote_input_shell(
                    app,
                    remote,
                    prepared.raw_input,
                    "git status --short --branch".to_string(),
                )
                .await?;
            } else {
                app.push_display_message(DisplayMessage::error("Usage: /git or /git status"));
            }
            return Ok(());
        }

        if let Some(topic) = trimmed
            .strip_prefix("/help ")
            .or_else(|| trimmed.strip_prefix("/? "))
        {
            if let Some(help) = app.command_help(topic) {
                app.push_display_message(DisplayMessage::system(help));
            } else {
                app.push_display_message(DisplayMessage::error(format!(
                    "Unknown command '{}'. Use /help to list commands.",
                    topic.trim()
                )));
            }
            return Ok(());
        }

        if trimmed == "/help" || trimmed == "/?" || trimmed == "/commands" {
            app.help_scroll = Some(0);
            return Ok(());
        }

        if app_mod::commands::handle_dictation_command(app, trimmed) {
            return Ok(());
        }

        if handle_remote_rewind_command(app, remote, trimmed).await? {
            return Ok(());
        }

        if trimmed == "/reload" {
            let client_needs_reload = app.has_newer_binary();
            let server_needs_reload = app.remote_server_has_update.unwrap_or(client_needs_reload);

            if !client_needs_reload && !server_needs_reload {
                app.push_display_message(DisplayMessage::system(
                    "No newer binary found. Nothing to reload.".to_string(),
                ));
                return Ok(());
            }

            if server_needs_reload {
                app.append_reload_message("Reloading server with newer binary...");
                remote.reload().await?;
            }

            if client_needs_reload {
                app.push_display_message(DisplayMessage::system(
                    "Reloading client with newer binary...".to_string(),
                ));
                let session_id = app.reload_handoff_session_id();
                app.save_input_for_reload(&session_id);
                app.reload_requested = Some(session_id);
                app.should_quit = true;
            }
            return Ok(());
        }

        if trimmed == "/client-reload" {
            app.push_display_message(DisplayMessage::system("Reloading client...".to_string()));
            let session_id = app.reload_handoff_session_id();
            app.save_input_for_reload(&session_id);
            app.reload_requested = Some(session_id);
            app.should_quit = true;
            return Ok(());
        }

        if trimmed == "/server-reload" {
            app.append_reload_message("Reloading server...");
            remote.reload().await?;
            return Ok(());
        }

        if trimmed == "/continue" || trimmed == "/resumeall" || trimmed == "/resume-all" {
            app.push_display_message(DisplayMessage::system(
                "Continuing all interrupted sessions...".to_string(),
            ));
            match remote.resume_all_sessions().await {
                Ok(_) => app.set_status_notice("Continuing interrupted sessions..."),
                Err(error) => {
                    app.push_display_message(DisplayMessage::error(format!(
                        "Failed to continue sessions: {}",
                        error
                    )));
                    app.set_status_notice("Continue all failed");
                }
            }
            return Ok(());
        }

        if trimmed == "/rebuild" {
            let session_id = app
                .remote_session_id
                .clone()
                .unwrap_or_else(|| crate::id::new_id("ses"));
            app.start_background_client_rebuild(session_id);
            return Ok(());
        }

        if trimmed == "/update" {
            handle_remote_update_command(app, remote).await?;
            return Ok(());
        }

        if trimmed == "/quit" {
            crate::telemetry::end_session_with_reason(
                app.provider.name(),
                &app.provider.model(),
                crate::telemetry::SessionEndReason::NormalExit,
            );
            // In remote mode the shared server owns session lifecycle persistence.
            // Exiting this client should not overwrite the server's session file.
            app.should_quit = true;
            return Ok(());
        }

        if app_mod::model_context::is_refresh_model_list_command(trimmed)
            || matches!(trimmed, "/model refresh" | "/models refresh")
        {
            app.pending_remote_model_refresh_snapshot = Some((
                app.remote_available_entries.clone(),
                app.remote_model_options.clone(),
            ));
            super::super::local::handle_ui_activity(
                app,
                crate::bus::UiActivity::catalog(
                    app.remote_session_id
                        .clone()
                        .or_else(|| Some(app.session.id.clone())),
                    "Model List Refresh Started\n\nAsked the remote server to refresh the provider model catalog. Jcode will show the discovered model and route changes when the server responds.",
                    Some("Refreshing model list..."),
                ),
            );
            match remote.refresh_models().await {
                Ok(()) => app.set_status_notice("Refreshing model list..."),
                Err(error) => {
                    app.pending_remote_model_refresh_snapshot = None;
                    app.push_display_message(DisplayMessage::error(format!(
                        "Failed to refresh model list: {}",
                        error
                    )));
                    app.set_status_notice("Model list refresh failed");
                }
            }
            return Ok(());
        }

        if trimmed == "/model" || trimmed == "/models" {
            // Opening the picker is a read-only UI action. The session
            // bootstrap and explicit `/model refresh` command own
            // catalog I/O; doing it here races startup and briefly
            // replaces the session catalog with remote fallback rows.
            app.open_model_picker();
            return Ok(());
        }

        if app.handle_usage_reset_command(trimmed) {
            return Ok(());
        }

        if app_mod::commands::handle_usage_command(app, trimmed) {
            return Ok(());
        }

        if app_mod::commands::handle_agents_command(app, trimmed) {
            return Ok(());
        }

        if trimmed.starts_with("/subagent-model") {
            let rest = trimmed
                .strip_prefix("/subagent-model")
                .unwrap_or_default()
                .trim();
            if rest.is_empty() || matches!(rest, "show" | "status") {
                let current_model = app
                    .remote_provider_model
                    .clone()
                    .unwrap_or_else(|| app.provider.model());
                let summary = match app.session.subagent_model.as_deref() {
                    Some(model) => format!("fixed {}", model),
                    None => format!("inherit current ({})", current_model),
                };
                app.push_display_message(DisplayMessage::system(format!(
                            "Subagent model for this session: {}\n\nUse /subagent-model <name> to pin a model, or /subagent-model inherit to use the current model.",
                            summary
                        )));
                return Ok(());
            }
            if matches!(rest, "inherit" | "reset" | "clear") {
                let current_model = app
                    .remote_provider_model
                    .clone()
                    .unwrap_or_else(|| app.provider.model());
                remote.set_subagent_model(None).await?;
                app.session.subagent_model = None;
                app.push_display_message(DisplayMessage::system(format!(
                    "Subagent model reset to inherit the current model ({}).",
                    current_model
                )));
                app.set_status_notice("Subagent model: inherit");
                return Ok(());
            }
            remote.set_subagent_model(Some(rest.to_string())).await?;
            app.session.subagent_model = Some(rest.to_string());
            app.push_display_message(DisplayMessage::system(format!(
                "Subagent model pinned to {} for this session.",
                rest
            )));
            app.set_status_notice(format!("Subagent model → {}", rest));
            return Ok(());
        }

        if trimmed.starts_with("/subagent") {
            let rest = trimmed.strip_prefix("/subagent").unwrap_or_default().trim();
            if rest.is_empty() {
                app.push_display_message(DisplayMessage::error(
                            "Usage: /subagent [--type <kind>] [--model <name>] [--continue <session_id>] <prompt>",
                        ));
                return Ok(());
            }
            match app_mod::commands::parse_manual_subagent_spec(rest) {
                Ok(spec) => {
                    remote
                        .run_subagent(spec.prompt, spec.subagent_type, spec.model, spec.session_id)
                        .await?;
                    app.subagent_status = Some("starting subagent".to_string());
                    app.set_status_notice("Running subagent");
                }
                Err(error) => {
                    app.push_display_message(DisplayMessage::error(format!(
                                "{}\nUsage: /subagent [--type <kind>] [--model <name>] [--continue <session_id>] <prompt>",
                                error
                            )));
                }
            }
            return Ok(());
        }

        if let Some(model_name) = trimmed.strip_prefix("/model ") {
            let model_name = model_name.trim();
            if model_name.is_empty() {
                app.push_display_message(DisplayMessage::error("Usage: /model <name>"));
                return Ok(());
            }
            app.upstream_provider = None;
            remote.set_model(model_name).await?;
            app.remote_model_switch_in_flight = true;
            return Ok(());
        }

        if trimmed == "/effort" {
            let current = app.remote_reasoning_effort_hint();
            let current = current.as_deref();
            let label = current
                .map(app_mod::effort_display_label)
                .unwrap_or("default");
            let (provider_name, provider_model) = app.remote_effort_identity();
            let efforts = app_mod::inferred_reasoning_efforts(
                provider_name.as_deref(),
                provider_model.as_deref(),
            );
            if efforts.is_empty() {
                app.push_display_message(DisplayMessage::system(
                    "Reasoning effort not available for this provider.".to_string(),
                ));
                return Ok(());
            }
            let list: Vec<String> = efforts
                .iter()
                .map(|e| {
                    if Some(*e) == current {
                        format!("{} <- current", app_mod::effort_display_label(e))
                    } else {
                        app_mod::effort_display_label(e).to_string()
                    }
                })
                .collect();
            app.push_display_message(DisplayMessage::system(format!(
                "Effort: {}\nAvailable: {}\nUse /effort <level> or {} to change.",
                label,
                list.join(" · "),
                crate::tui::keybind::effort_switch_keys_label()
            )));
            return Ok(());
        }

        if let Some(level) = trimmed.strip_prefix("/effort ") {
            let level = level.trim();
            if level.is_empty() {
                app.push_display_message(DisplayMessage::error("Usage: /effort <level>"));
                return Ok(());
            }
            let (provider_name, provider_model) = app.remote_effort_identity();
            let efforts = app_mod::inferred_reasoning_efforts(
                provider_name.as_deref(),
                provider_model.as_deref(),
            );
            if efforts.contains(&level) {
                app.remote_reasoning_effort = Some(level.to_string());
                app.invalidate_model_picker_cache();
                app.set_status_notice(format!(
                    "Effort: {} (will apply to next request)",
                    app_mod::effort_display_label(level)
                ));
            }
            remote.set_reasoning_effort(level).await?;
            return Ok(());
        }

        if matches!(trimmed, "/fast default" | "/fast default status") {
            let default_tier = crate::config::Config::load().provider.openai_service_tier;
            let default_enabled = default_tier.as_deref() == Some("priority");
            let default_label = default_tier
                .as_deref()
                .map(app_mod::service_tier_display_label)
                .unwrap_or("Standard");
            app.push_display_message(DisplayMessage::system(app_mod::fast_mode_default_message(
                default_enabled,
                default_label,
            )));
            return Ok(());
        }

        if let Some(mode) = trimmed.strip_prefix("/fast default ") {
            let mode = mode.trim().to_ascii_lowercase();
            match mode.as_str() {
                "on" => {
                    app_mod::auth::save_openai_fast_setting_local(app, true);
                    request_remote_service_tier(app, remote, "priority").await?;
                }
                "off" => {
                    app_mod::auth::save_openai_fast_setting_local(app, false);
                    request_remote_service_tier(app, remote, "off").await?;
                }
                "status" => {
                    let default_tier = crate::config::Config::load().provider.openai_service_tier;
                    let default_enabled = default_tier.as_deref() == Some("priority");
                    let default_label = default_tier
                        .as_deref()
                        .map(app_mod::service_tier_display_label)
                        .unwrap_or("Standard");
                    app.push_display_message(DisplayMessage::system(
                        app_mod::fast_mode_default_message(default_enabled, default_label),
                    ));
                }
                _ => {
                    app.push_display_message(DisplayMessage::error(
                        "Usage: /fast default [on|off|status]",
                    ));
                }
            }
            return Ok(());
        }

        if matches!(trimmed, "/fast" | "/fast status") {
            let current = app.remote_service_tier.as_deref();
            let enabled = app_mod::service_tier_is_fast(current);
            let current_label = current
                .map(app_mod::service_tier_display_label)
                .unwrap_or("Standard");
            let default_tier = crate::config::Config::load().provider.openai_service_tier;
            let default_enabled = default_tier.as_deref() == Some("priority");
            let default_label = default_tier
                .as_deref()
                .map(app_mod::service_tier_display_label)
                .unwrap_or("Standard");
            app.push_display_message(DisplayMessage::system(app_mod::fast_mode_overview_message(
                enabled,
                current_label,
                default_enabled,
                default_label,
            )));
            return Ok(());
        }

        if let Some(mode) = trimmed.strip_prefix("/fast ") {
            let mode = mode.trim().to_ascii_lowercase();
            let service_tier = match mode.as_str() {
                "on" => "priority",
                "ultra" | "ultrafast" => "ultrafast",
                "off" => "off",
                "status" => {
                    let current = app.remote_service_tier.as_deref();
                    let enabled = app_mod::service_tier_is_fast(current);
                    let current_label = current
                        .map(app_mod::service_tier_display_label)
                        .unwrap_or("Standard");
                    let default_tier = crate::config::Config::load().provider.openai_service_tier;
                    let default_enabled = default_tier.as_deref() == Some("priority");
                    let default_label = default_tier
                        .as_deref()
                        .map(app_mod::service_tier_display_label)
                        .unwrap_or("Standard");
                    app.push_display_message(DisplayMessage::system(
                        app_mod::fast_mode_overview_message(
                            enabled,
                            current_label,
                            default_enabled,
                            default_label,
                        ),
                    ));
                    return Ok(());
                }
                _ => {
                    app.push_display_message(DisplayMessage::error(
                        "Usage: /fast [on|ultra|off|status|default ...]",
                    ));
                    return Ok(());
                }
            };
            request_remote_service_tier(app, remote, service_tier).await?;
            return Ok(());
        }

        if trimmed == "/transport" {
            let current = app.remote_transport.as_deref().unwrap_or("unknown");
            let transports = ["auto", "https", "websocket"];
            let list: Vec<String> = transports
                .iter()
                .map(|t| {
                    if Some(*t) == app.remote_transport.as_deref() {
                        format!("{} <- current", t)
                    } else {
                        t.to_string()
                    }
                })
                .collect();
            app.push_display_message(DisplayMessage::system(format!(
                "Transport: {}\nAvailable: {}\nUse /transport <mode> to change.",
                current,
                list.join(" · ")
            )));
            return Ok(());
        }

        if let Some(mode) = trimmed.strip_prefix("/transport ") {
            let mode = mode.trim();
            if mode.is_empty() {
                app.push_display_message(DisplayMessage::error("Usage: /transport <mode>"));
                return Ok(());
            }
            remote.set_transport(mode).await?;
            return Ok(());
        }

        if crate::tui::app::auth::handle_account_command_remote(app, trimmed, remote).await? {
            return Ok(());
        }

        if trimmed == "/autoreview" || trimmed == "/autoreview status" {
            app.push_display_message(DisplayMessage::system(
                app_mod::commands::autoreview_status_message(app),
            ));
            return Ok(());
        }

        if trimmed == "/autojudge" || trimmed == "/autojudge status" {
            app.push_display_message(DisplayMessage::system(
                app_mod::commands::autojudge_status_message(app),
            ));
            return Ok(());
        }

        if trimmed == "/autoreview on" {
            remote
                .set_feature(crate::protocol::FeatureToggle::Autoreview, true)
                .await?;
            app.set_autoreview_feature_enabled(true);
            app.set_status_notice("Autoreview: ON");
            app.push_display_message(DisplayMessage::system(
                "Autoreview enabled for this session.".to_string(),
            ));
            return Ok(());
        }

        if trimmed == "/autoreview off" {
            remote
                .set_feature(crate::protocol::FeatureToggle::Autoreview, false)
                .await?;
            app.set_autoreview_feature_enabled(false);
            app.set_status_notice("Autoreview: OFF");
            app.push_display_message(DisplayMessage::system(
                "Autoreview disabled for this session.".to_string(),
            ));
            return Ok(());
        }

        if trimmed == "/autoreview now" {
            let parent_session_id = app_mod::commands::current_feedback_target_session_id(app);
            app_mod::commands::queue_review_spawn_remote(
                app,
                "Autoreview",
                parent_session_id.clone(),
                app_mod::commands::build_autoreview_startup_message(&parent_session_id),
                app_mod::commands::current_autoreview_model_override(app),
                None,
            );
            if app.is_processing {
                app.set_status_notice("Autoreview queued");
            } else {
                app.pending_split_request = false;
                begin_remote_split_launch(app, "Autoreview");
                if let Err(error) = remote.split().await {
                    finish_remote_split_launch(app);
                    app.pending_split_startup_message = None;
                    app.pending_split_parent_session_id = None;
                    app.pending_split_prompt = None;
                    app.pending_split_model_override = None;
                    app.pending_split_provider_key_override = None;
                    app.pending_split_label = None;
                    app.push_display_message(DisplayMessage::error(format!(
                        "Failed to launch autoreview session: {}",
                        error
                    )));
                    app.set_status_notice("Autoreview launch failed");
                }
            }
            return Ok(());
        }

        if trimmed == "/autojudge on" {
            remote
                .set_feature(crate::protocol::FeatureToggle::Autojudge, true)
                .await?;
            app.set_autojudge_feature_enabled(true);
            app.set_status_notice("Autojudge: ON");
            app.push_display_message(DisplayMessage::system(
                "Autojudge enabled for this session.".to_string(),
            ));
            return Ok(());
        }

        if trimmed == "/autojudge off" {
            remote
                .set_feature(crate::protocol::FeatureToggle::Autojudge, false)
                .await?;
            app.set_autojudge_feature_enabled(false);
            app.set_status_notice("Autojudge: OFF");
            app.push_display_message(DisplayMessage::system(
                "Autojudge disabled for this session.".to_string(),
            ));
            return Ok(());
        }

        if trimmed == "/autojudge now" {
            let parent_session_id = app_mod::commands::current_feedback_target_session_id(app);
            app_mod::commands::queue_review_spawn_remote(
                app,
                "Autojudge",
                parent_session_id.clone(),
                app_mod::commands::build_autojudge_startup_message(&parent_session_id),
                app_mod::commands::current_autojudge_model_override(app),
                None,
            );
            if app.is_processing {
                app.set_status_notice("Autojudge queued");
            } else {
                app.pending_split_request = false;
                begin_remote_split_launch(app, "Autojudge");
                if let Err(error) = remote.split().await {
                    finish_remote_split_launch(app);
                    app.pending_split_startup_message = None;
                    app.pending_split_parent_session_id = None;
                    app.pending_split_prompt = None;
                    app.pending_split_model_override = None;
                    app.pending_split_provider_key_override = None;
                    app.pending_split_label = None;
                    app.push_display_message(DisplayMessage::error(format!(
                        "Failed to launch autojudge session: {}",
                        error
                    )));
                    app.set_status_notice("Autojudge launch failed");
                }
            }
            return Ok(());
        }

        if trimmed == "/review" {
            let (model_override, provider_key_override) =
                app_mod::commands::current_review_model_override(app);
            let parent_session_id = app_mod::commands::current_feedback_target_session_id(app);
            app_mod::commands::queue_review_spawn_remote(
                app,
                "Review",
                parent_session_id.clone(),
                app_mod::commands::build_review_startup_message(&parent_session_id),
                model_override,
                provider_key_override,
            );
            if app.is_processing {
                app.set_status_notice("Review queued");
            } else {
                app.pending_split_request = false;
                begin_remote_split_launch(app, "Review");
                if let Err(error) = remote.split().await {
                    finish_remote_split_launch(app);
                    app.pending_split_startup_message = None;
                    app.pending_split_parent_session_id = None;
                    app.pending_split_prompt = None;
                    app.pending_split_model_override = None;
                    app.pending_split_provider_key_override = None;
                    app.pending_split_label = None;
                    app.push_display_message(DisplayMessage::error(format!(
                        "Failed to launch review session: {}",
                        error
                    )));
                    app.set_status_notice("Review launch failed");
                }
            }
            return Ok(());
        }

        if trimmed == "/judge" {
            let (model_override, provider_key_override) =
                app_mod::commands::current_judge_model_override(app);
            let parent_session_id = app_mod::commands::current_feedback_target_session_id(app);
            app_mod::commands::queue_review_spawn_remote(
                app,
                "Judge",
                parent_session_id.clone(),
                app_mod::commands::build_judge_startup_message(&parent_session_id),
                model_override,
                provider_key_override,
            );
            if app.is_processing {
                app.set_status_notice("Judge queued");
            } else {
                app.pending_split_request = false;
                begin_remote_split_launch(app, "Judge");
                if let Err(error) = remote.split().await {
                    finish_remote_split_launch(app);
                    app.pending_split_startup_message = None;
                    app.pending_split_parent_session_id = None;
                    app.pending_split_prompt = None;
                    app.pending_split_model_override = None;
                    app.pending_split_provider_key_override = None;
                    app.pending_split_label = None;
                    app.push_display_message(DisplayMessage::error(format!(
                        "Failed to launch judge session: {}",
                        error
                    )));
                    app.set_status_notice("Judge launch failed");
                }
            }
            return Ok(());
        }

        if trimmed.starts_with("/autoreview ") {
            app.push_display_message(DisplayMessage::error(
                "Usage: /autoreview [on|off|status|now]".to_string(),
            ));
            return Ok(());
        }

        if trimmed.starts_with("/autojudge ") {
            app.push_display_message(DisplayMessage::error(
                "Usage: /autojudge [on|off|status|now]".to_string(),
            ));
            return Ok(());
        }

        if trimmed.starts_with("/review ") {
            app.push_display_message(DisplayMessage::error("Usage: /review".to_string()));
            return Ok(());
        }

        if trimmed.starts_with("/judge ") {
            app.push_display_message(DisplayMessage::error("Usage: /judge".to_string()));
            return Ok(());
        }

        if trimmed == "/memory status" {
            let default_enabled = crate::config::config().features.memory;
            app.push_display_message(DisplayMessage::system(format!(
                "Memory feature: {} (config default: {})",
                if app.memory_enabled {
                    "enabled"
                } else {
                    "disabled"
                },
                if default_enabled {
                    "enabled"
                } else {
                    "disabled"
                }
            )));
            return Ok(());
        }

        if trimmed == "/memory" {
            let new_state = !app.memory_enabled;
            remote
                .set_feature(crate::protocol::FeatureToggle::Memory, new_state)
                .await?;
            app.set_memory_feature_enabled(new_state);
            let label = if new_state { "ON" } else { "OFF" };
            app.set_status_notice(format!("Memory: {}", label));
            app.push_display_message(DisplayMessage::system(format!(
                "Memory feature {} for this session.",
                if new_state { "enabled" } else { "disabled" }
            )));
            return Ok(());
        }

        if trimmed == "/memory on" {
            remote
                .set_feature(crate::protocol::FeatureToggle::Memory, true)
                .await?;
            app.set_memory_feature_enabled(true);
            app.set_status_notice("Memory: ON");
            app.push_display_message(DisplayMessage::system(
                "Memory feature enabled for this session.".to_string(),
            ));
            return Ok(());
        }

        if trimmed == "/memory off" {
            remote
                .set_feature(crate::protocol::FeatureToggle::Memory, false)
                .await?;
            app.set_memory_feature_enabled(false);
            app.set_status_notice("Memory: OFF");
            app.push_display_message(DisplayMessage::system(
                "Memory feature disabled for this session.".to_string(),
            ));
            return Ok(());
        }

        if trimmed.starts_with("/memory ") {
            app.push_display_message(DisplayMessage::error(
                "Usage: /memory [on|off|status]".to_string(),
            ));
            return Ok(());
        }

        if trimmed == "/clear" {
            remote.clear().await?;
            app.clear_provider_messages();
            app.clear_display_messages();
            app.queued_messages.clear();
            app.queued_commands.clear();
            app.pasted_contents.clear();
            app.pending_images.clear();
            app.clear_inline_image_state();
            app.clear_streaming_render_state();
            app.clear_live_usage_state();
            // Full transcript discard: diagrams and side panel pages
            // are both orphaned (same rationale as
            // reset_current_session; side panel is #605).
            crate::tui::mermaid::clear_active_diagrams();
            app.swarm_plan_items.clear();
            app.swarm_plan_version = None;
            app.swarm_plan_swarm_id = None;
            super::super::commands_review::clear_side_panel_for_new_session(app);
            app.is_processing = false;
            app.status = ProcessingStatus::Idle;
            app.set_status_notice("Session cleared");
            return Ok(());
        }

        if trimmed == "/fork" || trimmed == "/split" {
            app.push_display_message(DisplayMessage::system("Forking session...".to_string()));
            remote.split().await?;
            return Ok(());
        }

        if trimmed == "/btw" || trimmed.starts_with("/btw ") || trimmed.starts_with("/fork ") {
            let prompt = trimmed
                .strip_prefix("/btw")
                .or_else(|| trimmed.strip_prefix("/fork"))
                .unwrap_or_default()
                .trim();
            if prompt.is_empty() {
                app.push_display_message(DisplayMessage::error(
                    "Usage: /btw <question>".to_string(),
                ));
                return Ok(());
            }
            // Attached images belong to the forked prompt, not the
            // parent's next message.
            let images = app.take_pending_image_payloads();
            let prepared = input::PreparedInput {
                raw_input: prompt.to_string(),
                expanded: prompt.to_string(),
                images,
            };
            route_prepared_input_to_new_remote_session(app, remote, prepared).await?;
            return Ok(());
        }

        if trimmed == "/todo" || trimmed == "/todos" || trimmed.starts_with("/todos ") {
            app_mod::todos_view::handle_todos_view_command(app, trimmed);
            if crate::tui::is_ssh_remote() {
                remote.request_todos().await?;
            }
            return Ok(());
        }

        if trimmed == "/observe"
            || trimmed == "/observe on"
            || trimmed == "/observe off"
            || trimmed == "/observe status"
            || trimmed == "/splitview"
            || trimmed == "/splitview on"
            || trimmed == "/splitview off"
            || trimmed == "/splitview status"
            || trimmed == "/split-view"
            || trimmed == "/split-view on"
            || trimmed == "/split-view off"
            || trimmed == "/split-view status"
        {
            let _ = app_mod::commands::handle_session_command(app, trimmed);
            return Ok(());
        }

        if app_mod::commands::handle_test_command(app, trimmed) {
            return Ok(());
        }

        if app_mod::commands::handle_disabled_mission_command(app, trimmed) {
            return Ok(());
        }

        if app_mod::commands::handle_goals_command(app, trimmed) {
            return Ok(());
        }

        if trimmed == "/swarm" || trimmed == "/swarm status" {
            let default_enabled = crate::config::config().features.swarm;
            app.push_display_message(DisplayMessage::system(format!(
                "Swarm feature: {} (config default: {})",
                if app.swarm_enabled {
                    "enabled"
                } else {
                    "disabled"
                },
                if default_enabled {
                    "enabled"
                } else {
                    "disabled"
                }
            )));
            return Ok(());
        }

        if trimmed == "/swarm on" {
            remote
                .set_feature(crate::protocol::FeatureToggle::Swarm, true)
                .await?;
            app.set_swarm_feature_enabled(true);
            app.set_status_notice("Swarm: ON");
            app.push_display_message(DisplayMessage::system(
                "Swarm feature enabled for this session.".to_string(),
            ));
            return Ok(());
        }

        if trimmed == "/swarm off" {
            remote
                .set_feature(crate::protocol::FeatureToggle::Swarm, false)
                .await?;
            app.set_swarm_feature_enabled(false);
            app.set_status_notice("Swarm: OFF");
            app.push_display_message(DisplayMessage::system(
                "Swarm feature disabled for this session.".to_string(),
            ));
            return Ok(());
        }

        if trimmed.starts_with("/swarm ") {
            app.push_display_message(DisplayMessage::error(
                "Usage: /swarm [on|off|status]".to_string(),
            ));
            return Ok(());
        }

        if trimmed == "/resume" || trimmed == "/sessions" || trimmed == "/session" {
            app.open_session_picker();
            app.record_keybinding_slow(crate::tui::app::shortcut_hints::LearnableAction::Resume);
            return Ok(());
        }

        if trimmed == "/active" {
            app.open_active_sessions_picker();
            return Ok(());
        }

        if trimmed == "/save" || trimmed.starts_with("/save ") {
            let label = trimmed.strip_prefix("/save").unwrap_or_default().trim();
            let label = if label.is_empty() {
                None
            } else {
                Some(label.to_string())
            };
            if let Err(e) = persist_remote_session_metadata(app, |session| {
                session.mark_saved(label.clone());
            }) {
                app.push_display_message(DisplayMessage::error(format!(
                    "Failed to save session: {}",
                    e
                )));
                return Ok(());
            }
            // The daemon's in-memory session owns later writes. Without
            // this it would persist `saved: false` on its next save.
            remote.set_session_saved(true, label.clone()).await?;
            crate::tui::session_picker::invalidate_session_list_cache();
            if app.memory_enabled
                && let Err(err) = remote.trigger_memory_extraction().await
            {
                crate::logging::info(&format!(
                    "Failed to trigger memory extraction for saved remote session: {}",
                    err
                ));
            }
            let name = app.session.display_name().to_string();
            let msg = if let Some(ref lbl) = app.session.save_label {
                format!(
                    "📌 Session {} saved as \"{}\". It will appear at the top of /resume.",
                    name, lbl,
                )
            } else {
                format!(
                    "📌 Session {} saved. It will appear at the top of /resume.",
                    name,
                )
            };
            app.push_display_message(DisplayMessage::system(msg));
            app.set_status_notice("Session saved");
            return Ok(());
        }

        if trimmed == "/unsave" {
            if let Err(e) = persist_remote_session_metadata(app, |session| {
                session.unmark_saved();
            }) {
                app.push_display_message(DisplayMessage::error(format!(
                    "Failed to save session: {}",
                    e
                )));
                return Ok(());
            }
            remote.set_session_saved(false, None).await?;
            crate::tui::session_picker::invalidate_session_list_cache();
            let name = app.session.display_name().to_string();
            app.push_display_message(DisplayMessage::system(format!(
                "Removed bookmark from session {}.",
                name,
            )));
            app.set_status_notice("Bookmark removed");
            return Ok(());
        }

        if trimmed == "/rename" || trimmed.starts_with("/rename ") {
            let title = trimmed.strip_prefix("/rename").unwrap_or_default().trim();
            if title.is_empty() {
                app.push_display_message(DisplayMessage::error(
                    "Usage: /rename <session name> or /rename --clear".to_string(),
                ));
                return Ok(());
            }

            if title == "--clear" {
                remote.rename_session(None).await?;
                app.set_status_notice("Clearing session name...");
                return Ok(());
            }

            remote.rename_session(Some(title.to_string())).await?;
            app.set_status_notice("Renaming session...");
            return Ok(());
        }

        if trimmed == "/transfer" {
            if app.pending_transfer_request {
                app.push_display_message(DisplayMessage::system(
                    "A transfer is already pending.".to_string(),
                ));
                app.set_status_notice("Transfer already pending");
                return Ok(());
            }

            app.pending_split_label = Some("Transfer".to_string());
            if app.is_processing {
                let pause_message = app_mod::commands::transfer_pause_message();
                let pause_display = pause_message.clone();
                match remote
                    .soft_interrupt(pause_message, Vec::new(), false)
                    .await
                {
                    Ok(request_id) => {
                        app.track_pending_soft_interrupt(request_id, pause_display);
                        app.pending_transfer_request = true;
                        app.push_display_message(DisplayMessage::system(
                                    "Queued /transfer. The current session will be asked to pause, then the compacted handoff will open in a new window."
                                        .to_string(),
                                ));
                        app.set_status_notice("Transfer queued after current turn");
                    }
                    Err(error) => {
                        app.pending_split_label = None;
                        app.push_display_message(DisplayMessage::error(format!(
                            "Failed to queue transfer pause: {}",
                            error
                        )));
                        app.set_status_notice("Transfer queue failed");
                    }
                }
            } else {
                app.push_display_message(DisplayMessage::system(
                    "Preparing transfer...".to_string(),
                ));
                begin_remote_split_launch(app, "Transfer");
                if let Err(error) = remote.transfer().await {
                    finish_remote_split_launch(app);
                    app.pending_split_label = None;
                    app.push_display_message(DisplayMessage::error(format!(
                        "Failed to launch transfer session: {}",
                        error
                    )));
                    app.set_status_notice("Transfer launch failed");
                }
            }
            return Ok(());
        }

        if handle_workspace_command(app, remote, trimmed).await? {
            return Ok(());
        }

        if trimmed == "/commit"
            || trimmed == "/merge"
            || trimmed == "/merge-remote-release"
            || trimmed == "/commit-push"
            || trimmed == "/commit-and-push"
            || trimmed == "/fast-release"
            || trimmed == "/fast-macos-release"
            || trimmed == "/remote-release"
            || trimmed == "/cut-release"
            || trimmed == "/commit-push-release"
            || trimmed == "/triage"
            || trimmed.starts_with("/triage ")
        {
            let is_triage = trimmed == "/triage" || trimmed.starts_with("/triage ");
            let is_fast_release = matches!(
                trimmed,
                "/fast-release" | "/cut-release" | "/commit-push-release"
            );
            let is_remote_release = trimmed == "/remote-release";
            let is_fast_macos_release = trimmed == "/fast-macos-release";
            let is_merge = trimmed == "/merge";
            let is_merge_remote_release = trimmed == "/merge-remote-release";
            let is_push = matches!(trimmed, "/commit-push" | "/commit-and-push");
            let prompt = if is_merge_remote_release {
                app_mod::commands::build_merge_remote_release_prompt()
            } else if is_merge {
                app_mod::commands::build_merge_prompt()
            } else if is_triage {
                app_mod::commands::build_triage_prompt(
                    trimmed.strip_prefix("/triage").unwrap_or_default(),
                )
            } else if is_fast_macos_release {
                app_mod::commands::build_fast_macos_release_prompt()
            } else if is_fast_release {
                app_mod::commands::build_fast_release_prompt()
            } else if is_remote_release {
                app_mod::commands::build_remote_release_prompt()
            } else if is_push {
                app_mod::commands::build_commit_push_prompt()
            } else {
                app_mod::commands::build_commit_prompt()
            };
            let launch_notice = |interrupted: bool| {
                if is_merge_remote_release {
                    app_mod::commands::merge_remote_release_launch_notice(interrupted)
                } else if is_merge {
                    app_mod::commands::merge_launch_notice(interrupted)
                } else if is_triage {
                    app_mod::commands::triage_launch_notice(interrupted)
                } else if is_fast_macos_release {
                    app_mod::commands::fast_macos_release_launch_notice(interrupted)
                } else if is_fast_release {
                    app_mod::commands::fast_release_launch_notice(interrupted)
                } else if is_remote_release {
                    app_mod::commands::remote_release_launch_notice(interrupted)
                } else if is_push {
                    app_mod::commands::commit_push_launch_notice(interrupted)
                } else {
                    app_mod::commands::commit_launch_notice(interrupted)
                }
            };
            let cmd_label = if is_merge_remote_release {
                "/merge-remote-release"
            } else if is_merge {
                "/merge"
            } else if is_triage {
                "/triage"
            } else if is_fast_macos_release {
                "/fast-macos-release"
            } else if is_fast_release {
                "/fast-release"
            } else if is_remote_release {
                "/remote-release"
            } else if is_push {
                "/commit-push"
            } else {
                "/commit"
            };
            if app.is_processing {
                app.push_display_message(DisplayMessage::system(launch_notice(true)));
                match remote
                    .soft_interrupt(prompt.clone(), Vec::new(), false)
                    .await
                {
                    Ok(request_id) => {
                        app.track_pending_soft_interrupt(request_id, prompt);
                        app.set_status_notice(format!("Interrupting for {}...", cmd_label));
                    }
                    Err(error) => {
                        app.push_display_message(DisplayMessage::error(format!(
                            "Failed to start {}: {}",
                            cmd_label, error
                        )));
                        app.set_status_notice(format!("{} failed", cmd_label));
                    }
                }
            } else {
                app.push_display_message(DisplayMessage::system(launch_notice(false)));
                input_dispatch::begin_remote_send(
                    app,
                    remote,
                    prompt,
                    Vec::new(),
                    false,
                    None,
                    false,
                    0,
                )
                .await?;
            }
            return Ok(());
        }

        if trimmed == "/compact" {
            app.push_display_message(DisplayMessage::system(
                "Requesting compaction...".to_string(),
            ));
            remote.compact().await?;
            return Ok(());
        }

        if trimmed == "/compact mode" || trimmed == "/compact mode status" {
            let mode = app
                .remote_compaction_mode
                .clone()
                .unwrap_or(crate::config::CompactionMode::Reactive);
            app.push_display_message(DisplayMessage::system(format!(
                        "Compaction mode: {}\nAvailable: reactive, proactive, semantic\nUse /compact mode <mode> to change it for this session.",
                        mode.as_str()
                    )));
            return Ok(());
        }

        if let Some(mode_str) = trimmed.strip_prefix("/compact mode ") {
            let mode_str = mode_str.trim();
            let Some(mode) = crate::config::CompactionMode::parse(mode_str) else {
                app.push_display_message(DisplayMessage::error(
                    "Usage: /compact mode <reactive|proactive|semantic>".to_string(),
                ));
                return Ok(());
            };
            remote.set_compaction_mode(mode).await?;
            return Ok(());
        }

        if app.pending_login.is_some() {
            app.input = trimmed.to_string();
            app.cursor_pos = app.input.len();
            app.submit_input();
            return Ok(());
        }

        if trimmed == "/z" || trimmed == "/zz" || trimmed == "/zzz" {
            use crate::provider::copilot::PremiumMode;
            let current = app.provider.premium_mode();

            if trimmed == "/z" {
                app.provider.set_premium_mode(PremiumMode::Normal);
                let _ = remote.set_premium_mode(PremiumMode::Normal as u8).await;
                let _ = crate::config::Config::set_copilot_premium(None);
                app.set_status_notice("Premium: normal");
                app.push_display_message(DisplayMessage::system(
                    "Premium request mode reset to normal. (saved to config)".to_string(),
                ));
                return Ok(());
            }

            let mode = if trimmed == "/zzz" {
                PremiumMode::Zero
            } else {
                PremiumMode::OnePerSession
            };
            if current == mode {
                app.provider.set_premium_mode(PremiumMode::Normal);
                let _ = remote.set_premium_mode(PremiumMode::Normal as u8).await;
                let _ = crate::config::Config::set_copilot_premium(None);
                app.set_status_notice("Premium: normal");
                app.push_display_message(DisplayMessage::system(
                    "Premium request mode reset to normal. (saved to config)".to_string(),
                ));
            } else {
                app.provider.set_premium_mode(mode);
                let _ = remote.set_premium_mode(mode as u8).await;
                let config_val = match mode {
                    PremiumMode::Zero => "zero",
                    PremiumMode::OnePerSession => "one",
                    PremiumMode::Normal => "normal",
                };
                let _ = crate::config::Config::set_copilot_premium(Some(config_val));
                let label = match mode {
                    PremiumMode::OnePerSession => "one premium per session",
                    PremiumMode::Zero => "zero premium requests",
                    PremiumMode::Normal => "normal",
                };
                app.set_status_notice(format!("Premium: {}", label));
                app.push_display_message(DisplayMessage::system(format!(
                    "Premium mode: {}. Toggle off with /z. (saved to config)",
                    label,
                )));
            }
            return Ok(());
        }

        if let Some(command) = app_mod::commands::parse_poke_command(trimmed) {
            match command {
                Err(error) => app.push_display_message(DisplayMessage::error(error)),
                Ok(app_mod::commands::PokeCommand::Status) => {
                    app.push_display_message(DisplayMessage::system(
                        app_mod::commands::poke_status_message(app),
                    ));
                }
                Ok(app_mod::commands::PokeCommand::Off) => {
                    let cleared = app_mod::commands::disable_auto_poke(app);
                    app.set_status_notice("Poke: OFF");
                    app.push_display_message(DisplayMessage::system(
                        app_mod::commands::poke_disabled_message(cleared),
                    ));
                }
                Ok(app_mod::commands::PokeCommand::Trigger)
                | Ok(app_mod::commands::PokeCommand::On) => {
                    match app_mod::commands::activate_auto_poke(app) {
                        app_mod::commands::PokeActivation::EnabledNoIncomplete => {
                            app.push_display_message(DisplayMessage::system(
                                app_mod::commands::poke_enabled_without_incomplete_message(),
                            ));
                        }
                        app_mod::commands::PokeActivation::Queued => {
                            app.push_display_message(DisplayMessage::system(
                                app_mod::commands::poke_queued_display_message(),
                            ));
                        }
                        app_mod::commands::PokeActivation::SendNow {
                            incomplete_count,
                            poke_msg,
                        } => {
                            app.push_display_message(DisplayMessage::system(
                                app_mod::commands::poke_triggered_display_message(incomplete_count),
                            ));

                            let _ = begin_remote_send(
                                app,
                                remote,
                                poke_msg,
                                vec![],
                                true,
                                None,
                                true,
                                0,
                            )
                            .await;
                            app.visible_turn_started = Some(Instant::now());
                        }
                    }
                }
            }
            return Ok(());
        }

        if let Some(command) = app_mod::commands::parse_plan_command(trimmed) {
            let prompt = app_mod::commands::build_plan_prompt(command.goal.as_deref());
            if app.is_processing {
                remote.cancel_with_reason("slash_plan").await?;
                app.set_status_notice("Interrupting for /plan...");
                app.push_display_message(DisplayMessage::system(
                    app_mod::commands::plan_launch_notice(command.goal.as_deref(), true),
                ));
                app.queued_messages.push(prompt);
            } else {
                app.push_display_message(DisplayMessage::system(
                    app_mod::commands::plan_launch_notice(command.goal.as_deref(), false),
                ));
                let _ = begin_remote_send(app, remote, prompt, vec![], true, None, true, 0).await;
            }
            return Ok(());
        }

        if let Some(command) = app_mod::commands::parse_improve_command(trimmed) {
            match command {
                Err(error) => app.push_display_message(DisplayMessage::error(error)),
                Ok(app_mod::commands::ImproveCommand::Resume) => {
                    let session_id = app
                        .remote_session_id
                        .clone()
                        .unwrap_or_else(|| app.session.id.clone());
                    let todos = crate::todo::load_todos(&session_id).unwrap_or_default();
                    let incomplete: Vec<_> = todos
                        .iter()
                        .filter(|todo| todo.status != "completed" && todo.status != "cancelled")
                        .collect();

                    let mode = app
                        .improve_mode
                        .or_else(|| {
                            app.session
                                .improve_mode
                                .map(app_mod::commands::restore_improve_mode)
                        })
                        .filter(|mode| mode.is_improve());
                    let Some(mode) = mode else {
                        app.push_display_message(DisplayMessage::system(
                                    "No saved improve run found for this session. Use /improve or /improve plan to start one."
                                        .to_string(),
                                ));
                        return Ok(());
                    };

                    persist_remote_session_metadata(app, |session| {
                        session.improve_mode =
                            Some(app_mod::commands::session_improve_mode_for(mode));
                    })?;
                    app.improve_mode = Some(mode);
                    let prompt = app_mod::commands::build_improve_resume_prompt(mode, &incomplete);

                    if app.is_processing {
                        remote.cancel_with_reason("slash_improve_resume").await?;
                        app.set_status_notice("Interrupting for /improve resume...");
                        app.push_display_message(DisplayMessage::system(format!(
                            "♻️ Interrupting and resuming {}...",
                            mode.status_label()
                        )));
                        app.queued_messages.push(prompt);
                    } else {
                        app.push_display_message(DisplayMessage::system(format!(
                            "♻️ Resuming {}...",
                            mode.status_label()
                        )));
                        let _ = begin_remote_send(app, remote, prompt, vec![], true, None, true, 0)
                            .await;
                    }
                }
                Ok(app_mod::commands::ImproveCommand::Status) => {
                    app.push_display_message(DisplayMessage::system(
                        app_mod::commands::format_improve_status(app),
                    ));
                }
                Ok(app_mod::commands::ImproveCommand::Stop) => {
                    let session_id = app
                        .remote_session_id
                        .clone()
                        .unwrap_or_else(|| app.session.id.clone());
                    let todos = crate::todo::load_todos(&session_id).unwrap_or_default();
                    let has_incomplete = todos
                        .iter()
                        .any(|todo| todo.status != "completed" && todo.status != "cancelled");

                    let active_improve_mode = app
                        .improve_mode
                        .or_else(|| {
                            app.session
                                .improve_mode
                                .map(app_mod::commands::restore_improve_mode)
                        })
                        .filter(|mode| mode.is_improve());

                    if active_improve_mode.is_none() && !app.is_processing && !has_incomplete {
                        app.push_display_message(DisplayMessage::system(
                            "No active improve loop to stop. Use /improve to start one."
                                .to_string(),
                        ));
                        return Ok(());
                    }

                    persist_remote_session_metadata(app, |session| {
                        session.improve_mode = None;
                    })?;
                    app.improve_mode = None;
                    let stop_prompt = app_mod::commands::improve_stop_prompt();
                    if app.is_processing {
                        remote.cancel_with_reason("slash_improve_stop").await?;
                        app.set_status_notice("Interrupting for /improve stop...");
                        app.push_display_message(DisplayMessage::system(
                            app_mod::commands::improve_stop_notice(true),
                        ));
                        app.queued_messages.push(stop_prompt);
                    } else {
                        app.push_display_message(DisplayMessage::system(
                            app_mod::commands::improve_stop_notice(false),
                        ));
                        let _ = begin_remote_send(
                            app,
                            remote,
                            stop_prompt,
                            vec![],
                            true,
                            None,
                            true,
                            0,
                        )
                        .await;
                    }
                }
                Ok(app_mod::commands::ImproveCommand::Run { plan_only, focus }) => {
                    let mode = app_mod::commands::improve_mode_for(plan_only);
                    persist_remote_session_metadata(app, |session| {
                        session.improve_mode =
                            Some(app_mod::commands::session_improve_mode_for(mode));
                    })?;
                    app.improve_mode = Some(mode);
                    let prompt =
                        app_mod::commands::build_improve_prompt(plan_only, focus.as_deref());
                    if app.is_processing {
                        remote.cancel_with_reason("slash_improve_run").await?;
                        app.set_status_notice(if plan_only {
                            "Interrupting for /improve plan..."
                        } else {
                            "Interrupting for /improve..."
                        });
                        app.push_display_message(DisplayMessage::system(
                            app_mod::commands::improve_launch_notice(
                                plan_only,
                                focus.as_deref(),
                                true,
                            ),
                        ));
                        app.queued_messages.push(prompt);
                    } else {
                        app.push_display_message(DisplayMessage::system(
                            app_mod::commands::improve_launch_notice(
                                plan_only,
                                focus.as_deref(),
                                false,
                            ),
                        ));

                        let _ = begin_remote_send(app, remote, prompt, vec![], true, None, true, 0)
                            .await;
                    }
                }
            }
            return Ok(());
        }

        if let Some(command) = app_mod::commands::parse_refactor_command(trimmed) {
            match command {
                Err(error) => app.push_display_message(DisplayMessage::error(error)),
                Ok(app_mod::commands::RefactorCommand::Resume) => {
                    let session_id = app
                        .remote_session_id
                        .clone()
                        .unwrap_or_else(|| app.session.id.clone());
                    let todos = crate::todo::load_todos(&session_id).unwrap_or_default();
                    let incomplete: Vec<_> = todos
                        .iter()
                        .filter(|todo| todo.status != "completed" && todo.status != "cancelled")
                        .collect();

                    let mode = app
                        .improve_mode
                        .or_else(|| {
                            app.session
                                .improve_mode
                                .map(app_mod::commands::restore_improve_mode)
                        })
                        .filter(|mode| mode.is_refactor());
                    let Some(mode) = mode else {
                        app.push_display_message(DisplayMessage::system(
                                    "No saved refactor run found for this session. Use /refactor or /refactor plan to start one."
                                        .to_string(),
                                ));
                        return Ok(());
                    };

                    persist_remote_session_metadata(app, |session| {
                        session.improve_mode =
                            Some(app_mod::commands::session_improve_mode_for(mode));
                    })?;
                    app.improve_mode = Some(mode);
                    let prompt = app_mod::commands::build_refactor_resume_prompt(mode, &incomplete);

                    if app.is_processing {
                        remote.cancel_with_reason("slash_refactor_resume").await?;
                        app.set_status_notice("Interrupting for /refactor resume...");
                        app.push_display_message(DisplayMessage::system(format!(
                            "♻️ Interrupting and resuming {}...",
                            mode.status_label()
                        )));
                        app.queued_messages.push(prompt);
                    } else {
                        app.push_display_message(DisplayMessage::system(format!(
                            "♻️ Resuming {}...",
                            mode.status_label()
                        )));
                        let _ = begin_remote_send(app, remote, prompt, vec![], true, None, true, 0)
                            .await;
                    }
                }
                Ok(app_mod::commands::RefactorCommand::Status) => {
                    app.push_display_message(DisplayMessage::system(
                        app_mod::commands::format_refactor_status(app),
                    ));
                }
                Ok(app_mod::commands::RefactorCommand::Stop) => {
                    let session_id = app
                        .remote_session_id
                        .clone()
                        .unwrap_or_else(|| app.session.id.clone());
                    let todos = crate::todo::load_todos(&session_id).unwrap_or_default();
                    let has_incomplete = todos
                        .iter()
                        .any(|todo| todo.status != "completed" && todo.status != "cancelled");

                    let active_refactor_mode = app
                        .improve_mode
                        .or_else(|| {
                            app.session
                                .improve_mode
                                .map(app_mod::commands::restore_improve_mode)
                        })
                        .filter(|mode| mode.is_refactor());

                    if active_refactor_mode.is_none() && !app.is_processing && !has_incomplete {
                        app.push_display_message(DisplayMessage::system(
                            "No active refactor loop to stop. Use /refactor to start one."
                                .to_string(),
                        ));
                        return Ok(());
                    }

                    persist_remote_session_metadata(app, |session| {
                        session.improve_mode = None;
                    })?;
                    app.improve_mode = None;
                    let stop_prompt = app_mod::commands::refactor_stop_prompt();
                    if app.is_processing {
                        remote.cancel_with_reason("slash_refactor_stop").await?;
                        app.set_status_notice("Interrupting for /refactor stop...");
                        app.push_display_message(DisplayMessage::system(
                            app_mod::commands::refactor_stop_notice(true),
                        ));
                        app.queued_messages.push(stop_prompt);
                    } else {
                        app.push_display_message(DisplayMessage::system(
                            app_mod::commands::refactor_stop_notice(false),
                        ));
                        let _ = begin_remote_send(
                            app,
                            remote,
                            stop_prompt,
                            vec![],
                            true,
                            None,
                            true,
                            0,
                        )
                        .await;
                    }
                }
                Ok(app_mod::commands::RefactorCommand::Run { plan_only, focus }) => {
                    let mode = app_mod::commands::refactor_mode_for(plan_only);
                    persist_remote_session_metadata(app, |session| {
                        session.improve_mode =
                            Some(app_mod::commands::session_improve_mode_for(mode));
                    })?;
                    app.improve_mode = Some(mode);
                    let prompt =
                        app_mod::commands::build_refactor_prompt(plan_only, focus.as_deref());
                    if app.is_processing {
                        remote.cancel_with_reason("slash_refactor_run").await?;
                        app.set_status_notice(if plan_only {
                            "Interrupting for /refactor plan..."
                        } else {
                            "Interrupting for /refactor..."
                        });
                        app.push_display_message(DisplayMessage::system(
                            app_mod::commands::refactor_launch_notice(
                                plan_only,
                                focus.as_deref(),
                                true,
                            ),
                        ));
                        app.queued_messages.push(prompt);
                    } else {
                        app.push_display_message(DisplayMessage::system(
                            app_mod::commands::refactor_launch_notice(
                                plan_only,
                                focus.as_deref(),
                                false,
                            ),
                        ));

                        let _ = begin_remote_send(app, remote, prompt, vec![], true, None, true, 0)
                            .await;
                    }
                }
            }
            return Ok(());
        }

        if trimmed.starts_with('/') {
            if crate::tui::is_ssh_remote() {
                // Known client commands must never fall through as an agent
                // prompt. Unknown skills and absolute paths belong to the VM.
                let token = trimmed.split_whitespace().next();
                if (app_mod::registered_command_names().any(|name| Some(name) == token)
                    || app_mod::commands_dispatch::ssh_presentation_command(trimmed))
                    && app_mod::commands_dispatch::dispatch_local_command(app, trimmed)
                {
                    return Ok(());
                }
                return submit_prepared_remote_input(app, remote, prepared).await;
            }
            submit_remote_slash_input(app, remote, prepared).await?;
            return Ok(());
        }

        if app.route_next_prompt_to_new_session {
            route_prepared_input_to_new_remote_session(app, remote, prepared).await?;
            return Ok(());
        }

        match app.send_action(false) {
            SendAction::Submit => submit_prepared_remote_input(app, remote, prepared).await?,
            SendAction::Queue => {
                app.queued_messages.push(prepared.expanded);
            }
            SendAction::Interleave => {
                app.send_interleave_now(prepared.expanded, prepared.images, remote)
                    .await;
            }
        }
    }
    Ok(())
}
