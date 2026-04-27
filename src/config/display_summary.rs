use super::*;

impl Config {
    pub fn display_string(&self) -> String {
        let path = Self::path()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "unknown".to_string());

        format!(
            r#"**Configuration** (`{}`)

**Keybindings:**
- Scroll up: `{}`
- Scroll down: `{}`
- Scroll up fallback: `{}`
- Scroll down fallback: `{}`
- Page up: `{}`
- Page down: `{}`
- Model next: `{}`
- Model prev: `{}`
- Effort increase: `{}`
- Effort decrease: `{}`
- Centered toggle: `{}`
- Prompt up: `{}`
- Prompt down: `{}`
- Scroll bookmark: `{}`
- Workspace left: `{}`
- Workspace down: `{}`
- Workspace up: `{}`
- Workspace right: `{}`

**Dictation:**
- Command: `{}`
- Mode: `{}`
- Hotkey: `{}`
- Timeout: {}s

**Display:**
- Diff mode: {}
- Centered: {}
- Markdown spacing: {}
- Pin images: {}
- Diff line wrap: {}
- Queue mode: {}
- Auto server reload: {}
- Mouse capture: {}
- Debug socket: {}
- Idle animation: {}
- Prompt entry animation: {}
- Chat native scrollbar: {}
- Side panel native scrollbar: {}
- Disabled animations: {}
- Performance tier: {}
- Animation FPS: {}
- Redraw FPS: {}

**Features:**
- Memory: {}
- Swarm: {}
- Message timestamps: {}
- Update channel: {}

**Provider:**
- Default model: {}
- Default provider: {}
- OpenAI reasoning effort: {}
- OpenAI transport: {}
- OpenAI service tier: {}
- OpenAI native compaction: {}
- OpenAI native compaction threshold ratio: {:.2}
- Cross-provider failover: {}

**Agent models:**
- Swarm / subagent: {}
- Review: {}
- Judge: {}
- Memory: {}
- Ambient: {}

**Gateway:**
- Enabled: {}
- Bind address: {}:{}

**Ambient:**
- Enabled: {}
- Provider: {}
- Model: {}
- Interval: {}-{} minutes
- Pause on active session: {}
- Proactive work: {}
- Work branch prefix: `{}`
- Visible mode: {}

**Notifications:**
- ntfy.sh: {}
- Desktop: {}
- Email: {}
- Email replies: {}
- Telegram: {}
- Telegram replies: {}
- Discord: {}
- Discord replies: {}

*Edit the config file or set environment variables to customize.*
*Environment variables (e.g., `JCODE_SCROLL_UP_KEY`, `JCODE_GATEWAY_ENABLED`) override file settings.*"#,
            path,
            self.keybindings.scroll_up,
            self.keybindings.scroll_down,
            self.keybindings.scroll_up_fallback,
            self.keybindings.scroll_down_fallback,
            self.keybindings.scroll_page_up,
            self.keybindings.scroll_page_down,
            self.keybindings.model_switch_next,
            self.keybindings.model_switch_prev,
            self.keybindings.effort_increase,
            self.keybindings.effort_decrease,
            self.keybindings.centered_toggle,
            self.keybindings.scroll_prompt_up,
            self.keybindings.scroll_prompt_down,
            self.keybindings.scroll_bookmark,
            self.keybindings.workspace_left,
            self.keybindings.workspace_down,
            self.keybindings.workspace_up,
            self.keybindings.workspace_right,
            if self.dictation.command.trim().is_empty() {
                "(disabled)"
            } else {
                self.dictation.command.as_str()
            },
            match self.dictation.mode {
                crate::protocol::TranscriptMode::Insert => "insert",
                crate::protocol::TranscriptMode::Append => "append",
                crate::protocol::TranscriptMode::Replace => "replace",
                crate::protocol::TranscriptMode::Send => "send",
            },
            self.dictation.key,
            self.dictation.timeout_secs,
            self.display.diff_mode.label(),
            self.display.centered,
            self.display.markdown_spacing.label(),
            self.display.pin_images,
            self.display.diff_line_wrap,
            self.display.queue_mode,
            self.display.auto_server_reload,
            self.display.mouse_capture,
            self.display.debug_socket,
            self.display.idle_animation,
            self.display.prompt_entry_animation,
            self.display.native_scrollbars.chat,
            self.display.native_scrollbars.side_panel,
            if self.display.disabled_animations.is_empty() {
                "(none)".to_string()
            } else {
                self.display.disabled_animations.join(", ")
            },
            if self.display.performance.is_empty() {
                "auto"
            } else {
                &self.display.performance
            },
            self.display.animation_fps,
            self.display.redraw_fps,
            self.features.memory,
            self.features.swarm,
            self.features.message_timestamps,
            self.features.update_channel,
            self.provider
                .default_model
                .as_deref()
                .unwrap_or("(provider default)"),
            self.provider
                .default_provider
                .as_deref()
                .unwrap_or("(auto)"),
            self.provider
                .openai_reasoning_effort
                .as_deref()
                .unwrap_or("(provider default)"),
            self.provider
                .openai_transport
                .as_deref()
                .unwrap_or("(auto)"),
            self.provider
                .openai_service_tier
                .as_deref()
                .unwrap_or("(default)"),
            self.provider.openai_native_compaction_mode.as_str(),
            self.provider.openai_native_compaction_threshold_tokens,
            self.provider.cross_provider_failover.as_str(),
            self.agents
                .swarm_model
                .as_deref()
                .unwrap_or("(inherit current session)"),
            self.autoreview
                .model
                .as_deref()
                .unwrap_or("(inherit current session)"),
            self.autojudge
                .model
                .as_deref()
                .unwrap_or("(inherit current session)"),
            self.agents
                .memory_model
                .as_deref()
                .unwrap_or("(sidecar auto-select)"),
            self.ambient
                .model
                .as_deref()
                .unwrap_or("(provider default)"),
            self.gateway.enabled,
            self.gateway.bind_addr,
            self.gateway.port,
            self.ambient.enabled,
            self.ambient.provider.as_deref().unwrap_or("(auto)"),
            self.ambient
                .model
                .as_deref()
                .unwrap_or("(provider default)"),
            self.ambient.min_interval_minutes,
            self.ambient.max_interval_minutes,
            self.ambient.pause_on_active_session,
            self.ambient.proactive_work,
            self.ambient.work_branch_prefix,
            self.ambient.visible,
            self.safety
                .ntfy_topic
                .as_deref()
                .map(|t| format!("enabled (topic: {})", t))
                .unwrap_or_else(|| "disabled".to_string()),
            if self.safety.desktop_notifications {
                "enabled"
            } else {
                "disabled"
            },
            if self.safety.email_enabled {
                self.safety
                    .email_to
                    .as_deref()
                    .unwrap_or("enabled (no recipient)")
            } else {
                "disabled"
            },
            if self.safety.email_reply_enabled {
                self.safety
                    .email_imap_host
                    .as_deref()
                    .unwrap_or("enabled (no IMAP host)")
            } else {
                "disabled"
            },
            if self.safety.telegram_enabled {
                self.safety
                    .telegram_chat_id
                    .as_deref()
                    .unwrap_or("enabled (no chat_id)")
            } else {
                "disabled"
            },
            if self.safety.telegram_reply_enabled {
                "enabled"
            } else {
                "disabled"
            },
            if self.safety.discord_enabled {
                self.safety
                    .discord_channel_id
                    .as_deref()
                    .unwrap_or("enabled (no channel_id)")
            } else {
                "disabled"
            },
            if self.safety.discord_reply_enabled {
                "enabled"
            } else {
                "disabled"
            },
        )
    }
}
