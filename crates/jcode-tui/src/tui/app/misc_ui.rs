use super::*;

/// Update cost calculation based on token usage (for API-key providers)
impl App {
    pub(super) fn current_streaming_tps_elapsed(&self) -> Duration {
        let mut elapsed = self.streaming_tps_elapsed;
        if let Some(start) = self.streaming_tps_start {
            elapsed += start.elapsed();
        }
        elapsed
    }

    pub(super) fn snapshot_streaming_tps(&mut self) {
        self.streaming_tps_observed_output_tokens = self.streaming_total_output_tokens;
        self.streaming_tps_observed_elapsed = self.current_streaming_tps_elapsed();
    }

    pub(super) fn resume_streaming_tps(&mut self) {
        self.streaming_tps_collect_output = true;
        if self.streaming_tps_start.is_none() {
            self.streaming_tps_start = Some(Instant::now());
        }
    }

    pub(super) fn pause_streaming_tps(&mut self, keep_collecting_output: bool) {
        if let Some(start) = self.streaming_tps_start.take() {
            self.streaming_tps_elapsed += start.elapsed();
        }
        self.streaming_tps_collect_output = keep_collecting_output;
    }

    pub(super) fn reset_streaming_tps(&mut self) {
        self.streaming_tps_start = None;
        self.streaming_tps_elapsed = Duration::ZERO;
        self.streaming_tps_collect_output = false;
        self.streaming_total_output_tokens = 0;
        self.streaming_tps_observed_output_tokens = 0;
        self.streaming_tps_observed_elapsed = Duration::ZERO;
    }

    pub(super) fn open_usage_inline_loading(&mut self) {
        self.push_usage_loading_card();
        self.inline_interactive_state = None;
        self.inline_view_state = None;
        self.input.clear();
        self.cursor_pos = 0;
        self.set_status_notice("Usage → refreshing");
    }

    pub(super) fn request_usage_report(&mut self) {
        use crate::bus::{Bus, BusEvent};

        if self.usage_report_refreshing {
            return;
        }
        self.usage_report_refreshing = true;

        let publish = || async move {
            let results = crate::usage::fetch_all_provider_usage_progressive(|progress| {
                Bus::global().publish(BusEvent::UsageReportProgress(progress));
            })
            .await;
            Bus::global().publish(BusEvent::UsageReport(results));
        };

        if tokio::runtime::Handle::try_current().is_ok() {
            tokio::spawn(publish());
        } else {
            std::thread::spawn(move || {
                if let Ok(runtime) = tokio::runtime::Runtime::new() {
                    runtime.block_on(publish());
                }
            });
        }
    }

    pub(super) fn update_cost_impl(&mut self) {
        let provider_name = self.provider.name().to_lowercase();
        let runtime_provider = active_runtime_provider_key();
        let auth_status = crate::auth::AuthStatus::check_fast();

        let is_explicit_anthropic_api = matches!(
            runtime_provider.as_deref(),
            Some("claude-api" | "anthropic-api")
        );
        let is_explicit_anthropic_oauth =
            matches!(runtime_provider.as_deref(), Some("claude" | "anthropic"));
        let is_explicit_openai_api = matches!(runtime_provider.as_deref(), Some("openai-api"));
        let is_explicit_openai_oauth = matches!(runtime_provider.as_deref(), Some("openai"));

        let is_anthropic = provider_name.contains("anthropic") || provider_name.contains("claude");

        // Whether the user is billed per token for this turn (direct API key).
        let billed_per_token = if provider_name.contains("openrouter") {
            crate::provider::openrouter::OpenRouterTransportState::from_current_env(
                runtime_provider.as_deref(),
            )
            .accrues_user_api_key_cost()
        } else if is_anthropic {
            // Anthropic Auto prefers OAuth (Claude subscription, no per-token
            // user cost) when OAuth credentials exist, so only accrue API-key
            // cost when the API key is the credential that will actually be used.
            is_explicit_anthropic_api
                || (!is_explicit_anthropic_oauth
                    && auth_status.anthropic.has_api_key
                    && !auth_status.anthropic.has_oauth)
        } else if provider_name.contains("openai") {
            is_explicit_openai_api
                || (!is_explicit_openai_oauth
                    && auth_status.openai_has_api_key
                    && !auth_status.openai_has_oauth)
        } else {
            provider_name.contains("bedrock")
                || provider_name.contains("azure-openai")
                || crate::provider_catalog::openai_compatible_profile_by_id(provider_name.trim())
                    .is_some_and(|profile| profile.requires_api_key)
        };

        // For Anthropic OAuth / Claude subscription users there is no per-token
        // billing, but we can still surface an estimated equivalent API cost so
        // the info widget shows a dollar figure (like OpenRouter does).
        let estimate_only = !billed_per_token && is_anthropic;

        if !billed_per_token && !estimate_only {
            return;
        }

        self.refresh_cached_pricing(is_anthropic);

        // Pricing in $/1M tokens. Anthropic resolves real per-model pricing in
        // refresh_cached_pricing; other providers fall back to the generic
        // defaults cached here.
        let prompt_price = *self.cached_prompt_price.get_or_insert(15.0);
        let completion_price = *self.cached_completion_price.get_or_insert(60.0);
        let cache_read_price = self.cached_cache_read_price;

        // Cache-read tokens are billed at the (cheaper) cache-read rate when we
        // know it; otherwise treat them as regular input tokens.
        let cache_read_tokens = self.streaming_cache_read_tokens.unwrap_or(0);
        let full_input_tokens = self
            .streaming_input_tokens
            .saturating_sub(cache_read_tokens.min(self.streaming_input_tokens));

        let prompt_cost = (full_input_tokens as f32 * prompt_price) / 1_000_000.0;
        let completion_cost =
            (self.streaming_output_tokens as f32 * completion_price) / 1_000_000.0;
        let cache_read_cost = match cache_read_price {
            Some(price) => (cache_read_tokens as f32 * price) / 1_000_000.0,
            None => (cache_read_tokens as f32 * prompt_price) / 1_000_000.0,
        };
        let turn_cost = prompt_cost + completion_cost + cache_read_cost;

        if estimate_only {
            *self.estimated_cost.get_or_insert(0.0) += turn_cost;
        } else {
            self.total_cost += turn_cost;
        }
    }

    /// Resolve and cache per-model pricing for the active provider. For
    /// Anthropic/Claude models we use the published API pricing (input, output
    /// and cache-read), which lets us show an accurate dollar estimate even on
    /// the subscription/OAuth plan. Re-resolves when the active model changes.
    fn refresh_cached_pricing(&mut self, is_anthropic: bool) {
        let model = self.provider.model().to_string();
        if self.cached_price_model.as_deref() == Some(model.as_str()) {
            return;
        }

        if is_anthropic {
            if let Some(estimate) = jcode_provider_core::pricing::anthropic_api_pricing(&model) {
                let per_mtok = |micros: Option<u64>| micros.map(|m| m as f32 / 1_000_000.0);
                self.cached_prompt_price = per_mtok(estimate.input_price_per_mtok_micros);
                self.cached_completion_price = per_mtok(estimate.output_price_per_mtok_micros);
                self.cached_cache_read_price = per_mtok(estimate.cache_read_price_per_mtok_micros);
                self.cached_price_model = Some(model);
                return;
            }
        }

        // Unknown model: leave existing defaults in place but remember the model
        // so we do not repeatedly attempt resolution for it.
        self.cached_price_model = Some(model);
    }

    pub(super) fn compute_streaming_tps(&self) -> Option<f32> {
        let elapsed_secs = self.streaming_tps_observed_elapsed.as_secs_f32();
        let total_tokens = self.streaming_tps_observed_output_tokens;
        if elapsed_secs > 0.1 && total_tokens > 0 {
            Some(total_tokens as f32 / elapsed_secs)
        } else {
            None
        }
    }

    pub(super) fn handle_changelog_key(&mut self, code: KeyCode) -> Result<()> {
        let scroll = self.changelog_scroll.unwrap_or(0);
        match code {
            KeyCode::Esc | KeyCode::Char('q') => {
                self.changelog_scroll = None;
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.changelog_scroll = Some(scroll.saturating_add(1));
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.changelog_scroll = Some(scroll.saturating_sub(1));
            }
            KeyCode::PageDown | KeyCode::Char(' ') => {
                self.changelog_scroll = Some(scroll.saturating_add(20));
            }
            KeyCode::PageUp => {
                self.changelog_scroll = Some(scroll.saturating_sub(20));
            }
            KeyCode::Home | KeyCode::Char('g') => {
                self.changelog_scroll = Some(0);
            }
            KeyCode::End | KeyCode::Char('G') => {
                self.changelog_scroll = Some(usize::MAX);
            }
            _ => {}
        }
        Ok(())
    }

    pub(super) fn handle_help_key(&mut self, code: KeyCode) -> Result<()> {
        let scroll = self.help_scroll.unwrap_or(0);
        match code {
            KeyCode::Esc | KeyCode::Char('q') => {
                self.help_scroll = None;
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.help_scroll = Some(scroll.saturating_add(1));
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.help_scroll = Some(scroll.saturating_sub(1));
            }
            KeyCode::PageDown | KeyCode::Char(' ') => {
                self.help_scroll = Some(scroll.saturating_add(20));
            }
            KeyCode::PageUp => {
                self.help_scroll = Some(scroll.saturating_sub(20));
            }
            KeyCode::Home | KeyCode::Char('g') => {
                self.help_scroll = Some(0);
            }
            KeyCode::End | KeyCode::Char('G') => {
                self.help_scroll = Some(usize::MAX);
            }
            _ => {}
        }
        Ok(())
    }

    pub(super) fn handle_model_status_key(&mut self, code: KeyCode) -> Result<()> {
        let scroll = self.model_status_scroll.unwrap_or(0);
        match code {
            KeyCode::Esc | KeyCode::Char('q') => {
                self.model_status_scroll = None;
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.model_status_scroll = Some(scroll.saturating_add(1));
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.model_status_scroll = Some(scroll.saturating_sub(1));
            }
            KeyCode::PageDown | KeyCode::Char(' ') => {
                self.model_status_scroll = Some(scroll.saturating_add(20));
            }
            KeyCode::PageUp => {
                self.model_status_scroll = Some(scroll.saturating_sub(20));
            }
            KeyCode::Home | KeyCode::Char('g') => {
                self.model_status_scroll = Some(0);
            }
            KeyCode::End | KeyCode::Char('G') => {
                self.model_status_scroll = Some(usize::MAX);
            }
            KeyCode::Char('c') => {
                let success = super::helpers::copy_to_clipboard(&self.model_status_content);
                if success {
                    self.set_status_notice("Copied provider test coverage report".to_string());
                } else {
                    self.set_status_notice(
                        "Failed to copy provider test coverage report".to_string(),
                    );
                }
            }
            _ => {}
        }
        Ok(())
    }
}
