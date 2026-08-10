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
        let mut begin_new_panel_slide = false;
        let mut create_startup_panel = false;
        while let Ok(update) = updates.try_recv() {
            match update {
                harness::HarnessUpdate::Status(status) => self.model.status = status,
                harness::HarnessUpdate::ConnectionLost(message) => {
                    // The harness worker reattaches automatically. Keep the
                    // transcript and in-flight turn intact while it does so.
                    // Turning this into `Failed` used to add two scary error
                    // cards for one routine daemon reload.
                    self.model.status = message;
                    self.model
                        .set_notice("connection interrupted, reconnecting");
                }
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
                    let initial_attach = self.model.session_id.is_none();
                    let reconnected = self.model.failure.is_some();
                    // SessionNew is a panel creation, not a destructive clear of
                    // the panel under the pointer. Keep that old panel visible
                    // while the daemon creates its replacement, then reset the
                    // live page only when the new id actually attaches. Its
                    // transcript was cached by `new_session`, so it remains
                    // available as the left-hand neighbor during the slide.
                    if self.new_session_transition_pending
                        && self.model.session_id.as_deref() != Some(session_id.as_str())
                    {
                        Self::clear_model_for_session_change(&mut self.model);
                    }
                    self.model.status = format!("attached: {session_id}");
                    // A successful attach is the proof the failure is over: a
                    // reconnected window must not keep reporting the outage it
                    // just recovered from.
                    self.model.failure = None;
                    // A reconnect re-attaches the same session; the transcript
                    // on screen is the one that was being read, so it stays.
                    self.model.strips.focus_session(&session_id);
                    self.model.session_id = Some(session_id);
                    self.model.working_dir = working_dir;
                    if initial_attach && self.startup_panel_pending {
                        // A desktop window is a workspace rather than a lone
                        // conversation: create its right-hand neighbor as soon
                        // as the first panel really exists. Spend the flag before
                        // sending so a reconnect can never multiply panels.
                        self.startup_panel_pending = false;
                        create_startup_panel = true;
                    }
                    if reconnected {
                        self.model.set_notice("reconnected");
                    }
                    self.retitle();
                }
                harness::HarnessUpdate::Model { provider, model } => {
                    self.model.model = Some(ModelId { provider, model });
                }
                harness::HarnessUpdate::Models { models, current } => {
                    self.model.model_picker.set_models(models, current);
                }
                harness::HarnessUpdate::ModelSelected(model) => {
                    self.model.model_picker.mark_selected(model);
                }
                harness::HarnessUpdate::Text(text) => {
                    // The reply is now the visible proof of life. Retire the
                    // provisional "thinking" row (or the final tool row) so it
                    // does not linger below an answer that is already arriving.
                    self.model.transcript.clear_live_tool();
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
                harness::HarnessUpdate::Todo(card) => {
                    self.model.transcript.set_todo(&card);
                    self.model.stream.reveal_all();
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
                // A background task the agent is waiting on. The card lands in
                // the transcript's live status band rather than in the footnote:
                // the footnote is one line shared with failures and the model
                // caption, and a turn can be waiting on several tasks at once.
                harness::HarnessUpdate::Progress {
                    task_id,
                    label,
                    summary,
                    percent,
                    done,
                } => {
                    if done {
                        self.model.transcript.clear_progress(&task_id);
                    } else {
                        self.model
                            .transcript
                            .set_progress(&task_id, &label, &summary, percent);
                    }
                    // One clock for every bar on screen, started when the first
                    // card appears and stopped when the last one leaves: an
                    // indeterminate bar sweeps off it, and an idle window with
                    // no cards must not be asking for frames.
                    self.model.progress_clock = self.model.transcript.has_progress().then(|| {
                        self.model
                            .progress_clock
                            .unwrap_or(std::time::Instant::now())
                    });
                    // A card appearing changes the transcript's height, and the
                    // reveal counts characters, so the sweep is told about the
                    // new tail rather than being left to animate a stale one.
                    self.model.stream.extend_to(
                        self.model.transcript.streaming_len(),
                        std::time::Instant::now(),
                    );
                }
                harness::HarnessUpdate::TurnDone => {
                    self.model.busy = false;
                    self.model.activity.finish();
                    // The card shows the call in flight; the turn ending means
                    // there is none, and a card left behind would claim work
                    // is still happening.
                    self.model.transcript.clear_live_tool();
                    // Progress cards deliberately survive the turn: a
                    // backgrounded build keeps running after the agent stops
                    // waiting on it, and its own completion event is what
                    // retires the bar.
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
                    let had_panels = !self.model.strips.is_empty();
                    // Rebuild around the session we are actually attached to,
                    // so a refresh never silently moves the highlight off the
                    // conversation currently on screen.
                    self.model.strips =
                        strip::Strips::build(entries, self.model.session_id.as_deref());
                    if self.new_session_transition_pending
                        // A poll containing only the old panel can race ahead of
                        // Attached. It must not consume the pending transition.
                        && !self.model.status.starts_with("starting a new session")
                        && self.model.strips.focused_session().is_some()
                    {
                        // Creation changes both the daemon and the spatial
                        // workspace. Only start the camera once the new panel is
                        // actually in the strip; starting on keydown animated a
                        // one-panel row and visibly did nothing.
                        if had_panels {
                            begin_new_panel_slide = true;
                        }
                        self.new_session_transition_pending = false;
                    }
                }
            }
        }
        if begin_new_panel_slide {
            self.begin_workspace_transition(crate::workspace::Direction::Right);
        }
        if create_startup_panel {
            self.new_session();
        }
        self.model
            .file_tree
            .sync_root(self.model.working_dir.as_deref());
        // The turn is over and the channel is drained: if the user typed
        // while the agent was busy, the oldest waiting message goes now. Its
        // card leaves the queued tone, and the send is exactly the one the
        // submit would have made had the agent been free.
        if turn_ended && !self.model.busy {
            self.flush_queued_message();
        }
        // The horizontal workspace keeps neighboring pages visible even when
        // the overview is closed. Fetch their tails as soon as the session list
        // is known; `Peeks` deduplicates this call across ordinary redraws.
        self.request_peek();
    }

    /// Drop everything that belonged to the session being left.
    ///
    /// Shared by attaching to an existing session and by creating a new one:
    /// both put a different conversation on screen, and a transcript, a
    /// reveal, or a progress clock carried across from the old one would be
    /// output attributed to the wrong session.
    pub(crate) fn clear_for_session_change(&mut self) {
        Self::clear_model_for_session_change(&mut self.model);
    }

    fn clear_model_for_session_change(model: &mut crate::Model) {
        // Switching sessions changes the conversation, not the user's view
        // preferences: the thinking-display mode is carried across so a new
        // session does not silently revert to the structural default.
        let reasoning = model.transcript.reasoning_mode();
        model.transcript = transcript::Transcript::default();
        model.transcript.set_reasoning_mode(reasoning);
        model.stream.reveal_all();
        model.busy = false;
        model.activity.finish();
        // The fresh transcript took the bars with it (they belong to the
        // session being left), so the clock they animate off stops too, or an
        // empty page would keep asking for frames.
        model.progress_clock = None;
        model.scroll = 0.0;
        // Attaching is a jump, not a scroll: easing here would sweep through
        // the previous session's layout.
        model.smooth.settle();
        // A catalog and its open menu belong to the session that produced it.
        model.model_picker = crate::model_picker::Picker::default();
    }

    /// Start a fresh session and attach to it.
    ///
    /// The id is the daemon's to assign. Keep the current panel intact while
    /// creation is in flight, then clear the live page when `Attached` proves
    /// that the new panel exists. Clearing on keydown made this shortcut look
    /// like a destructive reset rather than a spatial panel creation.
    pub(crate) fn new_session(&mut self) {
        let Some((_, outgoing)) = self.harness.as_ref() else {
            self.model.notice = Some("not connected: cannot start a session".into());
            return;
        };
        if outgoing.send(harness::Command::New).is_err() {
            self.model.notice = Some("not connected: cannot start a session".into());
            return;
        }
        if let Some(current) = self.model.session_id.clone() {
            self.model
                .peeks
                .insert(&current, self.model.transcript.clone());
        }
        self.new_session_transition_pending = true;
        self.model.status = "starting a new session...".into();
    }

    /// Switch to whichever session the strip now points at.
    ///
    /// The transcript belongs to the session, so it is cleared rather than
    /// carried across: appending another conversation's output to the one on
    /// screen would be actively misleading. Reloading real history needs
    /// `GetHistory`; until that is wired, an empty page is the honest state.
    pub(crate) fn attach_focused_session(&mut self) {
        let Some(target) = self.model.strips.focused_session().map(str::to_string) else {
            return;
        };
        if self.model.session_id.as_deref() == Some(target.as_str()) {
            return;
        }
        // Once this model becomes a neighbor it must show the conversation the
        // user just left, not an older daemon peek. The live transcript is the
        // freshest cache entry and remains read-only while the target attaches.
        if let Some(current) = self.model.session_id.clone() {
            self.model
                .peeks
                .insert(&current, self.model.transcript.clone());
        }
        self.clear_for_session_change();
        self.model.status = format!("attaching: {target}");
        self.model.session_id = Some(target.clone());
        // The new session's directory arrives with its `Attached` event; until
        // then show the strip's entry rather than the previous session's path,
        // which would name the wrong project.
        self.model.working_dir = self.model.strips.focused_working_dir().map(str::to_string);
        self.retitle();
        if let Some((_, outgoing)) = self.harness.as_ref() {
            let _ = outgoing.send(harness::Command::Attach(target));
        }
    }
}
