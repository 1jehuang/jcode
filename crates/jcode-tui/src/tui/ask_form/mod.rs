//! TUI `ask_user` question form.
//!
//! This module is deliberately self-contained: it owns state, rendering, key
//! handling, paste handling and reconnect reconciliation, and is wired into
//! the rest of the TUI through a handful of calls from `ui.rs`,
//! `key_events.rs`, `input.rs` and `server_events.rs`.
//!
//! State is a `thread_local!` scoped to one remote-loop instance, one
//! connection generation and one session. There is no `App` field.
//!
//! v1 renders tagged forms only: the schema root must carry
//! `_meta["jcode.dev/askUser"]`. Untagged and applet forms are ignored without
//! a response, so another attached client can still answer them.

mod keys;
mod paste;
mod reconcile;
mod render;
mod schema;
mod state;

#[cfg(test)]
mod tests;

use ratatui::crossterm::event::{KeyCode, KeyModifiers};

use crate::protocol::ServerEvent;
use crate::tui::TuiState;
use crate::tui::app::App;

use state::State;

/// Outcome of routing a key into the form.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum KeyOutcome {
    /// The form is not focused (or must not consume this key); later handlers
    /// should run normally.
    Ignored,
    /// The form consumed the key and nothing else should.
    Consumed,
    /// Send this response; `record_response_id` follows a successful send.
    Send {
        session_id: String,
        elicitation_id: String,
        response: crate::protocol::elicitation::ElicitationResponse,
    },
}

/// A focused view of the app surface the module is allowed to consult.
///
/// Render, keys and paste all build this through [`FocusProbe`]. Keeping the
/// predicate in one place is what makes them agree about focus.
#[derive(Debug, Clone, Default)]
pub(super) struct ViewContext {
    pub session: Option<String>,
    pub replay: bool,
    pub blocked: Option<&'static str>,
}

impl ViewContext {
    pub(super) fn from_probe(probe: &(impl FocusProbe + ?Sized)) -> Self {
        Self {
            session: probe.probe_session(),
            replay: probe.probe_replay(),
            blocked: focus_block(probe),
        }
    }

    /// The form may render, capture keys and take paste only when it belongs
    /// to the current session, its connection is attached and no other UI
    /// owns input.
    pub(in crate::tui::ask_form) fn eligible(&self, state: &State) -> bool {
        !self.replay
            && self.blocked.is_none()
            && state.attached
            && state.loop_live
            && state.session.is_some()
            && state.session == self.session
    }
}

/// The UI modes that outrank the question form. Ordered by precedence, matching
/// the TUI's key handlers and `draw_inner`'s overlays.
fn focus_block(probe: &(impl FocusProbe + ?Sized)) -> Option<&'static str> {
    // The failover prompt owns keys before every other route.
    if probe.probe_provider_failover() {
        return Some("provider failover");
    }
    if probe.probe_prompt_history() {
        return Some("history search");
    }
    if probe.probe_onboarding() {
        return Some("onboarding");
    }
    if probe.probe_copy_selection() {
        return Some("copy selection");
    }
    if probe.probe_commit_log() {
        return Some("commit log");
    }
    if probe.probe_changelog() {
        return Some("changelog");
    }
    if probe.probe_help() {
        return Some("help");
    }
    if probe.probe_model_status() {
        return Some("model status");
    }
    if probe.probe_panel_image() {
        return Some("image preview");
    }
    if probe.probe_session_picker() {
        return Some("session picker");
    }
    if probe.probe_login_picker() {
        return Some("login picker");
    }
    if probe.probe_account_picker() {
        return Some("account picker");
    }
    if probe.probe_usage_overlay() {
        return Some("usage overlay");
    }
    if probe.probe_inline_interactive() {
        return Some("picker");
    }
    if probe.probe_pending_login() {
        return Some("login");
    }
    if probe.probe_swarm_page() {
        return Some("swarm page");
    }
    if probe.probe_side_panel_fullscreen() {
        return Some("side panel");
    }
    None
}

/// Everything the focus predicate reads, expressed as booleans so tests can
/// exercise every precedence source without constructing a full `App`.
pub(super) trait FocusProbe {
    fn probe_session(&self) -> Option<String>;
    fn probe_replay(&self) -> bool;
    fn probe_prompt_history(&self) -> bool;
    fn probe_onboarding(&self) -> bool;
    fn probe_copy_selection(&self) -> bool;
    fn probe_commit_log(&self) -> bool;
    fn probe_changelog(&self) -> bool;
    fn probe_help(&self) -> bool;
    fn probe_model_status(&self) -> bool;
    fn probe_panel_image(&self) -> bool;
    fn probe_session_picker(&self) -> bool;
    fn probe_login_picker(&self) -> bool;
    fn probe_account_picker(&self) -> bool;
    fn probe_usage_overlay(&self) -> bool;
    fn probe_inline_interactive(&self) -> bool;
    fn probe_pending_login(&self) -> bool;
    fn probe_swarm_page(&self) -> bool;
    fn probe_side_panel_fullscreen(&self) -> bool;
    fn probe_provider_failover(&self) -> bool;
}

impl<T: TuiState + ?Sized> FocusProbe for T {
    fn probe_session(&self) -> Option<String> {
        self.current_session_id()
    }
    fn probe_replay(&self) -> bool {
        TuiState::is_replay(self)
    }
    fn probe_prompt_history(&self) -> bool {
        self.prompt_history_search().is_some()
    }
    fn probe_onboarding(&self) -> bool {
        self.onboarding_welcome_active()
    }
    fn probe_copy_selection(&self) -> bool {
        self.copy_selection_mode()
    }
    fn probe_commit_log(&self) -> bool {
        self.commit_log_overlay().is_some()
    }
    fn probe_changelog(&self) -> bool {
        self.changelog_scroll().is_some()
    }
    fn probe_help(&self) -> bool {
        self.help_scroll().is_some()
    }
    fn probe_model_status(&self) -> bool {
        self.model_status_overlay().is_some()
    }
    fn probe_panel_image(&self) -> bool {
        self.panel_image_preview().is_some()
    }
    fn probe_session_picker(&self) -> bool {
        self.session_picker_overlay().is_some()
    }
    fn probe_login_picker(&self) -> bool {
        self.login_picker_overlay().is_some()
    }
    fn probe_account_picker(&self) -> bool {
        self.account_picker_overlay().is_some()
    }
    fn probe_usage_overlay(&self) -> bool {
        self.usage_overlay().is_some()
    }
    fn probe_inline_interactive(&self) -> bool {
        self.inline_interactive_state().is_some()
    }
    fn probe_pending_login(&self) -> bool {
        self.login_input_hint().is_some()
    }
    fn probe_swarm_page(&self) -> bool {
        self.swarm_panel_full_page()
    }
    fn probe_side_panel_fullscreen(&self) -> bool {
        self.side_panel_fullscreen()
    }
    fn probe_provider_failover(&self) -> bool {
        self.pending_provider_failover_active()
    }
}

/// Route a key into the focused form. Never holds a state borrow across an
/// await: this is a synchronous state transition that returns what to send.
///
/// Ctrl-modified keys are never consumed, so Ctrl-C still reaches the normal
/// interrupt handler.
pub(crate) fn handle_key(
    app: &mut App,
    code: KeyCode,
    modifiers: KeyModifiers,
    text_input: Option<&str>,
) -> KeyOutcome {
    let context = ViewContext::from_probe(app);
    let result =
        state::with(|state| keys::handle_key(state, &context, code, modifiers, text_input));
    if let Some(notice) = result.notice {
        app.set_status_notice(notice);
    }
    result.outcome
}

/// Route pasted text into the focused form editor, or open the selected Other
/// editor. Returns false so normal paste routing continues when the form has
/// no text target.
pub(crate) fn handle_paste(app: &mut App, text: &str) -> bool {
    let context = ViewContext::from_probe(app);
    let result = state::with(|state| paste::handle_paste(state, &context, text));
    if let Some(notice) = result.notice {
        app.set_status_notice(notice);
    }
    result.consumed
}

/// Consume server events owned by this module.
///
/// Must not respond while replaying history (`app.is_replay()`). `History` is
/// deliberately not replay completion: same-session refreshes preserve forms,
/// drafts and response ownership.
pub(crate) fn on_event(app: &mut App, event: &ServerEvent) -> bool {
    if let ServerEvent::ElicitationReplayComplete { session_id } = event {
        // Reconcile at the app level so the exported entry point owns the
        // state borrow. Recorded history never answers or discards drafts.
        if TuiState::is_replay(app) {
            return false;
        }
        on_replay_complete(app, session_id);
        return true;
    }
    let context = ViewContext::from_probe(app);
    let result = state::with(|state| reconcile::handle_event(state, &context, event));
    if let Some(notice) = result.notice {
        app.set_status_notice(notice);
    }
    result.consumed
}

/// Render at the end of `draw_inner`. Takes `&dyn TuiState`, never `App`.
pub(crate) fn render(frame: &mut ratatui::Frame, app: &dyn TuiState) {
    let context = ViewContext::from_probe(app);
    let area = frame.area();
    state::with(|state| render::render_view(frame, state, &context, area));
}

/// Record the request id returned by `send_elicitation_response`.
pub(crate) fn record_response_id(app: &mut App, request_id: u64) {
    let _ = app;
    state::with(|state| reconcile::record_response_id(state, request_id));
}

/// A transport send failure: restore editing, keep the draft, show a notice.
pub(crate) fn send_failed(app: &mut App, message: &str) {
    let context = ViewContext::from_probe(app);
    let applied = state::with(|state| reconcile::send_failed(state, &context, message));
    if applied {
        app.set_status_notice(format!("Could not send the answer: {message}"));
    }
}

/// Reconciliation entry point for `ServerEvent::ElicitationReplayComplete`.
pub(crate) fn on_replay_complete(app: &mut App, session_id: &str) {
    let context = ViewContext::from_probe(app);
    let notice = state::with(|state| reconcile::on_replay_complete(state, &context, session_id));
    if let Some(notice) = notice {
        app.set_status_notice(notice);
    }
}

/// A remote loop starts with a clean scope.
pub(crate) fn on_remote_loop_start(app: &mut App) {
    let session = ViewContext::from_probe(app).session;
    state::with(|state| {
        state.reset();
        state.loop_live = true;
        state.generation = 1;
        state.attached = true;
        state.session = session;
    });
}

/// The remote loop ended. Nothing survives into the next loop or the
/// non-remote path.
pub(crate) fn on_remote_loop_exit() {
    state::with(State::reset);
}

/// The transport dropped. Hide the panel and release keys and paste until the
/// reconnect succeeds, but keep drafts for replay.
pub(crate) fn on_connection_lost() {
    state::with(|state| state.attached = false);
}

/// A new attachment replaces the connection: hide the panel, freeze drafts and
/// drop response ownership from the old generation.
pub(crate) fn on_connection_replaced(app: &mut App) {
    let session = ViewContext::from_probe(app).session;
    let reopened = state::with(|state| reconcile::connection_replaced(state, session.as_deref()));
    if reopened > 0 {
        app.set_status_notice(format!(
            "{reopened} answer(s) were not acknowledged before the connection changed; review and submit again."
        ));
    }
}

#[cfg(test)]
pub(crate) use state::reset_for_test;
