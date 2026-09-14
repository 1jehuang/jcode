use super::*;
use jcode_provider_core::Currency;

/// Resolved per-million-token pricing for the active model, used to turn a
/// single API call's token usage into a dollar cost. Shared by the local
/// (`update_cost_impl`) and remote (`accrue_remote_call_cost`) billing paths so
/// they cannot drift apart.
#[derive(Clone, Debug)]
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
    /// OpenAI reports both reads and writes as subsets of input_tokens.
    pub is_openai: bool,
    /// Currency the rates above are denominated in. It is the currency of the
    /// layer that produced the rates, never the provider's (F1).
    pub currency: Currency,
}

/// How one API call is priced.
#[derive(Clone, Debug)]
pub(crate) enum PinnedCallPricing {
    /// Rates in effect for this call, resolved at the call's own instant.
    Priced(ResolvedTokenPricing),
    /// A hand-written `[pricing.providers]` rule claims this model but cannot
    /// price the call: the card is incomplete in a currency that cannot merge
    /// with the layer below, or its rule expired with `on_rule_expiry =
    /// "no_price"`. The call is deliberately left unpriced instead of being
    /// billed at the generic defaults (spec 4.4): a number the user never wrote
    /// is worse than no number.
    ConfiguredWithoutPrice,
}

impl ResolvedTokenPricing {
    /// The rates a resolved card states, in the card's own currency.
    fn from_rate_card(
        card: &crate::model_pricing::CallRateCard,
        is_anthropic: bool,
        is_openai: bool,
    ) -> Self {
        Self {
            prompt_price: card.input_per_mtok as f32,
            completion_price: card.output_per_mtok as f32,
            cache_read_price: card.cache_read_per_mtok.map(|rate| rate as f32),
            is_anthropic,
            is_openai,
            currency: card.currency.clone(),
        }
    }

    /// Dollar cost of one API call's reported usage.
    ///
    /// Returns the amount together with the currency its rates are in, so the
    /// caller can accrue and record it without assuming USD (F1).
    ///
    /// Providers report usage with two different conventions:
    ///   - Split accounting (Anthropic): `input_tokens` already EXCLUDES the
    ///     cache-read and cache-creation counts, which are reported separately.
    ///     Subtracting cache-read from input again would double count it and bill
    ///     fresh input at ~$0 on cache-hit turns.
    ///   - Subset accounting (OpenAI-style): cached tokens are counted INSIDE
    ///     `input_tokens`, so subtract both reads and writes before pricing each
    ///     category once. Modern OpenAI cache writes use a 1.25x input rate.
    pub fn cost_for_usage(
        &self,
        input_tokens: u64,
        output_tokens: u64,
        cache_read_tokens: u64,
        cache_creation_tokens: u64,
    ) -> (f32, Currency) {
        let split_accounting = self.is_anthropic
            || (!self.is_openai && (cache_creation_tokens > 0 || cache_read_tokens > input_tokens));

        let fresh_input_tokens = if split_accounting {
            input_tokens
        } else {
            input_tokens
                .saturating_sub(cache_read_tokens)
                .saturating_sub(cache_creation_tokens)
        };

        let prompt_cost = (fresh_input_tokens as f32 * self.prompt_price) / 1_000_000.0;
        let completion_cost = (output_tokens as f32 * self.completion_price) / 1_000_000.0;
        // Cache-read tokens are billed at the (cheaper) cache-read rate when we
        // know it; otherwise treat them as regular input tokens.
        let cache_read_cost = match self.cache_read_price {
            Some(price) => (cache_read_tokens as f32 * price) / 1_000_000.0,
            None => (cache_read_tokens as f32 * self.prompt_price) / 1_000_000.0,
        };
        // Write pricing replaces ordinary input pricing, it is not an extra
        // charge on top of already-billed OpenAI input tokens.
        let cache_write_cost = if cache_creation_tokens > 0 {
            let multiplier = if self.is_anthropic {
                if crate::provider::anthropic::is_cache_ttl_1h() {
                    2.0
                } else {
                    1.25
                }
            } else if self.is_openai {
                1.25
            } else {
                1.0
            };
            (cache_creation_tokens as f32 * self.prompt_price * multiplier) / 1_000_000.0
        } else {
            0.0
        };

        let amount = prompt_cost + completion_cost + cache_read_cost + cache_write_cost;
        (amount, self.currency.clone())
    }
}

fn remote_provider_is_inherently_billed(provider_name: &str) -> bool {
    provider_name.contains("opencode")
        || provider_name.contains("openrouter")
        || provider_name.contains("bedrock")
        || provider_name.contains("cerebras")
        || provider_name.contains("compatible")
        || crate::provider_catalog::openai_compatible_profile_id_for_display_name(provider_name)
            .and_then(crate::provider_catalog::openai_compatible_profile_by_id)
            .is_some_and(|profile| profile.requires_api_key)
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
        } else {
            provider_name.contains("bedrock")
                || provider_name.contains("azure-openai")
                || crate::provider_catalog::openai_compatible_profile_by_id(provider_name.trim())
                    .is_some_and(|profile| profile.requires_api_key)
        };

        if !billed_per_token {
            return;
        }

        let model = self.provider.model().to_string();
        // The call is priced at the instant it was sent (F15); a second
        // `update_cost_impl` for the same call reuses the pinned card.
        let at = self.cost.call_started_at.unwrap_or_else(SystemTime::now);
        let pricing = self.call_pricing(at, &model, is_anthropic, is_openai);
        let PinnedCallPricing::Priced(pricing) = pricing else {
            // The user configured a rule for this model that cannot price the
            // call. Leave it unpriced: the generic $15/$60 estimate would
            // report a number they never wrote (spec 4.4).
            return;
        };

        let (call_cost, currency) = pricing.cost_for_usage(
            self.streaming.streaming_input_tokens,
            self.streaming.streaming_output_tokens,
            self.streaming.streaming_cache_read_tokens.unwrap_or(0),
            self.streaming.streaming_cache_creation_tokens.unwrap_or(0),
        );
        self.cost.total_cost += call_cost;
        self.record_api_key_spend(call_cost, &currency);
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
    ///
    /// `at` is the instant of this usage snapshot. The first snapshot priced for
    /// a call decides its rate card and pins it (F15/F16); later deltas of the
    /// same call reuse that card whatever `at` says, so a call that crosses a
    /// peak/off-peak boundary is not re-priced mid-flight.
    pub(super) fn accrue_remote_call_cost(
        &mut self,
        input_delta: u64,
        output_delta: u64,
        cache_read_delta: u64,
        cache_creation_delta: u64,
        at: SystemTime,
    ) {
        if input_delta == 0
            && output_delta == 0
            && cache_read_delta == 0
            && cache_creation_delta == 0
        {
            return;
        }
        if !self.remote_call_is_metered() {
            return;
        }
        let (model, is_anthropic, is_openai) = self.remote_billing_identity();
        let PinnedCallPricing::Priced(pricing) =
            self.call_pricing(at, &model, is_anthropic, is_openai)
        else {
            return;
        };
        let (call_cost, currency) = pricing.cost_for_usage(
            input_delta,
            output_delta,
            cache_read_delta,
            cache_creation_delta,
        );
        self.cost.total_cost += call_cost;
        self.record_api_key_spend(call_cost, &currency);
    }

    /// Seed `cost.total_cost` from token totals restored when resuming a
    /// session, so the cost widget reflects prior spend instead of showing `$0`
    /// until a new call happens.
    ///
    /// The live path (`accrue_remote_call_cost`) only ever observes per-call
    /// usage events for calls that happen *during* this client's lifetime. When
    /// an older session is reopened, its historical token totals are restored
    /// from the server but the dollar cost was never reconstructed, leaving the
    /// widget stuck at `$0`. This prices the restored totals once, the same way
    /// a single completed call is priced, and overwrites `total_cost` (rather
    /// than accruing) so it is idempotent across repeated history snapshots.
    pub(super) fn seed_cost_from_history_totals(
        &mut self,
        totals: &crate::protocol::TokenUsageTotals,
        at: SystemTime,
    ) {
        if totals.input_tokens == 0 && totals.output_tokens == 0 {
            return;
        }
        if !self.remote_call_is_metered() {
            return;
        }
        // Restored totals have no per-call instant of their own; the snapshot's
        // instant is the best available and is passed in so tests can fix it.
        let (model, is_anthropic, is_openai) = self.remote_billing_identity();
        let PinnedCallPricing::Priced(pricing) =
            self.resolve_call_pricing(at, &model, is_anthropic, is_openai)
        else {
            return;
        };
        let (cost, _currency) = pricing.cost_for_usage(
            totals.input_tokens,
            totals.output_tokens,
            totals.cache_read_input_tokens,
            totals.cache_creation_input_tokens,
        );
        if cost.is_finite() {
            self.cost.total_cost = cost;
        }
    }

    /// Persist an API-key call cost into the cross-provider activity ledger so
    /// `/usage` can show per-login spend (today / month / all-time). Only ever
    /// called from the billed-per-token paths, so every dollar recorded here
    /// is real API-key spend rather than subscription usage.
    ///
    /// The cost is recorded in the currency its rate card is denominated in:
    /// the ledger keeps one bucket per currency, so a CNY call lands in a CNY
    /// bucket rather than being mislabelled as USD.
    fn record_api_key_spend(&self, call_cost: f32, currency: &Currency) {
        if !call_cost.is_finite() || call_cost <= 0.0 {
            return;
        }
        use crate::tui::TuiState;
        let label = <Self as TuiState>::provider_name(self);
        let runtime = active_runtime_provider_key();
        let source_key =
            crate::provider_activity::source_key_for_provider_label(&label, runtime.as_deref());
        let cost = call_cost as f64;
        let currency = currency.clone();
        // Ledger writes hit the filesystem; never block the render/input loop.
        std::thread::spawn(move || {
            crate::provider_activity::record_spend(&source_key, cost, &currency);
        });
    }

    /// Whether the active *remote* session bills per token. Mirrors the
    /// cost-based decision the info widget uses so the displayed total and the
    /// widget stay consistent. OAuth subscriptions are not metered.
    fn remote_call_is_metered(&self) -> bool {
        use crate::tui::TuiState;
        if !self.is_remote {
            return false;
        }

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
        if is_anthropic || is_openai {
            api_key_billed
        } else {
            // Providers that are inherently cost-based when proxied remotely.
            remote_provider_is_inherently_billed(&provider_name)
        }
    }

    /// The model and dual-auth flags the remote billing paths price with.
    fn remote_billing_identity(&self) -> (String, bool, bool) {
        use crate::tui::TuiState;
        let model = <Self as TuiState>::provider_model(self);
        let provider_name = <Self as TuiState>::provider_name(self).to_lowercase();
        let is_anthropic = provider_name.contains("anthropic") || provider_name.contains("claude");
        let is_openai = provider_name.contains("openai");
        (model, is_anthropic, is_openai)
    }

    /// Mark the start of a new API call: remember its instant (F15) and drop the
    /// card pinned to the previous call (F16).
    pub(super) fn begin_call_pricing(&mut self, at: SystemTime) {
        self.cost.call_started_at = Some(at);
        self.cost.pinned_call_pricing = None;
    }

    /// Everything that has to happen when a new API call starts.
    ///
    /// Per-call usage accounting (`mark_stream_usage_call_boundary`) and
    /// per-call pricing are the same boundary: the call's own instant decides
    /// its tariff (F15), and the card pinned to the previous call must not leak
    /// into this one (F16).
    pub(super) fn begin_api_call_accounting(&mut self) {
        self.mark_stream_usage_call_boundary();
        self.begin_call_pricing(SystemTime::now());
    }

    /// The rate card for the call currently being priced, resolved once at `at`
    /// and reused for every later snapshot of the same call (F16).
    fn call_pricing(
        &mut self,
        at: SystemTime,
        model: &str,
        is_anthropic: bool,
        is_openai: bool,
    ) -> PinnedCallPricing {
        if let Some(pinned) = self.cost.pinned_call_pricing.clone() {
            return pinned;
        }
        let pinned = self.resolve_call_pricing(at, model, is_anthropic, is_openai);
        self.cost.pinned_call_pricing = Some(pinned.clone());
        pinned
    }

    /// Resolve the rate card for `model` as of `at`.
    ///
    /// Hand-written `[pricing.providers]` rules are authoritative *and* time
    /// dependent (peak/off-peak), so they are resolved here, per call, and never
    /// taken from the cross-call memo. Only when no rule claims the model do the
    /// time-independent derived layers answer, and those still go through the
    /// memo below.
    fn resolve_call_pricing(
        &mut self,
        at: SystemTime,
        model: &str,
        is_anthropic: bool,
        is_openai: bool,
    ) -> PinnedCallPricing {
        let source_key = self.billing_source_key(is_anthropic, is_openai);
        match crate::model_pricing::config_call_rates(&source_key, model, at) {
            crate::model_pricing::ConfigCallRates::Priced(card) => {
                return PinnedCallPricing::Priced(ResolvedTokenPricing::from_rate_card(
                    &card,
                    is_anthropic,
                    is_openai,
                ));
            }
            crate::model_pricing::ConfigCallRates::ConfiguredWithoutPrice => {
                crate::logging::warn(&format!(
                    "pricing rule for {source_key}/{model} cannot price this call; \
                     leaving it unpriced instead of billing the generic defaults"
                ));
                return PinnedCallPricing::ConfiguredWithoutPrice;
            }
            crate::model_pricing::ConfigCallRates::Absent => {}
        }

        // No configured rule for this model: price from the derived layers.
        // Nothing here falls back to the generic defaults unless the model is
        // unknown to every source, which is the pre-feature behaviour.
        self.refresh_cached_pricing(model, is_anthropic, is_openai);
        PinnedCallPricing::Priced(ResolvedTokenPricing {
            prompt_price: *self.cost.cached_prompt_price.get_or_insert(15.0),
            completion_price: *self.cost.cached_completion_price.get_or_insert(60.0),
            cache_read_price: self.cost.cached_cache_read_price,
            is_anthropic,
            is_openai,
            currency: self
                .cost
                .cached_price_currency
                .clone()
                .unwrap_or_else(Currency::usd),
        })
    }

    /// The cross-provider activity key the billing path prices through. Also the
    /// identity hand-written `[pricing.providers]` keys are matched against.
    fn billing_source_key(&self, is_anthropic: bool, is_openai: bool) -> String {
        if is_anthropic {
            "claude:api-key".to_string()
        } else if is_openai {
            "openai:api-key".to_string()
        } else {
            use crate::tui::TuiState;
            let label = <Self as TuiState>::provider_name(self);
            let runtime = active_runtime_provider_key();
            crate::provider_activity::source_key_for_provider_label(&label, runtime.as_deref())
        }
    }

    /// Resolve and cache per-model pricing for the active provider. Uses the
    /// unified resolver (curated static tables, then the OpenRouter caches,
    /// then the live models.dev catalog) so any metered provider gets real
    /// per-model prices instead of the generic defaults. Honors the active
    /// service tier (`/fast on` priority, OpenAI flex), which changes
    /// per-token rates on premium models. Re-resolves when the model or tier
    /// changes.
    ///
    /// The pricing generation is part of the key too. These rates come from
    /// sources a `[pricing]` edit cannot change directly, but a hand-edited
    /// section *is* a statement about what prices are in force: once it lands,
    /// nothing memoized from before it should still be served (F18). Without
    /// the generation the memo would keep handing back what it resolved before
    /// the edit until the model or tier happened to change.
    ///
    /// Only the derived layers are memoized here: hand-written config cards are
    /// resolved separately, at each call's own instant, so this memo can never
    /// hand back a stale tariff.
    fn refresh_cached_pricing(&mut self, model: &str, is_anthropic: bool, is_openai: bool) {
        let service_tier = self.active_service_tier_for_pricing();
        let pricing_generation = crate::model_pricing::pricing_generation();
        // Tier and pricing generation are both part of the memo key so toggling
        // `/fast on` re-prices, and so does a hand-edited `[pricing]` section.
        let price_key = match service_tier.as_deref() {
            Some(tier) => format!("{model}|{tier}|{pricing_generation}"),
            None => format!("{model}|{pricing_generation}"),
        };
        if self.cost.cached_price_model.as_deref() == Some(price_key.as_str()) {
            return;
        }

        let per_mtok = |micros: Option<u64>| micros.map(|m| m as f32 / 1_000_000.0);
        let source_key = self.billing_source_key(is_anthropic, is_openai);
        let estimate = crate::provider::pricing::derived_pricing_for_source(
            &source_key,
            model,
            service_tier.as_deref(),
        );

        if let Some(estimate) = estimate {
            self.cost.cached_prompt_price = per_mtok(estimate.input_price_per_mtok_micros);
            self.cost.cached_completion_price = per_mtok(estimate.output_price_per_mtok_micros);
            self.cost.cached_cache_read_price = per_mtok(estimate.cache_read_price_per_mtok_micros);
            self.cost.cached_price_currency = Some(estimate.currency);
            self.cost.cached_price_model = Some(price_key);
            return;
        }

        // Unknown model/provider: clear any prices cached for a previous model
        // so the generic defaults apply instead of another model's rates, and
        // do NOT memoize the miss. The models.dev catalog refreshes in the
        // background, so a later call can succeed (e.g. first run with an empty
        // pricing cache); the retry is a cheap in-memory lookup per API call.
        if self.cost.cached_price_model.is_some() {
            self.cost.cached_prompt_price = None;
            self.cost.cached_completion_price = None;
            self.cost.cached_cache_read_price = None;
            self.cost.cached_price_currency = None;
            self.cost.cached_price_model = None;
        }
    }

    /// Active service tier for pricing purposes: the server-reported tier for
    /// remote sessions, the local provider's tier otherwise. `None` means the
    /// standard tier.
    fn active_service_tier_for_pricing(&self) -> Option<String> {
        if self.is_remote {
            self.remote_service_tier
                .as_deref()
                .map(str::trim)
                .filter(|tier| !tier.is_empty())
                .map(str::to_string)
        } else {
            self.provider.service_tier()
        }
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

#[cfg(test)]
mod tests {
    use super::{Currency, ResolvedTokenPricing, remote_provider_is_inherently_billed};

    #[test]
    fn remote_billing_recognizes_deepseek_display_name() {
        assert!(remote_provider_is_inherently_billed("DeepSeek"));
    }

    #[test]
    fn cost_for_usage_carries_the_currency_of_its_rates() {
        // F1: the amount and its currency travel together, so a caller never has
        // to assume the provider's currency.
        let pricing = ResolvedTokenPricing {
            prompt_price: 1.0,
            completion_price: 2.0,
            cache_read_price: None,
            is_anthropic: false,
            is_openai: false,
            currency: Currency::new("CNY"),
        };
        let (cost, currency) = pricing.cost_for_usage(1_000_000, 1_000_000, 0, 0);
        assert_eq!(currency.as_str(), "CNY");
        assert!((cost - 3.0).abs() < 1e-6, "1M in at ¥1 + 1M out at ¥2");
    }

    #[test]
    fn remote_billing_does_not_meter_no_auth_compatible_profiles() {
        for provider_name in ["LM Studio", "Ollama"] {
            assert!(
                !remote_provider_is_inherently_billed(provider_name),
                "{provider_name} should not be billed per token"
            );
        }
    }
}

#[cfg(test)]
mod cache_cost_tests {
    use super::{Currency, ResolvedTokenPricing};

    #[test]
    fn openai_cache_writes_replace_uncached_input_cost() {
        let pricing = ResolvedTokenPricing {
            prompt_price: 10.0,
            completion_price: 40.0,
            cache_read_price: Some(1.0),
            is_anthropic: false,
            is_openai: true,
            currency: Currency::usd(),
        };
        // 10K total = 2K ordinary + 6K cache reads + 2K cache writes.
        let (cost, _) = pricing.cost_for_usage(10_000, 100, 6_000, 2_000);
        let expected = (2_000.0 * 10.0 + 6_000.0 + 2_000.0 * 12.5 + 100.0 * 40.0) / 1_000_000.0;
        assert!((cost - expected).abs() < 0.000001, "{cost} != {expected}");
        assert!((pricing.cost_for_usage(10_000, 0, 6_000, 0).0 - 0.046).abs() < 0.000001);
    }
}
