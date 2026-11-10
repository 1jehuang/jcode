//! Key handling for the focused `ask_user` form.

use ratatui::crossterm::event::{KeyCode, KeyModifiers};

use crate::protocol::elicitation::ElicitationResponse;

use super::KeyOutcome;
use super::ViewContext;
use super::state::{EditorFocus, EditorKind, Form, MAX_TEXT_CODE_POINTS, SendTarget, State};

/// Key handling result, including a status notice for the caller to publish
/// after the state borrow is released.
pub(super) struct KeyResult {
    pub outcome: KeyOutcome,
    pub notice: Option<String>,
}

impl KeyResult {
    fn ignored() -> Self {
        Self {
            outcome: KeyOutcome::Ignored,
            notice: None,
        }
    }

    fn consumed() -> Self {
        Self {
            outcome: KeyOutcome::Consumed,
            notice: None,
        }
    }

    fn notified(notice: impl Into<String>) -> Self {
        Self {
            outcome: KeyOutcome::Consumed,
            notice: Some(notice.into()),
        }
    }

    fn send(form: &Form, response: ElicitationResponse) -> Self {
        Self {
            outcome: KeyOutcome::Send {
                session_id: form.session.clone(),
                elicitation_id: form.elicitation_id.clone(),
                response,
            },
            notice: None,
        }
    }
}

pub(super) fn handle_key(
    state: &mut State,
    context: &ViewContext,
    code: KeyCode,
    modifiers: KeyModifiers,
    text_input: Option<&str>,
) -> KeyResult {
    // Ctrl-modified keys are never consumed: Ctrl-C must reach the interrupt
    // handler even while the form covers the composer.
    if modifiers.contains(KeyModifiers::CONTROL) {
        return KeyResult::ignored();
    }
    if !context.eligible(state) || state.forms.is_empty() {
        return KeyResult::ignored();
    }

    let active = state.active;
    let Some(form) = state.forms.get_mut(active) else {
        return KeyResult::ignored();
    };

    // Repeat input while a response is outstanding is ignored, never sent
    // twice.
    if form.submit == super::state::SubmitState::Submitting {
        return KeyResult::consumed();
    }

    let result = if form.editor.is_some() {
        handle_editor(form, code, modifiers, text_input)
    } else {
        handle_normal(form, code, modifiers)
    };

    if let KeyOutcome::Send {
        session_id,
        elicitation_id,
        ..
    } = &result.outcome
    {
        state.last_sent = Some(SendTarget {
            session: session_id.clone(),
            elicitation_id: elicitation_id.clone(),
        });
    }
    result
}

fn handle_editor(
    form: &mut Form,
    code: KeyCode,
    modifiers: KeyModifiers,
    text_input: Option<&str>,
) -> KeyResult {
    let Some(editor) = form.editor else {
        return KeyResult::consumed();
    };
    if editor.question >= form.questions.len() {
        form.editor = None;
        return KeyResult::consumed();
    }

    let back = code == KeyCode::BackTab
        || (code == KeyCode::Tab && modifiers.contains(KeyModifiers::SHIFT));
    if back {
        form.editor = None;
        form.move_tab(false);
        return KeyResult::consumed();
    }
    if code == KeyCode::Tab {
        form.editor = None;
        form.move_tab(true);
        return KeyResult::consumed();
    }
    if matches!(code, KeyCode::Enter | KeyCode::Esc) {
        // Save and leave without declining.
        form.editor = None;
        return KeyResult::consumed();
    }

    let question = &mut form.questions[editor.question];
    let (text, cursor) = match editor.kind {
        EditorKind::Other => (&mut question.other_text, &mut question.other_cursor),
        EditorKind::Notes => (&mut question.notes, &mut question.notes_cursor),
    };
    let mut rejected = false;
    match code {
        KeyCode::Left => *cursor = previous_grapheme_boundary(text, *cursor),
        KeyCode::Right => *cursor = next_grapheme_boundary(text, *cursor),
        KeyCode::Home => *cursor = line_start(text, *cursor),
        KeyCode::End => *cursor = line_end(text, *cursor),
        KeyCode::Backspace => {
            let start = previous_grapheme_boundary(text, *cursor);
            if start < *cursor {
                text.replace_range(start..*cursor, "");
                *cursor = start;
            }
        }
        KeyCode::Delete => {
            let end = next_grapheme_boundary(text, *cursor);
            if end > *cursor {
                text.replace_range(*cursor..end, "");
            }
        }
        KeyCode::Char(_) => {
            let insert = text_input
                .map(str::to_string)
                .unwrap_or_else(|| match code {
                    KeyCode::Char(character) => character.to_string(),
                    _ => String::new(),
                });
            if !insert.is_empty() {
                rejected = !insert_text(text, cursor, &insert);
            }
        }
        _ => {}
    }

    if rejected {
        let notice = format!("Text limit is {MAX_TEXT_CODE_POINTS} characters; edit rejected.");
        form.notice = Some(notice.clone());
        return KeyResult::notified(notice);
    }
    if form
        .notice
        .as_deref()
        .is_some_and(|notice| notice.starts_with("Text limit"))
    {
        form.notice = None;
    }
    KeyResult::consumed()
}

fn handle_normal(form: &mut Form, code: KeyCode, modifiers: KeyModifiers) -> KeyResult {
    if form.is_submit_tab() {
        return handle_summary(form, code, modifiers);
    }

    match code {
        KeyCode::Left => {
            form.move_tab(false);
            KeyResult::consumed()
        }
        KeyCode::Right => {
            form.move_tab(true);
            KeyResult::consumed()
        }
        KeyCode::Tab => {
            form.move_tab(true);
            KeyResult::consumed()
        }
        KeyCode::BackTab => {
            form.move_tab(false);
            KeyResult::consumed()
        }
        KeyCode::Up => {
            move_highlight(form, false);
            KeyResult::consumed()
        }
        KeyCode::Down => {
            move_highlight(form, true);
            KeyResult::consumed()
        }
        KeyCode::PageUp => {
            scroll_body(form, false);
            KeyResult::consumed()
        }
        KeyCode::PageDown => {
            scroll_body(form, true);
            KeyResult::consumed()
        }
        KeyCode::Char(' ') => {
            let question = form.tab;
            choose_row(form, question, current_highlight(form));
            KeyResult::consumed()
        }
        KeyCode::Char(character @ '1'..='9') => {
            let row = character as usize - '1' as usize;
            let question = form.tab;
            if row
                < form
                    .questions
                    .get(question)
                    .map(super::state::Question::rows)
                    .unwrap_or(0)
            {
                if let Some(question_state) = form.questions.get_mut(question) {
                    question_state.highlight = row;
                }
                form.reveal_highlight = true;
                choose_row(form, question, row);
            }
            KeyResult::consumed()
        }
        KeyCode::Char('n') => {
            let question = form.tab;
            if form
                .questions
                .get(question)
                .is_some_and(|question| question.notes_field.is_some())
            {
                if let Some(question) = form.questions.get_mut(question) {
                    question.notes_cursor = question.notes.len();
                }
                form.editor = Some(EditorFocus {
                    question,
                    kind: EditorKind::Notes,
                });
                KeyResult::consumed()
            } else {
                KeyResult::notified("This question has no notes field.")
            }
        }
        KeyCode::Char('p') => {
            let question = form.tab;
            if form
                .questions
                .get(question)
                .and_then(super::state::Question::preview_for_highlight)
                .is_some()
            {
                form.preview_focus = !form.preview_focus;
                form.preview_scroll = 0;
                KeyResult::consumed()
            } else {
                KeyResult::notified("This option has no preview.")
            }
        }
        KeyCode::Enter => {
            let question = form.tab;
            let highlighted_other = form
                .questions
                .get(question)
                .is_some_and(super::state::Question::highlight_is_other);
            if highlighted_other {
                choose_row(form, question, current_highlight(form));
                return KeyResult::consumed();
            }
            let multi = form
                .questions
                .get(question)
                .is_some_and(|question| question.multi);
            if !multi {
                choose_row(form, question, current_highlight(form));
            }
            form.move_tab(true);
            KeyResult::consumed()
        }
        KeyCode::Esc => decline(form),
        KeyCode::Char(_) => {
            // Printable keys outside an editor are shortcuts only. Swallow
            // them so they cannot type into the composer hidden behind the
            // form.
            KeyResult::consumed()
        }
        _ => KeyResult::consumed(),
    }
}

fn handle_summary(form: &mut Form, code: KeyCode, modifiers: KeyModifiers) -> KeyResult {
    let submit = form.submit_tab();
    let back = code == KeyCode::BackTab
        || (code == KeyCode::Tab && modifiers.contains(KeyModifiers::SHIFT));
    match code {
        KeyCode::Up => {
            form.summary_index = form.summary_index.saturating_sub(1);
            KeyResult::consumed()
        }
        KeyCode::Down => {
            form.summary_index = (form.summary_index + 1).min(submit);
            KeyResult::consumed()
        }
        KeyCode::Left => {
            form.move_tab(false);
            KeyResult::consumed()
        }
        KeyCode::Right | KeyCode::Tab => {
            if back {
                form.move_tab(false);
            } else {
                form.move_tab(true);
            }
            KeyResult::consumed()
        }
        KeyCode::BackTab => {
            form.move_tab(false);
            KeyResult::consumed()
        }
        KeyCode::Enter => {
            if form.summary_index == submit {
                submit_form(form)
            } else {
                form.tab = form.summary_index;
                form.editor = None;
                form.preview_focus = false;
                KeyResult::consumed()
            }
        }
        KeyCode::Esc => decline(form),
        KeyCode::Char(_) => KeyResult::consumed(),
        _ => KeyResult::consumed(),
    }
}

fn current_highlight(form: &Form) -> usize {
    form.questions
        .get(form.tab)
        .map(|question| question.highlight)
        .unwrap_or(0)
}

fn move_highlight(form: &mut Form, forward: bool) {
    let Some(question) = form.current_question_mut() else {
        return;
    };
    let rows = question.rows();
    if rows == 0 {
        return;
    }
    let next = if forward {
        (question.highlight + 1).min(rows - 1)
    } else {
        question.highlight.saturating_sub(1)
    };
    if next != question.highlight {
        question.highlight = next;
        form.preview_scroll = 0;
        form.reveal_highlight = true;
    }
}

fn scroll_body(form: &mut Form, forward: bool) {
    let page = form.last_body_height.max(1);
    if form.preview_focus {
        let max = form
            .last_preview_lines
            .saturating_sub(form.last_body_height.max(1));
        form.preview_scroll = if forward {
            (form.preview_scroll + page).min(max)
        } else {
            form.preview_scroll.saturating_sub(page)
        };
    } else {
        let max = form
            .last_body_lines
            .saturating_sub(form.last_body_height.max(1));
        form.body_scroll = if forward {
            (form.body_scroll + page).min(max)
        } else {
            form.body_scroll.saturating_sub(page)
        };
    }
    form.reveal_highlight = false;
}

/// Select or toggle one option row. Opens the Other editor when that row was
/// chosen.
fn choose_row(form: &mut Form, question_index: usize, row: usize) {
    let Some(question) = form.questions.get_mut(question_index) else {
        return;
    };
    if row < question.options.len() {
        let value = question.options[row].value.clone();
        if question.multi {
            if !question.selected.remove(&value) {
                question.selected.insert(value);
            }
        } else {
            question.confirmed = Some(value);
        }
        return;
    }

    let Some(other) = question.other.clone() else {
        return;
    };
    let already_selected = question.multi && question.selected.contains(&other.value);
    if question.multi {
        if already_selected {
            question.selected.remove(&other.value);
        } else {
            question.selected.insert(other.value);
        }
    } else {
        question.confirmed = Some(other.value);
    }

    if already_selected {
        if form.editor.is_some_and(|editor| {
            editor.question == question_index && editor.kind == EditorKind::Other
        }) {
            form.editor = None;
        }
        return;
    }

    let cursor = form.questions[question_index].other_text.len();
    form.questions[question_index].other_cursor = cursor;
    form.editor = Some(EditorFocus {
        question: question_index,
        kind: EditorKind::Other,
    });
}

fn decline(form: &mut Form) -> KeyResult {
    form.mark_submitting();
    KeyResult::send(form, ElicitationResponse::Decline)
}

fn submit_form(form: &mut Form) -> KeyResult {
    if !form.all_answered() {
        let notice = "Answer every question before submitting.".to_string();
        form.notice = Some(notice.clone());
        return KeyResult::notified(notice);
    }
    match form.checked_content() {
        Ok(content) => {
            form.mark_submitting();
            KeyResult::send(
                form,
                ElicitationResponse::Accept {
                    content: Some(content),
                },
            )
        }
        Err(error) => {
            form.notice = Some(error.clone());
            KeyResult::notified(error)
        }
    }
}

/// Insert text at a byte cursor, rejecting (not truncating) anything that
/// would exceed the code-point limit.
pub(super) fn insert_text(text: &mut String, cursor: &mut usize, insert: &str) -> bool {
    let candidate_points = text.chars().count() + insert.chars().count();
    if candidate_points > MAX_TEXT_CODE_POINTS {
        return false;
    }
    *cursor = (*cursor).min(text.len());
    text.insert_str(*cursor, insert);
    *cursor += insert.len();
    true
}

fn previous_grapheme_boundary(text: &str, cursor: usize) -> usize {
    use unicode_segmentation::UnicodeSegmentation;
    let cursor = cursor.min(text.len());
    text[..cursor]
        .grapheme_indices(true)
        .next_back()
        .map(|(index, _)| index)
        .unwrap_or(0)
}

fn next_grapheme_boundary(text: &str, cursor: usize) -> usize {
    use unicode_segmentation::UnicodeSegmentation;
    let cursor = cursor.min(text.len());
    text[cursor..]
        .graphemes(true)
        .next()
        .map(|grapheme| cursor + grapheme.len())
        .unwrap_or(text.len())
}

fn line_start(text: &str, cursor: usize) -> usize {
    let cursor = cursor.min(text.len());
    text[..cursor]
        .rfind('\n')
        .map(|index| index + 1)
        .unwrap_or(0)
}

fn line_end(text: &str, cursor: usize) -> usize {
    let cursor = cursor.min(text.len());
    text[cursor..]
        .find('\n')
        .map(|offset| cursor + offset)
        .unwrap_or(text.len())
}
