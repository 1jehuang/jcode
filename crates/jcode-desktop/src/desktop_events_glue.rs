use super::*;

pub(crate) fn desktop_key_event_to_key_input(event: &DesktopKeyEvent) -> KeyInput {
    let modifiers = desktop_key_modifiers_to_winit(event.modifiers);
    let key = desktop_key_string_to_winit_key(&event.key, event.text.as_deref());
    to_key_input(&key, modifiers)
}

pub(crate) fn desktop_key_modifiers_to_winit(modifiers: DesktopKeyModifiers) -> ModifiersState {
    let mut state = ModifiersState::empty();
    if modifiers.shift {
        state |= ModifiersState::SHIFT;
    }
    if modifiers.ctrl {
        state |= ModifiersState::CONTROL;
    }
    if modifiers.alt {
        state |= ModifiersState::ALT;
    }
    if modifiers.super_key {
        state |= ModifiersState::SUPER;
    }
    state
}

pub(crate) fn desktop_key_string_to_winit_key(key: &str, text: Option<&str>) -> Key {
    match key {
        "Escape" => Key::Named(NamedKey::Escape),
        "Enter" => Key::Named(NamedKey::Enter),
        "Tab" => Key::Named(NamedKey::Tab),
        "Backspace" => Key::Named(NamedKey::Backspace),
        "Delete" => Key::Named(NamedKey::Delete),
        "PageUp" => Key::Named(NamedKey::PageUp),
        "PageDown" => Key::Named(NamedKey::PageDown),
        "ArrowUp" => Key::Named(NamedKey::ArrowUp),
        "ArrowDown" => Key::Named(NamedKey::ArrowDown),
        "ArrowLeft" => Key::Named(NamedKey::ArrowLeft),
        "ArrowRight" => Key::Named(NamedKey::ArrowRight),
        "Home" => Key::Named(NamedKey::Home),
        "End" => Key::Named(NamedKey::End),
        "Space" => Key::Named(NamedKey::Space),
        _ => Key::Character(text.unwrap_or(key).to_string().into()),
    }
}

pub(crate) fn desktop_wire_session_event_to_runtime_event(
    event: DesktopSessionEventWire,
) -> Option<session_launch::DesktopSessionEvent> {
    match event {
        DesktopSessionEventWire::Status { message } => Some(
            session_launch::DesktopSessionEvent::Status(DesktopSessionStatus::external(message)),
        ),
        DesktopSessionEventWire::AssistantTextDelta { text } => {
            Some(session_launch::DesktopSessionEvent::TextDelta(text))
        }
        DesktopSessionEventWire::ReasoningDelta { text } => {
            Some(session_launch::DesktopSessionEvent::ReasoningDelta(text))
        }
        DesktopSessionEventWire::ReasoningDone { duration_ms } => {
            Some(session_launch::DesktopSessionEvent::ReasoningDone { duration_ms })
        }
        DesktopSessionEventWire::ToolStarted {
            id,
            title,
            executing,
        } => {
            let id = (!id.is_empty()).then_some(id);
            Some(if executing {
                session_launch::DesktopSessionEvent::ToolExecuting { id, name: title }
            } else {
                session_launch::DesktopSessionEvent::ToolStarted { id, name: title }
            })
        }
        DesktopSessionEventWire::ToolInput { id, delta } => {
            Some(session_launch::DesktopSessionEvent::ToolInput {
                id: (!id.is_empty()).then_some(id),
                delta,
            })
        }
        DesktopSessionEventWire::ToolFinished { id, title, success } => {
            Some(session_launch::DesktopSessionEvent::ToolFinished {
                id: (!id.is_empty()).then_some(id),
                name: title,
                summary: String::new(),
                is_error: !success,
            })
        }
        DesktopSessionEventWire::Error { message } => {
            Some(session_launch::DesktopSessionEvent::Error(message))
        }
        DesktopSessionEventWire::RawJson { .. } => None,
    }
}

pub(crate) fn desktop_key_event_from_winit(
    key: &Key,
    modifiers: ModifiersState,
    pressed: bool,
) -> DesktopKeyEvent {
    DesktopKeyEvent {
        key: desktop_key_name(key),
        text: desktop_key_text(key),
        pressed,
        modifiers: desktop_key_modifiers(modifiers),
    }
}

pub(crate) fn desktop_key_name(key: &Key) -> String {
    match key {
        Key::Character(value) => value.to_string(),
        Key::Named(named) => format!("{named:?}"),
        other => format!("{other:?}"),
    }
}

pub(crate) fn desktop_key_text(key: &Key) -> Option<String> {
    match key {
        Key::Character(value) => Some(value.to_string()),
        _ => None,
    }
}

pub(crate) fn desktop_key_modifiers(modifiers: ModifiersState) -> DesktopKeyModifiers {
    DesktopKeyModifiers {
        shift: modifiers.shift_key(),
        ctrl: modifiers.control_key(),
        alt: modifiers.alt_key(),
        super_key: modifiers.super_key(),
    }
}

pub(crate) fn desktop_mouse_wheel_event(delta: MouseScrollDelta) -> DesktopMouseEvent {
    let (delta_x, delta_y) = match delta {
        MouseScrollDelta::LineDelta(x, y) => (x, y),
        MouseScrollDelta::PixelDelta(position) => (position.x as f32, position.y as f32),
    };
    DesktopMouseEvent::Wheel { delta_x, delta_y }
}

pub(crate) fn forward_app_worker_input(
    hot_reloader: &mut DesktopHotReloader,
    input: DesktopInputEvent,
) {
    if let Err(error) = hot_reloader.send_app_worker_input(input) {
        desktop_log::error(format_args!(
            "jcode-desktop: failed to forward input to app worker: {error:#}"
        ));
    }
}

pub(crate) fn forward_desktop_session_event_batch_to_worker(
    hot_reloader: &mut DesktopHotReloader,
    batch: &DesktopSessionEventBatch,
) {
    if !hot_reloader.has_app_worker() {
        return;
    }
    let wire = DesktopSessionEventBatchWire {
        events: batch
            .events
            .iter()
            .map(desktop_session_event_to_wire)
            .collect(),
        raw_event_count: batch.raw_event_count,
        raw_payload_bytes: batch.raw_payload_bytes,
    };
    if let Err(error) =
        hot_reloader.send_app_worker_message(DesktopHostToWorkerMessage::SessionEvents(wire))
    {
        desktop_log::error(format_args!(
            "jcode-desktop: failed to forward session events to app worker: {error:#}"
        ));
    }
}

pub(crate) fn desktop_session_event_to_wire(
    event: &session_launch::DesktopSessionEvent,
) -> DesktopSessionEventWire {
    match event {
        session_launch::DesktopSessionEvent::Status(status) => DesktopSessionEventWire::Status {
            message: status.label(),
        },
        session_launch::DesktopSessionEvent::TextDelta(text)
        | session_launch::DesktopSessionEvent::TextReplace(text) => {
            DesktopSessionEventWire::AssistantTextDelta { text: text.clone() }
        }
        session_launch::DesktopSessionEvent::ReasoningDelta(text) => {
            DesktopSessionEventWire::ReasoningDelta { text: text.clone() }
        }
        session_launch::DesktopSessionEvent::ReasoningDone { duration_ms } => {
            DesktopSessionEventWire::ReasoningDone {
                duration_ms: *duration_ms,
            }
        }
        session_launch::DesktopSessionEvent::ToolStarted { id, name } => {
            DesktopSessionEventWire::ToolStarted {
                id: id.clone().unwrap_or_default(),
                title: name.clone(),
                executing: false,
            }
        }
        session_launch::DesktopSessionEvent::ToolExecuting { id, name } => {
            DesktopSessionEventWire::ToolStarted {
                id: id.clone().unwrap_or_default(),
                title: name.clone(),
                executing: true,
            }
        }
        session_launch::DesktopSessionEvent::ToolInput { id, delta } => {
            DesktopSessionEventWire::ToolInput {
                id: id.clone().unwrap_or_default(),
                delta: delta.clone(),
            }
        }
        session_launch::DesktopSessionEvent::ToolFinished {
            id, name, is_error, ..
        } => DesktopSessionEventWire::ToolFinished {
            id: id.clone().unwrap_or_default(),
            title: name.clone(),
            success: !*is_error,
        },
        session_launch::DesktopSessionEvent::Error(message) => DesktopSessionEventWire::Error {
            message: message.clone(),
        },
        other => DesktopSessionEventWire::RawJson {
            event_type: desktop_session_event_type_name(other).to_string(),
            payload: format!("{other:?}"),
        },
    }
}

pub(crate) fn desktop_session_event_type_name(
    event: &session_launch::DesktopSessionEvent,
) -> &'static str {
    match event {
        session_launch::DesktopSessionEvent::Status(_) => "status",
        session_launch::DesktopSessionEvent::SessionStarted { .. } => "session_started",
        session_launch::DesktopSessionEvent::SessionRenamed { .. } => "session_renamed",
        session_launch::DesktopSessionEvent::TextDelta(_) => "text_delta",
        session_launch::DesktopSessionEvent::TextReplace(_) => "text_replace",
        session_launch::DesktopSessionEvent::ReasoningDelta(_) => "reasoning_delta",
        session_launch::DesktopSessionEvent::ReasoningDone { .. } => "reasoning_done",
        session_launch::DesktopSessionEvent::ToolStarted { .. } => "tool_started",
        session_launch::DesktopSessionEvent::ToolExecuting { .. } => "tool_executing",
        session_launch::DesktopSessionEvent::ToolInput { .. } => "tool_input",
        session_launch::DesktopSessionEvent::ToolFinished { .. } => "tool_finished",
        session_launch::DesktopSessionEvent::ModelChanged { .. } => "model_changed",
        session_launch::DesktopSessionEvent::ModelCatalog { .. } => "model_catalog",
        session_launch::DesktopSessionEvent::ModelCatalogError { .. } => "model_catalog_error",
        session_launch::DesktopSessionEvent::StdinRequest { .. } => "stdin_request",
        session_launch::DesktopSessionEvent::ReloadProgress { .. } => "reload_progress",
        session_launch::DesktopSessionEvent::RuntimeMetadata { .. } => "runtime_metadata",
        session_launch::DesktopSessionEvent::TokenUsage { .. } => "token_usage",
        session_launch::DesktopSessionEvent::SystemNotice { .. } => "system_notice",
        session_launch::DesktopSessionEvent::SessionCloseRequested { .. } => {
            "session_close_requested"
        }
        session_launch::DesktopSessionEvent::Reloading { .. } => "reloading",
        session_launch::DesktopSessionEvent::Reloaded { .. } => "reloaded",
        session_launch::DesktopSessionEvent::Done => "done",
        session_launch::DesktopSessionEvent::Error(_) => "error",
    }
}

pub(crate) fn to_key_input(key: &Key, modifiers: ModifiersState) -> KeyInput {
    match key {
        Key::Named(NamedKey::Escape) => KeyInput::Escape,
        Key::Named(NamedKey::Space) => KeyInput::Character(" ".to_string()),
        Key::Named(NamedKey::Copy) => KeyInput::CopyLatestResponse,
        Key::Named(NamedKey::Cut) => KeyInput::CutInputLine,
        Key::Named(NamedKey::Paste) => KeyInput::PasteText,
        Key::Named(NamedKey::Undo) => KeyInput::UndoInput,
        Key::Named(NamedKey::Enter) if modifiers.control_key() => KeyInput::QueueDraft,
        Key::Named(NamedKey::Enter) if modifiers.shift_key() || modifiers.alt_key() => {
            KeyInput::Enter
        }
        Key::Named(NamedKey::Enter) => KeyInput::SubmitDraft,
        Key::Named(NamedKey::Tab) if modifiers.control_key() && modifiers.shift_key() => {
            KeyInput::CycleModel(-1)
        }
        Key::Named(NamedKey::Tab) if modifiers.control_key() => KeyInput::CycleModel(1),
        Key::Named(NamedKey::Tab) => KeyInput::Autocomplete,
        Key::Named(NamedKey::Backspace)
            if modifiers.control_key() || modifiers.alt_key() || modifiers.super_key() =>
        {
            KeyInput::DeletePreviousWord
        }
        Key::Named(NamedKey::Backspace) => KeyInput::Backspace,
        Key::Named(NamedKey::Delete) => KeyInput::DeleteNextChar,
        Key::Named(NamedKey::PageUp) => KeyInput::ScrollBodyPages(1),
        Key::Named(NamedKey::PageDown) => KeyInput::ScrollBodyPages(-1),
        Key::Named(NamedKey::ArrowUp) if modifiers.control_key() => KeyInput::RetrieveQueuedDraft,
        Key::Named(NamedKey::ArrowUp) if modifiers.alt_key() => KeyInput::JumpPrompt(-1),
        Key::Named(NamedKey::ArrowDown) if modifiers.alt_key() => KeyInput::JumpPrompt(1),
        Key::Named(NamedKey::ArrowUp) => KeyInput::ModelPickerMove(-1),
        Key::Named(NamedKey::ArrowDown) => KeyInput::ModelPickerMove(1),
        Key::Named(NamedKey::ArrowLeft) if modifiers.alt_key() => {
            KeyInput::CycleReasoningEffort(-1)
        }
        Key::Named(NamedKey::ArrowRight) if modifiers.alt_key() => {
            KeyInput::CycleReasoningEffort(1)
        }
        Key::Named(NamedKey::ArrowLeft) if modifiers.control_key() => KeyInput::MoveCursorWordLeft,
        Key::Named(NamedKey::ArrowRight) if modifiers.control_key() => {
            KeyInput::MoveCursorWordRight
        }
        Key::Named(NamedKey::ArrowLeft) => KeyInput::MoveCursorLeft,
        Key::Named(NamedKey::ArrowRight) => KeyInput::MoveCursorRight,
        Key::Named(NamedKey::Home) if modifiers.control_key() => KeyInput::ScrollBodyToTop,
        Key::Named(NamedKey::End) if modifiers.control_key() => KeyInput::ScrollBodyToBottom,
        Key::Named(NamedKey::Home) => KeyInput::MoveToLineStart,
        Key::Named(NamedKey::End) => KeyInput::MoveToLineEnd,
        Key::Character(text) if modifiers.control_key() && text.eq_ignore_ascii_case("a") => {
            KeyInput::MoveToLineStart
        }
        Key::Character(text) if modifiers.control_key() && text.eq_ignore_ascii_case("e") => {
            KeyInput::MoveToLineEnd
        }
        Key::Character(text) if modifiers.control_key() && text.eq_ignore_ascii_case("b") => {
            KeyInput::MoveCursorWordLeft
        }
        Key::Character(text) if modifiers.control_key() && text.eq_ignore_ascii_case("f") => {
            KeyInput::MoveCursorWordRight
        }
        Key::Character(text) if modifiers.control_key() && text.eq_ignore_ascii_case("u") => {
            KeyInput::DeleteToLineStart
        }
        Key::Character(text)
            if desktop_clipboard_shortcut_modifier(modifiers)
                && modifiers.shift_key()
                && text.eq_ignore_ascii_case("k") =>
        {
            KeyInput::CopyLatestCodeBlock
        }
        Key::Character(text) if modifiers.control_key() && text.eq_ignore_ascii_case("k") => {
            KeyInput::DeleteToLineEnd
        }
        Key::Character(text) if modifiers.control_key() && text.eq_ignore_ascii_case("w") => {
            KeyInput::DeletePreviousWord
        }
        Key::Character(text)
            if desktop_clipboard_shortcut_modifier(modifiers) && text.eq_ignore_ascii_case("x") =>
        {
            KeyInput::CutInputLine
        }
        Key::Character(text)
            if desktop_clipboard_shortcut_modifier(modifiers) && text.eq_ignore_ascii_case("z") =>
        {
            KeyInput::UndoInput
        }
        Key::Character(text)
            if desktop_clipboard_shortcut_modifier(modifiers)
                && modifiers.shift_key()
                && text.eq_ignore_ascii_case("c") =>
        {
            KeyInput::CopyLatestResponse
        }
        Key::Character(text)
            if desktop_clipboard_shortcut_modifier(modifiers)
                && modifiers.shift_key()
                && text.eq_ignore_ascii_case("t") =>
        {
            KeyInput::CopyTranscript
        }
        Key::Character(text)
            if modifiers.control_key()
                && (text.eq_ignore_ascii_case("c") || text.eq_ignore_ascii_case("d")) =>
        {
            KeyInput::CancelGeneration
        }
        Key::Character(text) if modifiers.super_key() && text.eq_ignore_ascii_case("c") => {
            KeyInput::CopyLatestResponse
        }
        Key::Character(text) if modifiers.alt_key() && text.eq_ignore_ascii_case("b") => {
            KeyInput::MoveCursorWordLeft
        }
        Key::Character(text) if modifiers.alt_key() && text.eq_ignore_ascii_case("f") => {
            KeyInput::MoveCursorWordRight
        }
        Key::Character(text) if modifiers.alt_key() && text.eq_ignore_ascii_case("d") => {
            KeyInput::DeleteNextWord
        }
        Key::Character(text) if modifiers.alt_key() && text.eq_ignore_ascii_case("v") => {
            KeyInput::PasteText
        }
        Key::Character(text) if modifiers.control_key() && text == "[" => KeyInput::JumpPrompt(-1),
        Key::Character(text) if modifiers.control_key() && text == "]" => KeyInput::JumpPrompt(1),
        Key::Character(text) if modifiers.super_key() && text.eq_ignore_ascii_case("k") => {
            KeyInput::JumpPrompt(-1)
        }
        Key::Character(text) if modifiers.super_key() && text.eq_ignore_ascii_case("j") => {
            KeyInput::JumpPrompt(1)
        }
        Key::Character(text)
            if (modifiers.control_key() || modifiers.super_key())
                && text.eq_ignore_ascii_case("q") =>
        {
            KeyInput::ExitApp
        }
        Key::Character(text) if modifiers.super_key() && text == ";" => {
            KeyInput::SpawnSelfDevSession
        }
        Key::Character(text) if modifiers.super_key() && text == "'" => KeyInput::SpawnHomeSession,
        Key::Character(text) if modifiers.control_key() && text == ";" => KeyInput::SpawnPanel,
        Key::Character(text) if modifiers.control_key() && (text == "?" || text == "/") => {
            KeyInput::HotkeyHelp
        }
        Key::Character(text)
            if modifiers.control_key()
                && modifiers.shift_key()
                && text.eq_ignore_ascii_case("s") =>
        {
            KeyInput::ToggleSessionInfo
        }
        Key::Character(text)
            if modifiers.control_key()
                && (text.eq_ignore_ascii_case("p") || text.eq_ignore_ascii_case("o")) =>
        {
            KeyInput::OpenSessionSwitcher
        }
        Key::Character(text) if modifiers.control_key() && text.eq_ignore_ascii_case("r") => {
            KeyInput::RefreshSessions
        }
        Key::Character(text) if modifiers.control_key() && (text == "-" || text == "_") => {
            KeyInput::AdjustTextScale(-1)
        }
        Key::Character(text) if modifiers.control_key() && (text == "=" || text == "+") => {
            KeyInput::AdjustTextScale(1)
        }
        Key::Character(text) if modifiers.control_key() && text == "0" => KeyInput::ResetTextScale,
        Key::Character(text)
            if desktop_clipboard_shortcut_modifier(modifiers) && text.eq_ignore_ascii_case("v") =>
        {
            KeyInput::PasteText
        }
        Key::Character(text)
            if modifiers.control_key()
                && modifiers.shift_key()
                && text.eq_ignore_ascii_case("i") =>
        {
            KeyInput::ClearAttachedImages
        }
        Key::Character(text) if modifiers.control_key() && text.eq_ignore_ascii_case("i") => {
            KeyInput::AttachClipboardImage
        }
        Key::Character(text)
            if modifiers.control_key()
                && modifiers.shift_key()
                && text.eq_ignore_ascii_case("m") =>
        {
            KeyInput::OpenModelPicker
        }
        Key::Character(text) if modifiers.control_key() && text.eq_ignore_ascii_case("m") => {
            KeyInput::CycleModel(1)
        }
        Key::Character(text) if modifiers.control_key() && text.eq_ignore_ascii_case("n") => {
            KeyInput::CycleModel(-1)
        }
        Key::Character(text) if modifiers.control_key() && text == "1" => {
            KeyInput::SetPanelSize(PanelSizePreset::Quarter)
        }
        Key::Character(text) if modifiers.control_key() && text == "2" => {
            KeyInput::SetPanelSize(PanelSizePreset::Half)
        }
        Key::Character(text) if modifiers.control_key() && text == "3" => {
            KeyInput::SetPanelSize(PanelSizePreset::ThreeQuarter)
        }
        Key::Character(text) if modifiers.control_key() && text == "4" => {
            KeyInput::SetPanelSize(PanelSizePreset::Full)
        }
        Key::Character(_)
            if modifiers.control_key() || modifiers.alt_key() || modifiers.super_key() =>
        {
            KeyInput::Other
        }
        Key::Character(text) => KeyInput::Character(text.to_string()),
        _ => KeyInput::Other,
    }
}

pub(crate) fn desktop_clipboard_shortcut_modifier(modifiers: ModifiersState) -> bool {
    modifiers.control_key() || modifiers.alt_key() || modifiers.super_key()
}

pub(crate) fn is_space_key(key: &Key) -> bool {
    matches!(key, Key::Named(NamedKey::Space)) || matches!(key, Key::Character(text) if text == " ")
}

pub(crate) fn workspace_space_hold_progress(
    app: &DesktopApp,
    started_at: Option<Instant>,
    consumed: bool,
) -> Option<f32> {
    let DesktopApp::Workspace(workspace) = app else {
        return None;
    };
    let started_at = started_at?;
    if consumed {
        return None;
    }
    let threshold = workspace.space_hold_toggle_duration();
    if threshold.is_zero() {
        return Some(1.0);
    }
    Some(
        (Instant::now()
            .saturating_duration_since(started_at)
            .as_secs_f32()
            / threshold.as_secs_f32())
        .clamp(0.0, 1.0),
    )
}

pub(crate) fn apply_desktop_session_event_batch(
    app: &mut DesktopApp,
    events: Vec<session_launch::DesktopSessionEvent>,
) -> bool {
    apply_desktop_session_event_batch_with_stats(app, events).visible_changed
}

#[derive(Debug, Clone)]
pub(crate) struct DesktopSessionApplyStats {
    pub(crate) visible_changed: bool,
    pub(crate) event_count: usize,
    pub(crate) text_delta_bytes: usize,
    pub(crate) session_card_refresh_requested: bool,
    pub(crate) elapsed: Duration,
}

pub(crate) fn apply_desktop_session_event_batch_with_stats(
    app: &mut DesktopApp,
    events: Vec<session_launch::DesktopSessionEvent>,
) -> DesktopSessionApplyStats {
    if events.is_empty() {
        return DesktopSessionApplyStats {
            visible_changed: false,
            event_count: 0,
            text_delta_bytes: 0,
            session_card_refresh_requested: false,
            elapsed: Duration::ZERO,
        };
    }
    let started = Instant::now();
    let event_count = events.len();
    let mut text_delta_bytes = 0usize;
    let mut visible_changed = false;
    let mut session_card_refresh_requested = false;
    for event in events {
        log_desktop_session_event_error(&event);
        if let session_launch::DesktopSessionEvent::TextDelta(text) = &event {
            text_delta_bytes += text.len();
        }
        session_card_refresh_requested |= desktop_session_event_refreshes_session_card(&event);
        visible_changed |= desktop_session_event_affects_visible_state(&event);
        app.apply_session_event(event);
    }
    let elapsed = started.elapsed();
    log_desktop_slow_interaction(
        "session_event_apply",
        elapsed,
        serde_json::json!({
            "events": event_count,
            "text_delta_bytes": text_delta_bytes,
        }),
    );
    DesktopSessionApplyStats {
        visible_changed,
        event_count,
        text_delta_bytes,
        session_card_refresh_requested,
        elapsed,
    }
}

pub(crate) fn log_desktop_session_event_error(event: &session_launch::DesktopSessionEvent) {
    match event {
        session_launch::DesktopSessionEvent::Error(error) => {
            desktop_log::error(format_args!(
                "jcode-desktop: session error event: {}",
                desktop_log::truncate_for_log(error, 2048)
            ));
        }
        session_launch::DesktopSessionEvent::ModelCatalogError { error } => {
            desktop_log::error(format_args!(
                "jcode-desktop: model catalog error event: {}",
                desktop_log::truncate_for_log(error, 2048)
            ));
        }
        session_launch::DesktopSessionEvent::ModelChanged {
            model,
            provider_name,
            error: Some(error),
        } => {
            desktop_log::error(format_args!(
                "jcode-desktop: model switch failed model={} provider={} error={}",
                desktop_log::truncate_for_log(model, 256),
                provider_name
                    .as_deref()
                    .map(|provider| desktop_log::truncate_for_log(provider, 256))
                    .unwrap_or_else(|| "<unknown>".to_string()),
                desktop_log::truncate_for_log(error, 2048)
            ));
        }
        session_launch::DesktopSessionEvent::ToolFinished {
            id: _,
            name,
            summary,
            is_error: true,
        } => {
            desktop_log::warn(format_args!(
                "jcode-desktop: tool failed name={} summary={}",
                desktop_log::truncate_for_log(name, 256),
                desktop_log::truncate_for_log(summary, 2048)
            ));
        }
        _ => {}
    }
}

pub(crate) fn desktop_session_event_refreshes_session_card(
    event: &session_launch::DesktopSessionEvent,
) -> bool {
    matches!(
        event,
        session_launch::DesktopSessionEvent::SessionStarted { .. }
            | session_launch::DesktopSessionEvent::SessionRenamed { .. }
            | session_launch::DesktopSessionEvent::Reloaded { .. }
            | session_launch::DesktopSessionEvent::Done
            | session_launch::DesktopSessionEvent::Error(_)
    )
}

pub(crate) fn desktop_session_event_affects_visible_state(
    event: &session_launch::DesktopSessionEvent,
) -> bool {
    !matches!(event, session_launch::DesktopSessionEvent::ToolInput { .. })
}

#[cfg(test)]
pub(crate) fn apply_pending_session_events(
    app: &mut DesktopApp,
    session_event_rx: &mpsc::Receiver<session_launch::DesktopSessionEvent>,
) -> bool {
    let mut events = Vec::new();
    while let Ok(event) = session_event_rx.try_recv() {
        events.push(event);
    }
    apply_desktop_session_event_batch(app, events)
}
