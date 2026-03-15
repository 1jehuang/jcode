use super::{
    App, ContentBlock, DisplayMessage, Message, ProcessingStatus, Role, SendAction, SkillRegistry,
    commands, ctrl_bracket_fallback_to_esc, is_context_limit_error, remote,
};
use anyhow::Result;
use crossterm::event::{EventStream, KeyCode, KeyModifiers};
use ratatui::DefaultTerminal;
use std::time::{Duration, Instant};

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

pub(super) fn paste_from_clipboard(app: &mut App) {
    if let Ok(mut clipboard) = arboard::Clipboard::new() {
        if let Ok(text) = clipboard.get_text() {
            handle_paste(app, text);
            return;
        }
    }

    if let Some((media_type, base64_data)) = super::clipboard_image() {
        attach_image(app, media_type, base64_data);
        return;
    }

    app.set_status_notice("No text or image in clipboard");
}

pub(super) fn cut_input_line_to_clipboard(app: &mut App) -> bool {
    cut_input_line_to_clipboard_with(app, super::copy_to_clipboard)
}

pub(super) fn cut_input_line_to_clipboard_with<F>(app: &mut App, mut copy_text: F) -> bool
where
    F: FnMut(&str) -> bool,
{
    if app.input.is_empty() {
        return false;
    }

    if !copy_text(&app.input) {
        app.set_status_notice("Failed to copy input line");
        return false;
    }

    app.input.clear();
    app.cursor_pos = 0;
    app.reset_tab_completion();
    app.sync_model_picker_preview_from_input();
    app.set_status_notice("✂ Cut input line");
    true
}

pub(super) fn handle_paste(app: &mut App, text: String) {
    // Note: clipboard_image() is NOT checked here. Bracketed paste events from the
    // terminal always deliver text. Checking clipboard_image() here caused a bug where
    // text pastes were misidentified as images when the clipboard also had image data
    // (common on Wayland where apps advertise multiple MIME types). Image pasting is
    // handled by explicit clipboard shortcuts instead (Ctrl+V smart-pastes, Alt+V forces image).
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
        insert_input_text(app, &text);
    } else {
        app.pasted_contents.push(text);
        let placeholder = format!(
            "[pasted {} line{}]",
            line_count,
            if line_count == 1 { "" } else { "s" }
        );
        insert_input_text(app, &placeholder);
    }
}

pub(super) fn insert_input_text(app: &mut App, text: &str) {
    if text.is_empty() {
        return;
    }

    app.input.insert_str(app.cursor_pos, text);
    app.cursor_pos += text.len();
    app.reset_tab_completion();
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

pub(super) fn send_action(app: &App, alternate_shortcut: bool) -> SendAction {
    if !app.is_processing {
        return SendAction::Submit;
    }
    if app.input.trim().starts_with('/') {
        return SendAction::Submit;
    }
    if alternate_shortcut {
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
    insert_input_text(app, "\n");
}

pub(super) fn handle_alternate_enter(app: &mut App) {
    if app.activate_model_picker_from_preview() {
        return;
    }

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
        KeyCode::Char('x') => {
            cut_input_line_to_clipboard(app);
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
                app.cursor_pos = app.find_word_boundary_back();
            }
            true
        }
        KeyCode::Char('f') => {
            if app.cursor_pos < app.input.len() {
                app.cursor_pos = app.find_word_boundary_forward();
            }
            true
        }
        KeyCode::Char('w') | KeyCode::Backspace => {
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
            paste_from_clipboard(app);
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
        KeyCode::Left => {
            if app.cursor_pos > 0 {
                app.cursor_pos = app.find_word_boundary_back();
            }
            true
        }
        KeyCode::Right => {
            if app.cursor_pos < app.input.len() {
                app.cursor_pos = app.find_word_boundary_forward();
            }
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
    if let Some(amount) = app.scroll_keys.scroll_amount(code, modifiers) {
        if amount < 0 {
            app.scroll_up((-amount) as usize);
        } else {
            app.scroll_down(amount as usize);
        }
        return true;
    }

    if let Some(dir) = app.scroll_keys.prompt_jump(code, modifiers) {
        if dir < 0 {
            app.scroll_to_prev_prompt();
        } else {
            app.scroll_to_next_prompt();
        }
        return true;
    }

    if let Some(ratio) = App::ctrl_side_panel_ratio_preset(&code, modifiers) {
        app.set_side_panel_ratio_preset(ratio);
        return true;
    }

    if let Some(rank) = App::ctrl_prompt_rank(&code, modifiers) {
        app.scroll_to_recent_prompt_rank(rank);
        return true;
    }

    if app.scroll_keys.is_bookmark(code, modifiers) {
        app.toggle_scroll_bookmark();
        return true;
    }

    if code == KeyCode::BackTab {
        app.diff_mode = app.diff_mode.cycle();
        if !app.diff_pane_visible() {
            app.diff_pane_focus = false;
        }
        let status = format!("Diffs: {}", app.diff_mode.label());
        app.set_status_notice(&status);
        return true;
    }

    false
}

pub(super) fn is_scroll_only_key(app: &App, code: KeyCode, modifiers: KeyModifiers) -> bool {
    let mut code = code;
    let mut modifiers = modifiers;
    ctrl_bracket_fallback_to_esc(&mut code, &mut modifiers);

    if app.scroll_keys.scroll_amount(code, modifiers).is_some()
        || app.scroll_keys.prompt_jump(code, modifiers).is_some()
        || App::ctrl_side_panel_ratio_preset(&code, modifiers).is_some()
        || App::ctrl_prompt_rank(&code, modifiers).is_some()
        || app.scroll_keys.is_bookmark(code, modifiers)
        || code == KeyCode::BackTab
    {
        return true;
    }

    if app.diff_pane_focus && !modifiers.contains(KeyModifiers::CONTROL) {
        match code {
            KeyCode::Char('j')
            | KeyCode::Down
            | KeyCode::Char('k')
            | KeyCode::Up
            | KeyCode::Char('d')
            | KeyCode::PageDown
            | KeyCode::Char('u')
            | KeyCode::PageUp
            | KeyCode::Char('g')
            | KeyCode::Home
            | KeyCode::Char('G')
            | KeyCode::End
            | KeyCode::Esc => return true,
            _ => {}
        }
    }

    let diagram_available = app.diagram_available();
    if diagram_available && app.diagram_focus && !modifiers.contains(KeyModifiers::CONTROL) {
        match code {
            KeyCode::Char('h')
            | KeyCode::Left
            | KeyCode::Char('l')
            | KeyCode::Right
            | KeyCode::Char('k')
            | KeyCode::Up
            | KeyCode::Char('j')
            | KeyCode::Down
            | KeyCode::Char('+')
            | KeyCode::Char('=')
            | KeyCode::Char('-')
            | KeyCode::Char('_')
            | KeyCode::Char(']')
            | KeyCode::Char('[')
            | KeyCode::Char('o')
            | KeyCode::Esc => return true,
            _ => {}
        }
    }

    if modifiers.contains(KeyModifiers::CONTROL) {
        if diagram_available {
            match code {
                KeyCode::Left | KeyCode::Right | KeyCode::Char('h') | KeyCode::Char('l') => {
                    return true;
                }
                _ => {}
            }
        }
        if app.diff_pane_visible() {
            match code {
                KeyCode::Char('h') | KeyCode::Char('l') => return true,
                _ => {}
            }
        }
    }

    false
}

pub(super) fn handle_pre_control_shortcuts(
    app: &mut App,
    code: KeyCode,
    modifiers: KeyModifiers,
) -> bool {
    if modifiers.contains(KeyModifiers::ALT) && matches!(code, KeyCode::Char('y')) {
        app.toggle_copy_selection_mode();
        return true;
    }

    if handle_visible_copy_shortcut(app, code, modifiers) {
        return true;
    }

    if modifiers.contains(KeyModifiers::ALT) && matches!(code, KeyCode::Char('m')) {
        app.toggle_diagram_pane();
        return true;
    }
    if modifiers.contains(KeyModifiers::ALT) && matches!(code, KeyCode::Char('t')) {
        app.toggle_diagram_pane_position();
        return true;
    }
    if modifiers.contains(KeyModifiers::ALT) && matches!(code, KeyCode::Char('s')) {
        app.toggle_typing_scroll_lock();
        return true;
    }
    if app.dictation_key_matches(code, modifiers) {
        app.handle_dictation_trigger();
        return true;
    }
    if let Some(direction) = app.model_switch_keys.direction_for(code, modifiers) {
        app.cycle_model(direction);
        return true;
    }
    if let Some(direction) = app.effort_switch_keys.direction_for(code, modifiers) {
        app.cycle_effort(direction);
        return true;
    }
    if app.centered_toggle_keys.toggle.matches(code, modifiers) {
        app.toggle_centered_mode();
        return true;
    }

    app.normalize_diagram_state();
    let diagram_available = app.diagram_available();
    if app.handle_diagram_focus_key(code, modifiers, diagram_available) {
        return true;
    }
    if app.handle_diff_pane_focus_key(code, modifiers) {
        return true;
    }
    if modifiers.contains(KeyModifiers::ALT) && handle_alt_key(app, code) {
        return true;
    }

    handle_navigation_shortcuts(app, code, modifiers)
}

pub(super) fn handle_visible_copy_shortcut(
    app: &mut App,
    code: KeyCode,
    modifiers: KeyModifiers,
) -> bool {
    let KeyCode::Char(c) = code else {
        return false;
    };

    if !modifiers.contains(KeyModifiers::ALT) {
        return false;
    }

    // Many terminals encode Alt+Shift+<letter> as just Alt + uppercase letter
    // instead of reporting an explicit Shift modifier. Accept either form so the
    // on-screen [Alt] [⇧] copy badges behave consistently.
    let explicit_shift = modifiers.contains(KeyModifiers::SHIFT);
    let implicit_shift = c.is_ascii_uppercase();
    if !explicit_shift && !implicit_shift {
        return false;
    }

    if let Some(target) = crate::tui::ui::visible_copy_target_for_key(c) {
        let success = super::copy_to_clipboard(&target.content);
        app.record_copy_badge_key_press(c);
        app.record_copy_badge_feedback(c, success);
        if success {
            app.set_status_notice(target.copied_notice);
        } else {
            app.set_status_notice(format!("Failed to copy {}", target.kind_label));
        }
        return true;
    }

    false
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

    if app.account_picker_overlay.is_some() {
        if let Some(command) = app.next_account_picker_action(code, modifiers)? {
            app.handle_account_picker_command(command);
        }
        return Ok(true);
    }

    if app.copy_selection_mode {
        if modifiers.contains(KeyModifiers::CONTROL)
            && matches!(code, KeyCode::Char('c') | KeyCode::Char('d'))
        {
            return Ok(false);
        }

        let handled = app.handle_copy_selection_key(code, modifiers)
            || handle_navigation_shortcuts(app, code, modifiers);
        return Ok(handled || true);
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
    if app.handle_diagram_ctrl_key(code, diagram_available) {
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
                            app.follow_chat_bottom_for_typing();
                            return true;
                        }
                    }
                }
            }
            insert_input_text(app, &c.to_string());
            true
        }
        KeyCode::Backspace => {
            if app.cursor_pos > 0 {
                let prev = crate::tui::core::prev_char_boundary(&app.input, app.cursor_pos);
                app.input.drain(prev..app.cursor_pos);
                app.cursor_pos = prev;
                app.reset_tab_completion();
                app.sync_model_picker_preview_from_input();
            }
            true
        }
        KeyCode::Delete => {
            if app.cursor_pos < app.input.len() {
                let next = crate::tui::core::next_char_boundary(&app.input, app.cursor_pos);
                app.input.drain(app.cursor_pos..next);
                app.reset_tab_completion();
                app.sync_model_picker_preview_from_input();
            }
            true
        }
        KeyCode::Left => {
            if app.cursor_pos > 0 {
                app.cursor_pos = crate::tui::core::prev_char_boundary(&app.input, app.cursor_pos);
            }
            true
        }
        KeyCode::Right => {
            if app.cursor_pos < app.input.len() {
                app.cursor_pos = crate::tui::core::next_char_boundary(&app.input, app.cursor_pos);
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

impl App {
    pub(super) fn handle_key_event(&mut self, event: crossterm::event::KeyEvent) {
        // Record the event if recording is active
        use crate::tui::test_harness::{TestEvent, record_event};
        let modifiers: Vec<String> = {
            let mut mods = vec![];
            if event.modifiers.contains(KeyModifiers::CONTROL) {
                mods.push("ctrl".to_string());
            }
            if event.modifiers.contains(KeyModifiers::ALT) {
                mods.push("alt".to_string());
            }
            if event.modifiers.contains(KeyModifiers::SHIFT) {
                mods.push("shift".to_string());
            }
            mods
        };
        let code_str = format!("{:?}", event.code);
        record_event(TestEvent::Key {
            code: code_str,
            modifiers,
        });

        self.update_copy_badge_key_event(event);
        if matches!(
            event.kind,
            crossterm::event::KeyEventKind::Press | crossterm::event::KeyEventKind::Repeat
        ) {
            let _ = self.handle_key(event.code, event.modifiers);
        }
    }

    pub(super) fn handle_key(&mut self, code: KeyCode, modifiers: KeyModifiers) -> Result<()> {
        let mut code = code;
        let mut modifiers = modifiers;
        ctrl_bracket_fallback_to_esc(&mut code, &mut modifiers);

        if handle_modal_key(self, code, modifiers)? {
            return Ok(());
        }

        if handle_pre_control_shortcuts(self, code, modifiers) {
            return Ok(());
        }

        self.normalize_diagram_state();
        let diagram_available = self.diagram_available();

        // Handle ctrl combos regardless of processing state
        if modifiers.contains(KeyModifiers::CONTROL)
            && handle_global_control_shortcuts(self, code, diagram_available)
        {
            return Ok(());
        }

        // Ctrl+Enter: does opposite of queue_mode during processing
        if code == KeyCode::Enter && modifiers.contains(KeyModifiers::CONTROL) {
            handle_alternate_enter(self);
            return Ok(());
        }

        // Shift+Enter inserts a newline in the input box
        if code == KeyCode::Enter && modifiers.contains(KeyModifiers::SHIFT) {
            handle_shift_enter(self);
            return Ok(());
        }

        // When the model picker preview is visible, arrow keys navigate the picker list
        if self
            .picker_state
            .as_ref()
            .map(|p| p.preview)
            .unwrap_or(false)
        {
            match code {
                KeyCode::Up | KeyCode::Down | KeyCode::PageUp | KeyCode::PageDown => {
                    return self.handle_picker_key(code, modifiers);
                }
                _ => {}
            }
        }

        if code == KeyCode::Enter {
            handle_enter(self);
            return Ok(());
        }

        if handle_basic_key(self, code) {
            return Ok(());
        }

        Ok(())
    }

    pub(super) fn redraw_now(&self, terminal: &mut DefaultTerminal) -> Result<()> {
        terminal.draw(|frame| crate::tui::ui::draw(frame, self))?;
        Ok(())
    }

    pub(super) fn update_copy_badge_key_event(&mut self, event: crossterm::event::KeyEvent) {
        use crossterm::event::{KeyCode, KeyEventKind, ModifierKeyCode};

        self.prune_copy_badge_ui();
        let pulse_until = std::time::Instant::now() + std::time::Duration::from_millis(240);

        match (event.kind, event.code) {
            (KeyEventKind::Press | KeyEventKind::Repeat, KeyCode::Modifier(modifier)) => {
                match modifier {
                    ModifierKeyCode::LeftAlt | ModifierKeyCode::RightAlt => {
                        self.copy_badge_ui.alt_active = true;
                        self.copy_badge_ui.alt_pulse_until = Some(pulse_until);
                    }
                    ModifierKeyCode::LeftShift | ModifierKeyCode::RightShift => {
                        self.copy_badge_ui.shift_active = true;
                        self.copy_badge_ui.shift_pulse_until = Some(pulse_until);
                    }
                    _ => {}
                }
            }
            (KeyEventKind::Release, KeyCode::Modifier(modifier)) => match modifier {
                ModifierKeyCode::LeftAlt | ModifierKeyCode::RightAlt => {
                    self.copy_badge_ui.alt_active = false;
                }
                ModifierKeyCode::LeftShift | ModifierKeyCode::RightShift => {
                    self.copy_badge_ui.shift_active = false;
                }
                _ => {}
            },
            (KeyEventKind::Press | KeyEventKind::Repeat, KeyCode::Char(c)) => {
                if event.modifiers.contains(KeyModifiers::ALT) {
                    self.copy_badge_ui.alt_pulse_until = Some(pulse_until);
                }
                if event.modifiers.contains(KeyModifiers::SHIFT) || c.is_ascii_uppercase() {
                    self.copy_badge_ui.shift_pulse_until = Some(pulse_until);
                }
                self.record_copy_badge_key_press(c);
            }
            (KeyEventKind::Release, KeyCode::Char(c)) => {
                if let Some((active, _)) = self.copy_badge_ui.key_active {
                    if active.eq_ignore_ascii_case(&c) {
                        self.copy_badge_ui.key_active = None;
                    }
                }
                if !event.modifiers.contains(KeyModifiers::ALT) {
                    self.copy_badge_ui.alt_active = false;
                }
                if !event.modifiers.contains(KeyModifiers::SHIFT) {
                    self.copy_badge_ui.shift_active = false;
                }
            }
            _ => {}
        }
    }

    pub(super) fn record_copy_badge_key_press(&mut self, key: char) {
        let expiry = std::time::Instant::now() + std::time::Duration::from_millis(240);
        self.copy_badge_ui.key_active = Some((key, expiry));
    }

    pub(super) fn record_copy_badge_feedback(&mut self, key: char, success: bool) {
        self.copy_badge_ui.copied_feedback = Some(crate::tui::app::CopyBadgeFeedback {
            key,
            success,
            expires_at: std::time::Instant::now() + std::time::Duration::from_millis(1100),
        });
    }

    pub(super) fn prune_copy_badge_ui(&mut self) {
        let now = std::time::Instant::now();
        if self
            .copy_badge_ui
            .alt_pulse_until
            .map(|expires_at| expires_at <= now)
            .unwrap_or(false)
        {
            self.copy_badge_ui.alt_pulse_until = None;
        }
        if self
            .copy_badge_ui
            .shift_pulse_until
            .map(|expires_at| expires_at <= now)
            .unwrap_or(false)
        {
            self.copy_badge_ui.shift_pulse_until = None;
        }
        if self
            .copy_badge_ui
            .key_active
            .as_ref()
            .map(|(_, expires_at)| *expires_at <= now)
            .unwrap_or(false)
        {
            self.copy_badge_ui.key_active = None;
        }
        if self
            .copy_badge_ui
            .copied_feedback
            .as_ref()
            .map(|feedback| feedback.expires_at <= now)
            .unwrap_or(false)
        {
            self.copy_badge_ui.copied_feedback = None;
        }
    }

    /// Try to paste an image from the clipboard. Checks native image data first,
    /// then falls back to HTML clipboard for <img> URLs, then arboard text.
    /// Used by Alt+V handlers in both local and remote mode.
    pub(super) fn paste_image_from_clipboard(&mut self) {
        paste_image_from_clipboard(self);
    }

    /// Try to paste whatever is in the clipboard.
    /// Prefers text when available, otherwise falls back to image data.
    /// Used by Ctrl+V handlers in both local and remote mode.
    pub(super) fn paste_from_clipboard(&mut self) {
        paste_from_clipboard(self);
    }

    /// Queue a message to be sent later
    /// Handle bracketed paste: store text content (image URLs are still detected inline)
    pub(super) fn handle_paste(&mut self, text: String) {
        handle_paste(self, text);
    }

    /// Expand paste placeholders in input with actual content
    pub(super) fn expand_paste_placeholders(&mut self, input: &str) -> String {
        expand_paste_placeholders(self, input)
    }

    pub(super) fn queue_message(&mut self) {
        queue_message(self);
    }

    /// Send an interleave message immediately to the server as a soft interrupt.
    /// Skips the intermediate buffer stage - goes directly to pending_soft_interrupts.
    pub(super) async fn send_interleave_now(
        &mut self,
        content: String,
        remote: &mut crate::tui::backend::RemoteConnection,
    ) {
        remote::send_interleave_now(self, content, remote).await;
    }

    /// Retrieve all pending unsent messages into the input for editing.
    /// Priority: pending soft interrupts first, then interleave, then queued.
    /// Returns true if pending soft interrupts were retrieved (caller should cancel on server).
    pub(super) fn retrieve_pending_message_for_edit(&mut self) -> bool {
        retrieve_pending_message_for_edit(self)
    }

    pub(super) fn send_action(&self, shift: bool) -> SendAction {
        send_action(self, shift)
    }

    pub(super) fn insert_thought_line(&mut self, line: String) {
        if self.thought_line_inserted || line.is_empty() {
            return;
        }
        self.thought_line_inserted = true;
        let mut prefix = line;
        if !prefix.ends_with('\n') {
            prefix.push('\n');
        }
        prefix.push('\n');
        if self.streaming_text.is_empty() {
            self.streaming_text = prefix;
        } else {
            self.streaming_text = format!("{}{}", prefix, self.streaming_text);
        }
    }

    pub(super) fn clear_streaming_render_state(&mut self) {
        self.streaming_text.clear();
        self.streaming_md_renderer.borrow_mut().reset();
        crate::tui::mermaid::clear_streaming_preview_diagram();
    }

    pub(super) fn take_streaming_text(&mut self) -> String {
        let content = std::mem::take(&mut self.streaming_text);
        self.streaming_md_renderer.borrow_mut().reset();
        crate::tui::mermaid::clear_streaming_preview_diagram();
        content
    }

    pub(super) fn commit_pending_streaming_assistant_message(&mut self) -> bool {
        if let Some(chunk) = self.stream_buffer.flush() {
            self.streaming_text.push_str(&chunk);
        }

        if self.streaming_text.is_empty() {
            self.stream_buffer.clear();
            return false;
        }

        let content = self.take_streaming_text();
        self.push_display_message(DisplayMessage::assistant(content));
        self.stream_buffer.clear();
        true
    }

    pub(super) fn accumulate_streaming_output_tokens(
        &mut self,
        output_tokens: u64,
        call_output_tokens_seen: &mut u64,
    ) {
        let delta = if output_tokens >= *call_output_tokens_seen {
            output_tokens - *call_output_tokens_seen
        } else {
            // Usage snapshots should be monotonic within one API call. If they are not,
            // treat this as a reset and count the full value once.
            output_tokens
        };
        self.streaming_total_output_tokens += delta;
        *call_output_tokens_seen = output_tokens;
    }

    pub(super) fn command_help(&self, topic: &str) -> Option<String> {
        let topic = topic.trim().trim_start_matches('/').to_lowercase();
        let help = match topic.as_str() {
            "help" | "commands" => {
                "`/help`\nShow general command list and keyboard shortcuts.\n\n`/help <command>`\nShow detailed help for one command."
            }
            "compact" => {
                "`/compact`\nForce context compaction now.\nStarts background summarization and applies it automatically when ready.\n\n`/compact mode`\nShow current compaction mode for this session.\n\n`/compact mode <reactive|proactive|semantic>`\nChange compaction mode for this session."
            }
            "fix" => {
                "`/fix`\nRun recovery actions when the model cannot continue.\nRepairs missing tool outputs, resets provider session state, and starts compaction when possible."
            }
            "rewind" => {
                "`/rewind`\nShow numbered conversation history.\n\n`/rewind N`\nRewind to message N (drops everything after it and resets provider session)."
            }
            "clear" => {
                "`/clear`\nClear current conversation, queue, and display; starts a fresh session."
            }
            "model" => {
                "`/model`\nOpen model picker.\n\n`/model <name>`\nSwitch model.\n\n`/model <name>@<provider>`\nPin OpenRouter routing (`@auto` clears pin)."
            }
            "effort" => {
                "`/effort`\nShow current reasoning effort.\n\n`/effort <level>`\nSet reasoning effort (none|low|medium|high|xhigh).\n\nAlso: Alt+←/→ to cycle."
            }
            "fast" => {
                "`/fast`\nShow whether OpenAI/Codex fast mode is enabled.\n\n`/fast on`\nEnable fast mode (`service_tier = \"priority\"`).\n\n`/fast off`\nDisable fast mode.\n\n`/fast status`\nShow current fast-mode status."
            }
            "memory" => "`/memory [on|off|status]`\nToggle memory features for this session.",
            "swarm" => "`/swarm [on|off|status]`\nToggle swarm features for this session.",
            "dictate" | "dictation" => {
                "`/dictate`\nRun the configured external speech-to-text command and inject the transcript into jcode.\n\nConfigure `[dictation]` in `~/.jcode/config.toml`:\n- `command`: shell command that prints transcript to stdout\n- `mode`: `insert|append|replace|send`\n- `key`: optional hotkey (for example `alt+;`)\n- `timeout_secs`: max wait time"
            }
            "poke" => {
                "`/poke`\nPoke the model to resume when it has stopped with incomplete todos.\n\
                Injects a reminder listing all pending/in-progress tasks and prompts the model to either\n\
                finish the work, update the todo list to reflect what is done, or ask for user input if genuinely blocked."
            }
            "reload" => "`/reload`\nReload to a newer binary if one is available.",
            "restart" => {
                "`/restart`\nRestart jcode with the current binary. Session is preserved.\nUseful after config changes, MCP server updates, or env var changes."
            }
            "rebuild" => "`/rebuild`\nRun full update flow (git pull + cargo build + tests).",
            "split" => {
                "`/split`\nSplit the current session into a new window. Clones the full conversation history so both sessions continue from the same point."
            }
            "resume" | "sessions" => {
                "`/resume`\nOpen the interactive session picker. Browse and search all sessions, preview conversation history, and open any session in a new terminal window.\n\nPress `Esc` to return to your current session."
            }
            "info" => "`/info`\nShow session metadata and token usage.",
            "usage" => {
                "`/usage`\nFetch and display subscription usage limits for connected providers. Today this shows OAuth provider windows (Anthropic, OpenAI/ChatGPT); jcode subscription budget reporting is scaffolded but not yet backed by a live billing service."
            }
            "subscription" => {
                "`/subscription`\nShow curated jcode subscription status for this session, including router config, runtime mode, curated models, and planned tier budget scaffolding."
            }
            "version" => "`/version`\nShow jcode version/build details.",
            "changelog" => "`/changelog`\nShow recent changes embedded in this build.",
            "quit" => "`/quit`\nExit jcode.",
            "config" => {
                "`/config`\nShow active configuration.\n\n`/config init`\nCreate default config file.\n\n`/config edit`\nOpen config in `$EDITOR`."
            }
            "auth" | "login" => {
                "`/auth`\nShow authentication status for all providers.\n\n`/login`\nInteractive provider selection - pick a provider to log into.\n\n`/login <provider>`\nStart login flow directly for any provider shown by `/login` or the `/login ` completions.\n\nUse `/login jcode` for curated jcode subscription access via your router, not OpenRouter BYOK."
            }
            "account" | "accounts" => {
                "`/account`\nOpen the interactive account picker for Anthropic and OpenAI logins.\n\n`/account add <label>`\nAdd a new Anthropic account via OAuth login.\n\n`/account switch <label>`\nSwitch the active Anthropic account.\n\n`/account remove <label>`\nRemove an Anthropic account.\n\n`/account openai`\nOpen the interactive picker focused on OpenAI accounts.\n\n`/account openai add <label>`\nAdd a new OpenAI account via OAuth login.\n\n`/account openai switch <label>`\nSwitch the active OpenAI account.\n\n`/account openai remove <label>`\nRemove an OpenAI account."
            }
            "save" => {
                "`/save`\nBookmark the current session so it appears at the top of `/resume`.\n\n`/save <label>`\nBookmark with a custom label for easy identification.\n\nSaved sessions are shown in a dedicated \"Saved\" section in the session picker."
            }
            "unsave" => "`/unsave`\nRemove the bookmark from the current session.",
            "client-reload" if self.is_remote => {
                "`/client-reload`\nForce client binary reload in remote mode."
            }
            "server-reload" if self.is_remote => {
                "`/server-reload`\nForce server binary reload in remote mode."
            }
            _ => return None,
        };
        Some(help.to_string())
    }

    /// Submit input - just sets up message and flags, processing happens in next loop iteration
    pub(super) fn submit_input(&mut self) {
        if self.activate_model_picker_from_preview() {
            return;
        }

        let raw_input = std::mem::take(&mut self.input);
        let input = self.expand_paste_placeholders(&raw_input);
        self.pasted_contents.clear();
        self.cursor_pos = 0;
        self.follow_chat_bottom(); // Reset to bottom and resume auto-scroll on new input

        if let Some(pending) = self.pending_login.take() {
            self.handle_login_input(pending, input);
            return;
        }

        if let Some(pending) = self.pending_account_label.take() {
            self.handle_pending_account_label_input(pending, input);
            return;
        }

        let trimmed = input.trim();
        if commands::handle_help_command(self, trimmed)
            || commands::handle_session_command(self, trimmed)
            || commands::handle_dictation_command(self, trimmed)
            || commands::handle_config_command(self, trimmed)
            || super::debug::handle_debug_command(self, trimmed)
            || super::model_context::handle_model_command(self, trimmed)
            || super::state_ui::handle_info_command(self, trimmed)
            || super::auth::handle_auth_command(self, trimmed)
            || super::tui_lifecycle::handle_dev_command(self, trimmed)
        {
            return;
        }

        // Check for skill invocation
        if let Some(skill_name) = SkillRegistry::parse_invocation(&input) {
            if let Some(skill) = self.skills.get(skill_name) {
                self.active_skill = Some(skill_name.to_string());
                self.push_display_message(DisplayMessage {
                    role: "system".to_string(),
                    content: format!("Activated skill: {} - {}", skill.name, skill.description),
                    tool_calls: vec![],
                    duration_secs: None,
                    title: None,
                    tool_data: None,
                });
            } else {
                self.push_display_message(DisplayMessage {
                    role: "error".to_string(),
                    content: format!("Unknown skill: /{}", skill_name),
                    tool_calls: vec![],
                    duration_secs: None,
                    title: None,
                    tool_data: None,
                });
            }
            return;
        }

        // Add user message to display (show placeholder to user, not full paste)
        self.push_display_message(DisplayMessage {
            role: "user".to_string(),
            content: raw_input, // Show placeholder to user (condensed view)
            tool_calls: vec![],
            duration_secs: None,
            title: None,
            tool_data: None,
        });
        // Send expanded content (with actual pasted text) to model
        let images = std::mem::take(&mut self.pending_images);
        if !images.is_empty() {
            crate::logging::info(&format!(
                "Submitting with {} image(s): {}",
                images.len(),
                images
                    .iter()
                    .map(|(t, d)| format!("{} ({}KB)", t, d.len() / 1024))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if images.is_empty() {
            self.add_provider_message(Message::user(&input));
            self.session.add_message(
                Role::User,
                vec![ContentBlock::Text {
                    text: input.clone(),
                    cache_control: None,
                }],
            );
        } else {
            self.add_provider_message(Message::user_with_images(&input, images.clone()));
            let mut blocks: Vec<ContentBlock> = images
                .into_iter()
                .map(|(media_type, data)| ContentBlock::Image { media_type, data })
                .collect();
            blocks.push(ContentBlock::Text {
                text: input.clone(),
                cache_control: None,
            });
            self.session.add_message(Role::User, blocks);
        }
        crate::telemetry::record_turn();
        let _ = self.session.save();

        // Set up processing state - actual processing happens after UI redraws
        self.is_processing = true;
        self.status = ProcessingStatus::Sending;
        self.clear_streaming_render_state();
        self.stream_buffer.clear();
        self.thought_line_inserted = false;
        self.thinking_prefix_emitted = false;
        self.thinking_buffer.clear();
        self.streaming_tool_calls.clear();
        self.streaming_input_tokens = 0;
        self.streaming_output_tokens = 0;
        self.streaming_cache_read_tokens = None;
        self.streaming_cache_creation_tokens = None;
        self.upstream_provider = None;
        self.streaming_tps_start = None;
        self.streaming_tps_elapsed = Duration::ZERO;
        self.streaming_total_output_tokens = 0;
        self.processing_started = Some(Instant::now());
        self.pending_turn = true;
    }

    /// Process all queued messages (combined into a single request)
    /// Loops until queue is empty (in case more messages are queued during processing)
    pub(super) async fn process_queued_messages(
        &mut self,
        terminal: &mut DefaultTerminal,
        event_stream: &mut EventStream,
    ) {
        while !self.queued_messages.is_empty() || !self.hidden_queued_system_messages.is_empty() {
            // Combine all currently queued messages into one, treating [SYSTEM: ...]
            // startup continuations as system reminders rather than user turns.
            let queued_messages = std::mem::take(&mut self.queued_messages);
            let hidden_reminders = std::mem::take(&mut self.hidden_queued_system_messages);
            let (messages, reminder, display_system_messages) =
                super::helpers::partition_queued_messages(queued_messages, hidden_reminders);
            let combined = messages.join("\n\n");

            for msg in display_system_messages {
                self.push_display_message(DisplayMessage::system(msg));
            }

            for msg in &messages {
                self.push_display_message(DisplayMessage::user(msg.clone()));
            }

            self.current_turn_system_reminder = reminder;

            if !combined.is_empty() {
                self.add_provider_message(Message::user(&combined));
                self.session.add_message(
                    Role::User,
                    vec![ContentBlock::Text {
                        text: combined,
                        cache_control: None,
                    }],
                );
            }
            let _ = self.session.save();
            self.clear_streaming_render_state();
            self.stream_buffer.clear();
            self.thought_line_inserted = false;
            self.thinking_prefix_emitted = false;
            self.thinking_buffer.clear();
            self.streaming_tool_calls.clear();
            self.streaming_input_tokens = 0;
            self.streaming_output_tokens = 0;
            self.streaming_cache_read_tokens = None;
            self.streaming_cache_creation_tokens = None;
            self.upstream_provider = None;
            self.streaming_tps_start = None;
            self.streaming_tps_elapsed = Duration::ZERO;
            self.streaming_total_output_tokens = 0;
            self.processing_started = Some(Instant::now());
            self.is_processing = true;
            self.status = ProcessingStatus::Sending;

            match self.run_turn_interactive(terminal, event_stream).await {
                Ok(()) => {
                    self.last_stream_error = None;
                }
                Err(e) => {
                    let err_str = crate::util::format_error_chain(&e);
                    if is_context_limit_error(&err_str) {
                        if self
                            .try_auto_compact_and_retry(terminal, event_stream)
                            .await
                        {
                            // Successfully recovered
                        } else {
                            self.handle_turn_error(err_str);
                        }
                    } else {
                        self.handle_turn_error(err_str);
                    }
                }
            }
            self.current_turn_system_reminder = None;
            // Loop will check if more messages were queued during this turn
        }
    }
}
