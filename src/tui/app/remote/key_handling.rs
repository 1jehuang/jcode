use super::*;
use crate::tui::app as app_mod;
use crate::tui::core;

pub(in crate::tui::app) fn handle_remote_char_input(app: &mut App, c: char) {
    input::handle_text_input(app, &c.to_string());
    app.follow_chat_bottom_for_typing();
}

pub(in crate::tui::app) async fn send_interleave_now(
    app: &mut App,
    content: String,
    remote: &mut RemoteConnection,
) {
    if content.trim().is_empty() {
        return;
    }
    let msg_clone = content.clone();
    match remote.soft_interrupt(content, false).await {
        Err(e) => {
            app.push_display_message(DisplayMessage::error(format!(
                "Failed to send interleave: {}",
                e
            )));
        }
        Ok(request_id) => {
            app.track_pending_soft_interrupt(request_id, msg_clone);
            app.set_status_notice("⏭ Interleave sent");
        }
    }
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
                if let Some(input) = Self::account_command_for_picker(&other) {
                    crate::tui::app::auth::handle_account_command_remote(self, &input, remote)
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

pub(in crate::tui::app) async fn handle_remote_key_event(
    app: &mut App,
    event: KeyEvent,
    remote: &mut RemoteConnection,
) -> Result<()> {
    handle_remote_key_internal(
        app,
        event.code,
        event.modifiers,
        remote,
        input::text_input_for_key_event(&event),
    )
    .await
}

async fn handle_remote_key_internal(
    app: &mut App,
    code: KeyCode,
    modifiers: KeyModifiers,
    remote: &mut RemoteConnection,
    text_input: Option<String>,
) -> Result<()> {
    let mut code = code;
    let mut modifiers = modifiers;
    ctrl_bracket_fallback_to_esc(&mut code, &mut modifiers);

    if app.changelog_scroll.is_some() {
        return app.handle_changelog_key(code);
    }

    if app.help_scroll.is_some() {
        return app.handle_help_key(code);
    }

    if app.session_picker_overlay.is_some() {
        return app.handle_session_picker_key(code, modifiers);
    }

    if app.login_picker_overlay.is_some() {
        return app.handle_login_picker_key(code, modifiers);
    }

    if app.account_picker_overlay.is_some() {
        if let Some(command) = app.next_account_picker_action(code, modifiers)? {
            app.handle_account_picker_command_remote(remote, command)
                .await?;
        }
        return Ok(());
    }

    if app.usage_overlay.is_some() {
        app.handle_usage_overlay_key(code, modifiers)?;
        return Ok(());
    }

    if let Some(ref picker) = app.inline_interactive_state
        && !picker.preview
    {
        return app.handle_inline_interactive_key(code, modifiers);
    }

    if app.handle_model_preview_key_as_active(code, modifiers)? {
        return Ok(());
    }

    if app.handle_inline_interactive_preview_key(&code, modifiers)? {
        return Ok(());
    }

    if input::handle_visible_copy_shortcut(app, code, modifiers) {
        return Ok(());
    }

    if input::is_next_prompt_new_session_hotkey(code, modifiers) {
        app.toggle_next_prompt_new_session_routing();
        return Ok(());
    }

    if app.dictation_key_matches(code, modifiers) {
        app.handle_dictation_trigger();
        return Ok(());
    }

    if handle_workspace_navigation_key(app, code, modifiers, remote).await? {
        return Ok(());
    }

    if modifiers.contains(KeyModifiers::ALT) && matches!(code, KeyCode::Char('m')) {
        app.toggle_side_panel();
        return Ok(());
    }
    if modifiers.contains(KeyModifiers::ALT) && matches!(code, KeyCode::Char('t')) {
        app.toggle_diagram_pane_position();
        return Ok(());
    }
    if let Some(direction) = app.model_switch_keys.direction_for(code, modifiers) {
        remote.cycle_model(direction).await?;
        return Ok(());
    }
    if let Some(direction) = app.effort_switch_keys.direction_for(code, modifiers) {
        let efforts = ["none", "low", "medium", "high", "xhigh"];
        let current = app.remote_reasoning_effort.as_deref();
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
            remote.set_reasoning_effort(next_effort).await?;
        }
        return Ok(());
    }
    if modifiers.contains(KeyModifiers::ALT) && matches!(code, KeyCode::Char('s')) {
        app.toggle_typing_scroll_lock();
        return Ok(());
    }
    if app.centered_toggle_keys.toggle.matches(code, modifiers) {
        app.toggle_centered_mode();
        return Ok(());
    }
    app.normalize_diagram_state();
    let diagram_available = app.diagram_available();
    if app.handle_diagram_focus_key(code, modifiers, diagram_available) {
        return Ok(());
    }
    if app.handle_diff_pane_focus_key(code, modifiers) {
        return Ok(());
    }

    if modifiers.contains(KeyModifiers::ALT) {
        match code {
            KeyCode::Char('b') => {
                if matches!(app.status, ProcessingStatus::RunningTool(_)) {
                    remote.background_tool().await?;
                    app.set_status_notice("Moving tool to background...");
                    return Ok(());
                }
                app.cursor_pos = app.find_word_boundary_back();
                return Ok(());
            }
            KeyCode::Char('f') => {
                app.cursor_pos = app.find_word_boundary_forward();
                return Ok(());
            }
            KeyCode::Char('d') => {
                let end = app.find_word_boundary_forward();
                if app.cursor_pos < end {
                    app.remember_input_undo_state();
                }
                app.input.drain(app.cursor_pos..end);
                return Ok(());
            }
            KeyCode::Backspace => {
                let start = app.find_word_boundary_back();
                if start < app.cursor_pos {
                    app.remember_input_undo_state();
                }
                app.input.drain(start..app.cursor_pos);
                app.cursor_pos = start;
                return Ok(());
            }
            KeyCode::Char('v') => {
                app.paste_from_clipboard();
                return Ok(());
            }
            _ => {}
        }
    }

    if let Some(amount) = app.scroll_keys.scroll_amount(code, modifiers) {
        if amount < 0 {
            app.scroll_up((-amount) as usize);
        } else {
            app.scroll_down(amount as usize);
        }
        return Ok(());
    }

    if let Some(dir) = app.scroll_keys.prompt_jump(code, modifiers) {
        if dir < 0 {
            app.scroll_to_prev_prompt();
        } else {
            app.scroll_to_next_prompt();
        }
        return Ok(());
    }

    if let Some(ratio) = App::ctrl_side_panel_ratio_preset(&code, modifiers) {
        app.set_side_panel_ratio_preset(ratio);
        return Ok(());
    }

    if let Some(rank) = App::ctrl_prompt_rank(&code, modifiers) {
        app.scroll_to_recent_prompt_rank(rank);
        return Ok(());
    }

    if app.centered_toggle_keys.toggle.matches(code, modifiers) {
        app.toggle_centered_mode();
        return Ok(());
    }

    if modifiers.contains(KeyModifiers::ALT) && matches!(code, KeyCode::Char('s')) {
        app.toggle_typing_scroll_lock();
        return Ok(());
    }

    if app.scroll_keys.is_bookmark(code, modifiers) {
        app.toggle_scroll_bookmark();
        return Ok(());
    }

    if code == KeyCode::BackTab {
        app.diff_mode = app.diff_mode.cycle();
        if !app.diff_pane_visible() {
            app.diff_pane_focus = false;
        }
        let status = format!("Diffs: {}", app.diff_mode.label());
        app.set_status_notice(&status);
        return Ok(());
    }

    if modifiers.contains(KeyModifiers::CONTROL) {
        if app.handle_diagram_ctrl_key(code, diagram_available) {
            return Ok(());
        }
        match code {
            KeyCode::Char('b') => {
                if matches!(app.status, ProcessingStatus::RunningTool(_)) {
                    remote.background_tool().await?;
                    app.set_status_notice("Moving tool to background...");
                    return Ok(());
                }
                if app.cursor_pos > 0 {
                    app.cursor_pos = app.find_word_boundary_back();
                }
                return Ok(());
            }
            KeyCode::Char('c') | KeyCode::Char('d') => {
                if app.is_processing {
                    remote.cancel().await?;
                    app.set_status_notice("Interrupting...");
                } else {
                    app.handle_quit_request();
                }
                return Ok(());
            }
            KeyCode::Char('r') => {
                app.recover_session_without_tools();
                return Ok(());
            }
            KeyCode::Char('l') => {
                return Ok(());
            }
            KeyCode::Char('u') => {
                if app.cursor_pos > 0 {
                    app.remember_input_undo_state();
                }
                app.input.drain(..app.cursor_pos);
                app.cursor_pos = 0;
                return Ok(());
            }
            KeyCode::Char('k') => {
                if app.cursor_pos < app.input.len() {
                    app.remember_input_undo_state();
                }
                app.input.truncate(app.cursor_pos);
                return Ok(());
            }
            KeyCode::Char('z') => {
                app.undo_input_change();
                return Ok(());
            }
            KeyCode::Char('x') => {
                input::cut_input_line_to_clipboard(app);
                return Ok(());
            }
            KeyCode::Char('a') => {
                app.cursor_pos = 0;
                return Ok(());
            }
            KeyCode::Char('e') => {
                app.cursor_pos = app.input.len();
                return Ok(());
            }
            KeyCode::Char('f') => {
                if app.cursor_pos < app.input.len() {
                    app.cursor_pos = app.find_word_boundary_forward();
                }
                return Ok(());
            }
            KeyCode::Left => {
                if app.cursor_pos > 0 {
                    app.cursor_pos = app.find_word_boundary_back();
                }
                return Ok(());
            }
            KeyCode::Right => {
                if app.cursor_pos < app.input.len() {
                    app.cursor_pos = app.find_word_boundary_forward();
                }
                return Ok(());
            }
            KeyCode::Char('w') | KeyCode::Char('\u{8}') | KeyCode::Backspace => {
                let start = app.find_word_boundary_back();
                if start < app.cursor_pos {
                    app.remember_input_undo_state();
                }
                app.input.drain(start..app.cursor_pos);
                app.cursor_pos = start;
                app.sync_model_picker_preview_from_input();
                return Ok(());
            }
            KeyCode::Char('s') => {
                app.toggle_input_stash();
                return Ok(());
            }
            KeyCode::Char('v') => {
                app.paste_from_clipboard();
                return Ok(());
            }
            KeyCode::Tab | KeyCode::Char('t') => {
                app.queue_mode = !app.queue_mode;
                let mode_str = if app.queue_mode {
                    "Queue mode: messages wait until response completes"
                } else {
                    "Immediate mode: messages send next (no interrupt)"
                };
                app.set_status_notice(mode_str);
                return Ok(());
            }
            KeyCode::Up => {
                let had_pending = app.retrieve_pending_message_for_edit();
                if had_pending {
                    let _ = remote.cancel_soft_interrupts().await;
                }
                return Ok(());
            }
            _ => {}
        }
    }

    if code == KeyCode::Enter
        && modifiers.contains(KeyModifiers::CONTROL)
        && !app.input.trim().starts_with('/')
    {
        if app.activate_picker_from_preview() {
            return Ok(());
        }

        if !app.input.is_empty() {
            let prepared = input::take_prepared_input(app);

            if app.route_next_prompt_to_new_session {
                route_prepared_input_to_new_remote_session(app, remote, prepared).await?;
                return Ok(());
            }

            match app.send_action(true) {
                SendAction::Submit => submit_prepared_remote_input(app, remote, prepared).await?,
                SendAction::Queue => {
                    app.queued_messages.push(prepared.expanded);
                }
                SendAction::Interleave => {
                    app.send_interleave_now(prepared.expanded, remote).await;
                }
            }
        }
        return Ok(());
    }

    if code == KeyCode::Enter && modifiers.contains(KeyModifiers::SHIFT) {
        input::insert_input_text(app, "\n");
        app.follow_chat_bottom_for_typing();
        return Ok(());
    }

    // Never fall through and insert literal text for unhandled Ctrl+key chords.
    if modifiers.contains(KeyModifiers::CONTROL) {
        return Ok(());
    }

    if let Some(text) = text_input.or_else(|| input::text_input_for_key(code, modifiers)) {
        input::handle_text_input(app, &text);
        app.follow_chat_bottom_for_typing();
        return Ok(());
    }

    if app
        .inline_interactive_state
        .as_ref()
        .map(|p| p.preview)
        .unwrap_or(false)
    {
        match code {
            KeyCode::Up | KeyCode::Down | KeyCode::PageUp | KeyCode::PageDown => {
                return app.handle_inline_interactive_key(code, modifiers);
            }
            _ => {}
        }
    }

    match code {
        KeyCode::Char(c) => {
            handle_remote_char_input(app, c);
        }
        KeyCode::Backspace => {
            if app.cursor_pos > 0 {
                let prev = core::prev_char_boundary(&app.input, app.cursor_pos);
                app.remember_input_undo_state();
                app.input.drain(prev..app.cursor_pos);
                app.cursor_pos = prev;
                app.reset_tab_completion();
                app.sync_model_picker_preview_from_input();
            }
        }
        KeyCode::Delete => {
            if app.cursor_pos < app.input.len() {
                let next = core::next_char_boundary(&app.input, app.cursor_pos);
                app.remember_input_undo_state();
                app.input.drain(app.cursor_pos..next);
                app.reset_tab_completion();
                app.sync_model_picker_preview_from_input();
            }
        }
        KeyCode::Left => {
            if app.cursor_pos > 0 {
                app.cursor_pos = core::prev_char_boundary(&app.input, app.cursor_pos);
            }
        }
        KeyCode::Right => {
            if app.cursor_pos < app.input.len() {
                app.cursor_pos = core::next_char_boundary(&app.input, app.cursor_pos);
            }
        }
        KeyCode::Home => {
            app.cursor_pos = 0;
        }
        KeyCode::End => {
            app.cursor_pos = app.input.len();
        }
        KeyCode::Tab => {
            app.autocomplete();
        }
        KeyCode::Enter => {
            if app.activate_picker_from_preview() {
                return Ok(());
            }
            if !app.input.is_empty() {
                let prepared = input::take_prepared_input(app);
                let trimmed = prepared.expanded.trim();

                if let Some(topic) = trimmed
                    .strip_prefix("/help ")
                    .or_else(|| trimmed.strip_prefix("/? "))
                {
                    if let Some(help) = app.command_help(topic) {
                        app.push_display_message(DisplayMessage::system(help));
                    } else {
                        app.push_display_message(DisplayMessage::error(format!(
                            "Unknown command '{}'. Use `/help` to list commands.",
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

                if trimmed == "/reload" {
                    let client_needs_reload = app.has_newer_binary();
                    let server_needs_reload =
                        app.remote_server_has_update.unwrap_or(client_needs_reload);

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
                        let session_id = app
                            .remote_session_id
                            .clone()
                            .unwrap_or_else(|| crate::id::new_id("ses"));
                        app.save_input_for_reload(&session_id);
                        app.reload_requested = Some(session_id);
                        app.should_quit = true;
                    }
                    return Ok(());
                }

                if trimmed == "/client-reload" {
                    app.push_display_message(DisplayMessage::system(
                        "Reloading client...".to_string(),
                    ));
                    let session_id = app
                        .remote_session_id
                        .clone()
                        .unwrap_or_else(|| crate::id::new_id("ses"));
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

                if trimmed == "/rebuild" {
                    let session_id = app
                        .remote_session_id
                        .clone()
                        .unwrap_or_else(|| crate::id::new_id("ses"));
                    app.start_background_client_rebuild(session_id);
                    return Ok(());
                }

                if trimmed == "/update" {
                    let session_id = app
                        .remote_session_id
                        .clone()
                        .unwrap_or_else(|| crate::id::new_id("ses"));
                    app.start_background_client_update(session_id);
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

                if trimmed == "/model" || trimmed == "/models" {
                    let _ = remote.refresh_models().await;
                    app.set_status_notice("Refreshing model catalog...");
                    app.open_model_picker();
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
                            Some(model) => format!("fixed `{}`", model),
                            None => format!("inherit current (`{}`)", current_model),
                        };
                        app.push_display_message(DisplayMessage::system(format!(
                            "Subagent model for this session: {}\n\nUse `/subagent-model <name>` to pin a model, or `/subagent-model inherit` to use the current model.",
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
                            "Subagent model reset to inherit the current model (`{}`).",
                            current_model
                        )));
                        app.set_status_notice("Subagent model: inherit");
                        return Ok(());
                    }
                    remote.set_subagent_model(Some(rest.to_string())).await?;
                    app.session.subagent_model = Some(rest.to_string());
                    app.push_display_message(DisplayMessage::system(format!(
                        "Subagent model pinned to `{}` for this session.",
                        rest
                    )));
                    app.set_status_notice(format!("Subagent model → {}", rest));
                    return Ok(());
                }

                if trimmed.starts_with("/subagent") {
                    let rest = trimmed.strip_prefix("/subagent").unwrap_or_default().trim();
                    if rest.is_empty() {
                        app.push_display_message(DisplayMessage::error(
                            "Usage: `/subagent [--type <kind>] [--model <name>] [--continue <session_id>] <prompt>`",
                        ));
                        return Ok(());
                    }
                    match app_mod::commands::parse_manual_subagent_spec(rest) {
                        Ok(spec) => {
                            remote
                                .run_subagent(
                                    spec.prompt,
                                    spec.subagent_type,
                                    spec.model,
                                    spec.session_id,
                                )
                                .await?;
                            app.subagent_status = Some("starting subagent".to_string());
                            app.set_status_notice("Running subagent");
                        }
                        Err(error) => {
                            app.push_display_message(DisplayMessage::error(format!(
                                "{}\nUsage: `/subagent [--type <kind>] [--model <name>] [--continue <session_id>] <prompt>`",
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
                    return Ok(());
                }

                if trimmed == "/effort" {
                    let current = app.remote_reasoning_effort.as_deref();
                    let label = current
                        .map(app_mod::effort_display_label)
                        .unwrap_or("default");
                    let efforts = ["none", "low", "medium", "high", "xhigh"];
                    let list: Vec<String> = efforts
                        .iter()
                        .map(|e| {
                            if Some(*e) == current {
                                format!("**{}** ← current", app_mod::effort_display_label(e))
                            } else {
                                app_mod::effort_display_label(e).to_string()
                            }
                        })
                        .collect();
                    app.push_display_message(DisplayMessage::system(format!(
                        "Reasoning effort: {}\nAvailable: {}\nUse `/effort <level>` or Alt+←/→ to change.",
                        label,
                        list.join(" · ")
                    )));
                    return Ok(());
                }

                if let Some(level) = trimmed.strip_prefix("/effort ") {
                    let level = level.trim();
                    if level.is_empty() {
                        app.push_display_message(DisplayMessage::error("Usage: /effort <level>"));
                        return Ok(());
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
                    app.push_display_message(DisplayMessage::system(
                        app_mod::fast_mode_default_message(default_enabled, default_label),
                    ));
                    return Ok(());
                }

                if let Some(mode) = trimmed.strip_prefix("/fast default ") {
                    let mode = mode.trim().to_ascii_lowercase();
                    match mode.as_str() {
                        "on" => {
                            app_mod::auth::save_openai_fast_setting_local(app, true);
                            remote.set_service_tier("priority").await?;
                        }
                        "off" => {
                            app_mod::auth::save_openai_fast_setting_local(app, false);
                            remote.set_service_tier("off").await?;
                        }
                        "status" => {
                            let default_tier =
                                crate::config::Config::load().provider.openai_service_tier;
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
                    let enabled = current == Some("priority");
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

                if let Some(mode) = trimmed.strip_prefix("/fast ") {
                    let mode = mode.trim().to_ascii_lowercase();
                    let service_tier = match mode.as_str() {
                        "on" => "priority",
                        "off" => "off",
                        "status" => {
                            let current = app.remote_service_tier.as_deref();
                            let enabled = current == Some("priority");
                            let current_label = current
                                .map(app_mod::service_tier_display_label)
                                .unwrap_or("Standard");
                            let default_tier =
                                crate::config::Config::load().provider.openai_service_tier;
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
                                "Usage: /fast [on|off|status|default ...]",
                            ));
                            return Ok(());
                        }
                    };
                    remote.set_service_tier(service_tier).await?;
                    return Ok(());
                }

                if trimmed == "/transport" {
                    let current = app.remote_transport.as_deref().unwrap_or("unknown");
                    let transports = ["auto", "https", "websocket"];
                    let list: Vec<String> = transports
                        .iter()
                        .map(|t| {
                            if Some(*t) == app.remote_transport.as_deref() {
                                format!("**{}** ← current", t)
                            } else {
                                t.to_string()
                            }
                        })
                        .collect();
                    app.push_display_message(DisplayMessage::system(format!(
                        "Transport: {}\nAvailable: {}\nUse `/transport <mode>` to change.",
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

                if crate::tui::app::auth::handle_account_command_remote(app, trimmed, remote)
                    .await?
                {
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
                    let parent_session_id =
                        app_mod::commands::current_feedback_target_session_id(app);
                    app_mod::commands::queue_review_spawn_remote(
                        app,
                        "Autoreview",
                        parent_session_id.clone(),
                        app_mod::commands::build_autoreview_startup_message(&parent_session_id),
                        crate::config::config().autoreview.model.clone(),
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
                    let parent_session_id =
                        app_mod::commands::current_feedback_target_session_id(app);
                    app_mod::commands::queue_review_spawn_remote(
                        app,
                        "Autojudge",
                        parent_session_id.clone(),
                        app_mod::commands::build_autojudge_startup_message(&parent_session_id),
                        crate::config::config().autojudge.model.clone(),
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
                        app_mod::commands::preferred_one_shot_review_override()
                            .map(|(model, provider_key)| (Some(model), Some(provider_key)))
                            .unwrap_or_else(|| {
                                (crate::config::config().autoreview.model.clone(), None)
                            });
                    let parent_session_id =
                        app_mod::commands::current_feedback_target_session_id(app);
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
                        app_mod::commands::preferred_one_shot_review_override()
                            .map(|(model, provider_key)| (Some(model), Some(provider_key)))
                            .unwrap_or_else(|| {
                                (crate::config::config().autojudge.model.clone(), None)
                            });
                    let parent_session_id =
                        app_mod::commands::current_feedback_target_session_id(app);
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
                        "Memory feature: **{}** (config default: {})",
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

                if trimmed == "/observe"
                    || trimmed == "/observe on"
                    || trimmed == "/observe off"
                    || trimmed == "/observe status"
                    || trimmed == "/todos"
                    || trimmed == "/todos on"
                    || trimmed == "/todos off"
                    || trimmed == "/todos status"
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

                if app_mod::commands::handle_goals_command(app, trimmed) {
                    return Ok(());
                }

                if trimmed == "/swarm" || trimmed == "/swarm status" {
                    let default_enabled = crate::config::config().features.swarm;
                    app.push_display_message(DisplayMessage::system(format!(
                        "Swarm feature: **{}** (config default: {})",
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

                if trimmed == "/resume" || trimmed == "/sessions" {
                    app.open_session_picker();
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
                            "📌 Session **{}** saved as \"**{}**\". It will appear at the top of `/resume`.",
                            name, lbl,
                        )
                    } else {
                        format!(
                            "📌 Session **{}** saved. It will appear at the top of `/resume`.",
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
                    let name = app.session.display_name().to_string();
                    app.push_display_message(DisplayMessage::system(format!(
                        "Removed bookmark from session **{}**.",
                        name,
                    )));
                    app.set_status_notice("Bookmark removed");
                    return Ok(());
                }

                if trimmed == "/split" {
                    app.push_display_message(DisplayMessage::system(
                        "Splitting session...".to_string(),
                    ));
                    remote.split().await?;
                    return Ok(());
                }

                if handle_workspace_command(app, remote, trimmed).await? {
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
                        "Compaction mode: **{}**\nAvailable: reactive · proactive · semantic\nUse `/compact mode <mode>` to change it for this session.",
                        mode.as_str()
                    )));
                    return Ok(());
                }

                if let Some(mode_str) = trimmed.strip_prefix("/compact mode ") {
                    let mode_str = mode_str.trim();
                    let Some(mode) = crate::config::CompactionMode::parse(mode_str) else {
                        app.push_display_message(DisplayMessage::error(
                            "Usage: `/compact mode <reactive|proactive|semantic>`".to_string(),
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
                            "Premium mode: **{}**. Toggle off with `/z`. (saved to config)",
                            label,
                        )));
                    }
                    return Ok(());
                }

                if trimmed == "/poke" {
                    let incomplete = app_mod::commands::incomplete_poke_todos(app);

                    if incomplete.is_empty() {
                        app.auto_poke_incomplete_todos = false;
                        app.push_display_message(DisplayMessage::system(
                            "No incomplete todos found. Nothing to poke about.".to_string(),
                        ));
                        return Ok(());
                    }

                    app.auto_poke_incomplete_todos = true;

                    if app.is_processing {
                        app.set_status_notice("Poke queued after current turn");
                        app.push_display_message(DisplayMessage::system(
                            "👉 Queued /poke for after the current turn. Incomplete todos will be re-checked then."
                                .to_string(),
                        ));
                    } else {
                        let poke_msg = app_mod::commands::build_poke_message(&incomplete);
                        app.push_display_message(DisplayMessage::system(format!(
                            "👉 Poking model with {} incomplete todo{}...",
                            incomplete.len(),
                            if incomplete.len() == 1 { "" } else { "s" },
                        )));

                        let _ =
                            begin_remote_send(app, remote, poke_msg, vec![], true, None, true, 0)
                                .await;
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
                                .filter(|todo| {
                                    todo.status != "completed" && todo.status != "cancelled"
                                })
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
                                    "No saved improve run found for this session. Use `/improve` or `/improve plan` to start one."
                                        .to_string(),
                                ));
                                return Ok(());
                            };

                            persist_remote_session_metadata(app, |session| {
                                session.improve_mode =
                                    Some(app_mod::commands::session_improve_mode_for(mode));
                            })?;
                            app.improve_mode = Some(mode);
                            let prompt =
                                app_mod::commands::build_improve_resume_prompt(mode, &incomplete);

                            if app.is_processing {
                                remote.cancel().await?;
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
                                let _ = begin_remote_send(
                                    app,
                                    remote,
                                    prompt,
                                    vec![],
                                    true,
                                    None,
                                    true,
                                    0,
                                )
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
                            let has_incomplete = todos.iter().any(|todo| {
                                todo.status != "completed" && todo.status != "cancelled"
                            });

                            let active_improve_mode = app
                                .improve_mode
                                .or_else(|| {
                                    app.session
                                        .improve_mode
                                        .map(app_mod::commands::restore_improve_mode)
                                })
                                .filter(|mode| mode.is_improve());

                            if active_improve_mode.is_none()
                                && !app.is_processing
                                && !has_incomplete
                            {
                                app.push_display_message(DisplayMessage::system(
                                    "No active improve loop to stop. Use `/improve` to start one."
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
                                remote.cancel().await?;
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
                            let prompt = app_mod::commands::build_improve_prompt(
                                plan_only,
                                focus.as_deref(),
                            );
                            if app.is_processing {
                                remote.cancel().await?;
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

                                let _ = begin_remote_send(
                                    app,
                                    remote,
                                    prompt,
                                    vec![],
                                    true,
                                    None,
                                    true,
                                    0,
                                )
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
                                .filter(|todo| {
                                    todo.status != "completed" && todo.status != "cancelled"
                                })
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
                                    "No saved refactor run found for this session. Use `/refactor` or `/refactor plan` to start one."
                                        .to_string(),
                                ));
                                return Ok(());
                            };

                            persist_remote_session_metadata(app, |session| {
                                session.improve_mode =
                                    Some(app_mod::commands::session_improve_mode_for(mode));
                            })?;
                            app.improve_mode = Some(mode);
                            let prompt =
                                app_mod::commands::build_refactor_resume_prompt(mode, &incomplete);

                            if app.is_processing {
                                remote.cancel().await?;
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
                                let _ = begin_remote_send(
                                    app,
                                    remote,
                                    prompt,
                                    vec![],
                                    true,
                                    None,
                                    true,
                                    0,
                                )
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
                            let has_incomplete = todos.iter().any(|todo| {
                                todo.status != "completed" && todo.status != "cancelled"
                            });

                            let active_refactor_mode = app
                                .improve_mode
                                .or_else(|| {
                                    app.session
                                        .improve_mode
                                        .map(app_mod::commands::restore_improve_mode)
                                })
                                .filter(|mode| mode.is_refactor());

                            if active_refactor_mode.is_none()
                                && !app.is_processing
                                && !has_incomplete
                            {
                                app.push_display_message(DisplayMessage::system(
                                    "No active refactor loop to stop. Use `/refactor` to start one."
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
                                remote.cancel().await?;
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
                            let prompt = app_mod::commands::build_refactor_prompt(
                                plan_only,
                                focus.as_deref(),
                            );
                            if app.is_processing {
                                remote.cancel().await?;
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

                                let _ = begin_remote_send(
                                    app,
                                    remote,
                                    prompt,
                                    vec![],
                                    true,
                                    None,
                                    true,
                                    0,
                                )
                                .await;
                            }
                        }
                    }
                    return Ok(());
                }

                if trimmed.starts_with('/') {
                    app.input = trimmed.to_string();
                    app.cursor_pos = app.input.len();
                    app.submit_input();
                    return Ok(());
                }

                if app.route_next_prompt_to_new_session {
                    route_prepared_input_to_new_remote_session(app, remote, prepared).await?;
                    return Ok(());
                }

                match app.send_action(false) {
                    SendAction::Submit => {
                        submit_prepared_remote_input(app, remote, prepared).await?
                    }
                    SendAction::Queue => {
                        app.queued_messages.push(prepared.expanded);
                    }
                    SendAction::Interleave => {
                        app.send_interleave_now(prepared.expanded, remote).await;
                    }
                }
            }
        }
        KeyCode::Up | KeyCode::PageUp => {
            let inc = if code == KeyCode::PageUp { 10 } else { 1 };
            app.scroll_up(inc);
        }
        KeyCode::Down | KeyCode::PageDown => {
            let dec = if code == KeyCode::PageDown { 10 } else { 1 };
            app.scroll_down(dec);
        }
        KeyCode::Esc => {
            if app
                .inline_interactive_state
                .as_ref()
                .map(|p| p.preview)
                .unwrap_or(false)
            {
                app.inline_interactive_state = None;
                input::clear_input_for_escape(app);
            } else if app.is_processing {
                remote.cancel().await?;
                app.set_status_notice("Interrupting...");
            } else {
                app.follow_chat_bottom();
                input::clear_input_for_escape(app);
            }
        }
        _ => {}
    }

    Ok(())
}
