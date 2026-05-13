//! Global pending-question registry for the `askUserQuestion` tool.
//!
//! When the tool is invoked it stages an `AskUserQuestion` in this registry,
//! publishes a `BusEvent::AskUserQuestionOpened` so the server can forward it
//! to the active TUI client (local or remote), and `await`s on a oneshot
//! receiver. When the user answers (or cancels) via the modal, the host
//! calls [`submit_answer`] which removes the entry and fulfils the receiver.
//!
//! The wire-level types live in `jcode_protocol`; this module re-exports them
//! under shorter names so call sites can use one canonical type for both the
//! in-process bus event and the cross-process protocol payload.

use jcode_protocol::{
    AskUserAnswerKindPayload, AskUserAnswerPayload, AskUserOptionPayload, AskUserQuestionPayload,
};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use tokio::sync::oneshot;

pub type AskUserOption = AskUserOptionPayload;
pub type AskUserQuestion = AskUserQuestionPayload;
pub type AskUserAnswer = AskUserAnswerPayload;
pub type AskUserAnswerKind = AskUserAnswerKindPayload;

/// Process-wide registry of in-flight ask-user requests.
fn registry() -> &'static Mutex<HashMap<String, oneshot::Sender<AskUserAnswer>>> {
    static R: OnceLock<Mutex<HashMap<String, oneshot::Sender<AskUserAnswer>>>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Register a pending question and return the receiver half. The caller
/// should then publish `BusEvent::AskUserQuestionOpened` so the host can
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
