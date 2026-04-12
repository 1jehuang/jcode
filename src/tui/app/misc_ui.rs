use super::*;

/// Update cost calculation based on token usage (for API-key providers)
impl App {
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
    }

    pub(super) fn open_usage_overlay_loading(&mut self) {
        self.inline_view_state = None;
        self.inline_interactive_state = None;
        self.usage_overlay = Some(std::cell::RefCell::new(
            crate::tui::usage_overlay::UsageOverlay::loading(),
        ));
        self.set_status_notice("Usage → refreshing");
    }

    pub(super) fn open_usage_inline_loading(&mut self) {
        self.usage_overlay = None;
        self.inline_interactive_state = None;
        self.inline_view_state = Some(crate::tui::InlineViewState {
            title: "USAGE".to_string(),
            status: Some("refreshing".to_string()),
            lines: vec![
                "Refreshing usage".to_string(),
                "Fetching usage limits from all connected providers...".to_string(),
                "".to_string(),
                "This inline view will update automatically when the usage report returns."
                    .to_string(),
            ],
        });
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
            let results = crate::usage::fetch_all_provider_usage().await;
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

    pub(super) fn handle_usage_overlay_key(
        &mut self,
        code: KeyCode,
        modifiers: KeyModifiers,
    ) -> Result<()> {
        use crate::tui::usage_overlay::OverlayAction;

        let Some(overlay_cell) = self.usage_overlay.as_ref() else {
            return Ok(());
        };

        let action = {
            let mut overlay = overlay_cell.borrow_mut();
            overlay.handle_overlay_key(code, modifiers)?
        };

        if matches!(action, OverlayAction::Close) {
            self.usage_overlay = None;
            self.set_status_notice("Usage closed");
        }

        Ok(())
    }

    pub(super) fn update_cost_impl(&mut self) {
        let provider_name = self.provider.name().to_lowercase();

        // Only calculate cost for API-key providers
        if !provider_name.contains("openrouter")
            && !provider_name.contains("anthropic")
            && !provider_name.contains("openai")
        {
            return;
        }

        // For OAuth providers, cost is already tracked in subscription
        let is_oauth = (provider_name.contains("anthropic") || provider_name.contains("claude"))
            && std::env::var("ANTHROPIC_API_KEY").is_err();
        if is_oauth {
            return;
        }

        // Default pricing (will be cached after first turn)
        let prompt_price = *self.cached_prompt_price.get_or_insert(15.0); // $15/1M tokens default
        let completion_price = *self.cached_completion_price.get_or_insert(60.0); // $60/1M tokens default

        // Calculate cost for this turn
        let prompt_cost = (self.streaming_input_tokens as f32 * prompt_price) / 1_000_000.0;
        let completion_cost =
            (self.streaming_output_tokens as f32 * completion_price) / 1_000_000.0;
        self.total_cost += prompt_cost + completion_cost;
    }

    pub(super) fn compute_streaming_tps(&self) -> Option<f32> {
        let mut elapsed = self.streaming_tps_elapsed;
        let total_tokens = if self.streaming_total_output_tokens > 0 {
            self.streaming_total_output_tokens
        } else {
            self.streaming_output_tokens
        };
        if let Some(start) = self.streaming_tps_start {
            elapsed += start.elapsed();
        }
        let elapsed_secs = elapsed.as_secs_f32();
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
}
