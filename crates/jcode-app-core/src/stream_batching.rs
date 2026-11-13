//! Coalesces streamed text for sessions nobody is looking at.
//!
//! Every text delta the agent publishes wakes the turn's fanout task, then each
//! attached client's writer task, then each client process. A backtrace of the
//! server's worker unparks with four sessions streaming attributed 117 of 160
//! to that first send. When every client attached to a session has reported
//! that its window is unfocused, the same text can travel in one event per
//! [`UNFOCUSED_TEXT_BATCH`] instead, which is the rate those clients redraw
//! streamed text at anyway.
//!
//! Clients that never report focus (older builds, the desktop bridge, SDK and
//! mobile clients) count as focused, so a session with any of them attached is
//! never batched.

use crate::protocol::ServerEvent;
use std::collections::{HashMap, HashSet};
use std::sync::{LazyLock, Mutex};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::Instant;

/// Matches the unfocused client's own streamed-text redraw period.
pub(crate) const UNFOCUSED_TEXT_BATCH: Duration = Duration::from_millis(250);

#[derive(Default)]
struct State {
    /// Last focus report per connection. Absent means "never reported".
    focus: HashMap<String, bool>,
    /// Sessions whose attached clients are all known to be unfocused.
    batched_sessions: HashSet<String>,
}

static STATE: LazyLock<Mutex<State>> = LazyLock::new(|| Mutex::new(State::default()));

fn state() -> std::sync::MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

/// Record a client's focus report. Gaining focus stops batching for its session
/// immediately instead of waiting for the next fanout to notice.
pub(crate) fn set_connection_focus(connection_id: &str, session_id: &str, focused: bool) {
    let mut state = state();
    state.focus.insert(connection_id.to_string(), focused);
    if focused {
        state.batched_sessions.remove(session_id);
    }
}

pub(crate) fn forget_connection(connection_id: &str) {
    state().focus.remove(connection_id);
}

/// Forgets a connection's focus report when its handler ends.
pub(crate) struct ConnectionFocusGuard(pub String);

impl Drop for ConnectionFocusGuard {
    fn drop(&mut self) {
        forget_connection(&self.0);
    }
}

/// Called by the session fanout with the connections it is about to deliver to,
/// so the batching decision follows attach, detach and resume without each of
/// those paths having to maintain it.
pub(crate) fn note_session_targets<'a>(
    session_id: &str,
    connection_ids: impl IntoIterator<Item = &'a String>,
) {
    let mut state = state();
    let mut any = false;
    let mut all_unfocused = true;
    for connection_id in connection_ids {
        any = true;
        if state.focus.get(connection_id) != Some(&false) {
            all_unfocused = false;
            break;
        }
    }
    if any && all_unfocused {
        // Checked first so the per-event path does not allocate.
        if !state.batched_sessions.contains(session_id) {
            state.batched_sessions.insert(session_id.to_string());
        }
    } else {
        state.batched_sessions.remove(session_id);
    }
}

pub(crate) fn session_batches(session_id: &str) -> bool {
    state().batched_sessions.contains(session_id)
}

#[derive(Clone, PartialEq, Eq)]
enum Kind {
    Text,
    Reasoning,
    /// Streamed tool arguments, which the client appends per tool id. A large
    /// file write arrives as hundreds of these.
    ToolInput(Option<String>),
}

impl Kind {
    fn event(self, text: String) -> ServerEvent {
        match self {
            Kind::Text => ServerEvent::TextDelta { text },
            Kind::Reasoning => ServerEvent::ReasoningDelta { text },
            Kind::ToolInput(id) => ServerEvent::ToolInput { id, delta: text },
        }
    }
}

struct Pending {
    kind: Kind,
    text: String,
    flush_at: Instant,
}

/// A turn's event sender that holds back streamed text while the session is
/// batched. Every other event flushes the held text first, so ordering between
/// text and tool calls, completion and errors is unchanged.
pub(crate) struct BatchingSender {
    tx: mpsc::UnboundedSender<ServerEvent>,
    session_id: String,
    /// A lock rather than `&mut self` so the turn loop's many send sites and
    /// its timer branch can share one sender.
    pending: Mutex<Option<Pending>>,
}

impl BatchingSender {
    pub(crate) fn new(tx: mpsc::UnboundedSender<ServerEvent>, session_id: String) -> Self {
        Self {
            tx,
            session_id,
            pending: Mutex::new(None),
        }
    }

    fn pending(&self) -> std::sync::MutexGuard<'_, Option<Pending>> {
        self.pending.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn flush_locked(&self, pending: &mut Option<Pending>) {
        if let Some(pending) = pending.take() {
            let _ = self.tx.send(pending.kind.event(pending.text));
        }
    }

    /// The unbatched sender, for events that carry no ordering with the text.
    pub(crate) fn raw(&self) -> &mpsc::UnboundedSender<ServerEvent> {
        &self.tx
    }

    /// When held text must be flushed, if any is held.
    pub(crate) fn deadline(&self) -> Option<Instant> {
        self.pending().as_ref().map(|pending| pending.flush_at)
    }

    pub(crate) fn flush(&self) {
        let mut pending = self.pending();
        self.flush_locked(&mut pending);
    }

    pub(crate) fn send(
        &self,
        event: ServerEvent,
    ) -> Result<(), mpsc::error::SendError<ServerEvent>> {
        let (kind, text) = match event {
            ServerEvent::TextDelta { text } => (Kind::Text, text),
            ServerEvent::ReasoningDelta { text } => (Kind::Reasoning, text),
            ServerEvent::ToolInput { id, delta } => (Kind::ToolInput(id), delta),
            other => {
                let mut pending = self.pending();
                self.flush_locked(&mut pending);
                return self.tx.send(other);
            }
        };
        let mut pending = self.pending();
        if !session_batches(&self.session_id) {
            self.flush_locked(&mut pending);
            return self.tx.send(kind.event(text));
        }
        if self.tx.is_closed() {
            *pending = None;
            return Err(mpsc::error::SendError(kind.event(text)));
        }
        let now = Instant::now();
        match &mut *pending {
            Some(held) if held.kind == kind => {
                held.text.push_str(&text);
                // The timer normally does this. A busy loop that keeps getting
                // deltas before it polls the timer must not hold text longer.
                if now >= held.flush_at {
                    self.flush_locked(&mut pending);
                }
            }
            _ => {
                self.flush_locked(&mut pending);
                *pending = Some(Pending {
                    kind,
                    text,
                    flush_at: now + UNFOCUSED_TEXT_BATCH,
                });
            }
        }
        Ok(())
    }
}

impl Drop for BatchingSender {
    fn drop(&mut self) {
        self.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(event: ServerEvent) -> String {
        match event {
            ServerEvent::TextDelta { text } => format!("text:{text}"),
            ServerEvent::ReasoningDelta { text } => format!("reasoning:{text}"),
            ServerEvent::TextDone => "done".to_string(),
            ServerEvent::ToolInput { id, delta } => {
                format!("input[{}]:{delta}", id.unwrap_or_default())
            }
            other => panic!("unexpected event {other:?}"),
        }
    }

    fn delta(text: &str) -> ServerEvent {
        ServerEvent::TextDelta { text: text.into() }
    }

    fn ids(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| name.to_string()).collect()
    }

    #[tokio::test(start_paused = true)]
    async fn unreported_or_focused_clients_keep_every_delta() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let sender = BatchingSender::new(tx, "batch-s1".into());
        // An older client never reports focus, so it counts as focused.
        note_session_targets("batch-s1", &ids(&["batch-s1-old"]));
        sender.send(delta("a")).unwrap();
        sender.send(delta("b")).unwrap();
        assert_eq!(text(rx.try_recv().unwrap()), "text:a");
        assert_eq!(text(rx.try_recv().unwrap()), "text:b");

        // One unfocused and one silent client: still not batched.
        set_connection_focus("batch-s1-new", "batch-s1", false);
        note_session_targets("batch-s1", &ids(&["batch-s1-old", "batch-s1-new"]));
        sender.send(delta("c")).unwrap();
        assert_eq!(text(rx.try_recv().unwrap()), "text:c");
        assert!(sender.deadline().is_none());
        forget_connection("batch-s1-new");
    }

    #[tokio::test(start_paused = true)]
    async fn unfocused_session_coalesces_text_until_the_deadline() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let sender = BatchingSender::new(tx, "batch-s2".into());
        set_connection_focus("batch-s2-a", "batch-s2", false);
        set_connection_focus("batch-s2-b", "batch-s2", false);
        note_session_targets("batch-s2", &ids(&["batch-s2-a", "batch-s2-b"]));
        assert!(session_batches("batch-s2"));

        sender.send(delta("one ")).unwrap();
        let deadline = sender.deadline().expect("held text has a deadline");
        sender.send(delta("two ")).unwrap();
        assert_eq!(sender.deadline(), Some(deadline), "deadline is per batch");
        assert!(rx.try_recv().is_err());

        // The stream loop's timer path.
        tokio::time::sleep_until(deadline).await;
        sender.flush();
        assert_eq!(text(rx.try_recv().unwrap()), "text:one two ");
        assert!(sender.deadline().is_none());

        // A delta arriving past the deadline flushes without the timer.
        sender.send(delta("three ")).unwrap();
        tokio::time::advance(UNFOCUSED_TEXT_BATCH).await;
        sender.send(delta("four")).unwrap();
        assert_eq!(text(rx.try_recv().unwrap()), "text:three four");
        forget_connection("batch-s2-a");
        forget_connection("batch-s2-b");
    }

    #[tokio::test(start_paused = true)]
    async fn other_events_and_kind_changes_flush_held_text_in_order() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let sender = BatchingSender::new(tx, "batch-s3".into());
        set_connection_focus("batch-s3-a", "batch-s3", false);
        note_session_targets("batch-s3", &ids(&["batch-s3-a"]));

        sender
            .send(ServerEvent::ReasoningDelta { text: "hm".into() })
            .unwrap();
        sender.send(delta("an")).unwrap();
        sender.send(delta("swer")).unwrap();
        sender.send(ServerEvent::TextDone).unwrap();
        assert_eq!(text(rx.try_recv().unwrap()), "reasoning:hm");
        assert_eq!(text(rx.try_recv().unwrap()), "text:answer");
        assert_eq!(text(rx.try_recv().unwrap()), "done");

        // Dropping the sender (an early return from the turn) loses nothing.
        sender.send(delta("tail")).unwrap();
        drop(sender);
        assert_eq!(text(rx.try_recv().unwrap()), "text:tail");
        forget_connection("batch-s3-a");
    }

    #[tokio::test(start_paused = true)]
    async fn tool_input_coalesces_per_tool_and_never_merges_across_tools() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let sender = BatchingSender::new(tx, "batch-s5".into());
        set_connection_focus("batch-s5-a", "batch-s5", false);
        note_session_targets("batch-s5", &ids(&["batch-s5-a"]));
        let input = |id: &str, delta: &str| ServerEvent::ToolInput {
            id: Some(id.into()),
            delta: delta.into(),
        };

        sender.send(input("t1", "{\"path\":")).unwrap();
        sender.send(input("t1", "\"a.rs\"}")).unwrap();
        assert!(rx.try_recv().is_err());
        // Interleaved parallel tool calls keep their own argument streams.
        sender.send(input("t2", "{}")).unwrap();
        assert_eq!(
            text(rx.try_recv().unwrap()),
            "input[t1]:{\"path\":\"a.rs\"}"
        );
        assert!(rx.try_recv().is_err());
        sender.send(delta("next")).unwrap();
        assert_eq!(text(rx.try_recv().unwrap()), "input[t2]:{}");
        sender.flush();
        assert_eq!(text(rx.try_recv().unwrap()), "text:next");
        forget_connection("batch-s5-a");
    }

    #[tokio::test(start_paused = true)]
    async fn gaining_focus_or_detaching_stops_batching() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let sender = BatchingSender::new(tx, "batch-s4".into());
        set_connection_focus("batch-s4-a", "batch-s4", false);
        note_session_targets("batch-s4", &ids(&["batch-s4-a"]));
        sender.send(delta("held")).unwrap();
        assert!(rx.try_recv().is_err());

        set_connection_focus("batch-s4-a", "batch-s4", true);
        assert!(!session_batches("batch-s4"));
        sender.send(delta("live")).unwrap();
        assert_eq!(text(rx.try_recv().unwrap()), "text:held");
        assert_eq!(text(rx.try_recv().unwrap()), "text:live");

        // Unfocused again, then the client goes away: no attached client means
        // nothing is known about who will read the stream, so do not batch.
        set_connection_focus("batch-s4-a", "batch-s4", false);
        note_session_targets("batch-s4", &ids(&["batch-s4-a"]));
        assert!(session_batches("batch-s4"));
        note_session_targets("batch-s4", &ids(&[]));
        assert!(!session_batches("batch-s4"));

        // A connection whose report was forgotten is unknown again.
        note_session_targets("batch-s4", &ids(&["batch-s4-a"]));
        assert!(session_batches("batch-s4"));
        forget_connection("batch-s4-a");
        note_session_targets("batch-s4", &ids(&["batch-s4-a"]));
        assert!(!session_batches("batch-s4"));
    }
}
