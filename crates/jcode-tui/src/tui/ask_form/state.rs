//! Scoped state for the `ask_user` TUI form.
//!
//! All form state lives in a `thread_local!`; no `App` field is added. The
//! state is scoped to one remote-loop instance, one connection generation and
//! one session. Render, keys and paste all consult
//! [`ViewContext`](super::ViewContext) before touching it.

use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};

use serde_json::json;
use serde_json::{Map, Value};

use crate::protocol::elicitation::{FormElicitation, FormSchema};

use super::schema;

/// Schema tag shared by the tool and the TUI. Deliberately a private constant
/// in this crate; `jcode-protocol` does not know about it.
pub(crate) const TAG: &str = "jcode.dev/askUser";

/// Maximum number of Unicode code points accepted by one text editor.
pub(super) const MAX_TEXT_CODE_POINTS: usize = 8192;

/// One selectable option from a tagged question.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Choice {
    pub value: String,
    pub label: String,
    pub description: Option<String>,
    pub preview: Option<String>,
}

/// The automatically added "Other" free-text choice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct OtherField {
    /// The schema `const` for the Other choice (`__other__`).
    pub value: String,
    /// The sibling string field that carries the typed text.
    pub field: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum EditorKind {
    Other,
    Notes,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct EditorFocus {
    pub question: usize,
    pub kind: EditorKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum SubmitState {
    /// Normal editing.
    Idle,
    /// A response has been handed to the transport.
    Submitting,
    /// The server rejected the response; editing resumes with this message.
    Rejected(String),
}

/// Per-question answer and cursor state.
#[derive(Debug, Clone)]
pub(super) struct Question {
    pub field: String,
    pub header: String,
    pub text: String,
    pub multi: bool,
    pub options: Vec<Choice>,
    pub other: Option<OtherField>,
    pub notes_field: Option<String>,
    /// Highlighted row. This is *not* a confirmed selection.
    pub highlight: usize,
    /// Confirmed single-select value.
    pub confirmed: Option<String>,
    /// Confirmed multi-select values.
    pub selected: BTreeSet<String>,
    pub other_text: String,
    pub notes: String,
    pub other_cursor: usize,
    pub notes_cursor: usize,
}

impl Question {
    pub(super) fn new(
        field: String,
        header: String,
        text: String,
        multi: bool,
        options: Vec<Choice>,
        other: Option<OtherField>,
        notes_field: Option<String>,
    ) -> Self {
        Self {
            field,
            header,
            text,
            multi,
            options,
            other,
            notes_field,
            highlight: 0,
            confirmed: None,
            selected: BTreeSet::new(),
            other_text: String::new(),
            notes: String::new(),
            other_cursor: 0,
            notes_cursor: 0,
        }
    }

    /// Rows include the selectable options and, when present, the Other line.
    pub(super) fn rows(&self) -> usize {
        self.options.len() + usize::from(self.other.is_some())
    }

    pub(super) fn other_row(&self) -> Option<usize> {
        self.other.as_ref().map(|_| self.options.len())
    }

    pub(super) fn highlight_is_other(&self) -> bool {
        self.other_row() == Some(self.highlight)
    }

    pub(super) fn other_value(&self) -> &str {
        self.other
            .as_ref()
            .map(|other| other.value.as_str())
            .unwrap_or("__other__")
    }

    pub(super) fn value_is_other(&self, value: &str) -> bool {
        self.other.is_some() && value == self.other_value()
    }

    pub(super) fn other_selected(&self) -> bool {
        if self.multi {
            self.selected.iter().any(|value| self.value_is_other(value))
        } else {
            self.confirmed
                .as_deref()
                .is_some_and(|value| self.value_is_other(value))
        }
    }

    pub(super) fn answered(&self) -> bool {
        if self.multi {
            let normal = self
                .selected
                .iter()
                .any(|value| !self.value_is_other(value));
            normal || (self.other_selected() && !self.other_text.trim().is_empty())
        } else {
            match self.confirmed.as_deref() {
                None => false,
                Some(value) if self.value_is_other(value) => !self.other_text.trim().is_empty(),
                Some(_) => true,
            }
        }
    }

    pub(super) fn selected_values(&self) -> Vec<String> {
        self.selected.iter().cloned().collect()
    }

    pub(super) fn label_for(&self, value: &str) -> String {
        self.options
            .iter()
            .find(|choice| choice.value == value)
            .map(|choice| choice.label.clone())
            .unwrap_or_else(|| value.to_string())
    }

    /// Human-readable answer for the summary tab.
    pub(super) fn answer_text(&self) -> String {
        let mut answer = if self.multi {
            let mut labels: Vec<String> = self
                .selected
                .iter()
                .filter(|value| !self.value_is_other(value))
                .map(|value| self.label_for(value))
                .collect();
            if self.other_selected() {
                labels.push(other_answer_text(self));
            }
            labels.join(", ")
        } else if let Some(value) = self.confirmed.as_deref() {
            if self.value_is_other(value) {
                other_answer_text(self)
            } else {
                self.label_for(value)
            }
        } else {
            String::new()
        };
        if answer.trim().is_empty() {
            return "not answered".to_string();
        }
        if !self.notes.trim().is_empty() {
            answer.push_str(" · notes: ");
            answer.push_str(self.notes.trim());
        }
        answer
    }

    pub(super) fn preview_for_highlight(&self) -> Option<&str> {
        self.options
            .get(self.highlight)
            .and_then(|choice| choice.preview.as_deref())
    }
}

fn other_answer_text(question: &Question) -> String {
    let text = question.other_text.trim();
    if text.is_empty() {
        "Other".to_string()
    } else {
        text.to_string()
    }
}

/// One queued form. Replay restores this object when the schema is identical.
#[derive(Debug, Clone)]
pub(super) struct Form {
    pub session: String,
    pub elicitation_id: String,
    pub message: String,
    pub schema: FormSchema,
    pub questions: Vec<Question>,
    /// `0..questions.len()` are question tabs; `questions.len()` is Submit.
    pub tab: usize,
    /// Selected row on the Submit summary tab, `0..=questions.len()`.
    pub summary_index: usize,
    pub editor: Option<EditorFocus>,
    pub preview_focus: bool,
    pub submit: SubmitState,
    pub notice: Option<String>,
    /// Generation in which this form was last received. `None` means it is
    /// awaiting replay after a connection replacement.
    pub seen_generation: Option<u64>,
    pub body_scroll: usize,
    pub preview_scroll: usize,
    pub last_body_height: usize,
    pub last_body_lines: usize,
    pub last_preview_lines: usize,
    pub reveal_highlight: bool,
}

impl Form {
    pub(super) fn new(request: &FormElicitation, generation: u64) -> Option<Self> {
        let questions = schema::parse_questions(&request.requested_schema.0)?;
        let first_unanswered = questions
            .iter()
            .position(|question| !question.answered())
            .unwrap_or(questions.len());
        let tab = first_unanswered;
        let summary_index = if tab == questions.len() {
            questions.len()
        } else {
            tab
        };
        Some(Self {
            session: request.session_id.clone(),
            elicitation_id: request.elicitation_id.clone(),
            message: request.message.clone(),
            schema: request.requested_schema.clone(),
            questions,
            tab,
            summary_index,
            editor: None,
            preview_focus: false,
            submit: SubmitState::Idle,
            notice: None,
            seen_generation: Some(generation),
            body_scroll: 0,
            preview_scroll: 0,
            last_body_height: 0,
            last_body_lines: 0,
            last_preview_lines: 0,
            reveal_highlight: false,
        })
    }

    pub(super) fn question_count(&self) -> usize {
        self.questions.len()
    }

    pub(super) fn submit_tab(&self) -> usize {
        self.questions.len()
    }

    pub(super) fn all_answered(&self) -> bool {
        self.questions.iter().all(Question::answered)
    }

    pub(super) fn first_unanswered(&self) -> Option<usize> {
        self.questions
            .iter()
            .position(|question| !question.answered())
    }

    pub(super) fn is_submit_tab(&self) -> bool {
        self.tab == self.submit_tab()
    }

    pub(super) fn current_question(&self) -> Option<&Question> {
        self.questions.get(self.tab)
    }

    pub(super) fn current_question_mut(&mut self) -> Option<&mut Question> {
        self.questions.get_mut(self.tab)
    }

    /// Enter the Submit tab, focusing the summary entry the plan asks for.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn focus_submit_tab(&mut self) {
        self.tab = self.submit_tab();
        self.editor = None;
        self.preview_focus = false;
        self.summary_index = if self.all_answered() {
            self.submit_tab()
        } else {
            self.first_unanswered().unwrap_or(0)
        };
    }

    /// Move forward/back through question and Submit tabs. Answers are
    /// untouched. Movement clamps at the first and last tab.
    pub(super) fn move_tab(&mut self, forward: bool) {
        let last = self.submit_tab();
        self.tab = if forward {
            (self.tab + 1).min(last)
        } else {
            self.tab.saturating_sub(1)
        };
        self.editor = None;
        self.preview_focus = false;
        if self.is_submit_tab() {
            self.summary_index = if self.all_answered() {
                self.submit_tab()
            } else {
                self.first_unanswered().unwrap_or(0)
            };
        }
    }

    pub(super) fn mark_submitting(&mut self) {
        self.submit = SubmitState::Submitting;
        self.editor = None;
        self.notice = None;
    }

    pub(super) fn restore_editing_after_rejection(&mut self, message: String) {
        self.submit = SubmitState::Rejected(message.clone());
        self.editor = None;
        self.notice = Some(message);
    }

    /// Build the ACP content map, or explain why the form cannot be submitted.
    pub(super) fn content(&self) -> Result<Map<String, Value>, String> {
        let mut content = Map::new();
        for (index, question) in self.questions.iter().enumerate() {
            if !question.answered() {
                return Err(format!("question {} still needs an answer", index + 1));
            }
            let value = if question.multi {
                json!(question.selected_values())
            } else {
                Value::String(
                    question
                        .confirmed
                        .clone()
                        .expect("answered single-select has a confirmed value"),
                )
            };
            content.insert(question.field.clone(), value);
            if question.other_selected()
                && let Some(other) = &question.other
            {
                content.insert(
                    other.field.clone(),
                    Value::String(question.other_text.clone()),
                );
            }
            if let Some(notes_field) = &question.notes_field
                && !question.notes.trim().is_empty()
            {
                content.insert(notes_field.clone(), Value::String(question.notes.clone()));
            }
        }
        Ok(content)
    }

    /// Local content validation, so a server rejection should only mean the
    /// form no longer belongs to this attachment.
    pub(super) fn checked_content(&self) -> Result<Map<String, Value>, String> {
        let content = self.content()?;
        self.schema
            .check_content(&content)
            .map(|()| content)
            .map_err(|error| format!("cannot submit: {error}"))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ResponseOwner {
    pub session: String,
    pub elicitation_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SendTarget {
    pub session: String,
    pub elicitation_id: String,
}

/// All state in this module. Reset on remote-loop entry/exit and scoped to the
/// session and connection generation recorded here.
#[derive(Debug, Default)]
pub(super) struct State {
    pub loop_live: bool,
    pub generation: u64,
    pub attached: bool,
    pub session: Option<String>,
    pub forms: Vec<Form>,
    pub active: usize,
    pub responses: HashMap<u64, ResponseOwner>,
    /// The form handed to the transport by the most recent `Send`.
    pub last_sent: Option<SendTarget>,
}

impl State {
    pub(super) fn reset(&mut self) {
        *self = Self::default();
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn active_form(&self) -> Option<&Form> {
        self.forms.get(self.active)
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn active_form_mut(&mut self) -> Option<&mut Form> {
        let active = self.active;
        self.forms.get_mut(active)
    }

    pub(super) fn find_form(&self, session: &str, elicitation_id: &str) -> Option<usize> {
        self.forms
            .iter()
            .position(|form| form.session == session && form.elicitation_id == elicitation_id)
    }

    pub(super) fn remove_form(&mut self, session: &str, elicitation_id: &str) -> Option<Form> {
        let index = self.find_form(session, elicitation_id)?;
        let form = self.forms.remove(index);
        if self.active >= self.forms.len() {
            self.active = self.forms.len().saturating_sub(1);
        }
        Some(form)
    }

    /// Drop every form that was not replayed in the current generation.
    pub(super) fn discard_unreplayed(&mut self) -> usize {
        let generation = self.generation;
        let before = self.forms.len();
        self.forms
            .retain(|form| form.seen_generation == Some(generation));
        if self.active >= self.forms.len() {
            self.active = self.forms.len().saturating_sub(1);
        }
        before - self.forms.len()
    }

    /// Clear every form and response ownership while keeping the lifecycle
    /// scope (loop/generation/session).
    pub(super) fn clear_forms(&mut self) {
        self.forms.clear();
        self.active = 0;
        self.responses.clear();
        self.last_sent = None;
    }
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
}

pub(super) fn with<R>(f: impl FnOnce(&mut State) -> R) -> R {
    STATE.with(|state| f(&mut state.borrow_mut()))
}

#[cfg(test)]
pub(crate) fn reset_for_test() {
    STATE.with(|state| *state.borrow_mut() = State::default());
}

#[cfg(test)]
pub(super) fn snapshot<R>(f: impl FnOnce(&State) -> R) -> R {
    STATE.with(|state| f(&state.borrow()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::elicitation::{ElicitationResponse, FormSchema};
    use serde_json::json;

    fn schema() -> FormSchema {
        FormSchema(json!({
            "_meta": {"jcode.dev/askUser": {"version": 1}},
            "type": "object",
            "required": ["q1", "q2"],
            "properties": {
                "q1": {
                    "type": "string",
                    "title": "Pick",
                    "description": "Which?",
                    "_meta": {"jcode.dev/askUser": {"header": "Pick", "otherField": "q1_other"}},
                    "oneOf": [
                        {"const": "a", "title": "A"},
                        {"const": "__other__", "title": "Other"}
                    ]
                },
                "q1_other": {"type": "string"},
                "q2": {
                    "type": "array",
                    "title": "Many",
                    "items": {"anyOf": [{"const": "x", "title": "X"}, {"const": "y", "title": "Y"}]}
                }
            }
        }))
    }

    fn request() -> FormElicitation {
        FormElicitation {
            elicitation_id: "e".into(),
            session_id: "s".into(),
            tool_call_id: None,
            message: "Questions".into(),
            requested_schema: schema(),
        }
    }

    #[test]
    fn form_starts_unanswered_with_first_question_focused() {
        let form = Form::new(&request(), 1).expect("form");
        assert_eq!(form.tab, 0);
        assert!(!form.questions[0].answered());
        assert_eq!(form.questions[0].highlight, 0);
        assert!(form.questions[0].confirmed.is_none());
        assert!(form.questions[0].selected.is_empty());
    }

    #[test]
    fn empty_other_is_unanswered_but_typed_other_answers() {
        let mut form = Form::new(&request(), 1).expect("form");
        let question = &mut form.questions[0];
        question.confirmed = Some("__other__".into());
        assert!(!question.answered());
        question.other_text = "  typed  ".into();
        assert!(question.answered());
    }

    #[test]
    fn content_only_includes_other_while_other_is_selected() {
        let mut form = Form::new(&request(), 1).expect("form");
        form.questions[0].confirmed = Some("a".into());
        form.questions[0].other_text = "ignored".into();
        form.questions[1].selected.insert("x".into());
        let map = form.content().expect("content");
        assert_eq!(map.get("q1"), Some(&json!("a")));
        assert!(map.get("q1_other").is_none());
        assert_eq!(map.get("q2"), Some(&json!(["x"])));
        assert!(form.checked_content().is_ok());
    }

    #[test]
    fn rejection_restores_editing_with_message() {
        let mut form = Form::new(&request(), 1).expect("form");
        form.mark_submitting();
        assert_eq!(form.submit, SubmitState::Submitting);
        form.restore_editing_after_rejection("nope".into());
        assert!(matches!(form.submit, SubmitState::Rejected(_)));
        assert_eq!(form.notice.as_deref(), Some("nope"));
    }

    #[test]
    fn response_variants_are_constructible() {
        // Keeps the exported response shape honest in this module's tests.
        let accept = ElicitationResponse::Accept { content: None };
        assert!(!matches!(accept, ElicitationResponse::Decline));
    }
}
