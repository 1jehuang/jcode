use super::TuiState;
use ratatui::layout::Rect;
#[cfg(test)]
use ratatui::text::Line;
use std::sync::Mutex;

struct StartupComposer {
    session_id: Option<String>,
    chat_area: Rect,
    input_y: u16,
    preview_started: bool,
    preview_finished: bool,
}

static STARTUP_COMPOSER: Mutex<Option<StartupComposer>> = Mutex::new(None);

pub(super) fn initial_screen(app: &dyn TuiState) -> bool {
    app.onboarding_preview_mode()
        || (app.display_messages().is_empty()
            && !app.is_processing()
            && app.streaming_text().is_empty())
}

/// Remember actual rendered geometry, including suggestions and any space the
/// idle animation borrowed. This is a viewport floor, never transcript padding.
pub(super) fn remember_startup_composer(app: &dyn TuiState, chat_area: Rect, input_y: u16) {
    *STARTUP_COMPOSER.lock().unwrap_or_else(|e| e.into_inner()) = Some(StartupComposer {
        session_id: app.current_session_id(),
        chat_area,
        input_y,
        preview_started: false,
        preview_finished: false,
    });
}

pub(super) fn startup_messages_floor(
    app: &dyn TuiState,
    chat_area: Rect,
    chrome_above_input: u16,
) -> Option<u16> {
    let mut state = STARTUP_COMPOSER.lock().unwrap_or_else(|e| e.into_inner());
    let saved = state.as_ref()?;
    // Never carry a welcome position into a different session or geometry.
    if saved.session_id != app.current_session_id() || saved.chat_area != chat_area {
        *state = None;
        return None;
    }
    Some(
        saved
            .input_y
            .saturating_sub(chat_area.y)
            .saturating_sub(chrome_above_input),
    )
}

pub(super) fn reset_startup_composer() {
    *STARTUP_COMPOSER.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

pub(super) fn first_prompt_preview() -> bool {
    let mut state = STARTUP_COMPOSER.lock().unwrap_or_else(|e| e.into_inner());
    let Some(saved) = state.as_mut() else {
        return false;
    };
    if saved.preview_finished {
        return false;
    }
    let snap = super::take_tail_follow_snap_request();
    if snap && saved.preview_started {
        // End / return-to-bottom after submission wins permanently. The first
        // snap belongs to submit itself and must not skip the prompt's start.
        saved.preview_finished = true;
        super::request_tail_follow_snap();
        return false;
    }
    saved.preview_started = true;
    true
}

pub(super) fn finish_first_prompt_preview() {
    if let Some(saved) = STARTUP_COMPOSER
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_mut()
    {
        saved.preview_finished = true;
    }
}

/// Hand manual navigation the on-screen position rather than the live tail.
pub(crate) fn take_first_prompt_preview_scroll() -> Option<usize> {
    let mut state = STARTUP_COMPOSER.lock().unwrap_or_else(|e| e.into_inner());
    let saved = state.as_mut()?;
    if !saved.preview_started || saved.preview_finished {
        return None;
    }
    saved.preview_finished = true;
    Some(super::last_resolved_chat_scroll())
}

#[cfg(test)]
pub(crate) fn inline_ui_gap_height(app: &dyn TuiState) -> u16 {
    if app.inline_ui_state().is_some() {
        1
    } else {
        0
    }
}

#[cfg(test)]
pub(crate) fn extract_line_text(line: &Line) -> String {
    line.spans.iter().map(|s| s.content.as_ref()).collect()
}
