use super::*;

impl SessionPicker {
    /// Snapshot the active-pid registry + streaming markers into the picker.
    pub(in crate::tui) fn refresh_live_presence(&mut self) {
        self.live_presence = crate::session::session_presence()
            .into_iter()
            .map(|presence| (presence.session_id.clone(), presence))
            .collect();
        if let Ok(sessions) = crate::claude_live::live_claude_sessions() {
            for session in sessions {
                let session_id = format!("claude:{}", session.session_id);
                self.live_presence.insert(
                    session_id.clone(),
                    crate::session::SessionPresence {
                        session_id,
                        pid: session.pid,
                        streaming: false,
                        streaming_since: None,
                        internal: false,
                    },
                );
            }
        }
        self.recent_window_closures =
            crate::session::recent_window_closures(std::time::Duration::from_secs(
                crate::config::config().display.resume_recent_close_seconds,
            ));
        self.live_presence_refreshed_at = Some(std::time::Instant::now());
    }

    /// Periodically re-snapshot live presence while the picker is on screen so
    /// working/ready badges track reality. Rebuilds the list when the Active
    /// view is showing and membership or streaming state changed (a session
    /// finished its turn, went idle, or exited). Returns true when anything
    /// changed so callers can redraw.
    pub fn maybe_refresh_live_presence(&mut self) -> bool {
        if self.loading_message.is_some() {
            return false;
        }
        let due = self
            .live_presence_refreshed_at
            .is_none_or(|at| at.elapsed() >= LIVE_PRESENCE_REFRESH_INTERVAL);
        if !due {
            return false;
        }
        let before = std::mem::take(&mut self.live_presence);
        let closures_before = std::mem::take(&mut self.recent_window_closures);
        self.refresh_live_presence();
        let closures_changed = closures_before != self.recent_window_closures;
        let changed = before != self.live_presence || closures_changed;
        if closures_changed || (changed && self.filter_mode == SessionFilterMode::Active) {
            self.rebuild_items();
        }
        changed
    }

    /// A recent final-client EOF, not recent messages or a daemon-owned Agent.
    pub(in crate::tui) fn session_recently_closed(&self, session: &SessionInfo) -> bool {
        if self.session_is_current(session) {
            return false;
        }
        let seconds = crate::config::config().display.resume_recent_close_seconds;
        seconds > 0
            && self
                .recent_window_closures
                .get(&session.id)
                .is_some_and(|at| {
                    chrono::Utc::now()
                        .signed_duration_since(*at)
                        .to_std()
                        .is_ok_and(|age| age <= std::time::Duration::from_secs(seconds))
                })
    }

    /// Whether the session has a live process right now.
    pub(in crate::tui) fn session_is_live(&self, session: &SessionInfo) -> bool {
        self.live_presence.contains_key(&session.id)
    }

    /// Whether the session's live process is streaming a model response.
    pub(in crate::tui) fn session_is_streaming(&self, session: &SessionInfo) -> bool {
        self.live_presence
            .get(&session.id)
            .is_some_and(|presence| presence.streaming)
    }

    /// How long the session's current streaming turn has been running.
    pub(in crate::tui) fn session_streaming_duration(
        &self,
        session: &SessionInfo,
    ) -> Option<std::time::Duration> {
        self.live_presence
            .get(&session.id)
            .and_then(|presence| presence.streaming_since)
            .and_then(
                |since| match std::time::SystemTime::now().duration_since(since) {
                    Ok(duration) => Some(duration),
                    Err(error) => {
                        // Session-presence timestamps can be in the future after clock drift.
                        crate::logging::debug(&format!(
                            "Cannot display future session streaming timestamp: {error}"
                        ));
                        None
                    }
                },
            )
    }
}
