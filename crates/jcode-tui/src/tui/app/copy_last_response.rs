//! Copy the last assistant answer through the existing clipboard path (#1929).

use super::App;
use crate::tui::DisplayMessageRoleExt;
use crate::tui::keybind::SequenceMatch;
use crossterm::event::{KeyCode, KeyModifiers};
use jcode_tui_messages::DisplayMessage;

const NO_ANSWER_NOTICE: &str = "No answer to copy yet";

/// Drop sentinel-wrapped reasoning lines and keep remaining bytes as-is,
/// including blank lines and trailing whitespace in the answer itself.
fn strip_reasoning_for_copy(content: &str) -> String {
    let sentinel = jcode_tui_markdown::REASONING_SENTINEL;
    if !content.contains(sentinel) {
        return content.to_string();
    }
    let mut out = String::with_capacity(content.len());
    let mut wrote = false;
    for line in content.split('\n') {
        if line.contains(sentinel) {
            continue;
        }
        if wrote {
            out.push('\n');
        }
        wrote = true;
        out.push_str(line);
    }
    out
}

/// Markdown text of the latest assistant message, as the model wrote it.
///
/// Skips reasoning (sentinel-wrapped lines and dedicated reasoning messages)
/// and tool output. Empty assistant turns are ignored so a collapsed
/// reasoning-only message does not hide an earlier real answer.
pub(super) fn last_assistant_answer_text(messages: &[DisplayMessage]) -> Option<String> {
    messages.iter().rev().find_map(|msg| {
        if msg.effective_role() != "assistant" {
            return None;
        }
        let text = strip_reasoning_for_copy(&msg.content);
        if text.trim().is_empty() {
            None
        } else {
            Some(text)
        }
    })
}

pub(super) fn copied_last_answer_notice(chars: usize) -> String {
    format!(
        "Copied the last answer ({} characters)",
        jcode_core::util::format_number(chars)
    )
}

pub(super) fn copy_last_answer(app: &mut App) {
    copy_last_answer_with(app, super::helpers::copy_to_clipboard);
}

fn copy_last_answer_with(app: &mut App, mut copy_text: impl FnMut(&str) -> bool) {
    let Some(text) = last_assistant_answer_text(&app.display_messages) else {
        app.set_status_notice(NO_ANSWER_NOTICE);
        return;
    };
    if copy_text(&text) {
        app.set_status_notice(copied_last_answer_notice(text.chars().count()));
    } else {
        app.set_status_notice("Failed to copy the last answer");
    }
}

pub(crate) fn handle_key(app: &mut App, code: KeyCode, modifiers: KeyModifiers) -> bool {
    match app.toggle_keys.copy_last_response.feed(code, modifiers) {
        SequenceMatch::Complete => {
            copy_last_answer(app);
            true
        }
        SequenceMatch::Prefix => true,
        SequenceMatch::None => false,
    }
}

/// Feed the copy sequence only when a prefix is already pending.
///
/// Fallback/merge offers share Ctrl+Y with the default copy chord. A started
/// sequence (Ctrl+X already fed) must complete the copy instead of accepting
/// the offer. Standalone Ctrl+Y still accepts an offer when nothing is pending.
pub(crate) fn handle_key_if_pending(app: &mut App, code: KeyCode, modifiers: KeyModifiers) -> bool {
    if !app.toggle_keys.copy_last_response.is_pending() {
        return false;
    }
    handle_key(app, code, modifiers)
}
