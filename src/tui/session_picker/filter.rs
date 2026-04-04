use super::*;

impl SessionPicker {
    /// Check if a session matches the current search query.
    fn session_matches_search(session: &SessionInfo, query: &str) -> bool {
        if query.is_empty() {
            return true;
        }
        let q = query.to_lowercase();
        session.search_index.contains(&q)
    }

    pub(super) fn session_is_claude_code(session: &SessionInfo) -> bool {
        session.source == SessionSource::ClaudeCode || session.id.starts_with("imported_cc_")
    }

    pub(super) fn session_is_codex(session: &SessionInfo) -> bool {
        if session.source == SessionSource::Codex {
            return true;
        }
        session
            .model
            .as_deref()
            .map(|model| model.to_ascii_lowercase().contains("codex"))
            .unwrap_or(false)
    }

    pub(super) fn session_is_pi(session: &SessionInfo) -> bool {
        if session.source == SessionSource::Pi {
            return true;
        }
        let provider_matches = session
            .provider_key
            .as_deref()
            .map(|key| {
                let key = key.to_ascii_lowercase();
                key == "pi" || key.starts_with("pi-")
            })
            .unwrap_or(false);
        let model_matches = session
            .model
            .as_deref()
            .map(|model| {
                let model = model.to_ascii_lowercase();
                model == "pi"
                    || model.starts_with("pi-")
                    || model.starts_with("pi/")
                    || model.contains("/pi-")
            })
            .unwrap_or(false);
        provider_matches || model_matches
    }

    pub(super) fn session_is_open_code(session: &SessionInfo) -> bool {
        if session.source == SessionSource::OpenCode {
            return true;
        }
        session
            .provider_key
            .as_deref()
            .map(|key| {
                let key = key.to_ascii_lowercase();
                key == "opencode" || key == "opencode-go" || key.contains("opencode")
            })
            .unwrap_or(false)
    }

    fn session_matches_filter_mode(session: &SessionInfo, filter_mode: SessionFilterMode) -> bool {
        match filter_mode {
            SessionFilterMode::All => true,
            SessionFilterMode::CatchUp => session.needs_catchup,
            SessionFilterMode::Saved => session.saved,
            SessionFilterMode::ClaudeCode => Self::session_is_claude_code(session),
            SessionFilterMode::Codex => Self::session_is_codex(session),
            SessionFilterMode::Pi => Self::session_is_pi(session),
            SessionFilterMode::OpenCode => Self::session_is_open_code(session),
        }
    }

    fn collect_filtered_sessions(
        &self,
        session_visible: impl Fn(&SessionInfo) -> bool,
    ) -> Vec<SessionRef> {
        let mut filtered = Vec::new();

        if !self.all_server_groups.is_empty() {
            for (group_idx, group) in self.all_server_groups.iter().enumerate() {
                for (session_idx, session) in group.sessions.iter().enumerate() {
                    if session_visible(session) {
                        filtered.push(SessionRef::Group {
                            group_idx,
                            session_idx,
                        });
                    }
                }
            }
            for (idx, session) in self.all_orphan_sessions.iter().enumerate() {
                if session_visible(session) {
                    filtered.push(SessionRef::Orphan(idx));
                }
            }
        } else {
            for (idx, session) in self.all_sessions.iter().enumerate() {
                if session_visible(session) {
                    filtered.push(SessionRef::Flat(idx));
                }
            }
        }

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
        filtered
    }

    /// Rebuild the items list based on current filters.
    pub(super) fn rebuild_items(&mut self) {
        let current_selected_id = self.selected_session().map(|session| session.id.clone());
        let show_test = self.show_test_sessions;
        let filter_mode = self.filter_mode;
        let query = self.search_query.clone();

        let session_visible = |s: &SessionInfo| -> bool {
            (show_test || !s.is_debug)
                && Self::session_matches_search(s, &query)
                && Self::session_matches_filter_mode(s, filter_mode)
        };

        self.items.clear();
        self.visible_sessions.clear();
        self.item_to_session.clear();

        if filter_mode != SessionFilterMode::All {
            let filtered = self.collect_filtered_sessions(session_visible);
            for session_ref in filtered {
                self.push_visible_session(session_ref);
            }

            self.hidden_test_count = if show_test {
                0
            } else {
                self.all_server_groups
                    .iter()
                    .flat_map(|g| g.sessions.iter())
                    .chain(self.all_orphan_sessions.iter())
                    .chain(self.all_sessions.iter())
                    .filter(|s| {
                        s.is_debug
                            && Self::session_matches_search(s, &query)
                            && Self::session_matches_filter_mode(s, filter_mode)
                    })
                    .count()
            };

            let visible_ids: std::collections::HashSet<String> = self
                .visible_sessions
                .iter()
                .filter_map(|session_ref| self.session_by_ref(*session_ref))
                .map(|session| session.id.clone())
                .collect();
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

        if !self.all_server_groups.is_empty() {
            for (group_idx, group) in self.all_server_groups.iter().enumerate() {
                for (session_idx, s) in group.sessions.iter().enumerate() {
                    if s.saved && session_visible(s) {
                        saved_ids.insert(s.id.clone());
                        saved_sessions.push(SessionRef::Group {
                            group_idx,
                            session_idx,
                        });
                    }
                }
            }
            for (idx, s) in self.all_orphan_sessions.iter().enumerate() {
                if s.saved && session_visible(s) {
                    saved_ids.insert(s.id.clone());
                    saved_sessions.push(SessionRef::Orphan(idx));
                }
            }
        } else {
            for (idx, s) in self.all_sessions.iter().enumerate() {
                if s.saved && session_visible(s) {
                    saved_ids.insert(s.id.clone());
                    saved_sessions.push(SessionRef::Flat(idx));
                }
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
            let grouped_sections: Vec<(String, String, String, Vec<SessionRef>)> = self
                .all_server_groups
                .iter()
                .enumerate()
                .filter_map(|(group_idx, group)| {
                    let visible: Vec<SessionRef> = group
                        .sessions
                        .iter()
                        .enumerate()
                        .filter_map(|(session_idx, s)| {
                            (session_visible(s) && !saved_ids.contains(&s.id)).then_some(
                                SessionRef::Group {
                                    group_idx,
                                    session_idx,
                                },
                            )
                        })
                        .collect();

                    if visible.is_empty() {
                        None
                    } else {
                        Some((
                            group.name.clone(),
                            group.icon.clone(),
                            group.version.clone(),
                            visible,
                        ))
                    }
                })
                .collect();

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

            let visible_orphans: Vec<SessionRef> = self
                .all_orphan_sessions
                .iter()
                .enumerate()
                .filter_map(|(idx, s)| {
                    (session_visible(s) && !saved_ids.contains(&s.id))
                        .then_some(SessionRef::Orphan(idx))
                })
                .collect();
            if !visible_orphans.is_empty() {
                self.items.push(PickerItem::OrphanHeader {
                    session_count: visible_orphans.len(),
                });
                self.item_to_session.push(None);

                for session_ref in visible_orphans {
                    self.push_visible_session(session_ref);
                }
            }
        } else {
            let visible_sessions: Vec<SessionRef> = self
                .all_sessions
                .iter()
                .enumerate()
                .filter_map(|(idx, session)| {
                    (session_visible(session) && !saved_ids.contains(&session.id))
                        .then_some(SessionRef::Flat(idx))
                })
                .collect();
            for session_ref in visible_sessions {
                self.push_visible_session(session_ref);
            }
        }

        self.hidden_test_count = if show_test {
            0
        } else {
            self.all_server_groups
                .iter()
                .flat_map(|g| g.sessions.iter())
                .chain(self.all_orphan_sessions.iter())
                .chain(self.all_sessions.iter())
                .filter(|s| s.is_debug && Self::session_matches_search(s, &query))
                .count()
        };

        let visible_ids: std::collections::HashSet<String> = self
            .visible_sessions
            .iter()
            .filter_map(|session_ref| self.session_by_ref(*session_ref))
            .map(|session| session.id.clone())
            .collect();
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

    fn find_item_index_for_session_id(&self, session_id: &str) -> Option<usize> {
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

    pub(super) fn cycle_filter_mode(&mut self) {
        self.filter_mode = self.filter_mode.next();
        self.rebuild_items();
    }

    pub(super) fn cycle_filter_mode_backwards(&mut self) {
        self.filter_mode = self.filter_mode.previous();
        self.rebuild_items();
    }
}
