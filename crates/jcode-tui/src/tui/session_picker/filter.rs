use super::loading::session_matches_picker_query;
use super::*;

impl SessionPicker {
    fn normalized_search_query(query: &str) -> String {
        query.trim().to_lowercase()
    }

    /// Check if a session matches the current search query.
    fn session_matches_search(session: &SessionInfo, query: &str) -> bool {
        session_matches_picker_query(session, query)
    }

    fn all_session_refs(&self) -> Vec<SessionRef> {
        let mut refs = Vec::new();
        if !self.all_server_groups.is_empty() {
            for (group_idx, group) in self.all_server_groups.iter().enumerate() {
                refs.extend(
                    (0..group.sessions.len()).map(|session_idx| SessionRef::Group {
                        group_idx,
                        session_idx,
                    }),
                );
            }
            refs.extend((0..self.all_orphan_sessions.len()).map(SessionRef::Orphan));
        } else {
            refs.extend((0..self.all_sessions.len()).map(SessionRef::Flat));
        }
        refs
    }

    fn search_matched_session_refs(&mut self, query: &str) -> Vec<SessionRef> {
        let normalized = Self::normalized_search_query(query);
        let mut transcript_search = std::mem::take(&mut self.transcript_search);
        transcript_search.reset(
            &normalized,
            self.all_sessions
                .iter()
                .chain(self.all_orphan_sessions.iter())
                .chain(
                    self.all_server_groups
                        .iter()
                        .flat_map(|group| group.sessions.iter()),
                ),
        );
        self.transcript_search = transcript_search;
        if normalized.is_empty() {
            self.cached_search_query.clear();
            self.cached_search_refs.clear();
            return self.all_session_refs();
        }

        // Fuzzy matches are not monotonic: adding a character can increase the
        // error budget or turn a short literal query into a typo-tolerant one.
        // Reconsider every session rather than narrowing the previous results.
        let candidates = self.all_session_refs();
        let fuzzy = jcode_fuzzy::PreparedTokenQuery::new(&normalized);

        let matches = candidates
            .into_iter()
            .filter(|session_ref| {
                self.session_by_ref(*session_ref).is_some_and(|session| {
                    fuzzy.matches_search_text(&session.search_index)
                        || self.transcript_search.contains(&session.id)
                })
            })
            .collect::<Vec<_>>();

        self.cached_search_query = normalized;
        self.cached_search_refs = matches.clone();
        matches
    }

    /// Called once per rendered frame, so completed scans update results even
    /// when the user is not typing. Never wait for the worker on the UI thread.
    pub(super) fn poll_transcript_search(&mut self) {
        if self.transcript_search.poll() {
            let selected = self.selected_session().map(|session| session.id.clone());
            let preview_scroll = self.scroll_offset;
            let auto_scroll = self.auto_scroll_preview;
            self.cached_search_query.clear();
            self.cached_search_refs.clear();
            self.rebuild_items();
            if self.selected_session().map(|session| &session.id) == selected.as_ref() {
                self.scroll_offset = preview_scroll;
                self.auto_scroll_preview = auto_scroll;
            }
        }
    }

    fn filtered_session_refs(
        &self,
        search_matches: &[SessionRef],
        show_test: bool,
        filter_mode: SessionFilterMode,
    ) -> Vec<SessionRef> {
        let mut filtered = search_matches
            .iter()
            .copied()
            .filter(|session_ref| {
                self.session_by_ref(*session_ref).is_some_and(|session| {
                    (show_test || !session.is_debug)
                        && self.session_matches_filter_mode(session, filter_mode)
                })
            })
            .collect::<Vec<_>>();

        filtered.sort_by(|a, b| {
            let a = self
                .session_by_ref(*a)
                .map(|session| session.last_message_time)
                .unwrap_or_default();
            let b = self
                .session_by_ref(*b)
                .map(|session| session.last_message_time)
                .unwrap_or_default();
            b.cmp(&a)
        });
        // Preserve recency within each tier, but literal matches should not
        // be buried beneath newer sessions matching only through a typo.
        if !self.search_query.trim().is_empty() {
            filtered.sort_by_cached_key(|session_ref| {
                !self.session_by_ref(*session_ref).is_some_and(|session| {
                    Self::session_matches_search(session, &self.search_query)
                        || self.transcript_search.contains(&session.id)
                })
            });
        }
        if filter_mode != SessionFilterMode::Active && self.search_query.trim().is_empty() {
            filtered.sort_by_key(|session_ref| {
                std::cmp::Reverse(self.session_by_ref(*session_ref).and_then(|session| {
                    self.session_recently_closed(session)
                        .then(|| self.recent_window_closures.get(&session.id).copied())
                        .flatten()
                }))
            });
        }
        if filter_mode == SessionFilterMode::Active {
            // Triage order for the active sessions manager: sessions that are
            // ready for input (done streaming) float above ones still working,
            // each group keeping the recency order from the sort above.
            filtered.sort_by_key(|session_ref| {
                self.session_by_ref(*session_ref)
                    .map(|session| self.session_is_streaming(session))
                    .unwrap_or(false)
            });
        }
        filtered
    }

    fn hidden_test_count_for_refs(
        &self,
        refs: &[SessionRef],
        show_test: bool,
        filter_mode: SessionFilterMode,
    ) -> usize {
        if show_test {
            return 0;
        }
        refs.iter()
            .filter_map(|session_ref| self.session_by_ref(*session_ref))
            .filter(|session| {
                session.is_debug && self.session_matches_filter_mode(session, filter_mode)
            })
            .count()
    }

    fn visible_session_ids(&self) -> std::collections::HashSet<String> {
        self.visible_sessions
            .iter()
            .filter_map(|session_ref| self.session_by_ref(*session_ref))
            .map(|session| session.id.clone())
            .collect()
    }

    pub(super) fn session_is_claude_code(session: &SessionInfo) -> bool {
        jcode_tui_session_picker::session_is_claude_code(session.source, &session.id)
    }

    pub(super) fn session_is_codex(session: &SessionInfo) -> bool {
        jcode_tui_session_picker::session_is_codex(session.source, session.model.as_deref())
    }

    pub(super) fn session_is_pi(session: &SessionInfo) -> bool {
        jcode_tui_session_picker::session_is_pi(
            session.source,
            session.provider_key.as_deref(),
            session.model.as_deref(),
        )
    }

    pub(super) fn session_is_open_code(session: &SessionInfo) -> bool {
        jcode_tui_session_picker::session_is_open_code(
            session.source,
            session.provider_key.as_deref(),
        )
    }

    pub(super) fn session_is_cursor(session: &SessionInfo) -> bool {
        jcode_tui_session_picker::session_is_cursor(session.source, session.provider_key.as_deref())
    }

    fn session_matches_filter_mode(
        &self,
        session: &SessionInfo,
        filter_mode: SessionFilterMode,
    ) -> bool {
        match filter_mode {
            SessionFilterMode::All => true,
            SessionFilterMode::CurrentDir => self.session_in_current_dir(session),
            SessionFilterMode::CatchUp => session.needs_catchup,
            SessionFilterMode::Saved => session.saved,
            SessionFilterMode::Cloud => session.server_icon.as_deref() == Some("☁"),
            SessionFilterMode::Active => self.session_is_live(session),
            SessionFilterMode::ClaudeCode => Self::session_is_claude_code(session),
            SessionFilterMode::Codex => Self::session_is_codex(session),
            SessionFilterMode::Pi => Self::session_is_pi(session),
            SessionFilterMode::OpenCode => Self::session_is_open_code(session),
            SessionFilterMode::Cursor => Self::session_is_cursor(session),
            SessionFilterMode::ExternalClis => {
                Self::session_is_codex(session)
                    || Self::session_is_claude_code(session)
                    || Self::session_is_pi(session)
                    || Self::session_is_open_code(session)
                    || Self::session_is_cursor(session)
            }
        }
    }

    /// Rebuild the items list based on current filters.
    pub(super) fn rebuild_items(&mut self) {
        let current_selected_id = self.selected_session().map(|session| session.id.clone());
        let show_test = self.show_test_sessions;
        let filter_mode = self.filter_mode;
        let search_query = self.search_query.clone();
        let search_matches = self.search_matched_session_refs(&search_query);
        let filtered_refs = self.filtered_session_refs(&search_matches, show_test, filter_mode);

        self.items.clear();
        self.visible_sessions.clear();
        self.item_to_session.clear();

        if filter_mode != SessionFilterMode::All {
            for session_ref in filtered_refs {
                self.push_visible_session(session_ref);
            }

            self.hidden_test_count =
                self.hidden_test_count_for_refs(&search_matches, show_test, filter_mode);

            let visible_ids = self.visible_session_ids();
            self.selected_session_ids
                .retain(|id| visible_ids.contains(id));

            let selected = current_selected_id
                .as_deref()
                .and_then(|id| self.find_item_index_for_session_id(id))
                .or_else(|| self.item_to_session.iter().position(|x| x.is_some()));
            self.list_state.select(selected);
            self.scroll_offset = 0;
            self.auto_scroll_preview = true;
            return;
        }

        let mut saved_sessions: Vec<SessionRef> = Vec::new();
        let mut saved_ids: std::collections::HashSet<String> = std::collections::HashSet::new();

        // Put genuinely closed windows ahead of saved and server sections.
        // They still obey search, project, and debug visibility filters. Use
        // the same exclusion set as saved rows to avoid duplicate entries.
        if self.search_query.trim().is_empty() {
            for session_ref in &filtered_refs {
                if let Some(session) = self.session_by_ref(*session_ref)
                    && self.session_recently_closed(session)
                {
                    saved_ids.insert(session.id.clone());
                    self.push_visible_session(*session_ref);
                }
            }
        }

        let placement = saved_placement::SavedPlacement::load();
        let now = chrono::Utc::now();
        for session_ref in &filtered_refs {
            if let Some(session) = self.session_by_ref(*session_ref)
                && !saved_ids.contains(&session.id)
                && placement.boosted(session, now)
            {
                saved_ids.insert(session.id.clone());
                saved_sessions.push(*session_ref);
            }
        }

        saved_sessions.sort_by(|a, b| {
            let a = self
                .session_by_ref(*a)
                .map(|session| session.last_message_time)
                .unwrap_or_default();
            let b = self
                .session_by_ref(*b)
                .map(|session| session.last_message_time)
                .unwrap_or_default();
            b.cmp(&a)
        });

        if !saved_sessions.is_empty() {
            self.items.push(PickerItem::SavedHeader {
                session_count: saved_sessions.len(),
            });
            self.item_to_session.push(None);

            for session_ref in saved_sessions {
                self.push_visible_session(session_ref);
            }
        }

        if !self.all_server_groups.is_empty() {
            // Partition the filtered refs by group in a single pass instead of
            // rescanning every filtered ref once per group. The previous code
            // was O(groups * filtered_refs); with many remote/server groups and
            // many sessions this scaled poorly on every search keystroke. One
            // bucketing pass is O(filtered_refs), then emitting is O(groups).
            let mut group_buckets: Vec<Vec<SessionRef>> =
                vec![Vec::new(); self.all_server_groups.len()];
            for session_ref in filtered_refs.iter().copied() {
                if let SessionRef::Group {
                    group_idx,
                    session_idx,
                } = session_ref
                    && let Some(group) = self.all_server_groups.get(group_idx)
                    && group
                        .sessions
                        .get(session_idx)
                        .is_some_and(|session| !saved_ids.contains(&session.id))
                {
                    group_buckets[group_idx].push(session_ref);
                }
            }

            let mut grouped_sections: Vec<(String, String, String, Vec<SessionRef>)> = Vec::new();
            for (group_idx, group) in self.all_server_groups.iter().enumerate() {
                // Cloud lease groups describe storage ownership, not a live
                // server. Keep them in the chronological history below.
                if group.icon == "☁" && !group.is_running {
                    continue;
                }
                let visible = std::mem::take(&mut group_buckets[group_idx]);
                if visible.is_empty() {
                    continue;
                }
                grouped_sections.push((
                    group.name.clone(),
                    group.icon.clone(),
                    group.version.clone(),
                    visible,
                ));
            }

            for (name, icon, version, visible) in grouped_sections {
                self.items.push(PickerItem::ServerHeader {
                    name,
                    icon,
                    version,
                    session_count: visible.len(),
                });
                self.item_to_session.push(None);

                for session_ref in visible {
                    self.push_visible_session(session_ref);
                }
            }

            let visible_orphans: Vec<SessionRef> = filtered_refs
                .iter()
                .copied()
                .filter(|session_ref| match session_ref {
                    SessionRef::Orphan(idx) => self
                        .all_orphan_sessions
                        .get(*idx)
                        .is_some_and(|session| !saved_ids.contains(&session.id)),
                    SessionRef::Group {
                        group_idx,
                        session_idx,
                    } => self
                        .all_server_groups
                        .get(*group_idx)
                        .filter(|group| group.icon == "☁" && !group.is_running)
                        .and_then(|group| group.sessions.get(*session_idx))
                        .is_some_and(|session| !saved_ids.contains(&session.id)),
                    _ => false,
                })
                .collect();
            if !visible_orphans.is_empty() {
                // Only separate history when an actual server section precedes
                // it. A cloud-only group must not introduce artificial chrome.
                if self
                    .items
                    .iter()
                    .any(|item| matches!(item, PickerItem::ServerHeader { .. }))
                {
                    self.items.push(PickerItem::OrphanHeader {
                        session_count: visible_orphans.len(),
                    });
                    self.item_to_session.push(None);
                }

                for session_ref in visible_orphans {
                    self.push_visible_session(session_ref);
                }
            }
        } else {
            let visible_sessions: Vec<SessionRef> = filtered_refs
                .iter()
                .copied()
                .filter(|session_ref| match session_ref {
                    SessionRef::Flat(idx) => self
                        .all_sessions
                        .get(*idx)
                        .is_some_and(|session| !saved_ids.contains(&session.id)),
                    _ => false,
                })
                .collect();
            for session_ref in visible_sessions {
                self.push_visible_session(session_ref);
            }
        }

        self.hidden_test_count =
            self.hidden_test_count_for_refs(&search_matches, show_test, filter_mode);

        let visible_ids = self.visible_session_ids();
        self.selected_session_ids
            .retain(|id| visible_ids.contains(id));

        let selected = current_selected_id
            .as_deref()
            .and_then(|id| self.find_item_index_for_session_id(id))
            .or_else(|| self.item_to_session.iter().position(|x| x.is_some()));
        self.list_state.select(selected);
        self.scroll_offset = 0;
        self.auto_scroll_preview = true;
    }

    pub(super) fn find_item_index_for_session_id(&self, session_id: &str) -> Option<usize> {
        self.item_to_session
            .iter()
            .enumerate()
            .find_map(|(item_idx, session_idx)| {
                session_idx
                    .and_then(|visible_idx| self.visible_sessions.get(visible_idx).copied())
                    .and_then(|session_ref| self.session_by_ref(session_ref))
                    .filter(|session| session.id == session_id)
                    .map(|_| item_idx)
            })
    }

    /// Toggle debug session visibility.
    pub(super) fn toggle_test_sessions(&mut self) {
        self.show_test_sessions = !self.show_test_sessions;
        self.rebuild_items();
    }

    /// Apply the current-project default only when the user opted in with
    /// `display.resume_project_scope`. Resume is intentionally global by
    /// default, so this is a no-op unless enabled.
    pub fn apply_resume_project_scope_default(&mut self) {
        if crate::config::config().display.resume_project_scope {
            self.prefer_current_dir_filter();
        }
    }

    /// Open `/resume` scoped to the current project when it has sessions, so
    /// a crashed terminal finds its own conversation first. `s` still cycles
    /// to every session, and a choice made by the user is never overridden.
    pub fn prefer_current_dir_filter(&mut self) {
        if self.filter_mode_user_chosen || self.filter_mode != SessionFilterMode::All {
            return;
        }
        let has_match = self.all_session_refs().into_iter().any(|session_ref| {
            self.session_by_ref(session_ref)
                .is_some_and(|session| !session.is_debug && self.session_in_current_dir(session))
        });
        if has_match {
            self.filter_mode = SessionFilterMode::CurrentDir;
            self.rebuild_items();
        }
    }

    pub(super) fn cycle_filter_mode(&mut self) {
        self.filter_mode_user_chosen = true;
        self.filter_mode = self.filter_mode.next();
        if self.filter_mode == SessionFilterMode::Active {
            self.refresh_live_presence();
        }
        self.rebuild_items();
    }

    pub(super) fn cycle_filter_mode_backwards(&mut self) {
        self.filter_mode_user_chosen = true;
        self.filter_mode = self.filter_mode.previous();
        if self.filter_mode == SessionFilterMode::Active {
            self.refresh_live_presence();
        }
        self.rebuild_items();
    }
}
