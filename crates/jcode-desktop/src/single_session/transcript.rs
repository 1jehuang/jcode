use super::*;

impl SingleSessionApp {
    pub(crate) fn body_lines(&self) -> Vec<String> {
        self.body_styled_lines()
            .into_iter()
            .map(|line| line.text)
            .collect()
    }

    pub(crate) fn body_styled_lines(&self) -> Vec<SingleSessionStyledLine> {
        if let Some(stdin_response) = &self.stdin_response {
            return stdin_response_styled_lines(stdin_response);
        }
        self.body_styled_lines_without_inline_widgets()
    }

    pub(crate) fn body_styled_lines_without_inline_widgets(&self) -> Vec<SingleSessionStyledLine> {
        if !self.messages.is_empty()
            || !self.streaming_response.is_empty()
            || !self.reasoning.live.trim().is_empty()
            || self.error.is_some()
        {
            return self.transcript_styled_lines(true);
        }

        if self.is_welcome_timeline_visible() {
            if let Some(status) = &self.status
                && self.session.is_none()
                && !self.model_picker.open
                && !self.show_session_info
            {
                return vec![styled_line(status.clone(), SingleSessionLineStyle::Status)];
            }
            if self.welcome.recovery_session_count > 0 {
                return welcome_recovery_styled_lines(self.welcome.recovery_session_count);
            }
            return Vec::new();
        }

        if let Some(status) = &self.status
            && self.session.is_none()
            && !self.model_picker.open
            && !self.show_session_info
        {
            return vec![styled_line(status.clone(), SingleSessionLineStyle::Status)];
        }

        single_session_styled_lines(self.session.as_ref())
    }

    pub(crate) fn body_styled_lines_for_tick(&self, _tick: u64) -> Vec<SingleSessionStyledLine> {
        self.body_styled_lines()
    }

    pub(crate) fn body_styled_lines_without_streaming_response(
        &self,
    ) -> Option<Vec<SingleSessionStyledLine>> {
        if self.stdin_response.is_some()
            || self.session_switcher.open
            || self.model_picker.open
            || self.show_help
            || self.error.is_some()
        {
            return None;
        }
        if self.messages.is_empty()
            && self.streaming_response.is_empty()
            && self.reasoning.live.trim().is_empty()
        {
            return None;
        }
        Some(self.transcript_styled_lines(false))
    }

    pub(crate) fn streaming_response_styled_lines(&self) -> Vec<SingleSessionStyledLine> {
        self.streaming_response_revealed_styled_lines(self.streaming_response.len())
    }

    /// Styled lines for the first `revealed_bytes` of the streaming response.
    /// Drives the adaptive streaming reveal: the renderer grows the visible
    /// prefix smoothly instead of popping whole provider chunks in at once.
    pub(crate) fn streaming_response_revealed_styled_lines(
        &self,
        revealed_bytes: usize,
    ) -> Vec<SingleSessionStyledLine> {
        let mut end = revealed_bytes.min(self.streaming_response.len());
        while end > 0 && !self.streaming_response.is_char_boundary(end) {
            end -= 1;
        }
        let revealed = self.streaming_response[..end].trim_end();
        let mut lines = Vec::new();
        if !revealed.is_empty() {
            append_streaming_assistant_lines(&mut lines, revealed);
        }
        lines
    }

    pub(crate) fn transcript_styled_lines(
        &self,
        include_streaming_response: bool,
    ) -> Vec<SingleSessionStyledLine> {
        let mut lines = Vec::new();
        let mut user_turn = 1;
        let mut message_index = 0;
        while message_index < self.messages.len() {
            if !lines.is_empty() {
                lines.push(blank_styled_line());
            }
            let message = &self.messages[message_index];
            if message.role() == SingleSessionRole::Tool {
                let group_start = message_index;
                while message_index < self.messages.len()
                    && self.messages[message_index].role() == SingleSessionRole::Tool
                {
                    message_index += 1;
                }
                let tool_messages = &self.messages[group_start..message_index];
                let group_contains_active_tool = self
                    .tool
                    .active_message_index
                    .is_some_and(|index| (group_start..message_index).contains(&index));
                if tool_messages.len() > 1 && !group_contains_active_tool {
                    append_tool_group_summary(&mut lines, tool_messages);
                } else {
                    for (offset, tool_message) in tool_messages.iter().enumerate() {
                        let is_active_tool = self.tool.active_message_index
                            == Some(group_start.saturating_add(offset));
                        let tool_run =
                            self.tool_run_for_message_index(group_start.saturating_add(offset));
                        append_chat_message_lines(
                            &mut lines,
                            tool_message,
                            &mut user_turn,
                            is_active_tool,
                            if is_active_tool {
                                Some(self.tool.input_buffer.as_str())
                            } else {
                                None
                            },
                            tool_run,
                        );
                    }
                }
                continue;
            }
            append_chat_message_lines(&mut lines, message, &mut user_turn, false, None, None);
            message_index += 1;
        }
        if include_streaming_response {
            let reasoning_tail = self.reasoning.visible_tail();
            if !reasoning_tail.is_empty() {
                if !lines.is_empty() {
                    lines.push(blank_styled_line());
                }
                for tail_line in reasoning_tail {
                    lines.push(styled_line(
                        format!("  {tail_line}"),
                        SingleSessionLineStyle::Meta,
                    ));
                }
            }
        }
        if include_streaming_response && !self.streaming_response.is_empty() {
            if !lines.is_empty() {
                lines.push(blank_styled_line());
            }
            append_streaming_assistant_lines(&mut lines, self.streaming_response.trim_end());
        }
        if let Some(error) = &self.error {
            if !lines.is_empty() {
                lines.push(blank_styled_line());
            }
            lines.push(styled_line(
                format!("error: {error}"),
                SingleSessionLineStyle::Error,
            ));
        }
        lines
    }

    pub(crate) fn rendered_body_cache_key(&self, size: (u32, u32)) -> u64 {
        let mut hasher = DefaultHasher::new();
        size.hash(&mut hasher);
        self.session
            .as_ref()
            .map(|session| {
                (
                    session.session_id.as_str(),
                    session.title.as_str(),
                    session.subtitle.as_str(),
                    session.detail.as_str(),
                    session.preview_lines.as_slice(),
                    session.detail_lines.as_slice(),
                )
            })
            .hash(&mut hasher);
        hash_messages_cache_fingerprint(&self.messages, &mut hasher);
        hash_text_cache_fingerprint(&self.streaming_response, &mut hasher);
        hash_text_cache_fingerprint(&self.reasoning.live, &mut hasher);
        self.reasoning.streaming.hash(&mut hasher);
        hash_tool_cache_fingerprint(&self.tool, &mut hasher);
        self.status.hash(&mut hasher);
        self.error.hash(&mut hasher);
        self.show_help.hash(&mut hasher);
        self.show_session_info.hash(&mut hasher);
        self.model_picker.open.hash(&mut hasher);
        self.model_picker.filter.hash(&mut hasher);
        self.model_picker.selected.hash(&mut hasher);
        hash_session_switcher_cache_state(&self.session_switcher, &mut hasher);
        self.stdin_response.hash(&mut hasher);
        self.welcome.name.hash(&mut hasher);
        self.welcome.recovery_session_count.hash(&mut hasher);
        self.welcome.continuation_suggestion.hash(&mut hasher);
        self.welcome.timeline.hash(&mut hasher);
        self.welcome.hero_phrase_index.hash(&mut hasher);
        self.view.text_scale.to_bits().hash(&mut hasher);
        hasher.finish()
    }

    pub(crate) fn rendered_body_static_cache_key(&self, size: (u32, u32)) -> u64 {
        let mut hasher = DefaultHasher::new();
        size.hash(&mut hasher);
        self.session
            .as_ref()
            .map(|session| {
                (
                    session.session_id.as_str(),
                    session.title.as_str(),
                    session.subtitle.as_str(),
                    session.detail.as_str(),
                    session.preview_lines.as_slice(),
                    session.detail_lines.as_slice(),
                )
            })
            .hash(&mut hasher);
        hash_messages_cache_fingerprint(&self.messages, &mut hasher);
        hash_tool_cache_fingerprint(&self.tool, &mut hasher);
        self.status.hash(&mut hasher);
        self.error.hash(&mut hasher);
        self.show_help.hash(&mut hasher);
        self.show_session_info.hash(&mut hasher);
        self.model_picker.open.hash(&mut hasher);
        self.model_picker.filter.hash(&mut hasher);
        self.model_picker.selected.hash(&mut hasher);
        hash_session_switcher_cache_state(&self.session_switcher, &mut hasher);
        self.stdin_response.hash(&mut hasher);
        self.welcome.name.hash(&mut hasher);
        self.welcome.recovery_session_count.hash(&mut hasher);
        self.welcome.timeline.hash(&mut hasher);
        self.welcome.hero_phrase_index.hash(&mut hasher);
        self.view.text_scale.to_bits().hash(&mut hasher);
        hasher.finish()
    }

    pub(crate) fn apply_session_event(&mut self, event: DesktopSessionEvent) {
        match event {
            DesktopSessionEvent::Status(status) => self.set_backend_status(status),
            DesktopSessionEvent::Reloading { .. } => {
                self.set_status(SingleSessionStatus::ServerReloading);
                self.is_processing = true;
                self.runtime.reload_phase = ReloadPhase::AwaitingReconnect;
            }
            DesktopSessionEvent::ReloadProgress {
                step,
                message,
                success,
                output,
            } => {
                let marker = match success {
                    Some(true) => "✓ ",
                    Some(false) => "✗ ",
                    None => "",
                };
                let mut line = format!("reload {step}: {marker}{message}");
                if let Some(output) = output.as_deref().filter(|output| !output.trim().is_empty()) {
                    line.push_str(" — ");
                    line.push_str(output.trim());
                }
                self.messages.push(SingleSessionMessage::meta(line));
                self.set_status(SingleSessionStatus::Info(format!("reload: {message}")));
            }
            DesktopSessionEvent::RuntimeMetadata {
                connection_type,
                status_detail,
                upstream_provider,
            } => {
                if let Some(connection_type) = connection_type {
                    self.runtime_settings.connection_type = Some(connection_type);
                }
                if let Some(upstream_provider) = upstream_provider {
                    self.runtime_settings.upstream_provider = Some(upstream_provider);
                }
                if let Some(status_detail) = status_detail {
                    self.runtime_settings.status_detail = Some(status_detail.clone());
                    self.set_status(SingleSessionStatus::Info(status_detail));
                }
            }
            DesktopSessionEvent::TokenUsage {
                input,
                output,
                cache_read_input,
                cache_creation_input,
            } => {
                self.runtime_settings.token_usage = Some(SingleSessionTokenUsage {
                    input,
                    output,
                    cache_read_input,
                    cache_creation_input,
                });
            }
            DesktopSessionEvent::SystemNotice { title, message } => {
                let line = message
                    .as_deref()
                    .filter(|message| !message.trim().is_empty())
                    .map(|message| format!("{title}: {}", message.trim()))
                    .unwrap_or(title.clone());
                self.messages.push(SingleSessionMessage::meta(line));
                self.set_status(SingleSessionStatus::Info(title));
            }
            DesktopSessionEvent::SessionCloseRequested { reason } => {
                self.finish_streaming_response();
                self.is_processing = false;
                self.stdin_response = None;
                self.runtime.session_handle = None;
                self.set_status(SingleSessionStatus::Info(
                    "session close requested".to_string(),
                ));
                self.messages.push(SingleSessionMessage::meta(format!(
                    "session close requested by server: {reason}"
                )));
            }
            DesktopSessionEvent::Reloaded { session_id } => {
                self.live_session_id = Some(session_id);
                self.set_status(SingleSessionStatus::ServerReconnected);
                self.is_processing = true;
                self.runtime.reload_phase = ReloadPhase::Stable;
            }
            DesktopSessionEvent::SessionStarted { session_id } => {
                self.live_session_id = Some(session_id);
                self.set_status(SingleSessionStatus::Connected);
                self.notice_if_thinking_display_is_off();
            }
            DesktopSessionEvent::SessionRenamed {
                title,
                display_title,
            } => {
                if let Some(session) = &mut self.session {
                    session.title = display_title.clone();
                }
                let message = if title.is_some() {
                    format!("renamed session to {display_title}")
                } else {
                    format!("cleared session name; title is now {display_title}")
                };
                self.messages.push(SingleSessionMessage::meta(message));
                self.set_status(SingleSessionStatus::Info(if title.is_some() {
                    "session renamed".to_string()
                } else {
                    "session name cleared".to_string()
                }));
            }
            DesktopSessionEvent::TextDelta(text) => {
                self.runtime.reload_phase = ReloadPhase::Stable;
                // Answer text supersedes the live thinking tail for this step.
                self.finish_reasoning(None);
                self.streaming_response.push_str(&text);
                self.set_status(SingleSessionStatus::Receiving);
            }
            DesktopSessionEvent::ReasoningDelta(text) => {
                self.runtime.reload_phase = ReloadPhase::Stable;
                self.reasoning.live.push_str(&text);
                self.reasoning.streaming = true;
                self.set_status(SingleSessionStatus::Thinking);
            }
            DesktopSessionEvent::ReasoningDone { duration_ms } => {
                self.runtime.reload_phase = ReloadPhase::Stable;
                self.finish_reasoning(duration_ms);
            }
            DesktopSessionEvent::TextReplace(text) => {
                self.runtime.reload_phase = ReloadPhase::Stable;
                self.streaming_response = text;
                self.set_status(SingleSessionStatus::Receiving);
            }
            DesktopSessionEvent::ToolStarted { id, name } => {
                self.runtime.reload_phase = ReloadPhase::Stable;
                self.finish_reasoning(None);
                self.finish_streaming_response();
                self.collapse_active_tool_message();
                self.tool.input_buffer.clear();
                self.set_status(SingleSessionStatus::ToolPreparing(name.clone()));
                self.messages
                    .push(SingleSessionMessage::tool(format!("▾ {name} preparing")));
                self.tool.active_message_index = Some(self.messages.len().saturating_sub(1));
                let message_index = self.messages.len().saturating_sub(1);
                self.start_tool_run(id, &name, message_index);
            }
            DesktopSessionEvent::ToolExecuting { id, name } => {
                self.runtime.reload_phase = ReloadPhase::Stable;
                self.finish_streaming_response();
                self.set_status(SingleSessionStatus::ToolUsing(name.clone()));
                self.update_tool_run_state(id, &name, SingleSessionToolVisualState::Running, None);
                self.replace_active_tool_header(&format!("▾ {name} running"));
            }
            DesktopSessionEvent::ToolInput { id, delta } => {
                self.runtime.reload_phase = ReloadPhase::Stable;
                self.finish_streaming_response();
                self.append_tool_run_input(id, &delta);
                self.append_active_tool_input(&delta);
            }
            DesktopSessionEvent::ToolFinished {
                id,
                name,
                summary,
                is_error,
            } => {
                self.runtime.reload_phase = ReloadPhase::Stable;
                self.finish_streaming_response();
                self.set_status(SingleSessionStatus::ToolFinished {
                    name: name.clone(),
                    is_error,
                });
                let marker = if is_error { "failed" } else { "done" };
                let line = format!("▾ {name} {marker}: {summary}");
                let finished_call_id = self.update_tool_run_state(
                    id,
                    &name,
                    if is_error {
                        SingleSessionToolVisualState::Failed
                    } else {
                        SingleSessionToolVisualState::Succeeded
                    },
                    Some(summary.clone()),
                );
                self.flush_active_tool_input_to_message();
                if let Some(index) = self.tool.active_message_index
                    && let Some(message) = self.messages.get_mut(index)
                    && message.role() == SingleSessionRole::Tool
                {
                    let replacement =
                        merge_tool_finish_with_existing_context(message.content(), &line);
                    message.set_content(replacement);
                } else {
                    self.messages.push(SingleSessionMessage::tool(line));
                    let message_index = self.messages.len().saturating_sub(1);
                    self.tool.active_message_index = Some(message_index);
                    if let Some(run) = self
                        .tool
                        .runs
                        .iter_mut()
                        .find(|run| run.call_id == finished_call_id)
                    {
                        run.message_index = message_index;
                    }
                }
            }
            DesktopSessionEvent::ModelChanged {
                model,
                provider_name,
                error,
            } => {
                if let Some(error) = error {
                    self.set_status(SingleSessionStatus::ModelSwitchFailed);
                    self.model_picker.apply_error(error.clone());
                    self.messages.push(SingleSessionMessage::meta(format!(
                        "model switch failed: {error}"
                    )));
                    return;
                }
                let label = provider_name
                    .as_deref()
                    .filter(|provider| !provider.is_empty())
                    .map(|provider| format!("{provider} · {model}"))
                    .unwrap_or_else(|| model.clone());
                self.model_picker
                    .apply_model_change(model.clone(), provider_name.clone());
                self.set_status(SingleSessionStatus::ModelSelected(label.clone()));
                self.messages.push(SingleSessionMessage::meta(format!(
                    "model switched to {label}"
                )));
            }
            DesktopSessionEvent::ModelCatalog {
                current_model,
                provider_name,
                models,
                reasoning_effort,
                service_tier,
                compaction_mode,
            } => {
                if let Some(reasoning_effort) = reasoning_effort {
                    self.runtime_settings.reasoning_effort = Some(reasoning_effort);
                }
                if let Some(service_tier) = service_tier {
                    self.runtime_settings.service_tier = Some(service_tier);
                }
                if let Some(compaction_mode) = compaction_mode {
                    self.runtime_settings.compaction_mode = Some(compaction_mode);
                }
                self.model_picker
                    .apply_catalog(current_model, provider_name, models);
                self.set_status(SingleSessionStatus::ModelsLoaded);
            }
            DesktopSessionEvent::ModelCatalogError { error } => {
                self.model_picker.apply_error(error.clone());
                self.set_status(SingleSessionStatus::ModelPickerError);
            }
            DesktopSessionEvent::StdinRequest {
                request_id,
                prompt,
                is_password,
                tool_call_id,
            } => {
                self.runtime.reload_phase = ReloadPhase::Stable;
                self.set_status(SingleSessionStatus::InteractiveInputRequested);
                self.close_inline_widgets();
                let raw_prompt = prompt.trim();
                let display_prompt = if raw_prompt.is_empty() {
                    "interactive input requested"
                } else {
                    raw_prompt
                };
                self.stdin_response = Some(StdinResponseState {
                    request_id: request_id.clone(),
                    prompt: display_prompt.to_string(),
                    is_password,
                    tool_call_id: tool_call_id.clone(),
                    input: String::new(),
                });
                self.mark_tool_stdin_prompt(&tool_call_id, display_prompt);
                let sensitive = if is_password { " password" } else { "" };
                self.messages.push(SingleSessionMessage::meta(format!(
                    "interactive{sensitive} input requested by {tool_call_id} ({request_id}): {display_prompt}"
                )));
            }
            DesktopSessionEvent::Done => {
                if self.runtime.reload_phase == ReloadPhase::AwaitingReconnect {
                    self.set_status(SingleSessionStatus::ServerReloading);
                    self.is_processing = true;
                    return;
                }
                self.finish_streaming_response();
                self.is_processing = false;
                self.stdin_response = None;
                self.runtime.session_handle = None;
                self.tool.active_message_index = None;
                self.tool.active_call_id = None;
                self.tool.input_buffer.clear();
                self.clear_tool_stdin_prompts();
                self.set_status(SingleSessionStatus::Ready);
            }
            DesktopSessionEvent::Error(error) => {
                self.runtime.reload_phase = ReloadPhase::Stable;
                self.finish_streaming_response();
                self.is_processing = false;
                self.stdin_response = None;
                self.runtime.session_handle = None;
                self.tool.active_message_index = None;
                self.tool.active_call_id = None;
                self.tool.input_buffer.clear();
                self.clear_tool_stdin_prompts();
                self.set_status(SingleSessionStatus::Error);
                self.error = Some(error);
            }
        }
    }

    pub(crate) fn set_session_handle(&mut self, handle: DesktopSessionHandle) {
        self.runtime.session_handle = Some(handle);
    }

    pub(crate) fn cancel_generation(&mut self) -> bool {
        let Some(handle) = &self.runtime.session_handle else {
            return false;
        };
        match handle.cancel() {
            Ok(()) => {
                self.stdin_response = None;
                self.clear_tool_stdin_prompts();
                self.set_status(SingleSessionStatus::Cancelling);
                true
            }
            Err(error) => {
                self.error = Some(format!("{error:#}"));
                self.is_processing = false;
                self.stdin_response = None;
                self.clear_tool_stdin_prompts();
                self.runtime.session_handle = None;
                true
            }
        }
    }

    pub(crate) fn scroll_body_lines(&mut self, lines: impl Into<f64>) {
        let lines = lines.into() as f32;
        if !lines.is_finite() || lines.abs() < f32::EPSILON {
            return;
        }
        self.body_scroll_lines = (self.body_scroll_lines + lines).max(0.0);
    }

    pub(crate) fn scroll_body_to_top(&mut self) {
        self.body_scroll_lines = self
            .body_styled_lines_without_inline_widgets()
            .len()
            .saturating_sub(1) as f32;
    }

    pub(crate) fn scroll_body_to_bottom(&mut self) {
        self.body_scroll_lines = 0.0;
    }

    pub(crate) fn copy_latest_code_block(&mut self) -> KeyOutcome {
        if let Some(text) = self
            .latest_rich_code_block_text()
            .filter(|text| !text.trim().is_empty())
        {
            return KeyOutcome::CopyText {
                text,
                success_notice: "copied latest code block",
            };
        }
        self.set_status(SingleSessionStatus::Info(
            "no code block to copy".to_string(),
        ));
        KeyOutcome::Redraw
    }

    pub(crate) fn copy_transcript(&mut self) -> KeyOutcome {
        if let Some(text) = self
            .copy_rich_transcript_text(desktop_rich_text::TranscriptCopyMode::TranscriptPlainText)
            .filter(|text| !text.trim().is_empty())
        {
            return KeyOutcome::CopyText {
                text,
                success_notice: "copied transcript",
            };
        }
        self.set_status(SingleSessionStatus::Info(
            "no transcript to copy".to_string(),
        ));
        KeyOutcome::Redraw
    }

    pub(crate) fn latest_assistant_response(&self) -> Option<String> {
        if !self.streaming_response.trim().is_empty() {
            return Some(self.streaming_response.trim().to_string());
        }
        self.messages
            .iter()
            .rev()
            .find(|message| message.role() == SingleSessionRole::Assistant)
            .map(|message| message.content().trim().to_string())
            .filter(|message| !message.is_empty())
    }

    pub(crate) fn rich_transcript_document(&self) -> desktop_rich_text::RichTranscriptDocument {
        desktop_rich_text::build_rich_transcript(
            &self.rich_transcript_messages(true),
            &desktop_rich_text::RichTranscriptBuildOptions::default(),
        )
    }

    pub(crate) fn search_rich_transcript(
        &self,
        query: &str,
    ) -> Vec<desktop_rich_text::TranscriptSearchMatch> {
        let document = self.rich_transcript_document();
        desktop_rich_text::search_transcript(&document, query, false)
    }

    pub(crate) fn copy_rich_transcript_text(
        &self,
        mode: desktop_rich_text::TranscriptCopyMode,
    ) -> Option<String> {
        let document = self.rich_transcript_document();
        desktop_rich_text::copy_transcript_text(&document, mode)
    }

    pub(crate) fn latest_rich_code_block_text(&self) -> Option<String> {
        let document = self.rich_transcript_document();
        document.blocks.iter().rev().find_map(|block| {
            matches!(
                block.kind,
                desktop_rich_text::TranscriptBlockKind::CodeBlock { .. }
            )
            .then(|| block.copy_text.clone())
        })
    }

    #[allow(dead_code)]
    pub(crate) fn rich_transcript_jump_targets(
        &self,
    ) -> Vec<desktop_rich_text::TranscriptJumpTarget> {
        self.rich_transcript_document().jumps
    }

    pub(crate) fn rich_transcript_messages(
        &self,
        include_streaming_response: bool,
    ) -> Vec<desktop_rich_text::RichTranscriptMessage> {
        let mut messages = self
            .messages
            .iter()
            .enumerate()
            .map(|(index, message)| {
                let mut rich = desktop_rich_text::RichTranscriptMessage::new(
                    format!("message-{index}"),
                    rich_role_from_single_session_role(message.role()),
                    message.content().to_string(),
                );
                rich.attachments = message.rich_attachments().to_vec();
                rich
            })
            .collect::<Vec<_>>();

        if include_streaming_response && !self.streaming_response.trim().is_empty() {
            messages.push(desktop_rich_text::RichTranscriptMessage::new(
                "streaming-assistant",
                desktop_rich_text::TranscriptRole::Assistant,
                self.streaming_response.trim().to_string(),
            ));
        }
        if let Some(error) = &self.error {
            messages.push(desktop_rich_text::RichTranscriptMessage::new(
                "desktop-error",
                desktop_rich_text::TranscriptRole::System,
                format!("error: {error}"),
            ));
        }
        messages
    }

    pub(crate) fn jump_prompt(&mut self, direction: i32) {
        let lines = self.body_lines();
        let prompt_indices = lines
            .iter()
            .enumerate()
            .filter_map(|(index, line)| is_user_prompt_line(line).then_some(index))
            .collect::<Vec<_>>();
        if prompt_indices.is_empty() {
            return;
        }
        let current_line = lines
            .len()
            .saturating_sub(self.body_scroll_lines.floor().max(0.0) as usize)
            .saturating_sub(1);
        let target = if direction < 0 {
            prompt_indices
                .iter()
                .rev()
                .copied()
                .find(|index| *index < current_line)
                .or_else(|| prompt_indices.first().copied())
        } else {
            let next = prompt_indices
                .iter()
                .copied()
                .find(|index| *index > current_line);
            if next.is_none() {
                self.scroll_body_to_bottom();
                return;
            }
            next
        };
        if let Some(target) = target {
            self.body_scroll_lines = lines.len().saturating_sub(target + 1) as f32;
        }
    }

    pub(crate) fn record_user_submit(&mut self, message: &str, images: &[(String, String)]) {
        let attachments = images
            .iter()
            .enumerate()
            .map(|(index, (media_type, base64_data))| {
                desktop_rich_text::RichAttachment::image(
                    format!("user-{}-image-{index}", self.messages.len() + 1),
                    media_type.clone(),
                    format!("attached image {}", index + 1),
                    base64_data.len(),
                )
            })
            .collect::<Vec<_>>();
        self.messages
            .push(SingleSessionMessage::user(message).with_rich_attachments(attachments));
        self.draft.clear();
        self.draft_cursor = 0;
        self.composer.input_undo_stack.clear();
        self.streaming_response.clear();
        self.scroll_body_to_bottom();
        self.set_status(SingleSessionStatus::Sending);
        self.error = None;
        self.is_processing = true;
    }

    /// Warn once per session when thinking display is off.
    ///
    /// The desktop asks the server it spawns for `current` mode, but it often
    /// attaches to an already-running shared server whose config wins. Without
    /// this the transcript just silently shows nothing until the final
    /// response, which is the exact failure this feature exists to fix.
    fn notice_if_thinking_display_is_off(&mut self) {
        if self.runtime.thinking_display_notice_shown {
            return;
        }
        self.runtime.thinking_display_notice_shown = true;
        self.push_thinking_display_notice(&crate::desktop_config::desktop_reasoning_display_mode());
    }

    /// Split from the config lookup so the message itself is testable without
    /// touching process-global state.
    pub(crate) fn push_thinking_display_notice(&mut self, mode: &str) {
        if mode != "off" {
            return;
        }
        self.messages.push(SingleSessionMessage::meta(
            "thinking display is off, so this session will show nothing until the response \
             lands. Set display.reasoning_display = \"current\" in ~/.jcode/config.toml \
             (or run /thinking-display current in the terminal client) to watch work as it \
             happens."
                .to_string(),
        ));
    }

    /// Close the live reasoning region, leaving a collapsed trace in the
    /// transcript so the user can still see that the model thought here.
    pub(crate) fn finish_reasoning(&mut self, duration_ms: Option<u64>) {
        if self.reasoning.live.trim().is_empty() {
            self.reasoning.clear();
            return;
        }
        let trace = match duration_ms {
            Some(millis) if millis >= 50 => {
                format!("thought for {:.1}s", millis as f64 / 1000.0)
            }
            _ => "thought".to_string(),
        };
        self.messages.push(SingleSessionMessage::meta(trace));
        self.reasoning.clear();
    }

    pub(crate) fn finish_streaming_response(&mut self) {
        let response = self.streaming_response.trim().to_string();
        if !response.is_empty() {
            self.messages
                .push(SingleSessionMessage::assistant(response));
        }
        self.streaming_response.clear();
    }

    pub(crate) fn next_tool_event_sequence(&mut self) -> u64 {
        self.tool.event_sequence = self.tool.event_sequence.saturating_add(1);
        self.tool.event_sequence
    }

    pub(crate) fn start_tool_run(
        &mut self,
        id: Option<String>,
        name: &str,
        message_index: usize,
    ) -> String {
        let sequence = self.next_tool_event_sequence();
        let call_id =
            normalized_tool_call_id(id).unwrap_or_else(|| format!("desktop-tool-{sequence}"));
        if let Some(run) = self.tool.runs.iter_mut().find(|run| run.call_id == call_id) {
            run.message_index = message_index;
            run.name = name.to_string();
            run.state = SingleSessionToolVisualState::Preparing;
            run.summary = None;
            run.input_raw.clear();
            run.input_preview = None;
            run.stdin_prompt = None;
            run.updated_sequence = sequence;
            run.completed_sequence = None;
        } else {
            self.tool.runs.push(SingleSessionToolRun {
                call_id: call_id.clone(),
                message_index,
                name: name.to_string(),
                state: SingleSessionToolVisualState::Preparing,
                summary: None,
                input_raw: String::new(),
                input_preview: None,
                stdin_prompt: None,
                started_sequence: sequence,
                updated_sequence: sequence,
                completed_sequence: None,
            });
        }
        self.tool.active_call_id = Some(call_id.clone());
        call_id
    }

    pub(crate) fn update_tool_run_state(
        &mut self,
        id: Option<String>,
        name: &str,
        state: SingleSessionToolVisualState,
        summary: Option<String>,
    ) -> String {
        let sequence = self.next_tool_event_sequence();
        let call_id = normalized_tool_call_id(id)
            .or_else(|| self.tool.active_call_id.clone())
            .or_else(|| {
                self.tool
                    .runs
                    .iter()
                    .rev()
                    .find(|run| run.name == name && run.state.is_active())
                    .map(|run| run.call_id.clone())
            })
            .unwrap_or_else(|| format!("desktop-tool-{sequence}"));
        let message_index = self
            .tool
            .active_message_index
            .unwrap_or_else(|| self.messages.len().saturating_sub(1));
        let summary = summary.filter(|summary| !summary.trim().is_empty());
        if let Some(run) = self.tool.runs.iter_mut().find(|run| run.call_id == call_id) {
            run.message_index = message_index;
            run.name = name.to_string();
            run.state = state;
            run.summary = summary.clone();
            run.updated_sequence = sequence;
            run.completed_sequence = matches!(
                state,
                SingleSessionToolVisualState::Succeeded | SingleSessionToolVisualState::Failed
            )
            .then_some(sequence);
        } else {
            self.tool.runs.push(SingleSessionToolRun {
                call_id: call_id.clone(),
                message_index,
                name: name.to_string(),
                state,
                summary,
                input_raw: String::new(),
                input_preview: None,
                stdin_prompt: None,
                started_sequence: sequence,
                updated_sequence: sequence,
                completed_sequence: matches!(
                    state,
                    SingleSessionToolVisualState::Succeeded | SingleSessionToolVisualState::Failed
                )
                .then_some(sequence),
            });
        }
        self.tool.active_call_id = Some(call_id.clone());
        call_id
    }

    pub(crate) fn append_tool_run_input(&mut self, id: Option<String>, delta: &str) {
        if delta.is_empty() {
            return;
        }
        let sequence = self.next_tool_event_sequence();
        let call_id = normalized_tool_call_id(id)
            .or_else(|| self.tool.active_call_id.clone())
            .or_else(|| self.tool.runs.last().map(|run| run.call_id.clone()));
        let Some(call_id) = call_id else {
            return;
        };
        if let Some(run) = self.tool.runs.iter_mut().find(|run| run.call_id == call_id) {
            run.input_raw.push_str(delta);
            run.input_preview =
                compact_tool_metadata(&formatted_tool_input_lines(&run.name, &run.input_raw));
            run.updated_sequence = sequence;
        }
    }

    pub(crate) fn mark_tool_stdin_prompt(&mut self, tool_call_id: &str, prompt: &str) {
        let sequence = self.next_tool_event_sequence();
        if let Some(run) = self
            .tool
            .runs
            .iter_mut()
            .rev()
            .find(|run| run.call_id == tool_call_id)
        {
            run.stdin_prompt = Some(prompt.to_string());
            run.updated_sequence = sequence;
            return;
        }
        if let Some(run) = self
            .tool
            .runs
            .iter_mut()
            .rev()
            .find(|run| run.state.is_active())
        {
            run.stdin_prompt = Some(prompt.to_string());
            run.updated_sequence = sequence;
        }
    }

    pub(crate) fn clear_tool_stdin_prompts(&mut self) {
        let sequence = self.next_tool_event_sequence();
        for run in &mut self.tool.runs {
            if run.stdin_prompt.is_some() {
                run.stdin_prompt = None;
                run.updated_sequence = sequence;
            }
        }
    }

    pub(crate) fn tool_run_for_message_index(
        &self,
        message_index: usize,
    ) -> Option<&SingleSessionToolRun> {
        self.tool
            .runs
            .iter()
            .find(|run| run.message_index == message_index)
    }

    pub(crate) fn collapse_active_tool_message(&mut self) {
        let Some(index) = self.tool.active_message_index.take() else {
            return;
        };
        self.tool.active_call_id = None;
        let Some(message) = self.messages.get_mut(index) else {
            return;
        };
        if message.role() != SingleSessionRole::Tool {
            return;
        }
        if let Some(first_line) = message.content().lines().next() {
            message.set_content(first_line.replacen('▾', "▸", 1));
        }
    }

    pub(crate) fn append_active_tool_input(&mut self, delta: &str) {
        if delta.is_empty() {
            return;
        }
        self.tool.input_buffer.push_str(delta);
    }

    pub(crate) fn flush_active_tool_input_to_message(&mut self) {
        if self.tool.input_buffer.is_empty() {
            return;
        }
        let Some(index) = self.tool.active_message_index else {
            return;
        };
        let Some(message) = self.messages.get_mut(index) else {
            return;
        };
        if message.role() != SingleSessionRole::Tool {
            return;
        }
        if !message.content().contains("\n  input: ") {
            message.content_mut().push_str("\n  input: ");
        }
        message.content_mut().push_str(&self.tool.input_buffer);
        self.tool.input_buffer.clear();
    }

    pub(crate) fn replace_active_tool_header(&mut self, header: &str) {
        let Some(index) = self.tool.active_message_index else {
            self.messages
                .push(SingleSessionMessage::tool(header.to_string()));
            self.tool.active_message_index = Some(self.messages.len().saturating_sub(1));
            return;
        };
        let Some(message) = self.messages.get_mut(index) else {
            self.messages
                .push(SingleSessionMessage::tool(header.to_string()));
            self.tool.active_message_index = Some(self.messages.len().saturating_sub(1));
            return;
        };
        if message.role() == SingleSessionRole::Tool {
            let replacement = merge_tool_finish_with_existing_context(message.content(), header);
            if message.content() != replacement {
                message.set_content(replacement);
            }
        }
    }
}
