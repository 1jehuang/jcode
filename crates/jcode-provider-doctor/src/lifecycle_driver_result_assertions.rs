use super::*;

impl AuthLifecycleResult {
    pub(crate) fn assert_success(&self, spec: &AuthLifecycleSpec) {
        let transcript = self.transcript_text();
        assert!(self.catalog_report.ok(), "{}", self.failure_report(spec));
        assert_eq!(
            self.activation.provider_id.as_deref(),
            Some(spec.provider_id)
        );
        assert_eq!(
            self.activation.provider_label.as_deref(),
            Some(spec.provider_label)
        );
        assert_eq!(
            self.activation.expected_runtime.as_deref(),
            Some("openai-compatible")
        );
        assert_eq!(
            self.activation.expected_catalog_namespace.as_deref(),
            Some(spec.provider_id)
        );
        assert!(
            transcript.contains(&format!("{} catalog changed", spec.provider_label)),
            "{}",
            self.failure_report(spec)
        );
        // Anti-confusion guard: the transcript must never attribute the new
        // access to a different provider. Skip the marker matching the provider
        // under test: OpenRouter/OpenAI-labelled compat profiles legitimately
        // produce their own catalog-changed line.
        for other_label in ["OpenAI", "OpenRouter"] {
            if spec.provider_label == other_label {
                continue;
            }
            assert!(
                !transcript.contains(&format!("{other_label} catalog changed")),
                "{}",
                self.failure_report(spec)
            );
        }
        self.assert_transcript_order(spec);
        for forbidden in [
            "Auth Model Catalog Warning",
            "did not switch models",
            "contained no selectable",
            "Login: failed",
            "failed",
            "Unable to sign in",
            "Saved the API key and fetched the model catalog, but",
        ] {
            assert!(
                !transcript.contains(forbidden),
                "happy auth lifecycle transcript contained forbidden degraded-success marker `{forbidden}`:\n{}",
                self.failure_report(spec)
            );
        }
        assert!(
            !self.picker.provider_entries.is_empty(),
            "{}",
            self.failure_report(spec)
        );
        assert!(
            self.picker
                .selected_model
                .as_ref()
                .is_some_and(|selected| self
                    .picker
                    .provider_entries
                    .iter()
                    .any(|entry| entry == selected)),
            "{}",
            self.failure_report(spec)
        );
        assert!(
            self.picker.switch_target.is_some(),
            "{}",
            self.failure_report(spec)
        );
        assert!(
            self.picker
                .switch_request
                .as_ref()
                .is_some_and(|request| request.starts_with(&format!("{}:", spec.provider_id))),
            "{}",
            self.failure_report(spec)
        );
        assert!(
            self.picker
                .switch_route_api_method
                .as_deref()
                .is_some_and(|api_method| api_method
                    .eq_ignore_ascii_case(&format!("openai-compatible:{}", spec.provider_id))
                    || api_method.eq_ignore_ascii_case(spec.provider_id)),
            "{}",
            self.failure_report(spec)
        );
        let matching_routes = self
            .catalog_routes
            .iter()
            .filter(|route| route.available && route_matches_spec(route, spec))
            .collect::<Vec<_>>();
        assert!(
            matching_routes.iter().all(|route| spec
                .catalog_models_after_auth
                .iter()
                .any(|model| model == &route.model)),
            "happy auth lifecycle advertised provider routes that were not returned by the live catalog:\n{}",
            self.failure_report(spec)
        );
        assert!(
            self.picker.provider_entries.iter().all(|entry| spec
                .catalog_models_after_auth
                .iter()
                .any(|model| model == entry)),
            "happy auth lifecycle picker included models that were not returned by the live catalog:\n{}",
            self.failure_report(spec)
        );
        assert!(
            matching_routes
                .iter()
                .all(|route| route.detail.contains("live-catalog")),
            "happy auth lifecycle must be backed by live/provider catalog routes, not static fallback routes:\n{}",
            self.failure_report(spec)
        );
        assert!(
            matching_routes.iter().all(|route| !route
                .detail
                .to_ascii_lowercase()
                .contains("static fallback")),
            "happy auth lifecycle accepted a static fallback route:\n{}",
            self.failure_report(spec)
        );
    }

    fn assert_transcript_order(&self, spec: &AuthLifecycleSpec) {
        let transcript = self.transcript_text();
        let saved_or_detected = if spec.auth_path.shows_paste_prompt() {
            format!("**{} API key saved.**", spec.provider_label)
        } else {
            format!("**{} credentials detected.**", spec.provider_label)
        };
        let markers = [saved_or_detected.as_str(), "**Model ready:**"];
        let mut previous = None;
        for marker in markers {
            let first = transcript.find(marker).unwrap_or_else(|| {
                panic!(
                    "happy auth lifecycle transcript is missing `{marker}`:\n{}",
                    self.failure_report(spec)
                )
            });
            let last = transcript.rfind(marker).expect("marker found above");
            assert_eq!(
                first,
                last,
                "happy auth lifecycle transcript contained duplicate `{marker}`:\n{}",
                self.failure_report(spec)
            );
            if let Some(previous) = previous {
                assert!(
                    previous < first,
                    "happy auth lifecycle transcript marker `{marker}` was out of order:\n{}",
                    self.failure_report(spec)
                );
            }
            previous = Some(first);
        }
    }

    pub(crate) fn transcript_text(&self) -> String {
        self.transcript.join("\n\n")
    }

    pub(crate) fn failure_report(&self, spec: &AuthLifecycleSpec) -> String {
        let warning = self
            .catalog_report
            .warning_message()
            .unwrap_or_else(|| "none".to_string());
        let route_sample = self
            .catalog_routes
            .iter()
            .take(8)
            .map(|route| {
                format!(
                    "{} via {} provider={} available={}",
                    route.model, route.api_method, route.provider, route.available
                )
            })
            .collect::<Vec<_>>()
            .join("\n  ");
        format!(
            "auth lifecycle failed for {} via {:?}\ncredential: {:?}\nactivation: {:?}\ncatalog invariant: {:?}\nwarning: {}\npicker: {:?}\nroutes:\n  {}\ntranscript:\n{}",
            spec.provider_label,
            spec.auth_path,
            self.credential_location,
            self.activation,
            self.catalog_report,
            warning,
            self.picker,
            route_sample,
            self.transcript_text()
        )
    }
}
