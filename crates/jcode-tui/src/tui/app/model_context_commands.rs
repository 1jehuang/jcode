use super::*;

pub(in crate::tui::app) fn handle_model_command(app: &mut App, trimmed: &str) -> bool {
    if is_refresh_model_list_command(trimmed) {
        let session_id = app
            .active_client_session_id()
            .unwrap_or(app.session.id.as_str())
            .to_string();
        crate::bus::Bus::global().publish(crate::bus::BusEvent::UiActivity(
            crate::bus::UiActivity::catalog(
                Some(session_id.clone()),
                crate::message::format_model_refresh_progress_markdown(
                    "Starting provider model catalog refresh",
                    Some(5),
                ),
                Some("Refreshing model list..."),
            ),
        ));
        app.set_status_notice("Refreshing model list...");
        let provider = app.provider.clone();

        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                let result =
                    refresh_model_catalog_with_progress(provider, session_id.clone()).await;
                crate::bus::Bus::global().publish(crate::bus::BusEvent::ModelRefreshCompleted(
                    crate::bus::ModelRefreshCompleted { session_id, result },
                ));
            });
        } else {
            std::thread::spawn(move || {
                let result = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime.block_on(refresh_model_catalog_with_progress(
                        provider,
                        session_id.clone(),
                    )),
                    Err(error) => Err(error.to_string()),
                };
                crate::bus::Bus::global().publish(crate::bus::BusEvent::ModelRefreshCompleted(
                    crate::bus::ModelRefreshCompleted { session_id, result },
                ));
            });
        }
        return true;
    }

    if trimmed == "/model" || trimmed == "/models" {
        app.record_keybinding_slow(crate::tui::app::shortcut_hints::LearnableAction::ModelSwitch);
        app.open_model_picker();
        return true;
    }

    if let Some(model_name) = trimmed.strip_prefix("/model ") {
        app.record_keybinding_slow(crate::tui::app::shortcut_hints::LearnableAction::ModelSwitch);
        let model_name = model_name.trim();
        match app.provider.set_model(model_name) {
            Ok(()) => {
                let active_model = app.finalize_model_switch(model_name);
                let auth_suffix = app
                    .provider
                    .active_auth_method_label()
                    .map_or_else(String::new, |method| format!(" (via {})", method));
                app.push_display_message(DisplayMessage {
                    role: "system".to_string(),
                    content: format!("✓ Switched to model: {}{}", active_model, auth_suffix),
                    tool_calls: vec![],
                    duration_secs: None,
                    title: None,
                    tool_data: None,
                    tool_duration_ms: None,
                });
                app.set_status_notice(format!("Model → {}", model_name));
            }
            Err(e) => {
                app.push_display_message(DisplayMessage::error(model_switch_failure_message(
                    &e.to_string(),
                    app.is_remote,
                )));
                app.set_status_notice("Model switch failed");
            }
        }
        return true;
    }

    if trimmed == "/effort" {
        app.record_keybinding_slow(crate::tui::app::shortcut_hints::LearnableAction::EffortCycle);
        let current = app.provider.reasoning_effort();
        let efforts = app.provider.available_efforts();
        if efforts.is_empty() {
            app.push_display_message(DisplayMessage::system(
                "Reasoning effort not available for this provider.".to_string(),
            ));
        } else {
            let current_label = current
                .as_deref()
                .map(effort_display_label)
                .unwrap_or("default");
            let list: Vec<String> = efforts
                .iter()
                .map(|e| {
                    if Some(e.to_string()) == current {
                        format!("{} <- current", effort_display_label(e))
                    } else {
                        effort_display_label(e).to_string()
                    }
                })
                .collect();
            app.push_display_message(DisplayMessage::system(format!(
                "Effort: {}\nAvailable: {}\nUse /effort <level> or {} to change.",
                current_label,
                list.join(" · "),
                crate::tui::keybind::effort_switch_keys_label()
            )));
        }
        return true;
    }

    if let Some(level) = trimmed.strip_prefix("/effort ") {
        app.record_keybinding_slow(crate::tui::app::shortcut_hints::LearnableAction::EffortCycle);
        let level = level.trim();
        match app.provider.set_reasoning_effort(level) {
            Ok(()) => {
                let new_effort = app.provider.reasoning_effort();
                let label = new_effort
                    .as_deref()
                    .map(effort_display_label)
                    .unwrap_or("default");
                app.push_display_message(DisplayMessage::system(format!(
                    "✓ Reasoning effort → {}",
                    label
                )));
                let efforts = app.provider.available_efforts();
                let idx = new_effort
                    .as_ref()
                    .and_then(|e| efforts.iter().position(|x| *x == e.as_str()))
                    .unwrap_or(0);
                let bar = effort_bar(idx, efforts.len());
                app.set_status_notice(format!("Effort: {} {}", label, bar));
            }
            Err(e) => {
                app.push_display_message(DisplayMessage::error(format!(
                    "Failed to set effort: {}",
                    e
                )));
            }
        }
        return true;
    }

    if matches!(trimmed, "/fast default" | "/fast default status") {
        let default_tier = crate::config::Config::load().provider.openai_service_tier;
        let default_enabled = default_tier.as_deref() == Some("priority");
        let default_label = default_tier
            .as_deref()
            .map(service_tier_display_label)
            .unwrap_or("Standard");
        app.push_display_message(DisplayMessage::system(fast_mode_default_message(
            default_enabled,
            default_label,
        )));
        return true;
    }

    if let Some(mode) = trimmed.strip_prefix("/fast default ") {
        let mode = mode.trim().to_ascii_lowercase();
        match mode.as_str() {
            "on" => super::auth::save_openai_fast_setting_local(app, true),
            "off" => super::auth::save_openai_fast_setting_local(app, false),
            "status" => {
                let default_tier = crate::config::Config::load().provider.openai_service_tier;
                let default_enabled = default_tier.as_deref() == Some("priority");
                let default_label = default_tier
                    .as_deref()
                    .map(service_tier_display_label)
                    .unwrap_or("Standard");
                app.push_display_message(DisplayMessage::system(fast_mode_default_message(
                    default_enabled,
                    default_label,
                )));
            }
            _ => {
                app.push_display_message(DisplayMessage::error(
                    "Usage: /fast default [on|off|status]".to_string(),
                ));
            }
        }
        return true;
    }

    if matches!(trimmed, "/fast" | "/fast status") {
        let current = app.provider.service_tier();
        let status = if service_tier_is_fast(current.as_deref()) {
            "on"
        } else {
            "off"
        };
        let current_label = current
            .as_deref()
            .map(service_tier_display_label)
            .unwrap_or("Standard");
        let default_tier = crate::config::Config::load().provider.openai_service_tier;
        let default_enabled = default_tier.as_deref() == Some("priority");
        let default_label = default_tier
            .as_deref()
            .map(service_tier_display_label)
            .unwrap_or("Standard");
        app.push_display_message(DisplayMessage::system(fast_mode_overview_message(
            status == "on",
            current_label,
            default_enabled,
            default_label,
        )));
        return true;
    }

    if let Some(mode) = trimmed.strip_prefix("/fast ") {
        let mode = mode.trim().to_ascii_lowercase();
        let target = match mode.as_str() {
            "on" => "priority",
            "ultra" | "ultrafast" => "ultrafast",
            "off" => "off",
            "status" => {
                let current = app.provider.service_tier();
                let enabled = service_tier_is_fast(current.as_deref());
                let current_label = current
                    .as_deref()
                    .map(service_tier_display_label)
                    .unwrap_or("Standard");
                let default_tier = crate::config::Config::load().provider.openai_service_tier;
                let default_enabled = default_tier.as_deref() == Some("priority");
                let default_label = default_tier
                    .as_deref()
                    .map(service_tier_display_label)
                    .unwrap_or("Standard");
                app.push_display_message(DisplayMessage::system(fast_mode_overview_message(
                    enabled,
                    current_label,
                    default_enabled,
                    default_label,
                )));
                return true;
            }
            _ => {
                app.push_display_message(DisplayMessage::error(
                    "Usage: /fast [on|ultra|off|status|default ...]".to_string(),
                ));
                return true;
            }
        };

        match app.provider.set_service_tier(target) {
            Ok(()) => {
                let current = app.provider.service_tier();
                let enabled = service_tier_is_fast(current.as_deref());
                let label = current
                    .as_deref()
                    .map(service_tier_display_label)
                    .unwrap_or("Standard");
                let applies_next_request = app.is_processing;
                app.push_display_message(DisplayMessage::system(fast_mode_success_message(
                    enabled,
                    label,
                    applies_next_request,
                )));
                app.set_status_notice(fast_mode_status_notice(
                    current.as_deref(),
                    applies_next_request,
                ));
            }
            Err(e) => {
                app.push_display_message(DisplayMessage::error(format!(
                    "Failed to set fast mode: {}",
                    e
                )));
            }
        }
        return true;
    }

    if trimmed == "/transport" {
        let current = app.provider.transport();
        let transports = app.provider.available_transports();
        if transports.is_empty() {
            app.push_display_message(DisplayMessage::system(
                "Transport switching is not available for this provider.".to_string(),
            ));
        } else {
            let current_label = current.as_deref().unwrap_or("unknown");
            let list: Vec<String> = transports
                .iter()
                .map(|t| {
                    if Some(*t) == current.as_deref() {
                        format!("{} <- current", t)
                    } else {
                        t.to_string()
                    }
                })
                .collect();
            app.push_display_message(DisplayMessage::system(format!(
                "Transport: {}\nAvailable: {}\nUse /transport <mode> to change.",
                current_label,
                list.join(" · ")
            )));
        }
        return true;
    }

    if let Some(mode) = trimmed.strip_prefix("/transport ") {
        let mode = mode.trim();
        match app.provider.set_transport(mode) {
            Ok(()) => {
                let new_transport = app.provider.transport().unwrap_or_else(|| mode.to_string());
                app.push_display_message(DisplayMessage::system(format!(
                    "✓ Transport → {}",
                    new_transport
                )));
                app.set_status_notice(format!("Transport → {}", new_transport));
            }
            Err(e) => {
                app.push_display_message(DisplayMessage::error(format!(
                    "Failed to set transport: {}",
                    e
                )));
            }
        }
        return true;
    }

    false
}

pub(super) async fn refresh_model_catalog_with_progress(
    provider: std::sync::Arc<dyn crate::provider::Provider>,
    session_id: String,
) -> Result<crate::provider::ModelCatalogRefreshSummary, String> {
    let started = std::time::Instant::now();
    let refresh = provider.refresh_model_catalog();
    tokio::pin!(refresh);
    let mut heartbeat = tokio::time::interval(std::time::Duration::from_secs(2));
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            result = &mut refresh => {
                return result.map_err(|error| error.to_string());
            }
            _ = heartbeat.tick() => {
                let elapsed_secs = started.elapsed().as_secs();
                if elapsed_secs > 0 {
                    let percent = (10 + elapsed_secs.saturating_mul(5)).min(95) as u8;
                    crate::bus::Bus::global().publish(crate::bus::BusEvent::UiActivity(
                        crate::bus::UiActivity::catalog(
                            Some(session_id.clone()),
                            crate::message::format_model_refresh_progress_markdown(
                                &format!("Waiting on provider APIs ({elapsed_secs}s elapsed)"),
                                Some(percent),
                            ),
                            Some("Refreshing model list..."),
                        ),
                    ));
                }
            }
        }
    }
}
