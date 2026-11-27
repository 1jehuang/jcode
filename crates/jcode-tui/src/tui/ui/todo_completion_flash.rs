//! Brief text-color flash for a todo completion report when it first appears.
//!
//! Reports already in the transcript when the client starts are recorded as
//! seen without flashing, so only a report that arrives live gets the pulse.

use super::{DisplayMessage, prompt_entry_color, prompt_entry_shimmer_color};
use ratatui::prelude::*;
use std::collections::HashSet;
use unicode_width::UnicodeWidthStr;

pub(crate) const TODO_COMPLETION_FLASH_MS: u64 = 1200;
const TITLE: &str = "Todo completion";

#[derive(Default)]
struct FlashState {
    initialized: bool,
    seen: HashSet<u64>,
    active: Option<(u64, u64)>,
}

fn with_state<R>(f: impl FnOnce(&mut FlashState) -> R) -> R {
    #[cfg(test)]
    {
        thread_local! {
            static STATE: std::cell::RefCell<FlashState> = Default::default();
        }
        STATE.with(|state| f(&mut state.borrow_mut()))
    }
    #[cfg(not(test))]
    {
        static STATE: std::sync::OnceLock<std::sync::Mutex<FlashState>> =
            std::sync::OnceLock::new();
        let mut guard = STATE
            .get_or_init(Default::default)
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        f(&mut guard)
    }
}

/// Record completion reports in `messages` and start a flash for a new one.
/// Returns the index of the flashing message and its progress in `0..=1`.
pub(crate) fn update(messages: &[DisplayMessage], now_ms: u64) -> Option<(usize, f32)> {
    with_state(|state| {
        let first_frame = !state.initialized;
        state.initialized = true;
        for msg in messages
            .iter()
            .filter(|msg| msg.title.as_deref() == Some(TITLE))
        {
            let hash = msg.stable_cache_hash();
            if state.seen.insert(hash) && !first_frame {
                state.active = Some((hash, now_ms));
            }
        }
        let (hash, start) = state.active?;
        let elapsed = now_ms.saturating_sub(start);
        if elapsed > TODO_COMPLETION_FLASH_MS {
            state.active = None;
            return None;
        }
        let index = messages.iter().rposition(|msg| {
            msg.title.as_deref() == Some(TITLE) && msg.stable_cache_hash() == hash
        })?;
        Some((index, elapsed as f32 / TODO_COMPLETION_FLASH_MS as f32))
    })
}

/// Whether a flash is still running, so the redraw loop keeps ticking.
pub(crate) fn active(now_ms: u64) -> bool {
    with_state(|state| {
        state
            .active
            .is_some_and(|(_, start)| now_ms.saturating_sub(start) <= TODO_COMPLETION_FLASH_MS)
    })
}

/// Pulse the foreground of `line` toward a warm highlight with a sweeping
/// shimmer, the same treatment prompts get when scrolled into view. Leaves
/// backgrounds untouched.
pub(crate) fn apply(line: &mut Line<'static>, t: f32) {
    let line_width = line.width().max(1) as f32;
    let mut consumed = 0usize;
    for span in &mut line.spans {
        let span_width = span.content.as_ref().width();
        if span_width == 0 {
            continue;
        }
        let center = ((consumed as f32 + span_width as f32 * 0.5) / line_width).clamp(0.0, 1.0);
        let base = span.style.fg.unwrap_or(super::ai_text());
        let fg = prompt_entry_shimmer_color(prompt_entry_color(base, t), center, t);
        span.style = span.style.fg(fg);
        consumed += span_width;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(content: &str) -> DisplayMessage {
        let mut msg = DisplayMessage::system(content.to_owned());
        msg.title = Some(TITLE.into());
        msg
    }

    #[test]
    fn flashes_only_new_reports_and_expires() {
        let existing = report("old");
        let mut messages = vec![existing];
        assert!(update(&messages, 1000).is_none(), "history must not flash");
        messages.push(DisplayMessage::system("other".to_owned()));
        messages.push(report("new"));
        let (index, t) = update(&messages, 2000).expect("new report flashes");
        assert_eq!(index, 2);
        assert_eq!(t, 0.0);
        assert!(active(2000 + TODO_COMPLETION_FLASH_MS));
        assert!(update(&messages, 2000 + TODO_COMPLETION_FLASH_MS + 1).is_none());
        assert!(!active(2000 + TODO_COMPLETION_FLASH_MS + 1));
    }

    #[test]
    fn apply_changes_text_color_but_not_background() {
        let bg = Color::Rgb(1, 2, 3);
        let mut line = Line::from(Span::styled(
            "Todo completion",
            Style::default().fg(Color::Rgb(10, 10, 10)).bg(bg),
        ));
        apply(&mut line, 0.5);
        assert_ne!(line.spans[0].style.fg, Some(Color::Rgb(10, 10, 10)));
        assert_eq!(line.spans[0].style.bg, Some(bg));
    }
}
