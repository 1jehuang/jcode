//! How harness updates become model changes, and how sessions are switched.
//!
//! Split from `main.rs` so the event loop stays a router: this is the one
//! place the daemon's stream is folded into the [`crate::Model`], and the one
//! place attaching to another session resets the page. `app_selection.rs` is
//! the same pattern for the pointer.

use crate::{App, ModelId, harness, strip, transcript};

impl App {
    pub(crate) fn drain_harness_updates(&mut self) {
        let Some((updates, _)) = self.harness.as_ref() else {
            return;
        };
        // Whether a turn boundary went by in this batch. Queued messages are
        // sent at the boundary, after the drain: the flush needs `&mut self`,
        // which the loop's borrow of the update channel forbids.
        let mut turn_ended = false;
        while let Ok(update) = updates.try_recv() {
            match update {
                harness::HarnessUpdate::Status(status) => self.model.status = status,
                // A failure goes into the conversation, not only the status
                // line: the status line is suppressed once a session is
                // attached, which is exactly when a failed turn happens, so a
                // status-only report was invisible. The turn also ends, so the
                // spinner stops claiming work.
                harness::HarnessUpdate::Failed(message) => {
                    self.model.status = message.clone();
                    self.model.failure = Some(message.clone());
                    self.model.transcript.push_notice(&message);
                    self.model.busy = false;
                    self.model.activity.finish();
                    // The notice appears whole, so nothing is being revealed;
                    // leaving a reveal in flight would fade the failure in
                    // behind an animation that has nothing left to animate.
                    self.model.stream.reveal_all();
                    // A failure ends the turn as surely as `TurnDone` (the
                    // daemon sends `error` *instead of* `done`), so a message
                    // queued behind the failed turn gets its chance now
                    // rather than waiting forever.
                    turn_ended = true;
                }
                harness::HarnessUpdate::Attached {
                    session_id,
                    working_dir,
                } => {
                    self.model.status = format!("attached: {session_id}");
                    // A successful attach is the proof the failure is over: a
                    // reconnected window must not keep reporting the outage it
                    // just recovered from.
                    self.model.failure = None;
                    // A reconnect re-attaches the same session; the transcript
                    // on screen is the one that was being read, so it stays.
                    self.model.strip.focus_session(&session_id);
                    self.model.session_id = Some(session_id);
                    self.model.working_dir = working_dir;
                    self.retitle();
                }
                harness::HarnessUpdate::Model { provider, model } => {
                    self.model.model = Some(ModelId { provider, model });
                }
                harness::HarnessUpdate::Text(text) => {
                    self.model.transcript.append_assistant(&text);
                    // Chase the new length rather than jumping to it: the
                    // reveal is what turns a burst of tokens into a sweep.
                    self.model.stream.extend_to(
                        self.model.transcript.streaming_len(),
                        std::time::Instant::now(),
                    );
                }
                harness::HarnessUpdate::Reasoning(text) => {
                    self.model.transcript.append_reasoning(&text);
                    // Reasoning is revealed by the same sweep as the reply, so
                    // a thought does not appear as an instant wall of text
                    // while the answer below it types itself out.
                    self.model.stream.extend_to(
                        self.model.transcript.streaming_len(),
                        std::time::Instant::now(),
                    );
                }
                harness::HarnessUpdate::Activity(label) => {
                    self.model.busy = true;
                    self.model
                        .activity
                        .set_label(label, std::time::Instant::now());
                }
                harness::HarnessUpdate::Tool { call_id, label } => {
                    // Progress belongs in the transcript, not only in the
                    // composer's activity line: the call running right now is
                    // one card at the tail that refines in place as its
                    // streamed intent arrives, and the next call takes it
                    // over rather than adding a row.
                    self.model.busy = true;
                    self.model.transcript.set_live_tool(&call_id, &label);
                    self.model.stream.extend_to(
                        self.model.transcript.streaming_len(),
                        std::time::Instant::now(),
                    );
                }
                harness::HarnessUpdate::Edit(card) => {
                    // The one tool result that stays: an edit changed the
                    // user's files, so the transcript keeps its intent and its
                    // diff where the user can scroll back to them.
                    self.model.transcript.push_edit(&card);
                    self.model.stream.extend_to(
                        self.model.transcript.streaming_len(),
                        std::time::Instant::now(),
                    );
                }
                harness::HarnessUpdate::MessageAccepted => {
                    // The agent has the oldest message still in flight. Marking
                    // it here rather than on the first token is the point of
                    // the whole mechanism: "received" and "answered" are
                    // different facts, and the user is owed the first one now.
                    self.model
                        .transcript
                        .acknowledge_oldest_pending(std::time::Instant::now());
                }
                harness::HarnessUpdate::TurnDone => {
                    self.model.busy = false;
                    self.model.activity.finish();
                    // The card shows the call in flight; the turn ending means
                    // there is none, and a card left behind would claim work
                    // is still happening.
                    self.model.transcript.clear_live_tool();
                    turn_ended = true;
                }
                harness::HarnessUpdate::Peek {
                    session_id,
                    transcript,
                } => {
                    // The attached session's own transcript comes from the
                    // stream, so a peek reply for it is only ever cache: it
                    // must not overwrite the live page.
                    self.model.peeks.insert(&session_id, transcript);
                }
                harness::HarnessUpdate::Sessions(entries) => {
                    // Rebuild around the session we are actually attached to,
                    // so a refresh never silently moves the highlight off the
                    // conversation currently on screen.
                    self.model.strip =
                        strip::Strip::build(entries, self.model.session_id.as_deref());
                }
            }
        }
        // The turn is over and the channel is drained: if the user typed
        // while the agent was busy, the oldest waiting message goes now. Its
        // card leaves the queued tone, and the send is exactly the one the
        // submit would have made had the agent been free.
        if turn_ended && !self.model.busy {
            self.flush_queued_message();
        }
    }

    /// Switch to whichever session the strip now points at.
    ///
    /// The transcript belongs to the session, so it is cleared rather than
    /// carried across: appending another conversation's output to the one on
    /// screen would be actively misleading. Reloading real history needs
    /// `GetHistory`; until that is wired, an empty page is the honest state.
    pub(crate) fn attach_focused_session(&mut self) {
        let Some(target) = self.model.strip.focused_session().map(str::to_string) else {
            return;
        };
        if self.model.session_id.as_deref() == Some(target.as_str()) {
            return;
        }
        // Switching sessions changes the conversation, not the user's view
        // preferences: the thinking-display mode is carried across so a new
        // session does not silently revert to the structural default.
        let reasoning = self.model.transcript.reasoning_mode();
        self.model.transcript = transcript::Transcript::default();
        self.model.transcript.set_reasoning_mode(reasoning);
        self.model.stream.reveal_all();
        self.model.busy = false;
        self.model.activity.finish();
        self.model.scroll = 0.0;
        // Attaching is a jump, not a scroll: easing here would sweep through
        // the previous session's layout.
        self.model.smooth.settle();
        self.model.status = format!("attaching: {target}");
        self.model.session_id = Some(target.clone());
        // The new session's directory arrives with its `Attached` event; until
        // then show the strip's entry rather than the previous session's path,
        // which would name the wrong project.
        self.model.working_dir = self.model.strip.focused_working_dir().map(str::to_string);
        self.retitle();
        if let Some((_, outgoing)) = self.harness.as_ref() {
            let _ = outgoing.send(harness::Command::Attach(target));
        }
    }
}
