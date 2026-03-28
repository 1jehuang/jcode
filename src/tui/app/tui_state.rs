use super::*;
use std::cell::RefCell;
use std::sync::Mutex;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WidgetProviderKind {
    Anthropic,
    OpenAI,
    OpenRouter,
    Copilot,
    Gemini,
    Unknown,
}

impl WidgetProviderKind {
    fn from_provider_key(raw: Option<&str>) -> Self {
        match raw.map(|s| s.trim().to_ascii_lowercase()) {
            Some(provider) if provider == "openrouter" => Self::OpenRouter,
            Some(provider) if provider == "copilot" => Self::Copilot,
            Some(provider) if provider == "gemini" => Self::Gemini,
            Some(provider) if provider == "openai" => Self::OpenAI,
            Some(provider) if provider == "claude" => Self::Anthropic,
            _ => Self::Unknown,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct WidgetRouteInfo {
    provider: WidgetProviderKind,
    is_remote: bool,
}

impl App {
    fn effective_remote_provider_model(&self) -> Option<String> {
        self.remote_provider_model
            .clone()
            .or_else(|| self.session.model.clone())
            .filter(|model| {
                let trimmed = model.trim();
                !trimmed.is_empty() && !trimmed.eq_ignore_ascii_case("unknown")
            })
    }

    fn remote_header_provider_model(&self) -> Option<String> {
        self.remote_startup_phase
            .as_ref()
            .map(|phase| {
                let elapsed = self
                    .remote_startup_phase_started
                    .map(|started| started.elapsed())
                    .unwrap_or_default();
                phase.header_label_with_elapsed(elapsed)
            })
            .or_else(|| self.effective_remote_provider_model())
            .or_else(|| {
                (self.remote_session_id.is_some() || self.connection_type.is_some())
                    .then(|| "connected".to_string())
            })
    }

    fn remote_header_provider_name(&self) -> Option<String> {
        self.remote_provider_name
            .clone()
            .or_else(|| {
                self.effective_remote_provider_model().and_then(|model| {
                    crate::provider::provider_for_model_with_hint(&model, None).map(str::to_string)
                })
            })
            .filter(|provider| !provider.trim().is_empty())
    }

    fn widget_route_info(&self, model: Option<&str>) -> WidgetRouteInfo {
        let remote_provider_name = if self.is_remote || self.is_replay {
            self.remote_header_provider_name()
        } else {
            None
        };
        let provider_name = if self.is_remote || self.is_replay {
            remote_provider_name.as_deref()
        } else {
            Some(self.provider.name())
        };

        let provider = WidgetProviderKind::from_provider_key(
            model
                .map(|model| crate::provider::resolve_model_capabilities(model, provider_name))
                .and_then(|caps| caps.provider)
                .as_deref()
                .or(provider_name),
        );

        WidgetRouteInfo {
            provider,
            is_remote: self.is_remote || self.is_replay,
        }
    }

    fn widget_auth_method(&self, route: WidgetRouteInfo) -> crate::tui::info_widget::AuthMethod {
        if route.is_remote {
            return crate::tui::info_widget::AuthMethod::Unknown;
        }

        let auth_status = crate::auth::AuthStatus::check_fast();

        match route.provider {
            WidgetProviderKind::Anthropic => {
                if auth_status.anthropic.has_oauth {
                    crate::tui::info_widget::AuthMethod::AnthropicOAuth
                } else if auth_status.anthropic.has_api_key {
                    crate::tui::info_widget::AuthMethod::AnthropicApiKey
                } else {
                    crate::tui::info_widget::AuthMethod::Unknown
                }
            }
            WidgetProviderKind::OpenAI => {
                if auth_status.openai_has_oauth {
                    crate::tui::info_widget::AuthMethod::OpenAIOAuth
                } else if auth_status.openai_has_api_key {
                    crate::tui::info_widget::AuthMethod::OpenAIApiKey
                } else {
                    crate::tui::info_widget::AuthMethod::Unknown
                }
            }
            WidgetProviderKind::OpenRouter => crate::tui::info_widget::AuthMethod::OpenRouterApiKey,
            WidgetProviderKind::Copilot => crate::tui::info_widget::AuthMethod::CopilotOAuth,
            WidgetProviderKind::Gemini => {
                if auth_status.gemini == crate::auth::AuthState::Available {
                    crate::tui::info_widget::AuthMethod::GeminiOAuth
                } else {
                    crate::tui::info_widget::AuthMethod::Unknown
                }
            }
            WidgetProviderKind::Unknown => crate::tui::info_widget::AuthMethod::Unknown,
        }
    }

    fn widget_usage_info(
        &self,
        route: WidgetRouteInfo,
    ) -> Option<crate::tui::info_widget::UsageInfo> {
        let output_tps = if self.is_processing {
            self.compute_streaming_tps()
        } else {
            None
        };

        match route.provider {
            WidgetProviderKind::Copilot => Some(crate::tui::info_widget::UsageInfo {
                provider: crate::tui::info_widget::UsageProvider::Copilot,
                five_hour: 0.0,
                five_hour_resets_at: None,
                seven_day: 0.0,
                seven_day_resets_at: None,
                spark: None,
                spark_resets_at: None,
                total_cost: 0.0,
                input_tokens: self.total_input_tokens,
                output_tokens: self.total_output_tokens,
                cache_read_tokens: None,
                cache_write_tokens: None,
                output_tps,
                available: self.total_input_tokens > 0 || self.total_output_tokens > 0,
            }),
            WidgetProviderKind::Anthropic => {
                let usage = crate::usage::get_sync();
                Some(crate::tui::info_widget::UsageInfo {
                    provider: crate::tui::info_widget::UsageProvider::Anthropic,
                    five_hour: usage.five_hour,
                    five_hour_resets_at: usage.five_hour_resets_at.clone(),
                    seven_day: usage.seven_day,
                    seven_day_resets_at: usage.seven_day_resets_at.clone(),
                    spark: None,
                    spark_resets_at: None,
                    total_cost: 0.0,
                    input_tokens: 0,
                    output_tokens: 0,
                    cache_read_tokens: None,
                    cache_write_tokens: None,
                    output_tps,
                    available: true,
                })
            }
            WidgetProviderKind::OpenAI => {
                let openai_usage = crate::usage::get_openai_usage_sync();
                Some(crate::tui::info_widget::UsageInfo {
                    provider: crate::tui::info_widget::UsageProvider::OpenAI,
                    five_hour: openai_usage
                        .five_hour
                        .as_ref()
                        .map(|w| w.usage_ratio)
                        .unwrap_or(0.0),
                    five_hour_resets_at: openai_usage
                        .five_hour
                        .as_ref()
                        .and_then(|w| w.resets_at.clone()),
                    seven_day: openai_usage
                        .seven_day
                        .as_ref()
                        .map(|w| w.usage_ratio)
                        .unwrap_or(0.0),
                    seven_day_resets_at: openai_usage
                        .seven_day
                        .as_ref()
                        .and_then(|w| w.resets_at.clone()),
                    spark: openai_usage.spark.as_ref().map(|w| w.usage_ratio),
                    spark_resets_at: openai_usage
                        .spark
                        .as_ref()
                        .and_then(|w| w.resets_at.clone()),
                    total_cost: 0.0,
                    input_tokens: 0,
                    output_tokens: 0,
                    cache_read_tokens: None,
                    cache_write_tokens: None,
                    output_tps,
                    available: openai_usage.has_limits(),
                })
            }
            WidgetProviderKind::Gemini => None,
            WidgetProviderKind::OpenRouter => Some(crate::tui::info_widget::UsageInfo {
                provider: crate::tui::info_widget::UsageProvider::CostBased,
                five_hour: 0.0,
                five_hour_resets_at: None,
                seven_day: 0.0,
                seven_day_resets_at: None,
                spark: None,
                spark_resets_at: None,
                total_cost: self.total_cost,
                input_tokens: self.total_input_tokens,
                output_tokens: self.total_output_tokens,
                cache_read_tokens: self.streaming_cache_read_tokens,
                cache_write_tokens: self.streaming_cache_creation_tokens,
                output_tps,
                available: true,
            }),
            WidgetProviderKind::Unknown => None,
        }
    }
}

impl crate::tui::TuiState for App {
    fn display_messages(&self) -> &[DisplayMessage] {
        &self.display_messages
    }

    fn side_pane_images(&self) -> Vec<crate::session::RenderedImage> {
        if self.is_remote {
            self.remote_side_pane_images.clone()
        } else {
            crate::session::render_images(&self.session)
        }
    }

    fn display_messages_version(&self) -> u64 {
        self.display_messages_version
    }

    fn streaming_text(&self) -> &str {
        &self.streaming_text
    }

    fn input(&self) -> &str {
        &self.input
    }

    fn cursor_pos(&self) -> usize {
        self.cursor_pos
    }

    fn is_processing(&self) -> bool {
        self.is_processing || self.split_launch_in_flight()
    }

    fn queued_messages(&self) -> &[String] {
        &self.queued_messages
    }

    fn interleave_message(&self) -> Option<&str> {
        self.interleave_message.as_deref()
    }

    fn pending_soft_interrupts(&self) -> &[String] {
        &self.pending_soft_interrupts
    }

    fn scroll_offset(&self) -> usize {
        self.scroll_offset
    }

    fn auto_scroll_paused(&self) -> bool {
        self.auto_scroll_paused
    }

    fn provider_name(&self) -> String {
        if self.is_remote {
            self.remote_header_provider_name()
                .unwrap_or_else(|| self.provider.name().to_string())
        } else {
            self.remote_provider_name
                .clone()
                .unwrap_or_else(|| self.provider.name().to_string())
        }
    }

    fn provider_model(&self) -> String {
        if self.is_remote {
            self.remote_header_provider_model()
                .unwrap_or_else(|| "connecting to server…".to_string())
        } else {
            self.remote_provider_model
                .clone()
                .unwrap_or_else(|| self.provider.model().to_string())
        }
    }

    fn upstream_provider(&self) -> Option<String> {
        self.upstream_provider.clone()
    }

    fn connection_type(&self) -> Option<String> {
        self.connection_type.clone()
    }

    fn mcp_servers(&self) -> Vec<(String, usize)> {
        self.mcp_server_names.clone()
    }

    fn available_skills(&self) -> Vec<String> {
        if self.is_remote && !self.remote_skills.is_empty() {
            self.remote_skills.clone()
        } else {
            self.skills.list().iter().map(|s| s.name.clone()).collect()
        }
    }

    fn streaming_tokens(&self) -> (u64, u64) {
        (self.streaming_input_tokens, self.streaming_output_tokens)
    }

    fn streaming_cache_tokens(&self) -> (Option<u64>, Option<u64>) {
        (
            self.streaming_cache_read_tokens,
            self.streaming_cache_creation_tokens,
        )
    }

    fn output_tps(&self) -> Option<f32> {
        if !self.is_processing {
            return None;
        }
        self.compute_streaming_tps()
    }

    fn streaming_tool_calls(&self) -> Vec<ToolCall> {
        self.streaming_tool_calls.clone()
    }

    fn update_cost(&mut self) {
        self.update_cost_impl()
    }

    fn elapsed(&self) -> Option<std::time::Duration> {
        if let Some(d) = self.replay_elapsed_override {
            return Some(d);
        }
        self.processing_started.map(|t| t.elapsed()).or_else(|| {
            self.split_launch_in_flight()
                .then(|| self.pending_split_started_at.map(|t| t.elapsed()))
                .flatten()
        })
    }

    fn status(&self) -> ProcessingStatus {
        if self.split_launch_in_flight() {
            ProcessingStatus::Sending
        } else {
            self.status.clone()
        }
    }

    fn command_suggestions(&self) -> Vec<(String, &'static str)> {
        App::command_suggestions(self)
    }

    fn active_skill(&self) -> Option<String> {
        self.active_skill.clone()
    }

    fn subagent_status(&self) -> Option<String> {
        self.subagent_status.clone()
    }

    fn batch_progress(&self) -> Option<crate::bus::BatchProgress> {
        self.batch_progress.clone()
    }

    fn time_since_activity(&self) -> Option<std::time::Duration> {
        self.last_stream_activity.map(|t| t.elapsed())
    }

    fn total_session_tokens(&self) -> Option<(u64, u64)> {
        // In remote mode, use tokens from server
        // Standalone mode doesn't currently track total tokens
        self.remote_total_tokens
    }

    fn is_remote_mode(&self) -> bool {
        self.is_remote
    }

    fn is_canary(&self) -> bool {
        if self.is_remote {
            self.remote_is_canary.unwrap_or(self.session.is_canary)
        } else {
            self.session.is_canary
        }
    }

    fn is_replay(&self) -> bool {
        self.is_replay
    }

    fn diff_mode(&self) -> crate::config::DiffDisplayMode {
        self.diff_mode
    }

    fn current_session_id(&self) -> Option<String> {
        if self.is_remote {
            self.remote_session_id.clone()
        } else {
            Some(self.session.id.clone())
        }
    }

    fn session_display_name(&self) -> Option<String> {
        if self.is_remote {
            self.remote_session_id
                .as_ref()
                .or(self.resume_session_id.as_ref())
                .as_ref()
                .and_then(|id| crate::id::extract_session_name(id))
                .map(|s| s.to_string())
        } else {
            Some(self.session.display_name().to_string())
        }
    }

    fn server_display_name(&self) -> Option<String> {
        self.remote_server_short_name.clone()
    }

    fn server_display_icon(&self) -> Option<String> {
        self.remote_server_icon.clone()
    }

    fn server_sessions(&self) -> Vec<String> {
        self.remote_sessions.clone()
    }

    fn connected_clients(&self) -> Option<usize> {
        self.remote_client_count
    }

    fn status_notice(&self) -> Option<String> {
        self.status_notice.as_ref().and_then(|(text, at)| {
            if at.elapsed() <= Duration::from_secs(3) {
                Some(text.clone())
            } else {
                None
            }
        })
    }

    fn remote_startup_phase_active(&self) -> bool {
        self.remote_startup_phase.is_some()
    }

    fn dictation_key_label(&self) -> Option<String> {
        self.dictation_key_label().map(|s| s.to_string())
    }

    fn animation_elapsed(&self) -> f32 {
        self.app_started.elapsed().as_secs_f32()
    }

    fn rate_limit_remaining(&self) -> Option<Duration> {
        self.rate_limit_reset.and_then(|reset_time| {
            let now = Instant::now();
            if reset_time > now {
                Some(reset_time - now)
            } else {
                None
            }
        })
    }

    fn queue_mode(&self) -> bool {
        self.queue_mode
    }

    fn has_stashed_input(&self) -> bool {
        self.stashed_input.is_some()
    }

    fn context_info(&self) -> crate::prompt::ContextInfo {
        use crate::message::{ContentBlock, Role};
        use std::time::Instant;

        static CACHE: Mutex<Option<(Instant, CachedContextInfo)>> = Mutex::new(None);
        const TTL: Duration = Duration::from_millis(250);

        let session_key = if self.is_remote {
            self.remote_session_id
                .clone()
                .unwrap_or_else(|| self.session.id.clone())
        } else {
            self.session.id.clone()
        };
        let message_count = if self.is_remote {
            self.display_messages.len()
        } else {
            self.messages.len()
        };
        if let Ok(cache) = CACHE.lock()
            && let Some((ts, cached)) = &*cache
            && ts.elapsed() < TTL
            && cached.session_key == session_key
            && cached.is_remote == self.is_remote
            && cached.display_messages_version == self.display_messages_version
            && cached.message_count == message_count
        {
            return cached.context_info.clone();
        }

        let mut info = self.context_info.clone();

        // Compute dynamic stats from conversation
        let mut user_chars = 0usize;
        let mut user_count = 0usize;
        let mut asst_chars = 0usize;
        let mut asst_count = 0usize;
        let mut tool_call_chars = 0usize;
        let mut tool_call_count = 0usize;
        let mut tool_result_chars = 0usize;
        let mut tool_result_count = 0usize;

        if self.is_remote {
            for msg in &self.display_messages {
                match msg.role.as_str() {
                    "user" => {
                        user_count += 1;
                        user_chars += msg.content.len();
                    }
                    "assistant" => {
                        asst_count += 1;
                        asst_chars += msg.content.len();
                    }
                    "tool" => {
                        tool_result_count += 1;
                        tool_result_chars += msg.content.len();
                        if let Some(tool) = &msg.tool_data {
                            tool_call_count += 1;
                            tool_call_chars += tool.name.len() + tool.input.to_string().len();
                        }
                    }
                    _ => {}
                }
            }
        } else {
            let skip = if self.provider.uses_jcode_compaction() {
                let compaction = self.registry.compaction();
                let result = compaction
                    .try_read()
                    .ok()
                    .map(|manager| (manager.compacted_count(), manager.summary_chars()));
                if let Some((cc, sc)) = result {
                    if cc > 0 && sc > 0 {
                        user_count += 1;
                        user_chars += sc;
                    }
                    cc
                } else {
                    0
                }
            } else {
                0
            };

            for msg in self.messages.iter().skip(skip) {
                match msg.role {
                    Role::User => user_count += 1,
                    Role::Assistant => asst_count += 1,
                }

                for block in &msg.content {
                    match block {
                        ContentBlock::Text { text, .. } => match msg.role {
                            Role::User => user_chars += text.len(),
                            Role::Assistant => asst_chars += text.len(),
                        },
                        ContentBlock::ToolUse { name, input, .. } => {
                            tool_call_count += 1;
                            tool_call_chars += name.len() + input.to_string().len();
                        }
                        ContentBlock::ToolResult { content, .. } => {
                            tool_result_count += 1;
                            tool_result_chars += content.len();
                        }
                        ContentBlock::Reasoning { text } => {
                            asst_chars += text.len();
                        }
                        ContentBlock::Image { data, .. } => {
                            user_chars += data.len();
                        }
                        ContentBlock::OpenAICompaction { encrypted_content } => {
                            user_chars += encrypted_content.len();
                        }
                    }
                }
            }
        }

        // Estimate tool definitions size
        // jcode has ~25 built-in tools, each ~500 chars in definition
        // This is a rough estimate since we can't easily call async from here
        let tool_defs_count = 25;
        let tool_defs_chars = tool_defs_count * 500;

        info.user_messages_chars = user_chars;
        info.user_messages_count = user_count;
        info.assistant_messages_chars = asst_chars;
        info.assistant_messages_count = asst_count;
        info.tool_calls_chars = tool_call_chars;
        info.tool_calls_count = tool_call_count;
        info.tool_results_chars = tool_result_chars;
        info.tool_results_count = tool_result_count;
        info.tool_defs_chars = tool_defs_chars;
        info.tool_defs_count = tool_defs_count;

        // Update total
        info.total_chars = info.system_prompt_chars
            + info.env_context_chars
            + info.project_agents_md_chars
            + info.project_claude_md_chars
            + info.global_agents_md_chars
            + info.global_claude_md_chars
            + info.skills_chars
            + info.selfdev_chars
            + info.memory_chars
            + info.tool_defs_chars
            + info.user_messages_chars
            + info.assistant_messages_chars
            + info.tool_calls_chars
            + info.tool_results_chars;

        if let Ok(mut cache) = CACHE.lock() {
            *cache = Some((
                Instant::now(),
                CachedContextInfo {
                    session_key,
                    is_remote: self.is_remote,
                    display_messages_version: self.display_messages_version,
                    message_count,
                    context_info: info.clone(),
                },
            ));
        }

        info
    }

    fn context_limit(&self) -> Option<usize> {
        Some(self.context_limit as usize)
    }

    fn client_update_available(&self) -> bool {
        self.has_newer_binary()
    }

    fn server_update_available(&self) -> Option<bool> {
        if self.is_remote {
            self.remote_server_has_update
        } else {
            None
        }
    }

    fn info_widget_data(&self) -> crate::tui::info_widget::InfoWidgetData {
        let session_id = if self.is_remote {
            self.remote_session_id.as_deref()
        } else {
            Some(self.session.id.as_str())
        };

        let todos = if self.swarm_enabled && !self.swarm_plan_items.is_empty() {
            self.swarm_plan_items
                .iter()
                .map(|item| crate::todo::TodoItem {
                    content: item.content.clone(),
                    status: item.status.clone(),
                    priority: item.priority.clone(),
                    id: item.id.clone(),
                    blocked_by: item.blocked_by.clone(),
                    assigned_to: item.assigned_to.clone(),
                })
                .collect()
        } else {
            gather_todos_for_session(session_id)
        };

        let context_info = self.context_info();
        let context_info = if context_info.total_chars > 0 {
            Some(context_info)
        } else {
            None
        };

        let (
            model,
            reasoning_effort,
            service_tier,
            native_compaction_mode,
            native_compaction_threshold_tokens,
        ) = if self.is_remote || self.is_replay {
            (
                self.remote_provider_model.clone(),
                self.remote_reasoning_effort.clone(),
                self.remote_service_tier.clone(),
                None,
                None,
            )
        } else {
            (
                Some(self.provider.model()),
                self.provider.reasoning_effort(),
                self.provider.service_tier(),
                self.provider.native_compaction_mode(),
                self.provider.native_compaction_threshold_tokens(),
            )
        };

        let (session_count, client_count) = if self.is_remote {
            (Some(self.remote_sessions.len()), None)
        } else {
            (None, None)
        };
        let session_name = self.session_display_name().map(|name| {
            if let Some(ref srv) = self.remote_server_short_name {
                format!("{} {}", srv, name)
            } else {
                name
            }
        });

        let memory_info = gather_memory_info(self.memory_enabled);

        // Gather swarm info
        let swarm_info = if self.swarm_enabled {
            let subagent_status = self.subagent_status.clone();
            let mut members: Vec<crate::protocol::SwarmMemberStatus> = Vec::new();
            let (session_count, client_count, session_names, has_activity) = if self.is_remote {
                members = self.remote_swarm_members.clone();
                let session_names = if !members.is_empty() {
                    members
                        .iter()
                        .map(|m| {
                            m.friendly_name
                                .clone()
                                .unwrap_or_else(|| m.session_id.chars().take(8).collect())
                        })
                        .collect()
                } else {
                    self.remote_sessions.clone()
                };
                let session_count = if !members.is_empty() {
                    members.len()
                } else {
                    self.remote_sessions.len()
                };
                let has_activity = members
                    .iter()
                    .any(|m| m.status != "ready" || m.detail.is_some());
                (
                    session_count,
                    self.remote_client_count,
                    session_names,
                    has_activity,
                )
            } else {
                let (status, detail) = match &self.status {
                    ProcessingStatus::Idle => ("ready".to_string(), None),
                    ProcessingStatus::Sending => {
                        ("running".to_string(), Some("sending".to_string()))
                    }
                    ProcessingStatus::Connecting(phase) => {
                        ("running".to_string(), Some(phase.to_string()))
                    }
                    ProcessingStatus::Thinking(_) => ("thinking".to_string(), None),
                    ProcessingStatus::Streaming => {
                        ("running".to_string(), Some("streaming".to_string()))
                    }
                    ProcessingStatus::RunningTool(name) => {
                        ("running".to_string(), Some(format!("tool: {}", name)))
                    }
                };
                let detail = subagent_status.clone().or(detail);
                let has_activity = status != "ready" || detail.is_some();
                if has_activity {
                    members.push(crate::protocol::SwarmMemberStatus {
                        session_id: self.session.id.clone(),
                        friendly_name: Some(self.session.display_name().to_string()),
                        status,
                        detail,
                        role: None,
                    });
                }
                (
                    1,
                    None,
                    vec![self.session.display_name().to_string()],
                    has_activity,
                )
            };

            // Only show if there's something interesting
            if has_activity || session_count > 1 || client_count.is_some() {
                Some(crate::tui::info_widget::SwarmInfo {
                    session_count,
                    subagent_status,
                    client_count,
                    session_names,
                    members,
                })
            } else {
                None
            }
        } else {
            None
        };

        // Gather background task info
        let background_info = {
            let memory_agent_active = self.memory_enabled && crate::memory_agent::is_active();
            let memory_stats = crate::memory_agent::stats();

            // Get running background tasks count
            let bg_manager = crate::background::global();
            let (running_count, running_tasks) = bg_manager.running_snapshot();

            if memory_agent_active || running_count > 0 {
                Some(crate::tui::info_widget::BackgroundInfo {
                    running_count,
                    running_tasks,
                    memory_agent_active,
                    memory_agent_turns: memory_stats.turns_processed,
                })
            } else {
                None
            }
        };

        let route = self.widget_route_info(model.as_deref());
        let usage_info = self.widget_usage_info(route);

        let tokens_per_second = self.compute_streaming_tps();

        let auth_method = self.widget_auth_method(route);

        // Get active mermaid diagrams - only for margin mode (pinned mode uses dedicated pane)
        let diagrams = if self.diagram_mode == crate::config::DiagramDisplayMode::Margin {
            crate::tui::mermaid::get_active_diagrams()
        } else {
            Vec::new()
        };

        let workspace_rows = if crate::tui::workspace_client::is_enabled() {
            let session_id = if self.is_remote {
                self.remote_session_id.as_deref()
            } else {
                Some(self.session.id.as_str())
            };
            crate::tui::workspace_client::visible_rows(5, session_id, self.is_processing)
        } else {
            Vec::new()
        };

        let workspace_animation_tick = self.app_started.elapsed().as_millis() as u64 / 180;

        crate::tui::info_widget::InfoWidgetData {
            todos,
            context_info,
            queue_mode: Some(self.queue_mode),
            context_limit: Some(self.context_limit as usize),
            model,
            reasoning_effort,
            service_tier,
            native_compaction_mode,
            native_compaction_threshold_tokens,
            session_count,
            session_name,
            client_count,
            memory_info,
            swarm_info,
            background_info,
            usage_info,
            tokens_per_second,
            provider_name: if self.is_remote || self.is_replay {
                self.remote_provider_name
                    .clone()
                    .or_else(|| Some(self.provider.name().to_string()))
            } else {
                Some(self.provider.name().to_string())
            },
            auth_method,
            upstream_provider: self.upstream_provider.clone(),
            connection_type: self.connection_type.clone(),
            diagrams,
            workspace_rows,
            workspace_animation_tick,
            ambient_info: gather_ambient_info(crate::config::config().ambient.enabled),
            observed_context_tokens: self.current_stream_context_tokens(),
            is_compacting: if !self.is_remote && self.provider.uses_jcode_compaction() {
                let compaction = self.registry.compaction();
                compaction
                    .try_read()
                    .map(|m| m.is_compacting())
                    .unwrap_or(false)
            } else {
                false
            },
            git_info: gather_git_info(),
        }
    }

    fn workspace_mode_enabled(&self) -> bool {
        crate::tui::workspace_client::is_enabled()
    }

    fn workspace_map_rows(&self) -> Vec<crate::tui::workspace_map::VisibleWorkspaceRow> {
        let session_id = if self.is_remote {
            self.remote_session_id.as_deref()
        } else {
            Some(self.session.id.as_str())
        };
        crate::tui::workspace_client::visible_rows(5, session_id, self.is_processing)
    }

    fn workspace_animation_tick(&self) -> u64 {
        self.app_started.elapsed().as_millis() as u64 / 180
    }

    fn render_streaming_markdown(&self, width: usize) -> Vec<ratatui::text::Line<'static>> {
        let mut renderer = self.streaming_md_renderer.borrow_mut();
        renderer.set_width(Some(width));
        renderer.update(&self.streaming_text)
    }

    fn centered_mode(&self) -> bool {
        self.centered
    }

    fn auth_status(&self) -> crate::auth::AuthStatus {
        crate::auth::AuthStatus::check_fast()
    }

    fn diagram_mode(&self) -> crate::config::DiagramDisplayMode {
        self.diagram_mode
    }

    fn diagram_focus(&self) -> bool {
        self.diagram_focus
    }

    fn diagram_index(&self) -> usize {
        self.diagram_index
    }

    fn diagram_scroll(&self) -> (i32, i32) {
        (self.diagram_scroll_x, self.diagram_scroll_y)
    }

    fn diagram_pane_ratio(&self) -> u8 {
        self.animated_diagram_pane_ratio()
    }

    fn diagram_pane_animating(&self) -> bool {
        self.diagram_pane_anim_start
            .map(|s| s.elapsed().as_secs_f32() < Self::DIAGRAM_PANE_ANIM_DURATION)
            .unwrap_or(false)
    }

    fn diagram_pane_enabled(&self) -> bool {
        self.diagram_pane_enabled
    }

    fn diagram_pane_position(&self) -> crate::config::DiagramPanePosition {
        self.diagram_pane_position
    }

    fn diagram_zoom(&self) -> u8 {
        self.diagram_zoom
    }
    fn diff_pane_scroll(&self) -> usize {
        self.diff_pane_scroll
    }
    fn diff_pane_scroll_x(&self) -> i32 {
        self.diff_pane_scroll_x
    }
    fn diff_pane_focus(&self) -> bool {
        self.diff_pane_focus
    }
    fn side_panel(&self) -> &crate::side_panel::SidePanelSnapshot {
        &self.side_panel
    }
    fn pin_images(&self) -> bool {
        self.pin_images
    }
    fn chat_native_scrollbar(&self) -> bool {
        self.chat_native_scrollbar
    }
    fn side_panel_native_scrollbar(&self) -> bool {
        self.side_panel_native_scrollbar
    }
    fn diff_line_wrap(&self) -> bool {
        crate::config::config().display.diff_line_wrap
    }
    fn picker_state(&self) -> Option<&crate::tui::PickerState> {
        self.picker_state.as_ref()
    }

    fn changelog_scroll(&self) -> Option<usize> {
        self.changelog_scroll
    }

    fn help_scroll(&self) -> Option<usize> {
        self.help_scroll
    }

    fn session_picker_overlay(
        &self,
    ) -> Option<&RefCell<crate::tui::session_picker::SessionPicker>> {
        self.session_picker_overlay.as_ref()
    }

    fn login_picker_overlay(&self) -> Option<&RefCell<crate::tui::login_picker::LoginPicker>> {
        self.login_picker_overlay.as_ref()
    }

    fn account_picker_overlay(
        &self,
    ) -> Option<&RefCell<crate::tui::account_picker::AccountPicker>> {
        self.account_picker_overlay.as_ref()
    }

    fn working_dir(&self) -> Option<String> {
        self.session.working_dir.clone()
    }

    fn now_millis(&self) -> u64 {
        self.app_started.elapsed().as_millis() as u64
    }

    fn copy_badge_ui(&self) -> crate::tui::CopyBadgeUiState {
        self.copy_badge_ui.clone()
    }

    fn copy_selection_mode(&self) -> bool {
        self.copy_selection_mode
    }

    fn copy_selection_range(&self) -> Option<crate::tui::CopySelectionRange> {
        self.normalized_copy_selection()
    }

    fn copy_selection_status(&self) -> Option<crate::tui::CopySelectionStatus> {
        if !self.copy_selection_mode {
            return None;
        }

        let text = self.current_copy_selection_text().unwrap_or_default();
        let has_selection = !text.is_empty();
        Some(crate::tui::CopySelectionStatus {
            pane: self
                .current_copy_selection_pane()
                .unwrap_or(crate::tui::CopySelectionPane::Chat),
            has_selection,
            selected_chars: text.chars().count(),
            selected_lines: if has_selection {
                text.lines().count().max(1)
            } else {
                0
            },
            dragging: self.copy_selection_dragging,
        })
    }

    fn suggestion_prompts(&self) -> Vec<(String, String)> {
        App::suggestion_prompts(self)
    }

    fn cache_ttl_status(&self) -> Option<crate::tui::CacheTtlInfo> {
        let last_completed = self.last_api_completed?;
        let provider = self.provider_name();
        let ttl_secs = crate::tui::cache_ttl_for_provider(&provider)?;
        let elapsed = last_completed.elapsed().as_secs();
        let remaining = ttl_secs.saturating_sub(elapsed);
        Some(crate::tui::CacheTtlInfo {
            remaining_secs: remaining,
            ttl_secs,
            is_cold: remaining == 0,
            cached_tokens: self.last_turn_input_tokens,
        })
    }
}
