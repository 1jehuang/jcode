use super::state_ui_storage::infer_spawned_session_startup_hints;
use super::*;
use crate::tui::ui::tools_ui;
use crate::tui::{TuiState, backend};

pub(super) struct RestoredReloadInput {
    pub input: String,
    pub cursor: usize,
    pub pending_images: Vec<(String, String)>,
    pub submit_on_restore: bool,
    pub queued_messages: Vec<String>,
    pub hidden_queued_system_messages: Vec<String>,
    pub startup_status_notice: Option<String>,
    pub startup_display_message: Option<(String, String)>,
    pub interleave_message: Option<String>,
    pub pending_soft_interrupts: Vec<String>,
    pub pending_soft_interrupt_resend: Option<Vec<String>>,
    pub rate_limit_pending_message: Option<super::PendingRemoteMessage>,
    pub rate_limit_reset: Option<Instant>,
    pub observe_mode_enabled: bool,
    pub observe_page_markdown: String,
    pub observe_page_updated_at_ms: u64,
    pub split_view_enabled: bool,
    pub todos_view_enabled: bool,
}

impl App {
    fn recompute_display_message_stats(&mut self) {
        self.display_user_message_count = self
            .display_messages
            .iter()
            .filter(|message| message.effective_role() == "user")
            .count();
        self.display_edit_tool_message_count = self
            .display_messages
            .iter()
            .filter(|message| {
                message
                    .tool_data
                    .as_ref()
                    .map(|tool| tools_ui::is_edit_tool_name(&tool.name))
                    .unwrap_or(false)
            })
            .count();
    }

    pub(super) fn active_client_session_id(&self) -> Option<&str> {
        if self.is_remote {
            self.remote_session_id.as_deref()
        } else {
            Some(self.session.id.as_str())
        }
    }

    pub(super) fn note_client_focus(&mut self, force: bool) {
        let Some(session_id) = self.active_client_session_id() else {
            return;
        };
        let session_id = session_id.to_string();

        if !force
            && self.last_client_focus_session_id.as_deref() == Some(session_id.as_str())
            && self
                .last_client_focus_recorded_at
                .is_some_and(|last| last.elapsed() < Self::CLIENT_FOCUS_RECORD_DEBOUNCE)
        {
            return;
        }

        if crate::dictation::remember_last_focused_session(&session_id).is_ok() {
            self.last_client_focus_recorded_at = Some(Instant::now());
            self.last_client_focus_session_id = Some(session_id);
        }
    }

    pub(super) fn note_client_interaction(&mut self) {
        if !crate::perf::tui_policy().enable_focus_change {
            self.note_client_focus(false);
        }
    }

    pub fn display_messages(&self) -> &[DisplayMessage] {
        &self.display_messages
    }

    pub(super) fn bump_display_messages_version(&mut self) {
        self.recompute_display_message_stats();
        self.display_messages_version = self.display_messages_version.wrapping_add(1);
        self.refresh_split_view_if_needed();
    }

    pub(super) fn save_input_for_reload(&self, session_id: &str) {
        let resume_prompt = self.rate_limit_pending_message.as_ref().filter(|pending| {
            !pending.auto_retry
                && !pending.is_system
                && (!pending.content.trim().is_empty() || !pending.images.is_empty())
        });
        if self.input.is_empty()
            && self.pending_images.is_empty()
            && self.queued_messages.is_empty()
            && self.hidden_queued_system_messages.is_empty()
            && self.interleave_message.is_none()
            && self.pending_soft_interrupts.is_empty()
            && self.pending_soft_interrupt_requests.is_empty()
            && self.rate_limit_pending_message.is_none()
            && resume_prompt.is_none()
            && !self.observe_mode_enabled
            && !self.split_view_enabled
            && !self.todos_view_enabled
        {
            return;
        }
        if let Ok(jcode_dir) = crate::storage::jcode_dir() {
            let path = jcode_dir.join(format!("client-input-{}", session_id));
            let rate_limit_reset_in_ms = if resume_prompt.is_some() {
                None
            } else {
                self.rate_limit_reset.map(|reset| {
                    let now = Instant::now();
                    if reset <= now {
                        0
                    } else {
                        (reset - now).as_millis().min(u64::MAX as u128) as u64
                    }
                })
            };
            let rate_limit_pending_message = if resume_prompt.is_some() {
                None
            } else {
                self.rate_limit_pending_message.as_ref().map(|pending| {
                    serde_json::json!({
                        "content": pending.content,
                        "images": pending.images,
                        "is_system": pending.is_system,
                        "system_reminder": pending.system_reminder,
                        "auto_retry": pending.auto_retry,
                        "retry_attempts": pending.retry_attempts,
                    })
                })
            };
            let resume_input = resume_prompt.map(|pending| pending.content.as_str());
            let resume_images = resume_prompt.map(|pending| pending.images.as_slice());
            let rate_limit_reset_in_ms =
                rate_limit_reset_in_ms.or_else(|| resume_prompt.map(|_| 0));
            let pending_soft_interrupt_resend = self
                .pending_soft_interrupt_requests
                .iter()
                .map(|(_, content)| content.clone())
                .collect::<Vec<_>>();
            let data = serde_json::json!({
                "cursor": resume_input.map(|input| input.len()).unwrap_or(self.cursor_pos),
                "input": resume_input.unwrap_or(self.input.as_str()),
                "pending_images": resume_images.unwrap_or(self.pending_images.as_slice()).iter().map(|(media_type, data)| serde_json::json!({
                    "media_type": media_type,
                    "data": data,
                })).collect::<Vec<_>>(),
                "submit_on_restore": resume_prompt.is_some(),
                "queued_messages": self.queued_messages,
                "hidden_queued_system_messages": self.hidden_queued_system_messages,
                "interleave_message": self.interleave_message,
                "pending_soft_interrupts": self.pending_soft_interrupts,
                "pending_soft_interrupt_resend": pending_soft_interrupt_resend,
                "rate_limit_pending_message": rate_limit_pending_message,
                "rate_limit_reset_in_ms": rate_limit_reset_in_ms,
                "observe_mode_enabled": self.observe_mode_enabled,
                "observe_page_markdown": self.observe_page_markdown,
                "observe_page_updated_at_ms": self.observe_page_updated_at_ms,
                "split_view_enabled": self.split_view_enabled,
                "todos_view_enabled": self.todos_view_enabled,
            });
            let _ = std::fs::write(&path, data.to_string());
        }
    }

    pub(crate) fn save_startup_message_for_session(session_id: &str, message: String) {
        if message.trim().is_empty() {
            return;
        }
        if let Ok(jcode_dir) = crate::storage::jcode_dir() {
            let path = jcode_dir.join(format!("client-input-{}", session_id));
            let inferred_hints = infer_spawned_session_startup_hints(&message);
            let data = serde_json::json!({
                "cursor": 0,
                "input": "",
                "pending_images": [],
                "submit_on_restore": false,
                "queued_messages": [],
                "hidden_queued_system_messages": [message],
                "startup_status_notice": inferred_hints.as_ref().map(|(status, _)| status.clone()),
                "startup_display_message_title": inferred_hints.as_ref().map(|(_, (title, _))| title.clone()),
                "startup_display_message": inferred_hints.as_ref().map(|(_, (_, body))| body.clone()),
                "interleave_message": serde_json::Value::Null,
                "pending_soft_interrupts": [],
                "pending_soft_interrupt_resend": [],
                "rate_limit_pending_message": serde_json::Value::Null,
                "rate_limit_reset_in_ms": serde_json::Value::Null,
                "observe_mode_enabled": false,
                "observe_page_markdown": "",
                "observe_page_updated_at_ms": 0,
                "split_view_enabled": false,
                "todos_view_enabled": false,
            });
            let _ = std::fs::write(&path, data.to_string());
        }
    }

    pub(crate) fn save_startup_submission_for_session(
        session_id: &str,
        input: String,
        pending_images: Vec<(String, String)>,
    ) {
        if input.trim().is_empty() && pending_images.is_empty() {
            return;
        }
        if let Ok(jcode_dir) = crate::storage::jcode_dir() {
            let path = jcode_dir.join(format!("client-input-{}", session_id));
            let data = serde_json::json!({
                "cursor": input.len(),
                "input": input,
                "pending_images": pending_images.iter().map(|(media_type, data)| serde_json::json!({
                    "media_type": media_type,
                    "data": data,
                })).collect::<Vec<_>>(),
                "submit_on_restore": true,
                "queued_messages": [],
                "hidden_queued_system_messages": [],
                "startup_status_notice": "Startup prompt queued",
                "startup_display_message_title": serde_json::Value::Null,
                "startup_display_message": serde_json::Value::Null,
                "interleave_message": serde_json::Value::Null,
                "pending_soft_interrupts": [],
                "pending_soft_interrupt_resend": [],
                "rate_limit_pending_message": serde_json::Value::Null,
                "rate_limit_reset_in_ms": serde_json::Value::Null,
                "observe_mode_enabled": false,
                "observe_page_markdown": "",
                "observe_page_updated_at_ms": 0,
                "split_view_enabled": false,
                "todos_view_enabled": false,
            });
            let _ = std::fs::write(&path, data.to_string());
        }
    }

    pub(super) fn restore_input_for_reload(session_id: &str) -> Option<RestoredReloadInput> {
        let jcode_dir = crate::storage::jcode_dir().ok()?;
        let path = jcode_dir.join(format!("client-input-{}", session_id));
        if !path.exists() {
            return None;
        }
        let data = std::fs::read_to_string(&path).ok()?;
        let _ = std::fs::remove_file(&path);

        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&data) {
            let input = value
                .get("input")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let cursor = value.get("cursor").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
            let pending_images = value
                .get("pending_images")
                .and_then(|v| v.as_array())
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|item| {
                            Some((
                                item.get("media_type")?.as_str()?.to_string(),
                                item.get("data")?.as_str()?.to_string(),
                            ))
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let submit_on_restore = value
                .get("submit_on_restore")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let queued_messages = value
                .get("queued_messages")
                .and_then(|v| v.as_array())
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|item| item.as_str().map(|s| s.to_string()))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let hidden_queued_system_messages = value
                .get("hidden_queued_system_messages")
                .and_then(|v| v.as_array())
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|item| item.as_str().map(|s| s.to_string()))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let startup_status_notice = value
                .get("startup_status_notice")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .filter(|s| !s.is_empty());
            let startup_display_message = value
                .get("startup_display_message")
                .and_then(|v| v.as_str())
                .map(|body| {
                    let title = value
                        .get("startup_display_message_title")
                        .and_then(|v| v.as_str())
                        .unwrap_or("Launch")
                        .to_string();
                    (title, body.to_string())
                })
                .filter(|(_, body)| !body.is_empty());
            let interleave_message = value
                .get("interleave_message")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .filter(|s| !s.is_empty());
            let pending_soft_interrupts = value
                .get("pending_soft_interrupts")
                .and_then(|v| v.as_array())
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|item| item.as_str().map(|s| s.to_string()))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let pending_soft_interrupt_resend =
                value.get("pending_soft_interrupt_resend").map(|v| {
                    v.as_array()
                        .map(|items| {
                            items
                                .iter()
                                .filter_map(|item| item.as_str().map(|s| s.to_string()))
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default()
                });
            let rate_limit_pending_message = value
                .get("rate_limit_pending_message")
                .and_then(|pending| pending.as_object())
                .map(|pending| super::PendingRemoteMessage {
                    content: pending
                        .get("content")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    images: pending
                        .get("images")
                        .and_then(|v| v.as_array())
                        .map(|items| {
                            items
                                .iter()
                                .filter_map(|item| {
                                    let pair = item.as_array()?;
                                    let first = pair.first()?.as_str()?;
                                    let second = pair.get(1)?.as_str()?;
                                    Some((first.to_string(), second.to_string()))
                                })
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default(),
                    is_system: pending
                        .get("is_system")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false),
                    system_reminder: pending
                        .get("system_reminder")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string()),
                    auto_retry: pending
                        .get("auto_retry")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false),
                    retry_attempts: pending
                        .get("retry_attempts")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0) as u8,
                    retry_at: None,
                });
            let rate_limit_reset = value
                .get("rate_limit_reset_in_ms")
                .and_then(|v| v.as_u64())
                .map(|delay_ms| Instant::now() + Duration::from_millis(delay_ms));
            let mut rate_limit_pending_message = rate_limit_pending_message;
            if let (Some(pending), Some(reset)) =
                (&mut rate_limit_pending_message, rate_limit_reset)
            {
                pending.retry_at = Some(reset);
            }
            let observe_mode_enabled = value
                .get("observe_mode_enabled")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let observe_page_markdown = value
                .get("observe_page_markdown")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let observe_page_updated_at_ms = value
                .get("observe_page_updated_at_ms")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let split_view_enabled = value
                .get("split_view_enabled")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let todos_view_enabled = value
                .get("todos_view_enabled")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let cursor = cursor.min(input.len());
            return Some(RestoredReloadInput {
                input,
                cursor,
                pending_images,
                submit_on_restore,
                queued_messages,
                hidden_queued_system_messages,
                startup_status_notice,
                startup_display_message,
                interleave_message,
                pending_soft_interrupts,
                pending_soft_interrupt_resend,
                rate_limit_pending_message,
                rate_limit_reset,
                observe_mode_enabled,
                observe_page_markdown,
                observe_page_updated_at_ms,
                split_view_enabled,
                todos_view_enabled,
            });
        }

        let (cursor_str, input) = data.split_once('\n')?;
        let cursor = cursor_str.parse::<usize>().unwrap_or(0);
        let cursor = cursor.min(input.len());
        Some(RestoredReloadInput {
            input: input.to_string(),
            cursor,
            pending_images: Vec::new(),
            submit_on_restore: false,
            queued_messages: Vec::new(),
            hidden_queued_system_messages: Vec::new(),
            startup_status_notice: None,
            startup_display_message: None,
            interleave_message: None,
            pending_soft_interrupts: Vec::new(),
            pending_soft_interrupt_resend: None,
            rate_limit_pending_message: None,
            rate_limit_reset: None,
            observe_mode_enabled: false,
            observe_page_markdown: String::new(),
            observe_page_updated_at_ms: 0,
            split_view_enabled: false,
            todos_view_enabled: false,
        })
    }

    /// Toggle scroll bookmark: stash current position and jump to bottom,
    /// or restore stashed position if already at bottom.
    pub(super) fn toggle_scroll_bookmark(&mut self) {
        if let Some(saved) = self.scroll_bookmark.take() {
            // We have a bookmark — teleport back to it
            self.scroll_offset = saved;
            self.auto_scroll_paused = saved > 0;
            self.set_status_notice("📌 Returned to bookmark");
        } else if self.auto_scroll_paused && self.scroll_offset > 0 {
            // We're scrolled up — save position and jump to bottom
            self.scroll_bookmark = Some(self.scroll_offset);
            self.follow_chat_bottom();
            self.set_status_notice("📌 Bookmark set — press again to return");
        }
        // If already at bottom with no bookmark, do nothing
    }

    pub(super) fn follow_chat_bottom_for_typing(&mut self) {
        if !self.typing_scroll_lock {
            self.follow_chat_bottom();
        }
    }

    pub(super) fn set_side_panel_snapshot(
        &mut self,
        snapshot: crate::side_panel::SidePanelSnapshot,
    ) {
        self.refresh_split_view_if_needed();
        let focus_split = self.split_view_enabled
            && self.side_panel.focused_page_id.as_deref()
                == Some(super::split_view::SPLIT_VIEW_PAGE_ID);
        let focus_observe = self.observe_mode_enabled
            && self.side_panel.focused_page_id.as_deref() == Some(super::observe::OBSERVE_PAGE_ID);
        let snapshot = if self.split_view_enabled {
            self.decorate_side_panel_with_split_view(snapshot, focus_split)
        } else {
            snapshot
        };
        let focus_todos = self.todos_view_enabled
            && self.side_panel.focused_page_id.as_deref()
                == Some(super::todos_view::TODOS_VIEW_PAGE_ID);
        let snapshot = if self.todos_view_enabled {
            self.decorate_side_panel_with_todos_view(snapshot, focus_todos)
        } else {
            snapshot
        };
        let snapshot = if self.observe_mode_enabled {
            self.decorate_side_panel_with_observe(snapshot, focus_observe)
        } else {
            snapshot
        };
        self.apply_side_panel_snapshot(snapshot);
    }

    pub(super) fn apply_side_panel_snapshot(
        &mut self,
        snapshot: crate::side_panel::SidePanelSnapshot,
    ) {
        let focused_before = self.side_panel.focused_page_id.clone();
        let focused_after = snapshot.focused_page_id.clone();
        let focused_changed = focused_before != focused_after;
        let focused_title_after = snapshot.focused_page().map(|page| page.title.clone());
        if let Some(focused_after) = focused_after.as_deref() {
            if focused_after != super::observe::OBSERVE_PAGE_ID {
                self.last_side_panel_focus_id = Some(focused_after.to_string());
            }
        } else if snapshot.pages.is_empty() {
            self.last_side_panel_focus_id = None;
        }
        self.last_side_panel_refresh = None;
        self.side_panel = snapshot;
        self.note_runtime_memory_event("side_panel_updated", "side_panel_snapshot_applied");
        if focused_changed {
            self.diff_pane_scroll = 0;
            self.diff_pane_scroll_x = 0;
            self.diff_pane_auto_scroll = true;
        }
        if focused_changed {
            match (focused_after.as_deref(), focused_title_after.as_deref()) {
                (Some(super::split_view::SPLIT_VIEW_PAGE_ID), _) => {
                    self.set_status_notice("Split view")
                }
                (Some(super::todos_view::TODOS_VIEW_PAGE_ID), _) => self.set_status_notice("Todos"),
                (Some(super::observe::OBSERVE_PAGE_ID), _) => self.set_status_notice("Observe"),
                (Some("goals"), _) => self.set_status_notice("Goals"),
                (Some(id), Some(title)) if id.starts_with("goal.") => self.set_status_notice(title),
                _ => {}
            }
        }
        self.sync_diagram_fit_context();
        self.prewarm_focused_side_panel();
    }

    pub(super) fn refresh_side_panel_linked_content_if_due(&mut self) -> bool {
        let refresh_interval = crate::perf::tui_policy().linked_side_panel_refresh_interval;

        let should_refresh = self
            .side_panel
            .focused_page()
            .map(|page| page.source == crate::side_panel::SidePanelPageSource::LinkedFile)
            .unwrap_or(false);

        if !should_refresh {
            self.last_side_panel_refresh = None;
            return false;
        }

        let now = Instant::now();
        if self
            .last_side_panel_refresh
            .is_some_and(|last| now.duration_since(last) < refresh_interval)
        {
            return false;
        }

        self.last_side_panel_refresh = Some(now);
        let mut snapshot = self.side_panel.clone();
        let session_id = self.active_client_session_id().map(str::to_string);
        std::thread::spawn(move || {
            if crate::side_panel::refresh_linked_page_content(&mut snapshot, None)
                && let Some(session_id) = session_id
            {
                crate::bus::Bus::global().publish(crate::bus::BusEvent::SidePanelUpdated(
                    crate::bus::SidePanelUpdated {
                        session_id,
                        snapshot,
                    },
                ));
            }
        });

        false
    }

    pub(super) fn toggle_typing_scroll_lock(&mut self) {
        self.typing_scroll_lock = !self.typing_scroll_lock;
        let status = if self.typing_scroll_lock {
            "Typing scroll lock: ON — typing stays at current chat position"
        } else {
            "Typing scroll lock: OFF — typing follows chat bottom"
        };
        self.set_status_notice(status);
    }

    pub(super) fn toggle_centered_mode(&mut self) {
        self.centered = !self.centered;
        let mode = if self.centered {
            "Centered"
        } else {
            "Left-aligned"
        };
        self.set_status_notice(format!("Layout: {}", mode));
        self.prewarm_focused_side_panel();
    }

    pub fn set_centered(&mut self, centered: bool) {
        self.centered = centered;
        self.prewarm_focused_side_panel();
    }

    fn prewarm_focused_side_panel(&self) {
        let Ok((terminal_width, terminal_height)) = crossterm::terminal::size() else {
            return;
        };
        let has_protocol = crate::tui::mermaid::protocol_type().is_some();
        let _ = crate::tui::prewarm_focused_side_panel(
            &self.side_panel,
            terminal_width,
            terminal_height,
            self.diagram_pane_ratio,
            has_protocol,
            self.centered,
        );
    }

    // ==================== Debug Socket Methods ====================

    /// Enable debug socket and return the broadcast receiver
    /// Call this before run() to enable debug event broadcasting
    pub fn enable_debug_socket(&mut self) -> tokio::sync::broadcast::Receiver<backend::DebugEvent> {
        let (tx, rx) = tokio::sync::broadcast::channel(256);
        self.debug_tx = Some(tx);
        rx
    }

    /// Broadcast a debug event to connected clients (if debug socket enabled)
    pub(super) fn broadcast_debug(&self, event: backend::DebugEvent) {
        if let Some(ref tx) = self.debug_tx {
            let _ = tx.send(event); // Ignore errors (no receivers)
        }
    }

    /// Create a full state snapshot for debug socket
    pub fn create_debug_snapshot(&self) -> backend::DebugEvent {
        use backend::{DebugEvent, DebugMessage};

        DebugEvent::StateSnapshot {
            display_messages: self
                .display_messages
                .iter()
                .map(|m| DebugMessage {
                    role: m.role.clone(),
                    content: m.content.clone(),
                    tool_calls: m.tool_calls.clone(),
                    duration_secs: m.duration_secs,
                    title: m.title.clone(),
                    tool_data: m.tool_data.clone(),
                })
                .collect(),
            streaming_text: self.streaming_text.clone(),
            streaming_tool_calls: self.streaming_tool_calls.clone(),
            input: self.input.clone(),
            cursor_pos: self.cursor_pos,
            is_processing: self.is_processing,
            scroll_offset: self.scroll_offset,
            status: format!("{:?}", self.status),
            provider_name: self.provider.name().to_string(),
            provider_model: self.provider.model().to_string(),
            mcp_servers: self
                .mcp_server_names
                .iter()
                .map(|(name, _)| name.clone())
                .collect(),
            skills: self
                .current_skills_snapshot()
                .list()
                .iter()
                .map(|s| s.name.clone())
                .collect(),
            session_id: self.provider_session_id.clone(),
            input_tokens: self.streaming_input_tokens,
            output_tokens: self.streaming_output_tokens,
            cache_read_input_tokens: self.streaming_cache_read_tokens,
            cache_creation_input_tokens: self.streaming_cache_creation_tokens,
            queued_messages: self.queued_messages.clone(),
        }
    }

    /// Start debug socket listener task
    /// Returns a JoinHandle for the listener task
    pub fn start_debug_socket_listener(
        &self,
        mut rx: tokio::sync::broadcast::Receiver<backend::DebugEvent>,
    ) -> tokio::task::JoinHandle<()> {
        use crate::transport::Listener;
        use tokio::io::AsyncWriteExt;

        let socket_path = Self::debug_socket_path();
        let initial_snapshot = self.create_debug_snapshot();

        tokio::spawn(async move {
            // Clean up old socket
            let _ = std::fs::remove_file(&socket_path);

            #[cfg(windows)]
            let mut listener = match Listener::bind(&socket_path) {
                Ok(l) => l,
                Err(e) => {
                    crate::logging::error(&format!("Failed to bind debug socket: {}", e));
                    return;
                }
            };
            #[cfg(not(windows))]
            let listener = match Listener::bind(&socket_path) {
                Ok(l) => l,
                Err(e) => {
                    crate::logging::error(&format!("Failed to bind debug socket: {}", e));
                    return;
                }
            };

            // Restrict TUI debug socket to owner-only.
            let _ = crate::platform::set_permissions_owner_only(&socket_path);

            // Accept connections and forward events
            let clients: std::sync::Arc<tokio::sync::Mutex<Vec<crate::transport::WriteHalf>>> =
                std::sync::Arc::new(tokio::sync::Mutex::new(Vec::new()));

            let clients_clone = clients.clone();

            // Spawn event broadcaster
            let broadcast_handle = tokio::spawn(async move {
                while let Ok(event) = rx.recv().await {
                    let json = match serde_json::to_string(&event) {
                        Ok(j) => j + "\n",
                        Err(_) => continue,
                    };
                    let bytes = json.as_bytes();

                    let mut clients = clients_clone.lock().await;
                    let mut to_remove = Vec::new();

                    for (i, writer) in clients.iter_mut().enumerate() {
                        if writer.write_all(bytes).await.is_err() {
                            to_remove.push(i);
                        }
                    }

                    // Remove disconnected clients (reverse order to preserve indices)
                    for i in to_remove.into_iter().rev() {
                        clients.swap_remove(i);
                    }
                }
            });

            // Accept new connections
            while let Ok((stream, _)) = listener.accept().await {
                let (_, writer) = stream.into_split();
                let mut writer = writer;

                let snapshot_json =
                    serde_json::to_string(&initial_snapshot).unwrap_or_default() + "\n";
                if writer.write_all(snapshot_json.as_bytes()).await.is_ok() {
                    clients.lock().await.push(writer);
                }
            }

            broadcast_handle.abort();
            let _ = std::fs::remove_file(&socket_path);
        })
    }

    /// Get the debug socket path
    pub fn debug_socket_path() -> std::path::PathBuf {
        crate::storage::runtime_dir().join("jcode-debug.sock")
    }
}

fn cache_ratio_pct(numerator: u64, denominator: u64) -> u8 {
    if denominator == 0 {
        0
    } else {
        ((numerator as f32 / denominator as f32) * 100.0)
            .round()
            .clamp(0.0, 100.0) as u8
    }
}

fn format_cache_stats(app: &App) -> String {
    let reported = app.total_cache_reported_input_tokens;
    let read = app.total_cache_read_tokens;
    let write = app.total_cache_creation_tokens;
    let optimal = app.total_cache_optimal_input_tokens;
    let read_pct = cache_ratio_pct(read, reported);
    let write_pct = cache_ratio_pct(write, reported);
    let optimal_pct = if optimal > 0 {
        Some(cache_ratio_pct(read, optimal))
    } else {
        None
    };
    let ttl = if crate::provider::anthropic::is_cache_ttl_1h() {
        "1 hour"
    } else {
        "5 minutes"
    };

    let mut lines = Vec::new();
    lines.push("**KV cache stats**".to_string());
    lines.push(String::new());
    lines.push(format!("- TTL setting: **{}**", ttl));
    if reported == 0 {
        lines.push("- No provider cache telemetry recorded in this session yet.".to_string());
    } else {
        lines.push(format!("- Reported input: **{}** tokens", reported));
        lines.push(format!("- Cache read: **{}** tokens ({}%)", read, read_pct));
        lines.push(format!(
            "- Cache write: **{}** tokens ({}%)",
            write, write_pct
        ));
        if let Some(optimal_pct) = optimal_pct {
            lines.push(format!(
                "- Optimal baseline input: **{}** tokens; read vs optimal: **{}%**",
                optimal, optimal_pct
            ));
        }
    }

    if let Some(baseline) = app.kv_cache_baseline.as_ref() {
        lines.push(format!(
            "- Current baseline: **{}** input tokens, age **{}s**, provider `{}` model `{}`{}",
            baseline.input_tokens,
            baseline.completed_at.elapsed().as_secs(),
            baseline.provider,
            baseline.model,
            baseline
                .upstream_provider
                .as_ref()
                .map(|upstream| format!(" upstream `{}`", upstream))
                .unwrap_or_default()
        ));
    }

    if app.kv_cache_miss_samples.is_empty() {
        lines.push("- Recent misses: none attributed.".to_string());
    } else {
        lines.push("- Recent misses:".to_string());
        for sample in app.kv_cache_miss_samples.iter().rev().take(5) {
            lines.push(format!(
                "  - turn {} call {}: **{}** missed tokens ({})",
                sample.turn_number,
                sample.call_index,
                sample.missed_tokens,
                sample.reason.label()
            ));
        }
    }

    lines.join("\n")
}

pub(super) fn handle_info_command(app: &mut App, trimmed: &str) -> bool {
    if trimmed == "/version" {
        let version = env!("JCODE_VERSION");
        let is_canary = if app.session.is_canary {
            " (canary/self-dev)"
        } else {
            ""
        };
        app.push_display_message(DisplayMessage {
            role: "system".to_string(),
            content: format!("jcode {}{}", version, is_canary),
            tool_calls: vec![],
            duration_secs: None,
            title: None,
            tool_data: None,
        });
        return true;
    }

    if trimmed == "/changelog" {
        app.changelog_scroll = Some(0);
        return true;
    }

    if trimmed == "/cache" || trimmed.starts_with("/cache ") {
        let arg = trimmed.strip_prefix("/cache").unwrap_or("").trim();
        match arg {
            "stats" | "status" => {
                app.push_display_message(DisplayMessage::system(format_cache_stats(app)));
                app.set_status_notice("Cache stats");
            }
            "1h" | "1hour" | "extended" => {
                crate::provider::anthropic::set_cache_ttl_1h(true);
                app.push_display_message(DisplayMessage::system(
                    "Cache TTL set to 1 hour. Cache writes cost 2x base input tokens.".to_string(),
                ));
            }
            "5m" | "5min" | "default" | "reset" => {
                crate::provider::anthropic::set_cache_ttl_1h(false);
                app.push_display_message(DisplayMessage::system(
                    "Cache TTL set to 5 minutes (default).".to_string(),
                ));
            }
            "" => {
                let current = crate::provider::anthropic::is_cache_ttl_1h();
                let new_state = !current;
                crate::provider::anthropic::set_cache_ttl_1h(new_state);
                let msg = if new_state {
                    "Cache TTL toggled to 1 hour. Cache writes cost 2x base input tokens.\nUse `/cache 5m` to revert."
                } else {
                    "Cache TTL toggled to 5 minutes (default).\nUse `/cache 1h` to extend."
                };
                app.push_display_message(DisplayMessage::system(msg.to_string()));
            }
            _ => {
                app.push_display_message(DisplayMessage::error(
                    "Usage: `/cache` (toggle), `/cache stats`, `/cache 1h` (1 hour), `/cache 5m` (default)"
                        .to_string(),
                ));
            }
        }
        return true;
    }

    if trimmed == "/info" {
        let version = env!("JCODE_VERSION");
        let terminal_size = crossterm::terminal::size()
            .map(|(w, h)| format!("{}x{}", w, h))
            .unwrap_or_else(|_| "unknown".to_string());
        let cwd = std::env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| "unknown".to_string());

        let turn_count = app
            .display_messages
            .iter()
            .filter(|m| m.role == "user")
            .count();

        let session_duration = chrono::Utc::now().signed_duration_since(app.session.created_at);
        let duration_str = if session_duration.num_hours() > 0 {
            format!(
                "{}h {}m",
                session_duration.num_hours(),
                session_duration.num_minutes() % 60
            )
        } else if session_duration.num_minutes() > 0 {
            format!("{}m", session_duration.num_minutes())
        } else {
            format!("{}s", session_duration.num_seconds())
        };

        let mut info = String::new();
        info.push_str(&format!("**Version:** {}\n", version));
        info.push_str(&format!(
            "**Session:** {} ({})\n",
            app.session.short_name.as_deref().unwrap_or("unnamed"),
            &app.session.id[..8]
        ));
        info.push_str(&format!(
            "**Duration:** {} ({} turns)\n",
            duration_str, turn_count
        ));
        info.push_str(&format!(
            "**Tokens:** ↑{} ↓{}\n",
            app.total_input_tokens, app.total_output_tokens
        ));
        info.push_str(&format!("**Terminal:** {}\n", terminal_size));
        info.push_str(&format!("**CWD:** {}\n", cwd));
        info.push_str(&format!(
            "**Features:** memory={}, swarm={}\n",
            if app.memory_enabled { "on" } else { "off" },
            if app.swarm_enabled { "on" } else { "off" }
        ));

        if let Some(ref model) = app.remote_provider_model {
            info.push_str(&format!("**Model:** {}\n", model));
        }
        if let Some(ref provider_id) = app.provider_session_id {
            info.push_str(&format!(
                "**Provider Session:** {}...\n",
                &provider_id[..provider_id.len().min(16)]
            ));
        }

        if app.session.is_canary {
            info.push_str("\n**Self-Dev Mode:** enabled\n");
            if let Some(ref build) = app.session.testing_build {
                info.push_str(&format!("**Testing Build:** {}\n", build));
            }
        }

        if app.is_remote {
            info.push_str("\n**Remote Mode:** connected\n");
            if let Some(count) = app.remote_client_count {
                info.push_str(&format!("**Connected Clients:** {}\n", count));
            }
        }

        app.push_display_message(DisplayMessage {
            role: "system".to_string(),
            content: info,
            tool_calls: vec![],
            duration_secs: None,
            title: None,
            tool_data: None,
        });
        return true;
    }

    if trimmed == "/context" {
        let cwd = std::env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| "unknown".to_string());
        let terminal_size = crossterm::terminal::size()
            .map(|(w, h)| format!("{}x{}", w, h))
            .unwrap_or_else(|_| "unknown".to_string());
        let active_session_id = app
            .active_client_session_id()
            .unwrap_or(app.session.id.as_str())
            .to_string();
        let context = app.context_info();
        let todos = super::helpers::gather_todos_for_session(Some(active_session_id.as_str()));

        let (provider_name, model_name, reasoning_effort, service_tier, transport, total_tokens) =
            if app.is_remote {
                (
                    app.remote_provider_name
                        .clone()
                        .unwrap_or_else(|| app.provider.name().to_string()),
                    app.remote_provider_model
                        .clone()
                        .unwrap_or_else(|| app.provider.model()),
                    app.remote_reasoning_effort.clone(),
                    app.remote_service_tier.clone(),
                    app.remote_transport.clone(),
                    app.remote_total_tokens,
                )
            } else {
                (
                    app.provider.name().to_string(),
                    app.provider.model(),
                    app.provider.reasoning_effort(),
                    app.provider.service_tier(),
                    app.provider.transport(),
                    Some((app.total_input_tokens, app.total_output_tokens)),
                )
            };

        let compaction_summary = if app.provider.supports_compaction() {
            let manager = app.registry.compaction();
            if let Ok(manager) = manager.try_read() {
                let provider_messages = app.materialized_provider_messages();
                let stats = manager.stats_with(&provider_messages);
                let mode = if app.is_remote {
                    app.remote_compaction_mode
                        .as_ref()
                        .map(|mode| mode.as_str().to_string())
                        .unwrap_or_else(|| "unknown".to_string())
                } else {
                    manager.mode().as_str().to_string()
                };
                let summary_kind = match app.session.compaction.as_ref() {
                    Some(state) if state.openai_encrypted_content.is_some() => {
                        "native/openai-encrypted"
                    }
                    Some(_) => "summary-text",
                    None => "none",
                };
                format!(
                    "- supported: yes\n- mode: {}\n- jcode-managed: {}\n- active summary: {} ({})\n- compacted messages: {}\n- active messages: {}\n- summary chars: {}\n- estimated tokens: {}\n- effective tokens: {}\n- observed tokens: {}\n- usage: {:.1}%\n- compacting now: {}\n- budget: {}",
                    mode,
                    if app.provider.uses_jcode_compaction() {
                        "yes"
                    } else {
                        "no"
                    },
                    if stats.has_summary { "yes" } else { "no" },
                    summary_kind,
                    manager.compacted_count(),
                    stats.active_messages,
                    manager.summary_chars(),
                    stats.token_estimate,
                    stats.effective_tokens,
                    stats
                        .observed_input_tokens
                        .map(|v| v.to_string())
                        .unwrap_or_else(|| "n/a".to_string()),
                    stats.context_usage * 100.0,
                    if stats.is_compacting { "yes" } else { "no" },
                    manager.token_budget(),
                )
            } else {
                "- supported: yes\n- state: unavailable (compaction manager busy)".to_string()
            }
        } else {
            "- supported: no".to_string()
        };

        let pending_images = app.pending_images.len();
        let queued_messages = app.queued_messages.len();
        let soft_interrupts = app.pending_soft_interrupts.len();
        let side_panel_pages = app.side_panel.pages.len();
        let focused_side_panel = app.side_panel.focused_page_id.as_deref().unwrap_or("none");

        let mut todo_lines = String::new();
        if todos.is_empty() {
            todo_lines.push_str("- none\n");
        } else {
            for todo in todos.iter().take(8) {
                todo_lines.push_str(&format!(
                    "- [{}|{}] {}\n",
                    todo.status, todo.priority, todo.content
                ));
            }
            if todos.len() > 8 {
                todo_lines.push_str(&format!("- … {} more\n", todos.len() - 8));
            }
        }

        let mut context_report = String::new();
        context_report.push_str("# Session Context\n\n");
        context_report.push_str("## Runtime\n");
        context_report.push_str(&format!("- session id: `{}`\n", active_session_id));
        context_report.push_str(&format!("- session name: {}\n", app.session.display_name()));
        context_report.push_str(&format!(
            "- mode: {}{}{}\n",
            if app.is_remote { "remote" } else { "local" },
            if app.is_replay { ", replay" } else { "" },
            if app.session.is_canary {
                ", self-dev"
            } else {
                ""
            }
        ));
        context_report.push_str(&format!("- provider: {}\n", provider_name));
        context_report.push_str(&format!("- model: {}\n", model_name));
        context_report.push_str(&format!(
            "- reasoning effort: {}\n",
            reasoning_effort.as_deref().unwrap_or("default")
        ));
        context_report.push_str(&format!(
            "- service tier: {}\n",
            service_tier.as_deref().unwrap_or("default")
        ));
        context_report.push_str(&format!(
            "- transport: {}\n",
            transport.as_deref().unwrap_or("default")
        ));
        context_report.push_str(&format!("- cwd: {}\n", cwd));
        context_report.push_str(&format!("- terminal: {}\n", terminal_size));
        context_report.push_str(&format!(
            "- features: memory={}, swarm={}\n",
            if app.memory_enabled { "on" } else { "off" },
            if app.swarm_enabled { "on" } else { "off" }
        ));
        context_report.push_str(&format!(
            "- processing: {}\n",
            match &app.status {
                ProcessingStatus::Idle => "idle".to_string(),
                ProcessingStatus::Sending => "sending".to_string(),
                ProcessingStatus::Connecting(phase) => format!("connecting ({})", phase),
                ProcessingStatus::Thinking(_) => "thinking".to_string(),
                ProcessingStatus::Streaming => "streaming".to_string(),
                ProcessingStatus::RunningTool(name) => format!("running tool ({})", name),
            }
        ));
        if let Some((input, output)) = total_tokens {
            context_report.push_str(&format!("- session tokens: ↑{} ↓{}\n", input, output));
        }
        context_report.push_str("\n## Prompt / Context Composition\n");
        context_report.push_str(&format!(
            "- total chars: {} (~{} tokens)\n",
            context.total_chars,
            context.estimated_tokens()
        ));
        context_report.push_str(&format!(
            "- prompt prefix before any user text: {} chars (~{} tokens)\n- tool definitions only: {} chars (~{} tokens)\n",
            context.prompt_prefix_chars(),
            context.prompt_prefix_tokens(),
            context.tool_defs_chars,
            context.tool_definition_tokens(),
        ));
        context_report.push_str(&format!(
            "- system prompt: {} chars\n- session context: {} chars\n- project AGENTS.md: {} ({})\n- global ~/.AGENTS.md: {} ({})\n- prompt overlays: {} chars\n- skills section: {} chars\n- self-dev section: {} chars\n- memory section: {} chars\n- tool definitions: {} chars across {} tools\n- user messages: {} chars across {} messages\n- assistant messages: {} chars across {} messages\n- tool calls: {} chars across {} calls\n- tool results: {} chars across {} results\n",
            context.system_prompt_chars,
            context.session_context_chars,
            if context.has_project_agents_md { "loaded" } else { "not loaded" },
            context.project_agents_md_chars,
            if context.has_global_agents_md { "loaded" } else { "not loaded" },
            context.global_agents_md_chars,
            context.prompt_overlay_chars,
            context.skills_chars,
            context.selfdev_chars,
            context.memory_chars,
            context.tool_defs_chars,
            context.tool_defs_count,
            context.user_messages_chars,
            context.user_messages_count,
            context.assistant_messages_chars,
            context.assistant_messages_count,
            context.tool_calls_chars,
            context.tool_calls_count,
            context.tool_results_chars,
            context.tool_results_count,
        ));
        context_report.push_str("\n## Compaction\n");
        context_report.push_str(&compaction_summary);
        context_report.push_str("\n\n## Session State\n");
        context_report.push_str(&format!(
            "- queue mode: {}\n- queued messages: {}\n- interleave pending: {}\n- soft interrupts pending: {}\n- pasted snippets buffered: {}\n- pending images: {}\n- active skill: {}\n- autonomy mode: {}\n- subagent status: {}\n- provider session id: {}\n- status notice: {}\n- last stream error: {}\n- stashed input: {}\n",
            if app.queue_mode { "on" } else { "off" },
            queued_messages,
            if app.interleave_message.is_some() { "yes" } else { "no" },
            soft_interrupts,
            app.pasted_contents.len(),
            pending_images,
            app.active_skill.as_deref().unwrap_or("none"),
            app.improve_mode
                .map(|mode| mode.status_label())
                .unwrap_or("inactive"),
            app.subagent_status.as_deref().unwrap_or("idle"),
            app.provider_session_id.as_deref().unwrap_or("none"),
            app.status_notice()
                .as_deref()
                .unwrap_or("none"),
            app.last_stream_error.as_deref().unwrap_or("none"),
            if app.stashed_input.is_some() { "yes" } else { "no" },
        ));
        context_report.push_str("\n## Todos\n");
        context_report.push_str(&todo_lines);
        context_report.push_str("\n## Side Panel\n");
        context_report.push_str(&format!(
            "- pages: {}\n- focused page: {}\n",
            side_panel_pages, focused_side_panel
        ));

        if let Some(page) = app.side_panel.focused_page() {
            context_report.push_str(&format!(
                "- focused title: {}\n- focused source: {} ({})\n- focused content chars: {}\n",
                page.title,
                page.source.as_str(),
                page.format.as_str(),
                page.content.len(),
            ));
        }

        if app.swarm_enabled {
            context_report.push_str("\n## Swarm\n");
            context_report.push_str(&format!(
                "- plan items: {}\n- remote members: {}\n- connected clients: {}\n",
                app.swarm_plan_items.len(),
                app.remote_swarm_members.len(),
                app.remote_client_count
                    .map(|count| count.to_string())
                    .unwrap_or_else(|| "n/a".to_string()),
            ));
        }

        app.push_display_message(DisplayMessage::system(context_report).with_title("Context"));
        return true;
    }

    false
}
