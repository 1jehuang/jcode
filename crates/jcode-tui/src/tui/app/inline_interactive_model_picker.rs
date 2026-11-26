//! Interactive model picker setup.

use super::*;

impl App {
    pub(super) fn open_model_picker_inner(&mut self, preserve_input: bool) {
        let picker_started = std::time::Instant::now();
        const RECENT_AUTH_BOOST_TTL: std::time::Duration = std::time::Duration::from_secs(5 * 60);
        if self
            .recent_authenticated_provider
            .as_ref()
            .map(|(_, at)| at.elapsed() > RECENT_AUTH_BOOST_TTL)
            .unwrap_or(false)
        {
            self.recent_authenticated_provider = None;
            self.invalidate_model_picker_cache();
        }

        // During remote startup the authoritative session catalog has not
        // arrived yet. Do not make an old persisted catalog look current just
        // because the user opened `/model` quickly after spawning the client.
        let awaiting_initial_remote_catalog = self.is_remote
            && self.remote_startup_phase.is_some()
            && self.remote_model_options.is_empty()
            && self.remote_available_entries.is_empty();

        if self.is_remote
            && !awaiting_initial_remote_catalog
            && self.remote_model_options.is_empty()
        {
            self.hydrate_remote_model_catalog_cache();
        }

        let current_model = if self.is_remote {
            self.remote_provider_model
                .clone()
                .unwrap_or_else(|| "unknown".to_string())
        } else {
            self.provider.model().to_string()
        };

        if awaiting_initial_remote_catalog {
            self.open_loading_model_picker(&current_model);
            return;
        }

        // Never present the old catalog as authoritative immediately after a
        // login/import. Local mode clears this when the provider's synchronous
        // auth activation finishes; remote mode clears it when the server sends
        // the final catalog activity after all provider work completes.
        if self.auth_catalog_refresh_pending {
            self.open_loading_model_picker(&current_model);
            return;
        }

        // UI preferences may be local, but provider defaults must not be.
        let remote_defaults = crate::config::Config::default();
        let config = if crate::tui::is_ssh_remote() {
            &remote_defaults
        } else {
            crate::config::config()
        };
        let config_default_model = config.provider.default_model.clone();
        let config_default_provider = config.provider.default_provider.clone();

        let current_effort = if self.is_remote {
            self.remote_reasoning_effort.clone()
        } else {
            self.provider.reasoning_effort()
        };
        let available_efforts = if self.is_remote {
            inferred_reasoning_efforts(
                self.remote_provider_name.as_deref(),
                self.remote_provider_model.as_deref(),
            )
        } else {
            self.provider.available_efforts()
        };

        let cache_signature = self.model_picker_cache_signature(
            &current_model,
            config_default_model.clone(),
            config_default_provider.clone(),
            current_effort.clone(),
            &available_efforts,
        );
        if self.open_cached_model_picker_if_fresh(&cache_signature, picker_started, preserve_input)
        {
            return;
        }

        if !self.is_remote && !crate::perf::tui_policy().simplified_model_picker {
            let routes_started = std::time::Instant::now();
            let routes = self.simplified_model_routes_for_picker(&current_model);
            let routes_ms = routes_started.elapsed().as_millis();
            self.open_model_picker_with_routes(
                cache_signature.clone(),
                picker_started,
                routes,
                routes_ms,
                preserve_input,
                false,
            );
            if self.inline_interactive_state.is_some() {
                self.set_status_notice("Updating model routes…");
            } else {
                self.open_loading_model_picker(&current_model);
            }
            self.start_model_picker_route_load(cache_signature, picker_started);
            return;
        }

        let routes_started = std::time::Instant::now();
        let routes: Vec<crate::provider::ModelRoute> = if self.is_remote {
            // Small catalogs build in well under a frame, so keep them
            // synchronous: the first paint already has the real rows.
            const SYNC_REMOTE_ROUTES_MAX: usize = 256;
            if !self.remote_model_options.is_empty()
                && self.remote_model_options.len() <= SYNC_REMOTE_ROUTES_MAX
            {
                let mut routes = std::mem::take(&mut self.remote_model_options);
                self.extend_remote_routes_for_uncovered_models(&mut routes);
                let routes_ms = routes_started.elapsed().as_millis();
                self.remote_model_options = self.open_model_picker_with_routes(
                    cache_signature,
                    picker_started,
                    routes,
                    routes_ms,
                    preserve_input,
                    true,
                );
                let fresh_signature = self.model_picker_cache_signature(
                    &current_model,
                    config_default_model,
                    config_default_provider,
                    current_effort,
                    &available_efforts,
                );
                if let Some(cache) = self.model_picker_cache.as_mut() {
                    cache.signature = fresh_signature;
                }
                return;
            }
            if !self.remote_model_options.is_empty() {
                // Normalizing ~2k routes and expanding them into ~10k effort
                // rows takes hundreds of ms. Never do that on the UI thread:
                // show the picker immediately and build in the background.
                // `poll_model_picker_load` stores the normalized routes back
                // and re-signs the cache so the next open is instant.
                let refreshing_open_picker = self
                    .inline_interactive_state
                    .as_ref()
                    .is_some_and(picker_is_runtime_model_picker);
                if refreshing_open_picker {
                    self.set_status_notice("Updating model list…");
                } else {
                    self.open_loading_model_picker(&current_model);
                    if !preserve_input {
                        self.input.clear();
                        self.cursor_pos = 0;
                    }
                }
                let routes = self.remote_model_options.clone();
                let remote_provider_name = self.remote_provider_name.clone();
                let remote_available_entries = self.remote_available_entries.clone();
                self.start_model_picker_route_load_inner(
                    cache_signature,
                    picker_started,
                    true,
                    move || {
                        let mut routes = routes;
                        Self::extend_remote_routes_for_uncovered_models_static(
                            remote_provider_name.as_deref(),
                            &remote_available_entries,
                            &mut routes,
                        );
                        routes
                    },
                );
                return;
            }
            // Names-only remote catalog: synthesize properly classified
            // provider routes (Comtegra/Copilot/Bedrock/Gemini/OpenRouter/…)
            // rather than a generic "remote-catalog" placeholder. The full
            // fallback reads per-model disk caches and auth state, which can
            // take seconds on a large catalog, so for big catalogs open
            // instantly with lightweight names-only routes and upgrade in the
            // background. Small catalogs stay synchronous so the first paint
            // already has effort-expanded, provider-classified rows.
            const SYNC_REMOTE_FALLBACK_MAX_MODELS: usize = 64;
            if crate::tui::is_ssh_remote() {
                self.build_remote_model_routes_lightweight_fallback(&current_model)
            } else if self.remote_available_entries.len() <= SYNC_REMOTE_FALLBACK_MAX_MODELS {
                self.build_remote_model_routes_fallback()
            } else {
                let routes = self.build_remote_model_routes_lightweight_fallback(&current_model);
                let routes_ms = routes_started.elapsed().as_millis();
                self.open_model_picker_with_routes(
                    cache_signature.clone(),
                    picker_started,
                    routes,
                    routes_ms,
                    preserve_input,
                    false,
                );
                if self.inline_interactive_state.is_some() {
                    self.set_status_notice("Updating model routes…");
                } else {
                    self.open_loading_model_picker(&current_model);
                }
                let remote_provider_name = self.remote_provider_name.clone();
                let remote_available_entries = self.remote_available_entries.clone();
                self.start_model_picker_route_load_with(
                    cache_signature,
                    picker_started,
                    move || {
                        let mut routes = crate::provider::remote_model_routes_fallback(
                            remote_provider_name.as_deref(),
                            &remote_available_entries,
                        );
                        Self::extend_remote_routes_for_uncovered_models_static(
                            remote_provider_name.as_deref(),
                            &remote_available_entries,
                            &mut routes,
                        );
                        routes
                    },
                );
                return;
            }
        } else {
            self.simplified_model_routes_for_picker(&current_model)
        };
        let routes_ms = routes_started.elapsed().as_millis();

        let _ = self.open_model_picker_with_routes(
            cache_signature,
            picker_started,
            routes,
            routes_ms,
            preserve_input,
            true,
        );
    }
}
