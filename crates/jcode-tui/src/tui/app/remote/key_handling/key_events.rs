use super::*;

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

pub(super) async fn handle_remote_key_internal(
    app: &mut App,
    code: KeyCode,
    modifiers: KeyModifiers,
    remote: &mut RemoteConnection,
    text_input: Option<String>,
) -> Result<()> {
    app.wake_idle_hibernation();
    let mut code = code;
    let mut modifiers = modifiers;
    ctrl_bracket_fallback_to_esc(&mut code, &mut modifiers);

    if app.handle_key_during_provider_failover(code) {
        return Ok(());
    }

    if app.handle_ssh_login_key(code, modifiers, text_input.as_deref()) {
        return Ok(());
    }
    // A local login picker preview must not capture native SSH /login.
    if code == KeyCode::Enter && crate::tui::is_ssh_remote() {
        let input = app.input.clone();
        if app.handle_ssh_login_command(input.trim()) {
            app.input.clear();
            app.cursor_pos = 0;
            return Ok(());
        }
    }

    // Alt+5 always resets the simulator before modal routing, including in the
    // remote/client mode used by self-dev sessions.
    if app.handle_onboarding_sim_reset_shortcut(code, modifiers) {
        return Ok(());
    }
    if app.handle_update_sim_shortcut(code, modifiers) {
        return Ok(());
    }
    if app.handle_update_rehearsal_shortcut(code, modifiers) {
        return Ok(());
    }

    // Cmd+5 toggles the onboarding rehearsal. Every other key goes to the real
    // onboarding handlers, exactly as on a first run.
    if app.handle_onboarding_sim_toggle_shortcut(code, modifiers) {
        return Ok(());
    }

    if app.handle_onboarding_continue_prompt_key(code) {
        return Ok(());
    }

    if app.prompt_history_search.is_some() {
        app.handle_prompt_history_search_key(code, modifiers);
        return Ok(());
    }

    if input::handle_scroll_overlay_key(app, code)? {
        return Ok(());
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

    // Copy-selection mode owns the keyboard, mirroring the local path in
    // `input::handle_modal_key`. Without this, Enter/Y fell through to the
    // composer in client mode: Enter submitted the draft and letters were
    // typed into it, so nothing was ever copied.
    if app.copy_selection_mode {
        let ctrl_copy_or_quit = modifiers.contains(KeyModifiers::CONTROL)
            && matches!(code, KeyCode::Char('c') | KeyCode::Char('d'));
        if !ctrl_copy_or_quit {
            let _ = app.handle_copy_selection_key(code, modifiers)
                || input::handle_navigation_shortcuts(app, code, modifiers);
            return Ok(());
        }
        if code == KeyCode::Char('c')
            && app
                .current_copy_selection_text()
                .is_some_and(|text| !text.is_empty())
        {
            app.copy_current_selection_to_clipboard();
            return Ok(());
        }
    }

    if let Some(ref picker) = app.inline_interactive_state
        && !picker.preview
    {
        if code == KeyCode::Enter {
            let subagent_model = picker
                .filtered
                .get(picker.selected)
                .and_then(|index| picker.entries.get(*index))
                .and_then(|entry| match entry.action {
                    crate::tui::PickerAction::SubagentModelChoice { inherit: true } => Some(None),
                    crate::tui::PickerAction::SubagentModelChoice { inherit: false } => Some(Some(
                        crate::tui::app::inline_interactive::subagent_picker_model_spec(entry),
                    )),
                    _ => None,
                });
            if let Some(model) = subagent_model {
                remote.set_subagent_model(model).await?;
            }
            if let Some((target, model)) = picker
                .filtered
                .get(picker.selected)
                .and_then(|index| picker.entries.get(*index))
                .and_then(|entry| match entry.action {
                    crate::tui::PickerAction::AgentModelChoice {
                        target,
                        clear_override,
                    } => Some((
                        match target {
                            crate::tui::AgentModelTarget::Swarm => "swarm",
                            crate::tui::AgentModelTarget::Review => "review",
                            crate::tui::AgentModelTarget::Judge => "judge",
                            crate::tui::AgentModelTarget::Memory => "memory",
                            crate::tui::AgentModelTarget::Ambient => "ambient",
                        }
                        .to_string(),
                        (!clear_override).then(|| {
                            crate::tui::app::inline_interactive::subagent_picker_model_spec(entry)
                        }),
                    )),
                    _ => None,
                })
            {
                let global = app.agent_models_global_scope;
                app.pending_agent_model_request_id =
                    Some(remote.set_agent_model(target, model, global).await?);
            }
        }
        return app.handle_inline_interactive_key(code, modifiers);
    }

    if app.handle_inline_interactive_preview_key(&code, modifiers)? {
        return Ok(());
    }

    // While the runtime model picker preview is visible, route its
    // favorite/default hotkeys (Ctrl+O set default, Ctrl+N toggle favorite) to
    // the focused picker handler before the remote global Ctrl-key handling
    // can claim them. This mirrors the local path in `input.rs`
    // `handle_key_core`.
    if app.model_picker_preview_hotkey(code, modifiers)? {
        return Ok(());
    }

    // Inline hotkey feedback: when a known-but-rarely-used chord is pressed,
    // show "you just pressed X → does Y". Placed after the overlay handlers so
    // overlay-local keys stay silent. Unknown chords are reported at the
    // fall-through points below.
    app.observe_known_hotkey(code, modifiers, true);

    if input::handle_visible_copy_shortcut(app, code, modifiers) {
        return Ok(());
    }

    if input::is_next_prompt_new_session_hotkey(code, modifiers) {
        if app_mod::commands_dispatch::ssh_local_action_blocked(app, "New-session routing") {
            return Ok(());
        }
        app.toggle_next_prompt_new_session_routing();
        return Ok(());
    }

    if app.dictation_key_matches(code, modifiers) {
        // The microphone belongs to this terminal. Completed transcripts use
        // the same remote input path as typed prompts.
        app.handle_dictation_trigger();
        return Ok(());
    }

    if app.new_terminal_key_matches(code, modifiers) {
        if crate::tui::is_ssh_remote() {
            app.set_status_notice(
                "New local terminal disabled for SSH sessions; launch jcode --ssh separately",
            );
            return Ok(());
        }
        app.handle_new_terminal_hotkey();
        return Ok(());
    }

    // Accept an armed post-error fallback offer: stage the route switch and
    // resend so the remote dispatcher applies it (SetRoute + payload resend).
    // Checked before the merge offer to match the local key-handling order
    // (both share the same accept key).
    if app.pending_fallback_offer.is_some()
        && !app.is_processing
        && app.fallback_switch_key_matches(code, modifiers)
    {
        app.apply_pending_fallback_offer();
        return Ok(());
    }

    // Accept an armed "merge the diverged update" offer (self-dev/remote
    // sessions surface the same update card as local ones).
    if app.merge_offer_key_matches(code, modifiers) {
        if crate::tui::is_ssh_remote() {
            app.set_status_notice("Update the SSH server on its host");
            return Ok(());
        }
        app.accept_update_merge_offer();
        return Ok(());
    }

    if app.open_resume_key_matches(code, modifiers) {
        if crate::tui::is_ssh_remote() {
            app.set_status_notice(
                "Local session picker disabled for SSH; use --ssh HOST --resume REMOTE_ID",
            );
            return Ok(());
        }
        app.open_session_picker();
        return Ok(());
    }

    if handle_workspace_navigation_key(app, code, modifiers, remote).await? {
        return Ok(());
    }

    if app.toggle_keys.auto_poke.matches(code, modifiers) {
        if crate::tui::is_ssh_remote() {
            let args = if app.auto_poke_incomplete_todos {
                vec!["off".to_string()]
            } else if app.is_processing {
                vec!["on".to_string()]
            } else {
                Vec::new()
            };
            remote.runtime_command("poke", args).await?;
            app.set_status_notice("Requested remote poke toggle");
            return Ok(());
        }
        if app.auto_poke_incomplete_todos {
            let cleared = app_mod::commands::disable_auto_poke(app);
            app.set_status_notice("Poke: OFF");
            app.push_display_message(DisplayMessage::system(
                app_mod::commands::poke_disabled_message(cleared),
            ));
        } else {
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

                    let _ =
                        begin_remote_send(app, remote, poke_msg, vec![], true, None, true, 0).await;
                    app.visible_turn_started = Some(Instant::now());
                }
            }
        }
        return Ok(());
    }

    if app.toggle_keys.copy_selection.matches(code, modifiers) {
        app.toggle_copy_selection_mode();
        return Ok(());
    }

    if app
        .toggle_keys
        .diagram_pane_visibility
        .matches(code, modifiers)
    {
        app.toggle_diagram_pane();
        return Ok(());
    }

    if app.toggle_keys.side_panel.matches(code, modifiers) {
        app.toggle_side_panel();
        return Ok(());
    }

    if app.toggle_keys.info_widget.matches(code, modifiers) {
        crate::tui::info_widget::toggle_enabled();
        let status = if crate::tui::info_widget::is_enabled() {
            "Info widget: ON"
        } else {
            "Info widget: OFF"
        };
        app.set_status_notice(status);
        return Ok(());
    }

    // Swarm views: Alt+N cycles chat → inline controls → full live page → chat.
    // Selection/open/prompt controls stay available in both active views, while
    // plain typing continues to flow to the chat input.
    if app.toggle_keys.swarm_panel_focus.matches(code, modifiers) {
        match app.cycle_swarm_panel_view() {
            app_mod::tui_state::SwarmPanelView::Chat => {
                app.set_status_notice("Swarm view closed");
            }
            app_mod::tui_state::SwarmPanelView::Controls => {
                app.set_status_notice(crate::tui::keybind::swarm_view_hint("full page"));
            }
            app_mod::tui_state::SwarmPanelView::FullPage => {
                app.set_status_notice(crate::tui::keybind::swarm_page_hint());
            }
        }
        return Ok(());
    }
    {
        use crate::tui::TuiState as _;
        if app.handle_swarm_agent_shortcut(code, modifiers)
            || (app.swarm_panel_focused() && app.handle_swarm_panel_key(code, modifiers))
        {
            return Ok(());
        }
    }
    let macos_option_shortcut =
        crate::tui::keybind::shortcut_char_for_macos_option_key(code, modifiers);
    if app.toggle_keys.diagram_pane.matches(code, modifiers) {
        app.toggle_diagram_pane_position();
        return Ok(());
    }
    if let Some(direction) = app.model_switch_keys.direction_for(code, modifiers) {
        app.record_keybinding_fast(crate::tui::app::shortcut_hints::LearnableAction::ModelSwitch);
        remote.cycle_model(direction).await?;
        return Ok(());
    }
    if let Some(direction) = app.effort_switch_keys.direction_for(code, modifiers) {
        app.record_keybinding_fast(crate::tui::app::shortcut_hints::LearnableAction::EffortCycle);
        apply_remote_effort_direction(app, remote, direction).await?;
        return Ok(());
    }
    if let Some(direction) = app.speed_switch_keys.direction_for(code, modifiers) {
        apply_remote_speed_direction(app, remote, direction).await?;
        return Ok(());
    }
    if cfg!(target_os = "macos")
        && !matches!(app.status, ProcessingStatus::RunningTool(_))
        && let Some(direction) = app
            .effort_switch_keys
            .macos_option_arrow_escape_direction_for(code, modifiers)
    {
        apply_remote_effort_direction(app, remote, direction).await?;
        return Ok(());
    }
    if app.toggle_keys.typing_scroll_lock.matches(code, modifiers) {
        app.toggle_typing_scroll_lock();
        return Ok(());
    }
    if app.toggle_keys.todo_card.matches(code, modifiers) {
        app.toggle_todo_card();
        if crate::tui::is_ssh_remote() {
            remote.request_todos().await?;
        }
        return Ok(());
    }
    if app.centered_toggle_keys.matches(code, modifiers) {
        app.record_keybinding_fast(crate::tui::app::shortcut_hints::LearnableAction::Alignment);
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

    if modifiers.contains(KeyModifiers::ALT) || macos_option_shortcut.is_some() {
        let alt_code = macos_option_shortcut.map(KeyCode::Char).unwrap_or(code);
        match alt_code {
            KeyCode::Char('b') => {
                // Cloud/reconnected clients can display a connection phase while
                // the server is still executing a tool. The server is authoritative.
                if app.is_processing || matches!(app.status, ProcessingStatus::RunningTool(_)) {
                    remote.background_tool().await?;
                    app.set_status_notice("Moving tool to background...");
                    return Ok(());
                }
                app.cursor_pos = app.find_word_boundary_back();
                return Ok(());
            }
            // Alt/Option+Left/Right move by word, matching Alt+B / Alt+F.
            KeyCode::Left => {
                app.cursor_pos = app.find_word_boundary_back();
                return Ok(());
            }
            KeyCode::Char('f') | KeyCode::Right => {
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
            KeyCode::Backspace | KeyCode::Delete | KeyCode::Char('\u{7f}') => {
                input::delete_input_word_back(app);
                return Ok(());
            }
            KeyCode::Char('v') => {
                app.paste_from_clipboard();
                return Ok(());
            }
            KeyCode::Char('e') => {
                input::handle_alt_key(app, KeyCode::Char('e'));
                return Ok(());
            }
            _ => {}
        }
    }

    if modifiers.contains(KeyModifiers::SUPER) {
        match code {
            KeyCode::Backspace | KeyCode::Delete | KeyCode::Char('\u{7f}') => {
                input::delete_input_word_back(app);
                return Ok(());
            }
            KeyCode::Left | KeyCode::Home | KeyCode::Char('a') => {
                app.cursor_pos = 0;
                return Ok(());
            }
            KeyCode::Right | KeyCode::End | KeyCode::Char('e') => {
                app.cursor_pos = app.input.len();
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
            KeyCode::Char('v') => {
                app.paste_from_clipboard();
                return Ok(());
            }
            // Cmd+L mirrors Ctrl+L: terminal-style clear (blank spacer
            // pushes the transcript up into scrollback).
            KeyCode::Char('l') => {
                app.clear_view_terminal_style();
                return Ok(());
            }
            _ => {}
        }
    }

    if app.handle_command_suggestion_key(code, modifiers) {
        return Ok(());
    }

    if input::handle_account_shortcut(app, code, modifiers) {
        return Ok(());
    }

    if handle_ctrl_kill_to_end(app, code, modifiers) {
        return Ok(());
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

    if app.centered_toggle_keys.matches(code, modifiers) {
        app.record_keybinding_fast(crate::tui::app::shortcut_hints::LearnableAction::Alignment);
        app.toggle_centered_mode();
        return Ok(());
    }

    if app.scroll_keys.is_bookmark(code, modifiers) {
        app.toggle_scroll_bookmark();
        return Ok(());
    }

    if code == KeyCode::BackTab {
        app.cycle_model_favorite_hotkey();
        return Ok(());
    }

    if app.toggle_keys.commit_log.matches(code, modifiers) {
        app.toggle_commit_log_overlay();
        return Ok(());
    }

    if app.toggle_keys.diff_mode_cycle.matches(code, modifiers) {
        app.diff_mode = app.diff_mode.cycle();
        if !app.diff_pane_visible() {
            app.diff_pane_focus = false;
        }
        let status = format!("Diffs: {}", app.diff_mode.label());
        app.set_status_notice(&status);
        return Ok(());
    }

    if modifiers == KeyModifiers::CONTROL && code == KeyCode::Down {
        input::handle_prompt_history_navigation(app, code, modifiers);
        return Ok(());
    }

    if modifiers.contains(KeyModifiers::CONTROL) {
        if app.handle_diagram_ctrl_key(code, diagram_available) {
            return Ok(());
        }
        match code {
            KeyCode::Char('b') => {
                if app.is_processing || matches!(app.status, ProcessingStatus::RunningTool(_)) {
                    remote.background_tool().await?;
                    app.set_status_notice("Moving tool to background...");
                    return Ok(());
                }
                if app.cursor_pos > 0 {
                    app.cursor_pos = app.find_word_boundary_back();
                }
                return Ok(());
            }
            KeyCode::Char('d') if input::try_ctrl_d_forward_delete(app) => {
                return Ok(());
            }
            KeyCode::Char('c') | KeyCode::Char('d') => {
                if app.is_processing {
                    remote.cancel_with_reason("keyboard_ctrl_c_or_d").await?;
                    app.set_status_notice("Interrupting...");
                } else {
                    app.handle_quit_request();
                }
                return Ok(());
            }
            KeyCode::Char('r') => {
                app.open_prompt_history_search();
                return Ok(());
            }
            KeyCode::Char('l') => {
                // Terminal-style clear: a viewport-height blank spacer pushes
                // the transcript up into scrollback, leaving a clean prompt.
                // Nothing is deleted; /cls does the actual view wipe. The
                // diagram/diff focus handler above wins while a pane is
                // available.
                app.clear_view_terminal_style();
                return Ok(());
            }
            KeyCode::Char('u') => {
                input::delete_input_to_start(app);
                return Ok(());
            }
            KeyCode::Char('k') => {
                input::delete_input_to_end(app);
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
                input::delete_input_word_back(app);
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
                let had_server_interrupts = !app.pending_soft_interrupts.is_empty();
                let had_pending = app.retrieve_pending_message_for_edit();
                if had_pending && had_server_interrupts {
                    if let Err(error) = remote.cancel_soft_interrupts().await {
                        app.push_display_message(DisplayMessage::error(format!(
                            "Recalled draft retained, but server interrupt cancellation failed: {error}"
                        )));
                        app.set_status_notice("Draft recalled locally; server cancellation failed");
                    }
                } else if !had_pending {
                    input::handle_prompt_history_navigation(app, code, modifiers);
                }
                return Ok(());
            }
            KeyCode::Down => {
                input::handle_prompt_history_navigation(app, code, modifiers);
                return Ok(());
            }
            _ => {}
        }
    }

    if input::is_alternate_enter(code, modifiers) && input::try_queue_command(app) {
        return Ok(());
    }

    if input::is_alternate_enter(code, modifiers) && !app.input.trim().starts_with('/') {
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
                    app.send_interleave_now(prepared.expanded, prepared.images, remote)
                        .await;
                }
            }
        }
        return Ok(());
    }

    if crate::tui::app::input::newline::enter_inserts_newline(app, code, modifiers) {
        return Ok(());
    }

    if input::handle_multiline_input_navigation(app, code, modifiers)
        || input::handle_prompt_history_navigation(app, code, modifiers)
    {
        return Ok(());
    }

    if let Some(text) = text_input.or_else(|| input::text_input_for_key(code, modifiers)) {
        input::handle_text_input(app, &text);
        return Ok(());
    }

    // Never fall through and insert literal text for unhandled Ctrl+key chords. This stays after
    // text_input so Ctrl+Alt/AltGr symbols delivered as final printable text still work.
    if modifiers.contains(KeyModifiers::CONTROL) {
        app.note_unrecognized_hotkey(code, modifiers, true);
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

    // A modified chord (or function key) that reached this point is not bound
    // to anything; tell the user instead of silently swallowing it.
    app.note_unrecognized_hotkey(code, modifiers, true);

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
            } else {
                // Opt-in: Left on an empty input opens the active sessions
                // manager (no-op unless display.active_sessions_manager).
                app.maybe_open_active_sessions_on_left();
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
            submit_remote_enter_input(app, remote).await?;
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
            } else if app.is_processing && app.has_pending_user_followup() {
                // The user typed a new prompt while this turn ran, then hit
                // Esc: stop this turn and run the new prompt next. The server
                // leaves an unsent soft interrupt queued on cancel, and the
                // follow-up recovery path sends it as the next turn. Auto-poke
                // stays on: this is a redirect, not "stop everything".
                remote
                    .cancel_with_reason("keyboard_escape_redirect")
                    .await?;
                app.remote_interrupt_ack_deadline =
                    Some(Instant::now() + std::time::Duration::from_secs(3));
                app.set_status_notice("Interrupting... sending your next prompt");
            } else if app.is_processing {
                let disabled_auto_poke = app.auto_poke_incomplete_todos
                    || app
                        .queued_messages
                        .iter()
                        .any(|message| app_mod::commands::is_poke_message(message));
                remote.cancel_with_reason("keyboard_escape").await?;
                if disabled_auto_poke {
                    app_mod::commands::disable_auto_poke(app);
                    app.set_status_notice("Interrupting... Auto-poke OFF");
                } else {
                    app.set_status_notice("Interrupting...");
                }
            } else {
                app.follow_chat_bottom();
                input::clear_input_for_escape(app);
            }
        }
        _ => {}
    }

    Ok(())
}
