//! Paste routing for the focused `ask_user` form.

use super::ViewContext;
use super::keys::insert_text;
use super::state::{EditorFocus, EditorKind, Form, MAX_TEXT_CODE_POINTS, State};

pub(super) struct PasteResult {
    pub consumed: bool,
    pub notice: Option<String>,
}

impl PasteResult {
    fn ignored() -> Self {
        Self {
            consumed: false,
            notice: None,
        }
    }

    fn consumed() -> Self {
        Self {
            consumed: true,
            notice: None,
        }
    }

    fn rejected() -> Self {
        Self {
            consumed: true,
            notice: Some(format!(
                "Text limit is {MAX_TEXT_CODE_POINTS} characters; paste rejected."
            )),
        }
    }
}

/// True when the paste was consumed by a focused form editor (or opened the
/// selected Other editor). When the form has no text target this returns false
/// so normal paste routing continues to the composer, with a status notice
/// pointing at the covered destination.
pub(super) fn handle_paste(state: &mut State, context: &ViewContext, text: &str) -> PasteResult {
    if text.is_empty() || !context.eligible(state) || state.forms.is_empty() {
        return PasteResult::ignored();
    }

    let active = state.active;
    let Some(form) = state.forms.get_mut(active) else {
        return PasteResult::ignored();
    };

    if form.submit == super::state::SubmitState::Submitting {
        // No editor writes while a response is outstanding.
        return PasteResult::consumed();
    }

    if let Some(editor) = form.editor {
        return if paste_into_editor(form, editor, text) {
            PasteResult::consumed()
        } else {
            PasteResult::rejected()
        };
    }

    let question_index = form.tab;
    let highlighted_other = form
        .current_question()
        .is_some_and(super::state::Question::highlight_is_other);
    if highlighted_other && let Some(editor) = select_other(form, question_index) {
        return if paste_into_editor(form, editor, text) {
            PasteResult::consumed()
        } else {
            PasteResult::rejected()
        };
    }

    PasteResult {
        consumed: false,
        notice: Some(
            "Paste went to the composer behind the question form; press n or choose Other to type here."
                .to_string(),
        ),
    }
}

fn select_other(form: &mut Form, question_index: usize) -> Option<EditorFocus> {
    let question = form.questions.get_mut(question_index)?;
    let other = question.other.clone()?;
    if question.multi {
        question.selected.insert(other.value);
    } else {
        question.confirmed = Some(other.value);
    }
    question.other_cursor = question.other_text.len();
    let editor = EditorFocus {
        question: question_index,
        kind: EditorKind::Other,
    };
    form.editor = Some(editor);
    Some(editor)
}

fn paste_into_editor(form: &mut Form, editor: EditorFocus, text: &str) -> bool {
    let Some(question) = form.questions.get_mut(editor.question) else {
        return true;
    };
    let (target, cursor) = match editor.kind {
        EditorKind::Other => (&mut question.other_text, &mut question.other_cursor),
        EditorKind::Notes => (&mut question.notes, &mut question.notes_cursor),
    };
    if !insert_text(target, cursor, text) {
        form.notice = Some(format!(
            "Text limit is {MAX_TEXT_CODE_POINTS} characters; paste rejected."
        ));
        return false;
    }
    form.notice = None;
    true
}
