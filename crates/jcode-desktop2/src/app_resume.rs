//! The resume gesture on the `App`: scanning the session store, driving the
//! picker, and attaching to whatever the user lands on.
//!
//! Split from `main.rs` for the same reason as `app_overview.rs`: opening a
//! panel, walking it, previewing a row, and committing is one concern that
//! reads as a unit. The pure state lives in [`crate::resume`], the drawing in
//! [`crate::scene_resume`], and this is the only place the three meet.

use crate::{App, harness, keymap, resume};

impl App {
    /// Open or shut the picker, kicking off a scan when it opens.
    ///
    /// The scan runs on a worker thread and reports back through the harness
    /// channel-alike used for everything else asynchronous: the store holds
    /// tens of thousands of records on a machine that has been used for
    /// months, and reading them on the UI thread would freeze the window for
    /// the whole of the gesture that is supposed to feel instant.
    pub(crate) fn toggle_resume(&mut self) {
        if self.model.resume.is_open() {
            self.model.resume.close();
            self.request_redraw();
            return;
        }
        // A settings panel and a full-page overlay must not be up at once: the
        // panel would float over the picker with no way to reach it.
        self.model.panel.close();
        let scanning = self.start_resume_scan();
        self.model.resume.open(scanning);
        // Preview whatever the highlight starts on, if the last scan is still
        // in the model: reopening the picker should not blank the preview it
        // showed a moment ago.
        self.request_resume_peek();
        self.request_redraw();
    }

    /// Kick off a scan of the session store. Returns whether one started.
    fn start_resume_scan(&mut self) -> bool {
        let Some(dir) = resume::sessions_dir() else {
            self.model
                .set_notice("cannot find the session store: no HOME");
            return false;
        };
        let Some(sender) = self.resume_scans.as_ref().map(|(sender, _)| sender.clone()) else {
            return false;
        };
        std::thread::spawn(move || {
            // The receiver is dropped when the window closes, which is not a
            // failure: the scan simply has nobody left to tell.
            let _ = sender.send(resume::scan(&dir, resume::SCAN_LIMIT));
        });
        true
    }

    /// Fold a finished scan into the model. Called from the frame loop, beside
    /// the harness drain, so both asynchronous sources land at one point.
    pub(crate) fn drain_resume_scans(&mut self) {
        let Some((_, receiver)) = self.resume_scans.as_ref() else {
            return;
        };
        let mut landed = false;
        while let Ok(records) = receiver.try_recv() {
            self.model.resume.set_records(records);
            landed = true;
        }
        if landed {
            // The highlight may now be on a different session than the one
            // whose tail we fetched, so ask for the new one.
            self.request_resume_peek();
            self.request_redraw();
        }
    }

    /// Fetch the highlighted stored session's tail, if it is not cached.
    ///
    /// The same [`crate::overview::Peeks`] cache the overview uses, and the
    /// same `PeekSession` request: a stored session and a live one are
    /// previewed by one code path, so they can never disagree about what a
    /// preview looks like.
    pub(crate) fn request_resume_peek(&mut self) {
        let Some(target) = self
            .model
            .resume
            .selected()
            .map(|record| record.session_id.clone())
        else {
            return;
        };
        if !self.model.peeks.should_request(&target) {
            return;
        }
        if let Some((_, outgoing)) = self.harness.as_ref() {
            let _ = outgoing.send(harness::Command::Peek(target));
        }
    }

    /// Apply one picker action. Split out of `App::apply` so the overlay's
    /// bindings read as one table rather than as arms scattered through the
    /// composer's.
    pub(crate) fn apply_resume(&mut self, action: keymap::Action, typed: Option<&str>) {
        use keymap::Action;
        match action {
            Action::ResumeUp => self.model.resume.move_cursor(-1),
            Action::ResumeDown => self.model.resume.move_cursor(1),
            Action::ResumeGroupUp => self.model.resume.move_group(-1),
            Action::ResumeGroupDown => self.model.resume.move_group(1),
            Action::ResumeCollapse => self.model.resume.collapse(),
            Action::ResumeExpand => self.model.resume.expand(),
            Action::ResumeBackspace => self.model.resume.backspace(),
            Action::ResumeType => {
                // Only real text narrows the list: a control character would
                // otherwise become a query nothing can match.
                for ch in typed.unwrap_or_default().chars() {
                    if !ch.is_control() {
                        self.model.resume.type_char(ch);
                    }
                }
            }
            Action::ResumeCancel => self.model.resume.close(),
            Action::ResumeCommit => self.commit_resume(),
            _ => {}
        }
        // Whatever the highlight is on now is what the preview must show.
        self.request_resume_peek();
        self.request_redraw();
    }

    /// Attach to the highlighted stored session and close the overlay.
    ///
    /// A group row commits nothing: it is a heading, so Enter opens it rather
    /// than guessing which of its sessions was meant.
    fn commit_resume(&mut self) {
        let Some(record) = self.model.resume.selected().cloned() else {
            self.model.resume.expand();
            return;
        };
        self.model.resume.close();
        if self.model.session_id.as_deref() == Some(record.session_id.as_str()) {
            self.model.set_notice("already in that session".to_string());
            return;
        }
        let Some((_, outgoing)) = self.harness.as_ref() else {
            self.model.set_notice("not connected: cannot resume");
            return;
        };
        // The daemon resumes a stored session on attach, so this is the same
        // request the strip makes; what differs is only that the target came
        // from disk rather than from the live list.
        if outgoing
            .send(harness::Command::Attach(record.session_id.clone()))
            .is_err()
        {
            self.model.set_notice("not connected: cannot resume");
            return;
        }
        self.clear_for_session_change();
        self.model.status = format!("resuming {}...", record.label());
        self.model.session_id = Some(record.session_id.clone());
        // The directory is known from the record, so the masthead names the
        // right project immediately rather than showing the previous one until
        // the `Attached` event lands.
        self.model.working_dir = record.working_dir.clone();
        self.retitle();
    }

    /// The absolute row index under a logical point, or `None` off the list.
    ///
    /// The list scrolls, so a visible slot is not a row index: the same
    /// [`crate::scene_resume::window_start`] the renderer uses converts one to
    /// the other, which is what keeps the row that lights up the row that
    /// fires.
    pub(crate) fn resume_row_under(&self, x: f64, y: f64) -> Option<usize> {
        let total = self.model.resume.rows().len();
        let slot = self.frame.resume_row_at(total, x, y)?;
        let start = crate::scene_resume::window_start(
            self.model.resume.cursor(),
            total,
            self.frame.resume_visible_rows_for(total),
        );
        let row = start + slot;
        (row < total).then_some(row)
    }

    /// A key went down while the picker is up.
    ///
    /// The overlay owns the keyboard: an unbound key is swallowed rather than
    /// typed, because text landing in a composer hidden behind the panel is
    /// worse than a dead key.
    pub(crate) fn resume_keydown(
        &mut self,
        logical_key: &winit::keyboard::Key,
        typed: Option<&str>,
    ) -> bool {
        match keymap::resolve_resume(logical_key, self.modifiers) {
            // The toggle is the one action that belongs to the app rather than
            // to the picker, so it goes through the ordinary path.
            Some(keymap::Action::ToggleResume) => {
                self.toggle_resume();
            }
            Some(action) => self.apply_resume(action, typed),
            None => {}
        }
        true
    }
}
