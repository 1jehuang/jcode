//! Widget usage snapshots.

use super::*;

impl App {
    pub(super) fn widget_usage_info_inner(
        &self,
        route: WidgetRouteInfo,
        auth_method: crate::tui::info_widget::AuthMethod,
    ) -> Option<crate::tui::info_widget::UsageInfo> {
        let output_tps = if matches!(self.status, ProcessingStatus::Streaming) {
            self.compute_streaming_tps()
        } else {
            None
        };

        // On a resumed session, `token_accounting.total_*` is reset to 0 and the
        // prior usage lives in `remote_total_tokens` (restored from history). Add
        // them so the widget's "in + out" reflects the whole session, mirroring
        // the `/cache` stats path, rather than only tokens seen since resume.
        let (display_input_tokens, display_output_tokens) =
            if let Some((hist_in, hist_out)) = self.remote_total_tokens {
                (
                    hist_in.saturating_add(self.token_accounting.total_input_tokens),
                    hist_out.saturating_add(self.token_accounting.total_output_tokens),
                )
            } else {
                (
                    self.token_accounting.total_input_tokens,
                    self.token_accounting.total_output_tokens,
                )
            };

        let cost_based_usage = || crate::tui::info_widget::UsageInfo {
            provider: crate::tui::info_widget::UsageProvider::CostBased,
            primary_limit_label: None,
            five_hour: 0.0,
            five_hour_resets_at: None,
            secondary_limit_label: None,
            seven_day: 0.0,
            seven_day_resets_at: None,
            spark: None,
            spark_resets_at: None,
            total_cost: self.cost.total_cost,
            input_tokens: display_input_tokens,
            output_tokens: display_output_tokens,
            cache_read_tokens: self.streaming.streaming_cache_read_tokens,
            cache_write_tokens: self.streaming.streaming_cache_creation_tokens,
            output_tps,
            limit_reached: false,
            available: true,
            account_hint: None,
        };

        match route.provider {
            WidgetProviderKind::Copilot => Some(crate::tui::info_widget::UsageInfo {
                provider: crate::tui::info_widget::UsageProvider::Copilot,
                primary_limit_label: None,
                five_hour: 0.0,
                five_hour_resets_at: None,
                secondary_limit_label: None,
                seven_day: 0.0,
                seven_day_resets_at: None,
                spark: None,
                spark_resets_at: None,
                total_cost: 0.0,
                input_tokens: display_input_tokens,
                output_tokens: display_output_tokens,
                cache_read_tokens: None,
                cache_write_tokens: None,
                output_tps,
                limit_reached: false,
                available: display_input_tokens > 0 || display_output_tokens > 0,
                account_hint: None,
            }),
            WidgetProviderKind::Anthropic => {
                match auth_method {
                    crate::tui::info_widget::AuthMethod::AnthropicApiKey => {
                        return Some(cost_based_usage());
                    }
                    crate::tui::info_widget::AuthMethod::AnthropicOAuth => {}
                    _ => return None,
                }

                if route.is_remote {
                    // Daemon snapshots are authoritative for the bound account.
                    // Never call get_sync here: even a cache miss can start a
                    // local HTTP refresh with this machine's credentials.
                    return Some(remote_oauth_widget_usage_for_route(
                        crate::tui::info_widget::UsageProvider::Anthropic,
                        self.session.route_api_method.as_deref(),
                        self.remote_provider_oauth_account_label.as_deref(),
                        false,
                        output_tps,
                        |label| {
                            crate::usage::cached_account_usage_status_snapshot(
                                crate::usage::MultiAccountProviderKind::Anthropic,
                                label,
                            )
                        },
                    ));
                }

                let usage = crate::usage::get_sync();
                Some(crate::tui::info_widget::UsageInfo {
                    provider: crate::tui::info_widget::UsageProvider::Anthropic,
                    primary_limit_label: Some("5-hour".to_string()),
                    five_hour: usage.five_hour,
                    five_hour_resets_at: usage.five_hour_resets_at.clone(),
                    secondary_limit_label: Some("Weekly".to_string()),
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
                    limit_reached: false,
                    available: usage.last_error.is_none(),
                    account_hint: None,
                })
            }
            WidgetProviderKind::OpenAI => {
                match auth_method {
                    crate::tui::info_widget::AuthMethod::OpenAIApiKey => {
                        return Some(cost_based_usage());
                    }
                    crate::tui::info_widget::AuthMethod::OpenAIOAuth => {}
                    _ => return None,
                }

                if route.is_remote {
                    // The header reads the daemon's account cache by route label.
                    // An email lookup here would instead probe this machine's
                    // credentials and can show a different account's windows.
                    return Some(remote_oauth_widget_usage_for_route(
                        crate::tui::info_widget::UsageProvider::OpenAI,
                        self.session.route_api_method.as_deref(),
                        self.remote_provider_oauth_account_label.as_deref(),
                        self.usage_reset.openai_quota_rejected,
                        output_tps,
                        |label| {
                            crate::usage::cached_account_usage_status_snapshot(
                                crate::usage::MultiAccountProviderKind::OpenAI,
                                label,
                            )
                        },
                    ));
                }

                // The selected route is session-bound, unlike the global
                // active account. Never borrow another login's quota headroom.
                let openai_usage = self
                    .openai_widget_account_email()
                    .map(|email| crate::usage::get_openai_usage_for_email_sync(&email))
                    .unwrap_or_default();
                Some(crate::tui::info_widget::UsageInfo {
                    provider: crate::tui::info_widget::UsageProvider::OpenAI,
                    primary_limit_label: openai_usage
                        .five_hour
                        .as_ref()
                        .map(|window| window.name.trim_end_matches(" window").to_string()),
                    five_hour: openai_usage
                        .five_hour
                        .as_ref()
                        .map(|w| w.usage_ratio)
                        .unwrap_or(0.0),
                    five_hour_resets_at: openai_usage
                        .five_hour
                        .as_ref()
                        .and_then(|w| w.resets_at.clone()),
                    secondary_limit_label: openai_usage
                        .seven_day
                        .as_ref()
                        .map(|window| window.name.trim_end_matches(" window").to_string()),
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
                    limit_reached: openai_usage.hard_limit_reached
                        || self.usage_reset.openai_quota_rejected,
                    available: openai_usage.has_limits() && openai_usage.last_error.is_none(),
                    account_hint: None,
                })
            }
            WidgetProviderKind::Gemini => None,
            WidgetProviderKind::OpenRouter => {
                if route.is_remote {
                    return Some(cost_based_usage());
                }

                let runtime_provider = active_runtime_provider_key();
                let transport_state =
                    crate::provider::openrouter::OpenRouterTransportState::from_current_env(
                        runtime_provider.as_deref(),
                    );
                if transport_state.accrues_user_api_key_cost() {
                    Some(cost_based_usage())
                } else {
                    None
                }
            }
            WidgetProviderKind::OpenCode | WidgetProviderKind::CostBasedApiKey => {
                Some(cost_based_usage())
            }
            WidgetProviderKind::Unknown => None,
        }
    }
}
