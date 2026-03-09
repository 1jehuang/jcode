use super::{commands, App, SendAction};
use anyhow::Result;
use crossterm::event::{KeyCode, KeyModifiers};

pub(super) struct PreparedInput {
    pub raw_input: String,
    pub expanded: String,
    pub images: Vec<(String, String)>,
}

pub(super) fn paste_image_from_clipboard(app: &mut App) {
    if let Some((media_type, base64_data)) = super::clipboard_image() {
        attach_image(app, media_type, base64_data);
        return;
    }

    if let Ok(mut clipboard) = arboard::Clipboard::new() {
        if let Ok(text) = clipboard.get_text() {
            if let Some(url) = super::extract_image_url(&text) {
                app.set_status_notice("Downloading image...");
                if let Some((media_type, base64_data)) = super::download_image_url(&url) {
                    attach_image(app, media_type, base64_data);
                } else {
                    app.set_status_notice("Failed to download image");
                }
            } else {
                handle_paste(app, text);
            }
            return;
        }
    }

    app.set_status_notice("No image in clipboard");
}

pub(super) fn handle_paste(app: &mut App, text: String) {
    // Note: clipboard_image() is NOT checked here. Bracketed paste events from the
    // terminal always deliver text. Checking clipboard_image() here caused a bug where
    // text pastes were misidentified as images when the clipboard also had image data
    // (common on Wayland where apps advertise multiple MIME types). Image pasting is
    // handled by paste_image_from_clipboard() (Ctrl+V / Alt+V) instead.
    if let Some(url) = super::extract_image_url(&text) {
        crate::logging::info(&format!("Downloading image from pasted URL: {}", url));
        app.set_status_notice("Downloading image...");
        if let Some((media_type, base64_data)) = super::download_image_url(&url) {
            attach_image(app, media_type, base64_data);
            return;
        }
        app.set_status_notice("Failed to download image");
    }

    crate::logging::info(&format!(
        "Text paste: {} chars, {} lines",
        text.len(),
        text.lines().count()
    ));

    let line_count = text.lines().count().max(1);
    if line_count < 5 {
        app.input.insert_str(app.cursor_pos, &text);
        app.cursor_pos += text.len();
    } else {
        app.pasted_contents.push(text);
        let placeholder = format!(
            "[pasted {} line{}]",
            line_count,
            if line_count == 1 { "" } else { "s" }
        );
        app.input.insert_str(app.cursor_pos, &placeholder);
        app.cursor_pos += placeholder.len();
    }
    app.sync_model_picker_preview_from_input();
}

pub(super) fn expand_paste_placeholders(app: &mut App, input: &str) -> String {
    let mut result = input.to_string();
    for content in app.pasted_contents.iter().rev() {
        let placeholder = paste_placeholder(content);
        if let Some(pos) = result.rfind(&placeholder) {
            result.replace_range(pos..pos + placeholder.len(), content);
        }
    }
    result
}

pub(super) fn queue_message(app: &mut App) {
    let prepared = take_prepared_input(app);
    app.queued_messages.push(prepared.expanded);
}

pub(super) fn retrieve_pending_message_for_edit(app: &mut App) -> bool {
    if !app.input.is_empty() {
        return false;
    }

    let mut parts: Vec<String> = Vec::new();
    let mut had_pending = false;

    if !app.pending_soft_interrupts.is_empty() {
        parts.extend(std::mem::take(&mut app.pending_soft_interrupts));
        had_pending = true;
    }
    if let Some(msg) = app.interleave_message.take() {
        if !msg.is_empty() {
            parts.push(msg);
        }
    }
    parts.extend(std::mem::take(&mut app.queued_messages));

    if !parts.is_empty() {
        app.input = parts.join("\n\n");
        app.cursor_pos = app.input.len();
        let count = parts.len();
        app.set_status_notice(&format!(
            "Retrieved {} pending message{} for editing",
            count,
            if count == 1 { "" } else { "s" }
        ));
    }

    had_pending
}

pub(super) fn send_action(app: &App, shift: bool) -> SendAction {
    if !app.is_processing {
        return SendAction::Submit;
    }
    if app.input.trim().starts_with('/') {
        return SendAction::Submit;
    }
    if shift {
        if app.queue_mode {
            SendAction::Interleave
        } else {
            SendAction::Queue
        }
    } else if app.queue_mode {
        SendAction::Queue
    } else {
        SendAction::Interleave
    }
}

pub(super) fn handle_shift_enter(app: &mut App) {
    if app.input.is_empty() {
        return;
    }
    match send_action(app, true) {
        SendAction::Submit => app.submit_input(),
        SendAction::Queue => queue_message(app),
        SendAction::Interleave => {
            let prepared = take_prepared_input(app);
            stage_local_interleave(app, prepared.expanded);
        }
    }
}

pub(super) fn handle_control_key(app: &mut App, code: KeyCode) -> bool {
    match code {
        KeyCode::Char('u') => {
            app.input.drain(..app.cursor_pos);
            app.cursor_pos = 0;
            app.sync_model_picker_preview_from_input();
            true
        }
        KeyCode::Char('a') => {
            app.cursor_pos = 0;
            true
        }
        KeyCode::Char('e') => {
            app.cursor_pos = app.input.len();
            true
        }
        KeyCode::Char('b') => {
            if app.cursor_pos > 0 {
                app.cursor_pos = super::super::core::prev_char_boundary(&app.input, app.cursor_pos);
            }
            true
        }
        KeyCode::Char('f') => {
            if app.cursor_pos < app.input.len() {
                app.cursor_pos = super::super::core::next_char_boundary(&app.input, app.cursor_pos);
            }
            true
        }
        KeyCode::Char('w') => {
            let start = app.find_word_boundary_back();
            app.input.drain(start..app.cursor_pos);
            app.cursor_pos = start;
            app.sync_model_picker_preview_from_input();
            true
        }
        KeyCode::Char('s') => {
            app.toggle_input_stash();
            true
        }
        KeyCode::Char('v') => {
            paste_image_from_clipboard(app);
            true
        }
        KeyCode::Tab | KeyCode::Char('t') => {
            app.queue_mode = !app.queue_mode;
            let mode_str = if app.queue_mode {
                "Queue mode: messages wait until response completes"
            } else {
                "Immediate mode: messages send next (no interrupt)"
            };
            app.set_status_notice(mode_str);
            true
        }
        KeyCode::Up => {
            retrieve_pending_message_for_edit(app);
            true
        }
        _ => false,
    }
}

pub(super) fn handle_alt_key(app: &mut App, code: KeyCode) -> bool {
    match code {
        KeyCode::Char('b') => {
            app.cursor_pos = app.find_word_boundary_back();
            true
        }
        KeyCode::Char('f') => {
            app.cursor_pos = app.find_word_boundary_forward();
            true
        }
        KeyCode::Char('d') => {
            let end = app.find_word_boundary_forward();
            app.input.drain(app.cursor_pos..end);
            app.sync_model_picker_preview_from_input();
            true
        }
        KeyCode::Backspace => {
            let start = app.find_word_boundary_back();
            app.input.drain(start..app.cursor_pos);
            app.cursor_pos = start;
            app.sync_model_picker_preview_from_input();
            true
        }
        KeyCode::Char('i') => {
            crate::tui::info_widget::toggle_enabled();
            let status = if crate::tui::info_widget::is_enabled() {
                "Info widget: ON"
            } else {
                "Info widget: OFF"
            };
            app.set_status_notice(status);
            true
        }
        KeyCode::Char('v') => {
            paste_image_from_clipboard(app);
            true
        }
        _ => false,
    }
}

pub(super) fn handle_navigation_shortcuts(
    app: &mut App,
    code: KeyCode,
    modifiers: KeyModifiers,
) -> bool {
    if let Some(amount) = app.scroll_keys.scroll_amount(code.clone(), modifiers) {
        if amount < 0 {
            app.scroll_up((-amount) as usize);
        } else {
            app.scroll_down(amount as usize);
        }
        return true;
    }

    if let Some(dir) = app.scroll_keys.prompt_jump(code.clone(), modifiers) {
        if dir < 0 {
            app.scroll_to_prev_prompt();
        } else {
            app.scroll_to_next_prompt();
        }
        return true;
    }

    if let Some(rank) = App::ctrl_prompt_rank(&code, modifiers) {
        app.scroll_to_recent_prompt_rank(rank);
        return true;
    }

    if app.scroll_keys.is_bookmark(code.clone(), modifiers) {
        app.toggle_scroll_bookmark();
        return true;
    }

    if code == KeyCode::BackTab {
        app.diff_mode = app.diff_mode.cycle();
        if !app.diff_mode.has_side_pane() {
            app.diff_pane_focus = false;
        }
        let status = format!("Diffs: {}", app.diff_mode.label());
        app.set_status_notice(&status);
        return true;
    }

    false
}

pub(super) fn handle_pre_control_shortcuts(
    app: &mut App,
    code: KeyCode,
    modifiers: KeyModifiers,
) -> bool {
    if modifiers.contains(KeyModifiers::ALT) && matches!(code, KeyCode::Char('m')) {
        app.toggle_diagram_pane();
        return true;
    }
    if modifiers.contains(KeyModifiers::ALT) && matches!(code, KeyCode::Char('t')) {
        app.toggle_diagram_pane_position();
        return true;
    }
    if let Some(direction) = app.model_switch_keys.direction_for(code.clone(), modifiers) {
        app.cycle_model(direction);
        return true;
    }
    if let Some(direction) = app
        .effort_switch_keys
        .direction_for(code.clone(), modifiers)
    {
        app.cycle_effort(direction);
        return true;
    }
    if app
        .centered_toggle_keys
        .toggle
        .matches(code.clone(), modifiers)
    {
        app.toggle_centered_mode();
        return true;
    }

    app.normalize_diagram_state();
    let diagram_available = app.diagram_available();
    if app.handle_diagram_focus_key(code.clone(), modifiers, diagram_available) {
        return true;
    }
    if app.handle_diff_pane_focus_key(code.clone(), modifiers) {
        return true;
    }
    if modifiers.contains(KeyModifiers::ALT) && handle_alt_key(app, code.clone()) {
        return true;
    }

    handle_navigation_shortcuts(app, code, modifiers)
}

pub(super) fn handle_modal_key(
    app: &mut App,
    code: KeyCode,
    modifiers: KeyModifiers,
) -> Result<bool> {
    if app.changelog_scroll.is_some() {
        app.handle_changelog_key(code)?;
        return Ok(true);
    }

    if app.help_scroll.is_some() {
        app.handle_help_key(code)?;
        return Ok(true);
    }

    if app.session_picker_overlay.is_some() {
        app.handle_session_picker_key(code, modifiers)?;
        return Ok(true);
    }

    if let Some(ref picker) = app.picker_state {
        if !picker.preview {
            app.handle_picker_key(code, modifiers)?;
            return Ok(true);
        }
    }

    if app.handle_picker_preview_key(&code, modifiers)? {
        return Ok(true);
    }

    Ok(false)
}

pub(super) fn handle_global_control_shortcuts(
    app: &mut App,
    code: KeyCode,
    diagram_available: bool,
) -> bool {
    if app.handle_diagram_ctrl_key(code.clone(), diagram_available) {
        return true;
    }

    match code {
        KeyCode::Char('c') | KeyCode::Char('d') => {
            if app.is_processing {
                app.cancel_requested = true;
                app.interleave_message = None;
                app.pending_soft_interrupts.clear();
                app.set_status_notice("Interrupting...");
            } else {
                app.handle_quit_request();
            }
            true
        }
        KeyCode::Char('r') => {
            app.recover_session_without_tools();
            true
        }
        KeyCode::Char('l')
            if !app.is_processing && !diagram_available && !app.diff_pane_visible() =>
        {
            commands::reset_current_session(app);
            true
        }
        _ => handle_control_key(app, code),
    }
}

pub(super) fn handle_enter(app: &mut App) -> bool {
    if app.activate_model_picker_from_preview() {
        return true;
    }
    if !app.input.is_empty() {
        match send_action(app, false) {
            SendAction::Submit => app.submit_input(),
            SendAction::Queue => queue_message(app),
            SendAction::Interleave => {
                let prepared = take_prepared_input(app);
                stage_local_interleave(app, prepared.expanded);
            }
        }
    }
    true
}

pub(super) fn handle_basic_key(app: &mut App, code: KeyCode) -> bool {
    match code {
        KeyCode::Char(c) => {
            if app.input.is_empty() && !app.is_processing && app.display_messages.is_empty() {
                if let Some(digit) = c.to_digit(10) {
                    let suggestions = app.suggestion_prompts();
                    let idx = digit as usize;
                    if idx >= 1 && idx <= suggestions.len() {
                        let (_label, prompt) = &suggestions[idx - 1];
                        if !prompt.starts_with('/') {
                            app.input = prompt.clone();
                            app.cursor_pos = app.input.len();
                            app.follow_chat_bottom();
                            return true;
                        }
                    }
                }
            }
            app.input.insert(app.cursor_pos, c);
            app.cursor_pos += c.len_utf8();
            app.reset_tab_completion();
            app.sync_model_picker_preview_from_input();
            true
        }
        KeyCode::Backspace => {
            if app.cursor_pos > 0 {
                let prev = super::super::core::prev_char_boundary(&app.input, app.cursor_pos);
                app.input.drain(prev..app.cursor_pos);
                app.cursor_pos = prev;
                app.reset_tab_completion();
                app.sync_model_picker_preview_from_input();
            }
            true
        }
        KeyCode::Delete => {
            if app.cursor_pos < app.input.len() {
                let next = super::super::core::next_char_boundary(&app.input, app.cursor_pos);
                app.input.drain(app.cursor_pos..next);
                app.reset_tab_completion();
                app.sync_model_picker_preview_from_input();
            }
            true
        }
        KeyCode::Left => {
            if app.cursor_pos > 0 {
                app.cursor_pos = super::super::core::prev_char_boundary(&app.input, app.cursor_pos);
            }
            true
        }
        KeyCode::Right => {
            if app.cursor_pos < app.input.len() {
                app.cursor_pos = super::super::core::next_char_boundary(&app.input, app.cursor_pos);
            }
            true
        }
        KeyCode::Home => {
            app.cursor_pos = 0;
            true
        }
        KeyCode::End => {
            app.cursor_pos = app.input.len();
            true
        }
        KeyCode::Tab => {
            app.autocomplete();
            true
        }
        KeyCode::Up | KeyCode::PageUp => {
            let inc = if code == KeyCode::PageUp { 10 } else { 1 };
            app.scroll_up(inc);
            true
        }
        KeyCode::Down | KeyCode::PageDown => {
            let dec = if code == KeyCode::PageDown { 10 } else { 1 };
            app.scroll_down(dec);
            true
        }
        KeyCode::Esc => {
            if app
                .picker_state
                .as_ref()
                .map(|p| p.preview)
                .unwrap_or(false)
            {
                app.picker_state = None;
                app.input.clear();
                app.cursor_pos = 0;
            } else if app.is_processing {
                app.cancel_requested = true;
                app.interleave_message = None;
                app.pending_soft_interrupts.clear();
            } else {
                app.follow_chat_bottom();
                app.input.clear();
                app.cursor_pos = 0;
                app.sync_model_picker_preview_from_input();
            }
            true
        }
        _ => false,
    }
}

pub(super) fn take_prepared_input(app: &mut App) -> PreparedInput {
    let raw_input = std::mem::take(&mut app.input);
    let expanded = expand_paste_placeholders(app, &raw_input);
    app.pasted_contents.clear();
    let images = std::mem::take(&mut app.pending_images);
    app.cursor_pos = 0;
    PreparedInput {
        raw_input,
        expanded,
        images,
    }
}

pub(super) fn stage_local_interleave(app: &mut App, content: String) {
    app.interleave_message = Some(content);
    app.set_status_notice("⏭ Sending now (interleave)");
}

fn attach_image(app: &mut App, media_type: String, base64_data: String) {
    let size_kb = base64_data.len() / 1024;
    app.pending_images.push((media_type.clone(), base64_data));
    let placeholder = format!("[image {}]", app.pending_images.len());
    app.input.insert_str(app.cursor_pos, &placeholder);
    app.cursor_pos += placeholder.len();
    app.sync_model_picker_preview_from_input();
    app.set_status_notice(&format!("Pasted {} ({} KB)", media_type, size_kb));
}

fn paste_placeholder(content: &str) -> String {
    let line_count = content.lines().count().max(1);
    format!(
        "[pasted {} line{}]",
        line_count,
        if line_count == 1 { "" } else { "s" }
    )
}
