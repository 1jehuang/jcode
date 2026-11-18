//! `ask_user` question dialog: one question at a time, numbered options, an
//! "Other" row that takes free text, and checkboxes for multi-select.
use super::*;
use crate::protocol::{AskUserAnswer, AskUserQuestion};
use std::collections::VecDeque;

pub(super) struct AskRequest {
    request_id: String,
    questions: Vec<AskUserQuestion>,
}

pub(super) struct AskState {
    request: AskRequest,
    answers: Vec<AskUserAnswer>,
    checked: Vec<bool>,
    other: String,
    /// Parallel ask_user calls wait here instead of replacing the open one.
    queued: VecDeque<AskRequest>,
}

impl AskState {
    fn question(&self) -> &AskUserQuestion {
        &self.request.questions[self.answers.len()]
    }

    fn start(&mut self, request: AskRequest) {
        self.checked = vec![false; request.questions[0].options.len()];
        self.request = request;
        self.answers.clear();
        self.other.clear();
    }
}

impl Dialog {
    pub fn ask_user(request_id: String, questions: Vec<AskUserQuestion>) -> Self {
        let mut dialog = Self {
            stack: Vec::new(),
            mcp: None,
            skills: None,
            plugin: None,
            rewind: None,
            ask: None,
            pending_rewind: false,
            status: None,
            pending_mcp: None,
        };
        dialog.queue_ask_user(request_id, questions);
        dialog
    }

    pub fn is_ask_user(&self) -> bool {
        self.ask.is_some()
    }

    /// Show these questions now, or after the open ones when already asking.
    pub fn queue_ask_user(&mut self, request_id: String, questions: Vec<AskUserQuestion>) {
        // The daemon validates these; never index into an empty list.
        if questions.is_empty() || questions.iter().any(|q| q.options.is_empty()) {
            return;
        }
        let request = AskRequest {
            request_id,
            questions,
        };
        match self.ask.as_mut() {
            Some(ask) => ask.queued.push_back(request),
            None => {
                self.ask = Some(AskState {
                    checked: vec![false; request.questions[0].options.len()],
                    request,
                    answers: Vec::new(),
                    other: String::new(),
                    queued: VecDeque::new(),
                });
                self.stack = vec![self.ask_view()];
            }
        }
    }

    /// The daemon closed this request (interrupted call): drop its questions.
    /// Returns whether nothing is left to ask.
    pub fn ask_user_closed(&mut self, request_id: &str) -> bool {
        let Some(ask) = self.ask.as_mut() else {
            return true;
        };
        ask.queued.retain(|r| r.request_id != request_id);
        if ask.request.request_id == request_id {
            return self.next_ask_user();
        }
        false
    }

    /// Bracketed paste goes into a focused Other field and nowhere else.
    pub fn paste_ask_user(&mut self, text: &str) {
        let (Some(ask), Some(view)) = (self.ask.as_mut(), self.stack.last()) else {
            return;
        };
        if view.selected == ask.question().options.len() {
            let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
            ask.other.push_str(&text);
            self.rebuild();
        }
    }

    /// Advance to the next queued request. Returns whether none is left.
    fn next_ask_user(&mut self) -> bool {
        let Some(ask) = self.ask.as_mut() else {
            return true;
        };
        match ask.queued.pop_front() {
            Some(request) => {
                ask.start(request);
                self.stack = vec![self.ask_view()];
                false
            }
            None => {
                self.ask = None;
                self.stack.clear();
                true
            }
        }
    }

    pub(super) fn ask_view(&self) -> View {
        let mut view = View::new(Route::AskUser, "");
        let Some(ask) = self.ask.as_ref() else {
            return view;
        };
        let question = ask.question();
        view.title = format!(
            "Question {}/{} · {}",
            ask.answers.len() + 1,
            ask.request.questions.len(),
            question.header
        );
        view.body.push(question.question.clone());
        view.numbered = true;
        let checkbox = |on: bool| if on { "[x] " } else { "[ ] " };
        for (index, option) in question.options.iter().enumerate() {
            let mut label = option.label.clone();
            if question.multi_select {
                label.insert_str(0, checkbox(ask.checked[index]));
            }
            view.items.push(Item {
                group: None,
                icon: None,
                label,
                meta: vec![option.description.clone()],
                search_text: String::new(),
                action: None,
            });
        }
        let mut other = if ask.other.is_empty() {
            "Other (type your answer)".to_string()
        } else {
            format!("Other: {}", ask.other)
        };
        if question.multi_select {
            other.insert_str(0, checkbox(!ask.other.is_empty()));
        }
        // The Other row shows the end of long text, where the user types.
        view.tail_item = Some(question.options.len());
        let mut rows = vec![other];
        if question.multi_select {
            let last = ask.answers.len() + 1 == ask.request.questions.len();
            rows.push(if last { "Submit" } else { "Next" }.to_string());
        }
        for label in rows {
            view.items.push(Item {
                group: None,
                icon: None,
                label,
                meta: Vec::new(),
                search_text: String::new(),
                action: None,
            });
        }
        view
    }

    pub(super) fn ask_hint(&self, view: &View) -> String {
        let Some(ask) = self.ask.as_ref() else {
            return String::new();
        };
        let question = ask.question();
        if view.selected == question.options.len() {
            "Type your answer · Enter to submit · Esc to dismiss".into()
        } else if question.multi_select {
            "↑/↓ to navigate · Space to toggle · Enter on Submit · Esc to dismiss".into()
        } else {
            "↑/↓ to navigate · Enter to select · Esc to dismiss".into()
        }
    }

    pub(super) fn handle_ask_key(&mut self, code: KeyCode, modifiers: KeyModifiers) -> DialogKey {
        let (Some(ask), Some(view)) = (self.ask.as_mut(), self.stack.last_mut()) else {
            return DialogKey::Close;
        };
        let options = ask.question().options.len();
        let multi = ask.question().multi_select;
        let count = view.items.len();
        let on_other = view.selected == options;
        let typing = !modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
        let mut activate = None;
        match code {
            KeyCode::Esc => return self.answer_ask_user(None),
            KeyCode::Char('c' | 'd') if modifiers.contains(KeyModifiers::CONTROL) => {
                return self.answer_ask_user(None);
            }
            KeyCode::Up | KeyCode::BackTab => view.selected = (view.selected + count - 1) % count,
            KeyCode::Down | KeyCode::Tab => view.selected = (view.selected + 1) % count,
            KeyCode::Enter => activate = Some(view.selected),
            KeyCode::Backspace if on_other => {
                ask.other.pop();
            }
            KeyCode::Char(ch) if on_other && typing => ask.other.push(ch),
            KeyCode::Char(' ') if multi => activate = Some(view.selected),
            KeyCode::Char(digit @ '1'..='9') if typing => {
                let index = digit as usize - '1' as usize;
                if index < count {
                    view.selected = index;
                    // Digits pick plain options; Other and Submit just get focus.
                    activate = (index < options).then_some(index);
                }
            }
            _ => {}
        }
        if let Some(row) = activate {
            let Some(ask) = self.ask.as_mut() else {
                return DialogKey::Close;
            };
            if row < options && multi {
                ask.checked[row] = !ask.checked[row];
            } else {
                let question = ask.question();
                let mut answers: Vec<String> = if row < options {
                    vec![question.options[row].label.clone()]
                } else {
                    question
                        .options
                        .iter()
                        .zip(&ask.checked)
                        .filter(|(_, on)| **on)
                        .map(|(option, _)| option.label.clone())
                        .collect()
                };
                let other = ask.other.trim();
                if row >= options && !other.is_empty() {
                    answers.push(other.to_string());
                }
                // A single-select Other row needs text before it counts.
                if !multi && answers.is_empty() {
                    return DialogKey::Handled;
                }
                let question = question.question.clone();
                ask.answers.push(AskUserAnswer { question, answers });
                if ask.answers.len() == ask.request.questions.len() {
                    let answers = std::mem::take(&mut ask.answers);
                    return self.answer_ask_user(Some(answers));
                }
                ask.checked = vec![false; ask.question().options.len()];
                ask.other.clear();
                self.stack = vec![self.ask_view()];
                return DialogKey::Handled;
            }
        }
        self.rebuild();
        DialogKey::Handled
    }

    fn answer_ask_user(&mut self, answers: Option<Vec<AskUserAnswer>>) -> DialogKey {
        let Some(ask) = self.ask.as_ref() else {
            return DialogKey::Close;
        };
        let request_id = ask.request.request_id.clone();
        self.next_ask_user();
        DialogKey::AskUser {
            request_id,
            answers,
        }
    }
}
