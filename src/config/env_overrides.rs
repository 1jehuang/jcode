use super::*;

impl Config {
    /// Apply environment variable overrides
    #[expect(
        clippy::collapsible_if,
        reason = "Environment override parsing is intentionally explicit and grouped by config area"
    )]
    pub(crate) fn apply_env_overrides(&mut self) {
        // Keybindings
        if let Ok(v) = std::env::var("JCODE_SCROLL_UP_KEY") {
            self.keybindings.scroll_up = v;
        }
        if let Ok(v) = std::env::var("JCODE_SCROLL_DOWN_KEY") {
            self.keybindings.scroll_down = v;
        }
        if let Ok(v) = std::env::var("JCODE_SCROLL_PAGE_UP_KEY") {
            self.keybindings.scroll_page_up = v;
        }
        if let Ok(v) = std::env::var("JCODE_SCROLL_PAGE_DOWN_KEY") {
            self.keybindings.scroll_page_down = v;
        }
        if let Ok(v) = std::env::var("JCODE_MODEL_SWITCH_KEY") {
            self.keybindings.model_switch_next = v;
        }
        if let Ok(v) = std::env::var("JCODE_MODEL_SWITCH_PREV_KEY") {
            self.keybindings.model_switch_prev = v;
        }
        if let Ok(v) = std::env::var("JCODE_EFFORT_INCREASE_KEY") {
            self.keybindings.effort_increase = v;
        }
        if let Ok(v) = std::env::var("JCODE_EFFORT_DECREASE_KEY") {
            self.keybindings.effort_decrease = v;
        }
        if let Ok(v) = std::env::var("JCODE_CENTERED_TOGGLE_KEY") {
            self.keybindings.centered_toggle = v;
        }
        if let Ok(v) = std::env::var("JCODE_SCROLL_PROMPT_UP_KEY") {
            self.keybindings.scroll_prompt_up = v;
        }
        if let Ok(v) = std::env::var("JCODE_SCROLL_PROMPT_DOWN_KEY") {
            self.keybindings.scroll_prompt_down = v;
        }
        if let Ok(v) = std::env::var("JCODE_SCROLL_BOOKMARK_KEY") {
            self.keybindings.scroll_bookmark = v;
        }
        if let Ok(v) = std::env::var("JCODE_SCROLL_UP_FALLBACK_KEY") {
            self.keybindings.scroll_up_fallback = v;
        }
        if let Ok(v) = std::env::var("JCODE_SCROLL_DOWN_FALLBACK_KEY") {
            self.keybindings.scroll_down_fallback = v;
        }
        if let Ok(v) = std::env::var("JCODE_WORKSPACE_LEFT_KEY") {
            self.keybindings.workspace_left = v;
        }
        if let Ok(v) = std::env::var("JCODE_WORKSPACE_DOWN_KEY") {
            self.keybindings.workspace_down = v;
        }
        if let Ok(v) = std::env::var("JCODE_WORKSPACE_UP_KEY") {
            self.keybindings.workspace_up = v;
        }
        if let Ok(v) = std::env::var("JCODE_WORKSPACE_RIGHT_KEY") {
            self.keybindings.workspace_right = v;
        }

        // Dictation
        if let Ok(v) = std::env::var("JCODE_DICTATION_COMMAND") {
            self.dictation.command = v;
        }
        if let Ok(v) = std::env::var("JCODE_DICTATION_MODE")
            && let Ok(mode) = toml::from_str::<crate::protocol::TranscriptMode>(&format!(
                "\"{}\"",
                v.trim().to_ascii_lowercase()
            ))
        {
            self.dictation.mode = mode;
        }
        if let Ok(v) = std::env::var("JCODE_DICTATION_KEY") {
            self.dictation.key = v;
        }
        if let Ok(v) = std::env::var("JCODE_DICTATION_TIMEOUT_SECS")
            && let Ok(parsed) = v.trim().parse::<u64>()
        {
            self.dictation.timeout_secs = parsed;
        }

        // Display
        if let Ok(v) = std::env::var("JCODE_DIFF_MODE") {
            match v.to_lowercase().as_str() {
                "off" | "none" | "0" | "false" => self.display.diff_mode = DiffDisplayMode::Off,
                "inline" | "on" | "1" | "true" => self.display.diff_mode = DiffDisplayMode::Inline,
                "full-inline" | "full_inline" | "fullinline" | "inline-full" | "inline_full"
                | "inlinefull" | "full" => {
                    self.display.diff_mode = DiffDisplayMode::FullInline;
                }
                "pinned" | "pin" => self.display.diff_mode = DiffDisplayMode::Pinned,
                "file" => self.display.diff_mode = DiffDisplayMode::File,
                _ => {}
            }
        } else if let Ok(v) = std::env::var("JCODE_SHOW_DIFFS")
            && let Some(parsed) = parse_env_bool(&v)
        {
            self.display.diff_mode = if parsed {
                DiffDisplayMode::Inline
            } else {
                DiffDisplayMode::Off
            };
        }
        if let Ok(v) = std::env::var("JCODE_PIN_IMAGES")
            && let Some(parsed) = parse_env_bool(&v)
        {
            self.display.pin_images = parsed;
        }
        if let Ok(v) = std::env::var("JCODE_DISPLAY_CENTERED") {
            if let Some(parsed) = parse_env_bool(&v) {
                self.display.centered = parsed;
            }
        }
        if let Ok(v) = std::env::var("JCODE_DIFF_LINE_WRAP") {
            if let Some(parsed) = parse_env_bool(&v) {
                self.display.diff_line_wrap = parsed;
            }
        }
        if let Ok(v) = std::env::var("JCODE_QUEUE_MODE") {
            if let Some(parsed) = parse_env_bool(&v) {
                self.display.queue_mode = parsed;
            }
        }
        if let Ok(v) = std::env::var("JCODE_AUTO_SERVER_RELOAD") {
            if let Some(parsed) = parse_env_bool(&v) {
                self.display.auto_server_reload = parsed;
            }
        }
        if let Ok(v) = std::env::var("JCODE_MOUSE_CAPTURE") {
            if let Some(parsed) = parse_env_bool(&v) {
                self.display.mouse_capture = parsed;
            }
        }
        if let Ok(v) = std::env::var("JCODE_DEBUG_SOCKET") {
            if let Some(parsed) = parse_env_bool(&v) {
                self.display.debug_socket = parsed;
            }
        }
        if let Ok(v) = std::env::var("JCODE_SHOW_THINKING") {
            if let Some(parsed) = parse_env_bool(&v) {
                self.display.show_thinking = parsed;
            }
        }
        if let Ok(v) = std::env::var("JCODE_MARKDOWN_SPACING") {
            match v.trim().to_lowercase().as_str() {
                "compact" => self.display.markdown_spacing = MarkdownSpacingMode::Compact,
                "document" | "doc" => {
                    self.display.markdown_spacing = MarkdownSpacingMode::Document;
                }
                _ => {}
            }
        }
        if let Ok(v) = std::env::var("JCODE_IDLE_ANIMATION") {
            if let Some(parsed) = parse_env_bool(&v) {
                self.display.idle_animation = parsed;
            }
        }
        if let Ok(v) = std::env::var("JCODE_PROMPT_ENTRY_ANIMATION") {
            if let Some(parsed) = parse_env_bool(&v) {
                self.display.prompt_entry_animation = parsed;
            }
        }
        if let Ok(v) = std::env::var("JCODE_DISABLED_ANIMATIONS") {
            self.display.disabled_animations = parse_env_list(&v);
        }
        if let Ok(v) = std::env::var("JCODE_PERFORMANCE") {
            let trimmed = v.trim().to_lowercase();
            if matches!(trimmed.as_str(), "auto" | "full" | "reduced" | "minimal") {
                self.display.performance = trimmed;
            }
        }
        if let Ok(v) = std::env::var("JCODE_ANIMATION_FPS") {
            if let Ok(fps) = v.trim().parse::<u32>() {
                self.display.animation_fps = fps.clamp(1, 120);
            }
        }
        if let Ok(v) = std::env::var("JCODE_REDRAW_FPS") {
            if let Ok(fps) = v.trim().parse::<u32>() {
                self.display.redraw_fps = fps.clamp(1, 120);
            }
        }
        if let Ok(v) = std::env::var("JCODE_CHAT_NATIVE_SCROLLBAR") {
            if let Some(parsed) = parse_env_bool(&v) {
                self.display.native_scrollbars.chat = parsed;
            }
        }
        if let Ok(v) = std::env::var("JCODE_SIDE_PANEL_NATIVE_SCROLLBAR") {
            if let Some(parsed) = parse_env_bool(&v) {
                self.display.native_scrollbars.side_panel = parsed;
            }
        }

        // Features
        if let Ok(v) = std::env::var("JCODE_MEMORY_ENABLED") {
            if let Some(parsed) = parse_env_bool(&v) {
                self.features.memory = parsed;
            }
        }
        if let Ok(v) = std::env::var("JCODE_SWARM_ENABLED") {
            if let Some(parsed) = parse_env_bool(&v) {
                self.features.swarm = parsed;
            }
        }
        if let Ok(v) = std::env::var("JCODE_MESSAGE_TIMESTAMPS") {
            if let Some(parsed) = parse_env_bool(&v) {
                self.features.message_timestamps = parsed;
            }
        }
        if let Ok(v) = std::env::var("JCODE_UPDATE_CHANNEL") {
            match v.trim().to_lowercase().as_str() {
                "main" | "nightly" | "edge" => {
                    self.features.update_channel = UpdateChannel::Main;
                }
                "stable" | "release" => {
                    self.features.update_channel = UpdateChannel::Stable;
                }
                _ => {}
            }
        }

        if let Ok(v) = std::env::var("JCODE_TRUSTED_EXTERNAL_AUTH_SOURCES") {
            let mut source_ids = Vec::new();
            let mut source_paths = Vec::new();
            for value in parse_env_list(&v) {
                let trimmed = value.trim();
                if trimmed.is_empty() {
                    continue;
                }
                if trimmed.contains('|') {
                    source_paths.push(trimmed.to_ascii_lowercase());
                } else {
                    source_ids.push(trimmed.to_ascii_lowercase());
                }
            }
            self.auth.trusted_external_sources = source_ids;
            self.auth.trusted_external_source_paths = source_paths;
        }

        // Autoreview
        if let Ok(v) = std::env::var("JCODE_AUTOREVIEW_ENABLED") {
            if let Some(parsed) = parse_env_bool(&v) {
                self.autoreview.enabled = parsed;
            }
        }
        if let Ok(v) = std::env::var("JCODE_AUTOREVIEW_MODEL") {
            let trimmed = v.trim();
            self.autoreview.model = if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            };
        }

        // Autojudge
        if let Ok(v) = std::env::var("JCODE_AUTOJUDGE_ENABLED") {
            if let Some(parsed) = parse_env_bool(&v) {
                self.autojudge.enabled = parsed;
            }
        }
        if let Ok(v) = std::env::var("JCODE_AUTOJUDGE_MODEL") {
            let trimmed = v.trim();
            self.autojudge.model = if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            };
        }

        // Ambient
        if let Ok(v) = std::env::var("JCODE_AMBIENT_ENABLED") {
            if let Some(parsed) = parse_env_bool(&v) {
                self.ambient.enabled = parsed;
            }
        }
        if let Ok(v) = std::env::var("JCODE_AMBIENT_PROVIDER") {
            self.ambient.provider = Some(v);
        }
        if let Ok(v) = std::env::var("JCODE_AMBIENT_MODEL") {
            self.ambient.model = Some(v);
        }
        if let Ok(v) = std::env::var("JCODE_AMBIENT_MIN_INTERVAL") {
            if let Ok(parsed) = v.trim().parse::<u32>() {
                self.ambient.min_interval_minutes = parsed;
            }
        }
        if let Ok(v) = std::env::var("JCODE_AMBIENT_MAX_INTERVAL") {
            if let Ok(parsed) = v.trim().parse::<u32>() {
                self.ambient.max_interval_minutes = parsed;
            }
        }
        if let Ok(v) = std::env::var("JCODE_AMBIENT_PROACTIVE") {
            if let Some(parsed) = parse_env_bool(&v) {
                self.ambient.proactive_work = parsed;
            }
        }

        // Safety / notifications
        if let Ok(v) = std::env::var("JCODE_NTFY_TOPIC") {
            self.safety.ntfy_topic = Some(v);
        }
        if let Ok(v) = std::env::var("JCODE_NTFY_SERVER") {
            self.safety.ntfy_server = v;
        }
        if let Ok(v) = std::env::var("JCODE_SMTP_PASSWORD") {
            self.safety.email_password = Some(v);
        }
        if let Ok(v) = std::env::var("JCODE_EMAIL_TO") {
            self.safety.email_to = Some(v);
            self.safety.email_enabled = true;
        }
        if let Ok(v) = std::env::var("JCODE_IMAP_HOST") {
            self.safety.email_imap_host = Some(v);
        }
        if let Ok(v) = std::env::var("JCODE_EMAIL_REPLY_ENABLED") {
            if let Some(parsed) = parse_env_bool(&v) {
                self.safety.email_reply_enabled = parsed;
            }
        }
        if let Ok(v) = std::env::var("JCODE_TELEGRAM_BOT_TOKEN") {
            self.safety.telegram_bot_token = Some(v);
            self.safety.telegram_enabled = true;
        }
        if let Ok(v) = std::env::var("JCODE_TELEGRAM_CHAT_ID") {
            self.safety.telegram_chat_id = Some(v);
        }
        if let Ok(v) = std::env::var("JCODE_TELEGRAM_REPLY_ENABLED") {
            if let Some(parsed) = parse_env_bool(&v) {
                self.safety.telegram_reply_enabled = parsed;
            }
        }
        if let Ok(v) = std::env::var("JCODE_DISCORD_BOT_TOKEN") {
            self.safety.discord_bot_token = Some(v);
            self.safety.discord_enabled = true;
        }
        if let Ok(v) = std::env::var("JCODE_DISCORD_CHANNEL_ID") {
            self.safety.discord_channel_id = Some(v);
        }
        if let Ok(v) = std::env::var("JCODE_DISCORD_BOT_USER_ID") {
            self.safety.discord_bot_user_id = Some(v);
        }
        if let Ok(v) = std::env::var("JCODE_DISCORD_REPLY_ENABLED") {
            if let Some(parsed) = parse_env_bool(&v) {
                self.safety.discord_reply_enabled = parsed;
            }
        }
        if let Ok(v) = std::env::var("JCODE_AMBIENT_VISIBLE") {
            if let Some(parsed) = parse_env_bool(&v) {
                self.ambient.visible = parsed;
            }
        }

        // Gateway (iOS/web)
        if let Ok(v) = std::env::var("JCODE_GATEWAY_ENABLED") {
            if let Some(parsed) = parse_env_bool(&v) {
                self.gateway.enabled = parsed;
            }
        }
        if let Ok(v) = std::env::var("JCODE_GATEWAY_PORT") {
            if let Ok(parsed) = v.trim().parse::<u16>() {
                self.gateway.port = parsed;
            }
        }
        if let Ok(v) = std::env::var("JCODE_GATEWAY_BIND_ADDR") {
            let trimmed = v.trim();
            if !trimmed.is_empty() {
                self.gateway.bind_addr = trimmed.to_string();
            }
        }

        // Provider
        if let Ok(v) = std::env::var("JCODE_MODEL") {
            self.provider.default_model = Some(v);
        }
        if let Ok(v) = std::env::var("JCODE_PROVIDER") {
            let trimmed = v.trim().to_lowercase();
            if !trimmed.is_empty() {
                self.provider.default_provider = Some(trimmed);
            }
        }
        if let Ok(v) = std::env::var("JCODE_OPENAI_REASONING_EFFORT") {
            let trimmed = v.trim().to_string();
            if !trimmed.is_empty() {
                self.provider.openai_reasoning_effort = Some(trimmed);
            }
        }
        if let Ok(v) = std::env::var("JCODE_OPENAI_TRANSPORT") {
            let trimmed = v.trim().to_string();
            if !trimmed.is_empty() {
                self.provider.openai_transport = Some(trimmed);
            }
        }
        if let Ok(v) = std::env::var("JCODE_OPENAI_SERVICE_TIER") {
            let trimmed = v.trim().to_string();
            if !trimmed.is_empty() {
                self.provider.openai_service_tier = Some(trimmed);
            }
        }
        if let Ok(v) = std::env::var("JCODE_OPENAI_NATIVE_COMPACTION_MODE") {
            let trimmed = v.trim().to_ascii_lowercase();
            if !trimmed.is_empty() {
                self.provider.openai_native_compaction_mode = trimmed;
            }
        }
        if let Ok(v) = std::env::var("JCODE_OPENAI_NATIVE_COMPACTION_THRESHOLD_TOKENS") {
            if let Ok(parsed) = v.trim().parse::<usize>() {
                if parsed > 0 {
                    self.provider.openai_native_compaction_threshold_tokens = parsed;
                }
            }
        }
        if let Ok(v) = std::env::var("JCODE_CROSS_PROVIDER_FAILOVER") {
            if let Some(mode) = CrossProviderFailoverMode::parse(&v) {
                self.provider.cross_provider_failover = mode;
            }
        }
        if let Ok(v) = std::env::var("JCODE_SAME_PROVIDER_ACCOUNT_FAILOVER") {
            if let Some(enabled) = parse_env_bool(&v) {
                self.provider.same_provider_account_failover = enabled;
            }
        }

        // Copilot premium mode: env var overrides config
        // If set in config but not in env, propagate config -> env
        if let Ok(v) = std::env::var("JCODE_COPILOT_PREMIUM") {
            self.provider.copilot_premium = Some(v);
        } else if let Some(ref mode) = self.provider.copilot_premium {
            let env_val = match mode.as_str() {
                "zero" | "0" => "0",
                "one" | "1" => "1",
                _ => "",
            };
            if !env_val.is_empty() {
                crate::env::set_var("JCODE_COPILOT_PREMIUM", env_val);
            }
        }
    }
}

fn parse_env_bool(raw: &str) -> Option<bool> {
    match raw.trim().to_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

fn parse_env_list(raw: &str) -> Vec<String> {
    raw.split([',', '\n'])
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(ToString::to_string)
        .collect()
}
