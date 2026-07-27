use super::*;

impl SingleSessionApp {
    pub(crate) fn submit_draft(&mut self) -> KeyOutcome {
        let message = self.draft.trim().to_string();
        if message.is_empty() && self.pending_images.is_empty() {
            return KeyOutcome::None;
        }
        if self.pending_images.is_empty()
            && let Some(outcome) = self.handle_slash_command(&message)
        {
            return outcome;
        }
        let images = std::mem::take(&mut self.pending_images);
        self.record_user_submit(&message, &images);
        let Some(session) = &self.session else {
            return KeyOutcome::StartFreshSession { message, images };
        };
        let session_id = session.session_id.clone();
        let title = session.title.clone();
        KeyOutcome::SendDraft {
            session_id,
            title,
            message,
            images,
        }
    }

    pub(crate) fn handle_slash_command(&mut self, message: &str) -> Option<KeyOutcome> {
        if !message.starts_with('/') {
            return None;
        }

        let mut parts = message.splitn(2, char::is_whitespace);
        let command = parts.next().unwrap_or_default();
        let args = parts.next().unwrap_or_default().trim();

        if self.active_inline_widget() == Some(InlineWidgetKind::SlashSuggestions) {
            self.capture_inline_widget_exit();
        }

        let outcome = match command {
            "/help" | "/?" | "/commands" => {
                self.draft.clear();
                self.draft_cursor = 0;
                self.composer.input_undo_stack.clear();
                self.show_help = true;
                self.model_picker.close();
                self.session_switcher.close();
                self.mark_inline_widget_opened();
                self.set_status(SingleSessionStatus::Info(
                    "showing desktop slash commands".to_string(),
                ));
                self.scroll_body_to_bottom();
                KeyOutcome::Redraw
            }
            "/clear" => {
                self.messages.clear();
                self.streaming_response.clear();
                // Same gap as the TUI (issue #605): a cleared session's side
                // panel pages are orphaned, and nothing else resets them.
                self.side_panel = DesktopSidePanelState::default();
                self.reasoning.clear();
                self.error = None;
                self.is_processing = false;
                self.draft.clear();
                self.draft_cursor = 0;
                self.composer.input_undo_stack.clear();
                self.set_status(SingleSessionStatus::Info("session cleared".to_string()));
                self.scroll_body_to_bottom();
                if self.session.is_some() || self.live_session_id.is_some() {
                    KeyOutcome::ClearServerSession
                } else {
                    KeyOutcome::Redraw
                }
            }
            "/new" => {
                self.draft.clear();
                self.draft_cursor = 0;
                self.composer.input_undo_stack.clear();
                KeyOutcome::SpawnSession
            }
            "/issues" => {
                if matches!(args, "refresh" | "sync") {
                    return Some(self.toggle_issue_browser(Some(true)));
                }
                if args == "preview" {
                    let outcome = self.toggle_issue_browser(Some(true));
                    self.side_panel.focus = DesktopSidePanelFocus::IssuePreview;
                    return Some(outcome);
                }
                let visible = match args {
                    "on" | "open" | "show" => Some(true),
                    "off" | "close" | "hide" => Some(false),
                    _ => None,
                };
                self.toggle_issue_browser(visible)
            }
            "/sessions" | "/session" | "/resume" => {
                self.draft.clear();
                self.draft_cursor = 0;
                self.composer.input_undo_stack.clear();
                return Some(self.open_session_switcher());
            }
            "/model" | "/models" => {
                self.draft.clear();
                self.draft_cursor = 0;
                self.composer.input_undo_stack.clear();
                if args.is_empty() {
                    return Some(self.open_model_picker());
                }
                KeyOutcome::SetModel(args.to_string())
            }
            "/refresh-model-list" => {
                self.draft.clear();
                self.draft_cursor = 0;
                self.composer.input_undo_stack.clear();
                self.model_picker.open_loading();
                self.set_status(SingleSessionStatus::Info(
                    "refreshing model list".to_string(),
                ));
                KeyOutcome::RefreshModelCatalog
            }
            "/reload" | "/force-reload" => {
                self.draft.clear();
                self.draft_cursor = 0;
                self.composer.input_undo_stack.clear();
                self.set_status(SingleSessionStatus::Info(
                    "force reloading desktop".to_string(),
                ));
                KeyOutcome::ForceReload
            }
            "/effort" => {
                self.draft.clear();
                self.draft_cursor = 0;
                self.composer.input_undo_stack.clear();
                if args.is_empty() || args == "status" {
                    let current = self
                        .runtime_settings
                        .reasoning_effort
                        .as_deref()
                        .unwrap_or("default");
                    self.set_status(SingleSessionStatus::Info(format!(
                        "effort: {current} · use /effort <none|low|medium|high|xhigh|max>"
                    )));
                    KeyOutcome::Redraw
                } else if matches!(args, "none" | "low" | "medium" | "high" | "xhigh" | "max") {
                    KeyOutcome::SetReasoningEffort(args.to_string())
                } else {
                    self.set_status(SingleSessionStatus::Info(
                        "usage: /effort <none|low|medium|high|xhigh|max>".to_string(),
                    ));
                    KeyOutcome::Redraw
                }
            }
            "/thinking" | "/reasoning" | "/thinking-display" => {
                self.draft.clear();
                self.draft_cursor = 0;
                self.composer.input_undo_stack.clear();
                // `/effort` controls how hard the model thinks; this reports
                // whether the desktop transcript shows that thinking live.
                // The desktop cannot safely rewrite the shared config file, so
                // this explains where the setting lives rather than guessing.
                let current = crate::desktop_config::desktop_reasoning_display_mode();
                self.messages.push(SingleSessionMessage::meta(format!(
                    "thinking display: {current}. \
                     The desktop defaults to `current` (live thinking, collapsed once a tool runs \
                     or the answer commits). Change it with `/thinking-display <off|current|full>` \
                     in the terminal client, or set `display.reasoning_display` in ~/.jcode/config.toml. \
                     Use /effort to change how hard the model thinks."
                )));
                self.set_status(SingleSessionStatus::Info(format!(
                    "thinking display: {current}"
                )));
                KeyOutcome::Redraw
            }
            "/font" | "/fonts" => {
                self.draft.clear();
                self.draft_cursor = 0;
                self.composer.input_undo_stack.clear();
                let mut args = args.split_whitespace();
                match (args.next(), args.collect::<Vec<_>>().join(" ")) {
                    (None, _) | (Some("status"), _) => {
                        let options = SINGLE_SESSION_HANDWRITING_FONT_FAMILIES.join(", ");
                        self.set_status(SingleSessionStatus::Info(format!(
                            "fonts: user={} · ai={} · options: default, {options}",
                            single_session_user_font_family(),
                            single_session_assistant_font_family()
                        )));
                        KeyOutcome::Redraw
                    }
                    (Some("user"), value) if !value.is_empty() => {
                        if let Some(family) = set_single_session_user_font_family(&value) {
                            self.set_status(SingleSessionStatus::Info(format!(
                                "user font set to {family}"
                            )));
                        } else {
                            self.set_status(SingleSessionStatus::Info(
                                "unknown font · try /font status".to_string(),
                            ));
                        }
                        KeyOutcome::Redraw
                    }
                    (Some("ai" | "assistant"), value) if !value.is_empty() => {
                        if let Some(family) = set_single_session_assistant_font_family(&value) {
                            self.set_status(SingleSessionStatus::Info(format!(
                                "AI font set to {family}"
                            )));
                        } else {
                            self.set_status(SingleSessionStatus::Info(
                                "unknown font · try /font status".to_string(),
                            ));
                        }
                        KeyOutcome::Redraw
                    }
                    _ => {
                        self.set_status(SingleSessionStatus::Info(
                            "usage: /font [status|user <name>|ai <name>]".to_string(),
                        ));
                        KeyOutcome::Redraw
                    }
                }
            }
            "/fast" => {
                self.draft.clear();
                self.draft_cursor = 0;
                self.composer.input_undo_stack.clear();
                match args {
                    "" | "status" => {
                        let current = self
                            .runtime_settings
                            .service_tier
                            .as_deref()
                            .unwrap_or("standard");
                        self.set_status(SingleSessionStatus::Info(format!(
                            "fast mode: {current} · use /fast <on|off|status>"
                        )));
                        KeyOutcome::Redraw
                    }
                    "on" => KeyOutcome::SetServiceTier("priority".to_string()),
                    "off" => KeyOutcome::SetServiceTier("off".to_string()),
                    _ => {
                        self.set_status(SingleSessionStatus::Info(
                            "usage: /fast [on|off|status]".to_string(),
                        ));
                        KeyOutcome::Redraw
                    }
                }
            }
            "/transport" => {
                self.draft.clear();
                self.draft_cursor = 0;
                self.composer.input_undo_stack.clear();
                match args {
                    "" | "status" => {
                        let current = self
                            .runtime_settings
                            .transport
                            .as_deref()
                            .unwrap_or("unknown");
                        self.set_status(SingleSessionStatus::Info(format!(
                            "transport: {current} · use /transport <auto|https|websocket>"
                        )));
                        KeyOutcome::Redraw
                    }
                    "auto" | "https" | "websocket" => KeyOutcome::SetTransport(args.to_string()),
                    _ => {
                        self.set_status(SingleSessionStatus::Info(
                            "usage: /transport <auto|https|websocket>".to_string(),
                        ));
                        KeyOutcome::Redraw
                    }
                }
            }
            "/compact" => {
                self.draft.clear();
                self.draft_cursor = 0;
                self.composer.input_undo_stack.clear();
                if args.is_empty() {
                    KeyOutcome::CompactSession
                } else if args == "mode" || args == "mode status" {
                    let current = self
                        .runtime_settings
                        .compaction_mode
                        .as_deref()
                        .unwrap_or("reactive");
                    self.set_status(SingleSessionStatus::Info(format!(
                        "compaction: {current} · use /compact mode <reactive|proactive|semantic>"
                    )));
                    KeyOutcome::Redraw
                } else if let Some(mode) = args.strip_prefix("mode ") {
                    let mode = mode.trim();
                    if matches!(mode, "reactive" | "proactive" | "semantic") {
                        KeyOutcome::SetCompactionMode(mode.to_string())
                    } else {
                        self.set_status(SingleSessionStatus::Info(
                            "usage: /compact mode <reactive|proactive|semantic>".to_string(),
                        ));
                        KeyOutcome::Redraw
                    }
                } else {
                    self.set_status(SingleSessionStatus::Info(
                        "usage: /compact [mode <reactive|proactive|semantic>]".to_string(),
                    ));
                    KeyOutcome::Redraw
                }
            }
            "/commit" => {
                self.draft.clear();
                self.draft_cursor = 0;
                self.composer.input_undo_stack.clear();
                let message = desktop_commit_prompt();
                let Some(session) = &self.session else {
                    return Some(KeyOutcome::StartFreshSession {
                        message,
                        images: Vec::new(),
                    });
                };
                let session_id = session.session_id.clone();
                let title = session.title.clone();
                self.set_status(SingleSessionStatus::Info(
                    "starting logical commits".to_string(),
                ));
                return Some(KeyOutcome::SendDraft {
                    session_id,
                    title,
                    message,
                    images: Vec::new(),
                });
            }
            "/rename" => {
                self.draft.clear();
                self.draft_cursor = 0;
                self.composer.input_undo_stack.clear();
                if args.is_empty() {
                    self.set_status(SingleSessionStatus::Info(
                        "usage: /rename <session name> or /rename --clear".to_string(),
                    ));
                    KeyOutcome::Redraw
                } else if args == "--clear" {
                    KeyOutcome::RenameSession(None)
                } else {
                    KeyOutcome::RenameSession(Some(args.to_string()))
                }
            }
            "/usage" => {
                self.draft.clear();
                self.draft_cursor = 0;
                self.composer.input_undo_stack.clear();
                let usage = self.runtime_settings.token_usage.as_ref();
                let message = usage
                    .map(|usage| {
                        format!(
                            "desktop /usage overlay is not implemented yet · latest tokens: input={} output={}",
                            usage.input, usage.output
                        )
                    })
                    .unwrap_or_else(|| {
                        "desktop /usage overlay is not implemented yet · no token usage received for this session".to_string()
                    });
                self.set_status(SingleSessionStatus::Info(message));
                KeyOutcome::Redraw
            }
            "/todo" | "/todos" => {
                self.draft.clear();
                self.draft_cursor = 0;
                self.composer.input_undo_stack.clear();
                self.set_status(SingleSessionStatus::Info(
                    "desktop todo panel is not implemented yet · todo tool output is shown in transcript".to_string(),
                ));
                KeyOutcome::Redraw
            }
            "/memory" => {
                self.draft.clear();
                self.draft_cursor = 0;
                self.composer.input_undo_stack.clear();
                self.set_status(SingleSessionStatus::Info(
                    "desktop memory panel is not implemented yet · memory server events are not surfaced".to_string(),
                ));
                KeyOutcome::Redraw
            }
            "/changelog" => {
                self.draft.clear();
                self.draft_cursor = 0;
                self.composer.input_undo_stack.clear();
                self.set_status(SingleSessionStatus::Info(
                    "desktop changelog overlay is not implemented yet".to_string(),
                ));
                KeyOutcome::Redraw
            }
            "/diff" => {
                self.draft.clear();
                self.draft_cursor = 0;
                self.composer.input_undo_stack.clear();
                self.set_status(SingleSessionStatus::Info(
                    "desktop diff viewer is not implemented yet".to_string(),
                ));
                KeyOutcome::Redraw
            }
            "/account" | "/auth" => {
                self.draft.clear();
                self.draft_cursor = 0;
                self.composer.input_undo_stack.clear();
                self.set_status(SingleSessionStatus::Info(
                    "desktop account picker is not implemented yet · use the TUI for account management".to_string(),
                ));
                KeyOutcome::Redraw
            }
            "/swarm" | "/bg" => {
                self.draft.clear();
                self.draft_cursor = 0;
                self.composer.input_undo_stack.clear();
                self.set_status(SingleSessionStatus::Info(format!(
                    "desktop {command} panel is not implemented yet · related tool output is shown in transcript"
                )));
                KeyOutcome::Redraw
            }
            "/copy" => {
                self.draft.clear();
                self.draft_cursor = 0;
                self.composer.input_undo_stack.clear();
                return Some(match args {
                    "" | "latest" | "response" => self
                        .latest_assistant_response()
                        .map(KeyOutcome::CopyLatestResponse)
                        .unwrap_or_else(|| {
                            self.set_status(SingleSessionStatus::Info(
                                "no assistant response to copy".to_string(),
                            ));
                            KeyOutcome::Redraw
                        }),
                    "code" | "codeblock" | "code-block" => self
                        .latest_rich_code_block_text()
                        .map(|text| KeyOutcome::CopyText {
                            text,
                            success_notice: "copied latest code block",
                        })
                        .unwrap_or_else(|| {
                            self.set_status(SingleSessionStatus::Info(
                                "no code block to copy".to_string(),
                            ));
                            KeyOutcome::Redraw
                        }),
                    "transcript" | "all" => self
                        .copy_rich_transcript_text(
                            desktop_rich_text::TranscriptCopyMode::TranscriptPlainText,
                        )
                        .filter(|text| !text.trim().is_empty())
                        .map(|text| KeyOutcome::CopyText {
                            text,
                            success_notice: "copied transcript",
                        })
                        .unwrap_or_else(|| {
                            self.set_status(SingleSessionStatus::Info(
                                "no transcript to copy".to_string(),
                            ));
                            KeyOutcome::Redraw
                        }),
                    _ => {
                        self.set_status(SingleSessionStatus::Info(
                            "usage: /copy [latest|code|transcript]".to_string(),
                        ));
                        KeyOutcome::Redraw
                    }
                });
            }
            "/search" => {
                self.draft.clear();
                self.draft_cursor = 0;
                self.composer.input_undo_stack.clear();
                if args.is_empty() {
                    self.set_status(SingleSessionStatus::Info(
                        "usage: /search <query>".to_string(),
                    ));
                    KeyOutcome::Redraw
                } else {
                    let matches = self.search_rich_transcript(args);
                    if let Some(first) = matches.first() {
                        let body_len = self.body_lines().len();
                        self.body_scroll_lines =
                            body_len.saturating_sub(first.line_index + 1) as f32;
                    }
                    self.set_status(SingleSessionStatus::Info(format!(
                        "{} match(es) for \"{}\"",
                        matches.len(),
                        args
                    )));
                    KeyOutcome::Redraw
                }
            }
            "/stop" | "/cancel" => {
                self.draft.clear();
                self.draft_cursor = 0;
                self.composer.input_undo_stack.clear();
                if self.is_processing {
                    KeyOutcome::CancelGeneration
                } else {
                    self.set_status(SingleSessionStatus::Info("nothing is running".to_string()));
                    KeyOutcome::Redraw
                }
            }
            "/status" | "/info" => {
                self.draft.clear();
                self.draft_cursor = 0;
                self.composer.input_undo_stack.clear();
                self.show_help = false;
                self.show_session_info = true;
                self.model_picker.close();
                self.session_switcher.close();
                self.mark_inline_widget_opened();
                self.set_status(SingleSessionStatus::Info(
                    "showing session info".to_string(),
                ));
                self.scroll_body_to_bottom();
                KeyOutcome::Redraw
            }
            "/quit" | "/exit" => KeyOutcome::Exit,
            _ => {
                self.set_status(SingleSessionStatus::Info(format!(
                    "unknown desktop slash command: {command} · try /help"
                )));
                KeyOutcome::Redraw
            }
        };

        Some(outcome)
    }

    pub(crate) fn attach_image(&mut self, media_type: String, base64_data: String) {
        self.pending_images.push((media_type, base64_data));
        self.set_status(SingleSessionStatus::AttachedImages(
            self.pending_images.len(),
        ));
    }

    pub(crate) fn clear_attached_images(&mut self) -> bool {
        if self.pending_images.is_empty() {
            return false;
        }
        self.pending_images.clear();
        self.set_status(SingleSessionStatus::Info(
            "cleared image attachments".to_string(),
        ));
        true
    }

    pub(crate) fn accepts_clipboard_image_paste(&self) -> bool {
        self.stdin_response.is_none() && !self.model_picker.open && !self.session_switcher.open
    }

    pub(crate) fn paste_text(&mut self, text: &str) {
        if !text.is_empty() {
            if let Some(stdin_response) = &mut self.stdin_response {
                stdin_response.input.push_str(text);
                return;
            }
            self.insert_draft_text(text);
        }
    }

    pub(crate) fn send_stdin_response(
        &mut self,
        request_id: String,
        input: String,
    ) -> anyhow::Result<()> {
        let Some(handle) = &self.runtime.session_handle else {
            anyhow::bail!("no active desktop session to receive interactive input");
        };
        handle.send_stdin_response(request_id, input)?;
        self.clear_tool_stdin_prompts();
        self.set_status(SingleSessionStatus::Info(
            "interactive input sent".to_string(),
        ));
        Ok(())
    }

    pub(crate) fn set_reasoning_effort_via_active_session(
        &mut self,
        effort: String,
    ) -> anyhow::Result<()> {
        let Some(handle) = &self.runtime.session_handle else {
            anyhow::bail!("no active desktop session to receive reasoning effort change");
        };
        handle.set_reasoning_effort(effort)
    }

    pub(crate) fn queue_draft(&mut self) -> KeyOutcome {
        let message = self.draft.trim().to_string();
        if message.is_empty() && self.pending_images.is_empty() {
            return KeyOutcome::None;
        }
        let images = std::mem::take(&mut self.pending_images);
        self.composer.queued_drafts.push((message.clone(), images));
        self.messages.push(SingleSessionMessage::meta(format!(
            "queued prompt: {message}"
        )));
        self.draft.clear();
        self.draft_cursor = 0;
        self.composer.input_undo_stack.clear();
        self.set_status(SingleSessionStatus::Info(format!(
            "{} prompt(s) queued",
            self.composer.queued_drafts.len()
        )));
        KeyOutcome::Redraw
    }

    pub(crate) fn retrieve_queued_draft_for_edit(&mut self) -> KeyOutcome {
        let Some((message, images)) = self.composer.queued_drafts.pop() else {
            return KeyOutcome::None;
        };
        self.remember_input_undo_state();
        self.draft = message;
        self.draft_cursor = self.draft.len();
        self.pending_images = images;
        self.set_status(SingleSessionStatus::Info(format!(
            "{} prompt(s) queued",
            self.composer.queued_drafts.len()
        )));
        KeyOutcome::Redraw
    }

    pub(crate) fn take_next_queued_draft(&mut self) -> Option<(String, Vec<(String, String)>)> {
        if self.is_processing || self.error.is_some() || self.composer.queued_drafts.is_empty() {
            return None;
        }
        let (message, images) = self.composer.queued_drafts.remove(0);
        self.record_user_submit(&message, &images);
        Some((message, images))
    }
}
