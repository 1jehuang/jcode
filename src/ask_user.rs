//! Global pending-question registry for the `askUserQuestion` tool.
//!
//! When the tool is invoked it stages an `AskUserQuestionRequest` in this
//! registry, publishes a `BusEvent::AskUserQuestionOpened` so the TUI can
//! display its modal overlay, and `await`s on a oneshot receiver. When the
//! user answers (or cancels) via the modal, the TUI calls
//! [`submit_answer`] which removes the entry and fulfils the receiver.
//!
//! The mechanism mirrors the existing `StdinInputRequest` pattern but is
//! routed through a global map keyed by request_id so the tool execute
//! method does not need direct access to TUI state.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use tokio::sync::oneshot;

/// A single answer option offered to the user.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AskUserOption {
    /// Stable choice id (A, B, keep, rec, ...).
    pub id: String,
    /// Human-readable label.
    pub label: String,
    /// Optional explanation/notes shown under the label.
    pub description: Option<String>,
    /// Optional "exact value" the agent receives if this option is picked.
    pub value: Option<String>,
    /// True if this is the agent's recommended option.
    pub recommended: bool,
    /// Reason for the recommendation, displayed only on the recommended row.
    pub recommendation_reason: Option<String>,
}

/// Payload describing a pending question for the TUI to render.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AskUserQuestion {
    pub request_id: String,
    pub session_id: String,
    pub question: String,
    pub context: Option<String>,
    pub options: Vec<AskUserOption>,
    pub allow_multiple: bool,
    pub reply_instructions: Option<String>,
    pub title: Option<String>,
}

/// Final answer returned to the tool.
///
/// `kind` discriminates how the user responded so the agent can format the
/// downstream tool result appropriately.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AskUserAnswer {
    pub request_id: String,
    pub kind: AskUserAnswerKind,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AskUserAnswerKind {
    /// User picked one or more pre-defined options.
    Options {
        /// Option ids (preserving display order).
        ids: Vec<String>,
        /// Labels for the picked ids (for display in the tool result).
        labels: Vec<String>,
        /// `value` fields for the picked ids when set (parallel to `ids`).
        values: Vec<Option<String>>,
    },
    /// User typed a free-form answer instead of (or in addition to) picking.
    Custom { text: String },
    /// User dismissed the modal (Esc) without answering.
    Canceled,
}

/// Process-wide registry of in-flight ask-user requests.
fn registry() -> &'static Mutex<HashMap<String, oneshot::Sender<AskUserAnswer>>> {
    static R: OnceLock<Mutex<HashMap<String, oneshot::Sender<AskUserAnswer>>>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Register a pending question and return the receiver half. The caller
/// should then publish `BusEvent::AskUserQuestionOpened` so the TUI can
/// render the modal, and `await` on the returned receiver.
pub fn register_pending(request_id: String) -> oneshot::Receiver<AskUserAnswer> {
    let (tx, rx) = oneshot::channel();
    if let Ok(mut map) = registry().lock() {
        map.insert(request_id, tx);
    }
    rx
}

/// Submit an answer for a previously registered request. Returns true if the
/// request existed and was answered; false if it had already been answered
/// or canceled.
pub fn submit_answer(answer: AskUserAnswer) -> bool {
    let tx = match registry().lock() {
        Ok(mut map) => map.remove(&answer.request_id),
        Err(_) => return false,
    };
    match tx {
        Some(tx) => tx.send(answer).is_ok(),
        None => false,
    }
}

/// Discard a pending request (e.g. session reset) without answering. Any
/// awaiter will observe a closed channel and surface a cancellation error.
#[allow(dead_code)]
pub fn drop_pending(request_id: &str) {
    if let Ok(mut map) = registry().lock() {
        map.remove(request_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn submit_round_trip() {
        let id = "test-ask-user-1".to_string();
        let rx = register_pending(id.clone());
        let ok = submit_answer(AskUserAnswer {
            request_id: id.clone(),
            kind: AskUserAnswerKind::Custom {
                text: "hello".into(),
            },
        });
        assert!(ok);
        let got = rx.await.expect("answer should arrive");
        assert_eq!(got.request_id, id);
        match got.kind {
            AskUserAnswerKind::Custom { text } => assert_eq!(text, "hello"),
            other => panic!("unexpected kind: {:?}", other),
        }
    }

    #[tokio::test]
    async fn submit_unknown_id_is_false() {
        let ok = submit_answer(AskUserAnswer {
            request_id: "nope".into(),
            kind: AskUserAnswerKind::Canceled,
        });
        assert!(!ok);
    }

    #[tokio::test]
    async fn drop_pending_closes_channel() {
        let id = "test-ask-user-drop".to_string();
        let rx = register_pending(id.clone());
        drop_pending(&id);
        assert!(rx.await.is_err(), "awaiter should observe closed channel");
    }
}
