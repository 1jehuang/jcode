//! The session-overview gesture on the `App`: opening the field, moving the
//! highlight, committing or cancelling, and pacing the zoom.
//!
//! Split from `main.rs` for the same reason as `app_selection.rs`: these
//! methods are one concern (a held-Super gesture over the blob field) and
//! they read as a unit, while interleaved with window and harness plumbing
//! they read as neither. The drawing half lives in `crate::scene_overview`.

use crate::{App, OVERVIEW_FRAME, SUPER_BOUNCE, SUPER_TAP, harness, keymap, overview};

impl App {
    /// Lay out the blob field for the current model.
    ///
    /// Rebuilt per call rather than cached: it is pure arithmetic over a
    /// handful of sessions, and a cache here would be one more thing that can
    /// disagree with what was drawn when a poll changes the session set
    /// mid-gesture.
    pub(crate) fn overview_field(&self) -> overview::Field {
        overview::layout(
            &self.model.strips.panels(),
            self.model
                .overview
                .focus()
                .or(self.model.session_id.as_deref()),
            self.model.session_id.as_deref(),
            overview::area(&self.frame),
        )
    }

    /// Open the overview, highlighting the session we are in so the gesture
    /// starts from where the user already is.
    pub(crate) fn open_overview(&mut self) {
        if self.model.overview.is_open() {
            return;
        }
        self.model.overview.open(self.model.session_id.as_deref());
        self.request_peek();
        self.request_redraw();
    }

    /// Fetch the tail of every session in the field, so each card can show the
    /// conversation it holds rather than only its name.
    ///
    /// The whole field rather than only the highlight: the point of the
    /// overview is comparing sessions, and a strip where you must visit each
    /// card to learn what is in it is a list you have to read one item at a
    /// time. Requests are deduplicated by [`overview::Peeks`], so opening the
    /// field costs one read per session for the whole gesture rather than one
    /// per frame, and the highlight is asked for first so the card the eye is
    /// already on fills in before its neighbours.
    pub(crate) fn request_peek(&mut self) {
        for target in self.peek_targets() {
            if !self.model.peeks.should_request(&target) {
                continue;
            }
            if let Some((_, outgoing)) = self.harness.as_ref() {
                let _ = outgoing.send(harness::Command::Peek(target));
            }
        }
    }

    /// Which sessions a peek pass covers, highlight first.
    ///
    /// Split out so the *policy* (the whole field, not just the highlight) is
    /// testable without a live connection: the send itself needs a socket, and
    /// a test that had to stand one up would end up asserting on plumbing
    /// rather than on which sessions get previewed.
    pub(crate) fn peek_targets(&self) -> Vec<String> {
        // The highlight first so the card the eye is already on fills in before
        // its neighbours.
        self.model
            .overview
            .focus()
            .or(self.model.session_id.as_deref())
            .map(str::to_string)
            .into_iter()
            .chain(
                self.model
                    .strips
                    .panels()
                    .into_iter()
                    .map(|entry| entry.session_id),
            )
            .collect()
    }

    /// Close the field, attaching to the highlighted session when `commit`.
    ///
    /// Commit and cancel share one path so they can never drift: the only
    /// difference is whether the highlight is acted on.
    pub(crate) fn close_overview(&mut self, commit: bool) {
        if !self.model.overview.is_visible() {
            return;
        }
        if commit
            && let Some(target) = self.model.overview.focus().map(str::to_string)
            && self.model.session_id.as_deref() != Some(target.as_str())
            && self.model.strips.focus_session(&target)
        {
            self.attach_focused_session();
        }
        self.model.overview.close();
        self.request_redraw();
    }

    /// Move the highlight across the field.
    pub(crate) fn move_overview(&mut self, dir: overview::Dir) {
        let field = self.overview_field();
        let from = self
            .model
            .overview
            .focus()
            .or(self.model.session_id.as_deref())
            .unwrap_or_default()
            .to_string();
        if let Some(target) = field.neighbor(&from, dir) {
            let id = target.session_id.clone();
            self.model.overview.set_focus(&id);
            self.request_peek();
            self.request_redraw();
            return;
        }
        // Nothing that way. If the axis could never move here (one project
        // row makes j/k dead, a row of one makes h/l dead), a motion key that
        // does nothing reads as broken, so fall back to the reading-order
        // step in the same sense. But an *edge* of a live axis is a wall the
        // highlight should stop at: wrapping there made the key look like a
        // teleport, so stay put instead.
        if !field.axis_is_dead(&from, dir) {
            return;
        }
        let step = match dir {
            overview::Dir::Left | overview::Dir::Up => -1,
            overview::Dir::Right | overview::Dir::Down => 1,
        };
        self.cycle_overview(step);
    }

    /// Step the highlight through the field in reading order.
    pub(crate) fn cycle_overview(&mut self, step: isize) {
        let field = self.overview_field();
        let from = self
            .model
            .overview
            .focus()
            .or(self.model.session_id.as_deref())
            .unwrap_or_default()
            .to_string();
        if let Some(target) = field.next_in_order(&from, step) {
            let id = target.to_string();
            self.model.overview.set_focus(&id);
            self.request_peek();
            self.request_redraw();
        }
    }

    /// Super went down or came up.
    ///
    /// Pressing it opens the session overview at once, and releasing it
    /// commits to the highlighted session unless the press was a tap shorter
    /// than [`SUPER_TAP`]. Tracked from the modifier event rather than from a
    /// key event because a bare modifier never produces one of its own.
    ///
    /// A named method rather than an inline arm so the tests drive the same
    /// code the window does: a test that reimplemented this gesture would keep
    /// passing after the real handler stopped opening anything.
    pub(crate) fn on_super_changed(&mut self, down: bool, now: std::time::Instant) {
        if !self.super_overview && !self.model.overview.is_visible() {
            // The gesture is benched: Super stays a plain chord modifier.
            // The visibility check keeps release handling sane if the flag
            // is ever flipped off while the field is up.
            return;
        }
        if down {
            // A press inside the bounce window is a remapper's re-press around
            // a key it rewrote, not the user starting a new gesture: drop the
            // pending release and carry on with the gesture already running.
            if self.pending_super_release.take().is_some() {
                return;
            }
            if self.super_held_since.is_none() && !self.model.overview.is_open() {
                self.super_held_since = Some(now);
                self.open_overview();
            }
            return;
        }
        let tap = self
            .super_held_since
            .is_some_and(|since| now.duration_since(since) < SUPER_TAP);
        if !self.model.overview.is_open() {
            // Enter or Escape already resolved the gesture while Super was
            // still down; the release must not commit a second time.
            self.super_held_since = None;
            return;
        }
        // Do not resolve yet: a remapper may be about to press Super straight
        // back down. [`Self::settle_super_release`] finishes the job once the
        // bounce window has passed with no re-press.
        self.pending_super_release = Some((now, tap));
    }

    /// Resolve a Super release that has survived the bounce window.
    ///
    /// Called from the frame rather than from a timer thread for the same
    /// reason the zoom is: one clock drives everything on screen, and a window
    /// that is not drawing is not silently switching sessions either.
    pub(crate) fn settle_super_release(&mut self, now: std::time::Instant) {
        let Some((at, tap)) = self.pending_super_release else {
            return;
        };
        if now.duration_since(at) < SUPER_BOUNCE {
            return;
        }
        self.pending_super_release = None;
        self.super_held_since = None;
        if !self.model.overview.is_open() {
            return;
        }
        if tap && self.model.overview.focus() == self.model.session_id.as_deref() {
            // A bare tap on Super, with the highlight never moved: the user
            // was reaching for a chord they did not finish, not switching
            // session. Take the field straight off screen rather than
            // zooming it out.
            self.model.overview.abort();
            self.request_redraw();
            return;
        }
        // Releasing Super flies into whichever blob is highlighted.
        self.close_overview(true);
    }

    /// A key went down while the field is open.
    ///
    /// hjkl, the arrows, Tab and Enter drive the field. Anything else means
    /// the user was reaching for a chord, not gesturing: the field comes
    /// straight off screen and the chord does what it always does. A key
    /// that resolves to nothing is swallowed rather than typed, because text
    /// arriving in a composer the user cannot see would be worse than a dead
    /// key.
    ///
    /// Returns false when the chord asked the app to quit, mirroring
    /// [`Self::apply`].
    pub(crate) fn overview_keydown(
        &mut self,
        logical_key: &winit::keyboard::Key,
        typed: Option<&str>,
    ) -> bool {
        // Shift+Tab steps backwards, the Alt+Tab habit.
        let action = if self.modifiers.shift_key()
            && *logical_key == winit::keyboard::Key::Named(winit::keyboard::NamedKey::Tab)
        {
            Some(keymap::Action::OverviewPrev)
        } else {
            keymap::resolve_overview(logical_key)
        };
        if let Some(action) = action {
            let keep = self.apply(action, None);
            if std::env::var_os("JCODE_DESKTOP2_LOG_INPUT").is_some() {
                eprintln!(
                    "[input] overview {action:?} -> focus {:?}",
                    self.model.overview.focus()
                );
            }
            self.request_redraw();
            return keep;
        }
        // The field opened on the Super keydown, so this key is the second
        // half of a chord the user already knew. Abort rather than zoom out:
        // blobs washing over the composer after the user has moved on reads
        // as lag, not as an animation.
        self.super_held_since = None;
        self.pending_super_release = None;
        self.model.overview.abort();
        let keep = match keymap::resolve(logical_key, self.modifiers) {
            Some(action) => self.apply(action, typed),
            None => true,
        };
        self.request_redraw();
        keep
    }

    /// A key went down: the whole keyboard path, field or not.
    ///
    /// Extracted from the window arm so a headless script can drive exactly
    /// what the compositor drives. The overview gesture is the one part of
    /// this app whose bugs live entirely in dispatch order, and synthetic
    /// compositor input is too flaky to be the only way to check it.
    pub(crate) fn key_pressed(
        &mut self,
        logical_key: &winit::keyboard::Key,
        typed: Option<&str>,
    ) -> bool {
        // Reload is application chrome rather than overlay input. Resolve it
        // before an open help card, picker, or overview owns the keyboard.
        if keymap::resolve(logical_key, self.modifiers) == Some(keymap::Action::ManualReload) {
            return self.apply(keymap::Action::ManualReload, typed);
        }
        // F1 is application help rather than input for whichever picker happens
        // to be open. Let it rise above existing overlays so it is always a
        // reliable, discoverable way to ask what the window can do.
        if !self.model.help_open
            && keymap::resolve(logical_key, self.modifiers) == Some(keymap::Action::ToggleHelp)
        {
            self.apply(keymap::Action::ToggleHelp, typed);
            self.request_redraw();
            return true;
        }
        if self.model.help_open {
            if let Some(action) = keymap::resolve_help(logical_key) {
                self.apply(action, typed);
            }
            // The help card is modal. Unknown keys are consumed instead of
            // leaking into the composer hidden beneath it.
            self.request_redraw();
            return true;
        }
        if self.model.resume.is_open() {
            return self.resume_keydown(logical_key, typed);
        }
        if self.model.model_picker.is_open() {
            return self.model_picker_keydown(logical_key);
        }
        if self.model.overview.is_open() {
            return self.overview_keydown(logical_key, typed);
        }
        // A key landing while the field is still zooming out erases it in the
        // same frame: the user is typing, not gesturing.
        self.super_held_since = None;
        self.pending_super_release = None;
        if self.model.overview.is_visible() {
            self.model.overview.abort();
        }
        let action = keymap::resolve(logical_key, self.modifiers).unwrap_or(keymap::Action::Insert);
        let keep = self.apply(action, typed);
        self.request_redraw();
        keep
    }

    /// Advance the overview's zoom one frame.
    ///
    /// Done on the frame rather than on a timer thread so the zoom is driven
    /// by the same clock as everything else on screen, and so a window that is
    /// not drawing is not silently animating either.
    pub(crate) fn tick_overview(&mut self, now: std::time::Instant) {
        // Its own clock, not the donut's: the donut stops advancing the
        // moment there is a transcript, and a zoom driven by a stopped clock
        // would freeze halfway in exactly the sessions worth switching to.
        let dt = self
            .overview_frame
            .map(|last| now.duration_since(last).as_secs_f32().min(0.1))
            .unwrap_or(OVERVIEW_FRAME.as_secs_f32());
        self.overview_frame = Some(now);
        if self.model.overview.advance(dt) {
            self.request_redraw();
        }
    }
}
