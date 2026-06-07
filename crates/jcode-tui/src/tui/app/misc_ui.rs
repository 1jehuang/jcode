use super::*;

/// Resolved per-million-token pricing for the active model, used to turn a
/// single API call's token usage into a dollar cost. Shared by the local
/// (`update_cost_impl`) and remote (`accrue_remote_call_cost`) billing paths so
/// they cannot drift apart.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ResolvedTokenPricing {
    /// Fresh (uncached) input price in $/1M tokens.
    pub prompt_price: f32,
    /// Output/completion price in $/1M tokens.
    pub completion_price: f32,
    /// Cache-read price in $/1M tokens when known; falls back to `prompt_price`.
    pub cache_read_price: Option<f32>,
    /// Whether the active model is Anthropic/Claude (drives split-accounting and
    /// the cache-write premium).
    pub is_anthropic: bool,
}

impl ResolvedTokenPricing {
    /// Dollar cost of one API call's reported usage.
    ///
    /// Providers report usage with two different conventions:
    ///   - Split accounting (Anthropic): `input_tokens` already EXCLUDES the
    ///     cache-read and cache-creation counts, which are reported separately.
    ///     Subtracting cache-read from input again would double count it and bill
    ///     fresh input at ~$0 on cache-hit turns.
    ///   - Subset accounting (OpenAI-style): cached tokens are counted INSIDE
    ///     `input_tokens`, so we subtract the cache-read portion to bill it at the
    ///     cheaper cache rate.
    ///
    /// Mirrors the heuristic the cache/context paths use (see
    /// `effective_prompt_tokens` / `effective_context_tokens_from_usage`).
    pub fn cost_for_usage(
        &self,
        input_tokens: u64,
        output_tokens: u64,
        cache_read_tokens: u64,
        cache_creation_tokens: u64,
    ) -> f32 {
        let split_accounting =
            self.is_anthropic || cache_creation_tokens > 0 || cache_read_tokens > input_tokens;

        let fresh_input_tokens = if split_accounting {
            input_tokens
        } else {
            input_tokens.saturating_sub(cache_read_tokens.min(input_tokens))
        };

        let prompt_cost = (fresh_input_tokens as f32 * self.prompt_price) / 1_000_000.0;
        let completion_cost = (output_tokens as f32 * self.completion_price) / 1_000_000.0;
        // Cache-read tokens are billed at the (cheaper) cache-read rate when we
        // know it; otherwise treat them as regular input tokens.
        let cache_read_cost = match self.cache_read_price {
            Some(price) => (cache_read_tokens as f32 * price) / 1_000_000.0,
            None => (cache_read_tokens as f32 * self.prompt_price) / 1_000_000.0,
        };
        // Cache *writes* (cache-creation) are billed at a premium over the base
        // input rate. Anthropic charges 1.25x for the 5-minute TTL and 2x for the
        // 1-hour TTL; other split-accounting providers we approximate at the base
        // input rate. Subset-accounting providers fold writes into `input_tokens`
        // (and rarely report a creation count), so we only add this for split
        // accounting to avoid double counting.
        let cache_write_cost = if split_accounting && cache_creation_tokens > 0 {
            let multiplier = if self.is_anthropic {
                if crate::provider::anthropic::is_cache_ttl_1h() {
                    2.0
                } else {
                    1.25
                }
            } else {
                1.0
            };
            (cache_creation_tokens as f32 * self.prompt_price * multiplier) / 1_000_000.0
        } else {
            0.0
        };

        prompt_cost + completion_cost + cache_read_cost + cache_write_cost
    }
}

/// Update cost calculation based on token usage (for API-key providers)
impl App {
    pub(super) fn current_streaming_tps_elapsed(&self) -> Duration {
        let mut elapsed = self.streaming.streaming_tps_elapsed;
        if let Some(start) = self.streaming.streaming_tps_start {
            elapsed += start.elapsed();
        }
        elapsed
    }

    pub(super) fn snapshot_streaming_tps(&mut self) {
        self.streaming.streaming_tps_observed_output_tokens =
            self.streaming.streaming_total_output_tokens;
        self.streaming.streaming_tps_observed_elapsed = self.current_streaming_tps_elapsed();
    }

    pub(super) fn resume_streaming_tps(&mut self) {
        self.streaming.streaming_tps_collect_output = true;
        if self.streaming.streaming_tps_start.is_none() {
            self.streaming.streaming_tps_start = Some(Instant::now());
        }
    }

    pub(super) fn pause_streaming_tps(&mut self, keep_collecting_output: bool) {
        if let Some(start) = self.streaming.streaming_tps_start.take() {
            self.streaming.streaming_tps_elapsed += start.elapsed();
        }
        self.streaming.streaming_tps_collect_output = keep_collecting_output;
    }

    pub(super) fn reset_streaming_tps(&mut self) {
        self.streaming.streaming_tps_start = None;
        self.streaming.streaming_tps_elapsed = Duration::ZERO;
        self.streaming.streaming_tps_collect_output = false;
        self.streaming.streaming_total_output_tokens = 0;
        self.streaming.streaming_tps_observed_output_tokens = 0;
        self.streaming.streaming_tps_observed_elapsed = Duration::ZERO;
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

        let pinned_anthropic = jcode_provider_core::pinned_mode_for(
            jcode_provider_core::DualAuthProvider::Anthropic,
            runtime_provider.as_deref(),
        );
        let pinned_openai = jcode_provider_core::pinned_mode_for(
            jcode_provider_core::DualAuthProvider::OpenAI,
            runtime_provider.as_deref(),
        );
        let is_explicit_anthropic_api = matches!(
            pinned_anthropic,
            Some(jcode_provider_core::AuthMode::ApiKey)
        );
        let is_explicit_anthropic_oauth =
            matches!(pinned_anthropic, Some(jcode_provider_core::AuthMode::Oauth));
        let is_explicit_openai_api =
            matches!(pinned_openai, Some(jcode_provider_core::AuthMode::ApiKey));
        let is_explicit_openai_oauth =
            matches!(pinned_openai, Some(jcode_provider_core::AuthMode::Oauth));

        let is_anthropic = provider_name.contains("anthropic") || provider_name.contains("claude");
        let is_openai = provider_name.contains("openai");

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
        } else if is_openai {
            is_explicit_openai_api
                || (!is_explicit_openai_oauth
                    && auth_status.openai_has_api_key
                    && !auth_status.openai_has_oauth)
        } else if provider_name.contains("bedrock")
            || provider_name.contains("azure-openai")
            || crate::provider_catalog::openai_compatible_profile_by_id(provider_name.trim())
                .is_some_and(|profile| profile.requires_api_key)
        {
            true
        } else {
            false
        };

        if !billed_per_token {
            return;
        }

        let model = self.provider.model().to_string();
        self.refresh_cached_pricing(&model, is_anthropic, is_openai);

        // Pricing in $/1M tokens. Anthropic resolves real per-model pricing in
        // refresh_cached_pricing; other providers fall back to the generic
        // defaults cached here.
        let prompt_price = *self.cost.cached_prompt_price.get_or_insert(15.0);
        let completion_price = *self.cost.cached_completion_price.get_or_insert(60.0);
        let cache_read_price = self.cost.cached_cache_read_price;

        let pricing = ResolvedTokenPricing {
            prompt_price,
            completion_price,
            cache_read_price,
            is_anthropic,
        };

        self.cost.total_cost += pricing.cost_for_usage(
            self.streaming.streaming_input_tokens,
            self.streaming.streaming_output_tokens,
            self.streaming.streaming_cache_read_tokens.unwrap_or(0),
            self.streaming.streaming_cache_creation_tokens.unwrap_or(0),
        );
    }

    /// Accrue the dollar cost of a single completed remote API call.
    ///
    /// Local turns bill once at `finish_turn` via [`App::update_cost_impl`], but
    /// the default interactive TUI is a *remote* client: it receives per-call
    /// `ServerEvent::TokenUsage` and never runs the local cost path, so without
    /// this the cost figure was stuck at `$0`. The server does not report a
    /// dollar cost, only tokens, so the client prices each call itself.
    ///
    /// `input`/`output` are this call's totals and `*_delta` are the new tokens
    /// since the previous usage snapshot for the same call, so a streaming call
    /// that reports usage multiple times is billed exactly once overall.
    pub(super) fn accrue_remote_call_cost(
        &mut self,
        input_delta: u64,
        output_delta: u64,
        cache_read_delta: u64,
        cache_creation_delta: u64,
    ) {
        if input_delta == 0
            && output_delta == 0
            && cache_read_delta == 0
            && cache_creation_delta == 0
        {
            return;
        }
        let Some(pricing) = self.resolve_remote_cost_pricing() else {
            return;
        };
        self.cost.total_cost += pricing.cost_for_usage(
            input_delta,
            output_delta,
            cache_read_delta,
            cache_creation_delta,
        );
    }

    /// Resolve per-token pricing for the active *remote* session, or `None` when
    /// the session is not billed per token (e.g. an OAuth subscription, or a
    /// provider we cannot price). Mirrors the cost-based decision the info widget
    /// uses so the displayed `$` total and the widget stay consistent.
    fn resolve_remote_cost_pricing(&mut self) -> Option<ResolvedTokenPricing> {
        use crate::tui::TuiState;
        if !self.is_remote {
            return None;
        }

        let model = <Self as TuiState>::provider_model(self);
        let provider_name = <Self as TuiState>::provider_name(self).to_lowercase();
        let is_anthropic = provider_name.contains("anthropic") || provider_name.contains("claude");
        let is_openai = provider_name.contains("openai");

        // The server resolves the active credential authoritatively; only bill
        // when it is an API key (OAuth subscriptions are not metered per token).
        let api_key_billed = matches!(
            self.remote_resolved_credential,
            Some(jcode_provider_core::ResolvedCredential::ApiKey)
        );

        // For dual-auth providers (Anthropic/OpenAI) we require an API-key
        // credential. Other cost-based providers (OpenCode, OpenRouter direct,
        // bedrock-style API-key profiles) always meter per token when remote.
        let billed = if is_anthropic || is_openai {
            api_key_billed
        } else {
            // Providers that are inherently cost-based when proxied remotely.
            provider_name.contains("opencode")
                || provider_name.contains("openrouter")
                || provider_name.contains("bedrock")
                || provider_name.contains("cerebras")
                || provider_name.contains("compatible")
        };
        if !billed {
            return None;
        }

        self.refresh_cached_pricing(&model, is_anthropic, is_openai);
        Some(ResolvedTokenPricing {
            prompt_price: *self.cost.cached_prompt_price.get_or_insert(15.0),
            completion_price: *self.cost.cached_completion_price.get_or_insert(60.0),
            cache_read_price: self.cost.cached_cache_read_price,
            is_anthropic,
        })
    }

    /// Resolve and cache per-model pricing for the active provider. For
    /// Anthropic/Claude and OpenAI models we use the published API pricing
    /// (input, output and cache-read) so the API-key cost figure is accurate per
    /// model. Re-resolves when the active model changes.
    fn refresh_cached_pricing(&mut self, model: &str, is_anthropic: bool, is_openai: bool) {
        if self.cost.cached_price_model.as_deref() == Some(model) {
            return;
        }

        let per_mtok = |micros: Option<u64>| micros.map(|m| m as f32 / 1_000_000.0);
        let estimate = if is_anthropic {
            jcode_provider_core::pricing::anthropic_api_pricing(model)
        } else if is_openai {
            jcode_provider_core::pricing::openai_api_pricing(model)
        } else {
            None
        };

        if let Some(estimate) = estimate {
            self.cost.cached_prompt_price = per_mtok(estimate.input_price_per_mtok_micros);
            self.cost.cached_completion_price = per_mtok(estimate.output_price_per_mtok_micros);
            self.cost.cached_cache_read_price = per_mtok(estimate.cache_read_price_per_mtok_micros);
            self.cost.cached_price_model = Some(model.to_string());
            return;
        }

        // Unknown model: leave existing defaults in place but remember the model
        // so we do not repeatedly attempt resolution for it.
        self.cost.cached_price_model = Some(model.to_string());
    }

    pub(super) fn compute_streaming_tps(&self) -> Option<f32> {
        let elapsed_secs = self.streaming.streaming_tps_observed_elapsed.as_secs_f32();
        let total_tokens = self.streaming.streaming_tps_observed_output_tokens;
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
